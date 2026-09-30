//! ergo-parity: differential harness for sigma-rust vs the Ergo node's validation.
//! Data formats are described in SCHEMA.md.

mod diff;
mod eval;
mod extract;
mod node;
mod params;
mod schema;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "ergo-parity",
    about = "sigma-rust vs Ergo node transaction validation parity"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Extract fixtures from a node.
    Extract {
        /// Node base URL.
        #[arg(long)]
        node: String,
        /// Inclusive height range, `<from>..<to>`.
        #[arg(long)]
        heights: String,
        /// Fixture file to write.
        #[arg(long)]
        out: PathBuf,
        /// Reject log to write.
        #[arg(long)]
        rejects: Option<PathBuf>,
        /// Concurrent node requests.
        #[arg(long, default_value_t = 8)]
        concurrency: usize,
        /// Directory caching node responses.
        #[arg(long, default_value = "ergo-parity-cache")]
        cache: PathBuf,
        /// Do not read or write the response cache.
        #[arg(long)]
        no_cache: bool,
    },
    /// Validate fixtures with this workspace's ergo-lib.
    Eval {
        #[arg(long = "in")]
        input: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Compare two result files.
    Diff {
        a: PathBuf,
        b: PathBuf,
        /// Maximum mismatches to list.
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
}

fn parse_heights(s: &str) -> Result<(u32, u32)> {
    let (a, b) = s
        .split_once("..")
        .ok_or_else(|| anyhow!("--heights must look like 700000..700050"))?;
    let (a, b): (u32, u32) = (a.trim().parse()?, b.trim().parse()?);
    if a > b {
        return Err(anyhow!("--heights: from > to"));
    }
    Ok((a, b))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let res: Result<bool> = match cli.cmd {
        Cmd::Extract {
            node,
            heights,
            out,
            rejects,
            concurrency,
            cache,
            no_cache,
        } => parse_heights(&heights).and_then(|(from, to)| {
            extract::run(extract::ExtractArgs {
                node,
                from,
                to,
                out,
                rejects,
                concurrency,
                cache: if no_cache { None } else { Some(cache) },
            })
            .map(|_| true)
        }),
        Cmd::Eval { input, out } => eval::run(input, out).map(|_| true),
        Cmd::Diff { a, b, limit } => diff::run(&a, &b, limit),
    };
    match res {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}
