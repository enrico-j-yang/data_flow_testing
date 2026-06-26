use crate::ir::AnalysisCache;
use std::collections::BTreeMap;

/// Resolve C call sites to project-local function definitions when possible.
/// The C frontend emits every call with `resolution = "unresolved"` and an
/// empty candidate set; this pass walks the function index by simple name and
/// fills in the resolution kind plus `candidate_function_ids`. Calls whose
/// callee expression looks like a dereference (`obj->fn`, `obj.fn`, `*fp`)
/// are marked `indirect`; everything else without a matching definition is
/// marked `external`.
pub fn resolve_c_symbols(cache: &mut AnalysisCache) {
    let mut functions_by_name = BTreeMap::<String, Vec<String>>::new();
    for function in &cache.functions {
        functions_by_name
            .entry(simple_name(&function.qualified_name))
            .or_default()
            .push(function.function_id.clone());
    }

    for call in &mut cache.calls {
        if let Some(candidates) = functions_by_name.get(&call.callee_expr) {
            call.candidate_function_ids = candidates.clone();
            call.resolution = "project-local".to_string();
        } else if is_indirect_call_expr(&call.callee_expr) {
            call.resolution = "indirect".to_string();
        } else {
            call.resolution = "external".to_string();
        }
    }
}

fn simple_name(name: &str) -> String {
    name.rsplit("::").next().unwrap_or(name).to_string()
}

fn is_indirect_call_expr(expr: &str) -> bool {
    expr.contains("->") || expr.contains('.') || expr.starts_with('*') || expr.starts_with('(')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{CallRecord, FunctionRecord};
    use crate::source::SourceSpan;

    fn make_cache() -> AnalysisCache {
        let span = SourceSpan::synthetic("sample.c", "helper(value)");
        AnalysisCache {
            functions: vec![FunctionRecord {
                function_id: "F_helper".to_string(),
                module_id: "M_helper".to_string(),
                class_id: None,
                qualified_name: "helper".to_string(),
                kind: "function".to_string(),
                params: vec!["value".to_string()],
                scope_id: "S_helper".to_string(),
                span: span.clone(),
            }],
            calls: vec![
                CallRecord {
                    call_id: "CALL_1".to_string(),
                    function_id: Some("F_run".to_string()),
                    callee_expr: "helper".to_string(),
                    candidate_function_ids: Vec::new(),
                    resolution: "unresolved".to_string(),
                    arg_use_ids: Vec::new(),
                    return_target_def_id: None,
                    span: span.clone(),
                },
                CallRecord {
                    call_id: "CALL_2".to_string(),
                    function_id: Some("F_run".to_string()),
                    callee_expr: "obj->method".to_string(),
                    candidate_function_ids: Vec::new(),
                    resolution: "unresolved".to_string(),
                    arg_use_ids: Vec::new(),
                    return_target_def_id: None,
                    span: span.clone(),
                },
                CallRecord {
                    call_id: "CALL_3".to_string(),
                    function_id: Some("F_run".to_string()),
                    callee_expr: "printf".to_string(),
                    candidate_function_ids: Vec::new(),
                    resolution: "unresolved".to_string(),
                    arg_use_ids: Vec::new(),
                    return_target_def_id: None,
                    span,
                },
            ],
            ..AnalysisCache::default()
        }
    }

    #[test]
    fn resolves_project_local_calls_and_marks_indirect_external() {
        let mut cache = make_cache();
        resolve_c_symbols(&mut cache);
        assert_eq!(cache.calls[0].resolution, "project-local");
        assert_eq!(cache.calls[0].candidate_function_ids, vec!["F_helper".to_string()]);
        assert_eq!(cache.calls[1].resolution, "indirect");
        assert_eq!(cache.calls[2].resolution, "external");
    }
}
