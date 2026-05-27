use super::LanguageFrontend;
use crate::fs::SourceFile;
use crate::ids::stable_id;
use crate::ir::{
    AnalysisCache, ClassRecord, Definition, FunctionRecord, ImportRecord, ModuleRecord, Place,
    ScopeRecord, SourceFileRecord, Use, SCHEMA_VERSION,
};
use crate::source::SourceSpan;
use anyhow::{Context, Result, anyhow};
use sha2::{Digest, Sha256};
use std::fs;
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
        let module_name = file
            .relative_path
            .strip_suffix(".py")
            .unwrap_or(&file.relative_path)
            .replace('/', ".");
        let module_scope_id = stable_id("S", SCHEMA_VERSION, &[&module_id, "module"]);
        let parse_status = if root.has_error() { "error" } else { "ok" };

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
            class_stack: Vec::new(),
            function_stack: Vec::new(),
        };

        let mut cursor = root.walk();
        for child in root.named_children(&mut cursor) {
            self.walk(cache, &mut ctx, child);
        }
    }

    fn walk(&self, cache: &mut AnalysisCache, ctx: &mut LoweringContext<'_>, node: Node<'_>) {
        match node.kind() {
            "import_from_statement" => self.lower_import_from(cache, ctx, node),
            "class_definition" => self.lower_class(cache, ctx, node),
            "function_definition" => self.lower_function(cache, ctx, node),
            "assignment" => self.lower_assignment(cache, ctx, node),
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
            let label = format!(
                "{}:{}",
                import_node.start_position().row + 1,
                import_node.start_position().column + 1
            );
            let import_name = name.as_deref().unwrap_or("*");
            let import_alias = alias.as_deref().unwrap_or("_");

            cache.modules[ctx.module_index].imports.push(ImportRecord {
                import_id: stable_id(
                    "I",
                    SCHEMA_VERSION,
                    &[
                        &ctx.file.relative_path,
                        &normalized_module,
                        import_name,
                        import_alias,
                        &label,
                    ],
                ),
                module: normalized_module.clone(),
                name,
                alias,
                level,
                resolution: "parsed".to_string(),
                span: self.span(ctx.file, ctx.source, import_node),
            });
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
        } else {
            class_name.clone()
        };
        let start = format!(
            "{}:{}",
            node.start_position().row + 1,
            node.start_position().column + 1
        );
        let class_id = stable_id("C", SCHEMA_VERSION, &[&ctx.file.relative_path, &qualified_name, &start]);
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

        let class_index = cache.classes.len();
        cache.classes.push(ClassRecord {
            class_id: class_id.clone(),
            module_id: ctx.module_id.clone(),
            qualified_name: qualified_name.clone(),
            base_exprs,
            resolved_bases: Vec::new(),
            mro_status: "parsed".to_string(),
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
        let qualified_name = if let Some(class_frame) = ctx.class_stack.last() {
            format!("{}.{}", class_frame.qualified_name, function_name)
        } else if let Some(function_frame) = ctx.function_stack.last() {
            format!("{}.{}", function_frame.qualified_name, function_name)
        } else {
            function_name.clone()
        };
        let kind = if ctx.class_stack.last().is_some() {
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
            .map(|parameters| collect_identifiers(self, ctx.source, parameters))
            .unwrap_or_default();

        cache.functions.push(FunctionRecord {
            function_id: function_id.clone(),
            module_id: ctx.module_id.clone(),
            class_id: ctx.class_stack.last().map(|frame| frame.class_id.clone()),
            qualified_name: qualified_name.clone(),
            kind: kind.to_string(),
            params,
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

        if let Some(class_frame) = ctx.class_stack.last() {
            cache.classes[class_frame.cache_index]
                .methods
                .push(function_id.clone());
        }

        ctx.function_stack.push(FunctionFrame {
            function_id,
            qualified_name,
            scope_id,
        });

        if let Some(body) = node.child_by_field_name("body") {
            let mut cursor = body.walk();
            for child in body.named_children(&mut cursor) {
                self.walk(cache, ctx, child);
            }
        }

        ctx.function_stack.pop();
    }

    fn lower_assignment(
        &self,
        cache: &mut AnalysisCache,
        ctx: &mut LoweringContext<'_>,
        node: Node<'_>,
    ) {
        let Some(left) = node.child_by_field_name("left") else {
            return;
        };
        let place = self.place_for_node(ctx, left);
        let right = node.child_by_field_name("right");
        let expr = right
            .map(|value| self.node_text(ctx.source, value))
            .unwrap_or_default();
        let mut deps = Vec::new();

        if let Some(value) = right {
            for use_node in expression_uses(value) {
                if let Some(dep) = self.lower_identifier_use(cache, ctx, use_node, "assign:rhs") {
                    deps.push(dep);
                }
            }
        }

        let location = format!(
            "{}:{}",
            node.start_position().row + 1,
            node.start_position().column + 1
        );
        cache.definitions.push(Definition {
            def_id: stable_id(
                "D",
                SCHEMA_VERSION,
                &[&ctx.file.relative_path, &self.node_text(ctx.source, left), &location],
            ),
            place,
            def_kind: "assign".to_string(),
            scope_id: ctx.current_scope_id().to_string(),
            function_id: ctx.current_function_id().map(str::to_string),
            span: self.span(ctx.file, ctx.source, node),
            expr,
            deps,
        });
    }

    fn lower_return(
        &self,
        cache: &mut AnalysisCache,
        ctx: &mut LoweringContext<'_>,
        node: Node<'_>,
    ) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            for use_node in expression_uses(child) {
                self.lower_identifier_use(cache, ctx, use_node, "return value");
            }
        }
    }

    fn lower_identifier_use(
        &self,
        cache: &mut AnalysisCache,
        ctx: &LoweringContext<'_>,
        node: Node<'_>,
        context: &str,
    ) -> Option<Place> {
        if !matches!(node.kind(), "identifier" | "attribute") {
            return None;
        }

        let place = self.place_for_node(ctx, node);
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
            use_kind: "load".to_string(),
            scope_id: ctx.current_scope_id().to_string(),
            function_id: ctx.current_function_id().map(str::to_string),
            span: self.span(ctx.file, ctx.source, node),
            context: context.to_string(),
        });

        Some(place)
    }

    fn place_for_node(&self, ctx: &LoweringContext<'_>, node: Node<'_>) -> Place {
        match node.kind() {
            "identifier" => {
                let name = self.node_text(ctx.source, node);
                if ctx.function_stack.last().is_some() || ctx.class_stack.last().is_some() {
                    Place::Local {
                        scope_id: ctx.current_scope_id().to_string(),
                        name,
                    }
                } else {
                    Place::Global {
                        module_id: ctx.module_id.clone(),
                        name,
                    }
                }
            }
            "attribute" => {
                let base = node
                    .child_by_field_name("object")
                    .map(|value| self.node_text(ctx.source, value))
                    .unwrap_or_default();
                let attr = node
                    .child_by_field_name("attribute")
                    .map(|value| self.node_text(ctx.source, value))
                    .unwrap_or_default();
                Place::Attribute { base, attr }
            }
            _ => Place::Unknown {
                reason: self.node_text(ctx.source, node),
            },
        }
    }
}

impl LanguageFrontend for PythonFrontend {
    fn parse_files(&self, files: &[SourceFile]) -> Result<AnalysisCache> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .context("failed to load tree-sitter-python")?;

        let mut cache = AnalysisCache {
            tool_version: env!("CARGO_PKG_VERSION").to_string(),
            ..AnalysisCache::default()
        };

        for file in files {
            let source = fs::read_to_string(&file.absolute_path)
                .with_context(|| format!("failed to read {}", file.absolute_path.display()))?;
            let tree = parser
                .parse(&source, None)
                .ok_or_else(|| anyhow!("tree-sitter returned no parse tree for {}", file.relative_path))?;
            self.lower_module(&mut cache, file, &source, tree.root_node());
        }

        Ok(cache)
    }
}

struct LoweringContext<'a> {
    file: &'a SourceFile,
    source: &'a str,
    module_id: String,
    module_scope_id: String,
    module_index: usize,
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
        self.function_stack.last().map(|frame| frame.function_id.as_str())
    }
}

struct ClassFrame {
    class_id: String,
    qualified_name: String,
    scope_id: String,
    cache_index: usize,
}

struct FunctionFrame {
    function_id: String,
    qualified_name: String,
    scope_id: String,
}

fn source_hash(source: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source.as_bytes());
    hex::encode(hasher.finalize())
}

fn collect_identifiers(frontend: &PythonFrontend, source: &str, node: Node<'_>) -> Vec<String> {
    let mut identifiers = Vec::new();
    collect_identifiers_inner(frontend, source, node, &mut identifiers);
    identifiers
}

fn collect_identifiers_inner(
    frontend: &PythonFrontend,
    source: &str,
    node: Node<'_>,
    identifiers: &mut Vec<String>,
) {
    if node.kind() == "identifier" {
        identifiers.push(frontend.node_text(source, node));
        return;
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_identifiers_inner(frontend, source, child, identifiers);
    }
}

fn expression_uses(node: Node<'_>) -> Vec<Node<'_>> {
    let mut nodes = Vec::new();
    collect_expression_uses(node, &mut nodes);
    nodes
}

fn collect_expression_uses<'tree>(node: Node<'tree>, nodes: &mut Vec<Node<'tree>>) {
    match node.kind() {
        "identifier" | "attribute" => nodes.push(node),
        _ => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_expression_uses(child, nodes);
            }
        }
    }
}
