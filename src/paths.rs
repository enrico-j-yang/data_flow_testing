use crate::ids::stable_id;
use crate::ir::{AnalysisCache, CfgRecord, Definition, Use, SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PathQueryOptions {
    pub max_loop_unroll: usize,
    pub max_paths: usize,
    pub max_path_len: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DefClearPath {
    pub path_id: String,
    pub def_id: String,
    pub use_id: String,
    pub block_ids: Vec<String>,
    pub edge_labels: Vec<String>,
    pub loop_unrolls: BTreeMap<String, usize>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PathQueryResult {
    pub query: String,
    pub paths: Vec<DefClearPath>,
    pub truncated: bool,
    pub truncation_reason: Option<String>,
    pub options: PathQueryOptions,
}

pub fn query_function_paths(
    cache: &AnalysisCache,
    function_id: &str,
    def_id: Option<&str>,
    use_id: Option<&str>,
    options: PathQueryOptions,
) -> PathQueryResult {
    let query = format!(
        "function={function_id};def={};use={}",
        def_id.unwrap_or("*"),
        use_id.unwrap_or("*")
    );
    let Some(cfg) = cache.cfgs.iter().find(|cfg| cfg.function_id == function_id) else {
        return PathQueryResult {
            query,
            paths: Vec::new(),
            truncated: false,
            truncation_reason: Some("function-cfg-not-found".to_string()),
            options,
        };
    };

    let def_map = cache
        .definitions
        .iter()
        .map(|definition| (definition.def_id.clone(), definition))
        .collect::<BTreeMap<_, _>>();
    let use_map = cache
        .uses
        .iter()
        .map(|use_site| (use_site.use_id.clone(), use_site))
        .collect::<BTreeMap<_, _>>();
    let candidates = candidate_pairs(cache, function_id, def_id, use_id, &def_map, &use_map);

    let mut result = PathQueryResult {
        query,
        paths: Vec::new(),
        truncated: false,
        truncation_reason: None,
        options,
    };

    for (candidate_def_id, candidate_use_id) in candidates {
        if result.paths.len() >= result.options.max_paths {
            result.truncated = true;
            result.truncation_reason = Some("max-paths".to_string());
            break;
        }

        let Some(definition) = def_map.get(&candidate_def_id) else {
            continue;
        };
        let Some(use_site) = use_map.get(&candidate_use_id) else {
            continue;
        };
        if definition.place != use_site.place {
            continue;
        }

        let Some(start_block_id) = find_block_for_statement(cfg, &definition.def_id, &definition.span)
        else {
            continue;
        };
        let Some(target_block_id) = find_block_for_statement(cfg, &use_site.use_id, &use_site.span)
        else {
            continue;
        };

        let remaining = result.options.max_paths.saturating_sub(result.paths.len());
        let walk = bounded_walk(
            cache,
            cfg,
            definition,
            use_site,
            &start_block_id,
            &target_block_id,
            &result.options,
            remaining,
        );

        result.paths.extend(walk.paths);
        if walk.truncated && !result.truncated {
            result.truncated = true;
            result.truncation_reason = walk.truncation_reason;
        }
        if result.paths.len() >= result.options.max_paths {
            result.truncated = true;
            result.truncation_reason = Some("max-paths".to_string());
            result.paths.truncate(result.options.max_paths);
            break;
        }
    }

    result
}

fn candidate_pairs(
    cache: &AnalysisCache,
    function_id: &str,
    def_id: Option<&str>,
    use_id: Option<&str>,
    def_map: &BTreeMap<String, &Definition>,
    use_map: &BTreeMap<String, &Use>,
) -> Vec<(String, String)> {
    if let (Some(def_id), Some(use_id)) = (def_id, use_id) {
        return vec![(def_id.to_string(), use_id.to_string())];
    }

    cache
        .def_use_edges
        .iter()
        .filter(|edge| def_id.is_none_or(|value| value == edge.def_id))
        .filter(|edge| use_id.is_none_or(|value| value == edge.use_id))
        .filter(|edge| {
            def_map
                .get(&edge.def_id)
                .and_then(|definition| definition.function_id.as_deref())
                == Some(function_id)
                && use_map
                    .get(&edge.use_id)
                    .and_then(|use_site| use_site.function_id.as_deref())
                    == Some(function_id)
        })
        .map(|edge| (edge.def_id.clone(), edge.use_id.clone()))
        .collect()
}

fn bounded_walk(
    cache: &AnalysisCache,
    cfg: &CfgRecord,
    definition: &Definition,
    use_site: &Use,
    start_block_id: &str,
    target_block_id: &str,
    options: &PathQueryOptions,
    max_paths: usize,
) -> WalkResult {
    let adjacency = cfg
        .edges
        .iter()
        .fold(BTreeMap::<String, Vec<_>>::new(), |mut map, edge| {
            map.entry(edge.from_block_id.clone()).or_default().push(edge);
            map
        });
    let mut state = WalkState {
        cache,
        cfg,
        definition,
        use_site,
        options,
        max_paths,
        paths: Vec::new(),
        truncated: false,
        truncation_reason: None,
    };
    let mut block_ids = vec![start_block_id.to_string()];
    let mut edge_labels = Vec::new();
    let mut edge_visits = BTreeMap::new();
    let mut loop_unrolls = BTreeMap::new();

    state.dfs(
        &adjacency,
        start_block_id,
        target_block_id,
        &mut block_ids,
        &mut edge_labels,
        &mut edge_visits,
        &mut loop_unrolls,
    );

    WalkResult {
        paths: state.paths,
        truncated: state.truncated,
        truncation_reason: state.truncation_reason,
    }
}

fn find_block_for_statement(
    cfg: &CfgRecord,
    statement_id: &str,
    span: &crate::source::SourceSpan,
) -> Option<String> {
    if let Some(block) = cfg
        .blocks
        .iter()
        .find(|block| block.statements.iter().any(|statement| statement == statement_id))
    {
        return Some(block.block_id.clone());
    }

    if let Some(block) = cfg
        .blocks
        .iter()
        .find(|block| span_within(&block.span, span))
    {
        return Some(block.block_id.clone());
    }

    cfg.blocks
        .iter()
        .find(|block| block.block_id != cfg.entry_block_id && block.block_id != cfg.exit_block_id)
        .or_else(|| cfg.blocks.first())
        .map(|block| block.block_id.clone())
}

fn span_within(container: &crate::source::SourceSpan, inner: &crate::source::SourceSpan) -> bool {
    if container.line == 0 || container.end_line == 0 {
        return false;
    }

    (container.line, container.col) <= (inner.line, inner.col)
        && (container.end_line, container.end_col) >= (inner.end_line, inner.end_col)
}

struct WalkState<'a> {
    cache: &'a AnalysisCache,
    cfg: &'a CfgRecord,
    definition: &'a Definition,
    use_site: &'a Use,
    options: &'a PathQueryOptions,
    max_paths: usize,
    paths: Vec<DefClearPath>,
    truncated: bool,
    truncation_reason: Option<String>,
}

impl WalkState<'_> {
    fn dfs(
        &mut self,
        adjacency: &BTreeMap<String, Vec<&crate::ir::CfgEdge>>,
        current_block_id: &str,
        target_block_id: &str,
        block_ids: &mut Vec<String>,
        edge_labels: &mut Vec<String>,
        edge_visits: &mut BTreeMap<String, usize>,
        loop_unrolls: &mut BTreeMap<String, usize>,
    ) {
        if self.paths.len() >= self.max_paths {
            self.record_truncation("max-paths");
            return;
        }
        if block_ids.len() > self.options.max_path_len {
            self.record_truncation("truncated-by-max-path-len");
            return;
        }

        if current_block_id == target_block_id {
            if is_def_clear_path(
                self.cache,
                self.cfg,
                self.definition,
                self.use_site,
                block_ids,
            ) {
                self.paths.push(DefClearPath {
                    path_id: stable_id(
                        "P",
                        SCHEMA_VERSION,
                        &[
                            &self.definition.def_id,
                            &self.use_site.use_id,
                            &self.paths.len().to_string(),
                            &block_ids.join("->"),
                        ],
                    ),
                    def_id: self.definition.def_id.clone(),
                    use_id: self.use_site.use_id.clone(),
                    block_ids: block_ids.clone(),
                    edge_labels: edge_labels.clone(),
                    loop_unrolls: loop_unrolls.clone(),
                    truncated: false,
                });
            }
            return;
        }

        if let Some(edges) = adjacency.get(current_block_id) {
            for edge in edges {
                let visits = edge_visits.get(&edge.edge_id).copied().unwrap_or(0);
                if visits >= self.options.max_loop_unroll {
                    continue;
                }

                edge_visits.insert(edge.edge_id.clone(), visits + 1);
                loop_unrolls.insert(edge.edge_id.clone(), visits + 1);
                block_ids.push(edge.to_block_id.clone());
                edge_labels.push(if edge.label.is_empty() {
                    edge.edge_kind.clone()
                } else {
                    edge.label.clone()
                });

                self.dfs(
                    adjacency,
                    &edge.to_block_id,
                    target_block_id,
                    block_ids,
                    edge_labels,
                    edge_visits,
                    loop_unrolls,
                );

                edge_labels.pop();
                block_ids.pop();
                if visits == 0 {
                    edge_visits.remove(&edge.edge_id);
                    loop_unrolls.remove(&edge.edge_id);
                } else {
                    edge_visits.insert(edge.edge_id.clone(), visits);
                    loop_unrolls.insert(edge.edge_id.clone(), visits);
                }

                if self.paths.len() >= self.max_paths {
                    self.record_truncation("max-paths");
                    return;
                }
            }
        }
    }

    fn record_truncation(&mut self, reason: &str) {
        self.truncated = true;
        if self.truncation_reason.is_none() {
            self.truncation_reason = Some(reason.to_string());
        }
    }
}

fn is_def_clear_path(
    cache: &AnalysisCache,
    cfg: &CfgRecord,
    definition: &Definition,
    use_site: &Use,
    block_ids: &[String],
) -> bool {
    let place = &definition.place;
    let mut seen_def = false;

    for block_id in block_ids {
        let sequence = block_statement_sequence(cache, cfg, block_id, definition.function_id.as_deref());
        for statement_id in sequence {
            if !seen_def {
                if statement_id == definition.def_id {
                    seen_def = true;
                }
                continue;
            }

            if statement_id == use_site.use_id {
                return true;
            }

            if let Some(other_definition) = cache
                .definitions
                .iter()
                .find(|other_definition| other_definition.def_id == statement_id)
            {
                if other_definition.place == *place && other_definition.def_id != definition.def_id {
                    return false;
                }
            }
        }
    }

    false
}

fn block_statement_sequence(
    cache: &AnalysisCache,
    cfg: &CfgRecord,
    block_id: &str,
    function_id: Option<&str>,
) -> Vec<String> {
    let Some(block) = cfg.blocks.iter().find(|block| block.block_id == block_id) else {
        return Vec::new();
    };
    if !block.statements.is_empty() {
        return block.statements.clone();
    }

    collect_function_event_ids(cache, function_id).into_iter().collect()
}

fn collect_function_event_ids(cache: &AnalysisCache, function_id: Option<&str>) -> Vec<String> {
    let mut events = Vec::new();

    for (index, definition) in cache.definitions.iter().enumerate() {
        if definition.function_id.as_deref() != function_id {
            continue;
        }
        events.push(OrderedEvent {
            line: definition.span.end_line,
            col: definition.span.end_col,
            phase: 0,
            index,
            record_id: definition.def_id.clone(),
        });
    }

    for (index, use_site) in cache.uses.iter().enumerate() {
        if use_site.function_id.as_deref() != function_id {
            continue;
        }
        events.push(OrderedEvent {
            line: use_site.span.line,
            col: use_site.span.col,
            phase: 1,
            index,
            record_id: use_site.use_id.clone(),
        });
    }

    events.sort();
    events.into_iter().map(|event| event.record_id).collect()
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct OrderedEvent {
    line: usize,
    col: usize,
    phase: usize,
    index: usize,
    record_id: String,
}

struct WalkResult {
    paths: Vec<DefClearPath>,
    truncated: bool,
    truncation_reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{
        AnalysisCache, CfgBlock, CfgEdge, CfgRecord, Definition, Place, Use, SCHEMA_VERSION,
    };
    use crate::source::SourceSpan;

    #[test]
    fn path_limits_record_truncation_reason() {
        let options = PathQueryOptions {
            max_loop_unroll: 2,
            max_paths: 1,
            max_path_len: 3,
        };
        let result = PathQueryResult {
            query: "D_x -> U_x".to_string(),
            paths: Vec::new(),
            truncated: true,
            truncation_reason: Some("max-paths".to_string()),
            options,
        };

        assert!(result.truncated);
        assert_eq!(result.truncation_reason.as_deref(), Some("max-paths"));
    }

    #[test]
    fn query_finds_def_clear_path_within_single_block() {
        let place = Place::Local {
            scope_id: "S_fn".to_string(),
            name: "x".to_string(),
        };
        let cache = single_function_cache(
            vec![Definition {
                def_id: "D_x".to_string(),
                place: place.clone(),
                def_kind: "assign".to_string(),
                scope_id: "S_fn".to_string(),
                function_id: Some("FN".to_string()),
                span: SourceSpan::synthetic("a.py", "x = 1"),
                expr: "1".to_string(),
                deps: Vec::new(),
            }],
            vec![Use {
                use_id: "U_x".to_string(),
                place,
                use_kind: "load".to_string(),
                scope_id: "S_fn".to_string(),
                function_id: Some("FN".to_string()),
                span: SourceSpan::synthetic("a.py", "return x"),
                context: "return".to_string(),
            }],
            vec!["D_x".to_string(), "U_x".to_string()],
        );

        let result = query_function_paths(
            &cache,
            "FN",
            Some("D_x"),
            Some("U_x"),
            PathQueryOptions {
                max_loop_unroll: 2,
                max_paths: 10,
                max_path_len: 10,
            },
        );

        assert_eq!(result.paths.len(), 1);
        assert_eq!(result.paths[0].def_id, "D_x");
        assert_eq!(result.paths[0].use_id, "U_x");
    }

    #[test]
    fn explicit_query_rejects_path_when_redefinition_intervenes() {
        let place = Place::Local {
            scope_id: "S_fn".to_string(),
            name: "x".to_string(),
        };
        let cache = single_function_cache(
            vec![
                Definition {
                    def_id: "D_x_1".to_string(),
                    place: place.clone(),
                    def_kind: "assign".to_string(),
                    scope_id: "S_fn".to_string(),
                    function_id: Some("FN".to_string()),
                    span: SourceSpan::synthetic("a.py", "x = 1"),
                    expr: "1".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_x_2".to_string(),
                    place: place.clone(),
                    def_kind: "assign".to_string(),
                    scope_id: "S_fn".to_string(),
                    function_id: Some("FN".to_string()),
                    span: SourceSpan::synthetic("a.py", "x = 2"),
                    expr: "2".to_string(),
                    deps: Vec::new(),
                },
            ],
            vec![Use {
                use_id: "U_x".to_string(),
                place,
                use_kind: "load".to_string(),
                scope_id: "S_fn".to_string(),
                function_id: Some("FN".to_string()),
                span: SourceSpan::synthetic("a.py", "return x"),
                context: "return".to_string(),
            }],
            vec!["D_x_1".to_string(), "D_x_2".to_string(), "U_x".to_string()],
        );

        let result = query_function_paths(
            &cache,
            "FN",
            Some("D_x_1"),
            Some("U_x"),
            PathQueryOptions {
                max_loop_unroll: 2,
                max_paths: 10,
                max_path_len: 10,
            },
        );

        assert!(result.paths.is_empty());
    }

    fn single_function_cache(
        definitions: Vec<Definition>,
        uses: Vec<Use>,
        statements: Vec<String>,
    ) -> AnalysisCache {
        AnalysisCache {
            schema_version: SCHEMA_VERSION,
            definitions,
            uses,
            cfgs: vec![CfgRecord {
                function_id: "FN".to_string(),
                blocks: vec![
                    CfgBlock {
                        block_id: "B_entry".to_string(),
                        block_kind: "Entry".to_string(),
                        statements: Vec::new(),
                        span: SourceSpan::synthetic("a.py", "entry"),
                    },
                    CfgBlock {
                        block_id: "B_body".to_string(),
                        block_kind: "BasicBlock".to_string(),
                        statements,
                        span: SourceSpan::synthetic("a.py", "body"),
                    },
                    CfgBlock {
                        block_id: "B_exit".to_string(),
                        block_kind: "Exit".to_string(),
                        statements: Vec::new(),
                        span: SourceSpan::synthetic("a.py", "exit"),
                    },
                ],
                edges: vec![
                    CfgEdge {
                        edge_id: "E_1".to_string(),
                        from_block_id: "B_entry".to_string(),
                        to_block_id: "B_body".to_string(),
                        edge_kind: "sequence".to_string(),
                        label: "body".to_string(),
                    },
                    CfgEdge {
                        edge_id: "E_2".to_string(),
                        from_block_id: "B_body".to_string(),
                        to_block_id: "B_exit".to_string(),
                        edge_kind: "return".to_string(),
                        label: "return".to_string(),
                    },
                ],
                entry_block_id: "B_entry".to_string(),
                exit_block_id: "B_exit".to_string(),
            }],
            ..AnalysisCache::default()
        }
    }
}
