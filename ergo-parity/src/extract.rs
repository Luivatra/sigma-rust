//! `extract`: turn blocks from a node into fixtures.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{anyhow, bail, Context, Result};
use ergo_chain_types::Header;
use ergo_lib::chain::transaction::Transaction;
use ergotree_ir::chain::ergo_box::ErgoBox;
use ergotree_ir::serialization::SigmaSerializable;
use rayon::prelude::*;
use serde_json::Value;
use sigma_ser::ScorexSerializable;
use sigma_util::hash::blake2b256_hash;

use crate::node::Node;
use crate::params::{epoch_start, parse_parameters};
use crate::schema::{Fixture, Reject, SCHEMA_VERSION};

pub struct ExtractArgs {
    pub node: String,
    pub from: u32,
    pub to: u32,
    pub out: PathBuf,
    pub rejects: Option<PathBuf>,
    pub concurrency: usize,
    pub cache: Option<PathBuf>,
}

fn hex(b: &[u8]) -> String {
    base16::encode_lower(b)
}

fn unhex(s: &str) -> Result<Vec<u8>> {
    base16::decode(s.as_bytes()).map_err(|e| anyhow!("bad hex {s:?}: {e}"))
}

#[derive(Debug)]
struct Epoch {
    parameters: std::collections::BTreeMap<u8, i32>,
    fields: Vec<[String; 2]>,
}

type EpochCell = Arc<OnceLock<Result<Arc<Epoch>, String>>>;

struct Ctx {
    node: Node,
    epochs: Mutex<HashMap<u32, EpochCell>>,
}

/// Header from node JSON; its id must equal the hash of the header's serialization.
fn checked_header(v: &Value) -> Result<(Header, Vec<u8>)> {
    let claimed = v
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("header JSON has no id"))?
        .to_string();
    let header: Header = serde_json::from_value(v.clone()).context("parsing header JSON")?;
    let bytes = header
        .scorex_serialize_bytes()
        .map_err(|e| anyhow!("serializing header: {e}"))?;
    let id = hex(&*blake2b256_hash(&bytes));
    if id != claimed {
        bail!(
            "header id mismatch at height {}: node {claimed}, rebuilt {id}",
            header.height
        );
    }
    // Round-trip through the parser: the fixture consumers parse these bytes.
    let parsed =
        Header::scorex_parse_bytes(&bytes).map_err(|e| anyhow!("re-parsing header: {e}"))?;
    if hex(parsed.id.0.as_ref()) != claimed {
        bail!("re-parsed header id differs at height {}", header.height);
    }
    Ok((header, bytes))
}

fn checked_box(node: &Node, id: &str) -> Result<Vec<u8>> {
    let mut v: Value = node.get_json(&format!("/blockchain/box/byId/{id}"))?;
    // The index adds fields (spentTransactionId, address, ...) that are not part of the box.
    if let Some(o) = v.as_object_mut() {
        const KEEP: [&str; 8] = [
            "boxId",
            "value",
            "ergoTree",
            "assets",
            "creationHeight",
            "additionalRegisters",
            "transactionId",
            "index",
        ];
        o.retain(|k, _| KEEP.contains(&k.as_str()));
    }
    let b: ErgoBox = serde_json::from_value(v).context("parsing box JSON")?;
    let bytes = b
        .sigma_serialize_bytes()
        .map_err(|e| anyhow!("serializing box: {e}"))?;
    let rebuilt = hex(&*blake2b256_hash(&bytes));
    if rebuilt != id {
        bail!("box id mismatch: requested {id}, rebuilt {rebuilt}");
    }
    Ok(bytes)
}

impl Ctx {
    fn block_id_at(&self, height: u32) -> Result<String> {
        let ids: Vec<String> = self.node.get_json(&format!("/blocks/at/{height}"))?;
        ids.into_iter()
            .next()
            .ok_or_else(|| anyhow!("node has no block at height {height}"))
    }

    fn epoch(&self, height: u32) -> Result<Arc<Epoch>> {
        let start = epoch_start(height);
        let cell = self
            .epochs
            .lock()
            .unwrap()
            .entry(start)
            .or_default()
            .clone();
        cell.get_or_init(|| {
            self.load_epoch(start)
                .map(Arc::new)
                .map_err(|e| format!("{e:#}"))
        })
        .clone()
        .map_err(|e| anyhow!("epoch start {start}: {e}"))
    }

    fn load_epoch(&self, start: u32) -> Result<Epoch> {
        if start == 0 {
            bail!("no epoch-start block (height 0)");
        }
        let id = self.block_id_at(start)?;
        // The node's `/blocks/{id}/extension` returns the whole block, so read the block itself.
        let block: Value = self.node.get_json(&format!("/blocks/{id}"))?;
        let ext = block
            .get("extension")
            .ok_or_else(|| anyhow!("block has no extension"))?;
        let raw = ext
            .get("fields")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("extension has no fields array"))?;
        let mut fields = Vec::new();
        let mut decoded = Vec::new();
        for f in raw {
            let k = f
                .get(0)
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("bad extension field"))?;
            let v = f
                .get(1)
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("bad extension field"))?;
            decoded.push((unhex(k)?, unhex(v)?));
            fields.push([k.to_string(), v.to_string()]);
        }
        Ok(Epoch {
            parameters: parse_parameters(&decoded)?,
            fields,
        })
    }

    /// The (up to) ten headers before `header`, newest first, verified to chain to it.
    fn previous_headers(&self, header: &Header) -> Result<Vec<Vec<u8>>> {
        let h = header.height;
        if h <= 1 {
            return Ok(vec![]);
        }
        let from = h.saturating_sub(10).max(1);
        // `fromHeight` is exclusive on the node: the slice starts at fromHeight + 1.
        let slice: Vec<Value> = self.node.get_json(&format!(
            "/blocks/chainSlice?fromHeight={}&toHeight={}",
            from - 1,
            h - 1
        ))?;
        let mut parsed = Vec::new();
        for v in &slice {
            parsed.push(checked_header(v)?);
        }
        parsed.sort_by_key(|(hd, _)| std::cmp::Reverse(hd.height));
        let expect = (h - from) as usize;
        if parsed.len() != expect {
            bail!(
                "chainSlice {from}..{} returned {} headers, expected {expect}",
                h - 1,
                parsed.len()
            );
        }
        let mut child_parent = header.parent_id;
        let mut expected_height = h - 1;
        for (hd, _) in &parsed {
            if hd.height != expected_height || hd.id != child_parent {
                bail!("header window does not chain at height {expected_height}");
            }
            child_parent = hd.parent_id;
            expected_height -= 1;
        }
        Ok(parsed.into_iter().map(|(_, b)| b).collect())
    }

    fn block(&self, height: u32) -> (Vec<Fixture>, Vec<Reject>) {
        let mut fixtures = Vec::new();
        let mut rejects = Vec::new();
        let block_reject = |block_id: Option<String>, reason: &str, e: anyhow::Error| Reject {
            height,
            block_id,
            tx_index: None,
            tx_id: None,
            reason: reason.into(),
            detail: format!("{e:#}"),
        };
        let block_id = match self.block_id_at(height) {
            Ok(id) => id,
            Err(e) => return (fixtures, vec![block_reject(None, "block_lookup", e)]),
        };
        let block: Value = match self.node.get_json(&format!("/blocks/{block_id}")) {
            Ok(b) => b,
            Err(e) => {
                return (
                    fixtures,
                    vec![block_reject(Some(block_id), "block_fetch", e)],
                )
            }
        };
        let (header, header_bytes) = match block
            .get("header")
            .ok_or_else(|| anyhow!("block has no header"))
            .and_then(checked_header)
        {
            Ok(h) => h,
            Err(e) => return (fixtures, vec![block_reject(Some(block_id), "header_id", e)]),
        };
        if hex(header.id.0.as_ref()) != block_id {
            let e = anyhow!(
                "header id {} != requested block id {block_id}",
                hex(header.id.0.as_ref())
            );
            return (fixtures, vec![block_reject(Some(block_id), "header_id", e)]);
        }
        let epoch = match self.epoch(height) {
            Ok(e) => e,
            Err(e) => {
                return (
                    fixtures,
                    vec![block_reject(Some(block_id), "epoch_parameters", e)],
                )
            }
        };
        let headers = match self.previous_headers(&header) {
            Ok(h) => h,
            Err(e) => {
                return (
                    fixtures,
                    vec![block_reject(Some(block_id), "header_window", e)],
                )
            }
        };
        let txs = block
            .pointer("/blockTransactions/transactions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for (tx_index, txv) in txs.iter().enumerate() {
            let tx_index = tx_index as u32;
            let claimed_id = txv.get("id").and_then(Value::as_str).map(str::to_string);
            let reject = |reason: &str, e: anyhow::Error| Reject {
                height,
                block_id: Some(block_id.clone()),
                tx_index: Some(tx_index),
                tx_id: claimed_id.clone(),
                reason: reason.into(),
                detail: format!("{e:#}"),
            };
            match self.fixture(
                height,
                &block_id,
                tx_index,
                txv,
                &headers,
                &header_bytes,
                &epoch,
            ) {
                Ok(f) => fixtures.push(f),
                Err((reason, e)) => rejects.push(reject(reason, e)),
            }
        }
        (fixtures, rejects)
    }

    #[allow(clippy::too_many_arguments)]
    fn fixture(
        &self,
        height: u32,
        block_id: &str,
        tx_index: u32,
        txv: &Value,
        headers: &[Vec<u8>],
        header_bytes: &[u8],
        epoch: &Epoch,
    ) -> Result<Fixture, (&'static str, anyhow::Error)> {
        // Deserialization recomputes the tx id from the rebuilt bytes and compares it with the
        // JSON's id; the output box ids are checked here.
        let tx: Transaction =
            serde_json::from_value(txv.clone()).map_err(|e| ("tx_id", anyhow!("{e}")))?;
        let claimed = txv.get("id").and_then(Value::as_str).unwrap_or_default();
        let tx_bytes = tx
            .sigma_serialize_bytes()
            .map_err(|e| ("tx_bytes", anyhow!("{e}")))?;
        let signing = tx
            .bytes_to_sign()
            .map_err(|e| ("tx_bytes", anyhow!("{e}")))?;
        let rebuilt_id = hex(&*blake2b256_hash(&signing));
        if rebuilt_id != claimed || hex(tx.id().0.as_ref()) != claimed {
            return Err((
                "tx_id",
                anyhow!("tx id mismatch: node {claimed}, rebuilt {rebuilt_id}"),
            ));
        }
        let outs = txv
            .get("outputs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for (i, (o, jo)) in tx.outputs.iter().zip(outs.iter()).enumerate() {
            let rebuilt = hex(&*blake2b256_hash(
                &o.sigma_serialize_bytes()
                    .map_err(|e| ("box_id", anyhow!("{e}")))?,
            ));
            let claimed = jo.get("boxId").and_then(Value::as_str).unwrap_or_default();
            if rebuilt != claimed {
                return Err((
                    "box_id",
                    anyhow!("output {i} box id mismatch: node {claimed}, rebuilt {rebuilt}"),
                ));
            }
        }
        let inputs = tx
            .inputs
            .iter()
            .map(|i| checked_box(&self.node, &hex(i.box_id.as_ref())))
            .collect::<Result<Vec<_>>>()
            .map_err(|e| ("box_id", e))?;
        let data_inputs = tx
            .data_inputs
            .iter()
            .flat_map(|d| d.iter())
            .map(|d| checked_box(&self.node, &hex(d.box_id.as_ref())))
            .collect::<Result<Vec<_>>>()
            .map_err(|e| ("box_id", e))?;
        Ok(Fixture {
            schema: SCHEMA_VERSION,
            height,
            block_id: block_id.to_string(),
            tx_index,
            tx_id: claimed.to_string(),
            tx: hex(&tx_bytes),
            inputs: inputs.iter().map(|b| hex(b)).collect(),
            data_inputs: data_inputs.iter().map(|b| hex(b)).collect(),
            headers: headers.iter().map(|b| hex(b)).collect(),
            header: hex(header_bytes),
            parameters: epoch.parameters.clone(),
            epoch_extension: epoch.fields.clone(),
            notes: vec!["reencoded".into()],
        })
    }
}

pub fn run(a: ExtractArgs) -> Result<()> {
    let ctx = Ctx {
        node: Node::new(&a.node, a.cache)?,
        epochs: Mutex::default(),
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(a.concurrency.max(1))
        .build()?;
    let mut out = std::io::BufWriter::new(std::fs::File::create(&a.out).context("create --out")?);
    let mut rej_out = match &a.rejects {
        Some(p) => Some(std::io::BufWriter::new(
            std::fs::File::create(p).context("create --rejects")?,
        )),
        None => None,
    };
    let heights: Vec<u32> = (a.from..=a.to).collect();
    let (mut n_fix, mut n_rej) = (0usize, 0usize);
    let mut by_reason: std::collections::BTreeMap<String, usize> = Default::default();
    for chunk in heights.chunks(a.concurrency.max(1) * 4) {
        let results: Vec<_> = pool.install(|| chunk.par_iter().map(|h| ctx.block(*h)).collect());
        for (fixtures, rejects) in results {
            for f in fixtures {
                serde_json::to_writer(&mut out, &f)?;
                out.write_all(b"\n")?;
                n_fix += 1;
            }
            for r in rejects {
                *by_reason.entry(r.reason.clone()).or_default() += 1;
                eprintln!(
                    "reject: height {} tx {:?}: {}: {}",
                    r.height, r.tx_id, r.reason, r.detail
                );
                if let Some(w) = rej_out.as_mut() {
                    serde_json::to_writer(&mut *w, &r)?;
                    w.write_all(b"\n")?;
                }
                n_rej += 1;
            }
        }
        out.flush()?;
        if let Some(w) = rej_out.as_mut() {
            w.flush()?;
        }
        eprintln!(
            "heights up to {}: {n_fix} fixtures, {n_rej} rejects",
            chunk[chunk.len() - 1]
        );
    }
    println!("fixtures written: {n_fix}");
    println!("rejects: {n_rej}");
    for (reason, n) in by_reason {
        println!("  {reason}: {n}");
    }
    Ok(())
}
