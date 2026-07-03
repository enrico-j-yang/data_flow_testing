//! Standalone tool: load an analysis-cache.json and re-emit the report.
//! Used to iterate on report/graph performance without re-running analyze.
//!
//! cargo run --release --example render_report -- <cache.json> <out-dir>

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::time::Instant;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let cache_path = PathBuf::from(
        args.next()
            .context("usage: render_report <cache.json> <out-dir> [top_n]")?,
    );
    let out_dir = PathBuf::from(args.next().context("missing <out-dir>")?);
    let top_n: usize = args
        .next()
        .map(|raw| raw.parse().expect("top_n must be a non-negative integer"))
        .unwrap_or(100);

    let t = Instant::now();
    let text = std::fs::read_to_string(&cache_path)
        .with_context(|| format!("failed to read {}", cache_path.display()))?;
    eprintln!(
        "read   {:>6.1}s ({} MiB)",
        t.elapsed().as_secs_f32(),
        text.len() / (1024 * 1024)
    );

    let t = Instant::now();
    let cache: data_flow_analyzer::ir::AnalysisCache = serde_json::from_str(&text)
        .with_context(|| format!("failed to parse {}", cache_path.display()))?;
    eprintln!("parse  {:>6.1}s", t.elapsed().as_secs_f32());
    eprintln!(
        "cache  defs={} uses={} du_edges={} var_edges={} functions={}",
        cache.definitions.len(),
        cache.uses.len(),
        cache.def_use_edges.len(),
        cache.var_dependency_edges.len(),
        cache.functions.len(),
    );

    let t = Instant::now();
    data_flow_analyzer::report::write_report(&cache, &out_dir, top_n)?;
    eprintln!(
        "report {:>6.1}s -> {}",
        t.elapsed().as_secs_f32(),
        out_dir.display()
    );
    Ok(())
}
