use crate::ids::stable_id;
use crate::ir::{AnalysisCache, Place, SCHEMA_VERSION};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::Command;

pub fn dot_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct HotspotEdgeSpec {
    def_id: String,
    use_id: String,
    label: String,
    kind: HotspotEdgeKind,
}

impl HotspotEdgeSpec {
    fn local(def_id: String, use_id: String, label: String) -> Self {
        Self {
            def_id,
            use_id,
            label,
            kind: HotspotEdgeKind::Local,
        }
    }

    fn call_arg(def_id: String, use_id: String) -> Self {
        Self {
            def_id,
            use_id,
            label: "call-arg".to_string(),
            kind: HotspotEdgeKind::CallArg,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum HotspotEdgeKind {
    Local,
    CallArg,
    SameLine,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum TraversalNode {
    Def(String),
    Use(String),
}

#[derive(Clone)]
struct TraversalEdge<R> {
    to: TraversalNode,
    render_edge: R,
}

type HotspotTraversalNode = TraversalNode;
type HotspotTraversalEdge = TraversalEdge<HotspotEdgeSpec>;

#[derive(Clone)]
struct HotspotPathCandidate {
    root_def_id: String,
    node_sequence: Vec<HotspotTraversalNode>,
    render_edges: Vec<HotspotEdgeSpec>,
    path_id: String,
    path_len: usize,
    max_fan: usize,
}

#[derive(Clone, Copy)]
enum HotspotPathRankMode {
    Length,
    Fan,
}

struct RankedSelectionRound<'a, T> {
    candidates: &'a [T],
    limit: usize,
}

impl<'a, T> RankedSelectionRound<'a, T> {
    fn new(candidates: &'a [T], limit: usize) -> Self {
        Self { candidates, limit }
    }

    fn all(candidates: &'a [T]) -> Self {
        Self {
            candidates,
            limit: usize::MAX,
        }
    }
}

fn select_ranked_candidates_by_rounds<T, K, F>(
    rounds: &[RankedSelectionRound<'_, T>],
    total_limit: usize,
    key_fn: F,
) -> Vec<T>
where
    T: Clone,
    K: Ord,
    F: Fn(&T) -> K,
{
    if total_limit == 0 {
        return Vec::new();
    }

    let mut selected = Vec::new();
    let mut seen = BTreeSet::new();

    for round in rounds {
        if selected.len() >= total_limit || round.limit == 0 {
            break;
        }

        let mut added = 0usize;
        for candidate in round.candidates {
            if selected.len() >= total_limit || added >= round.limit {
                break;
            }
            if !seen.insert(key_fn(candidate)) {
                continue;
            }
            selected.push(candidate.clone());
            added += 1;
        }
    }

    selected
}

fn collect_reachable_traversal_nodes<R>(
    roots: &[TraversalNode],
    adjacency: &BTreeMap<TraversalNode, Vec<TraversalEdge<R>>>,
) -> BTreeSet<TraversalNode> {
    let mut visited = BTreeSet::new();
    let mut pending = roots.iter().rev().cloned().collect::<Vec<_>>();

    while let Some(node) = pending.pop() {
        if !visited.insert(node.clone()) {
            continue;
        }

        if let Some(edges) = adjacency.get(&node) {
            for edge in edges.iter().rev() {
                if !visited.contains(&edge.to) {
                    pending.push(edge.to.clone());
                }
            }
        }
    }

    visited
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RenderGraphDocument {
    schema_version: u32,
    graph_kind: String,
    nodes: Vec<RenderGraphNode>,
    edges: Vec<RenderGraphEdge>,
    views: Vec<RenderGraphView>,
    paths: Vec<RenderGraphPath>,
    stats: RenderGraphStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RenderGraphNode {
    id: String,
    kind: String,
    module: String,
    function: Option<String>,
    variable: String,
    line: usize,
    col: usize,
    place_kind: String,
    scope_id: Option<String>,
    function_id: Option<String>,
    label: String,
    tooltip: String,
    snippet: Option<String>,
    span: RenderSourceSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RenderGraphEdge {
    id: String,
    from: String,
    to: String,
    kind: String,
    label: String,
    color: Option<String>,
    style: Option<String>,
    dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RenderGraphView {
    id: String,
    title: String,
    node_ids: Vec<String>,
    edge_ids: Vec<String>,
    root_node_ids: Vec<String>,
    path_ids: Vec<String>,
    clusters: Vec<RenderGraphCluster>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RenderGraphCluster {
    id: String,
    label: String,
    node_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RenderGraphPath {
    id: String,
    root_node_id: String,
    node_ids: Vec<String>,
    edge_ids: Vec<String>,
    path_len: usize,
    max_fan_in: usize,
    max_fan_out: usize,
    score_kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RenderGraphStats {
    node_count: usize,
    edge_count: usize,
    def_count: usize,
    use_count: usize,
    root_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RenderSourceSpan {
    file: String,
    line: usize,
    col: usize,
    end_line: usize,
    end_col: usize,
}

struct VarDependencyGraphParts {
    nodes: BTreeMap<String, DefUseNodeRecord>,
    edges: BTreeSet<(String, String, String)>,
    root_node_ids: BTreeSet<String>,
}

struct HotspotGraphParts {
    nodes: BTreeMap<String, DefUseNodeRecord>,
    edges: BTreeSet<HotspotEdgeSpec>,
    root_node_ids: BTreeSet<String>,
    paths: Vec<HotspotPathCandidate>,
}

pub fn write_def_use_hotspots_dot(cache: &AnalysisCache, path: &Path, top_n: usize) -> Result<()> {
    let document = build_def_use_hotspot_graph_document(cache, top_n);
    write_dot(path, &render_def_use_hotspot_dot(&document))
}

pub fn write_def_use_hotspots_graph_json(
    cache: &AnalysisCache,
    path: &Path,
    top_n: usize,
) -> Result<()> {
    let document = build_def_use_hotspot_graph_document(cache, top_n);
    let json = serde_json::to_string_pretty(&document)?;
    fs::write(path, json).with_context(|| format!("failed to write {}", path.display()))
}

fn build_def_use_hotspot_graph_document(
    cache: &AnalysisCache,
    top_n: usize,
) -> RenderGraphDocument {
    let parts = collect_def_use_hotspot_graph_parts(cache, top_n);
    let labels = PlaceLabelContext::new(cache);
    let scope_owner_ids = cache
        .scopes
        .iter()
        .map(|scope| (scope.scope_id.clone(), scope.owner_id.clone()))
        .collect::<BTreeMap<_, _>>();
    let functions_by_id = cache
        .functions
        .iter()
        .map(|function| (function.function_id.clone(), function))
        .collect::<BTreeMap<_, _>>();
    let modules_by_id = cache
        .modules
        .iter()
        .map(|module| (module.module_id.clone(), module))
        .collect::<BTreeMap<_, _>>();
    let scope_module_ids = scope_module_ids(cache);
    let node_labels = build_render_graph_node_labels(&parts.nodes, &labels);

    let mut render_nodes = parts
        .nodes
        .iter()
        .map(|(node_id, record)| {
            let label = node_labels.get(node_id).cloned().unwrap_or_default();
            build_render_graph_node(
                node_id,
                record,
                &label,
                &labels,
                &scope_owner_ids,
                &functions_by_id,
                &modules_by_id,
                &scope_module_ids,
            )
        })
        .collect::<Vec<_>>();
    render_nodes.sort_by(|left, right| left.id.cmp(&right.id));

    let mut render_edges = parts
        .edges
        .iter()
        .map(build_hotspot_render_graph_edge)
        .collect::<Vec<_>>();
    render_edges.sort_by(|left, right| {
        left.from
            .cmp(&right.from)
            .then_with(|| left.to.cmp(&right.to))
            .then_with(|| left.kind.cmp(&right.kind))
    });

    let node_ids = render_nodes
        .iter()
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();
    let node_id_set = node_ids.iter().cloned().collect::<BTreeSet<_>>();
    let edge_ids = render_edges
        .iter()
        .map(|edge| edge.id.clone())
        .collect::<Vec<_>>();
    let root_node_ids = parts
        .root_node_ids
        .iter()
        .filter(|node_id| node_id_set.contains(*node_id))
        .cloned()
        .collect::<Vec<_>>();
    let clusters = build_render_graph_clusters(
        &render_nodes,
        &labels,
        &scope_owner_ids,
        &functions_by_id,
        &modules_by_id,
        &scope_module_ids,
    );
    let paths = build_hotspot_render_graph_paths(&parts.paths, &render_edges);
    let path_ids = paths.iter().map(|path| path.id.clone()).collect::<Vec<_>>();
    let stats = RenderGraphStats {
        node_count: render_nodes.len(),
        edge_count: render_edges.len(),
        def_count: render_nodes
            .iter()
            .filter(|node| node.kind == "def")
            .count(),
        use_count: render_nodes
            .iter()
            .filter(|node| node.kind == "use")
            .count(),
        root_count: root_node_ids.len(),
    };

    RenderGraphDocument {
        schema_version: SCHEMA_VERSION,
        graph_kind: "DefUseHotspots".to_string(),
        nodes: render_nodes,
        edges: render_edges,
        views: vec![RenderGraphView {
            id: "def_use_hotspots".to_string(),
            title: "Def-use hotspots".to_string(),
            node_ids,
            edge_ids,
            root_node_ids,
            path_ids,
            clusters,
        }],
        paths,
        stats,
    }
}

fn collect_def_use_hotspot_graph_parts(cache: &AnalysisCache, top_n: usize) -> HotspotGraphParts {
    let mut nodes = BTreeMap::new();
    let mut edges = BTreeSet::new();
    let definitions_by_id = cache
        .definitions
        .iter()
        .map(|definition| (definition.def_id.clone(), definition))
        .collect::<BTreeMap<_, _>>();
    let uses_by_id = cache
        .uses
        .iter()
        .map(|use_site| (use_site.use_id.clone(), use_site))
        .collect::<BTreeMap<_, _>>();
    let selected_paths = select_hotspot_seed_paths(cache, top_n);

    for selected_path in &selected_paths {
        for node in &selected_path.node_sequence {
            match node {
                HotspotTraversalNode::Def(def_id) => {
                    let Some(definition) = definitions_by_id.get(def_id) else {
                        continue;
                    };
                    nodes
                        .entry(def_id.clone())
                        .or_insert_with(|| DefUseNodeRecord::definition(definition));
                }
                HotspotTraversalNode::Use(use_id) => {
                    let Some(use_site) = uses_by_id.get(use_id) else {
                        continue;
                    };
                    nodes
                        .entry(use_id.clone())
                        .or_insert_with(|| DefUseNodeRecord::usage(use_site));
                }
            }
        }

        for edge in &selected_path.render_edges {
            edges.insert(edge.clone());
        }
    }

    for inferred_edge in infer_call_argument_edges(cache) {
        let should_include =
            nodes.contains_key(&inferred_edge.def_id) || nodes.contains_key(&inferred_edge.use_id);
        if !should_include {
            continue;
        }

        if let Some(definition) = definitions_by_id.get(&inferred_edge.def_id) {
            nodes
                .entry(inferred_edge.def_id.clone())
                .or_insert_with(|| DefUseNodeRecord::definition(definition));
        }
        if let Some(use_site) = uses_by_id.get(&inferred_edge.use_id) {
            nodes
                .entry(inferred_edge.use_id.clone())
                .or_insert_with(|| DefUseNodeRecord::usage(use_site));
        }
        edges.insert(inferred_edge);
    }

    let mut line_groups = BTreeMap::<(String, usize), (Vec<String>, Vec<String>)>::new();

    for (node_id, record) in &nodes {
        if !record.file.is_empty() && record.line > 0 {
            line_groups
                .entry((record.file.clone(), record.line))
                .or_default();
            let (defs, uses) = line_groups
                .get_mut(&(record.file.clone(), record.line))
                .expect("line group exists");
            match record.role {
                "def" => defs.push(node_id.clone()),
                "use" => uses.push(node_id.clone()),
                _ => {}
            }
        }
    }

    for ((_, _), (def_ids, use_ids)) in line_groups {
        if def_ids.is_empty() || use_ids.is_empty() {
            continue;
        }
        for def_id in &def_ids {
            for use_id in &use_ids {
                let already_connected = edges
                    .iter()
                    .any(|edge| edge.def_id == *def_id && edge.use_id == *use_id);
                if already_connected {
                    continue;
                }
                edges.insert(HotspotEdgeSpec {
                    def_id: def_id.clone(),
                    use_id: use_id.clone(),
                    label: String::new(),
                    kind: HotspotEdgeKind::SameLine,
                });
            }
        }
    }

    HotspotGraphParts {
        nodes,
        edges,
        root_node_ids: selected_paths
            .iter()
            .map(|path| path.root_def_id.clone())
            .collect(),
        paths: selected_paths,
    }
}

fn select_hotspot_seed_paths(cache: &AnalysisCache, top_n: usize) -> Vec<HotspotPathCandidate> {
    if top_n == 0 {
        return Vec::new();
    }

    let definitions_by_id = cache
        .definitions
        .iter()
        .map(|definition| (definition.def_id.clone(), definition))
        .collect::<BTreeMap<_, _>>();
    let uses_by_id = cache
        .uses
        .iter()
        .map(|use_site| (use_site.use_id.clone(), use_site))
        .collect::<BTreeMap<_, _>>();
    let same_line_edges = infer_hotspot_same_line_edges(cache);
    let mut adjacency = BTreeMap::<HotspotTraversalNode, Vec<HotspotTraversalEdge>>::new();
    let mut incoming_same_line_defs = BTreeSet::new();
    let mut outgoing_defs = BTreeSet::new();

    for edge in &cache.def_use_edges {
        let from = HotspotTraversalNode::Def(edge.def_id.clone());
        let to = HotspotTraversalNode::Use(edge.use_id.clone());
        outgoing_defs.insert(edge.def_id.clone());
        adjacency
            .entry(from)
            .or_default()
            .push(HotspotTraversalEdge {
                to,
                render_edge: HotspotEdgeSpec::local(
                    edge.def_id.clone(),
                    edge.use_id.clone(),
                    edge.edge_kind.clone(),
                ),
            });
    }

    for edge in infer_call_argument_edges(cache) {
        outgoing_defs.insert(edge.def_id.clone());
        adjacency
            .entry(HotspotTraversalNode::Def(edge.def_id.clone()))
            .or_default()
            .push(HotspotTraversalEdge {
                to: HotspotTraversalNode::Use(edge.use_id.clone()),
                render_edge: edge,
            });
    }

    for edge in same_line_edges {
        incoming_same_line_defs.insert(edge.def_id.clone());
        adjacency
            .entry(HotspotTraversalNode::Use(edge.use_id.clone()))
            .or_default()
            .push(HotspotTraversalEdge {
                to: HotspotTraversalNode::Def(edge.def_id.clone()),
                render_edge: edge,
            });
    }

    for edges in adjacency.values_mut() {
        edges.sort_by(|left, right| {
            hotspot_traversal_node_sort_key(&left.to, &definitions_by_id, &uses_by_id)
                .cmp(&hotspot_traversal_node_sort_key(
                    &right.to,
                    &definitions_by_id,
                    &uses_by_id,
                ))
                .then_with(|| left.render_edge.def_id.cmp(&right.render_edge.def_id))
                .then_with(|| left.render_edge.use_id.cmp(&right.render_edge.use_id))
                .then_with(|| left.render_edge.label.cmp(&right.render_edge.label))
        });
    }

    let fan_scores = build_hotspot_node_fan_scores(&adjacency);
    let mut root_def_ids = outgoing_defs
        .iter()
        .filter(|def_id| !incoming_same_line_defs.contains(*def_id))
        .cloned()
        .collect::<Vec<_>>();
    if root_def_ids.is_empty() {
        root_def_ids = outgoing_defs.iter().cloned().collect::<Vec<_>>();
    }
    root_def_ids.sort_by(|left, right| {
        def_sort_key(definitions_by_id.get(left))
            .cmp(&def_sort_key(definitions_by_id.get(right)))
            .then_with(|| left.cmp(right))
    });

    let mut length_paths = root_def_ids
        .iter()
        .filter_map(|root_def_id| {
            select_best_hotspot_path(
                root_def_id,
                HotspotPathRankMode::Length,
                &adjacency,
                &fan_scores,
                &definitions_by_id,
                &uses_by_id,
            )
        })
        .collect::<Vec<_>>();
    let mut fan_paths = root_def_ids
        .iter()
        .filter_map(|root_def_id| {
            select_best_hotspot_path(
                root_def_id,
                HotspotPathRankMode::Fan,
                &adjacency,
                &fan_scores,
                &definitions_by_id,
                &uses_by_id,
            )
        })
        .collect::<Vec<_>>();

    length_paths.sort_by(|left, right| {
        compare_hotspot_paths(left, right, HotspotPathRankMode::Length, &definitions_by_id)
    });
    fan_paths.sort_by(|left, right| {
        compare_hotspot_paths(left, right, HotspotPathRankMode::Fan, &definitions_by_id)
    });

    let length_quota = top_n.div_ceil(2);
    let fan_quota = top_n - length_quota;

    select_ranked_candidates_by_rounds(
        &[
            RankedSelectionRound::new(&length_paths, length_quota),
            RankedSelectionRound::new(&fan_paths, fan_quota),
            RankedSelectionRound::all(&length_paths),
            RankedSelectionRound::all(&fan_paths),
        ],
        top_n,
        |candidate| candidate.path_id.clone(),
    )
}

fn infer_hotspot_same_line_edges(cache: &AnalysisCache) -> Vec<HotspotEdgeSpec> {
    let mut defs_by_line = BTreeMap::<(String, usize), Vec<String>>::new();
    for definition in &cache.definitions {
        if definition.span.line == 0 || definition.span.file.is_empty() {
            continue;
        }
        defs_by_line
            .entry((definition.span.file.clone(), definition.span.line))
            .or_default()
            .push(definition.def_id.clone());
    }

    let existing_pairs = cache
        .def_use_edges
        .iter()
        .map(|edge| (edge.def_id.clone(), edge.use_id.clone()))
        .collect::<BTreeSet<_>>();

    let mut same_line_edges = Vec::new();
    for use_site in &cache.uses {
        if use_site.span.line == 0 || use_site.span.file.is_empty() {
            continue;
        }
        let Some(def_ids) = defs_by_line.get(&(use_site.span.file.clone(), use_site.span.line))
        else {
            continue;
        };
        for def_id in def_ids {
            if existing_pairs.contains(&(def_id.clone(), use_site.use_id.clone())) {
                continue;
            }
            same_line_edges.push(HotspotEdgeSpec {
                def_id: def_id.clone(),
                use_id: use_site.use_id.clone(),
                label: String::new(),
                kind: HotspotEdgeKind::SameLine,
            });
        }
    }

    same_line_edges
}

fn build_hotspot_node_fan_scores(
    adjacency: &BTreeMap<HotspotTraversalNode, Vec<HotspotTraversalEdge>>,
) -> BTreeMap<HotspotTraversalNode, usize> {
    let mut incoming = BTreeMap::<HotspotTraversalNode, usize>::new();
    let mut outgoing = BTreeMap::<HotspotTraversalNode, usize>::new();

    for (node, edges) in adjacency {
        outgoing.insert(node.clone(), edges.len());
        for edge in edges {
            *incoming.entry(edge.to.clone()).or_insert(0) += 1;
        }
    }

    adjacency
        .keys()
        .chain(incoming.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|node| {
            let fan = incoming
                .get(&node)
                .copied()
                .unwrap_or_default()
                .max(outgoing.get(&node).copied().unwrap_or_default());
            (node, fan)
        })
        .collect()
}

fn hotspot_traversal_node_sort_key(
    node: &HotspotTraversalNode,
    definitions_by_id: &BTreeMap<String, &crate::ir::Definition>,
    uses_by_id: &BTreeMap<String, &crate::ir::Use>,
) -> (u8, String, usize, usize, String) {
    match node {
        HotspotTraversalNode::Def(def_id) => definitions_by_id
            .get(def_id)
            .map(|definition| {
                (
                    0,
                    definition.span.file.clone(),
                    definition.span.line,
                    definition.span.col,
                    def_id.clone(),
                )
            })
            .unwrap_or_else(|| (0, String::new(), 0, 0, def_id.clone())),
        HotspotTraversalNode::Use(use_id) => uses_by_id
            .get(use_id)
            .map(|use_site| {
                (
                    1,
                    use_site.span.file.clone(),
                    use_site.span.line,
                    use_site.span.col,
                    use_id.clone(),
                )
            })
            .unwrap_or_else(|| (1, String::new(), 0, 0, use_id.clone())),
    }
}

fn select_best_hotspot_path(
    root_def_id: &str,
    mode: HotspotPathRankMode,
    adjacency: &BTreeMap<HotspotTraversalNode, Vec<HotspotTraversalEdge>>,
    fan_scores: &BTreeMap<HotspotTraversalNode, usize>,
    definitions_by_id: &BTreeMap<String, &crate::ir::Definition>,
    uses_by_id: &BTreeMap<String, &crate::ir::Use>,
) -> Option<HotspotPathCandidate> {
    let root = HotspotTraversalNode::Def(root_def_id.to_string());
    if !adjacency.contains_key(&root) {
        return None;
    }

    let mut visited = BTreeSet::new();
    let mut node_sequence = Vec::new();
    let mut render_edges = Vec::new();
    let mut best = None;

    visit_hotspot_paths(
        &root,
        root_def_id,
        mode,
        adjacency,
        fan_scores,
        definitions_by_id,
        uses_by_id,
        &mut visited,
        &mut node_sequence,
        &mut render_edges,
        &mut best,
    );

    best
}

#[allow(clippy::too_many_arguments)]
fn visit_hotspot_paths(
    current: &HotspotTraversalNode,
    root_def_id: &str,
    mode: HotspotPathRankMode,
    adjacency: &BTreeMap<HotspotTraversalNode, Vec<HotspotTraversalEdge>>,
    fan_scores: &BTreeMap<HotspotTraversalNode, usize>,
    definitions_by_id: &BTreeMap<String, &crate::ir::Definition>,
    uses_by_id: &BTreeMap<String, &crate::ir::Use>,
    visited: &mut BTreeSet<HotspotTraversalNode>,
    node_sequence: &mut Vec<HotspotTraversalNode>,
    render_edges: &mut Vec<HotspotEdgeSpec>,
    best: &mut Option<HotspotPathCandidate>,
) {
    visited.insert(current.clone());
    node_sequence.push(current.clone());

    let mut explored_child = false;
    if let Some(edges) = adjacency.get(current) {
        for edge in edges {
            if visited.contains(&edge.to) {
                continue;
            }
            explored_child = true;
            render_edges.push(edge.render_edge.clone());
            visit_hotspot_paths(
                &edge.to,
                root_def_id,
                mode,
                adjacency,
                fan_scores,
                definitions_by_id,
                uses_by_id,
                visited,
                node_sequence,
                render_edges,
                best,
            );
            render_edges.pop();
        }
    }

    if !explored_child {
        let candidate =
            build_hotspot_path_candidate(root_def_id, node_sequence, render_edges, fan_scores);
        let should_replace = best
            .as_ref()
            .map(|current_best| {
                compare_hotspot_paths(&candidate, current_best, mode, definitions_by_id).is_lt()
            })
            .unwrap_or(true);
        if should_replace {
            *best = Some(candidate);
        }
    }

    node_sequence.pop();
    visited.remove(current);
}

fn build_hotspot_path_candidate(
    root_def_id: &str,
    node_sequence: &[HotspotTraversalNode],
    render_edges: &[HotspotEdgeSpec],
    fan_scores: &BTreeMap<HotspotTraversalNode, usize>,
) -> HotspotPathCandidate {
    let path_id = node_sequence
        .iter()
        .map(|node| match node {
            HotspotTraversalNode::Def(def_id) => format!("D:{def_id}"),
            HotspotTraversalNode::Use(use_id) => format!("U:{use_id}"),
        })
        .collect::<Vec<_>>()
        .join("->");
    let max_fan = node_sequence
        .iter()
        .map(|node| fan_scores.get(node).copied().unwrap_or_default())
        .max()
        .unwrap_or_default();

    HotspotPathCandidate {
        root_def_id: root_def_id.to_string(),
        node_sequence: node_sequence.to_vec(),
        render_edges: render_edges.to_vec(),
        path_id,
        path_len: node_sequence.len(),
        max_fan,
    }
}

fn compare_hotspot_paths(
    left: &HotspotPathCandidate,
    right: &HotspotPathCandidate,
    mode: HotspotPathRankMode,
    definitions_by_id: &BTreeMap<String, &crate::ir::Definition>,
) -> std::cmp::Ordering {
    match mode {
        HotspotPathRankMode::Length => right
            .path_len
            .cmp(&left.path_len)
            .then_with(|| right.max_fan.cmp(&left.max_fan))
            .then_with(|| {
                def_sort_key(definitions_by_id.get(&left.root_def_id))
                    .cmp(&def_sort_key(definitions_by_id.get(&right.root_def_id)))
            })
            .then_with(|| left.path_id.cmp(&right.path_id)),
        HotspotPathRankMode::Fan => right
            .max_fan
            .cmp(&left.max_fan)
            .then_with(|| right.path_len.cmp(&left.path_len))
            .then_with(|| {
                def_sort_key(definitions_by_id.get(&left.root_def_id))
                    .cmp(&def_sort_key(definitions_by_id.get(&right.root_def_id)))
            })
            .then_with(|| left.path_id.cmp(&right.path_id)),
    }
}

pub fn write_module_dependency_dot(cache: &AnalysisCache, path: &Path) -> Result<()> {
    let mut text = String::from(
        "digraph ModuleDependencies {\n  rankdir=LR;\n  node [shape=box, fontsize=10];\n",
    );

    for module in &cache.modules {
        text.push_str(&format!(
            "  \"{}\" [label=\"{}\"];\n",
            dot_label(&module.module_id),
            dot_label(&module.module_name)
        ));
    }

    for module in &cache.modules {
        for import in &module.imports {
            let target_id = format!("import:{}", import.module);
            text.push_str(&format!(
                "  \"{}\" -> \"{}\" [label=\"{}\"];\n",
                dot_label(&module.module_id),
                dot_label(&target_id),
                dot_label(import.name.as_deref().unwrap_or(&import.resolution))
            ));
            text.push_str(&format!(
                "  \"{}\" [label=\"{}\"];\n",
                dot_label(&target_id),
                dot_label(&import.module)
            ));
        }
    }

    text.push_str("}\n");
    write_dot(path, &text)
}

pub fn write_function_dependency_dot(cache: &AnalysisCache, path: &Path) -> Result<()> {
    let mut text = String::from(
        "digraph FunctionDependencies {\n  rankdir=LR;\n  node [shape=box, fontsize=10];\n",
    );

    for function in &cache.functions {
        text.push_str(&format!(
            "  \"{}\" [label=\"{}\"];\n",
            dot_label(&function.function_id),
            dot_label(&function.qualified_name)
        ));
    }

    for call in &cache.calls {
        let Some(caller_id) = &call.function_id else {
            continue;
        };

        if call.candidate_function_ids.is_empty() {
            let external_id = format!("external:{}", call.call_id);
            text.push_str(&format!(
                "  \"{}\" [label=\"{}\"];\n",
                dot_label(&external_id),
                dot_label(&call.callee_expr)
            ));
            text.push_str(&format!(
                "  \"{}\" -> \"{}\" [label=\"{}\"];\n",
                dot_label(caller_id),
                dot_label(&external_id),
                dot_label(&call.resolution)
            ));
            continue;
        }

        for callee_id in &call.candidate_function_ids {
            text.push_str(&format!(
                "  \"{}\" -> \"{}\" [label=\"{}\"];\n",
                dot_label(caller_id),
                dot_label(callee_id),
                dot_label(&call.resolution)
            ));
        }
    }

    text.push_str("}\n");
    write_dot(path, &text)
}

fn def_sort_key(definition: Option<&&crate::ir::Definition>) -> (String, usize, usize, String) {
    definition
        .map(|definition| {
            (
                definition.span.file.clone(),
                definition.span.line,
                definition.span.col,
                definition.def_id.clone(),
            )
        })
        .unwrap_or_else(|| (String::new(), 0, 0, String::new()))
}

fn infer_call_argument_edges(cache: &AnalysisCache) -> Vec<HotspotEdgeSpec> {
    cache
        .var_dependency_edges
        .iter()
        .filter(|edge| edge.dep_kind == "call-arg")
        .map(|edge| HotspotEdgeSpec::call_arg(edge.target_id.clone(), edge.source_id.clone()))
        .collect()
}

struct PlaceLabelContext {
    scope_labels: BTreeMap<String, String>,
    module_labels: BTreeMap<String, String>,
}

impl PlaceLabelContext {
    fn new(cache: &AnalysisCache) -> Self {
        let module_labels = cache
            .modules
            .iter()
            .map(|module| (module.module_id.clone(), module.module_name.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut owner_labels = module_labels.clone();

        owner_labels.extend(cache.classes.iter().map(|class| {
            (
                class.class_id.clone(),
                qualify_owner_label(
                    module_labels.get(&class.module_id).map(String::as_str),
                    &class.qualified_name,
                ),
            )
        }));
        owner_labels.extend(cache.functions.iter().map(|function| {
            (
                function.function_id.clone(),
                qualify_owner_label(
                    module_labels.get(&function.module_id).map(String::as_str),
                    &function.qualified_name,
                ),
            )
        }));

        let scope_labels = cache
            .scopes
            .iter()
            .map(|scope| {
                let label = owner_labels
                    .get(&scope.owner_id)
                    .cloned()
                    .unwrap_or_else(|| scope.scope_id.clone());
                (scope.scope_id.clone(), label)
            })
            .collect();

        Self {
            scope_labels,
            module_labels,
        }
    }

    fn short_label(&self, place: &Place) -> String {
        match place {
            Place::Local { name, .. }
            | Place::Global { name, .. }
            | Place::Closure { name, .. } => name.clone(),
            Place::Attribute { base, attr } => format!("{base}.{attr}"),
            Place::Subscript { base, index } => format!("{base}[{index}]"),
            Place::External { name } => format!("external:{name}"),
            Place::Unknown { reason } => format!("unknown:{reason}"),
        }
    }

    fn disambiguated_label(&self, place: &Place) -> String {
        match place {
            Place::Local { scope_id, name } => self
                .scope_label(scope_id)
                .map(|scope| format!("{scope}::{name}"))
                .unwrap_or_else(|| name.clone()),
            Place::Global { module_id, name } => self
                .module_label(module_id)
                .map(|module| format!("{module}::{name}"))
                .unwrap_or_else(|| name.clone()),
            Place::Closure { scope_id, name } => self
                .scope_label(scope_id)
                .map(|scope| format!("{scope}::{name} [closure]"))
                .unwrap_or_else(|| format!("{name} [closure]")),
            Place::Attribute { .. }
            | Place::Subscript { .. }
            | Place::External { .. }
            | Place::Unknown { .. } => self.short_label(place),
        }
    }

    fn scoped_disambiguated_label(&self, scope_id: Option<&str>, place: &Place) -> String {
        match place {
            Place::Attribute { .. } | Place::Subscript { .. } => scope_id
                .and_then(|scope_id| self.scope_label(scope_id))
                .map(|scope| format!("{scope}::{}", self.short_label(place)))
                .unwrap_or_else(|| self.short_label(place)),
            _ => self.disambiguated_label(place),
        }
    }

    fn scope_label(&self, scope_id: &str) -> Option<String> {
        self.scope_labels.get(scope_id).cloned()
    }

    fn module_label(&self, module_id: &str) -> Option<String> {
        self.module_labels.get(module_id).cloned()
    }
}

fn qualify_owner_label(module_label: Option<&str>, qualified_name: &str) -> String {
    let Some(module_label) = module_label else {
        return qualified_name.to_string();
    };
    if qualified_name.is_empty() || qualified_name == module_label {
        return module_label.to_string();
    }
    if let Some(suffix) = qualified_name.strip_prefix(&format!("{module_label}.")) {
        return format!("{module_label}::{suffix}");
    }
    if let Some(suffix) = qualified_name.strip_prefix(&format!("{module_label}::")) {
        return format!("{module_label}::{suffix}");
    }
    format!("{module_label}::{qualified_name}")
}

#[derive(Clone)]
struct DefUseNodeRecord {
    role: &'static str,
    place: Place,
    scope_id: Option<String>,
    file: String,
    line: usize,
    col: usize,
    snippet: String,
}

impl DefUseNodeRecord {
    fn definition(definition: &crate::ir::Definition) -> Self {
        Self {
            role: "def",
            place: definition.place.clone(),
            scope_id: Some(definition.scope_id.clone()),
            file: definition.span.file.clone(),
            line: definition.span.line,
            col: definition.span.col,
            snippet: definition.span.snippet.clone(),
        }
    }

    fn usage(use_site: &crate::ir::Use) -> Self {
        Self {
            role: "use",
            place: use_site.place.clone(),
            scope_id: Some(use_site.scope_id.clone()),
            file: use_site.span.file.clone(),
            line: use_site.span.line,
            col: use_site.span.col,
            snippet: use_site.span.snippet.clone(),
        }
    }

    fn fallback(role: &'static str, place: Place) -> Self {
        Self {
            role,
            place,
            scope_id: None,
            file: String::new(),
            line: 0,
            col: 0,
            snippet: String::new(),
        }
    }

    fn fallback_with_span(
        role: &'static str,
        place: &Place,
        span: &crate::source::SourceSpan,
    ) -> Self {
        Self {
            role,
            place: place.clone(),
            scope_id: scope_id_for_place(place),
            file: span.file.clone(),
            line: span.line,
            col: span.col,
            snippet: span.snippet.clone(),
        }
    }

    fn base_label(&self, labels: &PlaceLabelContext) -> String {
        format!("{} {}", self.role, self.place_label(labels))
    }

    fn short_label(&self, labels: &PlaceLabelContext) -> String {
        format!(
            "{} {} {}",
            self.role,
            self.place_label(labels),
            self.line_suffix()
        )
    }

    fn disambiguated_label(&self, labels: &PlaceLabelContext) -> String {
        self.short_label(labels)
    }

    fn fully_disambiguated_label(&self, labels: &PlaceLabelContext) -> String {
        let label = self.disambiguated_label(labels);
        if self.line > 0 && self.col > 0 {
            format!("{label}:{}", self.col)
        } else {
            label
        }
    }

    fn line_suffix(&self) -> String {
        if self.line > 0 {
            format!("@ line {}", self.line)
        } else {
            "@ line ?".to_string()
        }
    }

    fn place_label(&self, labels: &PlaceLabelContext) -> String {
        self.snippet_derived_place_label(labels).unwrap_or_else(|| {
            labels.scoped_disambiguated_label(self.scope_id.as_deref(), &self.place)
        })
    }

    fn snippet_derived_place_label(&self, labels: &PlaceLabelContext) -> Option<String> {
        match self.place {
            Place::Attribute { .. } | Place::Subscript { .. } => {
                let place_text = if self.role == "def" {
                    extract_assignment_target_text(&self.snippet)?
                } else {
                    let snippet = self.snippet.trim();
                    if snippet.is_empty() {
                        return None;
                    }
                    snippet.to_string()
                };
                let scope_label = self
                    .scope_id
                    .as_deref()
                    .and_then(|scope_id| labels.scope_label(scope_id));
                Some(match scope_label {
                    Some(scope) => format!("{scope}::{place_text}"),
                    None => place_text,
                })
            }
            _ => None,
        }
    }
}

fn scope_id_for_place(place: &Place) -> Option<String> {
    match place {
        Place::Local { scope_id, .. } | Place::Closure { scope_id, .. } => Some(scope_id.clone()),
        _ => None,
    }
}

fn extract_assignment_target_text(snippet: &str) -> Option<String> {
    let snippet = snippet.trim();
    if snippet.is_empty() {
        return None;
    }

    let chars = snippet.char_indices().collect::<Vec<_>>();
    for (offset, (index, ch)) in chars.iter().enumerate() {
        if *ch != '=' {
            continue;
        }

        let prev = if offset > 0 {
            Some(chars[offset - 1].1)
        } else {
            None
        };
        let next = chars.get(offset + 1).map(|(_, next_ch)| *next_ch);
        if matches!(
            prev,
            Some('=') | Some('!') | Some('<') | Some('>') | Some(':')
        ) || matches!(next, Some('='))
        {
            continue;
        }

        let target = snippet[..*index].trim();
        if !target.is_empty() {
            return Some(target.to_string());
        }
    }

    None
}

fn canonical_use_record(
    use_site: &crate::ir::Use,
    exact_defined_scope_places: &BTreeSet<(String, Place)>,
    local_defs_by_scope_name: &BTreeMap<String, BTreeSet<String>>,
    global_defs_by_module_name: &BTreeMap<String, BTreeSet<String>>,
    scope_module_ids: &BTreeMap<String, String>,
) -> (String, DefUseNodeRecord) {
    if use_site.use_kind == "call-zero-arg"
        || exact_defined_scope_places.contains(&(use_site.scope_id.clone(), use_site.place.clone()))
    {
        return (use_site.use_id.clone(), DefUseNodeRecord::usage(use_site));
    }

    let ancestor_place = match &use_site.place {
        Place::Attribute { base, .. } | Place::Subscript { base, .. } if base != "*" => {
            if local_defs_by_scope_name
                .get(&use_site.scope_id)
                .map(|names| names.contains(base))
                .unwrap_or(false)
            {
                Some(Place::Local {
                    scope_id: use_site.scope_id.clone(),
                    name: base.clone(),
                })
            } else {
                scope_module_ids
                    .get(&use_site.scope_id)
                    .and_then(|module_id| {
                        global_defs_by_module_name
                            .get(module_id)
                            .filter(|names| names.contains(base))
                            .map(|_| Place::Global {
                                module_id: module_id.clone(),
                                name: base.clone(),
                            })
                    })
            }
        }
        _ => None,
    };

    let Some(place) = ancestor_place else {
        return (use_site.use_id.clone(), DefUseNodeRecord::usage(use_site));
    };

    let scope_id = match &place {
        Place::Local { scope_id, .. } | Place::Closure { scope_id, .. } => Some(scope_id.clone()),
        _ => None,
    };
    let node_id = format!(
        "UCAN:{}:{}:{}:{:?}",
        use_site.scope_id, use_site.span.file, use_site.span.line, place
    );

    (
        node_id,
        DefUseNodeRecord {
            role: "use",
            place,
            scope_id,
            file: use_site.span.file.clone(),
            line: use_site.span.line,
            col: 0,
            snippet: String::new(),
        },
    )
}

fn definition_precedes_use(definition: &crate::ir::Definition, use_site: &crate::ir::Use) -> bool {
    definition.span.end_line < use_site.span.line
        || (definition.span.end_line == use_site.span.line
            && definition.span.end_col <= use_site.span.col)
}

fn latest_definition_ids_for_place(
    place: &Place,
    use_site: &crate::ir::Use,
    definitions: &[crate::ir::Definition],
) -> Vec<String> {
    let mut latest_position = None;
    let mut def_ids = Vec::new();

    for definition in definitions {
        if &definition.place != place || !definition_precedes_use(definition, use_site) {
            continue;
        }

        let position = (definition.span.end_line, definition.span.end_col);
        match latest_position {
            None => {
                latest_position = Some(position);
                def_ids.clear();
                def_ids.push(definition.def_id.clone());
            }
            Some(current) if position > current => {
                latest_position = Some(position);
                def_ids.clear();
                def_ids.push(definition.def_id.clone());
            }
            Some(current) if position == current => {
                def_ids.push(definition.def_id.clone());
            }
            Some(_) => {}
        }
    }

    def_ids
}

fn is_assignment_callee_edge(
    edge: &crate::ir::VarDependencyEdge,
    definitions_by_id: &BTreeMap<String, &crate::ir::Definition>,
    uses_by_id: &BTreeMap<String, &crate::ir::Use>,
) -> bool {
    if edge.dep_kind != "assignment" {
        return false;
    }

    let Some(definition) = definitions_by_id.get(&edge.target_id) else {
        return false;
    };
    let Some(use_site) = uses_by_id.get(&edge.source_id) else {
        return false;
    };

    let expr = definition.expr.trim();
    let snippet = use_site.span.snippet.trim();
    if expr.is_empty() || snippet.is_empty() {
        return false;
    }

    expr.starts_with(snippet) && expr[snippet.len()..].trim_start().starts_with('(')
}

pub fn write_var_dependency_dot(cache: &AnalysisCache, path: &Path, top_n: usize) -> Result<()> {
    let document = build_var_dependency_graph_document(cache, top_n);
    write_dot(path, &render_var_dependency_dot(&document))
}

pub fn write_var_dependency_graph_json(
    cache: &AnalysisCache,
    path: &Path,
    top_n: usize,
) -> Result<()> {
    let document = build_var_dependency_graph_document(cache, top_n);
    let json = serde_json::to_string_pretty(&document)?;
    fs::write(path, json).with_context(|| format!("failed to write {}", path.display()))
}

fn build_var_dependency_graph_document(
    cache: &AnalysisCache,
    _top_n: usize,
) -> RenderGraphDocument {
    let parts = collect_var_dependency_graph_parts(cache);
    let labels = PlaceLabelContext::new(cache);
    let scope_owner_ids = cache
        .scopes
        .iter()
        .map(|scope| (scope.scope_id.clone(), scope.owner_id.clone()))
        .collect::<BTreeMap<_, _>>();
    let functions_by_id = cache
        .functions
        .iter()
        .map(|function| (function.function_id.clone(), function))
        .collect::<BTreeMap<_, _>>();
    let modules_by_id = cache
        .modules
        .iter()
        .map(|module| (module.module_id.clone(), module))
        .collect::<BTreeMap<_, _>>();
    let class_module_ids = cache
        .classes
        .iter()
        .map(|class_record| {
            (
                class_record.class_id.clone(),
                class_record.module_id.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let function_module_ids = cache
        .functions
        .iter()
        .map(|function| (function.function_id.clone(), function.module_id.clone()))
        .collect::<BTreeMap<_, _>>();
    let module_ids = cache
        .modules
        .iter()
        .map(|module_record| module_record.module_id.clone())
        .collect::<BTreeSet<_>>();
    let scope_module_ids =
        cache
            .scopes
            .iter()
            .fold(BTreeMap::<String, String>::new(), |mut map, scope| {
                if module_ids.contains(&scope.owner_id) {
                    map.insert(scope.scope_id.clone(), scope.owner_id.clone());
                } else if let Some(module_id) = function_module_ids.get(&scope.owner_id) {
                    map.insert(scope.scope_id.clone(), module_id.clone());
                } else if let Some(module_id) = class_module_ids.get(&scope.owner_id) {
                    map.insert(scope.scope_id.clone(), module_id.clone());
                }
                map
            });

    let mut short_label_counts = BTreeMap::new();
    for record in parts.nodes.values() {
        *short_label_counts
            .entry(record.base_label(&labels))
            .or_insert(0usize) += 1;
    }

    let mut node_labels = BTreeMap::new();
    for (node_id, record) in &parts.nodes {
        let base_label = record.base_label(&labels);
        let short_label = record.short_label(&labels);
        let label = if short_label_counts
            .get(&base_label)
            .copied()
            .unwrap_or_default()
            > 1
        {
            record.disambiguated_label(&labels)
        } else {
            short_label
        };
        node_labels.insert(node_id.clone(), label);
    }

    let mut final_label_counts = BTreeMap::new();
    for label in node_labels.values() {
        *final_label_counts.entry(label.clone()).or_insert(0usize) += 1;
    }

    let mut render_nodes = parts
        .nodes
        .iter()
        .map(|(node_id, record)| {
            let mut label = node_labels.get(node_id).cloned().unwrap_or_default();
            if final_label_counts.get(&label).copied().unwrap_or_default() > 1 {
                label = record.fully_disambiguated_label(&labels);
            }
            build_render_graph_node(
                node_id,
                record,
                &label,
                &labels,
                &scope_owner_ids,
                &functions_by_id,
                &modules_by_id,
                &scope_module_ids,
            )
        })
        .collect::<Vec<_>>();
    render_nodes.sort_by(|left, right| left.id.cmp(&right.id));

    let mut render_edges = parts
        .edges
        .iter()
        .map(|(from, to, kind)| build_render_graph_edge(from, to, kind))
        .collect::<Vec<_>>();
    render_edges.sort_by(|left, right| {
        left.from
            .cmp(&right.from)
            .then_with(|| left.to.cmp(&right.to))
            .then_with(|| left.kind.cmp(&right.kind))
    });

    let node_ids = render_nodes
        .iter()
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();
    let node_id_set = node_ids.iter().cloned().collect::<BTreeSet<_>>();
    let edge_ids = render_edges
        .iter()
        .map(|edge| edge.id.clone())
        .collect::<Vec<_>>();
    let root_node_ids = parts
        .root_node_ids
        .iter()
        .filter(|node_id| node_id_set.contains(*node_id))
        .cloned()
        .collect::<Vec<_>>();
    let clusters = build_render_graph_clusters(
        &render_nodes,
        &labels,
        &scope_owner_ids,
        &functions_by_id,
        &modules_by_id,
        &scope_module_ids,
    );
    let paths = build_var_dependency_render_graph_paths(&root_node_ids, &render_edges);
    let path_ids = paths.iter().map(|path| path.id.clone()).collect::<Vec<_>>();
    let stats = RenderGraphStats {
        node_count: render_nodes.len(),
        edge_count: render_edges.len(),
        def_count: render_nodes
            .iter()
            .filter(|node| node.kind == "def")
            .count(),
        use_count: render_nodes
            .iter()
            .filter(|node| node.kind == "use")
            .count(),
        root_count: root_node_ids.len(),
    };

    RenderGraphDocument {
        schema_version: SCHEMA_VERSION,
        graph_kind: "VariableDependencies".to_string(),
        nodes: render_nodes,
        edges: render_edges,
        views: vec![RenderGraphView {
            id: "variable_dependencies".to_string(),
            title: "Variable dependencies".to_string(),
            node_ids,
            edge_ids,
            root_node_ids,
            path_ids,
            clusters,
        }],
        paths,
        stats,
    }
}

fn collect_var_dependency_graph_parts(cache: &AnalysisCache) -> VarDependencyGraphParts {
    let mut nodes = BTreeMap::new();
    let mut edges = BTreeSet::new();
    let definitions_by_id = cache
        .definitions
        .iter()
        .map(|definition| (definition.def_id.clone(), definition))
        .collect::<BTreeMap<_, _>>();
    let uses_by_id = cache
        .uses
        .iter()
        .map(|use_site| (use_site.use_id.clone(), use_site))
        .collect::<BTreeMap<_, _>>();
    let exact_defined_scope_places = cache
        .definitions
        .iter()
        .map(|definition| (definition.scope_id.clone(), definition.place.clone()))
        .collect::<BTreeSet<_>>();
    let local_defs_by_scope_name = cache.definitions.iter().fold(
        BTreeMap::<String, BTreeSet<String>>::new(),
        |mut map, definition| {
            if let Place::Local { scope_id, name } = &definition.place {
                map.entry(scope_id.clone())
                    .or_default()
                    .insert(name.clone());
            }
            map
        },
    );
    let global_defs_by_module_name = cache.definitions.iter().fold(
        BTreeMap::<String, BTreeSet<String>>::new(),
        |mut map, definition| {
            if let Place::Global { module_id, name } = &definition.place {
                map.entry(module_id.clone())
                    .or_default()
                    .insert(name.clone());
            }
            map
        },
    );
    let function_module_ids = cache
        .functions
        .iter()
        .map(|function| (function.function_id.clone(), function.module_id.clone()))
        .collect::<BTreeMap<_, _>>();
    let class_module_ids = cache
        .classes
        .iter()
        .map(|class_record| {
            (
                class_record.class_id.clone(),
                class_record.module_id.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let module_ids = cache
        .modules
        .iter()
        .map(|module_record| module_record.module_id.clone())
        .collect::<BTreeSet<_>>();
    let scope_module_ids =
        cache
            .scopes
            .iter()
            .fold(BTreeMap::<String, String>::new(), |mut map, scope| {
                if module_ids.contains(&scope.owner_id) {
                    map.insert(scope.scope_id.clone(), scope.owner_id.clone());
                } else if let Some(module_id) = function_module_ids.get(&scope.owner_id) {
                    map.insert(scope.scope_id.clone(), module_id.clone());
                } else if let Some(module_id) = class_module_ids.get(&scope.owner_id) {
                    map.insert(scope.scope_id.clone(), module_id.clone());
                }
                map
            });
    let canonical_use_records = cache
        .uses
        .iter()
        .map(|use_site| {
            (
                use_site.use_id.clone(),
                canonical_use_record(
                    use_site,
                    &exact_defined_scope_places,
                    &local_defs_by_scope_name,
                    &global_defs_by_module_name,
                    &scope_module_ids,
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let incoming_var_edge_count = cache
        .var_dependency_edges
        .iter()
        .filter(|edge| {
            edge.dep_kind != "call-arg"
                && !is_assignment_callee_edge(edge, &definitions_by_id, &uses_by_id)
                && canonical_use_records
                    .get(&edge.source_id)
                    .map(|(node_id, _)| node_id == &edge.source_id)
                    .unwrap_or(true)
        })
        .fold(BTreeMap::<String, usize>::new(), |mut map, edge| {
            *map.entry(edge.target_id.clone()).or_insert(0) += 1;
            map
        });
    let mut root_target_ids = cache
        .definitions
        .iter()
        .filter(|definition| !incoming_var_edge_count.contains_key(&definition.def_id))
        .map(|definition| definition.def_id.clone())
        .collect::<BTreeSet<_>>();
    if root_target_ids.is_empty() {
        root_target_ids.extend(
            cache
                .definitions
                .iter()
                .map(|definition| definition.def_id.clone()),
        );
    }
    let var_edges_by_target_id = cache.var_dependency_edges.iter().fold(
        BTreeMap::<String, Vec<&crate::ir::VarDependencyEdge>>::new(),
        |mut map, edge| {
            map.entry(edge.target_id.clone()).or_default().push(edge);
            map
        },
    );
    let call_arg_target_ids = cache
        .var_dependency_edges
        .iter()
        .filter(|edge| edge.dep_kind == "call-arg")
        .fold(
            BTreeMap::<String, BTreeSet<String>>::new(),
            |mut map, edge| {
                map.entry(edge.source_id.clone())
                    .or_default()
                    .insert(edge.target_id.clone());
                map
            },
        );
    let def_use_edges_by_def_id = cache.def_use_edges.iter().fold(
        BTreeMap::<String, Vec<&crate::ir::DefUseEdge>>::new(),
        |mut map, edge| {
            map.entry(edge.def_id.clone()).or_default().push(edge);
            map
        },
    );
    let def_use_edges_by_use_id = cache.def_use_edges.iter().fold(
        BTreeMap::<String, Vec<&crate::ir::DefUseEdge>>::new(),
        |mut map, edge| {
            map.entry(edge.use_id.clone()).or_default().push(edge);
            map
        },
    );
    let fallback_def_ids_by_use_id = cache
        .uses
        .iter()
        .filter_map(|use_site| {
            let Some((canonical_node_id, canonical_record)) =
                canonical_use_records.get(&use_site.use_id)
            else {
                return None;
            };
            if canonical_node_id == &use_site.use_id
                || def_use_edges_by_use_id.contains_key(&use_site.use_id)
            {
                return None;
            }

            let def_ids = latest_definition_ids_for_place(
                &canonical_record.place,
                use_site,
                &cache.definitions,
            );
            if def_ids.is_empty() {
                None
            } else {
                Some((use_site.use_id.clone(), def_ids))
            }
        })
        .collect::<BTreeMap<_, _>>();
    let mut same_line_def_ids = BTreeMap::<(String, String, usize), BTreeSet<String>>::new();
    for definition in &cache.definitions {
        if definition.span.line == 0 {
            continue;
        }
        same_line_def_ids
            .entry((
                definition.scope_id.clone(),
                definition.span.file.clone(),
                definition.span.line,
            ))
            .or_default()
            .insert(definition.def_id.clone());
    }
    let concrete_use_to_def_edges = cache
        .var_dependency_edges
        .iter()
        .map(|edge| (edge.source_id.clone(), edge.target_id.clone()))
        .collect::<BTreeSet<_>>();
    let mut adjacency = BTreeMap::<TraversalNode, Vec<TraversalEdge<()>>>::new();
    let mut same_line_edges = BTreeSet::new();

    for (target_id, def_use_edges) in &def_use_edges_by_def_id {
        for edge in def_use_edges {
            adjacency
                .entry(TraversalNode::Def(target_id.clone()))
                .or_default()
                .push(TraversalEdge {
                    to: TraversalNode::Use(edge.use_id.clone()),
                    render_edge: (),
                });
        }
    }

    for (target_id, var_edges) in &var_edges_by_target_id {
        for edge in var_edges {
            adjacency
                .entry(TraversalNode::Def(target_id.clone()))
                .or_default()
                .push(TraversalEdge {
                    to: TraversalNode::Use(edge.source_id.clone()),
                    render_edge: (),
                });
        }
    }

    for (use_id, target_ids) in &call_arg_target_ids {
        for target_id in target_ids {
            adjacency
                .entry(TraversalNode::Use(use_id.clone()))
                .or_default()
                .push(TraversalEdge {
                    to: TraversalNode::Def(target_id.clone()),
                    render_edge: (),
                });
        }
    }

    for (use_id, def_ids) in &fallback_def_ids_by_use_id {
        for def_id in def_ids {
            adjacency
                .entry(TraversalNode::Use(use_id.clone()))
                .or_default()
                .push(TraversalEdge {
                    to: TraversalNode::Def(def_id.clone()),
                    render_edge: (),
                });
        }
    }

    for use_site in &cache.uses {
        let key = (
            use_site.scope_id.clone(),
            use_site.span.file.clone(),
            use_site.span.line,
        );
        let Some(def_ids) = same_line_def_ids.get(&key) else {
            continue;
        };
        for def_id in def_ids {
            adjacency
                .entry(TraversalNode::Use(use_site.use_id.clone()))
                .or_default()
                .push(TraversalEdge {
                    to: TraversalNode::Def(def_id.clone()),
                    render_edge: (),
                });
            if !concrete_use_to_def_edges.contains(&(use_site.use_id.clone(), def_id.clone())) {
                same_line_edges.insert((
                    use_site.use_id.clone(),
                    def_id.clone(),
                    "same-line".to_string(),
                ));
            }
        }
    }

    let selected_nodes = collect_reachable_traversal_nodes(
        &root_target_ids
            .iter()
            .cloned()
            .map(TraversalNode::Def)
            .collect::<Vec<_>>(),
        &adjacency,
    );
    let selected_target_ids = selected_nodes
        .iter()
        .filter_map(|node| match node {
            TraversalNode::Def(def_id) => Some(def_id.clone()),
            TraversalNode::Use(_) => None,
        })
        .collect::<BTreeSet<_>>();
    let selected_use_ids = selected_nodes
        .iter()
        .filter_map(|node| match node {
            TraversalNode::Use(use_id) => Some(use_id.clone()),
            TraversalNode::Def(_) => None,
        })
        .collect::<BTreeSet<_>>();

    for target_id in &selected_target_ids {
        let Some(bundle) = var_edges_by_target_id.get(target_id) else {
            continue;
        };
        for edge in bundle {
            let (source_node_id, source_record) = canonical_use_records
                .get(&edge.source_id)
                .cloned()
                .unwrap_or_else(|| {
                    (
                        edge.source_id.clone(),
                        uses_by_id
                            .get(&edge.source_id)
                            .map(|use_site| DefUseNodeRecord::usage(use_site))
                            .unwrap_or_else(|| {
                                DefUseNodeRecord::fallback_with_span(
                                    "use",
                                    &edge.source_place,
                                    &edge.span,
                                )
                            }),
                    )
                });
            let target_node_id = edge.target_id.clone();

            nodes
                .entry(source_node_id.clone())
                .or_insert_with(|| source_record.clone());
            nodes.entry(target_node_id.clone()).or_insert_with(|| {
                definitions_by_id
                    .get(&edge.target_id)
                    .map(|definition| DefUseNodeRecord::definition(definition))
                    .unwrap_or_else(|| {
                        DefUseNodeRecord::fallback_with_span("def", &edge.target_place, &edge.span)
                    })
            });
            edges.insert((source_node_id, target_node_id, edge.dep_kind.clone()));
        }
    }

    for target_id in &selected_target_ids {
        let Some(def_use_edges) = def_use_edges_by_def_id.get(target_id) else {
            continue;
        };
        for edge in def_use_edges {
            let def_node_id = edge.def_id.clone();
            let (use_node_id, use_record) = canonical_use_records
                .get(&edge.use_id)
                .cloned()
                .unwrap_or_else(|| {
                    (
                        edge.use_id.clone(),
                        uses_by_id
                            .get(&edge.use_id)
                            .map(|use_site| DefUseNodeRecord::usage(use_site))
                            .unwrap_or_else(|| {
                                DefUseNodeRecord::fallback("use", edge.place.clone())
                            }),
                    )
                });

            if let Some(definition) = definitions_by_id.get(&edge.def_id) {
                nodes
                    .entry(def_node_id.clone())
                    .or_insert_with(|| DefUseNodeRecord::definition(definition));
            }
            if uses_by_id.contains_key(&edge.use_id) {
                nodes
                    .entry(use_node_id.clone())
                    .or_insert_with(|| use_record.clone());
            }
            edges.insert((def_node_id, use_node_id, "def-use".to_string()));
        }
    }

    for (raw_use_id, def_id, edge_kind) in same_line_edges {
        if !selected_use_ids.contains(&raw_use_id) || !selected_target_ids.contains(&def_id) {
            continue;
        }
        let (use_id, use_record) = canonical_use_records
            .get(&raw_use_id)
            .cloned()
            .unwrap_or_else(|| {
                (
                    raw_use_id.clone(),
                    uses_by_id
                        .get(&raw_use_id)
                        .map(|use_site| DefUseNodeRecord::usage(use_site))
                        .unwrap_or_else(|| {
                            DefUseNodeRecord::fallback(
                                "use",
                                Place::Unknown {
                                    reason: "same-line".to_string(),
                                },
                            )
                        }),
                )
            });
        if uses_by_id.contains_key(&raw_use_id) {
            nodes
                .entry(use_id.clone())
                .or_insert_with(|| use_record.clone());
        }
        if let Some(definition) = definitions_by_id.get(&def_id) {
            nodes
                .entry(def_id.clone())
                .or_insert_with(|| DefUseNodeRecord::definition(definition));
        }
        edges.insert((use_id, def_id, edge_kind));
    }

    for (raw_use_id, def_ids) in &fallback_def_ids_by_use_id {
        if !selected_use_ids.contains(raw_use_id) {
            continue;
        }
        let Some((use_id, use_record)) = canonical_use_records.get(raw_use_id).cloned() else {
            continue;
        };
        if let Some(use_site) = uses_by_id.get(raw_use_id) {
            nodes
                .entry(use_id.clone())
                .or_insert_with(|| use_record.clone());
            for def_id in def_ids {
                if !selected_target_ids.contains(def_id) {
                    continue;
                }
                if let Some(definition) = definitions_by_id.get(def_id) {
                    nodes
                        .entry(def_id.clone())
                        .or_insert_with(|| DefUseNodeRecord::definition(definition));
                    if definition_precedes_use(definition, use_site) {
                        edges.insert((def_id.clone(), use_id.clone(), "def-use".to_string()));
                    }
                }
            }
        }
    }

    VarDependencyGraphParts {
        nodes,
        edges,
        root_node_ids: root_target_ids
            .into_iter()
            .map(TraversalNode::Def)
            .filter_map(|node| match node {
                TraversalNode::Def(def_id) if selected_target_ids.contains(&def_id) => Some(def_id),
                _ => None,
            })
            .collect(),
    }
}

fn render_var_dependency_dot(document: &RenderGraphDocument) -> String {
    let mut text = String::from(
        "digraph VariableDependencies {\n  rankdir=LR;\n  node [shape=box, fontsize=10];\n",
    );

    for node in &document.nodes {
        text.push_str(&format!(
            "  \"{}\" [label=\"{}\"];\n",
            dot_label(&node.id),
            dot_label(&node.label)
        ));
    }

    for edge in &document.edges {
        let extra_attrs = edge_dot_attributes(edge);
        text.push_str(&format!(
            "  \"{}\" -> \"{}\" [label=\"{}\"{}];\n",
            dot_label(&edge.from),
            dot_label(&edge.to),
            dot_label(&edge.label),
            extra_attrs
        ));
    }

    text.push_str("}\n");
    text
}

fn render_def_use_hotspot_dot(document: &RenderGraphDocument) -> String {
    let mut text =
        String::from("digraph DefUseHotspots {\n  rankdir=LR;\n  node [shape=box, fontsize=10];\n");

    for node in &document.nodes {
        text.push_str(&format!(
            "  \"{}\" [label=\"{}\"];\n",
            dot_label(&node.id),
            dot_label(&node.label)
        ));
    }

    for edge in &document.edges {
        let extra_attrs = edge_dot_attributes(edge);
        text.push_str(&format!(
            "  \"{}\" -> \"{}\" [label=\"{}\"{}];\n",
            dot_label(&edge.from),
            dot_label(&edge.to),
            dot_label(&edge.label),
            extra_attrs
        ));
    }

    text.push_str("}\n");
    text
}

fn build_render_graph_node(
    node_id: &str,
    record: &DefUseNodeRecord,
    label: &str,
    labels: &PlaceLabelContext,
    scope_owner_ids: &BTreeMap<String, String>,
    functions_by_id: &BTreeMap<String, &crate::ir::FunctionRecord>,
    modules_by_id: &BTreeMap<String, &crate::ir::ModuleRecord>,
    scope_module_ids: &BTreeMap<String, String>,
) -> RenderGraphNode {
    let function_id = record
        .scope_id
        .as_ref()
        .and_then(|scope_id| scope_owner_ids.get(scope_id))
        .filter(|owner_id| functions_by_id.contains_key(*owner_id))
        .cloned();
    let function = function_id
        .as_ref()
        .and_then(|function_id| functions_by_id.get(function_id))
        .map(|function| function.qualified_name.clone());
    let module = record
        .scope_id
        .as_ref()
        .and_then(|scope_id| scope_module_ids.get(scope_id))
        .and_then(|module_id| modules_by_id.get(module_id))
        .map(|module| module.module_name.clone())
        .or_else(|| match &record.place {
            Place::Global { module_id, .. } => modules_by_id
                .get(module_id)
                .map(|module| module.module_name.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let snippet = (!record.snippet.trim().is_empty()).then(|| record.snippet.clone());

    RenderGraphNode {
        id: node_id.to_string(),
        kind: record.role.to_string(),
        module,
        function,
        variable: render_graph_variable_name(record, labels),
        line: record.line,
        col: record.col,
        place_kind: place_kind_name(&record.place).to_string(),
        scope_id: record.scope_id.clone(),
        function_id,
        label: label.to_string(),
        tooltip: build_graph_node_tooltip(record, label),
        snippet,
        span: RenderSourceSpan {
            file: record.file.clone(),
            line: record.line,
            col: record.col,
            end_line: record.line,
            end_col: record.col,
        },
    }
}

fn scope_module_ids(cache: &AnalysisCache) -> BTreeMap<String, String> {
    let function_module_ids = cache
        .functions
        .iter()
        .map(|function| (function.function_id.clone(), function.module_id.clone()))
        .collect::<BTreeMap<_, _>>();
    let class_module_ids = cache
        .classes
        .iter()
        .map(|class_record| {
            (
                class_record.class_id.clone(),
                class_record.module_id.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let module_ids = cache
        .modules
        .iter()
        .map(|module_record| module_record.module_id.clone())
        .collect::<BTreeSet<_>>();

    cache
        .scopes
        .iter()
        .fold(BTreeMap::<String, String>::new(), |mut map, scope| {
            if module_ids.contains(&scope.owner_id) {
                map.insert(scope.scope_id.clone(), scope.owner_id.clone());
            } else if let Some(module_id) = function_module_ids.get(&scope.owner_id) {
                map.insert(scope.scope_id.clone(), module_id.clone());
            } else if let Some(module_id) = class_module_ids.get(&scope.owner_id) {
                map.insert(scope.scope_id.clone(), module_id.clone());
            }
            map
        })
}

fn build_render_graph_node_labels(
    nodes: &BTreeMap<String, DefUseNodeRecord>,
    labels: &PlaceLabelContext,
) -> BTreeMap<String, String> {
    let mut short_label_counts = BTreeMap::new();
    for record in nodes.values() {
        *short_label_counts
            .entry(record.base_label(labels))
            .or_insert(0usize) += 1;
    }

    let mut node_labels = BTreeMap::new();
    for (node_id, record) in nodes {
        let base_label = record.base_label(labels);
        let short_label = record.short_label(labels);
        let label = if short_label_counts
            .get(&base_label)
            .copied()
            .unwrap_or_default()
            > 1
        {
            record.disambiguated_label(labels)
        } else {
            short_label
        };
        node_labels.insert(node_id.clone(), label);
    }

    let mut final_label_counts = BTreeMap::new();
    for label in node_labels.values() {
        *final_label_counts.entry(label.clone()).or_insert(0usize) += 1;
    }

    for (node_id, record) in nodes {
        if let Some(label) = node_labels.get_mut(node_id) {
            if final_label_counts.get(label).copied().unwrap_or_default() > 1 {
                *label = record.fully_disambiguated_label(labels);
            }
        }
    }

    node_labels
}

fn build_render_graph_edge(from: &str, to: &str, kind: &str) -> RenderGraphEdge {
    let (style, color, dir) = match kind {
        "call-arg" => (
            Some("dashed".to_string()),
            Some("steelblue3".to_string()),
            None,
        ),
        "def-use" => (
            Some("dotted".to_string()),
            Some("darkolivegreen4".to_string()),
            None,
        ),
        "same-line" => (Some("dashed".to_string()), Some("gray60".to_string()), None),
        _ => (None, None, None),
    };

    RenderGraphEdge {
        id: stable_id("GE", SCHEMA_VERSION, &[from, to, kind]),
        from: from.to_string(),
        to: to.to_string(),
        kind: kind.to_string(),
        label: kind.to_string(),
        color,
        style,
        dir,
    }
}

fn build_hotspot_render_graph_edge(edge: &HotspotEdgeSpec) -> RenderGraphEdge {
    let kind = hotspot_edge_kind_name(edge);
    let (style, color, dir) = match edge.kind {
        HotspotEdgeKind::Local => (None, None, None),
        HotspotEdgeKind::CallArg => (
            Some("dashed".to_string()),
            Some("steelblue3".to_string()),
            None,
        ),
        HotspotEdgeKind::SameLine => (
            Some("dashed".to_string()),
            Some("gray60".to_string()),
            Some("none".to_string()),
        ),
    };

    RenderGraphEdge {
        id: stable_id("GE", SCHEMA_VERSION, &[&edge.def_id, &edge.use_id, &kind]),
        from: edge.def_id.clone(),
        to: edge.use_id.clone(),
        kind,
        label: edge.label.clone(),
        color,
        style,
        dir,
    }
}

fn hotspot_edge_kind_name(edge: &HotspotEdgeSpec) -> String {
    match edge.kind {
        HotspotEdgeKind::Local => {
            if edge.label.is_empty() {
                "local".to_string()
            } else {
                edge.label.clone()
            }
        }
        HotspotEdgeKind::CallArg => "call-arg".to_string(),
        HotspotEdgeKind::SameLine => "same-line".to_string(),
    }
}

fn build_render_graph_clusters(
    nodes: &[RenderGraphNode],
    labels: &PlaceLabelContext,
    scope_owner_ids: &BTreeMap<String, String>,
    functions_by_id: &BTreeMap<String, &crate::ir::FunctionRecord>,
    modules_by_id: &BTreeMap<String, &crate::ir::ModuleRecord>,
    scope_module_ids: &BTreeMap<String, String>,
) -> Vec<RenderGraphCluster> {
    let mut clusters = BTreeMap::<String, Vec<String>>::new();

    for node in nodes {
        let Some(cluster_label) = node_cluster_label(
            node,
            labels,
            scope_owner_ids,
            functions_by_id,
            modules_by_id,
            scope_module_ids,
        ) else {
            continue;
        };
        clusters
            .entry(cluster_label)
            .or_default()
            .push(node.id.clone());
    }

    clusters
        .into_iter()
        .map(|(label, mut node_ids)| {
            node_ids.sort();
            RenderGraphCluster {
                id: stable_id("GC", SCHEMA_VERSION, &[&label]),
                label,
                node_ids,
            }
        })
        .collect()
}

fn build_hotspot_render_graph_paths(
    paths: &[HotspotPathCandidate],
    edges: &[RenderGraphEdge],
) -> Vec<RenderGraphPath> {
    let mut incoming = BTreeMap::<String, usize>::new();
    let mut outgoing = BTreeMap::<String, usize>::new();
    for edge in edges {
        *outgoing.entry(edge.from.clone()).or_insert(0) += 1;
        *incoming.entry(edge.to.clone()).or_insert(0) += 1;
    }

    paths
        .iter()
        .map(|path| {
            let node_ids = path
                .node_sequence
                .iter()
                .map(|node| match node {
                    TraversalNode::Def(def_id) => def_id.clone(),
                    TraversalNode::Use(use_id) => use_id.clone(),
                })
                .collect::<Vec<_>>();
            let edge_ids = path
                .render_edges
                .iter()
                .map(|edge| build_hotspot_render_graph_edge(edge).id)
                .collect::<Vec<_>>();
            let max_fan_in = node_ids
                .iter()
                .map(|node_id| incoming.get(node_id).copied().unwrap_or_default())
                .max()
                .unwrap_or_default();
            let max_fan_out = node_ids
                .iter()
                .map(|node_id| outgoing.get(node_id).copied().unwrap_or_default())
                .max()
                .unwrap_or_default();

            RenderGraphPath {
                id: stable_id("GP", SCHEMA_VERSION, &[&path.path_id]),
                root_node_id: path.root_def_id.clone(),
                node_ids,
                edge_ids,
                path_len: path.path_len,
                max_fan_in,
                max_fan_out,
                score_kind: "selected".to_string(),
            }
        })
        .collect()
}

fn build_var_dependency_render_graph_paths(
    root_node_ids: &[String],
    edges: &[RenderGraphEdge],
) -> Vec<RenderGraphPath> {
    let mut adjacency = BTreeMap::<String, Vec<&RenderGraphEdge>>::new();
    let mut incoming = BTreeMap::<String, usize>::new();
    let mut outgoing = BTreeMap::<String, usize>::new();

    for edge in edges {
        adjacency.entry(edge.from.clone()).or_default().push(edge);
        *outgoing.entry(edge.from.clone()).or_insert(0) += 1;
        *incoming.entry(edge.to.clone()).or_insert(0) += 1;
    }

    for edge_list in adjacency.values_mut() {
        edge_list.sort_by(|left, right| {
            left.to
                .cmp(&right.to)
                .then_with(|| left.kind.cmp(&right.kind))
                .then_with(|| left.id.cmp(&right.id))
        });
    }

    let mut paths = Vec::new();
    let mut seen = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut node_ids = Vec::new();
    let mut edge_ids = Vec::new();

    for root_node_id in root_node_ids {
        visit_var_dependency_render_paths(
            root_node_id,
            root_node_id,
            &adjacency,
            &incoming,
            &outgoing,
            &mut visited,
            &mut node_ids,
            &mut edge_ids,
            &mut seen,
            &mut paths,
        );
    }

    paths
}

#[allow(clippy::too_many_arguments)]
fn visit_var_dependency_render_paths(
    current_node_id: &str,
    root_node_id: &str,
    adjacency: &BTreeMap<String, Vec<&RenderGraphEdge>>,
    incoming: &BTreeMap<String, usize>,
    outgoing: &BTreeMap<String, usize>,
    visited: &mut BTreeSet<String>,
    node_ids: &mut Vec<String>,
    edge_ids: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
    paths: &mut Vec<RenderGraphPath>,
) {
    visited.insert(current_node_id.to_string());
    node_ids.push(current_node_id.to_string());

    let mut explored_child = false;
    if let Some(next_edges) = adjacency.get(current_node_id) {
        for edge in next_edges {
            if visited.contains(&edge.to) {
                continue;
            }
            explored_child = true;
            edge_ids.push(edge.id.clone());
            visit_var_dependency_render_paths(
                &edge.to,
                root_node_id,
                adjacency,
                incoming,
                outgoing,
                visited,
                node_ids,
                edge_ids,
                seen,
                paths,
            );
            edge_ids.pop();
        }
    }

    if !explored_child {
        let path_key = format!("{}|{}", node_ids.join("->"), edge_ids.join("->"));
        if seen.insert(path_key) {
            let max_fan_in = node_ids
                .iter()
                .map(|node_id| incoming.get(node_id).copied().unwrap_or_default())
                .max()
                .unwrap_or_default();
            let max_fan_out = node_ids
                .iter()
                .map(|node_id| outgoing.get(node_id).copied().unwrap_or_default())
                .max()
                .unwrap_or_default();
            paths.push(RenderGraphPath {
                id: stable_id("GP", SCHEMA_VERSION, &[root_node_id, &node_ids.join("->")]),
                root_node_id: root_node_id.to_string(),
                node_ids: node_ids.clone(),
                edge_ids: edge_ids.clone(),
                path_len: node_ids.len(),
                max_fan_in,
                max_fan_out,
                score_kind: "reachable".to_string(),
            });
        }
    }

    node_ids.pop();
    visited.remove(current_node_id);
}

fn node_cluster_label(
    node: &RenderGraphNode,
    labels: &PlaceLabelContext,
    scope_owner_ids: &BTreeMap<String, String>,
    functions_by_id: &BTreeMap<String, &crate::ir::FunctionRecord>,
    modules_by_id: &BTreeMap<String, &crate::ir::ModuleRecord>,
    scope_module_ids: &BTreeMap<String, String>,
) -> Option<String> {
    let scope_id = node.scope_id.as_deref()?;
    if let Some(owner_id) = scope_owner_ids.get(scope_id) {
        if let Some(function) = functions_by_id.get(owner_id) {
            return Some(qualify_owner_label(
                modules_by_id
                    .get(&function.module_id)
                    .map(|module| module.module_name.as_str()),
                &function.qualified_name,
            ));
        }
    }

    labels.scope_label(scope_id).or_else(|| {
        scope_module_ids
            .get(scope_id)
            .and_then(|module_id| modules_by_id.get(module_id))
            .map(|module| module.module_name.clone())
    })
}

fn render_graph_variable_name(record: &DefUseNodeRecord, labels: &PlaceLabelContext) -> String {
    match &record.place {
        Place::Attribute { .. } | Place::Subscript { .. } => {
            if record.role == "def" {
                extract_assignment_target_text(&record.snippet)
                    .unwrap_or_else(|| labels.short_label(&record.place))
            } else if !record.snippet.trim().is_empty() {
                record.snippet.trim().to_string()
            } else {
                labels.short_label(&record.place)
            }
        }
        _ => labels.short_label(&record.place),
    }
}

fn build_graph_node_tooltip(record: &DefUseNodeRecord, label: &str) -> String {
    let mut parts = vec![label.to_string()];
    if !record.file.is_empty() {
        parts.push(format!("{}:{}:{}", record.file, record.line, record.col));
    }
    if !record.snippet.trim().is_empty() {
        parts.push(record.snippet.trim().to_string());
    }
    parts.join("\n")
}

fn place_kind_name(place: &Place) -> &'static str {
    match place {
        Place::Local { .. } => "local",
        Place::Global { .. } => "global",
        Place::Closure { .. } => "closure",
        Place::Attribute { .. } => "attribute",
        Place::Subscript { .. } => "subscript",
        Place::External { .. } => "external",
        Place::Unknown { .. } => "unknown",
    }
}

fn edge_dot_attributes(edge: &RenderGraphEdge) -> String {
    let mut attrs = Vec::new();
    if let Some(style) = &edge.style {
        attrs.push(format!("style={style}"));
    }
    if let Some(color) = &edge.color {
        attrs.push(format!("color=\"{color}\""));
    }
    if let Some(dir) = &edge.dir {
        attrs.push(format!("dir={dir}"));
    }
    if attrs.is_empty() {
        String::new()
    } else {
        format!(", {}", attrs.join(", "))
    }
}

#[cfg(test)]
fn select_var_dependency_seed_edges<'a>(
    cache: &'a AnalysisCache,
    top_n: usize,
) -> Vec<&'a crate::ir::VarDependencyEdge> {
    if top_n == 0 {
        return Vec::new();
    }

    let definitions_by_id = cache
        .definitions
        .iter()
        .map(|definition| (definition.def_id.clone(), definition))
        .collect::<BTreeMap<_, _>>();
    let mut bundles = BTreeMap::<String, Vec<&crate::ir::VarDependencyEdge>>::new();
    let mut source_to_target_ids = BTreeMap::<String, BTreeSet<String>>::new();
    let mut def_to_use_ids = BTreeMap::<String, BTreeSet<String>>::new();

    for edge in &cache.var_dependency_edges {
        bundles
            .entry(edge.target_id.clone())
            .or_default()
            .push(edge);
        source_to_target_ids
            .entry(edge.source_id.clone())
            .or_default()
            .insert(edge.target_id.clone());
    }
    for edge in &cache.def_use_edges {
        def_to_use_ids
            .entry(edge.def_id.clone())
            .or_default()
            .insert(edge.use_id.clone());
    }

    for bundle in bundles.values_mut() {
        bundle.sort_by(|left, right| {
            left.span
                .file
                .cmp(&right.span.file)
                .then_with(|| left.span.line.cmp(&right.span.line))
                .then_with(|| left.span.col.cmp(&right.span.col))
                .then_with(|| left.source_id.cmp(&right.source_id))
                .then_with(|| left.edge_id.cmp(&right.edge_id))
        });
    }
    let bundle_costs = bundles
        .iter()
        .map(|(target_id, bundle)| {
            (
                target_id.clone(),
                bundle
                    .iter()
                    .map(|edge| {
                        (
                            edge.source_id.clone(),
                            edge.target_id.clone(),
                            edge.dep_kind.clone(),
                        )
                    })
                    .collect::<BTreeSet<_>>()
                    .len(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let mut bundle_ids = bundles.keys().cloned().collect::<Vec<_>>();
    bundle_ids.sort_by(|left, right| {
        let left_len = bundle_costs.get(left).copied().unwrap_or_default();
        let right_len = bundle_costs.get(right).copied().unwrap_or_default();

        right_len
            .cmp(&left_len)
            .then_with(|| {
                def_sort_key(definitions_by_id.get(left))
                    .cmp(&def_sort_key(definitions_by_id.get(right)))
            })
            .then_with(|| left.cmp(right))
    });

    let call_arg_bundle_ids = bundle_ids
        .iter()
        .filter(|target_id| {
            bundles
                .get(*target_id)
                .map(|bundle| bundle.iter().any(|edge| edge.dep_kind == "call-arg"))
                .unwrap_or(false)
        })
        .cloned()
        .collect::<Vec<_>>();
    let regular_bundle_ids = bundle_ids
        .iter()
        .filter(|target_id| {
            bundles
                .get(*target_id)
                .map(|bundle| bundle.iter().all(|edge| edge.dep_kind != "call-arg"))
                .unwrap_or(false)
        })
        .cloned()
        .collect::<Vec<_>>();
    let round_count = call_arg_bundle_ids.len().max(regular_bundle_ids.len());
    let mut rounds = Vec::with_capacity(round_count.saturating_mul(2));
    for _ in 0..round_count {
        rounds.push(RankedSelectionRound::new(&call_arg_bundle_ids, 1));
        rounds.push(RankedSelectionRound::new(&regular_bundle_ids, 1));
    }
    bundle_ids =
        select_ranked_candidates_by_rounds(&rounds, usize::MAX, |target_id| target_id.clone());
    let bundle_ranks = bundle_ids
        .iter()
        .enumerate()
        .map(|(index, target_id)| (target_id.clone(), index))
        .collect::<BTreeMap<_, _>>();

    let mut selected = Vec::new();
    let mut selected_count = 0usize;
    let mut selected_target_ids = BTreeSet::new();

    for target_id in bundle_ids {
        if selected_count >= top_n && !selected.is_empty() {
            break;
        }
        select_var_dependency_bundle_with_source_closure(
            &target_id,
            &mut bundles,
            &bundle_costs,
            &source_to_target_ids,
            &def_to_use_ids,
            &bundle_ranks,
            &mut selected_target_ids,
            &mut selected_count,
            &mut selected,
        );
    }

    selected
}

#[cfg(test)]
fn select_var_dependency_bundle_with_source_closure<'a>(
    seed_target_id: &str,
    bundles: &mut BTreeMap<String, Vec<&'a crate::ir::VarDependencyEdge>>,
    bundle_costs: &BTreeMap<String, usize>,
    source_to_target_ids: &BTreeMap<String, BTreeSet<String>>,
    def_to_use_ids: &BTreeMap<String, BTreeSet<String>>,
    bundle_ranks: &BTreeMap<String, usize>,
    selected_target_ids: &mut BTreeSet<String>,
    selected_count: &mut usize,
    selected: &mut Vec<&'a crate::ir::VarDependencyEdge>,
) {
    let mut pending = vec![seed_target_id.to_string()];

    while let Some(target_id) = pending.pop() {
        if selected_target_ids.contains(&target_id) {
            continue;
        }
        let Some(bundle) = bundles.remove(&target_id) else {
            continue;
        };

        *selected_count += bundle_costs.get(&target_id).copied().unwrap_or_default();
        selected_target_ids.insert(target_id.clone());

        let mut closure_target_ids = bundle
            .iter()
            .flat_map(|edge| {
                source_to_target_ids
                    .get(&edge.source_id)
                    .into_iter()
                    .flat_map(|target_ids| target_ids.iter().cloned())
            })
            .filter(|sibling_target_id| {
                !selected_target_ids.contains(sibling_target_id)
                    && bundles.contains_key(sibling_target_id)
            })
            .collect::<BTreeSet<_>>();
        for use_id in def_to_use_ids
            .get(&target_id)
            .into_iter()
            .flat_map(|use_ids| use_ids.iter())
        {
            if let Some(target_ids) = source_to_target_ids.get(use_id) {
                closure_target_ids.extend(
                    target_ids
                        .iter()
                        .filter(|downstream_target_id| {
                            !selected_target_ids.contains(*downstream_target_id)
                                && bundles.contains_key(*downstream_target_id)
                        })
                        .cloned(),
                );
            }
        }

        let mut closure_target_ids = closure_target_ids.into_iter().collect::<Vec<_>>();
        closure_target_ids.sort_by(|left, right| {
            bundle_ranks
                .get(left)
                .copied()
                .unwrap_or(usize::MAX)
                .cmp(&bundle_ranks.get(right).copied().unwrap_or(usize::MAX))
                .then_with(|| left.cmp(right))
        });

        selected.extend(bundle);

        for closure_target_id in closure_target_ids.into_iter().rev() {
            pending.push(closure_target_id);
        }
    }
}

pub fn render_svg(dot_path: &Path, svg_path: &Path) -> Result<bool> {
    let output = Command::new("dot")
        .arg("-Tsvg")
        .arg(dot_path)
        .arg("-o")
        .arg(svg_path)
        .output();

    match output {
        Ok(output) if output.status.success() => {
            post_process_svg_metadata(dot_path, svg_path)?;
            Ok(true)
        }
        Ok(_) => Ok(false),
        Err(_) => Ok(false),
    }
}

fn write_dot(path: &Path, text: &str) -> Result<()> {
    fs::write(path, text).with_context(|| format!("failed to write {}", path.display()))
}

fn post_process_svg_metadata(dot_path: &Path, svg_path: &Path) -> Result<()> {
    let dot = fs::read_to_string(dot_path)
        .with_context(|| format!("failed to read {}", dot_path.display()))?;
    let svg = fs::read_to_string(svg_path)
        .with_context(|| format!("failed to read {}", svg_path.display()))?;
    let rewritten = rewrite_svg_metadata(&dot, &svg);
    fs::write(svg_path, rewritten)
        .with_context(|| format!("failed to write {}", svg_path.display()))
}

fn rewrite_svg_metadata(dot: &str, svg: &str) -> String {
    let metadata = build_svg_metadata_map(dot);
    svg.split_inclusive('\n')
        .map(|line| rewrite_svg_line(line, &metadata))
        .collect()
}

fn build_svg_metadata_map(dot: &str) -> BTreeMap<String, String> {
    let mut node_labels = BTreeMap::new();
    let mut edge_specs = Vec::new();

    for line in dot.lines() {
        if let Some((node_id, label)) = parse_dot_node_label(line) {
            node_labels.insert(node_id, label);
            continue;
        }
        if let Some((source_id, target_id, label)) = parse_dot_edge_label(line) {
            edge_specs.push((source_id, target_id, label));
        }
    }

    let mut metadata = node_labels.clone();
    for (source_id, target_id, edge_label) in edge_specs {
        let source_label = node_labels
            .get(&source_id)
            .cloned()
            .unwrap_or(source_id.clone());
        let target_label = node_labels
            .get(&target_id)
            .cloned()
            .unwrap_or(target_id.clone());
        let title = if edge_label.is_empty() {
            format!("{source_label} -> {target_label}")
        } else {
            format!("{source_label} -> {target_label} ({edge_label})")
        };
        metadata.insert(format!("{source_id}->{target_id}"), title.clone());
        metadata.insert(format!("{source_id}&#45;&gt;{target_id}"), title);
    }

    metadata
}

fn rewrite_svg_line(line: &str, metadata: &BTreeMap<String, String>) -> String {
    let line = rewrite_svg_comment(line, metadata);
    rewrite_svg_title(&line, metadata)
}

fn rewrite_svg_comment(line: &str, metadata: &BTreeMap<String, String>) -> String {
    let Some(start) = line.find("<!-- ") else {
        return line.to_string();
    };
    let Some(end) = line[start + 5..].find(" -->") else {
        return line.to_string();
    };
    let end = start + 5 + end;
    let key = &line[start + 5..end];
    let Some(label) = metadata.get(key) else {
        return line.to_string();
    };
    let safe_label = label.replace("--", "- -");
    format!(
        "{}<!-- {} -->{}",
        &line[..start],
        safe_label,
        &line[end + 4..]
    )
}

fn rewrite_svg_title(line: &str, metadata: &BTreeMap<String, String>) -> String {
    let Some(start) = line.find("<title>") else {
        return line.to_string();
    };
    let Some(end) = line[start + 7..].find("</title>") else {
        return line.to_string();
    };
    let end = start + 7 + end;
    let key = &line[start + 7..end];
    let Some(label) = metadata.get(key) else {
        return line.to_string();
    };
    let escaped = html_escape::encode_text(label);
    format!(
        "{}<title>{}</title>{}",
        &line[..start],
        escaped,
        &line[end + 8..]
    )
}

fn parse_dot_node_label(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    let (node_id, offset) = parse_dot_quoted(trimmed)?;
    let rest = trimmed[offset..].trim_start();
    if rest.starts_with("->") {
        return None;
    }
    let label_start = rest.find("[label=")?;
    let (_, label_offset) = parse_dot_quoted(&rest[label_start + 7..])?;
    let label = parse_dot_quoted(&rest[label_start + 7..])?.0;
    let _ = label_offset;
    Some((node_id, label))
}

fn parse_dot_edge_label(line: &str) -> Option<(String, String, String)> {
    let trimmed = line.trim();
    let (source_id, source_offset) = parse_dot_quoted(trimmed)?;
    let rest = trimmed[source_offset..].trim_start();
    if !rest.starts_with("->") {
        return None;
    }
    let rest = rest[2..].trim_start();
    let (target_id, target_offset) = parse_dot_quoted(rest)?;
    let rest = rest[target_offset..].trim_start();
    let label_start = rest.find("[label=")?;
    let label = parse_dot_quoted(&rest[label_start + 7..])?.0;
    Some((source_id, target_id, label))
}

fn parse_dot_quoted(text: &str) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    if bytes.first().copied()? != b'"' {
        return None;
    }

    let mut result = String::new();
    let mut index = 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => {
                index += 1;
                let escaped = *bytes.get(index)?;
                match escaped {
                    b'\\' => result.push('\\'),
                    b'"' => result.push('"'),
                    b'n' => result.push('\n'),
                    other => result.push(other as char),
                }
            }
            b'"' => return Some((result, index + 1)),
            byte => result.push(byte as char),
        }
        index += 1;
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{
        AnalysisCache, DefUseEdge, Definition, FunctionRecord, Place, SCHEMA_VERSION, ScopeRecord,
        Use,
    };

    #[test]
    fn dot_label_escapes_quotes_backslashes_and_newlines() {
        assert_eq!(dot_label("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
    }

    #[test]
    fn ranked_selector_balances_quotas_dedupes_and_backfills() {
        let length_ranked = vec![
            "path:length:1".to_string(),
            "path:length:2".to_string(),
            "path:shared".to_string(),
        ];
        let fan_ranked = vec![
            "path:shared".to_string(),
            "path:fan:1".to_string(),
            "path:fan:2".to_string(),
        ];

        let selected = select_ranked_candidates_by_rounds(
            &[
                RankedSelectionRound::new(&length_ranked, 2),
                RankedSelectionRound::new(&fan_ranked, 1),
                RankedSelectionRound::all(&length_ranked),
                RankedSelectionRound::all(&fan_ranked),
            ],
            5,
            |candidate| candidate.clone(),
        );

        assert_eq!(
            selected,
            vec![
                "path:length:1".to_string(),
                "path:length:2".to_string(),
                "path:shared".to_string(),
                "path:fan:1".to_string(),
                "path:fan:2".to_string(),
            ]
        );
    }

    #[test]
    fn ranked_selector_supports_round_robin_interleaving() {
        let call_arg_ranked = vec!["bundle:call:1".to_string(), "bundle:call:2".to_string()];
        let regular_ranked = vec![
            "bundle:regular:1".to_string(),
            "bundle:regular:2".to_string(),
            "bundle:regular:3".to_string(),
        ];

        let selected = select_ranked_candidates_by_rounds(
            &[
                RankedSelectionRound::new(&call_arg_ranked, 1),
                RankedSelectionRound::new(&regular_ranked, 1),
                RankedSelectionRound::new(&call_arg_ranked, 1),
                RankedSelectionRound::new(&regular_ranked, 1),
                RankedSelectionRound::new(&call_arg_ranked, 1),
                RankedSelectionRound::new(&regular_ranked, 1),
            ],
            5,
            |candidate| candidate.clone(),
        );

        assert_eq!(
            selected,
            vec![
                "bundle:call:1".to_string(),
                "bundle:regular:1".to_string(),
                "bundle:call:2".to_string(),
                "bundle:regular:2".to_string(),
                "bundle:regular:3".to_string(),
            ]
        );
    }

    #[test]
    fn traversal_reachability_walks_def_use_and_back_to_def() {
        let adjacency = BTreeMap::from([
            (
                TraversalNode::Def("D_root".to_string()),
                vec![TraversalEdge {
                    to: TraversalNode::Use("U_mid".to_string()),
                    render_edge: (),
                }],
            ),
            (
                TraversalNode::Use("U_mid".to_string()),
                vec![TraversalEdge {
                    to: TraversalNode::Def("D_leaf".to_string()),
                    render_edge: (),
                }],
            ),
        ]);

        let selected = collect_reachable_traversal_nodes(
            &[TraversalNode::Def("D_root".to_string())],
            &adjacency,
        );

        assert_eq!(
            selected,
            BTreeSet::from([
                TraversalNode::Def("D_root".to_string()),
                TraversalNode::Use("U_mid".to_string()),
                TraversalNode::Def("D_leaf".to_string()),
            ])
        );
    }

    #[test]
    fn def_use_hotspot_writer_emits_dot_edges() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hotspots.dot");
        let def_span = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 10,
            col: 4,
            end_line: 10,
            end_col: 9,
            snippet: "x = value".to_string(),
        };
        let use_span = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 11,
            col: 11,
            end_line: 11,
            end_col: 12,
            snippet: "print(x)".to_string(),
        };
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            definitions: vec![Definition {
                def_id: "D_x".to_string(),
                place: Place::Local {
                    scope_id: "S".to_string(),
                    name: "x".to_string(),
                },
                def_kind: "assign".to_string(),
                scope_id: "S".to_string(),
                function_id: None,
                span: def_span,
                expr: "value".to_string(),
                deps: Vec::new(),
            }],
            uses: vec![Use {
                use_id: "U_x".to_string(),
                place: Place::Local {
                    scope_id: "S".to_string(),
                    name: "x".to_string(),
                },
                use_kind: "load".to_string(),
                scope_id: "S".to_string(),
                function_id: None,
                span: use_span,
                context: "call".to_string(),
            }],
            def_use_edges: vec![DefUseEdge {
                edge_id: "DU_1".to_string(),
                def_id: "D_x".to_string(),
                use_id: "U_x".to_string(),
                place: Place::Local {
                    scope_id: "S".to_string(),
                    name: "x".to_string(),
                },
                edge_kind: "local".to_string(),
                path_summary: "same-block".to_string(),
            }],
            ..AnalysisCache::default()
        };

        write_def_use_hotspots_dot(&cache, &path, 10).unwrap();

        let dot = std::fs::read_to_string(path).unwrap();
        assert!(dot.contains("\"D_x\" -> \"U_x\""));
        assert!(dot.contains("[label=\"def x @ line 10\"]"));
        assert!(dot.contains("[label=\"use x @ line 11\"]"));
        assert!(dot.contains("local"));
    }

    #[test]
    fn def_use_hotspot_writer_prefers_longest_paths_over_wide_definition_bundles() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hotspots.dot");
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            definitions: vec![
                Definition {
                    def_id: "D_hot".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "hot".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan::synthetic("app/main.py", "hot = 1"),
                    expr: "1".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_chain_1".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "chain_1".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan::synthetic("app/main.py", "chain_1 = 1"),
                    expr: "1".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_chain_2".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "chain_2".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan {
                        file: "app/main.py".to_string(),
                        line: 10,
                        col: 12,
                        end_line: 10,
                        end_col: 23,
                        snippet: "value = chain_1".to_string(),
                    },
                    expr: "chain_1".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_chain_3".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "chain_3".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan {
                        file: "app/main.py".to_string(),
                        line: 20,
                        col: 12,
                        end_line: 20,
                        end_col: 23,
                        snippet: "value = chain_2".to_string(),
                    },
                    expr: "chain_2".to_string(),
                    deps: Vec::new(),
                },
            ],
            uses: vec![
                Use {
                    use_id: "U_hot_1".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "hot".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan::synthetic("app/main.py", "hot"),
                    context: "assign:rhs".to_string(),
                },
                Use {
                    use_id: "U_hot_2".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "hot".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan {
                        file: "app/main.py".to_string(),
                        line: 2,
                        col: 4,
                        end_line: 2,
                        end_col: 7,
                        snippet: "hot".to_string(),
                    },
                    context: "assign:rhs".to_string(),
                },
                Use {
                    use_id: "U_hot_3".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "hot".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan {
                        file: "app/main.py".to_string(),
                        line: 3,
                        col: 4,
                        end_line: 3,
                        end_col: 7,
                        snippet: "hot".to_string(),
                    },
                    context: "assign:rhs".to_string(),
                },
                Use {
                    use_id: "U_chain_1".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "chain_1".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan {
                        file: "app/main.py".to_string(),
                        line: 10,
                        col: 4,
                        end_line: 10,
                        end_col: 11,
                        snippet: "value = chain_1".to_string(),
                    },
                    context: "assign:rhs".to_string(),
                },
                Use {
                    use_id: "U_chain_2".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "chain_2".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan {
                        file: "app/main.py".to_string(),
                        line: 20,
                        col: 4,
                        end_line: 20,
                        end_col: 11,
                        snippet: "value = chain_2".to_string(),
                    },
                    context: "assign:rhs".to_string(),
                },
                Use {
                    use_id: "U_chain_3".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "chain_3".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan {
                        file: "app/main.py".to_string(),
                        line: 30,
                        col: 4,
                        end_line: 30,
                        end_col: 11,
                        snippet: "print(chain_3)".to_string(),
                    },
                    context: "assign:rhs".to_string(),
                },
            ],
            def_use_edges: vec![
                DefUseEdge {
                    edge_id: "DU_hot_1".to_string(),
                    def_id: "D_hot".to_string(),
                    use_id: "U_hot_1".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "hot".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "hot".to_string(),
                },
                DefUseEdge {
                    edge_id: "DU_hot_2".to_string(),
                    def_id: "D_hot".to_string(),
                    use_id: "U_hot_2".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "hot".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "hot".to_string(),
                },
                DefUseEdge {
                    edge_id: "DU_hot_3".to_string(),
                    def_id: "D_hot".to_string(),
                    use_id: "U_hot_3".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "hot".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "hot".to_string(),
                },
                DefUseEdge {
                    edge_id: "DU_chain_1".to_string(),
                    def_id: "D_chain_1".to_string(),
                    use_id: "U_chain_1".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "chain_1".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "chain".to_string(),
                },
                DefUseEdge {
                    edge_id: "DU_chain_2".to_string(),
                    def_id: "D_chain_2".to_string(),
                    use_id: "U_chain_2".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "chain_2".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "chain".to_string(),
                },
                DefUseEdge {
                    edge_id: "DU_chain_3".to_string(),
                    def_id: "D_chain_3".to_string(),
                    use_id: "U_chain_3".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "chain_3".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "chain".to_string(),
                },
            ],
            ..AnalysisCache::default()
        };

        write_def_use_hotspots_dot(&cache, &path, 1).unwrap();

        let dot = std::fs::read_to_string(path).unwrap();
        assert!(dot.contains("\"D_chain_1\" -> \"U_chain_1\" [label=\"local\"]"));
        assert!(dot.contains("\"D_chain_2\" -> \"U_chain_2\" [label=\"local\"]"));
        assert!(dot.contains("\"D_chain_3\" -> \"U_chain_3\" [label=\"local\"]"));
        assert!(!dot.contains("\"D_hot\" -> \"U_hot_1\" [label=\"local\"]"));
    }

    #[test]
    fn def_use_hotspot_writer_splits_odd_top_n_between_length_and_fan_paths() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hotspots.dot");
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            definitions: vec![
                Definition {
                    def_id: "D_long_1".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "long_1".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan::synthetic("app/main.py", "long_1 = 1"),
                    expr: "1".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_long_2".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "long_2".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan {
                        file: "app/main.py".to_string(),
                        line: 10,
                        col: 12,
                        end_line: 10,
                        end_col: 22,
                        snippet: "x = long_1".to_string(),
                    },
                    expr: "long_1".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_long_3".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "long_3".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan {
                        file: "app/main.py".to_string(),
                        line: 20,
                        col: 12,
                        end_line: 20,
                        end_col: 22,
                        snippet: "x = long_2".to_string(),
                    },
                    expr: "long_2".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_mid_1".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "mid_1".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan {
                        file: "app/main.py".to_string(),
                        line: 100,
                        col: 1,
                        end_line: 100,
                        end_col: 10,
                        snippet: "mid_1 = 1".to_string(),
                    },
                    expr: "1".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_mid_2".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "mid_2".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan {
                        file: "app/main.py".to_string(),
                        line: 110,
                        col: 12,
                        end_line: 110,
                        end_col: 21,
                        snippet: "x = mid_1".to_string(),
                    },
                    expr: "mid_1".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_fan".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "fan".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan {
                        file: "app/main.py".to_string(),
                        line: 200,
                        col: 1,
                        end_line: 200,
                        end_col: 8,
                        snippet: "fan = 1".to_string(),
                    },
                    expr: "1".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_short".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "short".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan {
                        file: "app/main.py".to_string(),
                        line: 300,
                        col: 1,
                        end_line: 300,
                        end_col: 10,
                        snippet: "short = 1".to_string(),
                    },
                    expr: "1".to_string(),
                    deps: Vec::new(),
                },
            ],
            uses: vec![
                Use {
                    use_id: "U_long_1".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "long_1".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan {
                        file: "app/main.py".to_string(),
                        line: 10,
                        col: 5,
                        end_line: 10,
                        end_col: 11,
                        snippet: "x = long_1".to_string(),
                    },
                    context: "assign:rhs".to_string(),
                },
                Use {
                    use_id: "U_long_2".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "long_2".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan {
                        file: "app/main.py".to_string(),
                        line: 20,
                        col: 5,
                        end_line: 20,
                        end_col: 11,
                        snippet: "x = long_2".to_string(),
                    },
                    context: "assign:rhs".to_string(),
                },
                Use {
                    use_id: "U_long_3".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "long_3".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan {
                        file: "app/main.py".to_string(),
                        line: 30,
                        col: 5,
                        end_line: 30,
                        end_col: 11,
                        snippet: "print(long_3)".to_string(),
                    },
                    context: "assign:rhs".to_string(),
                },
                Use {
                    use_id: "U_mid_1".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "mid_1".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan {
                        file: "app/main.py".to_string(),
                        line: 110,
                        col: 5,
                        end_line: 110,
                        end_col: 10,
                        snippet: "x = mid_1".to_string(),
                    },
                    context: "assign:rhs".to_string(),
                },
                Use {
                    use_id: "U_mid_2".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "mid_2".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan {
                        file: "app/main.py".to_string(),
                        line: 120,
                        col: 5,
                        end_line: 120,
                        end_col: 10,
                        snippet: "print(mid_2)".to_string(),
                    },
                    context: "assign:rhs".to_string(),
                },
                Use {
                    use_id: "U_fan_1".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "fan".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan {
                        file: "app/main.py".to_string(),
                        line: 201,
                        col: 4,
                        end_line: 201,
                        end_col: 7,
                        snippet: "fan".to_string(),
                    },
                    context: "assign:rhs".to_string(),
                },
                Use {
                    use_id: "U_fan_2".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "fan".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan {
                        file: "app/main.py".to_string(),
                        line: 202,
                        col: 4,
                        end_line: 202,
                        end_col: 7,
                        snippet: "fan".to_string(),
                    },
                    context: "assign:rhs".to_string(),
                },
                Use {
                    use_id: "U_fan_3".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "fan".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan {
                        file: "app/main.py".to_string(),
                        line: 203,
                        col: 4,
                        end_line: 203,
                        end_col: 7,
                        snippet: "fan".to_string(),
                    },
                    context: "assign:rhs".to_string(),
                },
                Use {
                    use_id: "U_fan_4".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "fan".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan {
                        file: "app/main.py".to_string(),
                        line: 204,
                        col: 4,
                        end_line: 204,
                        end_col: 7,
                        snippet: "fan".to_string(),
                    },
                    context: "assign:rhs".to_string(),
                },
                Use {
                    use_id: "U_short".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "short".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan {
                        file: "app/main.py".to_string(),
                        line: 301,
                        col: 4,
                        end_line: 301,
                        end_col: 9,
                        snippet: "short".to_string(),
                    },
                    context: "assign:rhs".to_string(),
                },
            ],
            def_use_edges: vec![
                DefUseEdge {
                    edge_id: "DU_long_1".to_string(),
                    def_id: "D_long_1".to_string(),
                    use_id: "U_long_1".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "long_1".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "long".to_string(),
                },
                DefUseEdge {
                    edge_id: "DU_long_2".to_string(),
                    def_id: "D_long_2".to_string(),
                    use_id: "U_long_2".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "long_2".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "long".to_string(),
                },
                DefUseEdge {
                    edge_id: "DU_long_3".to_string(),
                    def_id: "D_long_3".to_string(),
                    use_id: "U_long_3".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "long_3".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "long".to_string(),
                },
                DefUseEdge {
                    edge_id: "DU_mid_1".to_string(),
                    def_id: "D_mid_1".to_string(),
                    use_id: "U_mid_1".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "mid_1".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "mid".to_string(),
                },
                DefUseEdge {
                    edge_id: "DU_mid_2".to_string(),
                    def_id: "D_mid_2".to_string(),
                    use_id: "U_mid_2".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "mid_2".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "mid".to_string(),
                },
                DefUseEdge {
                    edge_id: "DU_fan_1".to_string(),
                    def_id: "D_fan".to_string(),
                    use_id: "U_fan_1".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "fan".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "fan".to_string(),
                },
                DefUseEdge {
                    edge_id: "DU_fan_2".to_string(),
                    def_id: "D_fan".to_string(),
                    use_id: "U_fan_2".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "fan".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "fan".to_string(),
                },
                DefUseEdge {
                    edge_id: "DU_fan_3".to_string(),
                    def_id: "D_fan".to_string(),
                    use_id: "U_fan_3".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "fan".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "fan".to_string(),
                },
                DefUseEdge {
                    edge_id: "DU_fan_4".to_string(),
                    def_id: "D_fan".to_string(),
                    use_id: "U_fan_4".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "fan".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "fan".to_string(),
                },
                DefUseEdge {
                    edge_id: "DU_short".to_string(),
                    def_id: "D_short".to_string(),
                    use_id: "U_short".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "short".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "short".to_string(),
                },
            ],
            ..AnalysisCache::default()
        };

        write_def_use_hotspots_dot(&cache, &path, 3).unwrap();

        let dot = std::fs::read_to_string(path).unwrap();
        assert!(dot.contains("\"D_long_1\" -> \"U_long_1\" [label=\"local\"]"));
        assert!(dot.contains("\"D_long_2\" -> \"U_long_2\" [label=\"local\"]"));
        assert!(dot.contains("\"D_long_3\" -> \"U_long_3\" [label=\"local\"]"));
        assert!(dot.contains("\"D_mid_1\" -> \"U_mid_1\" [label=\"local\"]"));
        assert!(dot.contains("\"D_mid_2\" -> \"U_mid_2\" [label=\"local\"]"));
        assert!(dot.contains("\"D_fan\" -> \"U_fan_1\" [label=\"local\"]"));
        assert!(!dot.contains("\"D_short\" -> \"U_short\" [label=\"local\"]"));
    }

    #[test]
    fn def_use_hotspot_writer_disambiguates_duplicate_local_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hotspots.dot");
        let span_foo_def = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 10,
            col: 4,
            end_line: 10,
            end_col: 9,
            snippet: "x = 1".to_string(),
        };
        let span_foo_use = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 11,
            col: 10,
            end_line: 11,
            end_col: 11,
            snippet: "print(x)".to_string(),
        };
        let span_bar_def = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 20,
            col: 4,
            end_line: 20,
            end_col: 9,
            snippet: "x = 2".to_string(),
        };
        let span_bar_use = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 21,
            col: 10,
            end_line: 21,
            end_col: 11,
            snippet: "print(x)".to_string(),
        };
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            scopes: vec![
                ScopeRecord {
                    scope_id: "S_foo".to_string(),
                    scope_kind: "function".to_string(),
                    parent_scope_id: None,
                    owner_id: "FN_foo".to_string(),
                    span: span_foo_def.clone(),
                },
                ScopeRecord {
                    scope_id: "S_bar".to_string(),
                    scope_kind: "function".to_string(),
                    parent_scope_id: None,
                    owner_id: "FN_bar".to_string(),
                    span: span_bar_def.clone(),
                },
            ],
            functions: vec![
                FunctionRecord {
                    function_id: "FN_foo".to_string(),
                    module_id: "M_app".to_string(),
                    class_id: None,
                    qualified_name: "foo".to_string(),
                    kind: "function".to_string(),
                    params: Vec::new(),
                    scope_id: "S_foo".to_string(),
                    span: span_foo_def.clone(),
                },
                FunctionRecord {
                    function_id: "FN_bar".to_string(),
                    module_id: "M_app".to_string(),
                    class_id: None,
                    qualified_name: "bar".to_string(),
                    kind: "function".to_string(),
                    params: Vec::new(),
                    scope_id: "S_bar".to_string(),
                    span: span_bar_def.clone(),
                },
            ],
            definitions: vec![
                Definition {
                    def_id: "D_foo_x".to_string(),
                    place: Place::Local {
                        scope_id: "S_foo".to_string(),
                        name: "x".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S_foo".to_string(),
                    function_id: Some("FN_foo".to_string()),
                    span: span_foo_def.clone(),
                    expr: "1".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_bar_x".to_string(),
                    place: Place::Local {
                        scope_id: "S_bar".to_string(),
                        name: "x".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S_bar".to_string(),
                    function_id: Some("FN_bar".to_string()),
                    span: span_bar_def.clone(),
                    expr: "2".to_string(),
                    deps: Vec::new(),
                },
            ],
            uses: vec![
                Use {
                    use_id: "U_foo_x".to_string(),
                    place: Place::Local {
                        scope_id: "S_foo".to_string(),
                        name: "x".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_foo".to_string(),
                    function_id: Some("FN_foo".to_string()),
                    span: span_foo_use.clone(),
                    context: "call".to_string(),
                },
                Use {
                    use_id: "U_bar_x".to_string(),
                    place: Place::Local {
                        scope_id: "S_bar".to_string(),
                        name: "x".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_bar".to_string(),
                    function_id: Some("FN_bar".to_string()),
                    span: span_bar_use.clone(),
                    context: "call".to_string(),
                },
            ],
            def_use_edges: vec![
                DefUseEdge {
                    edge_id: "DU_1".to_string(),
                    def_id: "D_foo_x".to_string(),
                    use_id: "U_foo_x".to_string(),
                    place: Place::Local {
                        scope_id: "S_foo".to_string(),
                        name: "x".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "foo".to_string(),
                },
                DefUseEdge {
                    edge_id: "DU_2".to_string(),
                    def_id: "D_bar_x".to_string(),
                    use_id: "U_bar_x".to_string(),
                    place: Place::Local {
                        scope_id: "S_bar".to_string(),
                        name: "x".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "bar".to_string(),
                },
            ],
            ..AnalysisCache::default()
        };

        write_def_use_hotspots_dot(&cache, &path, 10).unwrap();

        let dot = std::fs::read_to_string(path).unwrap();
        assert!(dot.contains("[label=\"def foo::x @ line 10\"]"));
        assert!(dot.contains("[label=\"use foo::x @ line 11\"]"));
        assert!(dot.contains("[label=\"def bar::x @ line 20\"]"));
        assert!(dot.contains("[label=\"use bar::x @ line 21\"]"));
        assert!(!dot.contains("[label=\"def x\"]"));
        assert!(!dot.contains("[label=\"use x\"]"));
    }

    #[test]
    fn def_use_hotspot_writer_links_only_same_line_defs_to_uses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hotspots.dot");
        let def_x_span = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 10,
            col: 4,
            end_line: 10,
            end_col: 5,
            snippet: "x = y".to_string(),
        };
        let def_y_span = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 10,
            col: 8,
            end_line: 10,
            end_col: 12,
            snippet: "x = y".to_string(),
        };
        let use_x_span = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 10,
            col: 14,
            end_line: 10,
            end_col: 15,
            snippet: "x = y".to_string(),
        };
        let use_y_span = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 10,
            col: 18,
            end_line: 10,
            end_col: 19,
            snippet: "x = y".to_string(),
        };
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            definitions: vec![
                Definition {
                    def_id: "D_x".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "x".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: def_x_span,
                    expr: "y".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_y".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "y".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: def_y_span,
                    expr: "x".to_string(),
                    deps: Vec::new(),
                },
            ],
            uses: vec![
                Use {
                    use_id: "U_x".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "x".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: use_x_span,
                    context: "assign".to_string(),
                },
                Use {
                    use_id: "U_y".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "y".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: use_y_span,
                    context: "assign".to_string(),
                },
            ],
            def_use_edges: vec![
                DefUseEdge {
                    edge_id: "DU_1".to_string(),
                    def_id: "D_x".to_string(),
                    use_id: "U_x".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "x".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "same-line".to_string(),
                },
                DefUseEdge {
                    edge_id: "DU_2".to_string(),
                    def_id: "D_y".to_string(),
                    use_id: "U_y".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "y".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "same-line".to_string(),
                },
            ],
            ..AnalysisCache::default()
        };

        write_def_use_hotspots_dot(&cache, &path, 10).unwrap();

        let dot = std::fs::read_to_string(path).unwrap();
        assert!(
            dot.contains(
                "\"D_x\" -> \"U_y\" [label=\"\", style=dashed, color=\"gray60\", dir=none]"
            )
        );
        assert!(
            dot.contains(
                "\"D_y\" -> \"U_x\" [label=\"\", style=dashed, color=\"gray60\", dir=none]"
            )
        );
        assert!(!dot.contains("[label=\"line app/main.py:10\", shape=ellipse]"));
        assert!(!dot.contains("\"LN_"));
    }

    #[test]
    fn def_use_hotspot_writer_qualifies_local_and_global_labels_with_module() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hotspots.dot");
        let module_span = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 1,
            col: 0,
            end_line: 1,
            end_col: 6,
            snippet: "logger".to_string(),
        };
        let local_def_span = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 10,
            col: 4,
            end_line: 10,
            end_col: 9,
            snippet: "x = 1".to_string(),
        };
        let local_use_span = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 11,
            col: 10,
            end_line: 11,
            end_col: 11,
            snippet: "print(x)".to_string(),
        };
        let global_use_span = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 4,
            col: 6,
            end_line: 4,
            end_col: 12,
            snippet: "logger.info".to_string(),
        };
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            modules: vec![crate::ir::ModuleRecord {
                module_id: "M_app".to_string(),
                file_id: "F_app".to_string(),
                module_name: "app.main".to_string(),
                exports: Vec::new(),
                imports: Vec::new(),
            }],
            scopes: vec![
                ScopeRecord {
                    scope_id: "S_module".to_string(),
                    scope_kind: "module".to_string(),
                    parent_scope_id: None,
                    owner_id: "M_app".to_string(),
                    span: module_span.clone(),
                },
                ScopeRecord {
                    scope_id: "S_fn".to_string(),
                    scope_kind: "function".to_string(),
                    parent_scope_id: Some("S_module".to_string()),
                    owner_id: "FN_handler".to_string(),
                    span: local_def_span.clone(),
                },
            ],
            functions: vec![FunctionRecord {
                function_id: "FN_handler".to_string(),
                module_id: "M_app".to_string(),
                class_id: None,
                qualified_name: "handler".to_string(),
                kind: "function".to_string(),
                params: Vec::new(),
                scope_id: "S_fn".to_string(),
                span: local_def_span.clone(),
            }],
            definitions: vec![
                Definition {
                    def_id: "D_local".to_string(),
                    place: Place::Local {
                        scope_id: "S_fn".to_string(),
                        name: "x".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S_fn".to_string(),
                    function_id: Some("FN_handler".to_string()),
                    span: local_def_span.clone(),
                    expr: "1".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_global".to_string(),
                    place: Place::Global {
                        module_id: "M_app".to_string(),
                        name: "logger".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S_module".to_string(),
                    function_id: None,
                    span: module_span.clone(),
                    expr: "logging.getLogger(__name__)".to_string(),
                    deps: Vec::new(),
                },
            ],
            uses: vec![
                Use {
                    use_id: "U_local".to_string(),
                    place: Place::Local {
                        scope_id: "S_fn".to_string(),
                        name: "x".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_fn".to_string(),
                    function_id: Some("FN_handler".to_string()),
                    span: local_use_span,
                    context: "call".to_string(),
                },
                Use {
                    use_id: "U_global".to_string(),
                    place: Place::Global {
                        module_id: "M_app".to_string(),
                        name: "logger".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_module".to_string(),
                    function_id: None,
                    span: global_use_span,
                    context: "call".to_string(),
                },
            ],
            def_use_edges: vec![
                DefUseEdge {
                    edge_id: "DU_local".to_string(),
                    def_id: "D_local".to_string(),
                    use_id: "U_local".to_string(),
                    place: Place::Local {
                        scope_id: "S_fn".to_string(),
                        name: "x".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "local".to_string(),
                },
                DefUseEdge {
                    edge_id: "DU_global".to_string(),
                    def_id: "D_global".to_string(),
                    use_id: "U_global".to_string(),
                    place: Place::Global {
                        module_id: "M_app".to_string(),
                        name: "logger".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "global".to_string(),
                },
            ],
            ..AnalysisCache::default()
        };

        write_def_use_hotspots_dot(&cache, &path, 10).unwrap();

        let dot = std::fs::read_to_string(path).unwrap();
        assert!(dot.contains("[label=\"def app.main::handler::x @ line 10\"]"));
        assert!(dot.contains("[label=\"use app.main::handler::x @ line 11\"]"));
        assert!(dot.contains("[label=\"def app.main::logger @ line 1\"]"));
        assert!(dot.contains("[label=\"use app.main::logger @ line 4\"]"));
    }

    #[test]
    fn def_use_hotspot_writer_qualifies_attribute_labels_with_scope() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hotspots.dot");
        let function_span = crate::source::SourceSpan {
            file: "routers/tests.py".to_string(),
            line: 209,
            col: 1,
            end_line: 209,
            end_col: 32,
            snippet: "async def export_test_results(...):".to_string(),
        };
        let param_span = crate::source::SourceSpan {
            file: "routers/tests.py".to_string(),
            line: 53,
            col: 1,
            end_line: 53,
            end_col: 32,
            snippet: "def handle_str(result: str) -> str:".to_string(),
        };
        let attr_use_span = crate::source::SourceSpan {
            file: "routers/tests.py".to_string(),
            line: 272,
            col: 49,
            end_line: 272,
            end_col: 69,
            snippet: "result.expected_text".to_string(),
        };
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            modules: vec![
                crate::ir::ModuleRecord {
                    module_id: "M_text".to_string(),
                    file_id: "F_text".to_string(),
                    module_name: "app.utils.text_utils".to_string(),
                    exports: Vec::new(),
                    imports: Vec::new(),
                },
                crate::ir::ModuleRecord {
                    module_id: "M_router".to_string(),
                    file_id: "F_router".to_string(),
                    module_name: "app.routers.tests".to_string(),
                    exports: Vec::new(),
                    imports: Vec::new(),
                },
            ],
            scopes: vec![
                ScopeRecord {
                    scope_id: "S_text_module".to_string(),
                    scope_kind: "module".to_string(),
                    parent_scope_id: None,
                    owner_id: "M_text".to_string(),
                    span: param_span.clone(),
                },
                ScopeRecord {
                    scope_id: "S_handle".to_string(),
                    scope_kind: "function".to_string(),
                    parent_scope_id: Some("S_text_module".to_string()),
                    owner_id: "FN_handle".to_string(),
                    span: param_span.clone(),
                },
                ScopeRecord {
                    scope_id: "S_router_module".to_string(),
                    scope_kind: "module".to_string(),
                    parent_scope_id: None,
                    owner_id: "M_router".to_string(),
                    span: function_span.clone(),
                },
                ScopeRecord {
                    scope_id: "S_export".to_string(),
                    scope_kind: "function".to_string(),
                    parent_scope_id: Some("S_router_module".to_string()),
                    owner_id: "FN_export".to_string(),
                    span: function_span.clone(),
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
                    span: param_span.clone(),
                },
                FunctionRecord {
                    function_id: "FN_export".to_string(),
                    module_id: "M_router".to_string(),
                    class_id: None,
                    qualified_name: "export_test_results".to_string(),
                    kind: "function".to_string(),
                    params: vec!["task_id".to_string(), "db".to_string()],
                    scope_id: "S_export".to_string(),
                    span: function_span.clone(),
                },
            ],
            definitions: vec![Definition {
                def_id: "D_result".to_string(),
                place: Place::Local {
                    scope_id: "S_handle".to_string(),
                    name: "result".to_string(),
                },
                def_kind: "param".to_string(),
                scope_id: "S_handle".to_string(),
                function_id: Some("FN_handle".to_string()),
                span: param_span,
                expr: String::new(),
                deps: Vec::new(),
            }],
            uses: vec![Use {
                use_id: "U_result_expected".to_string(),
                place: Place::Attribute {
                    base: "result".to_string(),
                    attr: "expected_text".to_string(),
                },
                use_kind: "load".to_string(),
                scope_id: "S_export".to_string(),
                function_id: Some("FN_export".to_string()),
                span: attr_use_span,
                context: "assign:rhs".to_string(),
            }],
            def_use_edges: vec![DefUseEdge {
                edge_id: "DU_result_expected".to_string(),
                def_id: "D_result".to_string(),
                use_id: "U_result_expected".to_string(),
                place: Place::Local {
                    scope_id: "S_handle".to_string(),
                    name: "result".to_string(),
                },
                edge_kind: "call-arg".to_string(),
                path_summary: "synthetic".to_string(),
            }],
            ..AnalysisCache::default()
        };

        write_def_use_hotspots_dot(&cache, &path, 10).unwrap();

        let dot = std::fs::read_to_string(path).unwrap();
        assert!(dot.contains(
            "[label=\"use app.routers.tests::export_test_results::result.expected_text @ line 272\"]"
        ));
    }

    #[test]
    fn def_use_hotspot_writer_uses_call_argument_edges_from_var_dependencies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hotspots.dot");
        let module_span = crate::source::SourceSpan {
            file: "utils/text_utils.py".to_string(),
            line: 1,
            col: 0,
            end_line: 1,
            end_col: 4,
            snippet: "text".to_string(),
        };
        let handle_fn_span = crate::source::SourceSpan {
            file: "utils/text_utils.py".to_string(),
            line: 53,
            col: 1,
            end_line: 53,
            end_col: 32,
            snippet: "def handle_str(result: str) -> str:".to_string(),
        };
        let handle_return_span = crate::source::SourceSpan {
            file: "utils/text_utils.py".to_string(),
            line: 59,
            col: 12,
            end_line: 59,
            end_col: 18,
            snippet: "return result".to_string(),
        };
        let calculate_fn_span = crate::source::SourceSpan {
            file: "utils/text_utils.py".to_string(),
            line: 201,
            col: 1,
            end_line: 201,
            end_col: 55,
            snippet: "def calculate_accuracy(expected: str, actual: str) -> float:".to_string(),
        };
        let expected_use_span = crate::source::SourceSpan {
            file: "utils/text_utils.py".to_string(),
            line: 210,
            col: 37,
            end_line: 210,
            end_col: 45,
            snippet: "handle_expected = handle_str(expected)".to_string(),
        };
        let handle_expected_span = crate::source::SourceSpan {
            file: "utils/text_utils.py".to_string(),
            line: 210,
            col: 26,
            end_line: 210,
            end_col: 36,
            snippet: "handle_expected = handle_str(expected)".to_string(),
        };
        let actual_use_span = crate::source::SourceSpan {
            file: "utils/text_utils.py".to_string(),
            line: 211,
            col: 35,
            end_line: 211,
            end_col: 41,
            snippet: "handle_actual = handle_str(actual)".to_string(),
        };
        let handle_actual_span = crate::source::SourceSpan {
            file: "utils/text_utils.py".to_string(),
            line: 211,
            col: 24,
            end_line: 211,
            end_col: 34,
            snippet: "handle_actual = handle_str(actual)".to_string(),
        };
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            modules: vec![crate::ir::ModuleRecord {
                module_id: "M_text".to_string(),
                file_id: "F_text".to_string(),
                module_name: "app.utils.text_utils".to_string(),
                exports: Vec::new(),
                imports: Vec::new(),
            }],
            scopes: vec![
                ScopeRecord {
                    scope_id: "S_module".to_string(),
                    scope_kind: "module".to_string(),
                    parent_scope_id: None,
                    owner_id: "M_text".to_string(),
                    span: module_span,
                },
                ScopeRecord {
                    scope_id: "S_handle".to_string(),
                    scope_kind: "function".to_string(),
                    parent_scope_id: Some("S_module".to_string()),
                    owner_id: "FN_handle".to_string(),
                    span: handle_fn_span.clone(),
                },
                ScopeRecord {
                    scope_id: "S_calc".to_string(),
                    scope_kind: "function".to_string(),
                    parent_scope_id: Some("S_module".to_string()),
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
                    def_id: "D_expected".to_string(),
                    place: Place::Local {
                        scope_id: "S_calc".to_string(),
                        name: "expected".to_string(),
                    },
                    def_kind: "param".to_string(),
                    scope_id: "S_calc".to_string(),
                    function_id: Some("FN_calc".to_string()),
                    span: calculate_fn_span.clone(),
                    expr: String::new(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_actual".to_string(),
                    place: Place::Local {
                        scope_id: "S_calc".to_string(),
                        name: "actual".to_string(),
                    },
                    def_kind: "param".to_string(),
                    scope_id: "S_calc".to_string(),
                    function_id: Some("FN_calc".to_string()),
                    span: calculate_fn_span,
                    expr: String::new(),
                    deps: Vec::new(),
                },
            ],
            uses: vec![
                Use {
                    use_id: "U_result".to_string(),
                    place: Place::Local {
                        scope_id: "S_handle".to_string(),
                        name: "result".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_handle".to_string(),
                    function_id: Some("FN_handle".to_string()),
                    span: handle_return_span,
                    context: "return value".to_string(),
                },
                Use {
                    use_id: "U_handle_expected".to_string(),
                    place: Place::Global {
                        module_id: "M_text".to_string(),
                        name: "handle_str".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_calc".to_string(),
                    function_id: Some("FN_calc".to_string()),
                    span: handle_expected_span,
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
                    span: handle_actual_span,
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
            def_use_edges: vec![
                DefUseEdge {
                    edge_id: "DU_result".to_string(),
                    def_id: "D_result".to_string(),
                    use_id: "U_result".to_string(),
                    place: Place::Local {
                        scope_id: "S_handle".to_string(),
                        name: "result".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "local".to_string(),
                },
                DefUseEdge {
                    edge_id: "DU_expected".to_string(),
                    def_id: "D_expected".to_string(),
                    use_id: "U_expected".to_string(),
                    place: Place::Local {
                        scope_id: "S_calc".to_string(),
                        name: "expected".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "local".to_string(),
                },
                DefUseEdge {
                    edge_id: "DU_actual".to_string(),
                    def_id: "D_actual".to_string(),
                    use_id: "U_actual".to_string(),
                    place: Place::Local {
                        scope_id: "S_calc".to_string(),
                        name: "actual".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "local".to_string(),
                },
            ],
            var_dependency_edges: vec![
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_call_expected".to_string(),
                    source_place: Place::Local {
                        scope_id: "S_calc".to_string(),
                        name: "expected".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S_handle".to_string(),
                        name: "result".to_string(),
                    },
                    source_id: "U_expected".to_string(),
                    target_id: "D_result".to_string(),
                    dep_kind: "call-arg".to_string(),
                    span: crate::source::SourceSpan::synthetic(
                        "utils/text_utils.py",
                        "handle_str(expected)",
                    ),
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_call_actual".to_string(),
                    source_place: Place::Local {
                        scope_id: "S_calc".to_string(),
                        name: "actual".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S_handle".to_string(),
                        name: "result".to_string(),
                    },
                    source_id: "U_actual".to_string(),
                    target_id: "D_result".to_string(),
                    dep_kind: "call-arg".to_string(),
                    span: crate::source::SourceSpan::synthetic(
                        "utils/text_utils.py",
                        "handle_str(actual)",
                    ),
                },
            ],
            ..AnalysisCache::default()
        };

        write_def_use_hotspots_dot(&cache, &path, 10).unwrap();

        let dot = std::fs::read_to_string(path).unwrap();
        assert!(dot.contains("\"D_result\" -> \"U_expected\" [label=\"call-arg\""));
        assert!(dot.contains("\"D_result\" -> \"U_actual\" [label=\"call-arg\""));
    }

    #[test]
    fn var_dependency_writer_keeps_target_bundles_intact_when_truncating() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("variables.dot");
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            definitions: vec![
                Definition {
                    def_id: "D_hot".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "hot".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan::synthetic("app/main.py", "hot = 1"),
                    expr: "1".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_bundle".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "bundle".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan::synthetic("app/main.py", "bundle = 1"),
                    expr: "1".to_string(),
                    deps: Vec::new(),
                },
            ],
            var_dependency_edges: vec![
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_hot_1".to_string(),
                    source_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "hot_a".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "hot".to_string(),
                    },
                    source_id: "U_hot_1".to_string(),
                    target_id: "D_hot".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: crate::source::SourceSpan::synthetic("app/main.py", "hot = hot_a"),
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_hot_2".to_string(),
                    source_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "hot_b".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "hot".to_string(),
                    },
                    source_id: "U_hot_2".to_string(),
                    target_id: "D_hot".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: crate::source::SourceSpan::synthetic("app/main.py", "hot = hot_b"),
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_hot_3".to_string(),
                    source_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "hot_c".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "hot".to_string(),
                    },
                    source_id: "U_hot_3".to_string(),
                    target_id: "D_hot".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: crate::source::SourceSpan::synthetic("app/main.py", "hot = hot_c"),
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_bundle_1".to_string(),
                    source_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "bundle_a".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "bundle".to_string(),
                    },
                    source_id: "U_bundle_1".to_string(),
                    target_id: "D_bundle".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: crate::source::SourceSpan::synthetic("app/main.py", "bundle = bundle_a"),
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_bundle_2".to_string(),
                    source_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "bundle_b".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "bundle".to_string(),
                    },
                    source_id: "U_bundle_2".to_string(),
                    target_id: "D_bundle".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: crate::source::SourceSpan::synthetic("app/main.py", "bundle = bundle_b"),
                },
            ],
            ..AnalysisCache::default()
        };

        write_var_dependency_dot(&cache, &path, 4).unwrap();

        let dot = std::fs::read_to_string(path).unwrap();
        assert!(dot.contains(&format!(
            "\"{}\" -> \"{}\" [label=\"assignment\"]",
            "U_bundle_1", "D_bundle"
        )));
        assert!(dot.contains(&format!(
            "\"{}\" -> \"{}\" [label=\"assignment\"]",
            "U_bundle_2", "D_bundle"
        )));
    }

    #[test]
    fn var_dependency_seed_selection_prioritizes_a_call_arg_bundle() {
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            definitions: vec![
                Definition {
                    def_id: "D_hot".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "hot".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan::synthetic("app/main.py", "hot = 1"),
                    expr: "1".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_call".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "call_target".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan::synthetic("app/main.py", "call_target = f(x)"),
                    expr: "f(x)".to_string(),
                    deps: Vec::new(),
                },
            ],
            var_dependency_edges: vec![
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_hot_1".to_string(),
                    source_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "hot_a".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "hot".to_string(),
                    },
                    source_id: "U_hot_1".to_string(),
                    target_id: "D_hot".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: crate::source::SourceSpan::synthetic("app/main.py", "hot = hot_a"),
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_hot_2".to_string(),
                    source_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "hot_b".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "hot".to_string(),
                    },
                    source_id: "U_hot_2".to_string(),
                    target_id: "D_hot".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: crate::source::SourceSpan::synthetic("app/main.py", "hot = hot_b"),
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_hot_3".to_string(),
                    source_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "hot_c".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "hot".to_string(),
                    },
                    source_id: "U_hot_3".to_string(),
                    target_id: "D_hot".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: crate::source::SourceSpan::synthetic("app/main.py", "hot = hot_c"),
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_call".to_string(),
                    source_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "arg".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "call_target".to_string(),
                    },
                    source_id: "U_arg".to_string(),
                    target_id: "D_call".to_string(),
                    dep_kind: "call-arg".to_string(),
                    span: crate::source::SourceSpan::synthetic(
                        "app/main.py",
                        "call_target = f(arg)",
                    ),
                },
            ],
            ..AnalysisCache::default()
        };

        let selected = select_var_dependency_seed_edges(&cache, 1);

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].target_id, "D_call");
        assert_eq!(selected[0].dep_kind, "call-arg");
    }

    #[test]
    fn var_dependency_seed_selection_interleaves_call_arg_and_assignment_bundles() {
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            definitions: vec![
                Definition {
                    def_id: "D_assign_big".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "assign_big".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan::synthetic("app/main.py", "assign_big = 1"),
                    expr: "1".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_assign_mid".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "assign_mid".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan::synthetic("app/main.py", "assign_mid = 1"),
                    expr: "1".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_call_a".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "call_a".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan::synthetic("app/main.py", "call_a = f(a)"),
                    expr: "f(a)".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_call_b".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "call_b".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan::synthetic("app/main.py", "call_b = f(b)"),
                    expr: "f(b)".to_string(),
                    deps: Vec::new(),
                },
            ],
            var_dependency_edges: vec![
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_big_1".to_string(),
                    source_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "big_1".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "assign_big".to_string(),
                    },
                    source_id: "U_big_1".to_string(),
                    target_id: "D_assign_big".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: crate::source::SourceSpan::synthetic("app/main.py", "assign_big = big_1"),
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_big_2".to_string(),
                    source_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "big_2".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "assign_big".to_string(),
                    },
                    source_id: "U_big_2".to_string(),
                    target_id: "D_assign_big".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: crate::source::SourceSpan::synthetic("app/main.py", "assign_big = big_2"),
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_big_3".to_string(),
                    source_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "big_3".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "assign_big".to_string(),
                    },
                    source_id: "U_big_3".to_string(),
                    target_id: "D_assign_big".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: crate::source::SourceSpan::synthetic("app/main.py", "assign_big = big_3"),
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_mid_1".to_string(),
                    source_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "mid_1".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "assign_mid".to_string(),
                    },
                    source_id: "U_mid_1".to_string(),
                    target_id: "D_assign_mid".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: crate::source::SourceSpan::synthetic("app/main.py", "assign_mid = mid_1"),
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_mid_2".to_string(),
                    source_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "mid_2".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "assign_mid".to_string(),
                    },
                    source_id: "U_mid_2".to_string(),
                    target_id: "D_assign_mid".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: crate::source::SourceSpan::synthetic("app/main.py", "assign_mid = mid_2"),
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_call_a".to_string(),
                    source_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "arg_a".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "call_a".to_string(),
                    },
                    source_id: "U_arg_a".to_string(),
                    target_id: "D_call_a".to_string(),
                    dep_kind: "call-arg".to_string(),
                    span: crate::source::SourceSpan::synthetic("app/main.py", "call_a = f(arg_a)"),
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_call_b".to_string(),
                    source_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "arg_b".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "call_b".to_string(),
                    },
                    source_id: "U_arg_b".to_string(),
                    target_id: "D_call_b".to_string(),
                    dep_kind: "call-arg".to_string(),
                    span: crate::source::SourceSpan::synthetic("app/main.py", "call_b = f(arg_b)"),
                },
            ],
            ..AnalysisCache::default()
        };

        let selected = select_var_dependency_seed_edges(&cache, 20);
        let mut bundle_order = Vec::new();
        for edge in selected {
            if !bundle_order.contains(&edge.target_id) {
                bundle_order.push(edge.target_id.clone());
            }
        }

        assert_eq!(
            bundle_order,
            vec![
                "D_call_a".to_string(),
                "D_assign_big".to_string(),
                "D_call_b".to_string(),
                "D_assign_mid".to_string(),
            ]
        );
    }

    #[test]
    fn var_dependency_seed_selection_counts_unique_visible_edges_per_bundle() {
        let duplicated_edge = crate::ir::VarDependencyEdge {
            edge_id: "VD_dup_1".to_string(),
            source_place: Place::Local {
                scope_id: "S".to_string(),
                name: "dup".to_string(),
            },
            target_place: Place::Local {
                scope_id: "S".to_string(),
                name: "dense".to_string(),
            },
            source_id: "U_dup".to_string(),
            target_id: "D_dense".to_string(),
            dep_kind: "assignment".to_string(),
            span: crate::source::SourceSpan::synthetic("app/main.py", "dense = dup"),
        };
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            definitions: vec![
                Definition {
                    def_id: "D_dense".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "dense".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan::synthetic("app/main.py", "dense = dup"),
                    expr: "dup".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_other".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "other".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan::synthetic("app/main.py", "other = x"),
                    expr: "x".to_string(),
                    deps: Vec::new(),
                },
            ],
            var_dependency_edges: vec![
                duplicated_edge.clone(),
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_dup_2".to_string(),
                    ..duplicated_edge.clone()
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_dup_3".to_string(),
                    ..duplicated_edge.clone()
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_other_1".to_string(),
                    source_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "x".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "other".to_string(),
                    },
                    source_id: "U_x".to_string(),
                    target_id: "D_other".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: crate::source::SourceSpan::synthetic("app/main.py", "other = x"),
                },
            ],
            ..AnalysisCache::default()
        };

        let selected = select_var_dependency_seed_edges(&cache, 2);
        let mut bundle_order = Vec::new();
        for edge in selected {
            if !bundle_order.contains(&edge.target_id) {
                bundle_order.push(edge.target_id.clone());
            }
        }

        assert_eq!(
            bundle_order,
            vec!["D_dense".to_string(), "D_other".to_string()]
        );
    }

    #[test]
    fn var_dependency_seed_selection_keeps_sibling_target_bundles_for_same_source() {
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            definitions: vec![
                Definition {
                    def_id: "D_call".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "result".to_string(),
                    },
                    def_kind: "param".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan::synthetic(
                        "app/main.py",
                        "def handle(result):",
                    ),
                    expr: String::new(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_assign".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "expected_processed".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan::synthetic(
                        "app/main.py",
                        "expected_processed = handle_str(r.expected_text)",
                    ),
                    expr: "handle_str(r.expected_text)".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_big".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "analytics".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: crate::source::SourceSpan::synthetic("app/main.py", "analytics = ..."),
                    expr: "...".to_string(),
                    deps: Vec::new(),
                },
            ],
            var_dependency_edges: vec![
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_call".to_string(),
                    source_place: Place::Attribute {
                        base: "r".to_string(),
                        attr: "expected_text".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "result".to_string(),
                    },
                    source_id: "U_expected_text".to_string(),
                    target_id: "D_call".to_string(),
                    dep_kind: "call-arg".to_string(),
                    span: crate::source::SourceSpan::synthetic("app/main.py", "r.expected_text"),
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_assign".to_string(),
                    source_place: Place::Attribute {
                        base: "r".to_string(),
                        attr: "expected_text".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "expected_processed".to_string(),
                    },
                    source_id: "U_expected_text".to_string(),
                    target_id: "D_assign".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: crate::source::SourceSpan::synthetic("app/main.py", "r.expected_text"),
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_big_1".to_string(),
                    source_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "big_1".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "analytics".to_string(),
                    },
                    source_id: "U_big_1".to_string(),
                    target_id: "D_big".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: crate::source::SourceSpan::synthetic("app/main.py", "analytics = big_1"),
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_big_2".to_string(),
                    source_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "big_2".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "analytics".to_string(),
                    },
                    source_id: "U_big_2".to_string(),
                    target_id: "D_big".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: crate::source::SourceSpan::synthetic("app/main.py", "analytics = big_2"),
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_big_3".to_string(),
                    source_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "big_3".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "analytics".to_string(),
                    },
                    source_id: "U_big_3".to_string(),
                    target_id: "D_big".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: crate::source::SourceSpan::synthetic("app/main.py", "analytics = big_3"),
                },
            ],
            ..AnalysisCache::default()
        };

        let selected = select_var_dependency_seed_edges(&cache, 3);
        let selected_target_ids = selected
            .iter()
            .map(|edge| edge.target_id.clone())
            .collect::<BTreeSet<_>>();

        assert!(selected_target_ids.contains("D_call"));
        assert!(selected_target_ids.contains("D_assign"));
    }

    #[test]
    fn var_dependency_seed_selection_closes_over_def_use_driven_downstream_bundles() {
        let processed_def_span = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 469,
            col: 13,
            end_line: 469,
            end_col: 38,
            snippet: "processed_text = raw_text".to_string(),
        };
        let actual_def_span = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 471,
            col: 13,
            end_line: 471,
            end_col: 61,
            snippet: "self.current_result.actual_text = processed_text".to_string(),
        };
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            definitions: vec![
                Definition {
                    def_id: "D_processed".to_string(),
                    place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "processed_text".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: processed_def_span.clone(),
                    expr: "raw_text".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_actual".to_string(),
                    place: Place::Attribute {
                        base: "*".to_string(),
                        attr: "actual_text".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S".to_string(),
                    function_id: None,
                    span: actual_def_span.clone(),
                    expr: "processed_text".to_string(),
                    deps: Vec::new(),
                },
            ],
            def_use_edges: vec![DefUseEdge {
                edge_id: "DU_processed".to_string(),
                def_id: "D_processed".to_string(),
                use_id: "U_processed".to_string(),
                place: Place::Local {
                    scope_id: "S".to_string(),
                    name: "processed_text".to_string(),
                },
                edge_kind: "local".to_string(),
                path_summary: "local".to_string(),
            }],
            var_dependency_edges: vec![
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_processed".to_string(),
                    source_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "raw_text".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "processed_text".to_string(),
                    },
                    source_id: "U_raw".to_string(),
                    target_id: "D_processed".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: processed_def_span,
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_actual".to_string(),
                    source_place: Place::Local {
                        scope_id: "S".to_string(),
                        name: "processed_text".to_string(),
                    },
                    target_place: Place::Attribute {
                        base: "*".to_string(),
                        attr: "actual_text".to_string(),
                    },
                    source_id: "U_processed".to_string(),
                    target_id: "D_actual".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: actual_def_span,
                },
            ],
            ..AnalysisCache::default()
        };

        let selected = select_var_dependency_seed_edges(&cache, 1);
        let selected_target_ids = selected
            .iter()
            .map(|edge| edge.target_id.clone())
            .collect::<BTreeSet<_>>();

        assert!(selected_target_ids.contains("D_processed"));
        assert!(selected_target_ids.contains("D_actual"));
    }

    #[test]
    fn var_dependency_writer_uses_def_use_labels_and_call_argument_edges() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("variables.dot");
        let handle_fn_span = crate::source::SourceSpan {
            file: "utils/text_utils.py".to_string(),
            line: 53,
            col: 1,
            end_line: 53,
            end_col: 32,
            snippet: "def handle_str(result: str) -> str:".to_string(),
        };
        let calculate_fn_span = crate::source::SourceSpan {
            file: "utils/text_utils.py".to_string(),
            line: 201,
            col: 1,
            end_line: 201,
            end_col: 55,
            snippet: "def calculate_accuracy(expected: str, actual: str) -> float:".to_string(),
        };
        let expected_use_span = crate::source::SourceSpan {
            file: "utils/text_utils.py".to_string(),
            line: 210,
            col: 37,
            end_line: 210,
            end_col: 45,
            snippet: "handle_expected = handle_str(expected)".to_string(),
        };
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            modules: vec![crate::ir::ModuleRecord {
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
                    params: vec!["expected".to_string()],
                    scope_id: "S_calc".to_string(),
                    span: calculate_fn_span.clone(),
                },
            ],
            definitions: vec![Definition {
                def_id: "D_result".to_string(),
                place: Place::Local {
                    scope_id: "S_handle".to_string(),
                    name: "result".to_string(),
                },
                def_kind: "param".to_string(),
                scope_id: "S_handle".to_string(),
                function_id: Some("FN_handle".to_string()),
                span: handle_fn_span.clone(),
                expr: String::new(),
                deps: Vec::new(),
            }],
            uses: vec![Use {
                use_id: "U_expected".to_string(),
                place: Place::Local {
                    scope_id: "S_calc".to_string(),
                    name: "expected".to_string(),
                },
                use_kind: "load".to_string(),
                scope_id: "S_calc".to_string(),
                function_id: Some("FN_calc".to_string()),
                span: expected_use_span.clone(),
                context: "assign:rhs".to_string(),
            }],
            var_dependency_edges: vec![crate::ir::VarDependencyEdge {
                edge_id: "VD_1".to_string(),
                source_place: Place::Local {
                    scope_id: "S_calc".to_string(),
                    name: "expected".to_string(),
                },
                target_place: Place::Local {
                    scope_id: "S_handle".to_string(),
                    name: "result".to_string(),
                },
                source_id: "U_expected".to_string(),
                target_id: "D_result".to_string(),
                dep_kind: "call-arg".to_string(),
                span: expected_use_span,
            }],
            ..AnalysisCache::default()
        };

        write_var_dependency_dot(&cache, &path, 10).unwrap();

        let dot = std::fs::read_to_string(path).unwrap();
        assert!(dot.contains(
            "[label=\"use app.utils.text_utils::calculate_accuracy::expected @ line 210\"]"
        ));
        assert!(dot.contains("[label=\"def app.utils.text_utils::handle_str::result @ line 53\"]"));
        assert!(dot.contains("[label=\"call-arg\", style=dashed, color=\"steelblue3\"]"));
    }

    #[test]
    fn var_dependency_writer_includes_def_use_edges_for_selected_defs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("variables.dot");
        let def_span = crate::source::SourceSpan {
            file: "asr_client.py".to_string(),
            line: 469,
            col: 13,
            end_line: 469,
            end_col: 50,
            snippet: "processed_text = handle_str(raw_text)".to_string(),
        };
        let use_span = crate::source::SourceSpan {
            file: "asr_client.py".to_string(),
            line: 471,
            col: 47,
            end_line: 471,
            end_col: 61,
            snippet: "self.current_result.actual_text = processed_text".to_string(),
        };
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            modules: vec![crate::ir::ModuleRecord {
                module_id: "M_asr".to_string(),
                file_id: "F_asr".to_string(),
                module_name: "app.asr_client".to_string(),
                exports: Vec::new(),
                imports: Vec::new(),
            }],
            scopes: vec![ScopeRecord {
                scope_id: "S_final".to_string(),
                scope_kind: "function".to_string(),
                parent_scope_id: None,
                owner_id: "FN_final".to_string(),
                span: def_span.clone(),
            }],
            functions: vec![FunctionRecord {
                function_id: "FN_final".to_string(),
                module_id: "M_asr".to_string(),
                class_id: None,
                qualified_name: "ASRClient._handle_final_result".to_string(),
                kind: "method".to_string(),
                params: vec!["self".to_string(), "raw_text".to_string()],
                scope_id: "S_final".to_string(),
                span: def_span.clone(),
            }],
            definitions: vec![Definition {
                def_id: "D_processed".to_string(),
                place: Place::Local {
                    scope_id: "S_final".to_string(),
                    name: "processed_text".to_string(),
                },
                def_kind: "assign".to_string(),
                scope_id: "S_final".to_string(),
                function_id: Some("FN_final".to_string()),
                span: def_span.clone(),
                expr: "handle_str(raw_text)".to_string(),
                deps: Vec::new(),
            }],
            uses: vec![
                Use {
                    use_id: "U_raw_text".to_string(),
                    place: Place::Local {
                        scope_id: "S_final".to_string(),
                        name: "raw_text".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_final".to_string(),
                    function_id: Some("FN_final".to_string()),
                    span: def_span.clone(),
                    context: "assign:rhs".to_string(),
                },
                Use {
                    use_id: "U_processed".to_string(),
                    place: Place::Local {
                        scope_id: "S_final".to_string(),
                        name: "processed_text".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_final".to_string(),
                    function_id: Some("FN_final".to_string()),
                    span: use_span,
                    context: "assign:rhs".to_string(),
                },
            ],
            def_use_edges: vec![DefUseEdge {
                edge_id: "DU_processed".to_string(),
                def_id: "D_processed".to_string(),
                use_id: "U_processed".to_string(),
                place: Place::Local {
                    scope_id: "S_final".to_string(),
                    name: "processed_text".to_string(),
                },
                edge_kind: "local".to_string(),
                path_summary: "local".to_string(),
            }],
            var_dependency_edges: vec![crate::ir::VarDependencyEdge {
                edge_id: "VD_processed".to_string(),
                source_place: Place::Local {
                    scope_id: "S_final".to_string(),
                    name: "raw_text".to_string(),
                },
                target_place: Place::Local {
                    scope_id: "S_final".to_string(),
                    name: "processed_text".to_string(),
                },
                source_id: "U_raw_text".to_string(),
                target_id: "D_processed".to_string(),
                dep_kind: "assignment".to_string(),
                span: def_span,
            }],
            ..AnalysisCache::default()
        };

        write_var_dependency_dot(&cache, &path, 1).unwrap();

        let dot = std::fs::read_to_string(path).unwrap();
        assert!(
            dot.contains(
                "[label=\"use app.asr_client::ASRClient._handle_final_result::processed_text @ line 471\"]"
            )
        );
        assert!(dot.contains("\"D_processed\" -> \"U_processed\" [label=\"def-use\""));
    }

    #[test]
    fn var_dependency_writer_walks_root_defs_through_same_line_definitions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("variables.dot");
        let function_span = crate::source::SourceSpan {
            file: "asr_client.py".to_string(),
            line: 462,
            col: 5,
            end_line: 483,
            end_col: 58,
            snippet: "async def _handle_final_result(self, data):".to_string(),
        };
        let raw_def_span = crate::source::SourceSpan {
            file: "asr_client.py".to_string(),
            line: 462,
            col: 36,
            end_line: 462,
            end_col: 44,
            snippet: "raw_text".to_string(),
        };
        let processed_def_span = crate::source::SourceSpan {
            file: "asr_client.py".to_string(),
            line: 469,
            col: 13,
            end_line: 469,
            end_col: 50,
            snippet: "processed_text = handle_str(raw_text)".to_string(),
        };
        let actual_def_span = crate::source::SourceSpan {
            file: "asr_client.py".to_string(),
            line: 471,
            col: 13,
            end_line: 471,
            end_col: 61,
            snippet: "self.current_result.actual_text = processed_text".to_string(),
        };
        let actual_use_span = crate::source::SourceSpan {
            file: "asr_client.py".to_string(),
            line: 477,
            col: 36,
            end_line: 477,
            end_col: 67,
            snippet: "self.current_result.actual_text".to_string(),
        };
        let list_use_span = crate::source::SourceSpan {
            file: "asr_client.py".to_string(),
            line: 477,
            col: 9,
            end_line: 477,
            end_col: 28,
            snippet: "self.real_text_list".to_string(),
        };
        let list_def_span = crate::source::SourceSpan {
            file: "asr_client.py".to_string(),
            line: 477,
            col: 9,
            end_line: 477,
            end_col: 68,
            snippet: "self.real_text_list = <mutated by append>".to_string(),
        };
        let list_after_use_span = crate::source::SourceSpan {
            file: "asr_client.py".to_string(),
            line: 478,
            col: 22,
            end_line: 478,
            end_col: 41,
            snippet: "self.real_text_list".to_string(),
        };
        let buffer_def_span = crate::source::SourceSpan {
            file: "asr_client.py".to_string(),
            line: 478,
            col: 13,
            end_line: 478,
            end_col: 41,
            snippet: "buffer = self.real_text_list".to_string(),
        };
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            modules: vec![crate::ir::ModuleRecord {
                module_id: "M_asr".to_string(),
                file_id: "F_asr".to_string(),
                module_name: "app.asr_client".to_string(),
                exports: Vec::new(),
                imports: Vec::new(),
            }],
            scopes: vec![ScopeRecord {
                scope_id: "S_final".to_string(),
                scope_kind: "function".to_string(),
                parent_scope_id: None,
                owner_id: "FN_final".to_string(),
                span: function_span.clone(),
            }],
            functions: vec![FunctionRecord {
                function_id: "FN_final".to_string(),
                module_id: "M_asr".to_string(),
                class_id: None,
                qualified_name: "ASRClient._handle_final_result".to_string(),
                kind: "method".to_string(),
                params: vec!["self".to_string(), "raw_text".to_string()],
                scope_id: "S_final".to_string(),
                span: function_span,
            }],
            definitions: vec![
                Definition {
                    def_id: "D_raw".to_string(),
                    place: Place::Local {
                        scope_id: "S_final".to_string(),
                        name: "raw_text".to_string(),
                    },
                    def_kind: "param".to_string(),
                    scope_id: "S_final".to_string(),
                    function_id: Some("FN_final".to_string()),
                    span: raw_def_span,
                    expr: String::new(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_processed".to_string(),
                    place: Place::Local {
                        scope_id: "S_final".to_string(),
                        name: "processed_text".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S_final".to_string(),
                    function_id: Some("FN_final".to_string()),
                    span: processed_def_span.clone(),
                    expr: "handle_str(raw_text)".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_actual".to_string(),
                    place: Place::Attribute {
                        base: "*".to_string(),
                        attr: "actual_text".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S_final".to_string(),
                    function_id: Some("FN_final".to_string()),
                    span: actual_def_span.clone(),
                    expr: "processed_text".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_real_text_list".to_string(),
                    place: Place::Attribute {
                        base: "InstanceField(ASRClient)".to_string(),
                        attr: "real_text_list".to_string(),
                    },
                    def_kind: "mut-call".to_string(),
                    scope_id: "S_final".to_string(),
                    function_id: Some("FN_final".to_string()),
                    span: list_def_span,
                    expr: "self.real_text_list.append(self.current_result.actual_text)".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_buffer".to_string(),
                    place: Place::Local {
                        scope_id: "S_final".to_string(),
                        name: "buffer".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S_final".to_string(),
                    function_id: Some("FN_final".to_string()),
                    span: buffer_def_span.clone(),
                    expr: "self.real_text_list".to_string(),
                    deps: Vec::new(),
                },
            ],
            uses: vec![
                Use {
                    use_id: "U_raw".to_string(),
                    place: Place::Local {
                        scope_id: "S_final".to_string(),
                        name: "raw_text".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_final".to_string(),
                    function_id: Some("FN_final".to_string()),
                    span: processed_def_span.clone(),
                    context: "assign:rhs".to_string(),
                },
                Use {
                    use_id: "U_processed".to_string(),
                    place: Place::Local {
                        scope_id: "S_final".to_string(),
                        name: "processed_text".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_final".to_string(),
                    function_id: Some("FN_final".to_string()),
                    span: crate::source::SourceSpan {
                        file: "asr_client.py".to_string(),
                        line: 471,
                        col: 47,
                        end_line: 471,
                        end_col: 61,
                        snippet: "processed_text".to_string(),
                    },
                    context: "assign:rhs".to_string(),
                },
                Use {
                    use_id: "U_actual".to_string(),
                    place: Place::Attribute {
                        base: "*".to_string(),
                        attr: "actual_text".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_final".to_string(),
                    function_id: Some("FN_final".to_string()),
                    span: actual_use_span,
                    context: "expr:statement".to_string(),
                },
                Use {
                    use_id: "U_real_text_list".to_string(),
                    place: Place::Attribute {
                        base: "InstanceField(ASRClient)".to_string(),
                        attr: "real_text_list".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_final".to_string(),
                    function_id: Some("FN_final".to_string()),
                    span: list_use_span.clone(),
                    context: "expr:statement".to_string(),
                },
                Use {
                    use_id: "U_real_text_list_after".to_string(),
                    place: Place::Attribute {
                        base: "InstanceField(ASRClient)".to_string(),
                        attr: "real_text_list".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_final".to_string(),
                    function_id: Some("FN_final".to_string()),
                    span: list_after_use_span,
                    context: "assign:rhs".to_string(),
                },
            ],
            def_use_edges: vec![
                DefUseEdge {
                    edge_id: "DU_raw".to_string(),
                    def_id: "D_raw".to_string(),
                    use_id: "U_raw".to_string(),
                    place: Place::Local {
                        scope_id: "S_final".to_string(),
                        name: "raw_text".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "local".to_string(),
                },
                DefUseEdge {
                    edge_id: "DU_processed".to_string(),
                    def_id: "D_processed".to_string(),
                    use_id: "U_processed".to_string(),
                    place: Place::Local {
                        scope_id: "S_final".to_string(),
                        name: "processed_text".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "local".to_string(),
                },
                DefUseEdge {
                    edge_id: "DU_actual".to_string(),
                    def_id: "D_actual".to_string(),
                    use_id: "U_actual".to_string(),
                    place: Place::Attribute {
                        base: "*".to_string(),
                        attr: "actual_text".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "local".to_string(),
                },
                DefUseEdge {
                    edge_id: "DU_real_text_list".to_string(),
                    def_id: "D_real_text_list".to_string(),
                    use_id: "U_real_text_list_after".to_string(),
                    place: Place::Attribute {
                        base: "InstanceField(ASRClient)".to_string(),
                        attr: "real_text_list".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "local".to_string(),
                },
            ],
            var_dependency_edges: vec![
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_processed".to_string(),
                    source_place: Place::Local {
                        scope_id: "S_final".to_string(),
                        name: "raw_text".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S_final".to_string(),
                        name: "processed_text".to_string(),
                    },
                    source_id: "U_raw".to_string(),
                    target_id: "D_processed".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: processed_def_span,
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_actual".to_string(),
                    source_place: Place::Local {
                        scope_id: "S_final".to_string(),
                        name: "processed_text".to_string(),
                    },
                    target_place: Place::Attribute {
                        base: "*".to_string(),
                        attr: "actual_text".to_string(),
                    },
                    source_id: "U_processed".to_string(),
                    target_id: "D_actual".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: actual_def_span,
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_real_text_list_actual".to_string(),
                    source_place: Place::Attribute {
                        base: "*".to_string(),
                        attr: "actual_text".to_string(),
                    },
                    target_place: Place::Attribute {
                        base: "InstanceField(ASRClient)".to_string(),
                        attr: "real_text_list".to_string(),
                    },
                    source_id: "U_actual".to_string(),
                    target_id: "D_real_text_list".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: list_use_span.clone(),
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_real_text_list_receiver".to_string(),
                    source_place: Place::Attribute {
                        base: "InstanceField(ASRClient)".to_string(),
                        attr: "real_text_list".to_string(),
                    },
                    target_place: Place::Attribute {
                        base: "InstanceField(ASRClient)".to_string(),
                        attr: "real_text_list".to_string(),
                    },
                    source_id: "U_real_text_list".to_string(),
                    target_id: "D_real_text_list".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: list_use_span,
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_buffer".to_string(),
                    source_place: Place::Attribute {
                        base: "InstanceField(ASRClient)".to_string(),
                        attr: "real_text_list".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S_final".to_string(),
                        name: "buffer".to_string(),
                    },
                    source_id: "U_real_text_list_after".to_string(),
                    target_id: "D_buffer".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: buffer_def_span,
                },
            ],
            ..AnalysisCache::default()
        };

        write_var_dependency_dot(&cache, &path, 1).unwrap();

        let dot = std::fs::read_to_string(path).unwrap();
        assert!(dot.contains(
            "[label=\"def app.asr_client::ASRClient._handle_final_result::self.real_text_list @ line 477\"]"
        ));
        assert!(dot.contains(
            "[label=\"def app.asr_client::ASRClient._handle_final_result::buffer @ line 478\"]"
        ));
        assert!(dot.contains("\"U_actual\" -> \"D_real_text_list\" [label=\"assignment\"]"));
        assert!(
            dot.contains("\"D_real_text_list\" -> \"U_real_text_list_after\" [label=\"def-use\"")
        );
        assert!(dot.contains("\"U_real_text_list_after\" -> \"D_buffer\" [label=\"assignment\"]"));
    }

    #[test]
    fn var_dependency_writer_disambiguates_duplicate_local_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("variables.dot");
        let foo_use_span = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 10,
            col: 4,
            end_line: 10,
            end_col: 5,
            snippet: "left = x".to_string(),
        };
        let bar_use_span = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 20,
            col: 4,
            end_line: 20,
            end_col: 5,
            snippet: "right = x".to_string(),
        };
        let foo_def_span = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 10,
            col: 0,
            end_line: 10,
            end_col: 8,
            snippet: "left = x".to_string(),
        };
        let bar_def_span = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 20,
            col: 0,
            end_line: 20,
            end_col: 9,
            snippet: "right = x".to_string(),
        };
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            modules: vec![crate::ir::ModuleRecord {
                module_id: "M_app".to_string(),
                file_id: "F_app".to_string(),
                module_name: "app".to_string(),
                exports: Vec::new(),
                imports: Vec::new(),
            }],
            scopes: vec![
                ScopeRecord {
                    scope_id: "S_foo".to_string(),
                    scope_kind: "function".to_string(),
                    parent_scope_id: None,
                    owner_id: "FN_foo".to_string(),
                    span: foo_def_span.clone(),
                },
                ScopeRecord {
                    scope_id: "S_bar".to_string(),
                    scope_kind: "function".to_string(),
                    parent_scope_id: None,
                    owner_id: "FN_bar".to_string(),
                    span: bar_def_span.clone(),
                },
            ],
            functions: vec![
                FunctionRecord {
                    function_id: "FN_foo".to_string(),
                    module_id: "M_app".to_string(),
                    class_id: None,
                    qualified_name: "foo".to_string(),
                    kind: "function".to_string(),
                    params: Vec::new(),
                    scope_id: "S_foo".to_string(),
                    span: foo_def_span.clone(),
                },
                FunctionRecord {
                    function_id: "FN_bar".to_string(),
                    module_id: "M_app".to_string(),
                    class_id: None,
                    qualified_name: "bar".to_string(),
                    kind: "function".to_string(),
                    params: Vec::new(),
                    scope_id: "S_bar".to_string(),
                    span: bar_def_span.clone(),
                },
            ],
            definitions: vec![
                Definition {
                    def_id: "D_left".to_string(),
                    place: Place::Attribute {
                        base: "self".to_string(),
                        attr: "left".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S_foo".to_string(),
                    function_id: Some("FN_foo".to_string()),
                    span: foo_def_span.clone(),
                    expr: "x".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_right".to_string(),
                    place: Place::Attribute {
                        base: "self".to_string(),
                        attr: "right".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S_bar".to_string(),
                    function_id: Some("FN_bar".to_string()),
                    span: bar_def_span.clone(),
                    expr: "x".to_string(),
                    deps: Vec::new(),
                },
            ],
            uses: vec![
                Use {
                    use_id: "U_foo_x".to_string(),
                    place: Place::Local {
                        scope_id: "S_foo".to_string(),
                        name: "x".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_foo".to_string(),
                    function_id: Some("FN_foo".to_string()),
                    span: foo_use_span.clone(),
                    context: "assign:rhs".to_string(),
                },
                Use {
                    use_id: "U_bar_x".to_string(),
                    place: Place::Local {
                        scope_id: "S_bar".to_string(),
                        name: "x".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_bar".to_string(),
                    function_id: Some("FN_bar".to_string()),
                    span: bar_use_span.clone(),
                    context: "assign:rhs".to_string(),
                },
            ],
            var_dependency_edges: vec![
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_1".to_string(),
                    source_place: Place::Local {
                        scope_id: "S_foo".to_string(),
                        name: "x".to_string(),
                    },
                    target_place: Place::Attribute {
                        base: "self".to_string(),
                        attr: "left".to_string(),
                    },
                    source_id: "U_foo_x".to_string(),
                    target_id: "D_left".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: foo_use_span,
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_2".to_string(),
                    source_place: Place::Local {
                        scope_id: "S_bar".to_string(),
                        name: "x".to_string(),
                    },
                    target_place: Place::Attribute {
                        base: "self".to_string(),
                        attr: "right".to_string(),
                    },
                    source_id: "U_bar_x".to_string(),
                    target_id: "D_right".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: bar_use_span,
                },
            ],
            ..AnalysisCache::default()
        };

        write_var_dependency_dot(&cache, &path, 10).unwrap();

        let dot = std::fs::read_to_string(path).unwrap();
        assert!(dot.contains("[label=\"use app::foo::x @ line 10\"]"));
        assert!(dot.contains("[label=\"use app::bar::x @ line 20\"]"));
        assert!(!dot.contains("[label=\"use x @ line ?\"]"));
    }

    #[test]
    fn var_dependency_writer_uses_real_target_text_for_attribute_and_subscript_definitions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("variables.dot");
        let function_span = crate::source::SourceSpan {
            file: "asr_client.py".to_string(),
            line: 460,
            col: 1,
            end_line: 460,
            end_col: 40,
            snippet: "def _handle_final_result(self):".to_string(),
        };
        let attr_def_span = crate::source::SourceSpan {
            file: "asr_client.py".to_string(),
            line: 471,
            col: 13,
            end_line: 471,
            end_col: 61,
            snippet: "self.current_result.actual_text = processed_text".to_string(),
        };
        let subscript_def_span = crate::source::SourceSpan {
            file: "asr_client.py".to_string(),
            line: 472,
            col: 13,
            end_line: 472,
            end_col: 62,
            snippet: "self.partial_results[result_id] = processed_payload".to_string(),
        };
        let processed_use_span = crate::source::SourceSpan {
            file: "asr_client.py".to_string(),
            line: 471,
            col: 47,
            end_line: 471,
            end_col: 61,
            snippet: "processed_text".to_string(),
        };
        let payload_use_span = crate::source::SourceSpan {
            file: "asr_client.py".to_string(),
            line: 472,
            col: 38,
            end_line: 472,
            end_col: 55,
            snippet: "processed_payload".to_string(),
        };
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            modules: vec![crate::ir::ModuleRecord {
                module_id: "M_asr".to_string(),
                file_id: "F_asr".to_string(),
                module_name: "app.asr_client".to_string(),
                exports: Vec::new(),
                imports: Vec::new(),
            }],
            scopes: vec![
                ScopeRecord {
                    scope_id: "S_asr_module".to_string(),
                    scope_kind: "module".to_string(),
                    parent_scope_id: None,
                    owner_id: "M_asr".to_string(),
                    span: function_span.clone(),
                },
                ScopeRecord {
                    scope_id: "S_final".to_string(),
                    scope_kind: "function".to_string(),
                    parent_scope_id: Some("S_asr_module".to_string()),
                    owner_id: "FN_final".to_string(),
                    span: function_span.clone(),
                },
            ],
            functions: vec![FunctionRecord {
                function_id: "FN_final".to_string(),
                module_id: "M_asr".to_string(),
                class_id: None,
                qualified_name: "ASRClient._handle_final_result".to_string(),
                kind: "method".to_string(),
                params: vec!["self".to_string()],
                scope_id: "S_final".to_string(),
                span: function_span,
            }],
            definitions: vec![
                Definition {
                    def_id: "D_actual_text".to_string(),
                    place: Place::Attribute {
                        base: "*".to_string(),
                        attr: "actual_text".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S_final".to_string(),
                    function_id: Some("FN_final".to_string()),
                    span: attr_def_span.clone(),
                    expr: "processed_text".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_partial_result".to_string(),
                    place: Place::Subscript {
                        base: "*".to_string(),
                        index: "*".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S_final".to_string(),
                    function_id: Some("FN_final".to_string()),
                    span: subscript_def_span.clone(),
                    expr: "processed_payload".to_string(),
                    deps: Vec::new(),
                },
            ],
            uses: vec![
                Use {
                    use_id: "U_processed_text".to_string(),
                    place: Place::Local {
                        scope_id: "S_final".to_string(),
                        name: "processed_text".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_final".to_string(),
                    function_id: Some("FN_final".to_string()),
                    span: processed_use_span.clone(),
                    context: "assign:rhs".to_string(),
                },
                Use {
                    use_id: "U_processed_payload".to_string(),
                    place: Place::Local {
                        scope_id: "S_final".to_string(),
                        name: "processed_payload".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_final".to_string(),
                    function_id: Some("FN_final".to_string()),
                    span: payload_use_span.clone(),
                    context: "assign:rhs".to_string(),
                },
            ],
            var_dependency_edges: vec![
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_actual_text".to_string(),
                    source_place: Place::Local {
                        scope_id: "S_final".to_string(),
                        name: "processed_text".to_string(),
                    },
                    target_place: Place::Attribute {
                        base: "*".to_string(),
                        attr: "actual_text".to_string(),
                    },
                    source_id: "U_processed_text".to_string(),
                    target_id: "D_actual_text".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: processed_use_span,
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_partial_result".to_string(),
                    source_place: Place::Local {
                        scope_id: "S_final".to_string(),
                        name: "processed_payload".to_string(),
                    },
                    target_place: Place::Subscript {
                        base: "*".to_string(),
                        index: "*".to_string(),
                    },
                    source_id: "U_processed_payload".to_string(),
                    target_id: "D_partial_result".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: payload_use_span,
                },
            ],
            ..AnalysisCache::default()
        };

        write_var_dependency_dot(&cache, &path, 10).unwrap();

        let dot = std::fs::read_to_string(path).unwrap();
        assert!(dot.contains(
            "[label=\"def app.asr_client::ASRClient._handle_final_result::self.current_result.actual_text @ line 471\"]"
        ));
        assert!(dot.contains(
            "[label=\"def app.asr_client::ASRClient._handle_final_result::self.partial_results[result_id] @ line 472\"]"
        ));
    }

    #[test]
    fn var_dependency_writer_uses_real_source_text_for_attribute_and_subscript_uses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("variables.dot");
        let function_span = crate::source::SourceSpan {
            file: "reports.py".to_string(),
            line: 200,
            col: 1,
            end_line: 200,
            end_col: 36,
            snippet: "def get_task_analytics(task):".to_string(),
        };
        let attr_use_span = crate::source::SourceSpan {
            file: "reports.py".to_string(),
            line: 213,
            col: 30,
            end_line: 213,
            end_col: 58,
            snippet: "task.completed_at.isoformat".to_string(),
        };
        let subscript_use_span = crate::source::SourceSpan {
            file: "reports.py".to_string(),
            line: 214,
            col: 22,
            end_line: 214,
            end_col: 52,
            snippet: "payload[task.result_key]".to_string(),
        };
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            modules: vec![crate::ir::ModuleRecord {
                module_id: "M_reports".to_string(),
                file_id: "F_reports".to_string(),
                module_name: "app.routers.reports".to_string(),
                exports: Vec::new(),
                imports: Vec::new(),
            }],
            scopes: vec![
                ScopeRecord {
                    scope_id: "S_reports_module".to_string(),
                    scope_kind: "module".to_string(),
                    parent_scope_id: None,
                    owner_id: "M_reports".to_string(),
                    span: function_span.clone(),
                },
                ScopeRecord {
                    scope_id: "S_analytics".to_string(),
                    scope_kind: "function".to_string(),
                    parent_scope_id: Some("S_reports_module".to_string()),
                    owner_id: "FN_analytics".to_string(),
                    span: function_span.clone(),
                },
            ],
            functions: vec![FunctionRecord {
                function_id: "FN_analytics".to_string(),
                module_id: "M_reports".to_string(),
                class_id: None,
                qualified_name: "get_task_analytics".to_string(),
                kind: "function".to_string(),
                params: vec!["task".to_string()],
                scope_id: "S_analytics".to_string(),
                span: function_span,
            }],
            definitions: vec![
                Definition {
                    def_id: "D_completed".to_string(),
                    place: Place::Local {
                        scope_id: "S_analytics".to_string(),
                        name: "completed".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S_analytics".to_string(),
                    function_id: Some("FN_analytics".to_string()),
                    span: crate::source::SourceSpan::synthetic(
                        "reports.py",
                        "completed = task.completed_at.isoformat()",
                    ),
                    expr: "task.completed_at.isoformat()".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_selected".to_string(),
                    place: Place::Local {
                        scope_id: "S_analytics".to_string(),
                        name: "selected".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S_analytics".to_string(),
                    function_id: Some("FN_analytics".to_string()),
                    span: crate::source::SourceSpan {
                        file: "reports.py".to_string(),
                        line: 214,
                        col: 1,
                        end_line: 214,
                        end_col: 37,
                        snippet: "selected = payload[task.result_key]".to_string(),
                    },
                    expr: "payload[task.result_key]".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_marker".to_string(),
                    place: Place::Local {
                        scope_id: "S_analytics".to_string(),
                        name: "marker".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S_analytics".to_string(),
                    function_id: Some("FN_analytics".to_string()),
                    span: crate::source::SourceSpan::synthetic("reports.py", "marker = True"),
                    expr: "True".to_string(),
                    deps: Vec::new(),
                },
            ],
            uses: vec![
                Use {
                    use_id: "U_isoformat".to_string(),
                    place: Place::Attribute {
                        base: "*".to_string(),
                        attr: "isoformat".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_analytics".to_string(),
                    function_id: Some("FN_analytics".to_string()),
                    span: attr_use_span.clone(),
                    context: "call:candidate".to_string(),
                },
                Use {
                    use_id: "U_payload_key".to_string(),
                    place: Place::Subscript {
                        base: "*".to_string(),
                        index: "*".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_analytics".to_string(),
                    function_id: Some("FN_analytics".to_string()),
                    span: subscript_use_span.clone(),
                    context: "assign:rhs".to_string(),
                },
                Use {
                    use_id: "U_marker".to_string(),
                    place: Place::Local {
                        scope_id: "S_analytics".to_string(),
                        name: "marker".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_analytics".to_string(),
                    function_id: Some("FN_analytics".to_string()),
                    span: crate::source::SourceSpan {
                        file: "reports.py".to_string(),
                        line: 214,
                        col: 9,
                        end_line: 214,
                        end_col: 15,
                        snippet: "marker".to_string(),
                    },
                    context: "expr:statement".to_string(),
                },
            ],
            def_use_edges: vec![DefUseEdge {
                edge_id: "DU_marker".to_string(),
                def_id: "D_marker".to_string(),
                use_id: "U_marker".to_string(),
                place: Place::Local {
                    scope_id: "S_analytics".to_string(),
                    name: "marker".to_string(),
                },
                edge_kind: "local".to_string(),
                path_summary: "local".to_string(),
            }],
            var_dependency_edges: vec![
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_completed".to_string(),
                    source_place: Place::Attribute {
                        base: "*".to_string(),
                        attr: "isoformat".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S_analytics".to_string(),
                        name: "completed".to_string(),
                    },
                    source_id: "U_isoformat".to_string(),
                    target_id: "D_completed".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: attr_use_span,
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_selected".to_string(),
                    source_place: Place::Subscript {
                        base: "*".to_string(),
                        index: "*".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S_analytics".to_string(),
                        name: "selected".to_string(),
                    },
                    source_id: "U_payload_key".to_string(),
                    target_id: "D_selected".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: subscript_use_span,
                },
            ],
            ..AnalysisCache::default()
        };

        write_var_dependency_dot(&cache, &path, 10).unwrap();

        let dot = std::fs::read_to_string(path).unwrap();
        assert!(dot.contains(
            "[label=\"use app.routers.reports::get_task_analytics::task.completed_at.isoformat @ line 213\"]"
        ));
        assert!(dot.contains("app.routers.reports::get_task_analytics::payload[task.result_key]"));
    }

    #[test]
    fn var_dependency_writer_promotes_undefined_nested_attributes_to_parent_variable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("variables.dot");
        let function_span = crate::source::SourceSpan {
            file: "services/result_service.py".to_string(),
            line: 20,
            col: 1,
            end_line: 20,
            end_col: 30,
            snippet: "def get_task_summary(task):".to_string(),
        };
        let task_param_span = crate::source::SourceSpan {
            file: "services/result_service.py".to_string(),
            line: 20,
            col: 23,
            end_line: 20,
            end_col: 27,
            snippet: "task".to_string(),
        };
        let total_duration_span = crate::source::SourceSpan {
            file: "services/result_service.py".to_string(),
            line: 95,
            col: 17,
            end_line: 95,
            end_col: 87,
            snippet: "total_duration = (task.completed_at - task.started_at).total_seconds()"
                .to_string(),
        };
        let completed_span = crate::source::SourceSpan {
            file: "services/result_service.py".to_string(),
            line: 95,
            col: 35,
            end_line: 95,
            end_col: 52,
            snippet: "task.completed_at".to_string(),
        };
        let started_span = crate::source::SourceSpan {
            file: "services/result_service.py".to_string(),
            line: 95,
            col: 55,
            end_line: 95,
            end_col: 70,
            snippet: "task.started_at".to_string(),
        };
        let cache = AnalysisCache {
            schema_version: crate::ir::SCHEMA_VERSION,
            modules: vec![crate::ir::ModuleRecord {
                module_id: "M_result_service".to_string(),
                file_id: "F_result_service".to_string(),
                module_name: "app.services.result_service".to_string(),
                exports: Vec::new(),
                imports: Vec::new(),
            }],
            scopes: vec![ScopeRecord {
                scope_id: "S_summary".to_string(),
                scope_kind: "function".to_string(),
                parent_scope_id: None,
                owner_id: "FN_summary".to_string(),
                span: function_span.clone(),
            }],
            functions: vec![FunctionRecord {
                function_id: "FN_summary".to_string(),
                module_id: "M_result_service".to_string(),
                class_id: Some("C_service".to_string()),
                qualified_name: "ResultService.get_task_summary".to_string(),
                kind: "method".to_string(),
                params: vec!["self".to_string(), "task".to_string()],
                scope_id: "S_summary".to_string(),
                span: function_span,
            }],
            definitions: vec![
                Definition {
                    def_id: "D_task".to_string(),
                    place: Place::Local {
                        scope_id: "S_summary".to_string(),
                        name: "task".to_string(),
                    },
                    def_kind: "param".to_string(),
                    scope_id: "S_summary".to_string(),
                    function_id: Some("FN_summary".to_string()),
                    span: task_param_span,
                    expr: String::new(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_total_duration".to_string(),
                    place: Place::Local {
                        scope_id: "S_summary".to_string(),
                        name: "total_duration".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S_summary".to_string(),
                    function_id: Some("FN_summary".to_string()),
                    span: total_duration_span.clone(),
                    expr: "(task.completed_at - task.started_at).total_seconds()".to_string(),
                    deps: Vec::new(),
                },
            ],
            uses: vec![
                Use {
                    use_id: "U_completed".to_string(),
                    place: Place::Attribute {
                        base: "task".to_string(),
                        attr: "completed_at".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_summary".to_string(),
                    function_id: Some("FN_summary".to_string()),
                    span: completed_span,
                    context: "assign:rhs".to_string(),
                },
                Use {
                    use_id: "U_started".to_string(),
                    place: Place::Attribute {
                        base: "task".to_string(),
                        attr: "started_at".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_summary".to_string(),
                    function_id: Some("FN_summary".to_string()),
                    span: started_span,
                    context: "assign:rhs".to_string(),
                },
            ],
            def_use_edges: vec![
                DefUseEdge {
                    edge_id: "DU_task_completed".to_string(),
                    def_id: "D_task".to_string(),
                    use_id: "U_completed".to_string(),
                    place: Place::Local {
                        scope_id: "S_summary".to_string(),
                        name: "task".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "local".to_string(),
                },
                DefUseEdge {
                    edge_id: "DU_task_started".to_string(),
                    def_id: "D_task".to_string(),
                    use_id: "U_started".to_string(),
                    place: Place::Local {
                        scope_id: "S_summary".to_string(),
                        name: "task".to_string(),
                    },
                    edge_kind: "local".to_string(),
                    path_summary: "local".to_string(),
                },
            ],
            var_dependency_edges: vec![
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_completed".to_string(),
                    source_place: Place::Attribute {
                        base: "task".to_string(),
                        attr: "completed_at".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S_summary".to_string(),
                        name: "total_duration".to_string(),
                    },
                    source_id: "U_completed".to_string(),
                    target_id: "D_total_duration".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: total_duration_span.clone(),
                },
                crate::ir::VarDependencyEdge {
                    edge_id: "VD_started".to_string(),
                    source_place: Place::Attribute {
                        base: "task".to_string(),
                        attr: "started_at".to_string(),
                    },
                    target_place: Place::Local {
                        scope_id: "S_summary".to_string(),
                        name: "total_duration".to_string(),
                    },
                    source_id: "U_started".to_string(),
                    target_id: "D_total_duration".to_string(),
                    dep_kind: "assignment".to_string(),
                    span: total_duration_span,
                },
            ],
            ..AnalysisCache::default()
        };

        write_var_dependency_dot(&cache, &path, 10).unwrap();

        let dot = std::fs::read_to_string(path).unwrap();
        assert!(dot.contains(
            "[label=\"use app.services.result_service::ResultService.get_task_summary::task @ line 95\"]"
        ));
        assert!(!dot.contains("task.completed_at @ line 95"));
        assert!(!dot.contains("task.started_at @ line 95"));
    }

    #[test]
    fn var_dependency_writer_reconnects_canonicalized_use_to_parent_definition() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("variables.dot");
        let function_span = crate::source::SourceSpan {
            file: "file_manager.py".to_string(),
            line: 176,
            col: 5,
            end_line: 287,
            end_col: 13,
            snippet: "async def upload_audio_files(self, files, test_set_name=None):".to_string(),
        };
        let test_set_def_span = crate::source::SourceSpan {
            file: "file_manager.py".to_string(),
            line: 194,
            col: 13,
            end_line: 197,
            end_col: 14,
            snippet: "test_set = TestSet(...)".to_string(),
        };
        let directory_path_def_span = crate::source::SourceSpan {
            file: "file_manager.py".to_string(),
            line: 272,
            col: 21,
            end_line: 272,
            end_col: 64,
            snippet: "directory_path = f\"test_sets/{test_set.id}\"".to_string(),
        };
        let test_set_id_use_span = crate::source::SourceSpan {
            file: "file_manager.py".to_string(),
            line: 272,
            col: 51,
            end_line: 272,
            end_col: 62,
            snippet: "test_set.id".to_string(),
        };
        let cache = AnalysisCache {
            schema_version: crate::ir::SCHEMA_VERSION,
            modules: vec![crate::ir::ModuleRecord {
                module_id: "M_file_manager".to_string(),
                file_id: "F_file_manager".to_string(),
                module_name: "app.file_manager".to_string(),
                exports: Vec::new(),
                imports: Vec::new(),
            }],
            scopes: vec![ScopeRecord {
                scope_id: "S_upload".to_string(),
                scope_kind: "function".to_string(),
                parent_scope_id: None,
                owner_id: "FN_upload".to_string(),
                span: function_span.clone(),
            }],
            classes: vec![crate::ir::ClassRecord {
                class_id: "C_file_manager".to_string(),
                module_id: "M_file_manager".to_string(),
                qualified_name: "FileManager".to_string(),
                span: function_span.clone(),
                base_exprs: Vec::new(),
                resolved_bases: Vec::new(),
                mro_status: "resolved".to_string(),
                methods: vec!["FN_upload".to_string()],
            }],
            functions: vec![FunctionRecord {
                function_id: "FN_upload".to_string(),
                module_id: "M_file_manager".to_string(),
                class_id: Some("C_file_manager".to_string()),
                qualified_name: "FileManager.upload_audio_files".to_string(),
                kind: "method".to_string(),
                params: vec![
                    "self".to_string(),
                    "files".to_string(),
                    "test_set_name".to_string(),
                ],
                scope_id: "S_upload".to_string(),
                span: function_span,
            }],
            definitions: vec![
                Definition {
                    def_id: "D_test_set".to_string(),
                    place: Place::Local {
                        scope_id: "S_upload".to_string(),
                        name: "test_set".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S_upload".to_string(),
                    function_id: Some("FN_upload".to_string()),
                    span: test_set_def_span,
                    expr: "TestSet(...)".to_string(),
                    deps: Vec::new(),
                },
                Definition {
                    def_id: "D_directory_path".to_string(),
                    place: Place::Local {
                        scope_id: "S_upload".to_string(),
                        name: "directory_path".to_string(),
                    },
                    def_kind: "assign".to_string(),
                    scope_id: "S_upload".to_string(),
                    function_id: Some("FN_upload".to_string()),
                    span: directory_path_def_span.clone(),
                    expr: "f\"test_sets/{test_set.id}\"".to_string(),
                    deps: Vec::new(),
                },
            ],
            uses: vec![Use {
                use_id: "U_test_set_id".to_string(),
                place: Place::Attribute {
                    base: "test_set".to_string(),
                    attr: "id".to_string(),
                },
                use_kind: "load".to_string(),
                scope_id: "S_upload".to_string(),
                function_id: Some("FN_upload".to_string()),
                span: test_set_id_use_span,
                context: "assign:rhs".to_string(),
            }],
            var_dependency_edges: vec![crate::ir::VarDependencyEdge {
                edge_id: "VD_directory_path".to_string(),
                source_place: Place::Attribute {
                    base: "test_set".to_string(),
                    attr: "id".to_string(),
                },
                target_place: Place::Local {
                    scope_id: "S_upload".to_string(),
                    name: "directory_path".to_string(),
                },
                source_id: "U_test_set_id".to_string(),
                target_id: "D_directory_path".to_string(),
                dep_kind: "assignment".to_string(),
                span: directory_path_def_span,
            }],
            ..AnalysisCache::default()
        };

        write_var_dependency_dot(&cache, &path, 10).unwrap();

        let dot = std::fs::read_to_string(path).unwrap();
        assert!(dot.contains(
            "[label=\"def app.file_manager::FileManager.upload_audio_files::test_set @ line 194\"]"
        ));
        assert!(dot.contains(
            "[label=\"use app.file_manager::FileManager.upload_audio_files::test_set @ line 272\"]"
        ));
        assert!(dot.contains("\"D_test_set\" -> \"UCAN:S_upload:file_manager.py:272:Local { scope_id: \\\"S_upload\\\", name: \\\"test_set\\\" }\" [label=\"def-use\""));
        assert!(dot.contains(
            "\"UCAN:S_upload:file_manager.py:272:Local { scope_id: \\\"S_upload\\\", name: \\\"test_set\\\" }\" -> \"D_directory_path\" [label=\"assignment\"]"
        ));
    }

    #[test]
    fn svg_post_processor_rewrites_internal_titles_from_dot_labels() {
        let dot = concat!(
            "digraph DefUseHotspots {\n",
            "  \"D_1\" [label=\"def x\"];\n",
            "  \"U_1\" [label=\"use x\"];\n",
            "  \"D_1\" -> \"U_1\" [label=\"local\"];\n",
            "}\n"
        );
        let svg = concat!(
            "<svg>\n",
            "<title>DefUseHotspots</title>\n",
            "<!-- D_1 -->\n",
            "<title>D_1</title>\n",
            "<!-- D_1&#45;&gt;U_1 -->\n",
            "<title>D_1&#45;&gt;U_1</title>\n",
            "</svg>\n"
        );

        let rewritten = rewrite_svg_metadata(dot, svg);

        assert!(rewritten.contains("<!-- def x -->"));
        assert!(rewritten.contains("<title>def x</title>"));
        assert!(rewritten.contains("<!-- def x -> use x (local) -->"));
        assert!(rewritten.contains("<title>def x -&gt; use x (local)</title>"));
        assert!(!rewritten.contains("<title>D_1</title>"));
        assert!(!rewritten.contains("D_1&#45;&gt;U_1"));
    }

    #[test]
    fn var_dependency_graph_json_writer_emits_render_ready_document() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("variable_dependencies.graph.json");
        let module_span = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 1,
            col: 1,
            end_line: 3,
            end_col: 1,
            snippet: "def foo(value):".to_string(),
        };
        let def_span = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 2,
            col: 5,
            end_line: 2,
            end_col: 14,
            snippet: "x = value".to_string(),
        };
        let use_span = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 3,
            col: 11,
            end_line: 3,
            end_col: 12,
            snippet: "print(x)".to_string(),
        };
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            modules: vec![crate::ir::ModuleRecord {
                module_id: "M_app".to_string(),
                file_id: "F_app".to_string(),
                module_name: "app.main".to_string(),
                exports: Vec::new(),
                imports: Vec::new(),
            }],
            scopes: vec![ScopeRecord {
                scope_id: "S_foo".to_string(),
                scope_kind: "function".to_string(),
                parent_scope_id: None,
                owner_id: "FN_foo".to_string(),
                span: module_span.clone(),
            }],
            functions: vec![FunctionRecord {
                function_id: "FN_foo".to_string(),
                module_id: "M_app".to_string(),
                class_id: None,
                qualified_name: "foo".to_string(),
                kind: "function".to_string(),
                params: vec!["value".to_string()],
                scope_id: "S_foo".to_string(),
                span: module_span.clone(),
            }],
            definitions: vec![Definition {
                def_id: "D_x".to_string(),
                place: Place::Local {
                    scope_id: "S_foo".to_string(),
                    name: "x".to_string(),
                },
                def_kind: "assign".to_string(),
                scope_id: "S_foo".to_string(),
                function_id: Some("FN_foo".to_string()),
                span: def_span.clone(),
                expr: "value".to_string(),
                deps: vec![Place::Local {
                    scope_id: "S_foo".to_string(),
                    name: "value".to_string(),
                }],
            }],
            uses: vec![
                Use {
                    use_id: "U_value".to_string(),
                    place: Place::Local {
                        scope_id: "S_foo".to_string(),
                        name: "value".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_foo".to_string(),
                    function_id: Some("FN_foo".to_string()),
                    span: def_span.clone(),
                    context: "rhs".to_string(),
                },
                Use {
                    use_id: "U_x".to_string(),
                    place: Place::Local {
                        scope_id: "S_foo".to_string(),
                        name: "x".to_string(),
                    },
                    use_kind: "load".to_string(),
                    scope_id: "S_foo".to_string(),
                    function_id: Some("FN_foo".to_string()),
                    span: use_span.clone(),
                    context: "call".to_string(),
                },
            ],
            def_use_edges: vec![DefUseEdge {
                edge_id: "DU_x".to_string(),
                def_id: "D_x".to_string(),
                use_id: "U_x".to_string(),
                place: Place::Local {
                    scope_id: "S_foo".to_string(),
                    name: "x".to_string(),
                },
                edge_kind: "local".to_string(),
                path_summary: "local".to_string(),
            }],
            var_dependency_edges: vec![crate::ir::VarDependencyEdge {
                edge_id: "VD_x".to_string(),
                source_place: Place::Local {
                    scope_id: "S_foo".to_string(),
                    name: "value".to_string(),
                },
                target_place: Place::Local {
                    scope_id: "S_foo".to_string(),
                    name: "x".to_string(),
                },
                source_id: "U_value".to_string(),
                target_id: "D_x".to_string(),
                dep_kind: "assignment".to_string(),
                span: def_span,
            }],
            ..AnalysisCache::default()
        };

        write_var_dependency_graph_json(&cache, &path, 10).unwrap();

        let json = std::fs::read_to_string(path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["graph_kind"], "VariableDependencies");
        assert_eq!(value["views"][0]["id"], "variable_dependencies");
        assert_eq!(value["views"][0]["root_node_ids"][0], "D_x");
        assert_eq!(value["views"][0]["path_ids"].as_array().unwrap().len(), 1);
        assert_eq!(value["stats"]["def_count"], 1);
        assert_eq!(value["stats"]["use_count"], 2);
        assert_eq!(value["paths"].as_array().unwrap().len(), 1);
        assert_eq!(value["paths"][0]["root_node_id"], "D_x");
        assert_eq!(value["paths"][0]["node_ids"][0], "D_x");
        assert_eq!(value["paths"][0]["node_ids"][1], "U_x");
        assert!(
            value["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| node["label"] == "def app.main::foo::x @ line 2")
        );
        assert!(
            value["edges"]
                .as_array()
                .unwrap()
                .iter()
                .any(|edge| edge["kind"] == "assignment"
                    && edge["from"] == "U_value"
                    && edge["to"] == "D_x")
        );
        assert!(
            value["edges"]
                .as_array()
                .unwrap()
                .iter()
                .any(|edge| edge["kind"] == "def-use"
                    && edge["from"] == "D_x"
                    && edge["to"] == "U_x")
        );
    }

    #[test]
    fn def_use_hotspot_graph_json_writer_emits_paths_and_render_ready_nodes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("def_use_hotspots.graph.json");
        let def_span = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 10,
            col: 4,
            end_line: 10,
            end_col: 9,
            snippet: "x = value".to_string(),
        };
        let use_span = crate::source::SourceSpan {
            file: "app/main.py".to_string(),
            line: 11,
            col: 11,
            end_line: 11,
            end_col: 12,
            snippet: "print(x)".to_string(),
        };
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            modules: vec![crate::ir::ModuleRecord {
                module_id: "M_app".to_string(),
                file_id: "F_app".to_string(),
                module_name: "app.main".to_string(),
                exports: Vec::new(),
                imports: Vec::new(),
            }],
            scopes: vec![ScopeRecord {
                scope_id: "S".to_string(),
                scope_kind: "function".to_string(),
                parent_scope_id: None,
                owner_id: "FN_foo".to_string(),
                span: def_span.clone(),
            }],
            functions: vec![FunctionRecord {
                function_id: "FN_foo".to_string(),
                module_id: "M_app".to_string(),
                class_id: None,
                qualified_name: "foo".to_string(),
                kind: "function".to_string(),
                params: Vec::new(),
                scope_id: "S".to_string(),
                span: def_span.clone(),
            }],
            definitions: vec![Definition {
                def_id: "D_x".to_string(),
                place: Place::Local {
                    scope_id: "S".to_string(),
                    name: "x".to_string(),
                },
                def_kind: "assign".to_string(),
                scope_id: "S".to_string(),
                function_id: Some("FN_foo".to_string()),
                span: def_span,
                expr: "value".to_string(),
                deps: Vec::new(),
            }],
            uses: vec![Use {
                use_id: "U_x".to_string(),
                place: Place::Local {
                    scope_id: "S".to_string(),
                    name: "x".to_string(),
                },
                use_kind: "load".to_string(),
                scope_id: "S".to_string(),
                function_id: Some("FN_foo".to_string()),
                span: use_span,
                context: "call".to_string(),
            }],
            def_use_edges: vec![DefUseEdge {
                edge_id: "DU_1".to_string(),
                def_id: "D_x".to_string(),
                use_id: "U_x".to_string(),
                place: Place::Local {
                    scope_id: "S".to_string(),
                    name: "x".to_string(),
                },
                edge_kind: "local".to_string(),
                path_summary: "same-block".to_string(),
            }],
            ..AnalysisCache::default()
        };

        write_def_use_hotspots_graph_json(&cache, &path, 10).unwrap();

        let json = std::fs::read_to_string(path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["graph_kind"], "DefUseHotspots");
        assert_eq!(value["views"][0]["id"], "def_use_hotspots");
        assert_eq!(value["views"][0]["root_node_ids"][0], "D_x");
        assert_eq!(value["stats"]["def_count"], 1);
        assert_eq!(value["stats"]["use_count"], 1);
        assert_eq!(value["paths"].as_array().unwrap().len(), 1);
        assert_eq!(value["paths"][0]["root_node_id"], "D_x");
        assert!(
            value["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| node["label"] == "def app.main::foo::x @ line 10")
        );
        assert!(
            value["edges"]
                .as_array()
                .unwrap()
                .iter()
                .any(|edge| edge["kind"] == "local"
                    && edge["from"] == "D_x"
                    && edge["to"] == "U_x")
        );
    }
}
