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

#[test]
fn c_frontend_normalizes_field_and_subscript_places() {
    let cache = parse_c(
        r#"
struct Item { int value; };

int read_item(struct Item *item, int index, int *values) {
    item->value = values[index];
    return item->value;
}
"#,
    );

    assert!(
        cache.definitions.iter().any(|definition| {
            matches!(
                &definition.place,
                Place::Attribute { base, attr }
                    if base == "item" && attr == "value"
            )
        }),
        "expected an attribute definition for item->value, got: {:?}",
        cache
            .definitions
            .iter()
            .map(|d| &d.place)
            .collect::<Vec<_>>(),
    );
    assert!(
        cache.uses.iter().any(|use_site| {
            matches!(
                &use_site.place,
                Place::Subscript { base, index }
                    if base == "values" && index == "index"
            )
        }),
        "expected a subscript use of values[index], got: {:?}",
        cache.uses.iter().map(|u| &u.place).collect::<Vec<_>>(),
    );
}

#[test]
fn c_frontend_records_calls_and_emits_cfg() {
    let cache = parse_c(
        r#"
int helper(int value) { return value; }

int run(int value, int (*fn_ptr)(int)) {
    if (value > 0) {
        value = helper(value);
    } else {
        value = fn_ptr(value);
    }
    while (value > 10) {
        value = value - 1;
    }
    return value;
}
"#,
    );

    let run_fn = cache
        .functions
        .iter()
        .find(|function| function.qualified_name == "run")
        .expect("run function recorded");
    let cfg = cache
        .cfgs
        .iter()
        .find(|cfg| cfg.function_id == run_fn.function_id)
        .expect("cfg recorded for run");

    assert!(
        cache.calls.iter().any(|call| call.callee_expr == "helper"),
        "expected a call to helper, got: {:?}",
        cache
            .calls
            .iter()
            .map(|c| &c.callee_expr)
            .collect::<Vec<_>>(),
    );
    assert!(
        cache.calls.iter().any(|call| call.callee_expr == "fn_ptr"),
        "expected an indirect call via fn_ptr, got: {:?}",
        cache
            .calls
            .iter()
            .map(|c| &c.callee_expr)
            .collect::<Vec<_>>(),
    );
    assert!(
        cfg.edges.iter().any(|edge| edge.edge_kind == "branch-true"),
        "expected branch-true edge in cfg"
    );
    assert!(
        cfg.edges
            .iter()
            .any(|edge| edge.edge_kind == "branch-false"),
        "expected branch-false edge in cfg"
    );
    assert!(
        cfg.edges.iter().any(|edge| edge.edge_kind == "loop-back"),
        "expected loop-back edge in cfg"
    );
}

fn parse_c_units(units: &[(&str, &str)]) -> AnalysisCache {
    let units = units
        .iter()
        .map(|(path, text)| SourceUnit {
            absolute_path: std::path::PathBuf::from(path),
            relative_path: (*path).to_string(),
            source_text: (*text).to_string(),
            original_path: None,
            line_markers: Vec::new(),
        })
        .collect::<Vec<_>>();
    CFrontend::new().parse_units(&units).unwrap()
}

#[test]
fn c_symbol_resolution_links_project_local_calls() {
    let mut cache = parse_c_units(&[
        (
            "helper.c",
            r#"
int helper(int value) { return value + 1; }
"#,
        ),
        (
            "main.c",
            r#"
int helper(int value);
int run(int input) { return helper(input); }
"#,
        ),
    ]);

    data_flow_analyzer::csymbols::resolve_c_symbols(&mut cache);

    let call = cache
        .calls
        .iter()
        .find(|call| call.callee_expr == "helper")
        .expect("a call to helper was recorded");
    assert_eq!(call.resolution, "project-local");
    assert_eq!(call.candidate_function_ids.len(), 1);
}

#[test]
fn c_summary_propagation_links_argument_to_return_target() {
    use data_flow_analyzer::analysis::{compute_def_use_edges, compute_var_dependencies};
    use data_flow_analyzer::summaries::{build_initial_summaries, propagate_call_summaries};

    let mut cache = parse_c_units(&[
        (
            "helper.c",
            r#"
int helper(int value) { return value; }
"#,
        ),
        (
            "main.c",
            r#"
int helper(int value);
int run(int input) {
    int result = helper(input);
    return result;
}
"#,
        ),
    ]);

    data_flow_analyzer::csymbols::resolve_c_symbols(&mut cache);
    compute_def_use_edges(&mut cache);
    compute_var_dependencies(&mut cache);
    build_initial_summaries(&mut cache);
    propagate_call_summaries(&mut cache);

    assert!(
        cache.function_summaries.iter().any(|summary| {
            let function = cache
                .functions
                .iter()
                .find(|item| item.function_id == summary.function_id)
                .expect("summary function exists");
            function.qualified_name == "run" && !summary.returns.is_empty()
        }),
        "expected run summary to have a non-empty returns list"
    );
    assert!(
        cache
            .var_dependency_edges
            .iter()
            .any(|edge| edge.dep_kind == "call-return"),
        "expected a call-return variable-dependency edge"
    );
}
