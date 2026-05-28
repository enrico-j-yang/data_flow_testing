use crate::ir::{AnalysisCache, Definition, ImportRecord, Place};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
struct ModuleContext {
    file_path: String,
    module_id: String,
    module_name: String,
    is_package: bool,
}

pub fn resolve_imports(cache: &mut AnalysisCache) {
    let file_paths: BTreeMap<String, String> = cache
        .files
        .iter()
        .map(|file| (file.file_id.clone(), file.path.clone()))
        .collect();
    let module_contexts: Vec<ModuleContext> = cache
        .modules
        .iter()
        .map(|module| ModuleContext {
            file_path: file_paths.get(&module.file_id).cloned().unwrap_or_default(),
            module_id: module.module_id.clone(),
            module_name: module.module_name.clone(),
            is_package: file_paths
                .get(&module.file_id)
                .map(|path| path.ends_with("__init__.py"))
                .unwrap_or(false),
        })
        .collect();
    let module_index: BTreeMap<String, ModuleContext> = module_contexts
        .iter()
        .cloned()
        .map(|ctx| (ctx.module_name.clone(), ctx))
        .collect();
    let exports = infer_exports(cache);
    let import_deps = build_import_dependency_map(cache, &module_contexts, &module_index);

    for module in &mut cache.modules {
        let names = exports
            .get(&module.module_name)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();
        module.exports = names;

        let owner = module_index.get(&module.module_name);
        for import in &mut module.imports {
            import.resolution = owner
                .and_then(|ctx| classify_import_resolution(ctx, import, &module_index))
                .unwrap_or_else(|| "external".to_string());
        }
    }

    for def in &mut cache.definitions {
        let key = import_binding_key(def);
        if let Some(deps) = import_deps.get(&key) {
            def.deps = deps.clone();
        }
    }
}

fn infer_exports(cache: &AnalysisCache) -> BTreeMap<String, BTreeSet<String>> {
    let mut exports = BTreeMap::new();

    for module in &cache.modules {
        let mut names = BTreeSet::new();
        let mut explicit = None;

        for def in cache.definitions.iter().filter(|def| match &def.place {
            Place::Global { module_id, .. } => module_id == &module.module_id,
            _ => false,
        }) {
            let Some(name) = definition_binding_name(def) else {
                continue;
            };

            if name == "__all__" {
                if let Some(parsed) = parse_all_expr(&def.expr) {
                    explicit = Some(parsed);
                }
                continue;
            }

            if !name.starts_with('_') {
                names.insert(name.to_string());
            }
        }

        exports.insert(module.module_name.clone(), explicit.unwrap_or(names));
    }

    exports
}

fn build_import_dependency_map(
    cache: &AnalysisCache,
    module_contexts: &[ModuleContext],
    module_index: &BTreeMap<String, ModuleContext>,
) -> BTreeMap<String, Vec<Place>> {
    let mut deps = BTreeMap::new();

    for module in &cache.modules {
        let Some(owner) = module_contexts
            .iter()
            .find(|ctx| ctx.module_id == module.module_id)
        else {
            continue;
        };

        for import in &module.imports {
            let key = import_record_key(owner, import);
            let resolved = resolve_target_module(owner, import, module_index);
            let places = match import.name.as_deref() {
                Some("*") => Vec::new(),
                Some(name) => {
                    if let Some(target) = resolved.and_then(|name| module_index.get(&name)) {
                        vec![Place::Global {
                            module_id: target.module_id.clone(),
                            name: leaf_name(name).to_string(),
                        }]
                    } else {
                        vec![Place::External {
                            name: import_target_label(import, true),
                        }]
                    }
                }
                None => Vec::new(),
            };
            deps.insert(key, places);
        }
    }

    deps
}

fn classify_import_resolution(
    owner: &ModuleContext,
    import: &ImportRecord,
    module_index: &BTreeMap<String, ModuleContext>,
) -> Option<String> {
    let resolved = resolve_target_module(owner, import, module_index)?;

    if module_index.contains_key(&resolved) {
        if import.level > 0 {
            Some("project-local-relative".to_string())
        } else {
            Some("project-local".to_string())
        }
    } else {
        Some("external".to_string())
    }
}

fn resolve_target_module(
    owner: &ModuleContext,
    import: &ImportRecord,
    module_index: &BTreeMap<String, ModuleContext>,
) -> Option<String> {
    if import.level == 0 {
        return Some(import.module.clone());
    }

    let mut segments: Vec<&str> = owner.module_name.split('.').collect();
    if !owner.is_package {
        segments.pop();
    }

    let levels_up = import.level.saturating_sub(1);
    if levels_up > segments.len() {
        return None;
    }
    let keep = segments.len().saturating_sub(levels_up);
    segments.truncate(keep);

    if !import.module.is_empty() {
        segments.extend(import.module.split('.'));
    }

    if segments.is_empty() {
        return None;
    }

    let candidate = segments.join(".");
    if module_index.contains_key(&candidate) {
        Some(candidate)
    } else {
        Some(candidate)
    }
}

fn parse_all_expr(expr: &str) -> Option<BTreeSet<String>> {
    let text = expr.trim();
    if text.len() < 2 {
        return None;
    }

    let (open, close) = (text.chars().next()?, text.chars().last()?);
    if !matches!((open, close), ('[', ']') | ('(', ')')) {
        return None;
    }

    let inner = &text[1..text.len() - 1];
    let mut names = BTreeSet::new();

    for part in inner.split(',') {
        let item = part.trim();
        if item.is_empty() {
            continue;
        }
        let quoted = item
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'));
        let single_quoted = item
            .strip_prefix('\'')
            .and_then(|value| value.strip_suffix('\''));
        let Some(name) = quoted.or(single_quoted) else {
            return None;
        };
        if !name.is_empty() {
            names.insert(name.to_string());
        }
    }

    Some(names)
}

fn definition_binding_name(def: &Definition) -> Option<&str> {
    match &def.place {
        Place::Local { name, .. } | Place::Global { name, .. } | Place::Closure { name, .. } => {
            Some(name)
        }
        _ => None,
    }
}

fn import_binding_key(def: &Definition) -> String {
    format!(
        "{}:{}:{}:{}:{}",
        def.span.file,
        def.span.line,
        def.span.col,
        def.def_kind,
        definition_binding_name(def).unwrap_or_default()
    )
}

fn import_record_key(owner: &ModuleContext, import: &ImportRecord) -> String {
    format!(
        "{}:{}:{}:{}:{}",
        owner.file_path,
        import.span.line,
        import.span.col,
        import_def_kind(import),
        import_binding_name(import)
    )
}

fn import_binding_name(import: &ImportRecord) -> String {
    if let Some(alias) = &import.alias {
        return alias.clone();
    }

    if let Some(name) = &import.name {
        return leaf_name(name).to_string();
    }

    import
        .module
        .split('.')
        .next()
        .unwrap_or(&import.module)
        .to_string()
}

fn import_def_kind(import: &ImportRecord) -> &'static str {
    if import.name.is_some() {
        "from_import"
    } else {
        "import"
    }
}

fn import_target_label(import: &ImportRecord, include_name: bool) -> String {
    let mut target = String::new();
    if import.level > 0 {
        target.push_str(&".".repeat(import.level));
    }
    target.push_str(&import.module);
    if include_name {
        if let Some(name) = &import.name {
            if !name.is_empty() {
                if !target.is_empty() {
                    target.push(':');
                }
                target.push_str(name);
            }
        }
    }
    target
}

fn leaf_name(value: &str) -> &str {
    value.rsplit('.').next().unwrap_or(value)
}
