use crate::ids::stable_id;
use crate::ir::{AnalysisCache, Place, SCHEMA_VERSION, VarDependencyEdge};
use std::collections::BTreeMap;

pub fn resolve_js_family_bindings(cache: &mut AnalysisCache) {
    resolve_relative_imports(cache);
    resolve_vue_same_file_bridge(cache);
}

fn resolve_relative_imports(cache: &mut AnalysisCache) {
    let modules_by_name = cache
        .modules
        .iter()
        .map(|module| (module.module_name.clone(), module.module_id.clone()))
        .collect::<BTreeMap<_, _>>();
    let exports_by_module = collect_module_exports(cache);

    for definition in &mut cache.definitions {
        if definition.def_kind != "import" {
            continue;
        }
        let Some((specifier, imported_name)) = definition.expr.split_once(':') else {
            continue;
        };
        if !specifier.starts_with("./") && !specifier.starts_with("../") {
            continue;
        }

        let owner_module_name = definition
            .span
            .file
            .split("?script=")
            .next()
            .unwrap_or(&definition.span.file)
            .to_string();
        let Some(target_module_name) =
            resolve_relative_module_name(&owner_module_name, specifier, &modules_by_name)
        else {
            continue;
        };
        let Some(target_module_id) = modules_by_name.get(&target_module_name) else {
            continue;
        };
        if exports_by_module
            .get(target_module_id)
            .map(|exports| exports.iter().any(|name| name == imported_name))
            .unwrap_or(false)
        {
            definition.deps = vec![Place::Global {
                module_id: target_module_id.clone(),
                name: imported_name.to_string(),
            }];
        }
    }
}

fn collect_module_exports(cache: &AnalysisCache) -> BTreeMap<String, Vec<String>> {
    let mut exports = BTreeMap::<String, Vec<String>>::new();
    for definition in &cache.definitions {
        if let Place::Global { module_id, name } = &definition.place {
            exports
                .entry(module_id.clone())
                .or_default()
                .push(name.clone());
        }
    }
    exports
}

fn resolve_relative_module_name(
    owner: &str,
    specifier: &str,
    modules_by_name: &BTreeMap<String, String>,
) -> Option<String> {
    let owner_dir = owner.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
    let combined = if owner_dir.is_empty() {
        specifier.to_string()
    } else {
        format!("{owner_dir}/{specifier}")
    };
    let base = normalize_relative_path(&combined);

    if let Some(module_name) = find_module_key(&base, modules_by_name) {
        return Some(module_name);
    }
    for suffix in [".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".vue"] {
        let candidate = format!("{base}{suffix}");
        if let Some(module_name) = find_module_key(&candidate, modules_by_name) {
            return Some(module_name);
        }
    }
    for index in [
        "index.ts",
        "index.tsx",
        "index.js",
        "index.jsx",
        "index.mjs",
        "index.cjs",
    ] {
        let candidate = format!("{base}/{index}");
        if let Some(module_name) = find_module_key(&candidate, modules_by_name) {
            return Some(module_name);
        }
    }
    None
}

fn find_module_key(candidate: &str, modules_by_name: &BTreeMap<String, String>) -> Option<String> {
    if modules_by_name.contains_key(candidate) {
        return Some(candidate.to_string());
    }
    let vue_prefix = format!("{candidate}?script=");
    modules_by_name
        .keys()
        .find(|module_name| module_name.starts_with(&vue_prefix))
        .cloned()
}

fn normalize_relative_path(path: &str) -> String {
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            value => parts.push(value),
        }
    }
    parts.join("/")
}

fn resolve_vue_same_file_bridge(cache: &mut AnalysisCache) {
    let mut normal_defs = BTreeMap::<(String, String), (String, Place)>::new();
    for definition in &cache.definitions {
        if !definition.span.file.contains(".vue?script=normal") {
            continue;
        }
        let name = match &definition.place {
            Place::Global { name, .. } | Place::Local { name, .. } => name,
            _ => continue,
        };
        let Some(vue_file) = definition.span.file.split("?script=").next() else {
            continue;
        };
        normal_defs.insert(
            (vue_file.to_string(), name.clone()),
            (definition.def_id.clone(), definition.place.clone()),
        );
    }

    for use_site in &cache.uses {
        if !use_site.span.file.contains(".vue?script=setup") {
            continue;
        }
        let Place::Local { name, .. } = &use_site.place else {
            continue;
        };
        let Some(vue_file) = use_site.span.file.split("?script=").next() else {
            continue;
        };
        let Some((def_id, source_place)) = normal_defs.get(&(vue_file.to_string(), name.clone()))
        else {
            continue;
        };
        cache.var_dependency_edges.push(VarDependencyEdge {
            edge_id: stable_id(
                "VD",
                SCHEMA_VERSION,
                &[def_id, &use_site.use_id, "vue-script-setup"],
            ),
            source_place: source_place.clone(),
            target_place: use_site.place.clone(),
            source_id: def_id.clone(),
            target_id: use_site.use_id.clone(),
            dep_kind: "vue-script-setup".to_string(),
            span: use_site.span.clone(),
        });
    }
}
