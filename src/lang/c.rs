use crate::cfg::ControlFlowGraph;
use crate::ids::stable_id;
use crate::ir::{
    AnalysisCache, CallRecord, Definition, FunctionRecord, ModuleRecord, Place, SCHEMA_VERSION,
    ScopeRecord, SourceFileRecord, Use,
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
    block_id: &str,
    cfg: &mut ControlFlowGraph,
    cache: &mut AnalysisCache,
) {
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        lower_statement(path, text, child, function_id, scope_id, block_id, cfg, cache);
    }
}

fn lower_statement(
    path: &str,
    text: &str,
    node: Node,
    function_id: &str,
    scope_id: &str,
    block_id: &str,
    cfg: &mut ControlFlowGraph,
    cache: &mut AnalysisCache,
) {
    match node.kind() {
        "declaration" => lower_local_declaration(path, text, node, function_id, scope_id, cache),
        "expression_statement" => {
            lower_expression_statement(path, text, node, function_id, scope_id, cache)
        }
        "return_statement" => {
            lower_return_statement(path, text, node, function_id, scope_id, cache)
        }
        "if_statement" => {
            lower_if_statement(path, text, node, function_id, scope_id, block_id, cfg, cache)
        }
        "while_statement" | "do_statement" => {
            lower_while_statement(path, text, node, function_id, scope_id, block_id, cfg, cache)
        }
        "for_statement" => {
            lower_for_statement(path, text, node, function_id, scope_id, block_id, cfg, cache)
        }
        "compound_statement" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                lower_statement(path, text, child, function_id, scope_id, block_id, cfg, cache);
            }
        }
        _ => {
            // Still scan deeper for nested calls or assignments inside unknown constructs.
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                lower_statement(path, text, child, function_id, scope_id, block_id, cfg, cache);
            }
        }
    }
}

fn lower_local_declaration(
    path: &str,
    text: &str,
    node: Node,
    function_id: &str,
    scope_id: &str,
    cache: &mut AnalysisCache,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "init_declarator" {
            continue;
        }
        let Some(declarator) = child.child_by_field_name("declarator") else {
            continue;
        };
        let Some(value) = child.child_by_field_name("value") else {
            continue;
        };
        let name = match identifier_text(declarator, text) {
            Some(name) => name,
            None => continue,
        };
        let snippet = child
            .utf8_text(text.as_bytes())
            .unwrap_or("")
            .trim()
            .to_string();
        let span = span_for(path, text, child);
        let rhs_uses = collect_expression_uses(
            path,
            text,
            value,
            function_id,
            scope_id,
            "assign:rhs",
        );
        cache.definitions.push(Definition {
            def_id: stable_id("D", SCHEMA_VERSION, &[function_id, &name, &snippet]),
            place: Place::Local {
                scope_id: scope_id.to_string(),
                name,
            },
            def_kind: "assign".to_string(),
            scope_id: scope_id.to_string(),
            function_id: Some(function_id.to_string()),
            span,
            expr: value
                .utf8_text(text.as_bytes())
                .unwrap_or("")
                .trim()
                .to_string(),
            deps: rhs_uses.iter().map(|use_site| use_site.place.clone()).collect(),
        });
        let def_id_for_call = cache
            .definitions
            .last()
            .map(|definition| definition.def_id.clone());
        for use_site in rhs_uses {
            cache.uses.push(use_site);
        }
        if value.kind() == "call_expression" {
            lower_call(path, text, value, function_id, scope_id, cache, "assign:rhs");
            if let (Some(def_id), Some(last_call)) = (def_id_for_call, cache.calls.last_mut()) {
                if last_call.return_target_def_id.is_none() {
                    last_call.return_target_def_id = Some(def_id);
                }
            }
        }
    }
}

fn lower_expression_statement(
    path: &str,
    text: &str,
    node: Node,
    function_id: &str,
    scope_id: &str,
    cache: &mut AnalysisCache,
) {
    let Some(expr) = node.named_child(0) else {
        return;
    };
    lower_expression(path, text, expr, function_id, scope_id, cache, "expr");
}

fn lower_expression(
    path: &str,
    text: &str,
    expr: Node,
    function_id: &str,
    scope_id: &str,
    cache: &mut AnalysisCache,
    context: &str,
) {
    match expr.kind() {
        "assignment_expression" => {
            let left = expr.child_by_field_name("left");
            let right = expr.child_by_field_name("right");
            let (Some(left), Some(right)) = (left, right) else {
                return;
            };
            let lhs_text = left.utf8_text(text.as_bytes()).unwrap_or("").trim();
            let place = normalize_lvalue(left, text, scope_id);
            let span = span_for(path, text, expr);
            let rhs_uses =
                collect_expression_uses(path, text, right, function_id, scope_id, "assign:rhs");
            let rhs_text = right.utf8_text(text.as_bytes()).unwrap_or("").trim();
            let call_target_def_id = stable_id(
                "D",
                SCHEMA_VERSION,
                &[function_id, lhs_text, rhs_text, "expr-assign"],
            );
            cache.definitions.push(Definition {
                def_id: call_target_def_id.clone(),
                place,
                def_kind: "assign".to_string(),
                scope_id: scope_id.to_string(),
                function_id: Some(function_id.to_string()),
                span: span.clone(),
                expr: rhs_text.to_string(),
                deps: rhs_uses.iter().map(|use_site| use_site.place.clone()).collect(),
            });
            for use_site in rhs_uses {
                cache.uses.push(use_site);
            }
            // If the RHS is a direct call expression, lower it as a call and
            // wire the call's return_target_def_id to this assignment.
            if right.kind() == "call_expression" {
                lower_call(path, text, right, function_id, scope_id, cache, "assign:rhs");
                if let Some(last_call) = cache.calls.last_mut() {
                    if last_call.return_target_def_id.is_none() {
                        last_call.return_target_def_id = Some(call_target_def_id);
                    }
                }
            }
        }
        "call_expression" => {
            lower_call(path, text, expr, function_id, scope_id, cache, context);
        }
        _ => {
            // For other expression statements (e.g. ++x, function()), still
            // collect any sub-expression uses and call records.
            for use_site in collect_expression_uses(
                path, text, expr, function_id, scope_id, context,
            ) {
                cache.uses.push(use_site);
            }
        }
    }
}

fn lower_call(
    path: &str,
    text: &str,
    call_node: Node,
    function_id: &str,
    scope_id: &str,
    cache: &mut AnalysisCache,
    context: &str,
) {
    let function_expr = call_node.child_by_field_name("function");
    let arguments = call_node.child_by_field_name("arguments");
    let callee_expr = function_expr
        .and_then(|node| node.utf8_text(text.as_bytes()).ok())
        .unwrap_or("")
        .trim()
        .to_string();
    let span = span_for(path, text, call_node);
    let mut arg_use_ids = Vec::new();
    if let Some(arguments) = arguments {
        let mut cursor = arguments.walk();
        for arg in arguments.named_children(&mut cursor) {
            for use_site in collect_expression_uses(
                path,
                text,
                arg,
                function_id,
                scope_id,
                "call:arg",
            ) {
                arg_use_ids.push(use_site.use_id.clone());
                cache.uses.push(use_site);
            }
        }
    }
    cache.calls.push(CallRecord {
        call_id: stable_id(
            "CALL",
            SCHEMA_VERSION,
            &[function_id, &callee_expr, &span.snippet, context],
        ),
        function_id: Some(function_id.to_string()),
        callee_expr,
        candidate_function_ids: Vec::new(),
        resolution: "unresolved".to_string(),
        arg_use_ids,
        return_target_def_id: None,
        span,
    });
}

fn lower_return_statement(
    path: &str,
    text: &str,
    node: Node,
    function_id: &str,
    scope_id: &str,
    cache: &mut AnalysisCache,
) {
    let Some(value) = node.named_child(0) else {
        return;
    };
    let value_text = value.utf8_text(text.as_bytes()).unwrap_or("").trim();
    let place = normalize_lvalue(value, text, scope_id);
    cache.uses.push(Use {
        use_id: stable_id("U", SCHEMA_VERSION, &[function_id, "return", value_text]),
        place,
        use_kind: "read".to_string(),
        scope_id: scope_id.to_string(),
        function_id: Some(function_id.to_string()),
        span: span_for(path, text, node),
        context: "return value".to_string(),
    });
    // If the return value is itself a call (or contains nested calls), lower
    // those into CallRecords so cross-file symbol resolution and summary
    // propagation see them.
    lower_nested_calls(path, text, value, function_id, scope_id, cache, "return");
}

/// Walk an expression and emit CallRecords for any call_expression nodes.
fn lower_nested_calls(
    path: &str,
    text: &str,
    expr: Node,
    function_id: &str,
    scope_id: &str,
    cache: &mut AnalysisCache,
    context: &str,
) {
    if expr.kind() == "call_expression" {
        lower_call(path, text, expr, function_id, scope_id, cache, context);
        if let Some(arguments) = expr.child_by_field_name("arguments") {
            let mut cursor = arguments.walk();
            for arg in arguments.named_children(&mut cursor) {
                lower_nested_calls(path, text, arg, function_id, scope_id, cache, context);
            }
        }
        return;
    }
    let mut cursor = expr.walk();
    for child in expr.named_children(&mut cursor) {
        lower_nested_calls(path, text, child, function_id, scope_id, cache, context);
    }
}

fn lower_if_statement(
    path: &str,
    text: &str,
    node: Node,
    function_id: &str,
    scope_id: &str,
    block_id: &str,
    cfg: &mut ControlFlowGraph,
    cache: &mut AnalysisCache,
) {
    let span = span_for(path, text, node);
    let then_block = cfg.add_block("Branch", span.clone());
    let else_block = cfg.add_block("Branch", span);
    cfg.add_edge(block_id, &then_block, "branch-true", "if");
    cfg.add_edge(block_id, &else_block, "branch-false", "else");
    if let Some(condition) = node.child_by_field_name("condition") {
        for use_site in collect_expression_uses(
            path, text, condition, function_id, scope_id, "if:cond",
        ) {
            cache.uses.push(use_site);
        }
    }
    if let Some(consequence) = node.child_by_field_name("consequence") {
        let mut cursor = consequence.walk();
        for child in consequence.named_children(&mut cursor) {
            lower_statement(path, text, child, function_id, scope_id, &then_block, cfg, cache);
        }
    }
    if let Some(alternative) = node.child_by_field_name("alternative") {
        let target = alternative.named_child(0).unwrap_or(alternative);
        let mut cursor = target.walk();
        for child in target.named_children(&mut cursor) {
            lower_statement(path, text, child, function_id, scope_id, &else_block, cfg, cache);
        }
    }
}

fn lower_while_statement(
    path: &str,
    text: &str,
    node: Node,
    function_id: &str,
    scope_id: &str,
    block_id: &str,
    cfg: &mut ControlFlowGraph,
    cache: &mut AnalysisCache,
) {
    let span = span_for(path, text, node);
    let loop_block = cfg.add_block("Loop", span);
    cfg.add_edge(block_id, &loop_block, "loop-enter", "while");
    cfg.add_edge(&loop_block, &loop_block, "loop-back", "while");
    if let Some(condition) = node.child_by_field_name("condition") {
        for use_site in collect_expression_uses(
            path, text, condition, function_id, scope_id, "while:cond",
        ) {
            cache.uses.push(use_site);
        }
    }
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for child in body.named_children(&mut cursor) {
            lower_statement(path, text, child, function_id, scope_id, &loop_block, cfg, cache);
        }
    }
}

fn lower_for_statement(
    path: &str,
    text: &str,
    node: Node,
    function_id: &str,
    scope_id: &str,
    block_id: &str,
    cfg: &mut ControlFlowGraph,
    cache: &mut AnalysisCache,
) {
    let span = span_for(path, text, node);
    let loop_block = cfg.add_block("Loop", span);
    cfg.add_edge(block_id, &loop_block, "loop-enter", "for");
    cfg.add_edge(&loop_block, &loop_block, "loop-back", "for");
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for child in body.named_children(&mut cursor) {
            lower_statement(path, text, child, function_id, scope_id, &loop_block, cfg, cache);
        }
    }
}

/// Recursively walk an expression and produce Use records for every place
/// referenced. Subscript and field accesses become structural Place variants.
fn collect_expression_uses(
    path: &str,
    text: &str,
    expr: Node,
    function_id: &str,
    scope_id: &str,
    context: &str,
) -> Vec<Use> {
    let mut uses = Vec::new();
    walk_expression(path, text, expr, function_id, scope_id, context, &mut uses);
    uses
}

fn walk_expression(
    path: &str,
    text: &str,
    expr: Node,
    function_id: &str,
    scope_id: &str,
    context: &str,
    uses: &mut Vec<Use>,
) {
    match expr.kind() {
        "identifier" => {
            let name = expr.utf8_text(text.as_bytes()).unwrap_or("").trim();
            if name.is_empty() {
                return;
            }
            uses.push(Use {
                use_id: stable_id("U", SCHEMA_VERSION, &[function_id, name, context]),
                place: Place::Local {
                    scope_id: scope_id.to_string(),
                    name: name.to_string(),
                },
                use_kind: "read".to_string(),
                scope_id: scope_id.to_string(),
                function_id: Some(function_id.to_string()),
                span: span_for(path, text, expr),
                context: context.to_string(),
            });
        }
        "field_expression" => {
            let base = expr
                .child_by_field_name("argument")
                .and_then(|node| node.utf8_text(text.as_bytes()).ok())
                .unwrap_or("")
                .trim()
                .trim_start_matches('*')
                .to_string();
            let attr = expr
                .child_by_field_name("field")
                .and_then(|node| node.utf8_text(text.as_bytes()).ok())
                .unwrap_or("")
                .trim()
                .to_string();
            uses.push(Use {
                use_id: stable_id(
                    "U",
                    SCHEMA_VERSION,
                    &[function_id, &base, &attr, context],
                ),
                place: Place::Attribute {
                    base: base.clone(),
                    attr: attr.clone(),
                },
                use_kind: "read".to_string(),
                scope_id: scope_id.to_string(),
                function_id: Some(function_id.to_string()),
                span: span_for(path, text, expr),
                context: context.to_string(),
            });
        }
        "subscript_expression" => {
            let base = expr
                .child_by_field_name("argument")
                .and_then(|node| node.utf8_text(text.as_bytes()).ok())
                .unwrap_or("")
                .trim()
                .to_string();
            let index_node = expr.child_by_field_name("index");
            let index = index_node
                .and_then(|node| node.utf8_text(text.as_bytes()).ok())
                .unwrap_or("")
                .trim()
                .to_string();
            uses.push(Use {
                use_id: stable_id(
                    "U",
                    SCHEMA_VERSION,
                    &[function_id, &base, &index, context],
                ),
                place: Place::Subscript {
                    base: base.clone(),
                    index: index.clone(),
                },
                use_kind: "read".to_string(),
                scope_id: scope_id.to_string(),
                function_id: Some(function_id.to_string()),
                span: span_for(path, text, expr),
                context: context.to_string(),
            });
            // Also emit a Use for the index expression itself so var-dep
            // analysis can see the dependency on `i`/`index`.
            if let Some(index_node) = index_node {
                walk_expression(
                    path,
                    text,
                    index_node,
                    function_id,
                    scope_id,
                    context,
                    uses,
                );
            }
        }
        "call_expression" => {
            // Calls inside a larger expression are lowered separately; just
            // walk their arguments so we collect identifier uses.
            if let Some(arguments) = expr.child_by_field_name("arguments") {
                let mut cursor = arguments.walk();
                for arg in arguments.named_children(&mut cursor) {
                    walk_expression(path, text, arg, function_id, scope_id, context, uses);
                }
            }
        }
        "pointer_expression" | "unary_expression" => {
            if let Some(operand) = expr.named_child(0) {
                walk_expression(path, text, operand, function_id, scope_id, context, uses);
            }
        }
        _ => {
            let mut cursor = expr.walk();
            for child in expr.named_children(&mut cursor) {
                walk_expression(path, text, child, function_id, scope_id, context, uses);
            }
        }
    }
}

/// Map an lvalue node to a structural Place. Falls back to Local when no
/// structural variant fits.
fn normalize_lvalue(node: Node, text: &str, scope_id: &str) -> Place {
    match node.kind() {
        "field_expression" => {
            let base = node
                .child_by_field_name("argument")
                .and_then(|n| n.utf8_text(text.as_bytes()).ok())
                .unwrap_or("")
                .trim()
                .trim_start_matches('*')
                .to_string();
            let attr = node
                .child_by_field_name("field")
                .and_then(|n| n.utf8_text(text.as_bytes()).ok())
                .unwrap_or("")
                .trim()
                .to_string();
            Place::Attribute { base, attr }
        }
        "subscript_expression" => {
            let base = node
                .child_by_field_name("argument")
                .and_then(|n| n.utf8_text(text.as_bytes()).ok())
                .unwrap_or("")
                .trim()
                .to_string();
            let index = node
                .child_by_field_name("index")
                .and_then(|n| n.utf8_text(text.as_bytes()).ok())
                .unwrap_or("")
                .trim()
                .to_string();
            Place::Subscript { base, index }
        }
        "pointer_expression" | "unary_expression" => node
            .named_child(0)
            .map(|inner| normalize_lvalue(inner, text, scope_id))
            .unwrap_or_else(|| Place::Unknown {
                reason: "deref".to_string(),
            }),
        "identifier" => Place::Local {
            scope_id: scope_id.to_string(),
            name: node
                .utf8_text(text.as_bytes())
                .unwrap_or("")
                .trim()
                .to_string(),
        },
        _ => {
            let text = node.utf8_text(text.as_bytes()).unwrap_or("").trim();
            Place::Local {
                scope_id: scope_id.to_string(),
                name: text.to_string(),
            }
        }
    }
}

fn identifier_text(decl: Node, text: &str) -> Option<String> {
    let mut current = Some(decl);
    while let Some(node) = current {
        match node.kind() {
            "identifier" | "field_identifier" => {
                return node.utf8_text(text.as_bytes()).ok().map(|s| s.to_string());
            }
            "pointer_declarator" | "array_declarator" | "function_declarator"
            | "parenthesized_declarator" | "init_declarator" => {
                current = node.child_by_field_name("declarator");
            }
            _ => return None,
        }
    }
    None
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
