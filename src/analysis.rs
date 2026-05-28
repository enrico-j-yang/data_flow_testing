use crate::ids::stable_id;
use crate::ir::{AnalysisCache, DefUseEdge, Place, VarDependencyEdge, SCHEMA_VERSION};
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

    for events in events_by_scope.values_mut() {
        events.sort();
        let mut reaching: BTreeMap<Place, BTreeSet<String>> = BTreeMap::new();

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
                    cache.def_use_edges.push(DefUseEdge {
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
    }
}

pub fn compute_var_dependencies(cache: &mut AnalysisCache) {
    cache.var_dependency_edges.clear();

    for def in &cache.definitions {
        for dep in &def.deps {
            cache.var_dependency_edges.push(VarDependencyEdge {
                edge_id: stable_id("VD", SCHEMA_VERSION, &[&def.def_id, &format!("{dep:?}")]),
                source_place: dep.clone(),
                target_place: def.place.clone(),
                source_id: format!("{dep:?}"),
                target_id: def.def_id.clone(),
                dep_kind: "assignment".to_string(),
                span: def.span.clone(),
            });
        }
    }
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
    use crate::ir::{AnalysisCache, Definition, Place, Use, SCHEMA_VERSION};
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
}
