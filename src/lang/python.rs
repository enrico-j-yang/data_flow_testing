use super::LanguageFrontend;
use crate::alias::{normalize_attribute, normalize_subscript};
use crate::cfg::ControlFlowGraph;
use crate::fs::SourceFile;
use crate::ids::stable_id;
use crate::ir::{
    AnalysisCache, CaptureRecord, ClassRecord, Definition, Diagnostic, FunctionRecord,
    ImportRecord, ModuleRecord, Place, SCHEMA_VERSION, ScopeRecord, SourceFileRecord, Use,
};
use crate::source::SourceSpan;
use anyhow::{Context, Result, anyhow};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Parser};

#[derive(Debug, Default, Clone, Copy)]
pub struct PythonFrontend;

impl PythonFrontend {
    pub fn new() -> Self {
        Self
    }

    fn span(&self, file: &SourceFile, source: &str, node: Node<'_>) -> SourceSpan {
        let start = node.start_position();
        let end = node.end_position();

        SourceSpan {
            file: file.relative_path.clone(),
            line: start.row + 1,
            col: start.column + 1,
            end_line: end.row + 1,
            end_col: end.column + 1,
            snippet: self.node_text(source, node),
        }
    }

    fn lower_module(
        &self,
        cache: &mut AnalysisCache,
        file: &SourceFile,
        source: &str,
        root: Node<'_>,
    ) {
        let file_id = stable_id("F", SCHEMA_VERSION, &[&file.relative_path]);
        let module_id = stable_id("M", SCHEMA_VERSION, &[&file.relative_path]);
        let module_name = self.module_name_for_file(file);
        let module_scope_id = stable_id("S", SCHEMA_VERSION, &[&module_id, "module"]);
        let parse_status = if root.has_error() { "partial" } else { "ok" };

        cache.files.push(SourceFileRecord {
            file_id: file_id.clone(),
            path: file.relative_path.clone(),
            hash: source_hash(source),
            line_count: source.lines().count(),
            parse_status: parse_status.to_string(),
        });

        let module_index = cache.modules.len();
        cache.modules.push(ModuleRecord {
            module_id: module_id.clone(),
            file_id,
            module_name,
            exports: Vec::new(),
            imports: Vec::new(),
        });
        cache.scopes.push(ScopeRecord {
            scope_id: module_scope_id.clone(),
            scope_kind: "module".to_string(),
            parent_scope_id: None,
            owner_id: module_id.clone(),
            span: self.span(file, source, root),
        });

        let mut ctx = LoweringContext {
            file,
            source,
            module_id,
            module_scope_id,
            module_index,
            module_bindings: collect_scope_declarations(self, source, root).bindings,
            class_stack: Vec::new(),
            function_stack: Vec::new(),
        };

        let mut cursor = root.walk();
        for child in root.named_children(&mut cursor) {
            self.walk(cache, &mut ctx, child);
        }
    }

    fn walk(&self, cache: &mut AnalysisCache, ctx: &mut LoweringContext<'_>, node: Node<'_>) {
        if node.is_error() || node.is_missing() || node.kind() == "ERROR" {
            return;
        }

        match node.kind() {
            "future_import_statement" | "import_statement" => {
                self.lower_import_statement(cache, ctx, node)
            }
            "import_from_statement" => self.lower_import_from(cache, ctx, node),
            "class_definition" => self.lower_class(cache, ctx, node),
            "function_definition" => self.lower_function(cache, ctx, node),
            "assignment" => self.lower_assignment(cache, ctx, node),
            "expression_statement" => self.lower_expression_statement(cache, ctx, node),
            "return_statement" => self.lower_return(cache, ctx, node),
            _ => {
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    self.walk(cache, ctx, child);
                }
            }
        }
    }

    fn node_text(&self, source: &str, node: Node<'_>) -> String {
        node.utf8_text(source.as_bytes())
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    fn push_import(
        &self,
        cache: &mut AnalysisCache,
        ctx: &LoweringContext<'_>,
        node: Node<'_>,
        module: String,
        name: Option<String>,
        alias: Option<String>,
        level: usize,
    ) {
        let label = format!(
            "{}:{}",
            node.start_position().row + 1,
            node.start_position().column + 1
        );
        let import_name = name.as_deref().unwrap_or("_");
        let import_alias = alias.as_deref().unwrap_or("_");

        cache.modules[ctx.module_index].imports.push(ImportRecord {
            import_id: stable_id(
                "I",
                SCHEMA_VERSION,
                &[
                    &ctx.file.relative_path,
                    &module,
                    import_name,
                    import_alias,
                    &label,
                ],
            ),
            module,
            name,
            alias,
            level,
            resolution: "parsed".to_string(),
            span: self.span(ctx.file, ctx.source, node),
        });
    }

    fn push_import_definition(
        &self,
        cache: &mut AnalysisCache,
        ctx: &mut LoweringContext<'_>,
        node: Node<'_>,
        binding_name: &str,
        expr: String,
        def_kind: &str,
    ) {
        if binding_name.is_empty() || binding_name == "*" {
            return;
        }

        let place = self.target_identifier_place(ctx, binding_name);
        self.push_capture_record(cache, ctx, &place, "write", node);
        let location = format!(
            "{}:{}",
            node.start_position().row + 1,
            node.start_position().column + 1
        );

        cache.definitions.push(Definition {
            def_id: stable_id(
                "D",
                SCHEMA_VERSION,
                &[&ctx.file.relative_path, binding_name, def_kind, &location],
            ),
            place,
            def_kind: def_kind.to_string(),
            scope_id: ctx.current_scope_id().to_string(),
            function_id: ctx.current_function_id().map(str::to_string),
            span: self.span(ctx.file, ctx.source, node),
            expr,
            deps: Vec::new(),
        });
    }

    fn lower_import_statement(
        &self,
        cache: &mut AnalysisCache,
        ctx: &mut LoweringContext<'_>,
        node: Node<'_>,
    ) {
        let mut cursor = node.walk();
        for import_node in node.children_by_field_name("name", &mut cursor) {
            let (module, alias) = if import_node.kind() == "aliased_import" {
                (
                    import_node
                        .child_by_field_name("name")
                        .map(|value| self.node_text(ctx.source, value))
                        .unwrap_or_default(),
                    import_node
                        .child_by_field_name("alias")
                        .map(|value| self.node_text(ctx.source, value)),
                )
            } else {
                (self.node_text(ctx.source, import_node), None)
            };

            self.push_import(cache, ctx, import_node, module, None, alias.clone(), 0);
            let binding_name = alias.clone().unwrap_or_else(|| {
                import_binding_from_module(&self.node_text(ctx.source, import_node))
            });
            self.push_import_definition(
                cache,
                ctx,
                import_node,
                &binding_name,
                self.node_text(ctx.source, import_node),
                "import",
            );
        }
    }

    fn lower_import_from(
        &self,
        cache: &mut AnalysisCache,
        ctx: &mut LoweringContext<'_>,
        node: Node<'_>,
    ) {
        let module = node
            .child_by_field_name("module_name")
            .map(|value| self.node_text(ctx.source, value))
            .unwrap_or_default();
        let level = module.chars().take_while(|ch| *ch == '.').count();
        let normalized_module = module.trim_start_matches('.').to_string();
        let mut cursor = node.walk();

        for import_node in node.children_by_field_name("name", &mut cursor) {
            let (name, alias) = if import_node.kind() == "aliased_import" {
                (
                    import_node
                        .child_by_field_name("name")
                        .map(|value| self.node_text(ctx.source, value)),
                    import_node
                        .child_by_field_name("alias")
                        .map(|value| self.node_text(ctx.source, value)),
                )
            } else {
                (Some(self.node_text(ctx.source, import_node)), None)
            };

            self.push_import(
                cache,
                ctx,
                import_node,
                normalized_module.clone(),
                name.clone(),
                alias.clone(),
                level,
            );
            let binding_name = alias.clone().unwrap_or_else(|| {
                name.as_deref()
                    .map(leaf_name)
                    .unwrap_or_default()
                    .to_string()
            });
            let import_expr = format!(
                "{}{}:{}",
                ".".repeat(level),
                normalized_module,
                name.as_deref().unwrap_or_default()
            );
            self.push_import_definition(
                cache,
                ctx,
                import_node,
                &binding_name,
                import_expr,
                "from_import",
            );
        }
    }

    fn lower_class(
        &self,
        cache: &mut AnalysisCache,
        ctx: &mut LoweringContext<'_>,
        node: Node<'_>,
    ) {
        let Some(name_node) = node.child_by_field_name("name") else {
            return;
        };
        let class_name = self.node_text(ctx.source, name_node);
        let qualified_name = if let Some(parent) = ctx.class_stack.last() {
            format!("{}.{}", parent.qualified_name, class_name)
        } else if let Some(function_frame) = ctx.function_stack.last() {
            format!("{}.{}", function_frame.qualified_name, class_name)
        } else {
            class_name.clone()
        };
        let start = format!(
            "{}:{}",
            node.start_position().row + 1,
            node.start_position().column + 1
        );
        let class_id = stable_id(
            "C",
            SCHEMA_VERSION,
            &[&ctx.file.relative_path, &qualified_name, &start],
        );
        let scope_id = stable_id("S", SCHEMA_VERSION, &[&class_id, "class"]);
        let base_exprs = node
            .child_by_field_name("superclasses")
            .map(|superclasses| {
                let mut cursor = superclasses.walk();
                superclasses
                    .named_children(&mut cursor)
                    .map(|child| self.node_text(ctx.source, child))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let bindings = node
            .child_by_field_name("body")
            .map(|body| collect_scope_declarations(self, ctx.source, body).bindings)
            .unwrap_or_default();

        let class_index = cache.classes.len();
        cache.classes.push(ClassRecord {
            class_id: class_id.clone(),
            module_id: ctx.module_id.clone(),
            qualified_name: qualified_name.clone(),
            base_exprs,
            resolved_bases: Vec::new(),
            mro_status: "local-unresolved".to_string(),
            methods: Vec::new(),
            span: self.span(ctx.file, ctx.source, node),
        });
        cache.scopes.push(ScopeRecord {
            scope_id: scope_id.clone(),
            scope_kind: "class".to_string(),
            parent_scope_id: Some(ctx.current_scope_id().to_string()),
            owner_id: class_id.clone(),
            span: self.span(ctx.file, ctx.source, node),
        });

        ctx.class_stack.push(ClassFrame {
            class_id,
            qualified_name,
            scope_id,
            cache_index: class_index,
            bindings,
        });

        if let Some(body) = node.child_by_field_name("body") {
            let mut cursor = body.walk();
            for child in body.named_children(&mut cursor) {
                self.walk(cache, ctx, child);
            }
        }

        ctx.class_stack.pop();
    }

    fn lower_function(
        &self,
        cache: &mut AnalysisCache,
        ctx: &mut LoweringContext<'_>,
        node: Node<'_>,
    ) {
        let Some(name_node) = node.child_by_field_name("name") else {
            return;
        };
        let function_name = self.node_text(ctx.source, name_node);
        let qualified_name = if let Some(function_frame) = ctx.function_stack.last() {
            format!("{}.{}", function_frame.qualified_name, function_name)
        } else if let Some(class_frame) = ctx.class_stack.last() {
            format!("{}.{}", class_frame.qualified_name, function_name)
        } else {
            function_name.clone()
        };
        let kind = if ctx.function_stack.last().is_some() {
            "function"
        } else if ctx.class_stack.last().is_some() {
            "method"
        } else {
            "function"
        };
        let start = format!(
            "{}:{}",
            node.start_position().row + 1,
            node.start_position().column + 1
        );
        let function_id = stable_id(
            "FN",
            SCHEMA_VERSION,
            &[&ctx.file.relative_path, &qualified_name, &start],
        );
        let scope_id = stable_id("S", SCHEMA_VERSION, &[&function_id, "function"]);
        let params = node
            .child_by_field_name("parameters")
            .map(|parameters| collect_parameter_bindings(self, ctx.source, parameters))
            .unwrap_or_default();
        let declarations = node
            .child_by_field_name("body")
            .map(|body| collect_scope_declarations(self, ctx.source, body))
            .unwrap_or_default();
        let mut bindings = declarations.bindings.clone();
        bindings.extend(params.iter().cloned());
        let direct_class_owner = if ctx.function_stack.is_empty() {
            ctx.class_stack.last().map(|frame| frame.class_id.clone())
        } else {
            None
        };

        cache.functions.push(FunctionRecord {
            function_id: function_id.clone(),
            module_id: ctx.module_id.clone(),
            class_id: direct_class_owner.clone(),
            qualified_name: qualified_name.clone(),
            kind: kind.to_string(),
            params: params.clone(),
            scope_id: scope_id.clone(),
            span: self.span(ctx.file, ctx.source, node),
        });
        cache.scopes.push(ScopeRecord {
            scope_id: scope_id.clone(),
            scope_kind: "function".to_string(),
            parent_scope_id: Some(ctx.current_scope_id().to_string()),
            owner_id: function_id.clone(),
            span: self.span(ctx.file, ctx.source, node),
        });

        if ctx.function_stack.is_empty() {
            if let Some(class_frame) = ctx.class_stack.last() {
                cache.classes[class_frame.cache_index]
                    .methods
                    .push(function_id.clone());
            }
        }

        ctx.function_stack.push(FunctionFrame {
            function_id: function_id.clone(),
            qualified_name,
            scope_id,
            bindings,
            global_decls: declarations.global_decls,
            nonlocal_decls: declarations.nonlocal_decls,
        });

        self.lower_parameter_definitions(cache, ctx, node, &params);

        if let Some(body) = node.child_by_field_name("body") {
            let mut cursor = body.walk();
            for child in body.named_children(&mut cursor) {
                self.walk(cache, ctx, child);
            }
            let cfg = self.build_baseline_cfg(ctx, &function_id, body);
            cache.cfgs.push(cfg.into_record());
        }

        ctx.function_stack.pop();
    }

    fn lower_assignment(
        &self,
        cache: &mut AnalysisCache,
        ctx: &mut LoweringContext<'_>,
        node: Node<'_>,
    ) {
        let mut targets = Vec::new();
        let right = collect_assignment_targets(node, &mut targets);
        if targets.is_empty() {
            return;
        }

        let expr = right
            .map(|value| self.node_text(ctx.source, value))
            .unwrap_or_default();
        let mut deps = Vec::new();

        if let Some(value) = right {
            for use_spec in expression_uses(value) {
                if let Some(dep) = self.lower_identifier_use(
                    cache,
                    ctx,
                    use_spec.node,
                    use_spec.use_kind,
                    "assign:rhs",
                ) {
                    deps.push(dep);
                }
            }
            self.lower_expression_effects(cache, ctx, value);
        }

        for target in targets {
            let Some(place) = self.target_place_for_node(ctx, target) else {
                continue;
            };
            self.push_capture_record(cache, ctx, &place, "write", target);
            let location = format!(
                "{}:{}",
                target.start_position().row + 1,
                target.start_position().column + 1
            );

            cache.definitions.push(Definition {
                def_id: stable_id(
                    "D",
                    SCHEMA_VERSION,
                    &[
                        &ctx.file.relative_path,
                        &self.node_text(ctx.source, target),
                        &location,
                    ],
                ),
                place,
                def_kind: "assign".to_string(),
                scope_id: ctx.current_scope_id().to_string(),
                function_id: ctx.current_function_id().map(str::to_string),
                span: self.span(ctx.file, ctx.source, node),
                expr: expr.clone(),
                deps: deps.clone(),
            });
        }
    }

    fn lower_return(
        &self,
        cache: &mut AnalysisCache,
        ctx: &mut LoweringContext<'_>,
        node: Node<'_>,
    ) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            for use_spec in expression_uses(child) {
                self.lower_identifier_use(
                    cache,
                    ctx,
                    use_spec.node,
                    use_spec.use_kind,
                    "return value",
                );
            }
            self.lower_expression_effects(cache, ctx, child);
        }
    }

    fn lower_expression_statement(
        &self,
        cache: &mut AnalysisCache,
        ctx: &mut LoweringContext<'_>,
        node: Node<'_>,
    ) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "assignment" {
                self.walk(cache, ctx, child);
                continue;
            }
            for use_spec in expression_uses(child) {
                self.lower_identifier_use(
                    cache,
                    ctx,
                    use_spec.node,
                    use_spec.use_kind,
                    "expr:statement",
                );
            }
            self.lower_expression_effects(cache, ctx, child);
        }
    }

    fn lower_expression_effects(
        &self,
        cache: &mut AnalysisCache,
        ctx: &mut LoweringContext<'_>,
        node: Node<'_>,
    ) {
        if node.kind() == "call" {
            self.lower_mutating_receiver_call_definition(cache, ctx, node);
        }

        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            self.lower_expression_effects(cache, ctx, child);
        }
    }

    fn lower_mutating_receiver_call_definition(
        &self,
        cache: &mut AnalysisCache,
        ctx: &mut LoweringContext<'_>,
        node: Node<'_>,
    ) {
        let Some(function) = node.child_by_field_name("function") else {
            return;
        };
        if function.kind() != "attribute" {
            return;
        }

        let Some(method_node) = function.child_by_field_name("attribute") else {
            return;
        };
        let method_name = self.node_text(ctx.source, method_node);
        if !is_mutating_receiver_method(&method_name) {
            return;
        }

        let Some(receiver) = function.child_by_field_name("object") else {
            return;
        };
        // A mutating receiver call mutates the resolved receiver binding rather than
        // rebinding a fresh assignment target, so preserve the receiver's actual scope.
        let Some(place) = self.use_place_for_node(ctx, receiver) else {
            return;
        };

        let receiver_text = self.node_text(ctx.source, receiver);
        let location = format!(
            "{}:{}",
            receiver.start_position().row + 1,
            receiver.start_position().column + 1
        );
        let mut deps = Vec::new();
        if let Some(receiver_place) = self.use_place_for_node(ctx, receiver) {
            deps.push(receiver_place);
        }

        if let Some(arguments) = node.child_by_field_name("arguments") {
            let mut cursor = arguments.walk();
            for child in arguments.named_children(&mut cursor) {
                for use_spec in expression_uses(child) {
                    let Some(dep) = self.use_place_for_node(ctx, use_spec.node) else {
                        continue;
                    };
                    if !deps.contains(&dep) {
                        deps.push(dep);
                    }
                }
            }
        }

        let mut span = self.span(ctx.file, ctx.source, node);
        span.col = receiver.start_position().column + 1;
        span.snippet = format!("{receiver_text} = <mutated by {method_name}>");

        cache.definitions.push(Definition {
            def_id: stable_id(
                "D",
                SCHEMA_VERSION,
                &[
                    &ctx.file.relative_path,
                    &receiver_text,
                    &location,
                    "mut-call",
                    &method_name,
                ],
            ),
            place,
            def_kind: "mut-call".to_string(),
            scope_id: ctx.current_scope_id().to_string(),
            function_id: ctx.current_function_id().map(str::to_string),
            span,
            expr: self.node_text(ctx.source, node),
            deps,
        });
    }

    fn lower_identifier_use(
        &self,
        cache: &mut AnalysisCache,
        ctx: &LoweringContext<'_>,
        node: Node<'_>,
        use_kind: &str,
        context: &str,
    ) -> Option<Place> {
        let place = self.use_place_for_node(ctx, node)?;
        let location = format!(
            "{}:{}",
            node.start_position().row + 1,
            node.start_position().column + 1
        );
        let label = self.node_text(ctx.source, node);
        cache.uses.push(Use {
            use_id: stable_id(
                "U",
                SCHEMA_VERSION,
                &[&ctx.file.relative_path, &label, &location, context],
            ),
            place: place.clone(),
            use_kind: use_kind.to_string(),
            scope_id: ctx.current_scope_id().to_string(),
            function_id: ctx.current_function_id().map(str::to_string),
            span: self.span(ctx.file, ctx.source, node),
            context: context.to_string(),
        });
        self.push_capture_record(cache, ctx, &place, "read", node);

        Some(place)
    }

    fn lower_parameter_definitions(
        &self,
        cache: &mut AnalysisCache,
        ctx: &LoweringContext<'_>,
        node: Node<'_>,
        params: &[String],
    ) {
        let Some(function_frame) = ctx.function_stack.last() else {
            return;
        };

        let span_node = node.child_by_field_name("parameters").unwrap_or(node);
        for (index, name) in params.iter().enumerate() {
            cache.definitions.push(Definition {
                def_id: stable_id(
                    "D",
                    SCHEMA_VERSION,
                    &[
                        &ctx.file.relative_path,
                        &function_frame.function_id,
                        "param",
                        name,
                        &index.to_string(),
                    ],
                ),
                place: Place::Local {
                    scope_id: function_frame.scope_id.clone(),
                    name: name.clone(),
                },
                def_kind: "param".to_string(),
                scope_id: function_frame.scope_id.clone(),
                function_id: Some(function_frame.function_id.clone()),
                span: self.span(ctx.file, ctx.source, span_node),
                expr: String::new(),
                deps: Vec::new(),
            });
        }
    }

    fn push_capture_record(
        &self,
        cache: &mut AnalysisCache,
        ctx: &LoweringContext<'_>,
        place: &Place,
        mode: &str,
        node: Node<'_>,
    ) {
        let Some(target_function_id) = ctx.current_function_id() else {
            return;
        };
        let Place::Closure { scope_id, name } = place else {
            return;
        };
        let location = format!(
            "{}:{}",
            node.start_position().row + 1,
            node.start_position().column + 1
        );

        cache.captures.push(CaptureRecord {
            capture_id: stable_id(
                "CAP",
                SCHEMA_VERSION,
                &[
                    &ctx.file.relative_path,
                    target_function_id,
                    scope_id,
                    name,
                    mode,
                    &location,
                ],
            ),
            source_scope_id: scope_id.clone(),
            target_function_id: target_function_id.to_string(),
            place: place.clone(),
            mode: mode.to_string(),
            span: self.span(ctx.file, ctx.source, node),
        });
    }

    fn use_place_for_node(&self, ctx: &LoweringContext<'_>, node: Node<'_>) -> Option<Place> {
        match node.kind() {
            "identifier" => {
                Some(self.resolve_identifier_use_place(ctx, &self.node_text(ctx.source, node)))
            }
            "attribute" => Some(self.attribute_place(ctx, node)),
            "subscript" => Some(self.subscript_place(ctx, node)),
            _ => None,
        }
    }

    fn target_place_for_node(&self, ctx: &LoweringContext<'_>, node: Node<'_>) -> Option<Place> {
        match node.kind() {
            "identifier" => {
                Some(self.target_identifier_place(ctx, &self.node_text(ctx.source, node)))
            }
            "attribute" => Some(self.attribute_place(ctx, node)),
            "subscript" => Some(self.subscript_place(ctx, node)),
            _ => None,
        }
    }

    fn resolve_identifier_use_place(&self, ctx: &LoweringContext<'_>, name: &str) -> Place {
        if !ctx.function_stack.is_empty() {
            let current_frame = ctx.function_stack.last().unwrap();
            if current_frame.global_decls.contains(name) {
                return Place::Global {
                    module_id: ctx.module_id.clone(),
                    name: name.to_string(),
                };
            }
            if current_frame.nonlocal_decls.contains(name) {
                if let Some(place) = self.resolve_enclosing_function_place(ctx, name) {
                    return place;
                }
            }

            for (index, frame) in ctx.function_stack.iter().enumerate().rev() {
                if index == ctx.function_stack.len() - 1
                    && current_frame.nonlocal_decls.contains(name)
                {
                    continue;
                }
                if frame.bindings.contains(name) {
                    return if index == ctx.function_stack.len() - 1 {
                        Place::Local {
                            scope_id: frame.scope_id.clone(),
                            name: name.to_string(),
                        }
                    } else {
                        Place::Closure {
                            scope_id: frame.scope_id.clone(),
                            name: name.to_string(),
                        }
                    };
                }
                if frame.nonlocal_decls.contains(name) {
                    continue;
                }
            }

            if ctx.module_bindings.contains(name) {
                return Place::Global {
                    module_id: ctx.module_id.clone(),
                    name: name.to_string(),
                };
            }

            return Place::External {
                name: name.to_string(),
            };
        }

        if !ctx.class_stack.is_empty() {
            for frame in ctx.class_stack.iter().rev() {
                if frame.bindings.contains(name) {
                    return Place::Local {
                        scope_id: frame.scope_id.clone(),
                        name: name.to_string(),
                    };
                }
            }

            if ctx.module_bindings.contains(name) {
                return Place::Global {
                    module_id: ctx.module_id.clone(),
                    name: name.to_string(),
                };
            }

            return Place::External {
                name: name.to_string(),
            };
        }

        if ctx.module_bindings.contains(name) {
            Place::Global {
                module_id: ctx.module_id.clone(),
                name: name.to_string(),
            }
        } else {
            Place::External {
                name: name.to_string(),
            }
        }
    }

    fn target_identifier_place(&self, ctx: &LoweringContext<'_>, name: &str) -> Place {
        if let Some(function_frame) = ctx.function_stack.last() {
            if function_frame.global_decls.contains(name) {
                return Place::Global {
                    module_id: ctx.module_id.clone(),
                    name: name.to_string(),
                };
            }
            if function_frame.nonlocal_decls.contains(name) {
                if let Some(place) = self.resolve_enclosing_function_place(ctx, name) {
                    return place;
                }
            }

            return Place::Local {
                scope_id: function_frame.scope_id.clone(),
                name: name.to_string(),
            };
        }

        if let Some(class_frame) = ctx.class_stack.last() {
            return Place::Local {
                scope_id: class_frame.scope_id.clone(),
                name: name.to_string(),
            };
        }

        Place::Global {
            module_id: ctx.module_id.clone(),
            name: name.to_string(),
        }
    }

    fn attribute_place(&self, ctx: &LoweringContext<'_>, node: Node<'_>) -> Place {
        let base = node
            .child_by_field_name("object")
            .map(|value| self.node_text(ctx.source, value))
            .unwrap_or_default();
        let attr = node
            .child_by_field_name("attribute")
            .map(|value| self.node_text(ctx.source, value))
            .unwrap_or_default();
        normalize_attribute(self.current_class_name(ctx), &base, &attr)
    }

    fn subscript_place(&self, ctx: &LoweringContext<'_>, node: Node<'_>) -> Place {
        let base = node
            .child_by_field_name("value")
            .map(|value| self.node_text(ctx.source, value))
            .unwrap_or_default();
        let index = node
            .child_by_field_name("subscript")
            .map(|value| self.node_text(ctx.source, value));
        normalize_subscript(&base, index.as_deref())
    }

    fn resolve_enclosing_function_place(
        &self,
        ctx: &LoweringContext<'_>,
        name: &str,
    ) -> Option<Place> {
        for frame in ctx.function_stack.iter().rev().skip(1) {
            if frame.bindings.contains(name) {
                return Some(Place::Closure {
                    scope_id: frame.scope_id.clone(),
                    name: name.to_string(),
                });
            }
        }

        None
    }

    fn current_class_name<'a>(&self, ctx: &'a LoweringContext<'_>) -> Option<&'a str> {
        ctx.class_stack
            .last()
            .map(|frame| frame.qualified_name.as_str())
    }

    fn build_baseline_cfg(
        &self,
        ctx: &LoweringContext<'_>,
        function_id: &str,
        body: Node<'_>,
    ) -> ControlFlowGraph {
        let mut cfg = ControlFlowGraph::new(function_id.to_string());
        let body_block = cfg.add_block("BasicBlock", self.span(ctx.file, ctx.source, body));
        let exit_kind = if contains_return(body) {
            "return"
        } else {
            "sequence"
        };

        cfg.add_edge(&cfg.entry_block_id.clone(), &body_block, "sequence", "body");
        cfg.add_edge(
            &body_block,
            &cfg.exit_block_id.clone(),
            exit_kind,
            exit_kind,
        );
        cfg
    }

    fn module_name_for_file(&self, file: &SourceFile) -> String {
        let relative_module = relative_module_name(&file.relative_path);
        let Some(root) = input_root_dir(file) else {
            return fallback_module_name(&file.relative_path, &relative_module);
        };

        let Some(root_package) = root_package_name(&root) else {
            return fallback_module_name(&file.relative_path, &relative_module);
        };

        if !root.join("__init__.py").exists() {
            return fallback_module_name(&file.relative_path, &relative_module);
        }

        if relative_module.is_empty() {
            root_package
        } else {
            format!("{root_package}.{relative_module}")
        }
    }

    fn record_parse_errors(
        &self,
        cache: &mut AnalysisCache,
        file: &SourceFile,
        source: &str,
        node: Node<'_>,
    ) {
        if node.is_error() || node.is_missing() || node.kind() == "ERROR" {
            cache.diagnostics.push(Diagnostic {
                diagnostic_id: stable_id(
                    "DIAG",
                    SCHEMA_VERSION,
                    &[
                        &file.relative_path,
                        "parse-error",
                        &node.start_byte().to_string(),
                    ],
                ),
                severity: "warning".to_string(),
                kind: "parse-error".to_string(),
                message:
                    "tree-sitter reported an invalid parse node; analysis skipped this subtree"
                        .to_string(),
                file: file.relative_path.clone(),
                span: self.span(file, source, node),
            });
            return;
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.record_parse_errors(cache, file, source, child);
        }
    }
}

impl LanguageFrontend for PythonFrontend {
    fn parse_files(&self, files: &[SourceFile]) -> Result<AnalysisCache> {
        let mut partials = files
            .par_iter()
            .map(|file| -> Result<(String, AnalysisCache)> {
                let cache = self.parse_single_file(file)?;
                let key = cache
                    .files
                    .first()
                    .map(|record| record.path.clone())
                    .unwrap_or_else(|| file.relative_path.clone());
                Ok((key, cache))
            })
            .collect::<Vec<_>>()
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
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

impl PythonFrontend {
    fn parse_single_file(&self, file: &SourceFile) -> Result<AnalysisCache> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .context("failed to load tree-sitter-python")?;

        let source = fs::read_to_string(&file.absolute_path)
            .with_context(|| format!("failed to read {}", file.absolute_path.display()))?;
        let tree = parser.parse(&source, None).ok_or_else(|| {
            anyhow!(
                "tree-sitter returned no parse tree for {}",
                file.relative_path
            )
        })?;

        let mut cache = AnalysisCache {
            tool_version: env!("CARGO_PKG_VERSION").to_string(),
            ..AnalysisCache::default()
        };
        self.record_parse_errors(&mut cache, file, &source, tree.root_node());
        self.lower_module(&mut cache, file, &source, tree.root_node());
        Ok(cache)
    }
}

struct LoweringContext<'a> {
    file: &'a SourceFile,
    source: &'a str,
    module_id: String,
    module_scope_id: String,
    module_index: usize,
    module_bindings: HashSet<String>,
    class_stack: Vec<ClassFrame>,
    function_stack: Vec<FunctionFrame>,
}

impl LoweringContext<'_> {
    fn current_scope_id(&self) -> &str {
        if let Some(function_frame) = self.function_stack.last() {
            &function_frame.scope_id
        } else if let Some(class_frame) = self.class_stack.last() {
            &class_frame.scope_id
        } else {
            &self.module_scope_id
        }
    }

    fn current_function_id(&self) -> Option<&str> {
        self.function_stack
            .last()
            .map(|frame| frame.function_id.as_str())
    }
}

struct ClassFrame {
    class_id: String,
    qualified_name: String,
    scope_id: String,
    cache_index: usize,
    bindings: HashSet<String>,
}

struct FunctionFrame {
    function_id: String,
    qualified_name: String,
    scope_id: String,
    bindings: HashSet<String>,
    global_decls: HashSet<String>,
    nonlocal_decls: HashSet<String>,
}

#[derive(Default)]
struct ScopeDeclarations {
    bindings: HashSet<String>,
    global_decls: HashSet<String>,
    nonlocal_decls: HashSet<String>,
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

fn input_root_dir(file: &SourceFile) -> Option<PathBuf> {
    let depth = Path::new(&file.relative_path).components().count();
    let mut root = file.absolute_path.clone();
    for _ in 0..depth {
        root = root.parent()?.to_path_buf();
    }
    Some(root)
}

fn root_package_name(root: &Path) -> Option<String> {
    root.file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn relative_module_name(relative_path: &str) -> String {
    let mut parts: Vec<&str> = relative_path.split('/').collect();
    let Some(last) = parts.pop() else {
        return String::new();
    };

    if last == "__init__.py" {
        return parts.join(".");
    }

    let stem = last.strip_suffix(".py").unwrap_or(last);
    if stem.is_empty() {
        parts.join(".")
    } else if parts.is_empty() {
        stem.to_string()
    } else {
        parts.push(stem);
        parts.join(".")
    }
}

fn fallback_module_name(relative_path: &str, relative_module: &str) -> String {
    if relative_module.is_empty() && relative_path.ends_with("__init__.py") {
        "__init__".to_string()
    } else {
        relative_module.to_string()
    }
}

fn leaf_name(value: &str) -> String {
    value.rsplit('.').next().unwrap_or(value).to_string()
}

fn import_binding_from_module(value: &str) -> String {
    value.split('.').next().unwrap_or(value).to_string()
}

fn collect_scope_declarations(
    frontend: &PythonFrontend,
    source: &str,
    node: Node<'_>,
) -> ScopeDeclarations {
    let mut declarations = ScopeDeclarations::default();
    scan_scope_declarations(frontend, source, node, &mut declarations);
    declarations
}

fn contains_return(node: Node<'_>) -> bool {
    if node.kind() == "return_statement" {
        return true;
    }

    match node.kind() {
        "function_definition" | "class_definition" | "lambda" => false,
        _ => {
            let mut cursor = node.walk();
            node.named_children(&mut cursor).any(contains_return)
        }
    }
}

fn scan_scope_declarations(
    frontend: &PythonFrontend,
    source: &str,
    node: Node<'_>,
    declarations: &mut ScopeDeclarations,
) {
    if node.is_error() || node.is_missing() || node.kind() == "ERROR" {
        return;
    }

    match node.kind() {
        "function_definition" | "class_definition" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                declarations
                    .bindings
                    .insert(frontend.node_text(source, name_node));
            }
        }
        "future_import_statement" | "import_statement" => {
            collect_plain_import_bindings(frontend, source, node, &mut declarations.bindings)
        }
        "import_from_statement" => {
            collect_from_import_bindings(frontend, source, node, &mut declarations.bindings)
        }
        "assignment" => {
            collect_assignment_binding_names(frontend, source, node, &mut declarations.bindings)
        }
        "global_statement" => {
            collect_statement_identifiers(frontend, source, node, &mut declarations.global_decls)
        }
        "nonlocal_statement" => {
            collect_statement_identifiers(frontend, source, node, &mut declarations.nonlocal_decls)
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                scan_scope_declarations(frontend, source, child, declarations);
            }
        }
    }
}

fn collect_plain_import_bindings(
    frontend: &PythonFrontend,
    source: &str,
    node: Node<'_>,
    bindings: &mut HashSet<String>,
) {
    let mut cursor = node.walk();
    for import_node in node.children_by_field_name("name", &mut cursor) {
        if import_node.kind() == "aliased_import" {
            if let Some(alias_node) = import_node.child_by_field_name("alias") {
                bindings.insert(frontend.node_text(source, alias_node));
            }
            continue;
        }

        let module = frontend.node_text(source, import_node);
        if let Some(root) = module.split('.').next() {
            bindings.insert(root.to_string());
        }
    }
}

fn collect_from_import_bindings(
    frontend: &PythonFrontend,
    source: &str,
    node: Node<'_>,
    bindings: &mut HashSet<String>,
) {
    let mut cursor = node.walk();
    for import_node in node.children_by_field_name("name", &mut cursor) {
        if import_node.kind() == "aliased_import" {
            if let Some(alias_node) = import_node.child_by_field_name("alias") {
                bindings.insert(frontend.node_text(source, alias_node));
            }
            continue;
        }

        let imported_name = frontend.node_text(source, import_node);
        if let Some(name) = imported_name.rsplit('.').next() {
            bindings.insert(name.to_string());
        }
    }
}

fn collect_assignment_binding_names(
    frontend: &PythonFrontend,
    source: &str,
    node: Node<'_>,
    bindings: &mut HashSet<String>,
) {
    let mut targets = Vec::new();
    collect_assignment_targets(node, &mut targets);
    for target in targets {
        if target.kind() == "identifier" {
            bindings.insert(frontend.node_text(source, target));
        }
    }
}

fn collect_statement_identifiers(
    frontend: &PythonFrontend,
    source: &str,
    node: Node<'_>,
    bindings: &mut HashSet<String>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "identifier" {
            bindings.insert(frontend.node_text(source, child));
        }
    }
}

fn collect_parameter_bindings(
    frontend: &PythonFrontend,
    source: &str,
    parameters: Node<'_>,
) -> Vec<String> {
    let mut bindings = Vec::new();
    let mut cursor = parameters.walk();
    for child in parameters.named_children(&mut cursor) {
        collect_parameter_binding_nodes(frontend, source, child, &mut bindings);
    }
    bindings
}

fn collect_parameter_binding_nodes(
    frontend: &PythonFrontend,
    source: &str,
    node: Node<'_>,
    bindings: &mut Vec<String>,
) {
    match node.kind() {
        "identifier" => bindings.push(frontend.node_text(source, node)),
        "parameter" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_parameter_binding_nodes(frontend, source, child, bindings);
            }
        }
        "default_parameter" | "typed_default_parameter" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                collect_parameter_binding_nodes(frontend, source, name_node, bindings);
            }
        }
        "typed_parameter" => {
            let type_node = node.child_by_field_name("type");
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if type_node.map(|value| value == child).unwrap_or(false) {
                    continue;
                }
                collect_parameter_binding_nodes(frontend, source, child, bindings);
            }
        }
        "tuple_pattern" | "list_pattern" | "list_splat_pattern" | "dictionary_splat_pattern" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_parameter_binding_nodes(frontend, source, child, bindings);
            }
        }
        _ => {}
    }
}

fn collect_assignment_targets<'tree>(
    node: Node<'tree>,
    targets: &mut Vec<Node<'tree>>,
) -> Option<Node<'tree>> {
    if let Some(left) = node.child_by_field_name("left") {
        collect_binding_target_nodes(left, targets);
    }

    let right = node.child_by_field_name("right")?;
    if right.kind() == "assignment" {
        collect_assignment_targets(right, targets)
    } else {
        Some(right)
    }
}

fn collect_binding_target_nodes<'tree>(node: Node<'tree>, targets: &mut Vec<Node<'tree>>) {
    match node.kind() {
        "identifier" | "attribute" | "subscript" => targets.push(node),
        "pattern_list"
        | "tuple_pattern"
        | "list_pattern"
        | "list_splat_pattern"
        | "dictionary_splat_pattern"
        | "parenthesized_expression" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_binding_target_nodes(child, targets);
            }
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_binding_target_nodes(child, targets);
            }
        }
    }
}

#[derive(Clone, Copy)]
struct ExpressionUseSpec<'tree> {
    node: Node<'tree>,
    use_kind: &'static str,
}

fn expression_uses(node: Node<'_>) -> Vec<ExpressionUseSpec<'_>> {
    let mut nodes = Vec::new();
    collect_expression_uses(node, &mut nodes);
    nodes
}

fn is_mutating_receiver_method(name: &str) -> bool {
    matches!(
        name,
        "append"
            | "appendleft"
            | "extend"
            | "extendleft"
            | "insert"
            | "remove"
            | "pop"
            | "popleft"
            | "clear"
            | "sort"
            | "reverse"
            | "add"
            | "discard"
            | "update"
            | "setdefault"
            | "popitem"
            | "put"
    )
}

fn collect_expression_uses<'tree>(node: Node<'tree>, nodes: &mut Vec<ExpressionUseSpec<'tree>>) {
    match node.kind() {
        "identifier" | "attribute" => nodes.push(ExpressionUseSpec {
            node,
            use_kind: "load",
        }),
        "subscript" => {
            nodes.push(ExpressionUseSpec {
                node,
                use_kind: "load",
            });
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_expression_uses(child, nodes);
            }
        }
        "call" => {
            let function = node.child_by_field_name("function");
            let has_arguments = node
                .child_by_field_name("arguments")
                .map(|arguments| arguments.named_child_count() > 0)
                .unwrap_or(false);
            if let Some(function) = function {
                if matches!(function.kind(), "attribute" | "subscript") {
                    let receiver = if function.kind() == "attribute" {
                        function.child_by_field_name("object")
                    } else {
                        function.child_by_field_name("value")
                    };

                    if has_arguments {
                        if let Some(receiver) = receiver {
                            collect_expression_uses(receiver, nodes);
                        }
                        if let Some(arguments) = node.child_by_field_name("arguments") {
                            let mut cursor = arguments.walk();
                            for child in arguments.named_children(&mut cursor) {
                                collect_expression_uses(child, nodes);
                            }
                        }
                        return;
                    }

                    if let Some(receiver) = receiver {
                        if receiver.kind() == "identifier" {
                            nodes.push(ExpressionUseSpec {
                                node: function,
                                use_kind: "call-zero-arg",
                            });
                        } else {
                            collect_expression_uses(receiver, nodes);
                        }
                        return;
                    }
                }
            }
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_expression_uses(child, nodes);
            }
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_expression_uses(child, nodes);
            }
        }
    }
}
