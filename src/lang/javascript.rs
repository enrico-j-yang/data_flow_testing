use super::LanguageFrontend;
use crate::cfg::ControlFlowGraph;
use crate::ids::stable_id;
use crate::ir::{
    AnalysisCache, ClassRecord, Definition, Diagnostic, FunctionRecord, ImportRecord, ModuleRecord,
    Place, ScopeRecord, SourceFileRecord, Use, SCHEMA_VERSION,
};
use crate::js_source::{syntax_for_unit, JavaScriptSyntax};
use crate::source::{SourceSpan, SourceUnit};
use anyhow::{anyhow, Context, Result};
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
        let module_index = cache.modules.len();

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
            scope_id: module_scope_id.clone(),
            scope_kind: "module".to_string(),
            parent_scope_id: None,
            owner_id: module_id.clone(),
            span: span_for(unit, root),
        });

        record_parse_errors(&mut cache, unit, root);
        lower_program(
            &mut cache,
            unit,
            root,
            &module_id,
            &module_scope_id,
            module_index,
        );
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

fn lower_program(
    cache: &mut AnalysisCache,
    unit: &SourceUnit,
    root: Node<'_>,
    module_id: &str,
    module_scope_id: &str,
    module_index: usize,
) {
    let mut class_stack = Vec::new();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        lower_toplevel_node(
            cache,
            unit,
            child,
            module_id,
            module_scope_id,
            module_index,
            &mut class_stack,
        );
    }
}

fn lower_toplevel_node(
    cache: &mut AnalysisCache,
    unit: &SourceUnit,
    node: Node<'_>,
    module_id: &str,
    module_scope_id: &str,
    module_index: usize,
    class_stack: &mut Vec<String>,
) {
    match node.kind() {
        "import_statement" => {
            lower_import_statement(cache, unit, node, module_scope_id, module_index)
        }
        "export_statement" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                lower_toplevel_node(
                    cache,
                    unit,
                    child,
                    module_id,
                    module_scope_id,
                    module_index,
                    class_stack,
                );
            }
        }
        "function_declaration" => {
            lower_function_node(
                cache,
                unit,
                node,
                function_name(node, unit).unwrap_or_default(),
                "function",
                module_id,
                module_scope_id,
                None,
                class_stack,
            );
        }
        "lexical_declaration" | "variable_declaration" => {
            lower_variable_declaration(cache, unit, node, module_id, module_scope_id, class_stack)
        }
        "class_declaration" => {
            lower_class_declaration(cache, unit, node, module_id, module_scope_id, class_stack)
        }
        _ => {}
    }
}

fn lower_import_statement(
    cache: &mut AnalysisCache,
    unit: &SourceUnit,
    node: Node<'_>,
    module_scope_id: &str,
    module_index: usize,
) {
    if has_direct_child_kind(node, "type") {
        return;
    }
    let Some(module_node) = direct_named_child(node, &["string"]) else {
        return;
    };
    let module = string_literal_value(module_node, unit);
    if module.is_empty() {
        return;
    }

    let mut imports = Vec::new();
    let mut definitions = Vec::new();
    if let Some(clause) = direct_named_child(node, &["import_clause"]) {
        let mut cursor = clause.walk();
        for child in clause.named_children(&mut cursor) {
            match child.kind() {
                "identifier" => {
                    imports.push(import_record(
                        unit,
                        &module,
                        Some("default"),
                        Some(text(child, unit)),
                        child,
                    ));
                    definitions.push(import_definition(
                        unit,
                        module_scope_id,
                        text(child, unit),
                        &module,
                        "default",
                        child,
                    ));
                }
                "named_imports" => lower_named_imports(
                    unit,
                    child,
                    module_scope_id,
                    &module,
                    &mut imports,
                    &mut definitions,
                ),
                "namespace_import" => {
                    if let Some(alias) = first_named_child(child, &["identifier"]) {
                        let alias_text = text(alias, unit);
                        imports.push(import_record(
                            unit,
                            &module,
                            Some("*"),
                            Some(alias_text),
                            child,
                        ));
                        definitions.push(import_definition(
                            unit,
                            module_scope_id,
                            alias_text,
                            &module,
                            "*",
                            child,
                        ));
                    }
                }
                _ => {}
            }
        }
    } else {
        imports.push(import_record(unit, &module, None, None, node));
    }

    if let Some(module_record) = cache.modules.get_mut(module_index) {
        module_record.imports.extend(imports);
    }
    cache.definitions.extend(definitions);
}

fn lower_named_imports(
    unit: &SourceUnit,
    node: Node<'_>,
    module_scope_id: &str,
    module: &str,
    imports: &mut Vec<ImportRecord>,
    definitions: &mut Vec<Definition>,
) {
    let mut cursor = node.walk();
    for specifier in node
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "import_specifier")
    {
        let names = named_descendants(specifier, unit, &["identifier", "property_identifier"]);
        let Some(imported) = names.first().copied() else {
            continue;
        };
        let local = names.get(1).copied().unwrap_or(imported);
        imports.push(import_record(
            unit,
            module,
            Some(imported),
            Some(local),
            specifier,
        ));
        definitions.push(import_definition(
            unit,
            module_scope_id,
            local,
            module,
            imported,
            specifier,
        ));
    }
}

fn import_record(
    unit: &SourceUnit,
    module: &str,
    name: Option<&str>,
    alias: Option<&str>,
    node: Node<'_>,
) -> ImportRecord {
    ImportRecord {
        import_id: stable_id(
            "I",
            SCHEMA_VERSION,
            &[
                &unit.relative_path,
                module,
                name.unwrap_or(""),
                alias.unwrap_or(""),
                &node.start_byte().to_string(),
            ],
        ),
        module: module.to_string(),
        name: name.map(str::to_string),
        alias: alias.map(str::to_string),
        level: 0,
        resolution: "external".to_string(),
        span: span_for(unit, node),
    }
}

fn import_definition(
    unit: &SourceUnit,
    module_scope_id: &str,
    local_name: &str,
    module: &str,
    imported_name: &str,
    node: Node<'_>,
) -> Definition {
    Definition {
        def_id: stable_id(
            "D",
            SCHEMA_VERSION,
            &[
                &unit.relative_path,
                local_name,
                module,
                imported_name,
                "import",
            ],
        ),
        place: Place::Local {
            scope_id: module_scope_id.to_string(),
            name: local_name.to_string(),
        },
        def_kind: "import".to_string(),
        scope_id: module_scope_id.to_string(),
        function_id: None,
        span: span_for(unit, node),
        expr: format!("{module}:{imported_name}"),
        deps: vec![Place::External {
            name: format!("{module}:{imported_name}"),
        }],
    }
}

fn lower_variable_declaration(
    cache: &mut AnalysisCache,
    unit: &SourceUnit,
    node: Node<'_>,
    module_id: &str,
    module_scope_id: &str,
    class_stack: &mut Vec<String>,
) {
    let mut cursor = node.walk();
    for declarator in node
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "variable_declarator")
    {
        let Some(name_node) = declarator
            .child_by_field_name("name")
            .or_else(|| direct_named_child(declarator, &["identifier"]))
        else {
            continue;
        };
        let Some(value_node) = declarator
            .child_by_field_name("value")
            .or_else(|| direct_named_child(declarator, &["arrow_function", "function_expression"]))
        else {
            continue;
        };
        if matches!(value_node.kind(), "arrow_function" | "function_expression") {
            lower_function_node(
                cache,
                unit,
                value_node,
                text(name_node, unit).to_string(),
                if value_node.kind() == "arrow_function" {
                    "arrow"
                } else {
                    "function"
                },
                module_id,
                module_scope_id,
                None,
                class_stack,
            );
        }
    }
}

fn lower_class_declaration(
    cache: &mut AnalysisCache,
    unit: &SourceUnit,
    node: Node<'_>,
    module_id: &str,
    module_scope_id: &str,
    class_stack: &mut Vec<String>,
) {
    let Some(name_node) = node
        .child_by_field_name("name")
        .or_else(|| direct_named_child(node, &["type_identifier", "identifier"]))
    else {
        return;
    };
    let class_name = text(name_node, unit).to_string();
    let qualified_name = format!("{}::{}", unit.relative_path, class_name);
    let class_id = stable_id("C", SCHEMA_VERSION, &[&unit.relative_path, &class_name]);
    let mut method_ids = Vec::new();
    class_stack.push(class_name);

    if let Some(body) = direct_named_child(node, &["class_body"]) {
        let mut cursor = body.walk();
        for method in body
            .named_children(&mut cursor)
            .filter(|child| child.kind() == "method_definition")
        {
            let method_name = method
                .child_by_field_name("name")
                .or_else(|| direct_named_child(method, &["property_identifier", "identifier"]))
                .map(|name| text(name, unit).to_string())
                .unwrap_or_default();
            let method_id = lower_function_node(
                cache,
                unit,
                method,
                method_name,
                "method",
                module_id,
                module_scope_id,
                Some(&class_id),
                class_stack,
            );
            method_ids.push(method_id);
        }
    }

    class_stack.pop();
    cache.classes.push(ClassRecord {
        class_id,
        module_id: module_id.to_string(),
        qualified_name,
        base_exprs: Vec::new(),
        resolved_bases: Vec::new(),
        mro_status: "not_applicable".to_string(),
        methods: method_ids,
        span: span_for(unit, node),
    });
}

fn lower_function_node(
    cache: &mut AnalysisCache,
    unit: &SourceUnit,
    node: Node<'_>,
    name: String,
    kind: &str,
    module_id: &str,
    parent_scope_id: &str,
    class_id: Option<&str>,
    class_stack: &[String],
) -> String {
    let span = span_for(unit, node);
    let qualified_name = qualified_name(unit, class_stack, &name, &span);
    let function_id = stable_id(
        "F",
        SCHEMA_VERSION,
        &[
            &unit.relative_path,
            &qualified_name,
            &span.line.to_string(),
            &span.col.to_string(),
        ],
    );
    let scope_id = stable_id("S", SCHEMA_VERSION, &[&function_id, "function"]);
    let params_node = node
        .child_by_field_name("parameters")
        .or_else(|| direct_named_child(node, &["formal_parameters"]));
    let params = params_node
        .map(|params| parameter_names(params, unit))
        .unwrap_or_default();

    cache.functions.push(FunctionRecord {
        function_id: function_id.clone(),
        module_id: module_id.to_string(),
        class_id: class_id.map(str::to_string),
        qualified_name,
        kind: kind.to_string(),
        params: params.clone(),
        scope_id: scope_id.clone(),
        span: span.clone(),
    });
    cache.scopes.push(ScopeRecord {
        scope_id: scope_id.clone(),
        scope_kind: "function".to_string(),
        parent_scope_id: Some(parent_scope_id.to_string()),
        owner_id: function_id.clone(),
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

    let body = node
        .child_by_field_name("body")
        .or_else(|| direct_named_child(node, &["statement_block"]))
        .unwrap_or(node);
    push_baseline_cfg(cache, &function_id, unit, body);
    lower_return_uses(cache, unit, body, &function_id, &scope_id);
    function_id
}

fn qualified_name(
    unit: &SourceUnit,
    class_stack: &[String],
    name: &str,
    span: &SourceSpan,
) -> String {
    if let Some(class_name) = class_stack.last() {
        format!("{}::{}.{}", unit.relative_path, class_name, name)
    } else if name.is_empty() {
        format!(
            "{}::<anonymous@{}:{}>",
            unit.relative_path, span.line, span.col
        )
    } else {
        format!("{}::{}", unit.relative_path, name)
    }
}

fn parameter_names(params: Node<'_>, unit: &SourceUnit) -> Vec<String> {
    let mut names = Vec::new();
    let mut cursor = params.walk();
    for child in params.named_children(&mut cursor) {
        match child.kind() {
            "identifier" => names.push(text(child, unit).to_string()),
            "required_parameter" | "optional_parameter" => {
                if let Some(identifier) = first_named_child(child, &["identifier"]) {
                    names.push(text(identifier, unit).to_string());
                }
            }
            _ => {}
        }
    }
    names
}

fn push_baseline_cfg(
    cache: &mut AnalysisCache,
    function_id: &str,
    unit: &SourceUnit,
    body: Node<'_>,
) {
    let mut cfg = ControlFlowGraph::new(function_id.to_string());
    let body_block = cfg.add_block("BasicBlock", span_for(unit, body));
    let entry_id = cfg.entry_block_id.clone();
    let exit_id = cfg.exit_block_id.clone();
    cfg.add_edge(&entry_id, &body_block, "sequence", "body");
    cfg.add_edge(&body_block, &exit_id, "sequence", "exit");
    cache.cfgs.push(cfg.into_record());
}

fn lower_return_uses(
    cache: &mut AnalysisCache,
    unit: &SourceUnit,
    node: Node<'_>,
    function_id: &str,
    scope_id: &str,
) {
    if node.kind() == "return_statement" {
        let value = first_return_value(node);
        let value_text = value.map(|value| text(value, unit).trim()).unwrap_or("");
        cache.uses.push(Use {
            use_id: stable_id(
                "U",
                SCHEMA_VERSION,
                &[
                    function_id,
                    "return",
                    value_text,
                    &node.start_byte().to_string(),
                ],
            ),
            place: value
                .map(|value| place_for_expression(value, unit, scope_id))
                .unwrap_or_else(|| Place::Unknown {
                    reason: "empty return".to_string(),
                }),
            use_kind: "read".to_string(),
            scope_id: scope_id.to_string(),
            function_id: Some(function_id.to_string()),
            span: span_for(unit, node),
            context: "return value".to_string(),
        });
        return;
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        lower_return_uses(cache, unit, child, function_id, scope_id);
    }
}

fn first_return_value(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).next()
}

fn place_for_expression(node: Node<'_>, unit: &SourceUnit, scope_id: &str) -> Place {
    match node.kind() {
        "identifier" => Place::Local {
            scope_id: scope_id.to_string(),
            name: text(node, unit).to_string(),
        },
        "call_expression" => node
            .named_child(0)
            .map(|callee| place_for_expression(callee, unit, scope_id))
            .unwrap_or_else(|| Place::Unknown {
                reason: "call".to_string(),
            }),
        _ => Place::Unknown {
            reason: node.kind().to_string(),
        },
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

fn function_name(node: Node<'_>, unit: &SourceUnit) -> Option<String> {
    node.child_by_field_name("name")
        .or_else(|| direct_named_child(node, &["identifier"]))
        .map(|name| text(name, unit).to_string())
}

fn direct_named_child<'a>(node: Node<'a>, kinds: &[&str]) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| kinds.contains(&child.kind()))
}

fn first_named_child<'a>(node: Node<'a>, kinds: &[&str]) -> Option<Node<'a>> {
    if kinds.contains(&node.kind()) {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(found) = first_named_child(child, kinds) {
            return Some(found);
        }
    }
    None
}

fn named_descendants<'a>(node: Node<'a>, unit: &'a SourceUnit, kinds: &[&str]) -> Vec<&'a str> {
    let mut names = Vec::new();
    collect_named_descendants(node, unit, kinds, &mut names);
    names
}

fn collect_named_descendants<'a>(
    node: Node<'a>,
    unit: &'a SourceUnit,
    kinds: &[&str],
    names: &mut Vec<&'a str>,
) {
    if kinds.contains(&node.kind()) {
        names.push(text(node, unit));
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_named_descendants(child, unit, kinds, names);
    }
}

fn has_direct_child_kind(node: Node<'_>, kind: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|child| child.kind() == kind)
}

fn string_literal_value<'a>(node: Node<'a>, unit: &'a SourceUnit) -> String {
    first_named_child(node, &["string_fragment"])
        .map(|fragment| text(fragment, unit).to_string())
        .unwrap_or_else(|| {
            text(node, unit)
                .trim_matches('"')
                .trim_matches('\'')
                .to_string()
        })
}

fn text<'a>(node: Node<'_>, unit: &'a SourceUnit) -> &'a str {
    node.utf8_text(unit.source_text.as_bytes()).unwrap_or("")
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
