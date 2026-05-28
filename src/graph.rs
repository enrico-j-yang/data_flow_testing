use crate::ir::AnalysisCache;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::process::Command;

pub fn dot_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

pub fn write_def_use_hotspots_dot(cache: &AnalysisCache, path: &Path, top_n: usize) -> Result<()> {
    let mut text =
        String::from("digraph DefUseHotspots {\n  rankdir=LR;\n  node [shape=box, fontsize=10];\n");

    for edge in cache.def_use_edges.iter().take(top_n) {
        text.push_str(&format!(
            "  \"{}\" -> \"{}\" [label=\"{}\"];\n",
            dot_label(&edge.def_id),
            dot_label(&edge.use_id),
            dot_label(&edge.edge_kind)
        ));
    }

    text.push_str("}\n");
    write_dot(path, &text)
}

pub fn write_module_dependency_dot(cache: &AnalysisCache, path: &Path) -> Result<()> {
    let mut text = String::from(
        "digraph ModuleDependencies {\n  rankdir=LR;\n  node [shape=box, fontsize=10];\n",
    );

    for module in &cache.modules {
        text.push_str(&format!(
            "  \"{}\" [label=\"{}\"];\n",
            dot_label(&module.module_id),
            dot_label(&module.module_name)
        ));
    }

    for module in &cache.modules {
        for import in &module.imports {
            let target_id = format!("import:{}", import.module);
            text.push_str(&format!(
                "  \"{}\" -> \"{}\" [label=\"{}\"];\n",
                dot_label(&module.module_id),
                dot_label(&target_id),
                dot_label(import.name.as_deref().unwrap_or(&import.resolution))
            ));
            text.push_str(&format!(
                "  \"{}\" [label=\"{}\"];\n",
                dot_label(&target_id),
                dot_label(&import.module)
            ));
        }
    }

    text.push_str("}\n");
    write_dot(path, &text)
}

pub fn write_function_dependency_dot(cache: &AnalysisCache, path: &Path) -> Result<()> {
    let mut text = String::from(
        "digraph FunctionDependencies {\n  rankdir=LR;\n  node [shape=box, fontsize=10];\n",
    );

    for function in &cache.functions {
        text.push_str(&format!(
            "  \"{}\" [label=\"{}\"];\n",
            dot_label(&function.function_id),
            dot_label(&function.qualified_name)
        ));
    }

    for call in &cache.calls {
        let Some(caller_id) = &call.function_id else {
            continue;
        };

        if call.candidate_function_ids.is_empty() {
            let external_id = format!("external:{}", call.call_id);
            text.push_str(&format!(
                "  \"{}\" [label=\"{}\"];\n",
                dot_label(&external_id),
                dot_label(&call.callee_expr)
            ));
            text.push_str(&format!(
                "  \"{}\" -> \"{}\" [label=\"{}\"];\n",
                dot_label(caller_id),
                dot_label(&external_id),
                dot_label(&call.resolution)
            ));
            continue;
        }

        for callee_id in &call.candidate_function_ids {
            text.push_str(&format!(
                "  \"{}\" -> \"{}\" [label=\"{}\"];\n",
                dot_label(caller_id),
                dot_label(callee_id),
                dot_label(&call.resolution)
            ));
        }
    }

    text.push_str("}\n");
    write_dot(path, &text)
}

pub fn write_var_dependency_dot(cache: &AnalysisCache, path: &Path, top_n: usize) -> Result<()> {
    let mut text = String::from(
        "digraph VariableDependencies {\n  rankdir=LR;\n  node [shape=box, fontsize=10];\n",
    );

    for edge in cache.var_dependency_edges.iter().take(top_n) {
        text.push_str(&format!(
            "  \"{}\" -> \"{}\" [label=\"{}\"];\n",
            dot_label(&edge.source_id),
            dot_label(&edge.target_id),
            dot_label(&edge.dep_kind)
        ));
    }

    text.push_str("}\n");
    write_dot(path, &text)
}

pub fn render_svg(dot_path: &Path, svg_path: &Path) -> Result<bool> {
    let output = Command::new("dot")
        .arg("-Tsvg")
        .arg(dot_path)
        .arg("-o")
        .arg(svg_path)
        .output();

    match output {
        Ok(output) if output.status.success() => Ok(true),
        Ok(_) => Ok(false),
        Err(_) => Ok(false),
    }
}

fn write_dot(path: &Path, text: &str) -> Result<()> {
    fs::write(path, text).with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{AnalysisCache, DefUseEdge, Place, SCHEMA_VERSION};

    #[test]
    fn dot_label_escapes_quotes_backslashes_and_newlines() {
        assert_eq!(dot_label("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
    }

    #[test]
    fn def_use_hotspot_writer_emits_dot_edges() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hotspots.dot");
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            def_use_edges: vec![DefUseEdge {
                edge_id: "DU_1".to_string(),
                def_id: "D_x".to_string(),
                use_id: "U_x".to_string(),
                place: Place::Local {
                    scope_id: "S".to_string(),
                    name: "x".to_string(),
                },
                edge_kind: "local".to_string(),
                path_summary: "same-block".to_string(),
            }],
            ..AnalysisCache::default()
        };

        write_def_use_hotspots_dot(&cache, &path, 10).unwrap();

        let dot = std::fs::read_to_string(path).unwrap();
        assert!(dot.contains("\"D_x\" -> \"U_x\""));
        assert!(dot.contains("local"));
    }
}
