//! `eval`: run stateful validation with this workspace's ergo-lib.

use std::io::{BufRead, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use ergo_chain_types::{Header, PreHeader};
use ergo_lib::chain::ergo_state_context::{ErgoStateContext, Headers};
use ergo_lib::chain::parameters::{Parameter, Parameters};
use ergo_lib::chain::transaction::ergo_transaction::{ErgoTransaction, TxValidationError};
use ergo_lib::chain::transaction::Transaction;
use ergo_lib::wallet::tx_context::{InputReport, TransactionContext};
use ergotree_interpreter::eval::EvalError;
use ergotree_interpreter::sigma_protocol::verifier::VerifierError;
use ergotree_ir::chain::ergo_box::ErgoBox;
use ergotree_ir::serialization::SigmaSerializable;
use rayon::prelude::*;
use sigma_ser::ScorexSerializable;

use crate::params::parse_parameters;
use crate::schema::{Fixture, InputResult, ResultRec, SCHEMA_VERSION};

const MAX_ERROR_LEN: usize = 1000;

fn unhex(s: &str) -> Result<Vec<u8>> {
    base16::decode(s.as_bytes()).map_err(|e| anyhow!("bad hex: {e}"))
}

fn runner_version() -> String {
    format!("sigma-rust@{}", env!("ERGO_PARITY_GIT_REV"))
}

fn truncate(mut s: String) -> String {
    if s.len() > MAX_ERROR_LEN {
        let mut cut = MAX_ERROR_LEN;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("...");
    }
    s
}

fn parameter_from_id(id: u8) -> Option<Parameter> {
    Some(match id {
        1 => Parameter::StorageFeeFactor,
        2 => Parameter::MinValuePerByte,
        3 => Parameter::MaxBlockSize,
        4 => Parameter::MaxBlockCost,
        5 => Parameter::TokenAccessCost,
        6 => Parameter::InputCost,
        7 => Parameter::DataInputCost,
        8 => Parameter::OutputCost,
        9 => Parameter::SubblocksPerBlock,
        121 => Parameter::SoftForkVotesCollected,
        122 => Parameter::SoftForkStartingHeight,
        123 => Parameter::BlockVersion,
        _ => return None,
    })
}

/// The fixture's `parameters` must be what `epoch_extension` yields.
fn check_parameters_agree(f: &Fixture) -> Result<()> {
    let fields = f
        .epoch_extension
        .iter()
        .map(|[k, v]| Ok((unhex(k)?, unhex(v)?)))
        .collect::<Result<Vec<_>>>()?;
    let from_ext = parse_parameters(&fields)?;
    if from_ext != f.parameters {
        bail!(
            "parameters disagree with epoch_extension: fixture {:?}, extension {:?}",
            f.parameters,
            from_ext
        );
    }
    Ok(())
}

fn build_parameters(f: &Fixture) -> Result<Parameters> {
    for id in [1u8, 2, 3, 4, 5, 6, 7, 8, 123] {
        if !f.parameters.contains_key(&id) {
            bail!("fixture parameters lack required id {id}");
        }
    }
    let mut params = Parameters::default();
    params.parameters_table.clear();
    for (id, v) in &f.parameters {
        if let Some(p) = parameter_from_id(*id) {
            params.parameters_table.insert(p, *v);
        }
    }
    Ok(params)
}

fn is_cost_error(e: &EvalError) -> bool {
    match e {
        EvalError::CostError(_) => true,
        EvalError::Spanned(s) => is_cost_error(&s.error),
        _ => false,
    }
}

/// Map a validation failure to (error_class, index of the failing input, if any).
pub fn classify(e: &TxValidationError) -> (&'static str, Option<usize>) {
    use TxValidationError as E;
    match e {
        E::OutputSumOverflow
        | E::InputSumOverflow
        | E::TokenAmountError(_)
        | E::ErgPreservationError(..)
        | E::TokenPreservationError { .. } => ("conservation", None),
        E::DoubleSpend(..)
        | E::DustOutput(..)
        | E::InvalidHeightError(_)
        | E::MonotonicHeightError(..)
        | E::NegativeHeight
        | E::BoxSizeExceeded(_)
        | E::ScriptSizeExceeded(_) => ("rule", None),
        E::InitCostExceeded(_) => ("cost_limit", None),
        E::TransactionContextError(_) | E::SigmaSerializationError(_) => ("other", None),
        E::ReducedToFalse(i, _) => ("proof", Some(*i)), // refined by the caller
        E::VerifierError(i, ve) => match ve {
            VerifierError::EvalError(ee) if is_cost_error(ee) => ("cost_limit", Some(*i)),
            VerifierError::SigParsingError(_) => ("proof", Some(*i)),
            _ => ("other", Some(*i)),
        },
    }
}

struct Parsed {
    tx: Transaction,
    inputs: Vec<ErgoBox>,
    data_inputs: Vec<ErgoBox>,
    headers: Headers,
    header: Header,
    parameters: Parameters,
}

fn parse_fixture(f: &Fixture) -> Result<Parsed> {
    let tx = Transaction::sigma_parse_bytes(&unhex(&f.tx)?).map_err(|e| anyhow!("tx: {e}"))?;
    let boxes = |v: &[String], what: &str| -> Result<Vec<ErgoBox>> {
        v.iter()
            .enumerate()
            .map(|(i, h)| {
                ErgoBox::sigma_parse_bytes(&unhex(h)?).map_err(|e| anyhow!("{what} box {i}: {e}"))
            })
            .collect()
    };
    let inputs = boxes(&f.inputs, "input")?;
    let data_inputs = boxes(&f.data_inputs, "data input")?;
    let header =
        Header::scorex_parse_bytes(&unhex(&f.header)?).map_err(|e| anyhow!("header: {e}"))?;
    let prev = f
        .headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            Header::scorex_parse_bytes(&unhex(h)?).map_err(|e| anyhow!("previous header {i}: {e}"))
        })
        .collect::<Result<Vec<_>>>()?;
    let headers: Headers = prev
        .try_into()
        .map_err(|_| anyhow!("fixture must carry between 1 and 10 previous headers"))?;
    Ok(Parsed {
        tx,
        inputs,
        data_inputs,
        headers,
        header,
        parameters: build_parameters(f)?,
    })
}

fn base_result(f: &Fixture) -> ResultRec {
    ResultRec {
        schema: SCHEMA_VERSION,
        runner: "rust".into(),
        runner_version: runner_version(),
        tx_id: f.tx_id.clone(),
        height: f.height,
        status: "error".into(),
        total_cost: None,
        inputs: vec![],
        error_class: None,
        error: None,
    }
}

fn validate_fixture(f: &Fixture) -> ResultRec {
    let mut r = base_result(f);
    let p = match parse_fixture(f) {
        Ok(p) => p,
        Err(e) => {
            r.error_class = Some("parse".into());
            r.error = Some(truncate(format!("{e:#}")));
            return r;
        }
    };
    if let Err(e) = check_parameters_agree(f) {
        r.error_class = Some("other".into());
        r.error = Some(truncate(format!("{e:#}")));
        return r;
    }
    let n_inputs = p.tx.inputs.len();
    let state = ErgoStateContext::new(PreHeader::from(p.header), p.headers, p.parameters);
    let ctx = match TransactionContext::new(p.tx, p.inputs, p.data_inputs) {
        Ok(c) => c,
        Err(e) => {
            r.error_class = Some("parse".into());
            r.error = Some(truncate(format!("transaction context: {e}")));
            return r;
        }
    };
    // Stateless checks first (the node's `validateStateless`), then the stateful ones.
    let mut reports: Vec<InputReport> = Vec::new();
    let outcome = match ctx.spending_tx.validate_stateless() {
        Err(e) => Err(e),
        Ok(()) => ctx.validate_with_report(&state, &mut reports),
    };
    reports.resize(n_inputs, InputReport::default());
    let (failed_idx, err_text) = match &outcome {
        Ok(_) => (None, None),
        Err(e) => (classify(e).1, Some(truncate(e.to_string()))),
    };
    r.inputs = reports
        .iter()
        .enumerate()
        .map(|(i, rep)| {
            let failed = failed_idx == Some(i);
            let evaluated = rep.verified || failed;
            InputResult {
                index: i as u32,
                ok: rep.verified,
                cost: rep.cost,
                error: if failed {
                    err_text.clone()
                } else if !evaluated {
                    Some("not evaluated".into())
                } else {
                    None
                },
            }
        })
        .collect();
    match outcome {
        Ok(total) => {
            r.status = "valid".into();
            r.total_cost = Some(total);
        }
        Err(e) => {
            let (mut class, idx) = classify(&e);
            if let (TxValidationError::ReducedToFalse(..), Some(i)) = (&e, idx) {
                // A trivially-false proposition is a script that reduced to false; anything else
                // reduced to a sigma statement whose proof did not verify.
                if reports[i].reduced_to_trivial_false {
                    class = "script_false";
                }
            }
            r.status = "invalid".into();
            r.error_class = Some(class.into());
            r.error = err_text;
        }
    }
    r
}

fn panic_message(p: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".into()
    }
}

/// Evaluate one fixture, turning a panic into an `error` result.
pub fn eval_fixture(f: &Fixture) -> ResultRec {
    match catch_unwind(AssertUnwindSafe(|| validate_fixture(f))) {
        Ok(r) => r,
        Err(p) => {
            let mut r = base_result(f);
            r.error_class = Some("other".into());
            r.error = Some(truncate(format!("panic: {}", panic_message(p))));
            r
        }
    }
}

pub fn run(input: PathBuf, output: PathBuf) -> Result<()> {
    std::panic::set_hook(Box::new(|_| {}));
    let file = std::fs::File::open(&input).with_context(|| format!("open {input:?}"))?;
    let mut lines = Vec::new();
    for line in std::io::BufReader::new(file).lines() {
        let line = line?;
        if !line.trim().is_empty() {
            lines.push(line);
        }
    }
    // Deeply nested scripts recurse; give workers a generous stack.
    let pool = rayon::ThreadPoolBuilder::new()
        .stack_size(256 << 20)
        .build()?;
    let results: Vec<ResultRec> = pool.install(|| {
        lines
            .par_iter()
            .map(|l| match serde_json::from_str::<Fixture>(l) {
                Ok(f) => eval_fixture(&f),
                Err(e) => {
                    // Not even a fixture: report under whatever id we can find.
                    let v: serde_json::Value = serde_json::from_str(l).unwrap_or_default();
                    ResultRec {
                        schema: SCHEMA_VERSION,
                        runner: "rust".into(),
                        runner_version: runner_version(),
                        tx_id: v.get("tx_id").and_then(|x| x.as_str()).unwrap_or("").into(),
                        height: v.get("height").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
                        status: "error".into(),
                        total_cost: None,
                        inputs: vec![],
                        error_class: Some("parse".into()),
                        error: Some(truncate(format!("fixture JSON: {e}"))),
                    }
                }
            })
            .collect()
    });
    let mut out = std::io::BufWriter::new(std::fs::File::create(&output)?);
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    for r in &results {
        serde_json::to_writer(&mut out, r)?;
        out.write_all(b"\n")?;
        *counts
            .entry(format!(
                "{} {}",
                r.status,
                r.error_class.as_deref().unwrap_or("-")
            ))
            .or_default() += 1;
    }
    out.flush()?;
    println!("results written: {}", results.len());
    for (k, n) in counts {
        println!("  {k}: {n}");
    }
    Ok(())
}
