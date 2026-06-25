use super::LanguageFrontend;
use crate::ids::stable_id;
use crate::ir::{
    AnalysisCache, Definition, FunctionRecord, ModuleRecord, Place, SCHEMA_VERSION, ScopeRecord,
    SourceFileRecord, Use,
};
use crate::source::{SourceSpan, SourceUnit};
use anyhow::{Context, Result, anyhow};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::Path;
use tree_sitter::{Node, Parser};

#[derive(Debug, Default, Clone, Copy)]
pub struct CFrontend;

impl CFrontend {
    pub fn new() -> Self {
        Self
    }

    fn parse_single_unit(&self, unit: &SourceUnit) -> Result<AnalysisCache> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_c::LANGUAGE.into())
            .context("failed to load tree-sitter-c")?;

        let tree = parser.parse(&unit.source_text, None).ok_or_else(|| {
            anyhow!(
                "tree-sitter returned no parse tree for {}",
                unit.relative_path
            )
        })?;

        let mut cache = AnalysisCache {
            tool_version: env!("CARGO_PKG_VERSION").to_string(),
            ..AnalysisCache::default()
        };
        self.lower_translation_unit(&mut cache, unit, tree.root_node());
        Ok(cache)
    }

    fn lower_translation_unit(&self, cache: &mut AnalysisCache, file: &SourceUnit, root: Node<'_>) {
        let file_id = stable_id("F", SCHEMA_VERSION, &[&file.relative_path]);
        let module_id = stable_id("M", SCHEMA_VERSION, &[&file.relative_path]);
        let module_scope_id = stable_id("S", SCHEMA_VERSION, &[&module_id, "module"]);

        cache.files.push(SourceFileRecord {
            file_id: file_id.clone(),
            path: file.relative_path.clone(),
            hash: source_hash(&file.source_text),
            line_count: file.source_text.lines().count(),
            parse_status: if root.has_error() { "partial" } else { "ok" }.to_string(),
        });
        cache.modules.push(ModuleRecord {
            module_id: module_id.clone(),
            file_id,
            module_name: module_name_for_file(file),
            exports: Vec::new(),
            imports: Vec::new(),
        });
        cache.scopes.push(ScopeRecord {
            scope_id: module_scope_id.clone(),
            scope_kind: "module".to_string(),
            parent_scope_id: None,
            owner_id: module_id.clone(),
            span: span_for(file, &file.source_text, root),
        });

        let global_bindings = collect_global_bindings(root, &file.source_text);
        let mut cursor = root.walk();
        for child in root.named_children(&mut cursor) {
            match child.kind() {
                "function_definition" => self.lower_function(
                    cache,
                    file,
                    &module_id,
                    &module_scope_id,
                    &global_bindings,
                    child,
                ),
                "declaration" => {
                    self.lower_global(
                        cache,
                        file,
                        &module_id,
                        &module_scope_id,
                        &global_bindings,
                        child,
                    );
                }
                _ => {}
            }
        }
    }

    fn lower_function(
        &self,
        cache: &mut AnalysisCache,
        file: &SourceUnit,
        module_id: &str,
        module_scope_id: &str,
        global_bindings: &BTreeSet<String>,
        node: Node<'_>,
    ) {
        let Some(declarator) = node.child_by_field_name("declarator") else {
            return;
        };
        let Some(function_name) = extract_declarator_name(declarator, &file.source_text) else {
            return;
        };

        let qualified_name = function_name.clone();
        let function_id = stable_id(
            "FN",
            SCHEMA_VERSION,
            &[
                &file.relative_path,
                &qualified_name,
                &location_label(node.start_position().row, node.start_position().column),
            ],
        );
        let scope_id = stable_id("S", SCHEMA_VERSION, &[&function_id, "function"]);
        let params = extract_c_params(declarator, &file.source_text);

        cache.functions.push(FunctionRecord {
            function_id: function_id.clone(),
            module_id: module_id.to_string(),
            class_id: None,
            qualified_name,
            kind: "function".to_string(),
            params: params.iter().map(|param| param.name.clone()).collect(),
            scope_id: scope_id.clone(),
            span: span_for(file, &file.source_text, node),
        });
        cache.scopes.push(ScopeRecord {
            scope_id: scope_id.clone(),
            scope_kind: "function".to_string(),
            parent_scope_id: Some(module_scope_id.to_string()),
            owner_id: function_id.clone(),
            span: span_for(file, &file.source_text, node),
        });

        for (index, param) in params.iter().enumerate() {
            cache.definitions.push(Definition {
                def_id: stable_id(
                    "D",
                    SCHEMA_VERSION,
                    &[
                        &file.relative_path,
                        &function_id,
                        "param",
                        &param.name,
                        &index.to_string(),
                    ],
                ),
                place: Place::Local {
                    scope_id: scope_id.clone(),
                    name: param.name.clone(),
                },
                def_kind: "param".to_string(),
                scope_id: scope_id.clone(),
                function_id: Some(function_id.clone()),
                span: span_for(file, &file.source_text, param.span_node),
                expr: String::new(),
                deps: Vec::new(),
            });
        }

        if let Some(body) = node.child_by_field_name("body") {
            let mut local_bindings = params.iter().map(|param| param.name.clone()).collect();
            let function_ctx = FunctionContext {
                function_id: &function_id,
                module_id,
                scope_id: &scope_id,
                global_bindings,
            };
            self.lower_function_body(cache, file, &function_ctx, &mut local_bindings, body);
        }
    }

    fn lower_global(
        &self,
        cache: &mut AnalysisCache,
        file: &SourceUnit,
        module_id: &str,
        module_scope_id: &str,
        global_bindings: &BTreeSet<String>,
        node: Node<'_>,
    ) {
        for declarator in declaration_initializers(node) {
            let Some(name) = extract_init_declarator_name(declarator, &file.source_text) else {
                continue;
            };
            let Some(value) = declarator.child_by_field_name("value") else {
                continue;
            };

            cache.definitions.push(Definition {
                def_id: stable_id(
                    "D",
                    SCHEMA_VERSION,
                    &[
                        &file.relative_path,
                        module_id,
                        "assign",
                        &name,
                        &location_label(
                            declarator.start_position().row,
                            declarator.start_position().column,
                        ),
                    ],
                ),
                place: Place::Global {
                    module_id: module_id.to_string(),
                    name,
                },
                def_kind: "assign".to_string(),
                scope_id: module_scope_id.to_string(),
                function_id: None,
                span: span_for(file, &file.source_text, declarator),
                expr: node_text(&file.source_text, value),
                deps: collect_expression_places(
                    value,
                    &file.source_text,
                    module_id,
                    None,
                    &BTreeSet::new(),
                    global_bindings,
                ),
            });
        }
    }

    fn lower_function_body(
        &self,
        cache: &mut AnalysisCache,
        file: &SourceUnit,
        function_ctx: &FunctionContext<'_>,
        local_bindings: &mut BTreeSet<String>,
        node: Node<'_>,
    ) {
        match node.kind() {
            "declaration" => {
                for declarator in declaration_initializers(node) {
                    let Some(name) = extract_init_declarator_name(declarator, &file.source_text)
                    else {
                        continue;
                    };
                    let Some(value) = declarator.child_by_field_name("value") else {
                        continue;
                    };

                    let deps = collect_expression_places(
                        value,
                        &file.source_text,
                        function_ctx.module_id,
                        Some(function_ctx.scope_id),
                        local_bindings,
                        function_ctx.global_bindings,
                    );
                    cache.definitions.push(Definition {
                        def_id: stable_id(
                            "D",
                            SCHEMA_VERSION,
                            &[
                                &file.relative_path,
                                function_ctx.function_id,
                                "assign",
                                &name,
                                &location_label(
                                    declarator.start_position().row,
                                    declarator.start_position().column,
                                ),
                            ],
                        ),
                        place: Place::Local {
                            scope_id: function_ctx.scope_id.to_string(),
                            name: name.clone(),
                        },
                        def_kind: "assign".to_string(),
                        scope_id: function_ctx.scope_id.to_string(),
                        function_id: Some(function_ctx.function_id.to_string()),
                        span: span_for(file, &file.source_text, declarator),
                        expr: node_text(&file.source_text, value),
                        deps,
                    });
                    local_bindings.insert(name);
                }
            }
            "return_statement" => {
                let Some(value) = first_named_child(node) else {
                    return;
                };
                cache.uses.push(Use {
                    use_id: stable_id(
                        "U",
                        SCHEMA_VERSION,
                        &[
                            &file.relative_path,
                            function_ctx.function_id,
                            "return",
                            &location_label(
                                value.start_position().row,
                                value.start_position().column,
                            ),
                        ],
                    ),
                    place: resolve_identifier_place(
                        value,
                        &file.source_text,
                        function_ctx.module_id,
                        Some(function_ctx.scope_id),
                        local_bindings,
                        function_ctx.global_bindings,
                    )
                    .unwrap_or_else(|| Place::Unknown {
                        reason: node_text(&file.source_text, value),
                    }),
                    use_kind: "load".to_string(),
                    scope_id: function_ctx.scope_id.to_string(),
                    function_id: Some(function_ctx.function_id.to_string()),
                    span: span_for(file, &file.source_text, value),
                    context: "return value".to_string(),
                });
            }
            _ => {
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    self.lower_function_body(cache, file, function_ctx, local_bindings, child);
                }
            }
        }
    }
}

impl LanguageFrontend for CFrontend {
    fn parse_units(&self, units: &[SourceUnit]) -> Result<AnalysisCache> {
        let mut partials = Vec::with_capacity(units.len());
        for unit in units {
            let cache = self.parse_single_unit(unit)?;
            let key = cache
                .files
                .first()
                .map(|record| record.path.clone())
                .unwrap_or_else(|| unit.relative_path.clone());
            partials.push((key, cache));
        }
        partials.sort_by(|left, right| left.0.cmp(&right.0));

        let mut cache = AnalysisCache {
            tool_version: env!("CARGO_PKG_VERSION").to_string(),
            ..AnalysisCache::default()
        };
        for (_, partial) in partials {
            merge_analysis_cache(&mut cache, partial);
        }

        Ok(cache)
    }
}

struct FunctionContext<'a> {
    function_id: &'a str,
    module_id: &'a str,
    scope_id: &'a str,
    global_bindings: &'a BTreeSet<String>,
}

struct CParam<'tree> {
    name: String,
    span_node: Node<'tree>,
}

fn extract_c_params<'tree>(declarator: Node<'tree>, source: &str) -> Vec<CParam<'tree>> {
    let Some(function_declarator) = find_function_declarator(declarator) else {
        return Vec::new();
    };
    let Some(parameters) = function_declarator.child_by_field_name("parameters") else {
        return Vec::new();
    };

    let mut params = Vec::new();
    let mut cursor = parameters.walk();
    for child in parameters.named_children(&mut cursor) {
        if child.kind() != "parameter_declaration" {
            continue;
        }
        let Some(declarator) = child.child_by_field_name("declarator") else {
            continue;
        };
        let Some(name) = extract_declarator_name(declarator, source) else {
            continue;
        };
        params.push(CParam {
            name,
            span_node: declarator,
        });
    }
    params
}

fn collect_global_bindings(node: Node<'_>, source: &str) -> BTreeSet<String> {
    let mut bindings = BTreeSet::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "declaration" {
            continue;
        }
        for declarator in declaration_initializers(child) {
            if let Some(name) = extract_init_declarator_name(declarator, source) {
                bindings.insert(name);
            }
        }
    }
    bindings
}

fn declaration_initializers<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
    let mut declarators = Vec::new();
    let mut cursor = node.walk();
    for child in node.children_by_field_name("declarator", &mut cursor) {
        if child.kind() == "init_declarator" {
            declarators.push(child);
        }
    }
    declarators
}

fn find_function_declarator(node: Node<'_>) -> Option<Node<'_>> {
    if node.kind() == "function_declarator" {
        return Some(node);
    }

    node.child_by_field_name("declarator")
        .and_then(find_function_declarator)
}

fn extract_init_declarator_name(node: Node<'_>, source: &str) -> Option<String> {
    let declarator = node.child_by_field_name("declarator")?;
    extract_declarator_name(declarator, source)
}

fn extract_declarator_name(node: Node<'_>, source: &str) -> Option<String> {
    if node.kind() == "identifier" {
        return Some(node_text(source, node));
    }

    node.child_by_field_name("declarator")
        .and_then(|child| extract_declarator_name(child, source))
}

fn collect_expression_places(
    node: Node<'_>,
    source: &str,
    module_id: &str,
    function_scope_id: Option<&str>,
    local_bindings: &BTreeSet<String>,
    global_bindings: &BTreeSet<String>,
) -> Vec<Place> {
    let mut seen = BTreeSet::new();
    collect_expression_places_into(
        node,
        source,
        module_id,
        function_scope_id,
        local_bindings,
        global_bindings,
        &mut seen,
    );
    seen.into_iter().collect()
}

fn collect_expression_places_into(
    node: Node<'_>,
    source: &str,
    module_id: &str,
    function_scope_id: Option<&str>,
    local_bindings: &BTreeSet<String>,
    global_bindings: &BTreeSet<String>,
    seen: &mut BTreeSet<Place>,
) {
    if let Some(place) = resolve_identifier_place(
        node,
        source,
        module_id,
        function_scope_id,
        local_bindings,
        global_bindings,
    ) {
        seen.insert(place);
        return;
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_expression_places_into(
            child,
            source,
            module_id,
            function_scope_id,
            local_bindings,
            global_bindings,
            seen,
        );
    }
}

fn resolve_identifier_place(
    node: Node<'_>,
    source: &str,
    module_id: &str,
    function_scope_id: Option<&str>,
    local_bindings: &BTreeSet<String>,
    global_bindings: &BTreeSet<String>,
) -> Option<Place> {
    if node.kind() != "identifier" {
        return None;
    }

    let name = node_text(source, node);
    if let Some(scope_id) = function_scope_id {
        if local_bindings.contains(&name) {
            return Some(Place::Local {
                scope_id: scope_id.to_string(),
                name,
            });
        }
    }

    if global_bindings.contains(&name) {
        return Some(Place::Global {
            module_id: module_id.to_string(),
            name,
        });
    }

    Some(Place::Unknown { reason: name })
}

fn first_named_child(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).next()
}

fn location_label(row: usize, column: usize) -> String {
    format!("{}:{}", row + 1, column + 1)
}

fn node_text(source: &str, node: Node<'_>) -> String {
    node.utf8_text(source.as_bytes())
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn span_for(file: &SourceUnit, source: &str, node: Node<'_>) -> SourceSpan {
    let start = node.start_position();
    let end = node.end_position();

    SourceSpan {
        file: file.relative_path.clone(),
        line: start.row + 1,
        col: start.column + 1,
        end_line: end.row + 1,
        end_col: end.column + 1,
        snippet: node_text(source, node),
    }
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

fn module_name_for_file(file: &SourceUnit) -> String {
    Path::new(&file.relative_path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or(&file.relative_path)
        .to_string()
}
