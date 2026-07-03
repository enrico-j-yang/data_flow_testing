use super::LanguageFrontend;
use crate::ids::stable_id;
use crate::ir::{
    AnalysisCache, Diagnostic, ModuleRecord, SCHEMA_VERSION, ScopeRecord, SourceFileRecord,
};
use crate::js_source::{JavaScriptSyntax, syntax_for_unit};
use crate::source::{SourceSpan, SourceUnit};
use anyhow::{Context, Result, anyhow};
use sha2::{Digest, Sha256};
use tree_sitter::{Language, Node, Parser};

pub struct JavaScriptFrontend;

impl Default for JavaScriptFrontend {
    fn default() -> Self {
        Self::new()
    }
}

impl JavaScriptFrontend {
    pub fn new() -> Self {
        Self
    }

    fn parse_unit(&self, unit: &SourceUnit) -> Result<AnalysisCache> {
        let mut parser = parser_for_unit(unit)?;
        let tree = parser.parse(&unit.source_text, None).ok_or_else(|| {
            anyhow!(
                "tree-sitter returned no parse tree for {}",
                unit.relative_path
            )
        })?;
        let root = tree.root_node();

        let mut cache = AnalysisCache {
            tool_version: env!("CARGO_PKG_VERSION").to_string(),
            ..AnalysisCache::default()
        };
        let file_id = stable_id("F", SCHEMA_VERSION, &[&unit.relative_path]);
        let module_id = stable_id("M", SCHEMA_VERSION, &[&unit.relative_path]);
        let module_scope_id = stable_id("S", SCHEMA_VERSION, &[&module_id, "module"]);

        cache.files.push(SourceFileRecord {
            file_id: file_id.clone(),
            path: unit.relative_path.clone(),
            hash: source_hash(&unit.source_text),
            line_count: unit.source_text.lines().count(),
            parse_status: if root.has_error() { "partial" } else { "ok" }.to_string(),
        });
        cache.modules.push(ModuleRecord {
            module_id: module_id.clone(),
            file_id,
            module_name: unit.relative_path.clone(),
            exports: Vec::new(),
            imports: Vec::new(),
        });
        cache.scopes.push(ScopeRecord {
            scope_id: module_scope_id,
            scope_kind: "module".to_string(),
            parent_scope_id: None,
            owner_id: module_id,
            span: span_for(unit, root),
        });

        record_parse_errors(&mut cache, unit, root);
        Ok(cache)
    }
}

impl LanguageFrontend for JavaScriptFrontend {
    fn parse_units(&self, units: &[SourceUnit]) -> Result<AnalysisCache> {
        let mut sorted_units = units.iter().collect::<Vec<_>>();
        sorted_units.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));

        let mut cache = AnalysisCache {
            tool_version: env!("CARGO_PKG_VERSION").to_string(),
            ..AnalysisCache::default()
        };
        for unit in sorted_units {
            merge_analysis_cache(&mut cache, self.parse_unit(unit)?);
        }

        Ok(cache)
    }
}

fn parser_for_unit(unit: &SourceUnit) -> Result<Parser> {
    let language: Language = match syntax_for_unit(unit) {
        JavaScriptSyntax::JavaScript | JavaScriptSyntax::Jsx => {
            tree_sitter_javascript::LANGUAGE.into()
        }
        JavaScriptSyntax::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        JavaScriptSyntax::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
    };

    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .context("failed to load tree-sitter JavaScript-family grammar")?;
    Ok(parser)
}

fn record_parse_errors(cache: &mut AnalysisCache, unit: &SourceUnit, node: Node<'_>) {
    if node.is_error() || node.is_missing() || node.kind() == "ERROR" {
        cache.diagnostics.push(Diagnostic {
            diagnostic_id: stable_id(
                "DIAG",
                SCHEMA_VERSION,
                &[
                    &unit.relative_path,
                    "parse-error",
                    node.kind(),
                    &node.start_byte().to_string(),
                    &node.end_byte().to_string(),
                ],
            ),
            severity: "warning".to_string(),
            kind: "parse-error".to_string(),
            message: format!("tree-sitter reported invalid syntax at {}", node.kind()),
            file: unit.relative_path.clone(),
            span: span_for(unit, node),
        });
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        record_parse_errors(cache, unit, child);
    }
}

fn span_for(unit: &SourceUnit, node: Node<'_>) -> SourceSpan {
    let start = node.start_position();
    let end = node.end_position();
    let line = map_line(unit, start.row + 1);
    let end_line = map_line(unit, end.row + 1);

    SourceSpan {
        file: unit.relative_path.clone(),
        line,
        col: start.column + 1,
        end_line,
        end_col: end.column + 1,
        snippet: node
            .utf8_text(unit.source_text.as_bytes())
            .unwrap_or("")
            .lines()
            .next()
            .unwrap_or(node.kind())
            .trim()
            .to_string(),
    }
}

fn map_line(unit: &SourceUnit, generated_line: usize) -> usize {
    if let Some(marker) = unit
        .line_markers
        .iter()
        .filter(|marker| generated_line >= marker.generated_line)
        .max_by_key(|marker| marker.generated_line)
    {
        return marker.original_line + (generated_line - marker.generated_line);
    }

    generated_line
}

fn source_hash(source: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source.as_bytes());
    hex::encode(hasher.finalize())
}

fn merge_analysis_cache(target: &mut AnalysisCache, mut source: AnalysisCache) {
    target.files.append(&mut source.files);
    target.modules.append(&mut source.modules);
    target.scopes.append(&mut source.scopes);
    target.classes.append(&mut source.classes);
    target.functions.append(&mut source.functions);
    target.definitions.append(&mut source.definitions);
    target.uses.append(&mut source.uses);
    target.captures.append(&mut source.captures);
    target.calls.append(&mut source.calls);
    target.cfgs.append(&mut source.cfgs);
    target.def_use_edges.append(&mut source.def_use_edges);
    target
        .var_dependency_edges
        .append(&mut source.var_dependency_edges);
    target
        .function_summaries
        .append(&mut source.function_summaries);
    target.diagnostics.append(&mut source.diagnostics);
    target.graph_index.append(&mut source.graph_index);
}
