use data_flow_analyzer::fs::SourceFile;
use data_flow_analyzer::ir::{AnalysisCache, Place};
use data_flow_analyzer::lang::python::PythonFrontend;
use data_flow_analyzer::lang::LanguageFrontend;
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
