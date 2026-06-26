use data_flow_analyzer::ir::{AnalysisCache, Place};
use data_flow_analyzer::lang::LanguageFrontend;
use data_flow_analyzer::lang::c::CFrontend;
use data_flow_analyzer::source::SourceUnit;

fn parse_c(source_text: &str) -> AnalysisCache {
    let unit = SourceUnit {
        absolute_path: std::path::PathBuf::from("sample.c"),
        relative_path: "sample.c".to_string(),
        source_text: source_text.to_string(),
        original_path: None,
        line_markers: Vec::new(),
    };

    CFrontend::new().parse_units(&[unit]).unwrap()
}

#[test]
fn c_frontend_extracts_functions_params_and_returns() {
    let cache = parse_c(
        r#"
int add_one(int value) {
    int next = value + 1;
    return next;
}
"#,
    );

    let function = cache
        .functions
        .iter()
        .find(|item| item.qualified_name == "add_one")
        .expect("add_one function recorded");

    assert_eq!(function.params, vec!["value".to_string()]);

    assert!(cache.definitions.iter().any(|definition| {
        definition.function_id.as_deref() == Some(function.function_id.as_str())
            && definition.def_kind == "assign"
            && matches!(
                &definition.place,
                Place::Local { scope_id, name }
                    if scope_id == &function.scope_id && name == "next"
            )
    }));

    assert!(cache.uses.iter().any(|use_site| {
        use_site.function_id.as_deref() == Some(function.function_id.as_str())
            && use_site.context == "return value"
    }));
}

#[test]
fn c_frontend_records_global_and_local_assignments() {
    let cache = parse_c(
        r#"
int counter = 0;

int bump(int delta) {
    counter = counter + delta;
    return counter;
}
"#,
    );

    let module = cache.modules.first().expect("module recorded");

    assert!(cache.definitions.iter().any(|definition| {
        definition.function_id.is_none()
            && matches!(
                &definition.place,
                Place::Global { module_id, name }
                    if module_id == &module.module_id && name == "counter"
            )
    }));
}
