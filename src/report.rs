use crate::graph;
use crate::ir::AnalysisCache;
use anyhow::{Context, Result};
use csv::Writer;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

pub fn write_report(cache: &AnalysisCache, out: &Path, top_n: usize) -> Result<()> {
    fs::create_dir_all(out.join("assets"))?;
    fs::create_dir_all(out.join("graphs"))?;
    fs::create_dir_all(out.join("functions"))?;
    fs::create_dir_all(out.join("data"))?;

    write_cache(cache, &out.join("data/analysis-cache.json"))?;
    write_csvs(cache, &out.join("data"))?;
    let graphs = write_graphs(cache, &out.join("graphs"), top_n)?;
    write_index(cache, out, &graphs)?;
    write_stylesheet(out)?;
    Ok(())
}

fn write_cache(cache: &AnalysisCache, path: &Path) -> Result<()> {
    let json = serde_json::to_string_pretty(cache)?;
    fs::write(path, json).with_context(|| format!("failed to write {}", path.display()))
}

fn write_csvs(cache: &AnalysisCache, data_dir: &Path) -> Result<()> {
    write_definitions_csv(cache, data_dir)?;
    write_uses_csv(cache, data_dir)?;
    write_def_use_edges_csv(cache, data_dir)?;
    write_var_dependencies_csv(cache, data_dir)?;
    write_function_summaries_csv(cache, data_dir)?;
    write_parse_diagnostics_csv(cache, data_dir)?;
    Ok(())
}

fn write_definitions_csv(cache: &AnalysisCache, data_dir: &Path) -> Result<()> {
    let mut writer = Writer::from_path(data_dir.join("definitions.csv"))?;
    writer.write_record([
        "def_id",
        "file",
        "path",
        "line",
        "col",
        "end_line",
        "end_col",
        "scope_id",
        "function_id",
        "place",
        "def_kind",
        "expr",
        "deps",
    ])?;

    for definition in &cache.definitions {
        writer.write_record([
            definition.def_id.as_str(),
            definition.span.file.as_str(),
            definition.span.file.as_str(),
            &definition.span.line.to_string(),
            &definition.span.col.to_string(),
            &definition.span.end_line.to_string(),
            &definition.span.end_col.to_string(),
            definition.scope_id.as_str(),
            definition.function_id.as_deref().unwrap_or(""),
            &json_field(&definition.place),
            definition.def_kind.as_str(),
            definition.expr.as_str(),
            &json_field(&definition.deps),
        ])?;
    }
    writer.flush()?;
    Ok(())
}

fn write_uses_csv(cache: &AnalysisCache, data_dir: &Path) -> Result<()> {
    let mut writer = Writer::from_path(data_dir.join("uses.csv"))?;
    writer.write_record([
        "use_id",
        "file",
        "path",
        "line",
        "col",
        "end_line",
        "end_col",
        "scope_id",
        "function_id",
        "place",
        "use_kind",
        "context",
    ])?;

    for use_site in &cache.uses {
        writer.write_record([
            use_site.use_id.as_str(),
            use_site.span.file.as_str(),
            use_site.span.file.as_str(),
            &use_site.span.line.to_string(),
            &use_site.span.col.to_string(),
            &use_site.span.end_line.to_string(),
            &use_site.span.end_col.to_string(),
            use_site.scope_id.as_str(),
            use_site.function_id.as_deref().unwrap_or(""),
            &json_field(&use_site.place),
            use_site.use_kind.as_str(),
            use_site.context.as_str(),
        ])?;
    }
    writer.flush()?;
    Ok(())
}

fn write_def_use_edges_csv(cache: &AnalysisCache, data_dir: &Path) -> Result<()> {
    let def_map = cache
        .definitions
        .iter()
        .map(|definition| (definition.def_id.clone(), definition))
        .collect::<BTreeMap<_, _>>();
    let use_map = cache
        .uses
        .iter()
        .map(|use_site| (use_site.use_id.clone(), use_site))
        .collect::<BTreeMap<_, _>>();
    let mut writer = Writer::from_path(data_dir.join("def_use_edges.csv"))?;
    writer.write_record([
        "edge_id",
        "def_id",
        "use_id",
        "place",
        "edge_kind",
        "def_file",
        "def_line",
        "use_file",
        "use_line",
        "path_summary",
    ])?;

    for edge in &cache.def_use_edges {
        let def = def_map.get(&edge.def_id);
        let use_site = use_map.get(&edge.use_id);
        writer.write_record([
            edge.edge_id.as_str(),
            edge.def_id.as_str(),
            edge.use_id.as_str(),
            &json_field(&edge.place),
            edge.edge_kind.as_str(),
            def.map(|item| item.span.file.as_str()).unwrap_or(""),
            &def.map(|item| item.span.line.to_string())
                .unwrap_or_default(),
            use_site.map(|item| item.span.file.as_str()).unwrap_or(""),
            &use_site
                .map(|item| item.span.line.to_string())
                .unwrap_or_default(),
            edge.path_summary.as_str(),
        ])?;
    }
    writer.flush()?;
    Ok(())
}

fn write_var_dependencies_csv(cache: &AnalysisCache, data_dir: &Path) -> Result<()> {
    let mut writer = Writer::from_path(data_dir.join("var_dependencies.csv"))?;
    writer.write_record([
        "edge_id",
        "source_place",
        "target_place",
        "source_id",
        "target_id",
        "dep_kind",
        "file",
        "line",
        "context",
    ])?;

    for edge in &cache.var_dependency_edges {
        writer.write_record([
            edge.edge_id.as_str(),
            &json_field(&edge.source_place),
            &json_field(&edge.target_place),
            edge.source_id.as_str(),
            edge.target_id.as_str(),
            edge.dep_kind.as_str(),
            edge.span.file.as_str(),
            &edge.span.line.to_string(),
            edge.span.snippet.as_str(),
        ])?;
    }
    writer.flush()?;
    Ok(())
}

fn write_function_summaries_csv(cache: &AnalysisCache, data_dir: &Path) -> Result<()> {
    let function_map = cache
        .functions
        .iter()
        .map(|function| (function.function_id.clone(), function))
        .collect::<BTreeMap<_, _>>();
    let mut writer = Writer::from_path(data_dir.join("function_summaries.csv"))?;
    writer.write_record([
        "function_id",
        "qualified_name",
        "file",
        "line",
        "inputs",
        "returns",
        "yields",
        "writes",
        "raises",
        "external_effects",
        "fixpoint_status",
    ])?;

    for summary in &cache.function_summaries {
        let function = function_map.get(&summary.function_id);
        writer.write_record([
            summary.function_id.as_str(),
            function
                .map(|item| item.qualified_name.as_str())
                .unwrap_or(""),
            function.map(|item| item.span.file.as_str()).unwrap_or(""),
            &function
                .map(|item| item.span.line.to_string())
                .unwrap_or_default(),
            &json_field(&summary.inputs),
            &json_field(&summary.returns),
            &json_field(&summary.yields),
            &json_field(&summary.writes),
            &json_field(&summary.raises),
            &json_field(&summary.external_effects),
            summary.fixpoint_status.as_str(),
        ])?;
    }
    writer.flush()?;
    Ok(())
}

fn write_parse_diagnostics_csv(cache: &AnalysisCache, data_dir: &Path) -> Result<()> {
    let mut writer = Writer::from_path(data_dir.join("parse_diagnostics.csv"))?;
    writer.write_record([
        "diagnostic_id",
        "severity",
        "kind",
        "file",
        "line",
        "col",
        "end_line",
        "end_col",
        "message",
    ])?;

    for diagnostic in &cache.diagnostics {
        writer.write_record([
            diagnostic.diagnostic_id.as_str(),
            diagnostic.severity.as_str(),
            diagnostic.kind.as_str(),
            diagnostic.file.as_str(),
            &diagnostic.span.line.to_string(),
            &diagnostic.span.col.to_string(),
            &diagnostic.span.end_line.to_string(),
            &diagnostic.span.end_col.to_string(),
            diagnostic.message.as_str(),
        ])?;
    }
    writer.flush()?;
    Ok(())
}

fn write_graphs(
    cache: &AnalysisCache,
    graph_dir: &Path,
    top_n: usize,
) -> Result<Vec<GeneratedGraph>> {
    let specs = [
        GraphSpec {
            name: "Def-use hotspots",
            stem: "def_use_hotspots",
            writer: GraphWriter::DefUseHotspots,
            top_n: Some(top_n),
        },
        GraphSpec {
            name: "Variable dependencies",
            stem: "variable_dependencies",
            writer: GraphWriter::VariableDependencies,
            top_n: Some(top_n),
        },
    ];
    remove_stale_graph_artifacts(graph_dir, &specs)?;
    let mut generated = Vec::new();

    for spec in specs {
        let dot_path = graph_dir.join(format!("{}.dot", spec.stem));
        let svg_path = graph_dir.join(format!("{}.svg", spec.stem));
        match spec.writer {
            GraphWriter::DefUseHotspots => {
                graph::write_def_use_hotspots_dot(cache, &dot_path, spec.top_n.unwrap_or(top_n))?
            }
            GraphWriter::VariableDependencies => {
                graph::write_var_dependency_dot(cache, &dot_path, spec.top_n.unwrap_or(top_n))?
            }
        }
        let svg_written = graph::render_svg(&dot_path, &svg_path)?;
        generated.push(GeneratedGraph {
            name: spec.name.to_string(),
            dot_rel: format!("graphs/{}.dot", spec.stem),
            svg_rel: svg_written.then(|| format!("graphs/{}.svg", spec.stem)),
        });
    }

    Ok(generated)
}

fn remove_stale_graph_artifacts(graph_dir: &Path, active_specs: &[GraphSpec]) -> Result<()> {
    const KNOWN_GRAPH_STEMS: &[&str] = &[
        "def_use_hotspots",
        "variable_dependencies",
        "module_dependencies",
        "function_dependencies",
    ];

    for stem in KNOWN_GRAPH_STEMS {
        if active_specs.iter().any(|spec| spec.stem == *stem) {
            continue;
        }

        for ext in ["dot", "svg"] {
            let path = graph_dir.join(format!("{stem}.{ext}"));
            if path.exists() {
                fs::remove_file(&path)
                    .with_context(|| format!("failed to remove stale graph {}", path.display()))?;
            }
        }
    }

    Ok(())
}

fn write_index(cache: &AnalysisCache, out: &Path, graphs: &[GeneratedGraph]) -> Result<()> {
    let graph_items = graphs
        .iter()
        .map(|graph| {
            let svg_link = graph
                .svg_rel
                .as_ref()
                .map(|svg| format!(r#"<a href="{svg}">SVG</a>"#))
                .unwrap_or_else(|| "<span>SVG unavailable</span>".to_string());
            format!(
                r#"<article class="graph-card"><h3>{}</h3><p><a href="{}">DOT</a> · {}</p></article>"#,
                escape_html(&graph.name),
                escape_html(&graph.dot_rel),
                svg_link
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let data_links = [
        "analysis-cache.json",
        "definitions.csv",
        "uses.csv",
        "def_use_edges.csv",
        "var_dependencies.csv",
        "function_summaries.csv",
        "parse_diagnostics.csv",
    ]
    .into_iter()
    .map(|file| format!(r#"<li><a href="data/{file}">{file}</a></li>"#))
    .collect::<Vec<_>>()
    .join("\n");

    let html = format!(
        r#"<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Data Flow Report</title>
  <link rel="stylesheet" href="assets/report.css">
</head>
<body>
  <main class="page">
    <header class="hero">
      <p class="eyebrow">Static analysis output</p>
      <h1>Data Flow Report</h1>
      <p class="lede">Definitions, uses, dependency edges, DOT graphs, SVG renders, and exportable CSV data in one static report.</p>
    </header>
    <section class="stats">
      <article><span>Files</span><strong>{}</strong></article>
      <article><span>Functions</span><strong>{}</strong></article>
      <article><span>Classes</span><strong>{}</strong></article>
      <article><span>Definitions</span><strong>{}</strong></article>
      <article><span>Uses</span><strong>{}</strong></article>
      <article><span>Def-use edges</span><strong>{}</strong></article>
      <article><span>Var deps</span><strong>{}</strong></article>
      <article><span>Diagnostics</span><strong>{}</strong></article>
    </section>
    <section class="panel">
      <div class="panel-head">
        <h2>Graphs</h2>
        <p>DOT is always emitted. SVG appears when Graphviz `dot` is available on the machine that generated this report.</p>
      </div>
      <div class="graph-grid">
        {}
      </div>
    </section>
    <section class="panel two-up">
      <div>
        <h2>Data exports</h2>
        <ul class="link-list">
          {}
        </ul>
      </div>
      <div>
        <h2>Analysis cache</h2>
        <p>The canonical machine-readable result is <a href="data/analysis-cache.json">analysis-cache.json</a>. The `paths` command can consume this file directly.</p>
      </div>
    </section>
  </main>
</body>
</html>
"#,
        cache.files.len(),
        cache.functions.len(),
        cache.classes.len(),
        cache.definitions.len(),
        cache.uses.len(),
        cache.def_use_edges.len(),
        cache.var_dependency_edges.len(),
        cache.diagnostics.len(),
        graph_items,
        data_links
    );
    fs::write(out.join("index.html"), html)
        .with_context(|| format!("failed to write {}", out.join("index.html").display()))
}

fn write_stylesheet(out: &Path) -> Result<()> {
    let css = r#":root {
  --ink: #1f2933;
  --muted: #52606d;
  --paper: #fffaf2;
  --panel: rgba(255, 255, 255, 0.86);
  --line: rgba(82, 96, 109, 0.18);
  --accent: #9c4f1a;
  --accent-soft: #f1d3bf;
  --shadow: 0 24px 60px rgba(75, 57, 41, 0.12);
}

* { box-sizing: border-box; }
body {
  margin: 0;
  color: var(--ink);
  background:
    radial-gradient(circle at top, rgba(241, 211, 191, 0.85), transparent 34%),
    linear-gradient(180deg, #f6efe5 0%, #fffaf2 100%);
  font-family: "Iowan Old Style", "Palatino Linotype", "Book Antiqua", Georgia, serif;
}
a { color: var(--accent); text-decoration: none; }
a:hover { text-decoration: underline; }
.page { max-width: 1120px; margin: 0 auto; padding: 40px 20px 64px; }
.hero {
  padding: 28px;
  border: 1px solid var(--line);
  border-radius: 28px;
  background: linear-gradient(135deg, rgba(255,255,255,0.88), rgba(255,244,233,0.92));
  box-shadow: var(--shadow);
}
.eyebrow {
  margin: 0 0 8px;
  color: var(--muted);
  text-transform: uppercase;
  letter-spacing: 0.12em;
  font-size: 12px;
}
.hero h1 { margin: 0; font-size: clamp(32px, 6vw, 56px); line-height: 0.95; }
.lede { max-width: 60ch; color: var(--muted); font-size: 18px; line-height: 1.6; }
.stats {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
  gap: 14px;
  margin: 24px 0;
}
.stats article,
.panel {
  border: 1px solid var(--line);
  border-radius: 22px;
  background: var(--panel);
  box-shadow: var(--shadow);
}
.stats article { padding: 18px; }
.stats span { display: block; color: var(--muted); font-size: 13px; text-transform: uppercase; letter-spacing: 0.08em; }
.stats strong { display: block; margin-top: 10px; font-size: 32px; }
.panel { padding: 22px; margin-top: 20px; }
.panel-head p { color: var(--muted); max-width: 70ch; }
.graph-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: 14px;
}
.graph-card {
  padding: 18px;
  border-radius: 18px;
  background: linear-gradient(180deg, rgba(255,255,255,0.92), rgba(250,237,225,0.92));
  border: 1px solid var(--line);
}
.graph-card h3 { margin-top: 0; margin-bottom: 8px; }
.two-up {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
  gap: 22px;
}
.link-list {
  margin: 0;
  padding-left: 18px;
  line-height: 1.9;
}
@media (max-width: 640px) {
  .page { padding: 20px 14px 40px; }
  .hero { padding: 22px; border-radius: 22px; }
  .panel, .stats article { border-radius: 18px; }
}"#;

    fs::write(out.join("assets/report.css"), css).with_context(|| {
        format!(
            "failed to write {}",
            out.join("assets/report.css").display()
        )
    })
}

fn json_field<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"<serialize-error>\"".to_string())
}

fn escape_html(value: &str) -> String {
    html_escape::encode_text(value).into_owned()
}

#[derive(Debug, Clone)]
struct GraphSpec {
    name: &'static str,
    stem: &'static str,
    writer: GraphWriter,
    top_n: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
enum GraphWriter {
    DefUseHotspots,
    VariableDependencies,
}

#[derive(Debug, Clone)]
struct GeneratedGraph {
    name: String,
    dot_rel: String,
    svg_rel: Option<String>,
}
