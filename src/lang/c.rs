use crate::cfg::ControlFlowGraph;
use crate::ids::stable_id;
use crate::ir::{
    AnalysisCache, Definition, FunctionRecord, ModuleRecord, Place, SCHEMA_VERSION, ScopeRecord,
    SourceFileRecord, Use,
};
use crate::lang::LanguageFrontend;
use crate::source::{SourceSpan, SourceUnit};
use anyhow::{Context, Result};
use tree_sitter::{Node, Parser};

/// First-version C frontend. Parses preprocessed translation units with
/// `tree-sitter-c` and lowers a baseline subset of constructs to the shared
/// IR: function definitions and parameters, top-level globals, simple local
/// assignments, and return statements.
pub struct CFrontend;

impl Default for CFrontend {
    fn default() -> Self {
        Self::new()
    }
}

impl CFrontend {
    pub fn new() -> Self {
        Self
    }

    fn parser() -> Result<Parser> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_c::LANGUAGE.into())
            .context("failed to load tree-sitter-c")?;
        Ok(parser)
    }
}

impl LanguageFrontend for CFrontend {
    fn parse_units(&self, units: &[SourceUnit]) -> Result<AnalysisCache> {
        let mut parser = Self::parser()?;
        let mut cache = AnalysisCache {
            tool_version: env!("CARGO_PKG_VERSION").to_string(),
            ..AnalysisCache::default()
        };

        for unit in units {
            let tree = parser
                .parse(&unit.source_text, None)
                .context("c parse returned no tree")?;
            let file_id = stable_id("FILE", SCHEMA_VERSION, &[&unit.relative_path]);
            let module_id = stable_id("M", SCHEMA_VERSION, &[&unit.relative_path]);
            let module_name = unit
                .relative_path
                .trim_end_matches(".c")
                .trim_end_matches(".i")
                .replace('/', "::");

            cache.files.push(SourceFileRecord {
                file_id: file_id.clone(),
                path: unit.relative_path.clone(),
                hash: stable_id("HASH", SCHEMA_VERSION, &[&unit.source_text]),
                line_count: unit.source_text.lines().count(),
                parse_status: "ok".to_string(),
            });
            cache.modules.push(ModuleRecord {
                module_id: module_id.clone(),
                file_id: file_id.clone(),
                module_name,
                exports: Vec::new(),
                imports: Vec::new(),
            });

            lower_translation_unit(
                &unit.relative_path,
                &unit.source_text,
                tree.root_node(),
                &module_id,
                &mut cache,
            );
        }

        Ok(cache)
    }
}

fn lower_translation_unit(
    path: &str,
    text: &str,
    root: Node,
    module_id: &str,
    cache: &mut AnalysisCache,
) {
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        match child.kind() {
            "function_definition" => lower_function(path, text, module_id, child, cache),
            "declaration" => lower_global(path, text, module_id, child, cache),
            _ => {}
        }
    }
}

fn function_name_from_declarator(decl: Node, text: &str) -> Option<String> {
    let mut current = Some(decl);
    while let Some(node) = current {
        match node.kind() {
            "identifier" | "field_identifier" | "type_identifier" => {
                return node.utf8_text(text.as_bytes()).ok().map(|s| s.to_string());
            }
            "function_declarator" => {
                current = node.child_by_field_name("declarator");
            }
            "pointer_declarator" | "parenthesized_declarator" => {
                current = node.child_by_field_name("declarator");
            }
            _ => {
                current = node.child_by_field_name("declarator");
            }
        }
    }
    None
}

fn function_declarator(decl: Node) -> Option<Node> {
    let mut current = Some(decl);
    while let Some(node) = current {
        if node.kind() == "function_declarator" {
            return Some(node);
        }
        current = node.child_by_field_name("declarator");
    }
    None
}

fn extract_c_params(node: Node, text: &str) -> Vec<String> {
    let Some(declarator) = node.child_by_field_name("declarator") else {
        return Vec::new();
    };
    let Some(fn_decl) = function_declarator(declarator) else {
        return Vec::new();
    };
    let Some(param_list) = fn_decl.child_by_field_name("parameters") else {
        return Vec::new();
    };
    let mut cursor = param_list.walk();
    param_list
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "parameter_declaration")
        .filter_map(|param| param_name(param, text))
        .collect()
}

fn param_name(param: Node, text: &str) -> Option<String> {
    let mut current = param.child_by_field_name("declarator");
    while let Some(node) = current {
        match node.kind() {
            "identifier" => {
                return node.utf8_text(text.as_bytes()).ok().map(|s| s.to_string());
            }
            "pointer_declarator" | "array_declarator" | "function_declarator"
            | "parenthesized_declarator" => {
                current = node.child_by_field_name("declarator");
            }
            _ => return None,
        }
    }
    None
}

fn lower_function(path: &str, text: &str, module_id: &str, node: Node, cache: &mut AnalysisCache) {
    let span = span_for(path, text, node);
    let declarator = node.child_by_field_name("declarator");
    let function_name = declarator
        .and_then(|decl| function_name_from_declarator(decl, text))
        .unwrap_or_else(|| "anonymous".to_string());
    let function_id = stable_id(
        "F",
        SCHEMA_VERSION,
        &[module_id, &function_name, &span.snippet],
    );
    let scope_id = stable_id("S", SCHEMA_VERSION, &[&function_id, "function"]);
    let params = extract_c_params(node, text);

    cache.scopes.push(ScopeRecord {
        scope_id: scope_id.clone(),
        scope_kind: "function".to_string(),
        parent_scope_id: None,
        owner_id: function_id.clone(),
        span: span.clone(),
    });
    cache.functions.push(FunctionRecord {
        function_id: function_id.clone(),
        module_id: module_id.to_string(),
        class_id: None,
        qualified_name: function_name.clone(),
        kind: "function".to_string(),
        params: params.clone(),
        scope_id: scope_id.clone(),
        span: span.clone(),
    });

    for param in &params {
        cache.definitions.push(Definition {
            def_id: stable_id("D", SCHEMA_VERSION, &[&function_id, "param", param]),
            place: Place::Local {
                scope_id: scope_id.clone(),
                name: param.clone(),
            },
            def_kind: "param".to_string(),
            scope_id: scope_id.clone(),
            function_id: Some(function_id.clone()),
            span: span.clone(),
            expr: param.clone(),
            deps: Vec::new(),
        });
    }

    let mut cfg = ControlFlowGraph::new(function_id.clone());
    let body_block = cfg.add_block("BasicBlock", span.clone());
    let entry_id = cfg.entry_block_id.clone();
    let exit_id = cfg.exit_block_id.clone();
    cfg.add_edge(&entry_id, &body_block, "sequence", "entry");

    lower_function_body(path, text, node, &function_id, &scope_id, &body_block, &mut cfg, cache);

    cfg.add_edge(&body_block, &exit_id, "sequence", "exit");
    cache.cfgs.push(cfg.into_record());
}

fn lower_global(path: &str, text: &str, module_id: &str, node: Node, cache: &mut AnalysisCache) {
    let snippet = node
        .utf8_text(text.as_bytes())
        .unwrap_or("")
        .trim()
        .to_string();
    let Some((left, right)) = snippet.split_once('=') else {
        return;
    };
    let name = left
        .split_whitespace()
        .last()
        .unwrap_or(left)
        .trim()
        .trim_start_matches('*')
        .trim_end_matches(',')
        .trim_end_matches(';')
        .to_string();
    if name.is_empty() {
        return;
    }
    cache.definitions.push(Definition {
        def_id: stable_id("D", SCHEMA_VERSION, &[module_id, &name, "global"]),
        place: Place::Global {
            module_id: module_id.to_string(),
            name,
        },
        def_kind: "assign".to_string(),
        scope_id: module_id.to_string(),
        function_id: None,
        span: span_for(path, text, node),
        expr: right.trim().trim_end_matches(';').to_string(),
        deps: Vec::new(),
    });
}

fn lower_function_body(
    path: &str,
    text: &str,
    node: Node,
    function_id: &str,
    scope_id: &str,
    _block_id: &str,
    _cfg: &mut ControlFlowGraph,
    cache: &mut AnalysisCache,
) {
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        let snippet = child
            .utf8_text(text.as_bytes())
            .unwrap_or("")
            .trim()
            .to_string();

        if child.kind() == "declaration" && snippet.contains('=') {
            let (left, right) = snippet.split_once('=').unwrap();
            let name = left
                .split_whitespace()
                .last()
                .unwrap_or(left)
                .trim()
                .trim_start_matches('*')
                .to_string();
            if name.is_empty() {
                continue;
            }
            cache.definitions.push(Definition {
                def_id: stable_id("D", SCHEMA_VERSION, &[function_id, &name, &snippet]),
                place: Place::Local {
                    scope_id: scope_id.to_string(),
                    name,
                },
                def_kind: "assign".to_string(),
                scope_id: scope_id.to_string(),
                function_id: Some(function_id.to_string()),
                span: span_for(path, text, child),
                expr: right.trim().trim_end_matches(';').to_string(),
                deps: Vec::new(),
            });
        }

        if child.kind() == "return_statement" {
            let value = snippet
                .trim_start_matches("return")
                .trim()
                .trim_end_matches(';')
                .trim()
                .to_string();
            cache.uses.push(Use {
                use_id: stable_id("U", SCHEMA_VERSION, &[function_id, "return", &value]),
                place: Place::Local {
                    scope_id: scope_id.to_string(),
                    name: value.clone(),
                },
                use_kind: "read".to_string(),
                scope_id: scope_id.to_string(),
                function_id: Some(function_id.to_string()),
                span: span_for(path, text, child),
                context: "return value".to_string(),
            });
        }
    }
}

fn span_for(path: &str, text: &str, node: Node) -> SourceSpan {
    let start = node.start_position();
    let end = node.end_position();
    SourceSpan {
        file: path.to_string(),
        line: start.row + 1,
        col: start.column + 1,
        end_line: end.row + 1,
        end_col: end.column + 1,
        snippet: node
            .utf8_text(text.as_bytes())
            .unwrap_or("")
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string(),
    }
}
