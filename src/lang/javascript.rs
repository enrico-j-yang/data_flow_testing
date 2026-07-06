use super::LanguageFrontend;
use crate::cfg::ControlFlowGraph;
use crate::ids::stable_id;
use crate::ir::{
    AnalysisCache, CallRecord, ClassRecord, Definition, Diagnostic, FunctionRecord, ImportRecord,
    ModuleRecord, Place, SCHEMA_VERSION, ScopeRecord, SourceFileRecord, Use,
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
            record_exported_bindings(cache, unit, node, module_id, module_scope_id, module_index);
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
        "lexical_declaration" | "variable_declaration" => lower_variable_declaration(
            cache,
            unit,
            node,
            module_id,
            module_scope_id,
            None,
            class_stack,
        ),
        "class_declaration" => {
            lower_class_declaration(cache, unit, node, module_id, module_scope_id, class_stack)
        }
        _ => {}
    }
}

fn record_exported_bindings(
    cache: &mut AnalysisCache,
    unit: &SourceUnit,
    node: Node<'_>,
    module_id: &str,
    module_scope_id: &str,
    module_index: usize,
) {
    let exports = exported_bindings(unit, node);
    if let Some(module_record) = cache.modules.get_mut(module_index) {
        for export in exports.iter().map(|export| export.exported_name.as_str()) {
            if !module_record
                .exports
                .iter()
                .any(|existing| existing == export)
            {
                module_record.exports.push(export.to_string());
            }
        }
    }
    for export in exports {
        cache.definitions.push(Definition {
            def_id: stable_id(
                "D",
                SCHEMA_VERSION,
                &[
                    &unit.relative_path,
                    module_id,
                    "export",
                    &export.exported_name,
                    &export.node.start_byte().to_string(),
                ],
            ),
            place: Place::Global {
                module_id: module_id.to_string(),
                name: export.exported_name,
            },
            def_kind: "export".to_string(),
            scope_id: module_scope_id.to_string(),
            function_id: None,
            span: span_for(unit, export.node),
            expr: export.local_name.clone(),
            deps: if export.local_name.is_empty() {
                Vec::new()
            } else {
                vec![Place::Local {
                    scope_id: module_scope_id.to_string(),
                    name: export.local_name,
                }]
            },
        });
    }
}

struct ExportBinding<'a> {
    exported_name: String,
    local_name: String,
    node: Node<'a>,
}

fn exported_bindings<'a>(unit: &SourceUnit, node: Node<'a>) -> Vec<ExportBinding<'a>> {
    let is_default = has_direct_child_kind(node, "default");
    let mut bindings = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "lexical_declaration" | "variable_declaration" => {
                collect_variable_export_bindings(unit, child, &mut bindings);
            }
            "function_declaration" | "class_declaration" => {
                let local_name = declaration_name(child, unit).unwrap_or_default();
                bindings.push(ExportBinding {
                    exported_name: if is_default {
                        "default".to_string()
                    } else {
                        local_name.clone()
                    },
                    local_name,
                    node: child,
                });
            }
            "export_clause" => {
                collect_export_clause_bindings(unit, child, &mut bindings);
            }
            _ => {}
        }
    }
    bindings.retain(|binding| !binding.exported_name.is_empty());
    bindings
}

fn collect_variable_export_bindings<'a>(
    unit: &SourceUnit,
    node: Node<'a>,
    bindings: &mut Vec<ExportBinding<'a>>,
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
        for name in binding_names(name_node, unit) {
            bindings.push(ExportBinding {
                exported_name: name.clone(),
                local_name: name,
                node: name_node,
            });
        }
    }
}

fn collect_export_clause_bindings<'a>(
    unit: &SourceUnit,
    node: Node<'a>,
    bindings: &mut Vec<ExportBinding<'a>>,
) {
    let mut cursor = node.walk();
    for specifier in node
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "export_specifier")
    {
        let names = named_descendants(specifier, unit, &["identifier", "property_identifier"]);
        let Some(local) = names.first().copied() else {
            continue;
        };
        let exported = names.get(1).copied().unwrap_or(local);
        bindings.push(ExportBinding {
            exported_name: exported.to_string(),
            local_name: local.to_string(),
            node: specifier,
        });
    }
}

fn declaration_name(node: Node<'_>, unit: &SourceUnit) -> Option<String> {
    node.child_by_field_name("name")
        .or_else(|| direct_named_child(node, &["identifier", "type_identifier"]))
        .map(|name| text(name, unit).to_string())
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
    scope_id: &str,
    function_id: Option<&str>,
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
        let value_node = declarator
            .child_by_field_name("value")
            .or_else(|| direct_named_child(declarator, &["arrow_function", "function_expression"]));
        let bindings = binding_names(name_node, unit);
        let macro_kind = value_node.and_then(|value| vue_macro_def_kind(value, unit));
        let mut first_def_id = None;
        let mut deps = Vec::new();
        if let Some(value) = value_node {
            let uses = collect_expression_uses(unit, value, function_id, scope_id, "assign:rhs");
            deps = uses.iter().map(|use_site| use_site.place.clone()).collect();
            cache.uses.extend(uses);
        }

        for binding in &bindings {
            let def_id = stable_id(
                "D",
                SCHEMA_VERSION,
                &[
                    &unit.relative_path,
                    scope_id,
                    function_id.unwrap_or("<module>"),
                    binding,
                    &declarator.start_byte().to_string(),
                ],
            );
            if first_def_id.is_none() {
                first_def_id = Some(def_id.clone());
            }
            cache.definitions.push(Definition {
                def_id,
                place: Place::Local {
                    scope_id: scope_id.to_string(),
                    name: binding.clone(),
                },
                def_kind: macro_kind.unwrap_or("assign").to_string(),
                scope_id: scope_id.to_string(),
                function_id: function_id.map(str::to_string),
                span: span_for(unit, declarator),
                expr: value_node
                    .map(|value| text(value, unit).trim().to_string())
                    .unwrap_or_default(),
                deps: deps.clone(),
            });
        }

        if let Some(value_node) = value_node {
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
                    scope_id,
                    None,
                    class_stack,
                );
            } else {
                lower_calls_in_expression(
                    cache,
                    unit,
                    value_node,
                    function_id,
                    scope_id,
                    first_def_id,
                );
            }
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
    lower_statement_tree(
        cache,
        unit,
        body,
        module_id,
        &function_id,
        &scope_id,
        &mut class_stack.to_vec(),
    );
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

fn lower_statement_tree(
    cache: &mut AnalysisCache,
    unit: &SourceUnit,
    node: Node<'_>,
    module_id: &str,
    function_id: &str,
    scope_id: &str,
    class_stack: &mut Vec<String>,
) {
    match node.kind() {
        "lexical_declaration" | "variable_declaration" => {
            lower_variable_declaration(
                cache,
                unit,
                node,
                module_id,
                scope_id,
                Some(function_id),
                class_stack,
            );
            return;
        }
        "expression_statement" => {
            if let Some(expr) = node.named_child(0) {
                lower_statement_tree(
                    cache,
                    unit,
                    expr,
                    module_id,
                    function_id,
                    scope_id,
                    class_stack,
                );
            }
            return;
        }
        "assignment_expression" | "augmented_assignment_expression" => {
            lower_assignment_expression(cache, unit, node, function_id, scope_id);
            return;
        }
        "if_statement" | "while_statement" | "for_statement" | "for_in_statement" => {
            lower_condition_uses(cache, unit, node, function_id, scope_id);
        }
        "return_statement" => {
            lower_return_statement(
                cache,
                unit,
                node,
                module_id,
                function_id,
                scope_id,
                class_stack,
            );
            return;
        }
        "call_expression" => {
            if is_define_expose_call(node, unit) {
                if let Some(arguments) = direct_named_child(node, &["arguments"]) {
                    let uses = collect_expression_uses(
                        unit,
                        arguments,
                        Some(function_id),
                        scope_id,
                        "vue-expose",
                    );
                    cache.uses.extend(uses);
                }
            } else if !is_vue_compiler_macro_call(node, unit) {
                let uses =
                    collect_expression_uses(unit, node, Some(function_id), scope_id, "call:arg");
                cache.uses.extend(uses);
                lower_calls_in_expression(cache, unit, node, Some(function_id), scope_id, None);
            }
            return;
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        lower_statement_tree(
            cache,
            unit,
            child,
            module_id,
            function_id,
            scope_id,
            class_stack,
        );
    }
}

fn lower_assignment_expression(
    cache: &mut AnalysisCache,
    unit: &SourceUnit,
    node: Node<'_>,
    function_id: &str,
    scope_id: &str,
) {
    let Some(left) = node
        .child_by_field_name("left")
        .or_else(|| node.named_child(0))
    else {
        return;
    };
    let right = node
        .child_by_field_name("right")
        .or_else(|| node.named_child(1));
    let mut deps = Vec::new();
    if node.kind() == "augmented_assignment_expression" {
        let uses = collect_expression_uses(unit, left, Some(function_id), scope_id, "assign:lhs");
        deps.extend(uses.iter().map(|use_site| use_site.place.clone()));
        cache.uses.extend(uses);
    }
    if let Some(right) = right {
        let uses = collect_expression_uses(unit, right, Some(function_id), scope_id, "assign:rhs");
        deps.extend(uses.iter().map(|use_site| use_site.place.clone()));
        cache.uses.extend(uses);
    }
    let def_id = stable_id(
        "D",
        SCHEMA_VERSION,
        &[function_id, "assign", &node.start_byte().to_string()],
    );
    cache.definitions.push(Definition {
        def_id: def_id.clone(),
        place: place_for_expression(left, unit, scope_id),
        def_kind: "assign".to_string(),
        scope_id: scope_id.to_string(),
        function_id: Some(function_id.to_string()),
        span: span_for(unit, node),
        expr: right
            .map(|right| text(right, unit).trim().to_string())
            .unwrap_or_default(),
        deps,
    });
    if let Some(right) = right {
        lower_calls_in_expression(
            cache,
            unit,
            right,
            Some(function_id),
            scope_id,
            Some(def_id),
        );
    }
}

fn lower_condition_uses(
    cache: &mut AnalysisCache,
    unit: &SourceUnit,
    node: Node<'_>,
    function_id: &str,
    scope_id: &str,
) {
    let condition = node
        .child_by_field_name("condition")
        .or_else(|| node.named_child(0));
    if let Some(condition) = condition {
        let uses =
            collect_expression_uses(unit, condition, Some(function_id), scope_id, "condition");
        cache.uses.extend(uses);
    }
}

fn lower_return_statement(
    cache: &mut AnalysisCache,
    unit: &SourceUnit,
    node: Node<'_>,
    module_id: &str,
    function_id: &str,
    scope_id: &str,
    class_stack: &mut Vec<String>,
) {
    let value = first_return_value(node);
    let value_text = value.map(|value| text(value, unit).trim()).unwrap_or("");
    if let Some(value) = value {
        let uses =
            collect_expression_uses(unit, value, Some(function_id), scope_id, "return value");
        cache.uses.extend(uses);
        lower_calls_in_expression(cache, unit, value, Some(function_id), scope_id, None);
        lower_embedded_statements(
            cache,
            unit,
            value,
            module_id,
            function_id,
            scope_id,
            class_stack,
        );
    }
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
}

fn first_return_value(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).next()
}

fn place_for_expression(node: Node<'_>, unit: &SourceUnit, scope_id: &str) -> Place {
    match node.kind() {
        "identifier" | "shorthand_property_identifier" => Place::Local {
            scope_id: scope_id.to_string(),
            name: text(node, unit).to_string(),
        },
        "member_expression" => {
            let base = node
                .named_child(0)
                .map(|base| expression_name(base, unit))
                .unwrap_or_default();
            let attr = node
                .named_child(1)
                .map(|attr| text(attr, unit).to_string())
                .unwrap_or_default();
            Place::Attribute { base, attr }
        }
        "subscript_expression" => {
            let base = node
                .named_child(0)
                .map(|base| expression_name(base, unit))
                .unwrap_or_default();
            let index = node
                .named_child(1)
                .map(|index| expression_name(index, unit))
                .unwrap_or_default();
            Place::Subscript { base, index }
        }
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

fn collect_expression_uses(
    unit: &SourceUnit,
    node: Node<'_>,
    function_id: Option<&str>,
    scope_id: &str,
    context: &str,
) -> Vec<Use> {
    let mut uses = Vec::new();
    walk_expression(unit, node, function_id, scope_id, context, &mut uses);
    uses
}

fn walk_expression(
    unit: &SourceUnit,
    node: Node<'_>,
    function_id: Option<&str>,
    scope_id: &str,
    context: &str,
    uses: &mut Vec<Use>,
) {
    if is_type_node(node) || is_nested_function_boundary(node.kind()) {
        return;
    }
    match node.kind() {
        "identifier" | "shorthand_property_identifier" => {
            let name = text(node, unit);
            if !name.is_empty() {
                uses.push(Use {
                    use_id: stable_id(
                        "U",
                        SCHEMA_VERSION,
                        &[
                            &unit.relative_path,
                            function_id.unwrap_or("<module>"),
                            name,
                            context,
                            &node.start_byte().to_string(),
                        ],
                    ),
                    place: Place::Local {
                        scope_id: scope_id.to_string(),
                        name: name.to_string(),
                    },
                    use_kind: "read".to_string(),
                    scope_id: scope_id.to_string(),
                    function_id: function_id.map(str::to_string),
                    span: span_for(unit, node),
                    context: context.to_string(),
                });
            }
        }
        "member_expression" | "subscript_expression" => {
            uses.push(Use {
                use_id: stable_id(
                    "U",
                    SCHEMA_VERSION,
                    &[
                        &unit.relative_path,
                        function_id.unwrap_or("<module>"),
                        &expression_name(node, unit),
                        context,
                        &node.start_byte().to_string(),
                    ],
                ),
                place: place_for_expression(node, unit, scope_id),
                use_kind: "read".to_string(),
                scope_id: scope_id.to_string(),
                function_id: function_id.map(str::to_string),
                span: span_for(unit, node),
                context: context.to_string(),
            });
            if let Some(base) = node.named_child(0) {
                walk_expression(unit, base, function_id, scope_id, context, uses);
            }
            if node.kind() == "subscript_expression" {
                if let Some(index) = node.named_child(1) {
                    walk_expression(unit, index, function_id, scope_id, context, uses);
                }
            }
        }
        "call_expression" => {
            if is_vue_compiler_macro_call(node, unit) {
                if is_define_expose_call(node, unit) {
                    if let Some(arguments) = direct_named_child(node, &["arguments"]) {
                        walk_expression(unit, arguments, function_id, scope_id, context, uses);
                    }
                }
                return;
            }
            if let Some(callee) = node.named_child(0) {
                walk_call_receiver(unit, callee, function_id, scope_id, context, uses);
            }
            if let Some(arguments) = direct_named_child(node, &["arguments"]) {
                let mut cursor = arguments.walk();
                for arg in arguments.named_children(&mut cursor) {
                    walk_expression(unit, arg, function_id, scope_id, "call:arg", uses);
                }
            }
        }
        "await_expression" | "yield_expression" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                walk_expression(unit, child, function_id, scope_id, context, uses);
            }
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                walk_expression(unit, child, function_id, scope_id, context, uses);
            }
        }
    }
}

fn walk_call_receiver(
    unit: &SourceUnit,
    callee: Node<'_>,
    function_id: Option<&str>,
    scope_id: &str,
    context: &str,
    uses: &mut Vec<Use>,
) {
    match callee.kind() {
        "member_expression" | "subscript_expression" => {
            if let Some(base) = callee.named_child(0) {
                walk_expression(unit, base, function_id, scope_id, context, uses);
            }
            if callee.kind() == "subscript_expression" {
                if let Some(index) = callee.named_child(1) {
                    walk_expression(unit, index, function_id, scope_id, context, uses);
                }
            }
        }
        _ => {}
    }
}

fn lower_embedded_statements(
    cache: &mut AnalysisCache,
    unit: &SourceUnit,
    node: Node<'_>,
    module_id: &str,
    function_id: &str,
    scope_id: &str,
    class_stack: &mut Vec<String>,
) {
    match node.kind() {
        "lexical_declaration" | "variable_declaration" => {
            lower_variable_declaration(
                cache,
                unit,
                node,
                module_id,
                scope_id,
                Some(function_id),
                class_stack,
            );
            return;
        }
        "assignment_expression" | "augmented_assignment_expression" => {
            lower_assignment_expression(cache, unit, node, function_id, scope_id);
            return;
        }
        "if_statement" | "while_statement" | "for_statement" | "for_in_statement" => {
            lower_condition_uses(cache, unit, node, function_id, scope_id);
        }
        "return_statement" => {
            lower_return_statement(
                cache,
                unit,
                node,
                module_id,
                function_id,
                scope_id,
                class_stack,
            );
            return;
        }
        "call_expression" => {
            if is_define_expose_call(node, unit) {
                if let Some(arguments) = direct_named_child(node, &["arguments"]) {
                    let uses = collect_expression_uses(
                        unit,
                        arguments,
                        Some(function_id),
                        scope_id,
                        "vue-expose",
                    );
                    cache.uses.extend(uses);
                }
            } else if !is_vue_compiler_macro_call(node, unit) {
                let uses =
                    collect_expression_uses(unit, node, Some(function_id), scope_id, "call:arg");
                cache.uses.extend(uses);
                lower_calls_in_expression(cache, unit, node, Some(function_id), scope_id, None);
            }
            return;
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        lower_embedded_statements(
            cache,
            unit,
            child,
            module_id,
            function_id,
            scope_id,
            class_stack,
        );
    }
}

fn lower_calls_in_expression(
    cache: &mut AnalysisCache,
    unit: &SourceUnit,
    node: Node<'_>,
    function_id: Option<&str>,
    scope_id: &str,
    root_return_target_def_id: Option<String>,
) {
    lower_calls_in_expression_inner(
        cache,
        unit,
        node,
        function_id,
        scope_id,
        node.start_byte(),
        root_return_target_def_id.as_deref(),
    );
}

fn lower_calls_in_expression_inner(
    cache: &mut AnalysisCache,
    unit: &SourceUnit,
    node: Node<'_>,
    function_id: Option<&str>,
    scope_id: &str,
    root_start_byte: usize,
    root_return_target_def_id: Option<&str>,
) {
    if is_type_node(node) {
        return;
    }
    if node.kind() == "call_expression" {
        if !is_vue_compiler_macro_call(node, unit) {
            let return_target_def_id = if node.start_byte() == root_start_byte {
                root_return_target_def_id.map(str::to_string)
            } else {
                None
            };
            lower_call_expression(
                cache,
                unit,
                node,
                function_id,
                scope_id,
                return_target_def_id,
            );
        }
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        lower_calls_in_expression_inner(
            cache,
            unit,
            child,
            function_id,
            scope_id,
            root_start_byte,
            root_return_target_def_id,
        );
    }
}

fn lower_call_expression(
    cache: &mut AnalysisCache,
    unit: &SourceUnit,
    node: Node<'_>,
    function_id: Option<&str>,
    scope_id: &str,
    return_target_def_id: Option<String>,
) {
    if is_vue_compiler_macro_call(node, unit) {
        return;
    }
    let callee = node.named_child(0);
    let callee_expr = callee
        .map(|callee| expression_name(callee, unit))
        .unwrap_or_else(|| "<unknown>".to_string());
    let mut arg_use_ids = Vec::new();
    if let Some(arguments) = direct_named_child(node, &["arguments"]) {
        let mut cursor = arguments.walk();
        for arg in arguments.named_children(&mut cursor) {
            let uses = collect_expression_uses(unit, arg, function_id, scope_id, "call:arg");
            arg_use_ids.extend(uses.iter().map(|use_site| use_site.use_id.clone()));
        }
    }
    cache.calls.push(CallRecord {
        call_id: stable_id(
            "CALL",
            SCHEMA_VERSION,
            &[
                &unit.relative_path,
                function_id.unwrap_or("<module>"),
                &callee_expr,
                &node.start_byte().to_string(),
            ],
        ),
        function_id: function_id.map(str::to_string),
        callee_expr,
        candidate_function_ids: Vec::new(),
        resolution: "unresolved".to_string(),
        arg_use_ids,
        return_target_def_id,
        span: span_for(unit, node),
    });
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

fn binding_names(node: Node<'_>, unit: &SourceUnit) -> Vec<String> {
    match node.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => {
            vec![text(node, unit).to_string()]
        }
        "pair_pattern" => {
            if let Some(value) = node.child_by_field_name("value") {
                return binding_names(value, unit);
            }
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .last()
                .map(|value| binding_names(value, unit))
                .unwrap_or_default()
        }
        "array_pattern" | "object_pattern" => {
            let mut names = Vec::new();
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                names.extend(binding_names(child, unit));
            }
            names
        }
        _ => Vec::new(),
    }
}

fn vue_macro_def_kind(node: Node<'_>, unit: &SourceUnit) -> Option<&'static str> {
    let callee = call_callee_name(node, unit)?;
    match callee.as_str() {
        "defineProps" => Some("vue-prop"),
        "defineEmits" => Some("vue-emit"),
        "defineModel" => Some("vue-model"),
        "withDefaults" => first_call_argument(node)
            .and_then(|arg| call_callee_name(arg, unit))
            .filter(|nested| nested == "defineProps")
            .map(|_| "vue-prop"),
        _ => None,
    }
}

fn is_vue_compiler_macro_call(node: Node<'_>, unit: &SourceUnit) -> bool {
    call_callee_name(node, unit)
        .map(|callee| {
            matches!(
                callee.as_str(),
                "defineProps" | "withDefaults" | "defineEmits" | "defineModel" | "defineExpose"
            )
        })
        .unwrap_or(false)
}

fn is_define_expose_call(node: Node<'_>, unit: &SourceUnit) -> bool {
    call_callee_name(node, unit)
        .map(|callee| callee == "defineExpose")
        .unwrap_or(false)
}

fn call_callee_name(node: Node<'_>, unit: &SourceUnit) -> Option<String> {
    if node.kind() != "call_expression" {
        return None;
    }
    node.named_child(0)
        .map(|callee| expression_name(callee, unit))
}

fn first_call_argument(node: Node<'_>) -> Option<Node<'_>> {
    let arguments = direct_named_child(node, &["arguments"])?;
    let mut cursor = arguments.walk();
    arguments.named_children(&mut cursor).next()
}

fn expression_name(node: Node<'_>, unit: &SourceUnit) -> String {
    match node.kind() {
        "identifier"
        | "property_identifier"
        | "private_property_identifier"
        | "shorthand_property_identifier" => text(node, unit).to_string(),
        "member_expression" => {
            let base = node
                .named_child(0)
                .map(|base| expression_name(base, unit))
                .unwrap_or_default();
            let attr = node
                .named_child(1)
                .map(|attr| expression_name(attr, unit))
                .unwrap_or_default();
            if base.is_empty() {
                attr
            } else if attr.is_empty() {
                base
            } else {
                format!("{base}.{attr}")
            }
        }
        "subscript_expression" => node
            .named_child(0)
            .map(|base| expression_name(base, unit))
            .unwrap_or_else(|| text(node, unit).trim().to_string()),
        "string" => string_literal_value(node, unit),
        _ => text(node, unit).trim().to_string(),
    }
}

fn is_type_node(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "type_annotation"
            | "type_arguments"
            | "type_parameters"
            | "predefined_type"
            | "object_type"
            | "tuple_type"
            | "type_identifier"
            | "property_signature"
    )
}

fn is_nested_function_boundary(kind: &str) -> bool {
    matches!(
        kind,
        "function_declaration"
            | "function_expression"
            | "arrow_function"
            | "method_definition"
            | "generator_function_declaration"
            | "generator_function"
    )
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
