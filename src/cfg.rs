use crate::ids::stable_id;
use crate::ir::{CfgBlock, CfgEdge, CfgRecord, SCHEMA_VERSION};
use crate::source::SourceSpan;

#[derive(Debug, Clone)]
pub struct ControlFlowGraph {
    pub function_id: String,
    pub entry_block_id: String,
    pub exit_block_id: String,
    pub blocks: Vec<CfgBlock>,
    pub edges: Vec<CfgEdge>,
}

impl ControlFlowGraph {
    pub fn new(function_id: String) -> Self {
        let entry = stable_id("B", SCHEMA_VERSION, &[&function_id, "entry"]);
        let exit = stable_id("B", SCHEMA_VERSION, &[&function_id, "exit"]);

        Self {
            function_id: function_id.clone(),
            entry_block_id: entry.clone(),
            exit_block_id: exit.clone(),
            blocks: vec![
                CfgBlock {
                    block_id: entry,
                    block_kind: "Entry".to_string(),
                    statements: Vec::new(),
                    span: SourceSpan::synthetic("<cfg>", "entry"),
                },
                CfgBlock {
                    block_id: exit,
                    block_kind: "Exit".to_string(),
                    statements: Vec::new(),
                    span: SourceSpan::synthetic("<cfg>", "exit"),
                },
            ],
            edges: Vec::new(),
        }
    }

    pub fn add_block(&mut self, kind: &str, span: SourceSpan) -> String {
        let block_id = stable_id(
            "B",
            SCHEMA_VERSION,
            &[
                &self.function_id,
                kind,
                &self.blocks.len().to_string(),
                &span.snippet,
            ],
        );
        self.blocks.push(CfgBlock {
            block_id: block_id.clone(),
            block_kind: kind.to_string(),
            statements: Vec::new(),
            span,
        });
        block_id
    }

    pub fn add_edge(&mut self, from: &str, to: &str, kind: &str, label: &str) {
        self.edges.push(CfgEdge {
            edge_id: stable_id(
                "E",
                SCHEMA_VERSION,
                &[from, to, kind, &self.edges.len().to_string()],
            ),
            from_block_id: from.to_string(),
            to_block_id: to.to_string(),
            edge_kind: kind.to_string(),
            label: label.to_string(),
        });
    }

    pub fn into_record(self) -> CfgRecord {
        CfgRecord {
            function_id: self.function_id,
            blocks: self.blocks,
            edges: self.edges,
            entry_block_id: self.entry_block_id,
            exit_block_id: self.exit_block_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cfg_models_for_else_break_and_continue_edges() {
        let mut cfg = ControlFlowGraph::new("F_loop".to_string());
        let entry = cfg.entry_block_id.clone();
        let body = cfg.add_block("BasicBlock", SourceSpan::synthetic("loop.py", "body"));
        let else_block = cfg.add_block("BasicBlock", SourceSpan::synthetic("loop.py", "else"));
        let exit = cfg.exit_block_id.clone();

        cfg.add_edge(&entry, &body, "loop-body", "for body");
        cfg.add_edge(&body, &entry, "continue-back", "continue");
        cfg.add_edge(&entry, &else_block, "loop-else", "normal completion");
        cfg.add_edge(&body, &exit, "break-exit", "break");

        assert!(cfg.edges.iter().any(|edge| edge.edge_kind == "loop-else"));
        assert!(cfg.edges.iter().any(|edge| edge.edge_kind == "break-exit"));
        assert!(
            cfg.edges
                .iter()
                .any(|edge| edge.edge_kind == "continue-back")
        );
    }
}
