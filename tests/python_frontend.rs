use data_flow_analyzer::analysis::compute_def_use_edges;
use data_flow_analyzer::config::AnalyzeConfig;
use data_flow_analyzer::fs::{discover_sources, SourceFile};
use data_flow_analyzer::ir::{AnalysisCache, Place};
use data_flow_analyzer::imports::resolve_imports;
use data_flow_analyzer::lang::python::PythonFrontend;
use data_flow_analyzer::lang::LanguageFrontend;
use data_flow_analyzer::paths::{query_function_paths, PathQueryOptions};
use std::fs;

fn parse_python(source_text: &str) -> AnalysisCache {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sample.py");
    fs::write(&path, source_text).unwrap();

    let source = SourceFile {
        absolute_path: path,
        relative_path: "sample.py".to_string(),
    };

    PythonFrontend::new().parse_files(&[source]).unwrap()
}

fn import_records(cache: &AnalysisCache) -> Vec<(String, Option<String>, Option<String>)> {
    cache
        .imports()
        .into_iter()
        .map(|import| {
            (
                import.module.clone(),
                import.name.clone(),
                import.alias.clone(),
            )
        })
        .collect()
}

#[test]
fn python_frontend_extracts_core_ir() {
    let cache = parse_python(
        r#"
from app.config import settings as cfg

class Child(Base):
    class_value = 1

    def method(self, x):
        y = x + self.class_value
        return y
"#,
    );

    assert_eq!(cache.modules.len(), 1);
    assert_eq!(
        import_records(&cache),
        vec![(
            "app.config".to_string(),
            Some("settings".to_string()),
            Some("cfg".to_string())
        )]
    );

    let class = cache
        .classes
        .iter()
        .find(|class| class.qualified_name == "Child")
        .unwrap();
    assert_eq!(class.base_exprs, vec!["Base".to_string()]);

    let method = cache
        .functions
        .iter()
        .find(|function| function.qualified_name == "Child.method")
        .unwrap();
    assert_eq!(method.kind, "method");
    assert_eq!(method.params, vec!["self".to_string(), "x".to_string()]);

    assert!(cache.definitions.iter().any(|definition| {
        definition.function_id.as_deref() == Some(method.function_id.as_str())
            && matches!(
                &definition.place,
                Place::Local { scope_id, name }
                    if scope_id == &method.scope_id && name == "y"
            )
            && definition.def_kind == "assign"
            && definition.expr == "x + self.class_value"
    }));

    assert!(cache.uses.iter().any(|use_record| {
        use_record.function_id.as_deref() == Some(method.function_id.as_str())
            && use_record.context == "return value"
            && matches!(
                &use_record.place,
                Place::Local { scope_id, name }
                    if scope_id == &method.scope_id && name == "y"
            )
    }));
}

#[test]
fn python_frontend_handles_scope_imports_parameters_and_multi_target_assignments() {
    let cache = parse_python(
        r#"
import os
import pkg.mod as pkg_alias
from app.config import settings as cfg

GLOBAL_VALUE = cfg

class Child:
    def outer(self, typed: Config, default=os.path, pair=(cfg, pkg_alias)):
        local = GLOBAL_VALUE
        alias = mirror = local
        first, second = pair

        def inner(value=GLOBAL_VALUE):
            return value

        return local
"#,
    );

    assert_eq!(
        import_records(&cache),
        vec![
            ("os".to_string(), None, None),
            ("pkg.mod".to_string(), None, Some("pkg_alias".to_string())),
            (
                "app.config".to_string(),
                Some("settings".to_string()),
                Some("cfg".to_string())
            ),
        ]
    );

    let outer = cache
        .functions
        .iter()
        .find(|function| function.qualified_name == "Child.outer")
        .unwrap();
    assert_eq!(
        outer.params,
        vec![
            "self".to_string(),
            "typed".to_string(),
            "default".to_string(),
            "pair".to_string()
        ]
    );

    let inner = cache
        .functions
        .iter()
        .find(|function| function.qualified_name == "Child.outer.inner")
        .unwrap();
    let class = cache
        .classes
        .iter()
        .find(|class| class.qualified_name == "Child")
        .unwrap();

    assert_eq!(outer.class_id.as_deref(), Some(class.class_id.as_str()));
    assert_eq!(inner.kind, "function");
    assert_eq!(inner.params, vec!["value".to_string()]);
    assert_eq!(inner.class_id, None);
    assert_eq!(class.methods, vec![outer.function_id.clone()]);
    assert!(cache
        .functions
        .iter()
        .all(|function| function.qualified_name != "Child.inner"));

    assert!(cache.uses.iter().any(|use_record| {
        use_record.function_id.as_deref() == Some(outer.function_id.as_str())
            && use_record.context == "assign:rhs"
            && matches!(
                &use_record.place,
                Place::Global { module_id, name }
                    if module_id == &outer.module_id && name == "GLOBAL_VALUE"
            )
    }));

    for expected_name in ["local", "alias", "mirror", "first", "second"] {
        assert!(cache.definitions.iter().any(|definition| {
            definition.function_id.as_deref() == Some(outer.function_id.as_str())
                && definition.def_kind == "assign"
                && matches!(
                    &definition.place,
                    Place::Local { scope_id, name }
                        if scope_id == &outer.scope_id && name == expected_name
                )
        }));
    }

    assert!(cache.uses.iter().any(|use_record| {
        use_record.function_id.as_deref() == Some(outer.function_id.as_str())
            && use_record.context == "assign:rhs"
            && matches!(
                &use_record.place,
                Place::Local { scope_id, name }
                    if scope_id == &outer.scope_id && name == "pair"
            )
    }));
}

#[test]
fn python_frontend_lowers_nonlocal_rebinding_as_closure_access() {
    let cache = parse_python(
        r#"
def outer():
    x = 0

    def inner():
        nonlocal x
        x = x + 1
        return x
"#,
    );

    let outer = cache
        .functions
        .iter()
        .find(|function| function.qualified_name == "outer")
        .unwrap();
    let inner = cache
        .functions
        .iter()
        .find(|function| function.qualified_name == "outer.inner")
        .unwrap();

    assert!(cache.definitions.iter().any(|definition| {
        definition.function_id.as_deref() == Some(inner.function_id.as_str())
            && definition.expr == "x + 1"
            && matches!(
                &definition.place,
                Place::Closure { scope_id, name }
                    if scope_id == &outer.scope_id && name == "x"
            )
    }));

    assert!(cache.uses.iter().any(|use_record| {
        use_record.function_id.as_deref() == Some(inner.function_id.as_str())
            && use_record.context == "assign:rhs"
            && matches!(
                &use_record.place,
                Place::Closure { scope_id, name }
                    if scope_id == &outer.scope_id && name == "x"
            )
    }));
}

#[test]
fn python_frontend_records_captures_params_and_bases() {
    let cache = parse_python(
        r#"
class Child(Base):
    def outer(self, value):
        total = value

        def inner(delta):
            nonlocal total
            total = total + delta
            return total

        return inner
"#,
    );

    let child = cache
        .classes
        .iter()
        .find(|class| class.qualified_name == "Child")
        .unwrap();
    let outer = cache
        .functions
        .iter()
        .find(|function| function.qualified_name == "Child.outer")
        .unwrap();
    let inner = cache
        .functions
        .iter()
        .find(|function| function.qualified_name == "Child.outer.inner")
        .unwrap();
    let inner_scope = cache
        .scopes
        .iter()
        .find(|scope| scope.scope_id == inner.scope_id)
        .unwrap();

    assert_eq!(child.base_exprs, vec!["Base".to_string()]);
    assert_eq!(child.mro_status, "local-unresolved");
    assert_eq!(outer.class_id.as_deref(), Some(child.class_id.as_str()));
    assert_eq!(inner.class_id, None);
    assert_eq!(inner_scope.parent_scope_id.as_deref(), Some(outer.scope_id.as_str()));

    assert!(cache.definitions.iter().any(|definition| {
        definition.function_id.as_deref() == Some(outer.function_id.as_str())
            && definition.def_kind == "param"
            && matches!(
                &definition.place,
                Place::Local { scope_id, name }
                    if scope_id == &outer.scope_id && name == "value"
            )
    }));

    assert!(cache.definitions.iter().any(|definition| {
        definition.function_id.as_deref() == Some(inner.function_id.as_str())
            && definition.def_kind == "param"
            && matches!(
                &definition.place,
                Place::Local { scope_id, name }
                    if scope_id == &inner.scope_id && name == "delta"
            )
    }));

    assert!(cache.captures.iter().any(|capture| {
        capture.target_function_id == inner.function_id
            && capture.mode == "read"
            && matches!(
                &capture.place,
                Place::Closure { scope_id, name }
                    if scope_id == &outer.scope_id && name == "total"
            )
    }));

    assert!(cache.captures.iter().any(|capture| {
        capture.target_function_id == inner.function_id
            && capture.mode == "write"
            && matches!(
                &capture.place,
                Place::Closure { scope_id, name }
                    if scope_id == &outer.scope_id && name == "total"
            )
    }));
}

#[test]
fn import_resolver_handles_init_all_and_reexports() {
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path().join("app");
    fs::create_dir_all(&pkg).unwrap();
    fs::write(
        pkg.join("__init__.py"),
        "__all__ = ['settings']\nfrom .config import settings\n",
    )
    .unwrap();
    fs::write(pkg.join("config.py"), "settings = {'debug': True}\n").unwrap();
    fs::write(pkg.join("main.py"), "from app import settings\nvalue = settings\n").unwrap();

    let cfg = AnalyzeConfig {
        input: pkg.clone(),
        ..AnalyzeConfig::default()
    };
    let files = discover_sources(&cfg).unwrap();
    let mut cache = PythonFrontend::new().parse_files(&files).unwrap();

    resolve_imports(&mut cache);

    let imports = cache.imports();
    assert!(imports.iter().any(|import| import.resolution == "project-local"));
    assert!(cache
        .modules
        .iter()
        .any(|module| module.exports.iter().any(|export| export == "settings")));
}

#[test]
fn python_frontend_normalizes_attribute_and_subscript_places() {
    let cache = parse_python(
        r#"
class Child:
    def method(self, items, index, value):
        self.token = value
        local = self.token
        current = items[index]
        items[index] = local
        return current
"#,
    );

    let method = cache
        .functions
        .iter()
        .find(|function| function.qualified_name == "Child.method")
        .unwrap();

    assert!(cache.definitions.iter().any(|definition| {
        definition.function_id.as_deref() == Some(method.function_id.as_str())
            && matches!(
                &definition.place,
                Place::Attribute { base, attr }
                    if base == "InstanceField(Child)" && attr == "token"
            )
    }));

    assert!(cache.uses.iter().any(|use_record| {
        use_record.function_id.as_deref() == Some(method.function_id.as_str())
            && matches!(
                &use_record.place,
                Place::Subscript { base, index }
                    if base == "items" && index == "index"
            )
    }));
}

#[test]
fn python_frontend_emits_baseline_cfg_for_functions() {
    let cache = parse_python(
        r#"
def choose(flag):
    value = 1
    if flag:
        return value
    return 0
"#,
    );

    let function = cache
        .functions
        .iter()
        .find(|function| function.qualified_name == "choose")
        .unwrap();
    let cfg = cache
        .cfgs
        .iter()
        .find(|cfg| cfg.function_id == function.function_id)
        .unwrap();

    assert_eq!(cfg.blocks.iter().filter(|block| block.block_kind == "Entry").count(), 1);
    assert_eq!(cfg.blocks.iter().filter(|block| block.block_kind == "Exit").count(), 1);
    assert!(cfg
        .edges
        .iter()
        .any(|edge| edge.from_block_id == cfg.entry_block_id && edge.edge_kind == "sequence"));
}

#[test]
fn python_frontend_supports_def_use_path_queries() {
    let mut cache = parse_python(
        r#"
def choose():
    value = 1
    return value
"#,
    );

    compute_def_use_edges(&mut cache);

    let function = cache
        .functions
        .iter()
        .find(|function| function.qualified_name == "choose")
        .unwrap();
    let edge = cache
        .def_use_edges
        .iter()
        .find(|edge| edge.edge_kind == "local")
        .unwrap();
    let result = query_function_paths(
        &cache,
        &function.function_id,
        Some(&edge.def_id),
        Some(&edge.use_id),
        PathQueryOptions {
            max_loop_unroll: 2,
            max_paths: 10,
            max_path_len: 10,
        },
    );

    assert_eq!(result.paths.len(), 1);
    assert_eq!(result.paths[0].def_id, edge.def_id);
    assert_eq!(result.paths[0].use_id, edge.use_id);
}
