use crate::ids::stable_id;
use crate::ir::{AnalysisCache, FunctionSummary, Place, SCHEMA_VERSION, VarDependencyEdge};
use std::collections::{BTreeMap, BTreeSet};

pub fn build_initial_summaries(cache: &mut AnalysisCache) {
    cache.function_summaries.clear();

    for function in &cache.functions {
        let inputs = function
            .params
            .iter()
            .map(|name| Place::Local {
                scope_id: function.scope_id.clone(),
                name: name.clone(),
            })
            .collect::<Vec<_>>();
        let returns = collect_return_places(cache, &function.function_id);
        let writes = collect_write_places(cache, &function.function_id);

        cache.function_summaries.push(FunctionSummary {
            function_id: function.function_id.clone(),
            inputs,
            returns,
            yields: Vec::new(),
            writes,
            raises: Vec::new(),
            external_effects: Vec::new(),
            fixpoint_status: "initial".to_string(),
        });
    }
}

pub fn merge_candidate_summaries(cache: &AnalysisCache, candidates: &[String]) -> FunctionSummary {
    let mut inputs = BTreeSet::new();
    let mut returns = BTreeSet::new();
    let mut yields = BTreeSet::new();
    let mut writes = BTreeSet::new();
    let mut raises = BTreeSet::new();
    let mut external_effects = BTreeSet::new();
    let mut status = "fixed".to_string();

    for candidate in candidates {
        if let Some(summary) = cache
            .function_summaries
            .iter()
            .find(|summary| &summary.function_id == candidate)
        {
            inputs.extend(summary.inputs.iter().cloned());
            returns.extend(summary.returns.iter().cloned());
            yields.extend(summary.yields.iter().cloned());
            writes.extend(summary.writes.iter().cloned());
            raises.extend(summary.raises.iter().cloned());
            external_effects.extend(summary.external_effects.iter().cloned());
            if summary.fixpoint_status != "fixed" {
                status = "partial".to_string();
            }
        }
    }

    FunctionSummary {
        function_id: "merged".to_string(),
        inputs: inputs.into_iter().collect(),
        returns: returns.into_iter().collect(),
        yields: yields.into_iter().collect(),
        writes: writes.into_iter().collect(),
        raises: raises.into_iter().collect(),
        external_effects: external_effects.into_iter().collect(),
        fixpoint_status: status,
    }
}

pub fn propagate_call_summaries(cache: &mut AnalysisCache) {
    let mut summary_by_id: BTreeMap<String, FunctionSummary> = cache
        .function_summaries
        .iter()
        .cloned()
        .map(|summary| (summary.function_id.clone(), summary))
        .collect();
    let uses_by_id = cache
        .uses
        .iter()
        .map(|use_site| (use_site.use_id.clone(), use_site.place.clone()))
        .collect::<BTreeMap<_, _>>();
    let defs_by_id = cache
        .definitions
        .iter()
        .map(|def| (def.def_id.clone(), def.place.clone()))
        .collect::<BTreeMap<_, _>>();
    let base_edges = cache
        .var_dependency_edges
        .iter()
        .filter(|edge| edge.dep_kind != "call-return")
        .cloned()
        .collect::<Vec<_>>();
    let mut propagated_edges = BTreeMap::new();

    let mut stabilized = false;
    for _ in 0..20 {
        let before = summary_by_id.clone();

        for call in &cache.calls {
            let Some(caller_id) = &call.function_id else {
                continue;
            };
            let Some(caller) = summary_by_id.get(caller_id).cloned() else {
                continue;
            };

            let merged =
                merge_candidate_summaries_from_map(&summary_by_id, &call.candidate_function_ids);
            let mut updated = caller.clone();
            merge_places(&mut updated.returns, &merged.returns);
            merge_places(&mut updated.writes, &merged.writes);
            merge_places(&mut updated.raises, &merged.raises);
            merge_places(&mut updated.yields, &merged.yields);
            merge_strings(&mut updated.external_effects, &merged.external_effects);

            if matches!(call.resolution.as_str(), "external" | "unresolved") {
                updated
                    .external_effects
                    .push(format!("call:{}:{}", call.resolution, call.callee_expr));
            }

            if call.candidate_function_ids.is_empty() || merged.fixpoint_status != "fixed" {
                updated.fixpoint_status = "partial".to_string();
            } else {
                updated.fixpoint_status = "fixed".to_string();
            }

            summary_by_id.insert(caller_id.clone(), dedup_summary(updated));

            if let Some(target_def_id) = &call.return_target_def_id {
                if let Some(target_place) = defs_by_id.get(target_def_id) {
                    for arg_use_id in &call.arg_use_ids {
                        if let Some(source_place) = uses_by_id.get(arg_use_id) {
                            let edge = VarDependencyEdge {
                                edge_id: stable_id(
                                    "VD",
                                    SCHEMA_VERSION,
                                    &[arg_use_id, target_def_id, &call.call_id],
                                ),
                                source_place: source_place.clone(),
                                target_place: target_place.clone(),
                                source_id: arg_use_id.clone(),
                                target_id: target_def_id.clone(),
                                dep_kind: "call-return".to_string(),
                                span: call.span.clone(),
                            };
                            propagated_edges.insert(edge.edge_id.clone(), edge);
                        }
                    }
                }
            }
        }

        if summary_by_id == before {
            stabilized = true;
            break;
        }
    }

    for summary in summary_by_id.values_mut() {
        if !stabilized {
            summary.fixpoint_status = "partial".to_string();
        }
    }

    cache.var_dependency_edges = base_edges;
    cache
        .var_dependency_edges
        .extend(propagated_edges.into_values());
    cache.function_summaries = summary_by_id.into_values().collect();
    cache
        .function_summaries
        .sort_by(|left, right| left.function_id.cmp(&right.function_id));
}

fn collect_return_places(cache: &AnalysisCache, function_id: &str) -> Vec<Place> {
    cache
        .uses
        .iter()
        .filter(|use_site| {
            use_site.function_id.as_deref() == Some(function_id)
                && use_site.context == "return value"
        })
        .map(|use_site| use_site.place.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn collect_write_places(cache: &AnalysisCache, function_id: &str) -> Vec<Place> {
    cache
        .definitions
        .iter()
        .filter(|definition| definition.function_id.as_deref() == Some(function_id))
        .map(|definition| definition.place.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn merge_places(target: &mut Vec<Place>, incoming: &[Place]) {
    let mut merged = target.iter().cloned().collect::<BTreeSet<_>>();
    merged.extend(incoming.iter().cloned());
    *target = merged.into_iter().collect();
}

fn merge_strings(target: &mut Vec<String>, incoming: &[String]) {
    let mut merged = target.iter().cloned().collect::<BTreeSet<_>>();
    merged.extend(incoming.iter().cloned());
    *target = merged.into_iter().collect();
}

fn dedup_summary(mut summary: FunctionSummary) -> FunctionSummary {
    summary.inputs = summary
        .inputs
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    summary.returns = summary
        .returns
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    summary.yields = summary
        .yields
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    summary.writes = summary
        .writes
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    summary.raises = summary
        .raises
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    summary.external_effects = summary
        .external_effects
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    summary
}

fn merge_candidate_summaries_from_map(
    summary_by_id: &BTreeMap<String, FunctionSummary>,
    candidates: &[String],
) -> FunctionSummary {
    let mut inputs = BTreeSet::new();
    let mut returns = BTreeSet::new();
    let mut yields = BTreeSet::new();
    let mut writes = BTreeSet::new();
    let mut raises = BTreeSet::new();
    let mut external_effects = BTreeSet::new();
    let mut status = "fixed".to_string();

    for candidate in candidates {
        if let Some(summary) = summary_by_id.get(candidate) {
            inputs.extend(summary.inputs.iter().cloned());
            returns.extend(summary.returns.iter().cloned());
            yields.extend(summary.yields.iter().cloned());
            writes.extend(summary.writes.iter().cloned());
            raises.extend(summary.raises.iter().cloned());
            external_effects.extend(summary.external_effects.iter().cloned());
            if summary.fixpoint_status != "fixed" {
                status = "partial".to_string();
            }
        }
    }

    FunctionSummary {
        function_id: "merged".to_string(),
        inputs: inputs.into_iter().collect(),
        returns: returns.into_iter().collect(),
        yields: yields.into_iter().collect(),
        writes: writes.into_iter().collect(),
        raises: raises.into_iter().collect(),
        external_effects: external_effects.into_iter().collect(),
        fixpoint_status: status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{AnalysisCache, FunctionRecord, FunctionSummary, Place, SCHEMA_VERSION};
    use crate::source::SourceSpan;

    #[test]
    fn summaries_merge_multi_target_outputs() {
        let mut cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            ..AnalysisCache::default()
        };
        cache.functions.push(FunctionRecord {
            function_id: "F_a".to_string(),
            module_id: "M".to_string(),
            class_id: None,
            qualified_name: "a".to_string(),
            kind: "function".to_string(),
            params: vec!["x".to_string()],
            scope_id: "S_a".to_string(),
            span: SourceSpan::synthetic("a.py", "def a"),
        });
        cache.function_summaries.push(FunctionSummary {
            function_id: "F_a".to_string(),
            inputs: vec![Place::Local {
                scope_id: "S_a".to_string(),
                name: "x".to_string(),
            }],
            returns: vec![Place::Local {
                scope_id: "S_a".to_string(),
                name: "x".to_string(),
            }],
            yields: Vec::new(),
            writes: Vec::new(),
            raises: Vec::new(),
            external_effects: Vec::new(),
            fixpoint_status: "fixed".to_string(),
        });

        let merged = merge_candidate_summaries(&cache, &["F_a".to_string()]);
        assert_eq!(merged.returns.len(), 1);
        assert_eq!(merged.fixpoint_status, "fixed");
    }
}
