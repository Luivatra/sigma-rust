//! `diff`: compare two result files.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};

use crate::schema::ResultRec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    pub tx_id: String,
    pub height: u32,
    pub kind: &'static str,
    pub detail: String,
}

#[derive(Debug, Default)]
pub struct DiffReport {
    pub compared: usize,
    pub mismatches: Vec<Mismatch>,
}

impl DiffReport {
    pub fn counts(&self) -> BTreeMap<&'static str, usize> {
        let mut m = BTreeMap::new();
        for x in &self.mismatches {
            *m.entry(x.kind).or_default() += 1;
        }
        m
    }
}

pub fn load(path: &Path) -> Result<Vec<ResultRec>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {path:?}"))?;
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| serde_json::from_str(l).with_context(|| format!("{path:?} line {}", i + 1)))
        .collect()
}

fn show<T: std::fmt::Debug>(v: &Option<T>) -> String {
    match v {
        Some(x) => format!("{x:?}"),
        None => "null".into(),
    }
}

/// Compare results paired by `tx_id`. Transactions appear in the report in `a`'s order,
/// followed by those only in `b`.
pub fn diff(a: &[ResultRec], b: &[ResultRec]) -> DiffReport {
    let mut rep = DiffReport::default();
    let index_b: BTreeMap<&str, &ResultRec> = b.iter().map(|r| (r.tx_id.as_str(), r)).collect();
    let index_a: BTreeMap<&str, &ResultRec> = a.iter().map(|r| (r.tx_id.as_str(), r)).collect();
    let mut push = |r: &ResultRec, kind: &'static str, detail: String| {
        rep.mismatches.push(Mismatch {
            tx_id: r.tx_id.clone(),
            height: r.height,
            kind,
            detail,
        });
    };
    for ra in a {
        let Some(rb) = index_b.get(ra.tx_id.as_str()) else {
            push(ra, "only_in_a", format!("status {}", ra.status));
            continue;
        };
        if !std::ptr::eq(index_a[ra.tx_id.as_str()], ra) {
            continue; // duplicate tx_id in `a`: the last record is compared
        }
        rep.compared += 1;
        if ra.status != rb.status {
            push(ra, "status", format!("{} vs {}", ra.status, rb.status));
        }
        if ra.error_class != rb.error_class {
            push(
                ra,
                "error_class",
                format!("{} vs {}", show(&ra.error_class), show(&rb.error_class)),
            );
        }
        if ra.total_cost != rb.total_cost {
            push(
                ra,
                "total_cost",
                format!("{} vs {}", show(&ra.total_cost), show(&rb.total_cost)),
            );
        }
        if ra.inputs.len() != rb.inputs.len() {
            push(
                ra,
                "input_count",
                format!("{} vs {}", ra.inputs.len(), rb.inputs.len()),
            );
        }
        for (ia, ib) in ra.inputs.iter().zip(rb.inputs.iter()) {
            if ia.ok != ib.ok {
                push(
                    ra,
                    "input_ok",
                    format!("input {}: {} vs {}", ia.index, ia.ok, ib.ok),
                );
            }
            if ia.cost != ib.cost {
                push(
                    ra,
                    "input_cost",
                    format!(
                        "input {}: {} vs {}",
                        ia.index,
                        show(&ia.cost),
                        show(&ib.cost)
                    ),
                );
            }
        }
    }
    for rb in b {
        if !index_a.contains_key(rb.tx_id.as_str()) {
            push(rb, "only_in_b", format!("status {}", rb.status));
        }
    }
    rep
}

pub fn run(a: &Path, b: &Path, limit: usize) -> Result<bool> {
    let (ra, rb) = (load(a)?, load(b)?);
    let rep = diff(&ra, &rb);
    println!(
        "a: {} results ({}), b: {} results ({})",
        ra.len(),
        a.display(),
        rb.len(),
        b.display()
    );
    println!("compared {} transactions", rep.compared);
    for m in rep.mismatches.iter().take(limit) {
        println!("{} height {} {}: {}", m.tx_id, m.height, m.kind, m.detail);
    }
    if rep.mismatches.len() > limit {
        println!(
            "... {} more not shown (--limit {limit})",
            rep.mismatches.len() - limit
        );
    }
    if rep.mismatches.is_empty() {
        println!("no mismatches");
    } else {
        println!("mismatches by kind:");
        for (k, n) in rep.counts() {
            println!("  {k}: {n}");
        }
    }
    Ok(rep.mismatches.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::InputResult;

    fn rec(id: &str, cost: u64) -> ResultRec {
        ResultRec {
            schema: 1,
            runner: "rust".into(),
            runner_version: "t".into(),
            tx_id: id.into(),
            height: 10,
            status: "valid".into(),
            total_cost: Some(cost),
            inputs: vec![InputResult {
                index: 0,
                ok: true,
                cost: Some(cost - 5),
                error: None,
            }],
            error_class: None,
            error: None,
        }
    }

    #[test]
    fn identical_files_match() {
        let a = vec![rec("a", 100), rec("b", 200)];
        let mut b = a.clone();
        b[0].runner = "jvm".into(); // runner name is not compared
        let d = diff(&a, &b);
        assert_eq!(d.compared, 2);
        assert!(d.mismatches.is_empty());
    }

    #[test]
    fn reports_every_kind() {
        let a = vec![rec("a", 100), rec("b", 200), rec("only_a", 1000)];
        let mut b = vec![rec("a", 101), rec("b", 200), rec("only_b", 1000)];
        b[1].status = "invalid".into();
        b[1].error_class = Some("proof".into());
        b[1].total_cost = None;
        b[1].inputs[0].ok = false;
        b[1].inputs[0].cost = None;
        b[1].inputs.push(InputResult {
            index: 1,
            ok: false,
            cost: None,
            error: None,
        });
        let d = diff(&a, &b);
        let c = d.counts();
        assert_eq!(c["total_cost"], 2); // a: cost differs, b: Some vs None
        assert_eq!(c["input_cost"], 2);
        assert_eq!(c["status"], 1);
        assert_eq!(c["error_class"], 1);
        assert_eq!(c["input_ok"], 1);
        assert_eq!(c["input_count"], 1);
        assert_eq!(c["only_in_a"], 1);
        assert_eq!(c["only_in_b"], 1);
        assert_eq!(d.compared, 2);
    }

    #[test]
    fn null_costs_compare_equal() {
        let mut x = rec("a", 100);
        x.total_cost = None;
        x.status = "invalid".into();
        let d = diff(&[x.clone()], &[x]);
        assert!(d.mismatches.is_empty());
    }
}
