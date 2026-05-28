use crate::ids::stable_id;
use crate::ir::{AnalysisCache, Place};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
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
    let mut nodes = BTreeMap::new();
    let mut edges = BTreeSet::new();
    let definitions_by_id = cache
        .definitions
        .iter()
        .map(|definition| (definition.def_id.clone(), definition))
        .collect::<BTreeMap<_, _>>();
    let uses_by_id = cache
        .uses
        .iter()
        .map(|use_site| (use_site.use_id.clone(), use_site))
        .collect::<BTreeMap<_, _>>();

    for edge in cache.def_use_edges.iter().take(top_n) {
        let def_record = definitions_by_id
            .get(&edge.def_id)
            .map(|definition| DefUseNodeRecord::definition(definition))
            .unwrap_or_else(|| DefUseNodeRecord::fallback("def", edge.place.clone()));
        let use_record = uses_by_id
            .get(&edge.use_id)
            .map(|use_site| DefUseNodeRecord::usage(use_site))
            .unwrap_or_else(|| DefUseNodeRecord::fallback("use", edge.place.clone()));

        nodes.entry(edge.def_id.clone()).or_insert(def_record);
        nodes.entry(edge.use_id.clone()).or_insert(use_record);
        edges.insert((
            edge.def_id.clone(),
            edge.use_id.clone(),
            edge.edge_kind.clone(),
        ));
    }

    let labels = PlaceLabelContext::new(cache);
    let mut short_label_counts = BTreeMap::new();
    for record in nodes.values() {
        *short_label_counts
            .entry(record.base_label(&labels))
            .or_insert(0usize) += 1;
    }

    let mut node_labels = BTreeMap::new();
    for (node_id, record) in &nodes {
        let base_label = record.base_label(&labels);
        let short_label = record.short_label(&labels);
        let label = if short_label_counts
            .get(&base_label)
            .copied()
            .unwrap_or_default()
            > 1
        {
            record.disambiguated_label(&labels)
        } else {
            short_label
        };
        node_labels.insert(node_id.clone(), label);
    }

    let mut final_label_counts = BTreeMap::new();
    for label in node_labels.values() {
        *final_label_counts.entry(label.clone()).or_insert(0usize) += 1;
    }

    for (node_id, record) in nodes {
        let mut label = node_labels.get(&node_id).cloned().unwrap_or_default();
        if final_label_counts.get(&label).copied().unwrap_or_default() > 1 {
            label = record.fully_disambiguated_label(&labels);
        }
        text.push_str(&format!(
            "  \"{}\" [label=\"{}\"];\n",
            dot_label(&node_id),
            dot_label(&label)
        ));
    }

    for (def_id, use_id, edge_kind) in edges {
        text.push_str(&format!(
            "  \"{}\" -> \"{}\" [label=\"{}\"];\n",
            dot_label(&def_id),
            dot_label(&use_id),
            dot_label(&edge_kind)
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

fn place_key(place: &Place) -> String {
    format!("{place:?}")
}

fn place_node_id(schema_version: u32, place: &Place) -> String {
    let key = place_key(place);
    stable_id("VP", schema_version, &[&key])
}

struct PlaceLabelContext {
    scope_labels: BTreeMap<String, String>,
    module_labels: BTreeMap<String, String>,
}

impl PlaceLabelContext {
    fn new(cache: &AnalysisCache) -> Self {
        let module_labels = cache
            .modules
            .iter()
            .map(|module| (module.module_id.clone(), module.module_name.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut owner_labels = module_labels.clone();

        owner_labels.extend(
            cache
                .classes
                .iter()
                .map(|class| (class.class_id.clone(), class.qualified_name.clone())),
        );
        owner_labels.extend(cache.functions.iter().map(|function| {
            (
                function.function_id.clone(),
                function.qualified_name.clone(),
            )
        }));

        let scope_labels = cache
            .scopes
            .iter()
            .map(|scope| {
                let label = owner_labels
                    .get(&scope.owner_id)
                    .cloned()
                    .unwrap_or_else(|| scope.scope_id.clone());
                (scope.scope_id.clone(), label)
            })
            .collect();

        Self {
            scope_labels,
            module_labels,
        }
    }

    fn short_label(&self, place: &Place) -> String {
        match place {
            Place::Local { name, .. }
            | Place::Global { name, .. }
            | Place::Closure { name, .. } => name.clone(),
            Place::Attribute { base, attr } => format!("{base}.{attr}"),
            Place::Subscript { base, index } => format!("{base}[{index}]"),
            Place::External { name } => format!("external:{name}"),
            Place::Unknown { reason } => format!("unknown:{reason}"),
        }
    }

    fn disambiguated_label(&self, place: &Place) -> String {
        match place {
            Place::Local { scope_id, name } => {
                format!("{}::{name}", self.scope_label(scope_id))
            }
            Place::Global { module_id, name } => {
                format!("{}::{name}", self.module_label(module_id))
            }
            Place::Closure { scope_id, name } => {
                format!("{}::{name} [closure]", self.scope_label(scope_id))
            }
            Place::Attribute { .. }
            | Place::Subscript { .. }
            | Place::External { .. }
            | Place::Unknown { .. } => self.short_label(place),
        }
    }

    fn scope_label(&self, scope_id: &str) -> String {
        self.scope_labels
            .get(scope_id)
            .cloned()
            .unwrap_or_else(|| scope_id.to_string())
    }

    fn module_label(&self, module_id: &str) -> String {
        self.module_labels
            .get(module_id)
            .cloned()
            .unwrap_or_else(|| module_id.to_string())
    }
}

#[derive(Clone)]
struct DefUseNodeRecord {
    role: &'static str,
    place: Place,
    line: usize,
    col: usize,
}

impl DefUseNodeRecord {
    fn definition(definition: &crate::ir::Definition) -> Self {
        Self {
            role: "def",
            place: definition.place.clone(),
            line: definition.span.line,
            col: definition.span.col,
        }
    }

    fn usage(use_site: &crate::ir::Use) -> Self {
        Self {
            role: "use",
            place: use_site.place.clone(),
            line: use_site.span.line,
            col: use_site.span.col,
        }
    }

    fn fallback(role: &'static str, place: Place) -> Self {
        Self {
            role,
            place,
            line: 0,
            col: 0,
        }
    }

    fn base_label(&self, labels: &PlaceLabelContext) -> String {
        format!("{} {}", self.role, labels.short_label(&self.place))
    }

    fn short_label(&self, labels: &PlaceLabelContext) -> String {
        format!(
            "{} {} {}",
            self.role,
            labels.short_label(&self.place),
            self.line_suffix()
        )
    }

    fn disambiguated_label(&self, labels: &PlaceLabelContext) -> String {
        let place_label = labels.disambiguated_label(&self.place);
        format!("{} {} {}", self.role, place_label, self.line_suffix())
    }

    fn fully_disambiguated_label(&self, labels: &PlaceLabelContext) -> String {
        let label = self.disambiguated_label(labels);
        if self.line > 0 && self.col > 0 {
            format!("{label}:{}", self.col)
        } else {
            label
        }
    }

    fn line_suffix(&self) -> String {
        if self.line > 0 {
            format!("@ line {}", self.line)
        } else {
            "@ line ?".to_string()
        }
    }
}

pub fn write_var_dependency_dot(cache: &AnalysisCache, path: &Path, top_n: usize) -> Result<()> {
    let mut text = String::from(
        "digraph VariableDependencies {\n  rankdir=LR;\n  node [shape=box, fontsize=10];\n",
    );
    let mut nodes = BTreeMap::new();
    let mut edges = BTreeSet::new();

    for edge in cache.var_dependency_edges.iter().take(top_n) {
        let source_node_id = place_node_id(cache.schema_version, &edge.source_place);
        let target_node_id = place_node_id(cache.schema_version, &edge.target_place);

        nodes
            .entry(source_node_id.clone())
            .or_insert_with(|| edge.source_place.clone());
        nodes
            .entry(target_node_id.clone())
            .or_insert_with(|| edge.target_place.clone());
        edges.insert((source_node_id, target_node_id, edge.dep_kind.clone()));
    }

    let labels = PlaceLabelContext::new(cache);
    let mut label_counts = BTreeMap::new();
    for place in nodes.values() {
        *label_counts
            .entry(labels.short_label(place))
            .or_insert(0usize) += 1;
    }

    for (node_id, place) in nodes {
        let short_label = labels.short_label(&place);
        let label = if label_counts.get(&short_label).copied().unwrap_or_default() > 1 {
            labels.disambiguated_label(&place)
        } else {
            short_label
        };
        text.push_str(&format!(
            "  \"{}\" [label=\"{}\"];\n",
            dot_label(&node_id),
            dot_label(&label)
        ));
    }

    for (source_node_id, target_node_id, dep_kind) in edges {
        text.push_str(&format!(
            "  \"{}\" -> \"{}\" [label=\"{}\"];\n",
            dot_label(&source_node_id),
            dot_label(&target_node_id),
            dot_label(&dep_kind)
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
        Ok(output) if output.status.success() => {
            post_process_svg_metadata(dot_path, svg_path)?;
            Ok(true)
        }
        Ok(_) => Ok(false),
        Err(_) => Ok(false),
    }
}

fn write_dot(path: &Path, text: &str) -> Result<()> {
    fs::write(path, text).with_context(|| format!("failed to write {}", path.display()))
}

fn post_process_svg_metadata(dot_path: &Path, svg_path: &Path) -> Result<()> {
    let dot = fs::read_to_string(dot_path)
        .with_context(|| format!("failed to read {}", dot_path.display()))?;
    let svg = fs::read_to_string(svg_path)
        .with_context(|| format!("failed to read {}", svg_path.display()))?;
    let rewritten = rewrite_svg_metadata(&dot, &svg);
    fs::write(svg_path, rewritten)
        .with_context(|| format!("failed to write {}", svg_path.display()))
}

fn rewrite_svg_metadata(dot: &str, svg: &str) -> String {
    let metadata = build_svg_metadata_map(dot);
    svg.split_inclusive('\n')
        .map(|line| rewrite_svg_line(line, &metadata))
        .collect()
}

fn build_svg_metadata_map(dot: &str) -> BTreeMap<String, String> {
    let mut node_labels = BTreeMap::new();
    let mut edge_specs = Vec::new();

    for line in dot.lines() {
        if let Some((node_id, label)) = parse_dot_node_label(line) {
            node_labels.insert(node_id, label);
            continue;
        }
        if let Some((source_id, target_id, label)) = parse_dot_edge_label(line) {
            edge_specs.push((source_id, target_id, label));
        }
    }

    let mut metadata = node_labels.clone();
    for (source_id, target_id, edge_label) in edge_specs {
        let source_label = node_labels
            .get(&source_id)
            .cloned()
            .unwrap_or(source_id.clone());
        let target_label = node_labels
            .get(&target_id)
            .cloned()
            .unwrap_or(target_id.clone());
        let title = if edge_label.is_empty() {
            format!("{source_label} -> {target_label}")
        } else {
            format!("{source_label} -> {target_label} ({edge_label})")
        };
        metadata.insert(format!("{source_id}->{target_id}"), title.clone());
        metadata.insert(format!("{source_id}&#45;&gt;{target_id}"), title);
    }

    metadata
}

fn rewrite_svg_line(line: &str, metadata: &BTreeMap<String, String>) -> String {
    let line = rewrite_svg_comment(line, metadata);
    rewrite_svg_title(&line, metadata)
}

fn rewrite_svg_comment(line: &str, metadata: &BTreeMap<String, String>) -> String {
    let Some(start) = line.find("<!-- ") else {
        return line.to_string();
    };
    let Some(end) = line[start + 5..].find(" -->") else {
        return line.to_string();
    };
    let end = start + 5 + end;
    let key = &line[start + 5..end];
    let Some(label) = metadata.get(key) else {
        return line.to_string();
    };
    let safe_label = label.replace("--", "- -");
    format!(
        "{}<!-- {} -->{}",
        &line[..start],
        safe_label,
        &line[end + 4..]
    )
}

fn rewrite_svg_title(line: &str, metadata: &BTreeMap<String, String>) -> String {
    let Some(start) = line.find("<title>") else {
        return line.to_string();
    };
    let Some(end) = line[start + 7..].find("</title>") else {
        return line.to_string();
    };
    let end = start + 7 + end;
    let key = &line[start + 7..end];
    let Some(label) = metadata.get(key) else {
        return line.to_string();
    };
    let escaped = html_escape::encode_text(label);
    format!(
        "{}<title>{}</title>{}",
        &line[..start],
        escaped,
        &line[end + 8..]
    )
}

fn parse_dot_node_label(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    let (node_id, offset) = parse_dot_quoted(trimmed)?;
    let rest = trimmed[offset..].trim_start();
    if rest.starts_with("->") {
        return None;
    }
    let label_start = rest.find("[label=")?;
    let (_, label_offset) = parse_dot_quoted(&rest[label_start + 7..])?;
    let label = parse_dot_quoted(&rest[label_start + 7..])?.0;
    let _ = label_offset;
    Some((node_id, label))
}

fn parse_dot_edge_label(line: &str) -> Option<(String, String, String)> {
    let trimmed = line.trim();
    let (source_id, source_offset) = parse_dot_quoted(trimmed)?;
    let rest = trimmed[source_offset..].trim_start();
    if !rest.starts_with("->") {
        return None;
    }
    let rest = rest[2..].trim_start();
    let (target_id, target_offset) = parse_dot_quoted(rest)?;
    let rest = rest[target_offset..].trim_start();
    let label_start = rest.find("[label=")?;
    let label = parse_dot_quoted(&rest[label_start + 7..])?.0;
    Some((source_id, target_id, label))
}

fn parse_dot_quoted(text: &str) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    if bytes.first().copied()? != b'"' {
        return None;
    }

    let mut result = String::new();
    let mut index = 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => {
                index += 1;
                let escaped = *bytes.get(index)?;
                match escaped {
                    b'\\' => result.push('\\'),
                    b'"' => result.push('"'),
                    b'n' => result.push('\n'),
                    other => result.push(other as char),
                }
            }
            b'"' => return Some((result, index + 1)),
            byte => result.push(byte as char),
        }
        index += 1;
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{
        AnalysisCache, DefUseEdge, Definition, FunctionRecord, Place, SCHEMA_VERSION, ScopeRecord,
        Use,
    };

    #[test]
    fn dot_label_escapes_quotes_backslashes_and_newlines() {
        assert_eq!(dot_label("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
    }

    #[test]
    fn def_use_hotspot_writer_emits_dot_edges() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hotspots.dot");
        let def_span = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 10,
            col: 4,
            end_line: 10,
            end_col: 9,
            snippet: "x = value".to_string(),
        };
        let use_span = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 11,
            col: 11,
            end_line: 11,
            end_col: 12,
            snippet: "print(x)".to_string(),
        };
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            definitions: vec![Definition {
                def_id: "D_x".to_string(),
                place: Place::Local {
                    scope_id: "S".to_string(),
                    name: "x".to_string(),
                },
                def_kind: "assign".to_string(),
                scope_id: "S".to_string(),
                function_id: None,
                span: def_span,
                expr: "value".to_string(),
                deps: Vec::new(),
            }],
            uses: vec![Use {
                use_id: "U_x".to_string(),
                place: Place::Local {
                    scope_id: "S".to_string(),
                    name: "x".to_string(),
                },
                use_kind: "load".to_string(),
                scope_id: "S".to_string(),
                function_id: None,
                span: use_span,
                context: "call".to_string(),
            }],
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
        assert!(dot.contains("[label=\"def x @ line 10\"]"));
        assert!(dot.contains("[label=\"use x @ line 11\"]"));
        assert!(dot.contains("local"));
    }

    #[test]
    fn def_use_hotspot_writer_disambiguates_duplicate_local_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hotspots.dot");
        let span_foo_def = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 10,
            col: 4,
            end_line: 10,
            end_col: 9,
            snippet: "x = 1".to_string(),
        };
        let span_foo_use = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 11,
            col: 10,
            end_line: 11,
            end_col: 11,
            snippet: "print(x)".to_string(),
        };
        let span_bar_def = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 20,
            col: 4,
            end_line: 20,
            end_col: 9,
            snippet: "x = 2".to_string(),
        };
        let span_bar_use = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 21,
            col: 10,
            end_line: 21,
            end_col: 11,
            snippet: "print(x)".to_string(),
        };
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            scopes: vec![
                ScopeRecord {
                    scope_id: "S_foo".to_string(),
                    scope_kind: "function".to_string(),
                    parent_scope_id: None,
                    owner_id: "FN_foo".to_string(),
                    span: span_foo_def.clone(),
                },
                ScopeRecord {
                    scope_id: "S_bar".to_string(),
                    scope_kind: "function".to_string(),
                    parent_scope_id: None,
                    owner_id: "FN_bar".to_string(),
                    span: span_bar_def.clone(),
                },
            ],
            functions: vec![
                FunctionRecord {
                    function_id: "FN_foo".to_string(),
                    module_id: "M_app".to_string(),
                    class_id: None,
                    qualified_name: "foo".to_string(),
                    kind: "function".to_string(),
                    params: Vec::new(),
                    scope_id: "S_foo".to_string(),
                    span: span_foo_def.clone(),
                },
                FunctionRecord {
                    function_id: "FN_bar".to_string(),
                    module_id: "M_app".to_string(),
                    class_id: None,
                    qualified_name: "bar".to_string(),
                    kind: "function".to_string(),
                    params: Vec::new(),
                    scope_id: "S_bar".to_string(),
                    span: span_bar_def.clone(),
                },
            ],
            definitions: vec![
                Definition {
                    def_id: "D_foo_x".to_string(),
                    place: Place::Local {
                        scope_id: "S_foo".to_string(),
                        name: "x".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S_foo".to_string(),
                    function_id: Some("FN_foo".to_string()),
                    span: span_foo_def.clone(),
                    expr: "1".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_bar_x".to_string(),
                    place: Place::Local {
                        scope_id: "S_bar".to_string(),
                        name: "x".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S_bar".to_string(),
                    function_id: Some("FN_bar".to_string()),
                    span: span_bar_def.clone(),
                    expr: "2".to_string(),
                    deps: Vec::new(),
                },
            ],
            uses: vec![
                Use {
                    use_id: "U_foo_x".to_string(),
                    place: Place::Local {
                        scope_id: "S_foo".to_string(),
                        name: "x".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_foo".to_string(),
                    function_id: Some("FN_foo".to_string()),
                    span: span_foo_use.clone(),
                    context: "call".to_string(),
                },
                Use {
                    use_id: "U_bar_x".to_string(),
                    place: Place::Local {
                        scope_id: "S_bar".to_string(),
                        name: "x".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_bar".to_string(),
                    function_id: Some("FN_bar".to_string()),
                    span: span_bar_use.clone(),
                    context: "call".to_string(),
                },
            ],
            def_use_edges: vec![
                DefUseEdge {
                    edge_id: "DU_1".to_string(),
                    def_id: "D_foo_x".to_string(),
                    use_id: "U_foo_x".to_string(),
                    place: Place::Local {
                        scope_id: "S_foo".to_string(),
                        name: "x".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "foo".to_string(),
                },
                DefUseEdge {
                    edge_id: "DU_2".to_string(),
                    def_id: "D_bar_x".to_string(),
                    use_id: "U_bar_x".to_string(),
                    place: Place::Local {
                        scope_id: "S_bar".to_string(),
                        name: "x".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "bar".to_string(),
                },
            ],
            ..AnalysisCache::default()
        };

        write_def_use_hotspots_dot(&cache, &path, 10).unwrap();

        let dot = std::fs::read_to_string(path).unwrap();
        assert!(dot.contains("[label=\"def foo::x @ line 10\"]"));
        assert!(dot.contains("[label=\"use foo::x @ line 11\"]"));
        assert!(dot.contains("[label=\"def bar::x @ line 20\"]"));
        assert!(dot.contains("[label=\"use bar::x @ line 21\"]"));
        assert!(!dot.contains("[label=\"def x\"]"));
        assert!(!dot.contains("[label=\"use x\"]"));
    }

    #[test]
    fn var_dependency_writer_uses_place_labels() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("variables.dot");
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            var_dependency_edges: vec![crate::ir::VarDependencyEdge {
                edge_id: "VD_1".to_string(),
                source_place: Place::Local {
                    scope_id: "S".to_string(),
                    name: "value".to_string(),
                },
                target_place: Place::Attribute {
                    base: "self".to_string(),
                    attr: "x".to_string(),
                },
                source_id: "U_value".to_string(),
                target_id: "D_x".to_string(),
                dep_kind: "assignment".to_string(),
                span: crate::source::SourceSpan::synthetic("app/main.py", "self.x = value"),
            }],
            ..AnalysisCache::default()
        };

        write_var_dependency_dot(&cache, &path, 10).unwrap();

        let dot = std::fs::read_to_string(path).unwrap();
        assert!(dot.contains("[label=\"value\"]"));
        assert!(dot.contains("[label=\"self.x\"]"));
        assert!(dot.contains("[label=\"assignment\"]"));
    }

    #[test]
    fn var_dependency_writer_disambiguates_duplicate_local_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("variables.dot");
        let span = crate::source::SourceSpan::synthetic("app/main.py", "x = 1");
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            scopes: vec![
                ScopeRecord {
                    scope_id: "S_foo".to_string(),
                    scope_kind: "function".to_string(),
                    parent_scope_id: None,
                    owner_id: "FN_foo".to_string(),
                    span: span.clone(),
                },
                ScopeRecord {
                    scope_id: "S_bar".to_string(),
                    scope_kind: "function".to_string(),
                    parent_scope_id: None,
                    owner_id: "FN_bar".to_string(),
                    span: span.clone(),
                },
            ],
            functions: vec![
                FunctionRecord {
                    function_id: "FN_foo".to_string(),
                    module_id: "M_app".to_string(),
                    class_id: None,
                    qualified_name: "foo".to_string(),
                    kind: "function".to_string(),
                    params: Vec::new(),
                    scope_id: "S_foo".to_string(),
                    span: span.clone(),
                },
                FunctionRecord {
                    function_id: "FN_bar".to_string(),
                    module_id: "M_app".to_string(),
                    class_id: None,
                    qualified_name: "bar".to_string(),
                    kind: "function".to_string(),
                    params: Vec::new(),
                    scope_id: "S_bar".to_string(),
                    span: span.clone(),
                },
            ],
            var_dependency_edges: vec![
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_1".to_string(),
                    source_place: Place::Local {
                        scope_id: "S_foo".to_string(),
                        name: "x".to_string(),
                    },
                    target_place: Place::Attribute {
                        base: "self".to_string(),
                        attr: "left".to_string(),
                    },
                    source_id: "U_foo_x".to_string(),
                    target_id: "D_left".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: span.clone(),
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_2".to_string(),
                    source_place: Place::Local {
                        scope_id: "S_bar".to_string(),
                        name: "x".to_string(),
                    },
                    target_place: Place::Attribute {
                        base: "self".to_string(),
                        attr: "right".to_string(),
                    },
                    source_id: "U_bar_x".to_string(),
                    target_id: "D_right".to_string(),
                    dep_kind: "assignment".to_string(),
                    span,
                },
            ],
            ..AnalysisCache::default()
        };

        write_var_dependency_dot(&cache, &path, 10).unwrap();

        let dot = std::fs::read_to_string(path).unwrap();
        assert!(dot.contains("[label=\"foo::x\"]"));
        assert!(dot.contains("[label=\"bar::x\"]"));
        assert!(!dot.contains("[label=\"x\"]"));
    }

    #[test]
    fn svg_post_processor_rewrites_internal_titles_from_dot_labels() {
        let dot = concat!(
            "digraph DefUseHotspots {\n",
            "  \"D_1\" [label=\"def x\"];\n",
            "  \"U_1\" [label=\"use x\"];\n",
            "  \"D_1\" -> \"U_1\" [label=\"local\"];\n",
            "}\n"
        );
        let svg = concat!(
            "<svg>\n",
            "<title>DefUseHotspots</title>\n",
            "<!-- D_1 -->\n",
            "<title>D_1</title>\n",
            "<!-- D_1&#45;&gt;U_1 -->\n",
            "<title>D_1&#45;&gt;U_1</title>\n",
            "</svg>\n"
        );

        let rewritten = rewrite_svg_metadata(dot, svg);

        assert!(rewritten.contains("<!-- def x -->"));
        assert!(rewritten.contains("<title>def x</title>"));
        assert!(rewritten.contains("<!-- def x -> use x (local) -->"));
        assert!(rewritten.contains("<title>def x -&gt; use x (local)</title>"));
        assert!(!rewritten.contains("<title>D_1</title>"));
        assert!(!rewritten.contains("D_1&#45;&gt;U_1"));
    }
}
