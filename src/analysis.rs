use crate::ids::stable_id;
use crate::ir::{AnalysisCache, DefUseEdge, Place, SCHEMA_VERSION, VarDependencyEdge};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

pub fn compute_def_use_edges(cache: &mut AnalysisCache) {
    cache.def_use_edges.clear();

    let mut events_by_scope: BTreeMap<String, Vec<ScopeEvent>> = BTreeMap::new();
    let def_places: BTreeMap<String, Place> = cache
        .definitions
        .iter()
        .map(|def| (def.def_id.clone(), def.place.clone()))
        .collect();

    for (index, def) in cache.definitions.iter().enumerate() {
        events_by_scope
            .entry(def.scope_id.clone())
            .or_default()
            .push(ScopeEvent::definition(index, def));
    }

    for (index, use_site) in cache.uses.iter().enumerate() {
        events_by_scope
            .entry(use_site.scope_id.clone())
            .or_default()
            .push(ScopeEvent::usage(index, use_site));
    }

    let mut edges = events_by_scope
        .into_values()
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(|mut events| process_scope_events(&mut events, &def_places))
        .flatten()
        .collect::<Vec<_>>();
    edges.sort_by(|left, right| left.edge_id.cmp(&right.edge_id));
    cache.def_use_edges = edges;
}

pub fn compute_var_dependencies(cache: &mut AnalysisCache) {
    let uses_by_scope_place = cache.uses.iter().fold(
        BTreeMap::<(String, Place), Vec<&crate::ir::Use>>::new(),
        |mut map, use_site| {
            map.entry((use_site.scope_id.clone(), use_site.place.clone()))
                .or_default()
                .push(use_site);
            map
        },
    );

    let mut assignment_edges = cache
        .definitions
        .par_iter()
        .map(|def| {
            def.deps
                .iter()
                .flat_map(|dep| assignment_edges_for_dependency(def, dep, &uses_by_scope_place))
                .collect::<Vec<_>>()
        })
        .reduce(Vec::new, |mut left, mut right| {
            left.append(&mut right);
            left
        });
    let mut call_arg_edges = infer_call_argument_var_dependencies(cache, &assignment_edges);

    assignment_edges.append(&mut call_arg_edges);
    assignment_edges.sort_by(|left, right| left.edge_id.cmp(&right.edge_id));
    cache.var_dependency_edges = assignment_edges;
}

fn assignment_edges_for_dependency(
    def: &crate::ir::Definition,
    dep: &Place,
    uses_by_scope_place: &BTreeMap<(String, Place), Vec<&crate::ir::Use>>,
) -> Vec<VarDependencyEdge> {
    let candidates = uses_by_scope_place
        .get(&(def.scope_id.clone(), dep.clone()))
        .map(|uses| {
            uses.iter()
                .copied()
                .filter(|use_site| span_contains(&def.span, &use_site.span))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if candidates.is_empty() {
        let source_id = stable_id("USYN", SCHEMA_VERSION, &[&def.def_id, &format!("{dep:?}")]);
        return vec![VarDependencyEdge {
            edge_id: stable_id(
                "VD",
                SCHEMA_VERSION,
                &[&source_id, &def.def_id, "assignment"],
            ),
            source_place: dep.clone(),
            target_place: def.place.clone(),
            source_id,
            target_id: def.def_id.clone(),
            dep_kind: "assignment".to_string(),
            span: def.span.clone(),
        }];
    }

    let mut edges = candidates
        .into_iter()
        .map(|use_site| VarDependencyEdge {
            edge_id: stable_id(
                "VD",
                SCHEMA_VERSION,
                &[&use_site.use_id, &def.def_id, "assignment"],
            ),
            source_place: dep.clone(),
            target_place: def.place.clone(),
            source_id: use_site.use_id.clone(),
            target_id: def.def_id.clone(),
            dep_kind: "assignment".to_string(),
            span: def.span.clone(),
        })
        .collect::<Vec<_>>();
    edges.sort_by(|left, right| left.edge_id.cmp(&right.edge_id));
    edges
}

fn span_contains(container: &crate::source::SourceSpan, inner: &crate::source::SourceSpan) -> bool {
    if container.file != inner.file {
        return false;
    }
    if inner.line < container.line || inner.line > container.end_line {
        return false;
    }
    if inner.line == container.line && inner.col < container.col {
        return false;
    }
    if inner.line == container.end_line && container.end_col > 0 && inner.col > container.end_col {
        return false;
    }
    true
}

fn infer_call_argument_var_dependencies(
    cache: &AnalysisCache,
    assignment_edges: &[VarDependencyEdge],
) -> Vec<VarDependencyEdge> {
    let functions_by_id = cache
        .functions
        .iter()
        .map(|function| (function.function_id.clone(), function))
        .collect::<BTreeMap<_, _>>();
    let definitions_by_id = cache
        .definitions
        .iter()
        .map(|definition| (definition.def_id.clone(), definition))
        .collect::<BTreeMap<_, _>>();
    let mut function_places = BTreeMap::<Place, Vec<String>>::new();
    let mut methods_by_class = BTreeMap::<(String, String), Vec<String>>::new();
    let mut caller_class_by_function = BTreeMap::<String, String>::new();
    let mut param_defs_by_function = BTreeMap::<String, BTreeMap<String, String>>::new();

    for function in &cache.functions {
        if let Some(class_id) = &function.class_id {
            caller_class_by_function.insert(function.function_id.clone(), class_id.clone());
            methods_by_class
                .entry((
                    class_id.clone(),
                    function_simple_name(&function.qualified_name),
                ))
                .or_default()
                .push(function.function_id.clone());
        } else {
            function_places
                .entry(Place::Global {
                    module_id: function.module_id.clone(),
                    name: function_simple_name(&function.qualified_name),
                })
                .or_default()
                .push(function.function_id.clone());
        }
    }

    for definition in &cache.definitions {
        if definition.def_kind != "param" {
            continue;
        }
        let Some(function_id) = &definition.function_id else {
            continue;
        };
        let Place::Local { name, .. } = &definition.place else {
            continue;
        };
        param_defs_by_function
            .entry(function_id.clone())
            .or_default()
            .insert(name.clone(), definition.def_id.clone());
    }

    let mut source_places_by_target = BTreeMap::<String, Vec<Place>>::new();
    for edge in assignment_edges {
        if edge.dep_kind != "assignment" {
            continue;
        }
        source_places_by_target
            .entry(edge.target_id.clone())
            .or_default()
            .push(edge.source_place.clone());
    }

    let mut resolved_places = function_places.clone();
    loop {
        let mut changed = false;
        for definition in &cache.definitions {
            if resolved_places.contains_key(&definition.place) {
                continue;
            }
            let Some(source_places) = source_places_by_target.get(&definition.def_id) else {
                continue;
            };
            if source_places.len() != 1 {
                continue;
            }
            let Some(function_ids) = resolved_places.get(&source_places[0]).cloned() else {
                continue;
            };
            resolved_places.insert(definition.place.clone(), function_ids);
            changed = true;
        }
        if !changed {
            break;
        }
    }

    let mut uses_by_line = BTreeMap::<(String, String, usize), Vec<&crate::ir::Use>>::new();
    for use_site in &cache.uses {
        if use_site.span.line == 0 {
            continue;
        }
        uses_by_line
            .entry((
                use_site.scope_id.clone(),
                use_site.span.file.clone(),
                use_site.span.line,
            ))
            .or_default()
            .push(use_site);
    }

    let mut edges = BTreeMap::<String, VarDependencyEdge>::new();
    for use_sites in uses_by_line.values_mut() {
        use_sites.sort_by(|left, right| {
            left.span
                .col
                .cmp(&right.span.col)
                .then_with(|| left.use_id.cmp(&right.use_id))
        });

        for use_site in use_sites.iter() {
            let function_ids = resolve_function_candidates(
                &use_site.place,
                use_site.function_id.as_deref(),
                &resolved_places,
                &methods_by_class,
                &caller_class_by_function,
            );
            if function_ids.len() != 1 {
                continue;
            }

            let Some(function) = functions_by_id.get(&function_ids[0]) else {
                continue;
            };
            let Some(param_def_ids) =
                ordered_param_definition_ids(function, &param_defs_by_function)
            else {
                continue;
            };
            if param_def_ids.is_empty() {
                continue;
            }

            let mut arg_uses = Vec::new();
            let mut nested_call_detected = false;

            for candidate in use_sites.iter().copied() {
                if candidate.use_id == use_site.use_id {
                    continue;
                }
                if candidate.context != use_site.context {
                    continue;
                }
                if candidate.span.col < use_site.span.end_col {
                    continue;
                }

                let nested_function_ids = resolve_function_candidates(
                    &candidate.place,
                    candidate.function_id.as_deref(),
                    &resolved_places,
                    &methods_by_class,
                    &caller_class_by_function,
                );
                if !nested_function_ids.is_empty() {
                    nested_call_detected = true;
                    break;
                }
                arg_uses.push(candidate);
            }

            if nested_call_detected || arg_uses.len() != param_def_ids.len() {
                continue;
            }

            for (param_def_id, arg_use) in param_def_ids.iter().zip(arg_uses) {
                let Some(param_def) = definitions_by_id.get(param_def_id) else {
                    continue;
                };
                let edge = VarDependencyEdge {
                    edge_id: stable_id(
                        "VD",
                        SCHEMA_VERSION,
                        &[&arg_use.use_id, param_def_id, "call-arg"],
                    ),
                    source_place: arg_use.place.clone(),
                    target_place: param_def.place.clone(),
                    source_id: arg_use.use_id.clone(),
                    target_id: param_def_id.clone(),
                    dep_kind: "call-arg".to_string(),
                    span: arg_use.span.clone(),
                };
                edges.insert(edge.edge_id.clone(), edge);
            }
        }
    }

    edges.into_values().collect()
}

fn resolve_function_candidates(
    place: &Place,
    caller_function_id: Option<&str>,
    resolved_places: &BTreeMap<Place, Vec<String>>,
    methods_by_class: &BTreeMap<(String, String), Vec<String>>,
    caller_class_by_function: &BTreeMap<String, String>,
) -> Vec<String> {
    if let Some(function_ids) = resolved_places.get(place) {
        return function_ids.clone();
    }

    let Place::Attribute { base, attr } = place else {
        return Vec::new();
    };
    if !matches!(base.as_str(), "self" | "cls") {
        return Vec::new();
    }
    let Some(caller_function_id) = caller_function_id else {
        return Vec::new();
    };
    let Some(class_id) = caller_class_by_function.get(caller_function_id) else {
        return Vec::new();
    };

    methods_by_class
        .get(&(class_id.clone(), attr.clone()))
        .cloned()
        .unwrap_or_default()
}

fn ordered_param_definition_ids(
    function: &crate::ir::FunctionRecord,
    param_defs_by_function: &BTreeMap<String, BTreeMap<String, String>>,
) -> Option<Vec<String>> {
    let params = match function.kind.as_str() {
        "method" | "classmethod" => function
            .params
            .iter()
            .filter(|param| param.as_str() != "self" && param.as_str() != "cls")
            .collect::<Vec<_>>(),
        _ => function.params.iter().collect::<Vec<_>>(),
    };
    let param_defs = param_defs_by_function.get(&function.function_id)?;

    params
        .into_iter()
        .map(|param| param_defs.get(param).cloned())
        .collect()
}

fn function_simple_name(qualified_name: &str) -> String {
    qualified_name
        .rsplit("::")
        .next()
        .unwrap_or(qualified_name)
        .rsplit('.')
        .next()
        .unwrap_or(qualified_name)
        .to_string()
}

fn process_scope_events(
    events: &mut [ScopeEvent],
    def_places: &BTreeMap<String, Place>,
) -> Vec<DefUseEdge> {
    events.sort();
    let mut reaching: BTreeMap<Place, BTreeSet<String>> = BTreeMap::new();
    let mut edges = Vec::new();

    for event in events.iter() {
        if event.is_definition {
            reaching.insert(
                event.place.clone(),
                BTreeSet::from([event.record_id.clone()]),
            );
            continue;
        }

        if let Some(def_ids) = reaching.get(&event.place) {
            for def_id in def_ids {
                let place = def_places
                    .get(def_id)
                    .cloned()
                    .unwrap_or_else(|| event.place.clone());
                edges.push(DefUseEdge {
                    edge_id: stable_id("DU", SCHEMA_VERSION, &[def_id, &event.record_id]),
                    def_id: def_id.clone(),
                    use_id: event.record_id.clone(),
                    place,
                    edge_kind: "local".to_string(),
                    path_summary: "ordered scope reaching approximation".to_string(),
                });
            }
        }
    }

    edges
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ScopeEvent {
    sort_line: usize,
    sort_col: usize,
    sort_phase: usize,
    sort_index: usize,
    place: Place,
    record_id: String,
    is_definition: bool,
}

impl ScopeEvent {
    fn definition(index: usize, def: &crate::ir::Definition) -> Self {
        Self {
            sort_line: def.span.end_line,
            sort_col: def.span.end_col,
            sort_phase: 0,
            sort_index: index,
            place: def.place.clone(),
            record_id: def.def_id.clone(),
            is_definition: true,
        }
    }

    fn usage(index: usize, use_site: &crate::ir::Use) -> Self {
        Self {
            sort_line: use_site.span.line,
            sort_col: use_site.span.col,
            sort_phase: 1,
            sort_index: index,
            place: use_site.place.clone(),
            record_id: use_site.use_id.clone(),
            is_definition: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{
        AnalysisCache, Definition, FunctionRecord, ModuleRecord, Place, SCHEMA_VERSION,
        ScopeRecord, Use,
    };
    use crate::source::SourceSpan;

    #[test]
    fn reaching_definitions_connect_definition_to_use() {
        let place = Place::Local {
            scope_id: "S".to_string(),
            name: "x".to_string(),
        };
        let mut cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            ..AnalysisCache::default()
        };
        cache.definitions.push(Definition {
            def_id: "D_x".to_string(),
            place: place.clone(),
            def_kind: "assign".to_string(),
            scope_id: "S".to_string(),
            function_id: Some("F".to_string()),
            span: SourceSpan::synthetic("a.py", "x = 1"),
            expr: "1".to_string(),
            deps: Vec::new(),
        });
        cache.uses.push(Use {
            use_id: "U_x".to_string(),
            place: place.clone(),
            use_kind: "name_load".to_string(),
            scope_id: "S".to_string(),
            function_id: Some("F".to_string()),
            span: SourceSpan::synthetic("a.py", "return x"),
            context: "return".to_string(),
        });

        compute_def_use_edges(&mut cache);
        assert_eq!(cache.def_use_edges.len(), 1);
        assert_eq!(cache.def_use_edges[0].def_id, "D_x");
        assert_eq!(cache.def_use_edges[0].use_id, "U_x");
    }

    #[test]
    fn later_definition_kills_earlier_same_place() {
        let place = Place::Local {
            scope_id: "S".to_string(),
            name: "x".to_string(),
        };
        let mut cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            ..AnalysisCache::default()
        };
        cache.definitions.push(Definition {
            def_id: "D_x_1".to_string(),
            place: place.clone(),
            def_kind: "assign".to_string(),
            scope_id: "S".to_string(),
            function_id: Some("F".to_string()),
            span: SourceSpan::synthetic("a.py", "x = 1"),
            expr: "1".to_string(),
            deps: Vec::new(),
        });
        cache.definitions.push(Definition {
            def_id: "D_x_2".to_string(),
            place: place.clone(),
            def_kind: "assign".to_string(),
            scope_id: "S".to_string(),
            function_id: Some("F".to_string()),
            span: SourceSpan::synthetic("a.py", "x = 2"),
            expr: "2".to_string(),
            deps: Vec::new(),
        });
        cache.uses.push(Use {
            use_id: "U_x".to_string(),
            place: place.clone(),
            use_kind: "name_load".to_string(),
            scope_id: "S".to_string(),
            function_id: Some("F".to_string()),
            span: SourceSpan::synthetic("a.py", "return x"),
            context: "return".to_string(),
        });

        compute_def_use_edges(&mut cache);
        assert_eq!(cache.def_use_edges.len(), 1);
        assert_eq!(cache.def_use_edges[0].def_id, "D_x_2");
        assert_eq!(cache.def_use_edges[0].use_id, "U_x");
    }

    #[test]
    fn var_dependencies_use_real_use_ids_and_include_call_argument_edges() {
        let handle_fn_span = SourceSpan {
            file: "utils/text_utils.py".to_string(),
            line: 53,
            col: 1,
            end_line: 53,
            end_col: 32,
            snippet: "def handle_str(result: str) -> str:".to_string(),
        };
        let calculate_fn_span = SourceSpan {
            file: "utils/text_utils.py".to_string(),
            line: 201,
            col: 1,
            end_line: 201,
            end_col: 55,
            snippet: "def calculate_accuracy(expected: str, actual: str) -> float:".to_string(),
        };
        let handle_expected_use_span = SourceSpan {
            file: "utils/text_utils.py".to_string(),
            line: 210,
            col: 26,
            end_line: 210,
            end_col: 36,
            snippet: "handle_expected = handle_str(expected)".to_string(),
        };
        let expected_use_span = SourceSpan {
            file: "utils/text_utils.py".to_string(),
            line: 210,
            col: 37,
            end_line: 210,
            end_col: 45,
            snippet: "handle_expected = handle_str(expected)".to_string(),
        };
        let handle_actual_use_span = SourceSpan {
            file: "utils/text_utils.py".to_string(),
            line: 211,
            col: 24,
            end_line: 211,
            end_col: 34,
            snippet: "handle_actual = handle_str(actual)".to_string(),
        };
        let actual_use_span = SourceSpan {
            file: "utils/text_utils.py".to_string(),
            line: 211,
            col: 35,
            end_line: 211,
            end_col: 41,
            snippet: "handle_actual = handle_str(actual)".to_string(),
        };
        let handle_expected_def_span = SourceSpan {
            file: "utils/text_utils.py".to_string(),
            line: 210,
            col: 8,
            end_line: 210,
            end_col: 45,
            snippet: "handle_expected = handle_str(expected)".to_string(),
        };
        let handle_actual_def_span = SourceSpan {
            file: "utils/text_utils.py".to_string(),
            line: 211,
            col: 8,
            end_line: 211,
            end_col: 41,
            snippet: "handle_actual = handle_str(actual)".to_string(),
        };
        let mut cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            modules: vec![ModuleRecord {
                module_id: "M_text".to_string(),
                file_id: "F_text".to_string(),
                module_name: "app.utils.text_utils".to_string(),
                exports: Vec::new(),
                imports: Vec::new(),
            }],
            scopes: vec![
                ScopeRecord {
                    scope_id: "S_handle".to_string(),
                    scope_kind: "function".to_string(),
                    parent_scope_id: None,
                    owner_id: "FN_handle".to_string(),
                    span: handle_fn_span.clone(),
                },
                ScopeRecord {
                    scope_id: "S_calc".to_string(),
                    scope_kind: "function".to_string(),
                    parent_scope_id: None,
                    owner_id: "FN_calc".to_string(),
                    span: calculate_fn_span.clone(),
                },
            ],
            functions: vec![
                FunctionRecord {
                    function_id: "FN_handle".to_string(),
                    module_id: "M_text".to_string(),
                    class_id: None,
                    qualified_name: "handle_str".to_string(),
                    kind: "function".to_string(),
                    params: vec!["result".to_string()],
                    scope_id: "S_handle".to_string(),
                    span: handle_fn_span.clone(),
                },
                FunctionRecord {
                    function_id: "FN_calc".to_string(),
                    module_id: "M_text".to_string(),
                    class_id: None,
                    qualified_name: "calculate_accuracy".to_string(),
                    kind: "function".to_string(),
                    params: vec!["expected".to_string(), "actual".to_string()],
                    scope_id: "S_calc".to_string(),
                    span: calculate_fn_span.clone(),
                },
            ],
            definitions: vec![
                Definition {
                    def_id: "D_result".to_string(),
                    place: Place::Local {
                        scope_id: "S_handle".to_string(),
                        name: "result".to_string(),
                    },
                    def_kind: "param".to_string(),
                    scope_id: "S_handle".to_string(),
                    function_id: Some("FN_handle".to_string()),
                    span: handle_fn_span,
                    expr: String::new(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_handle_expected".to_string(),
                    place: Place::Local {
                        scope_id: "S_calc".to_string(),
                        name: "handle_expected".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S_calc".to_string(),
                    function_id: Some("FN_calc".to_string()),
                    span: handle_expected_def_span,
                    expr: "handle_str(expected)".to_string(),
                    deps: vec![
                        Place::Global {
                            module_id: "M_text".to_string(),
                            name: "handle_str".to_string(),
                        },
                        Place::Local {
                            scope_id: "S_calc".to_string(),
                            name: "expected".to_string(),
                        },
                    ],
                },
                Definition {
                    def_id: "D_handle_actual".to_string(),
                    place: Place::Local {
                        scope_id: "S_calc".to_string(),
                        name: "handle_actual".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S_calc".to_string(),
                    function_id: Some("FN_calc".to_string()),
                    span: handle_actual_def_span,
                    expr: "handle_str(actual)".to_string(),
                    deps: vec![
                        Place::Global {
                            module_id: "M_text".to_string(),
                            name: "handle_str".to_string(),
                        },
                        Place::Local {
                            scope_id: "S_calc".to_string(),
                            name: "actual".to_string(),
                        },
                    ],
                },
            ],
            uses: vec![
                Use {
                    use_id: "U_handle_expected".to_string(),
                    place: Place::Global {
                        module_id: "M_text".to_string(),
                        name: "handle_str".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_calc".to_string(),
                    function_id: Some("FN_calc".to_string()),
                    span: handle_expected_use_span,
                    context: "assign:rhs".to_string(),
                },
                Use {
                    use_id: "U_expected".to_string(),
                    place: Place::Local {
                        scope_id: "S_calc".to_string(),
                        name: "expected".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_calc".to_string(),
                    function_id: Some("FN_calc".to_string()),
                    span: expected_use_span,
                    context: "assign:rhs".to_string(),
                },
                Use {
                    use_id: "U_handle_actual".to_string(),
                    place: Place::Global {
                        module_id: "M_text".to_string(),
                        name: "handle_str".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_calc".to_string(),
                    function_id: Some("FN_calc".to_string()),
                    span: handle_actual_use_span,
                    context: "assign:rhs".to_string(),
                },
                Use {
                    use_id: "U_actual".to_string(),
                    place: Place::Local {
                        scope_id: "S_calc".to_string(),
                        name: "actual".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_calc".to_string(),
                    function_id: Some("FN_calc".to_string()),
                    span: actual_use_span,
                    context: "assign:rhs".to_string(),
                },
            ],
            ..AnalysisCache::default()
        };

        compute_var_dependencies(&mut cache);

        assert!(cache.var_dependency_edges.iter().any(|edge| {
            edge.dep_kind == "assignment"
                && edge.source_id == "U_handle_expected"
                && edge.target_id == "D_handle_expected"
        }));
        assert!(cache.var_dependency_edges.iter().any(|edge| {
            edge.dep_kind == "assignment"
                && edge.source_id == "U_expected"
                && edge.target_id == "D_handle_expected"
        }));
        assert!(cache.var_dependency_edges.iter().any(|edge| {
            edge.dep_kind == "call-arg"
                && edge.source_id == "U_expected"
                && edge.target_id == "D_result"
        }));
        assert!(cache.var_dependency_edges.iter().any(|edge| {
            edge.dep_kind == "call-arg"
                && edge.source_id == "U_actual"
                && edge.target_id == "D_result"
        }));
        assert!(
            !cache
                .var_dependency_edges
                .iter()
                .any(|edge| edge.source_id.contains("Local"))
        );
    }
}
