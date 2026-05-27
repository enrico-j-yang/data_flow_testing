use crate::source::SourceSpan;
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Place {
    Local { scope_id: String, name: String },
    Global { module_id: String, name: String },
    Closure { scope_id: String, name: String },
    Attribute { base: String, attr: String },
    Subscript { base: String, index: String },
    External { name: String },
    Unknown { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Definition {
    pub def_id: String,
    pub place: Place,
    pub def_kind: String,
    pub scope_id: String,
    pub function_id: Option<String>,
    pub span: SourceSpan,
    pub expr: String,
    pub deps: Vec<Place>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Use {
    pub use_id: String,
    pub place: Place,
    pub use_kind: String,
    pub scope_id: String,
    pub function_id: Option<String>,
    pub span: SourceSpan,
    pub context: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisCache {
    pub schema_version: u32,
    pub tool_version: String,
    pub files: Vec<SourceFileRecord>,
    pub modules: Vec<ModuleRecord>,
    pub scopes: Vec<ScopeRecord>,
    pub classes: Vec<ClassRecord>,
    pub functions: Vec<FunctionRecord>,
    pub definitions: Vec<Definition>,
    pub uses: Vec<Use>,
    pub captures: Vec<CaptureRecord>,
    pub calls: Vec<CallRecord>,
    pub cfgs: Vec<CfgRecord>,
    pub def_use_edges: Vec<DefUseEdge>,
    pub var_dependency_edges: Vec<VarDependencyEdge>,
    pub function_summaries: Vec<FunctionSummary>,
    pub diagnostics: Vec<Diagnostic>,
    pub graph_index: Vec<GraphRecord>,
}

impl Default for AnalysisCache {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            tool_version: String::new(),
            files: Vec::new(),
            modules: Vec::new(),
            scopes: Vec::new(),
            classes: Vec::new(),
            functions: Vec::new(),
            definitions: Vec::new(),
            uses: Vec::new(),
            captures: Vec::new(),
            calls: Vec::new(),
            cfgs: Vec::new(),
            def_use_edges: Vec::new(),
            var_dependency_edges: Vec::new(),
            function_summaries: Vec::new(),
            diagnostics: Vec::new(),
            graph_index: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceFileRecord {
    pub file_id: String,
    pub path: String,
    pub hash: String,
    pub line_count: usize,
    pub parse_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub diagnostic_id: String,
    pub severity: String,
    pub kind: String,
    pub message: String,
    pub file: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleRecord {
    pub module_id: String,
    pub file_id: String,
    pub module_name: String,
    pub exports: Vec<String>,
    pub imports: Vec<ImportRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportRecord {
    pub import_id: String,
    pub module: String,
    pub name: Option<String>,
    pub alias: Option<String>,
    pub level: usize,
    pub resolution: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeRecord {
    pub scope_id: String,
    pub scope_kind: String,
    pub parent_scope_id: Option<String>,
    pub owner_id: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassRecord {
    pub class_id: String,
    pub module_id: String,
    pub qualified_name: String,
    pub base_exprs: Vec<String>,
    pub resolved_bases: Vec<String>,
    pub mro_status: String,
    pub methods: Vec<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionRecord {
    pub function_id: String,
    pub module_id: String,
    pub class_id: Option<String>,
    pub qualified_name: String,
    pub kind: String,
    pub params: Vec<String>,
    pub scope_id: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureRecord {
    pub capture_id: String,
    pub source_scope_id: String,
    pub target_function_id: String,
    pub place: Place,
    pub mode: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallRecord {
    pub call_id: String,
    pub function_id: Option<String>,
    pub callee_expr: String,
    pub candidate_function_ids: Vec<String>,
    pub resolution: String,
    pub arg_use_ids: Vec<String>,
    pub return_target_def_id: Option<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CfgRecord {
    pub function_id: String,
    pub blocks: Vec<CfgBlock>,
    pub edges: Vec<CfgEdge>,
    pub entry_block_id: String,
    pub exit_block_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CfgBlock {
    pub block_id: String,
    pub block_kind: String,
    pub statements: Vec<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CfgEdge {
    pub edge_id: String,
    pub from_block_id: String,
    pub to_block_id: String,
    pub edge_kind: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefUseEdge {
    pub edge_id: String,
    pub def_id: String,
    pub use_id: String,
    pub place: Place,
    pub edge_kind: String,
    pub path_summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VarDependencyEdge {
    pub edge_id: String,
    pub source_place: Place,
    pub target_place: Place,
    pub source_id: String,
    pub target_id: String,
    pub dep_kind: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionSummary {
    pub function_id: String,
    pub inputs: Vec<Place>,
    pub returns: Vec<Place>,
    pub yields: Vec<Place>,
    pub writes: Vec<Place>,
    pub raises: Vec<Place>,
    pub external_effects: Vec<String>,
    pub fixpoint_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphRecord {
    pub graph_id: String,
    pub kind: String,
    pub dot_path: String,
    pub svg_path: Option<String>,
    pub html_path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SourceSpan;

    #[test]
    fn analysis_cache_default_uses_current_schema_version() {
        assert_eq!(AnalysisCache::default().schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn ir_shell_types_round_trip_through_serde() {
        let span = SourceSpan::synthetic("app/main.py", "x = y");
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            tool_version: "0.1.0".to_string(),
            files: vec![SourceFileRecord {
                file_id: "F_deadbeef".to_string(),
                path: "app/main.py".to_string(),
                hash: "abc123".to_string(),
                line_count: 12,
                parse_status: "ok".to_string(),
            }],
            modules: Vec::new(),
            scopes: Vec::new(),
            classes: Vec::new(),
            functions: Vec::new(),
            definitions: vec![Definition {
                def_id: "D_deadbeef".to_string(),
                place: Place::Local {
                    scope_id: "S_module".to_string(),
                    name: "x".to_string(),
                },
                def_kind: "assign".to_string(),
                scope_id: "S_module".to_string(),
                function_id: None,
                span: span.clone(),
                expr: "y".to_string(),
                deps: vec![Place::Global {
                    module_id: "M_app".to_string(),
                    name: "y".to_string(),
                }],
            }],
            uses: vec![Use {
                use_id: "U_deadbeef".to_string(),
                place: Place::Global {
                    module_id: "M_app".to_string(),
                    name: "y".to_string(),
                },
                use_kind: "load".to_string(),
                scope_id: "S_module".to_string(),
                function_id: Some("FN_main".to_string()),
                span: span.clone(),
                context: "rhs".to_string(),
            }],
            captures: Vec::new(),
            calls: Vec::new(),
            cfgs: Vec::new(),
            def_use_edges: Vec::new(),
            var_dependency_edges: Vec::new(),
            function_summaries: Vec::new(),
            diagnostics: vec![Diagnostic {
                diagnostic_id: "G_deadbeef".to_string(),
                severity: "warning".to_string(),
                kind: "parse".to_string(),
                message: "example".to_string(),
                file: "app/main.py".to_string(),
                span,
            }],
            graph_index: Vec::new(),
        };

        let json = serde_json::to_string(&cache).expect("serialize cache");
        let round_trip: AnalysisCache = serde_json::from_str(&json).expect("deserialize cache");

        assert_eq!(round_trip.schema_version, SCHEMA_VERSION);
        assert_eq!(round_trip.tool_version, "0.1.0");
        assert_eq!(round_trip.files.len(), 1);
        assert_eq!(round_trip.files[0].file_id, "F_deadbeef");
        assert_eq!(round_trip.files[0].path, "app/main.py");
        assert_eq!(round_trip.files[0].hash, "abc123");
        assert_eq!(round_trip.files[0].line_count, 12);
        assert_eq!(round_trip.files[0].parse_status, "ok");

        assert_eq!(round_trip.definitions.len(), 1);
        assert_eq!(round_trip.definitions[0].def_id, "D_deadbeef");
        assert_eq!(
            round_trip.definitions[0].place,
            Place::Local {
                scope_id: "S_module".to_string(),
                name: "x".to_string(),
            }
        );
        assert_eq!(round_trip.definitions[0].def_kind, "assign");
        assert_eq!(round_trip.definitions[0].scope_id, "S_module");
        assert_eq!(round_trip.definitions[0].function_id, None);
        assert_eq!(
            round_trip.definitions[0].span,
            SourceSpan::synthetic("app/main.py", "x = y")
        );
        assert_eq!(round_trip.definitions[0].expr, "y");
        assert_eq!(
            round_trip.definitions[0].deps,
            vec![Place::Global {
                module_id: "M_app".to_string(),
                name: "y".to_string(),
            }]
        );

        assert_eq!(round_trip.uses.len(), 1);
        assert_eq!(round_trip.uses[0].use_id, "U_deadbeef");
        assert_eq!(
            round_trip.uses[0].place,
            Place::Global {
                module_id: "M_app".to_string(),
                name: "y".to_string(),
            }
        );
        assert_eq!(round_trip.uses[0].use_kind, "load");
        assert_eq!(round_trip.uses[0].scope_id, "S_module");
        assert_eq!(round_trip.uses[0].function_id.as_deref(), Some("FN_main"));
        assert_eq!(round_trip.uses[0].span, SourceSpan::synthetic("app/main.py", "x = y"));
        assert_eq!(round_trip.uses[0].context, "rhs");

        assert_eq!(round_trip.diagnostics.len(), 1);
        assert_eq!(round_trip.diagnostics[0].diagnostic_id, "G_deadbeef");
        assert_eq!(round_trip.diagnostics[0].severity, "warning");
        assert_eq!(round_trip.diagnostics[0].kind, "parse");
        assert_eq!(round_trip.diagnostics[0].message, "example");
        assert_eq!(round_trip.diagnostics[0].file, "app/main.py");
        assert_eq!(
            round_trip.diagnostics[0].span,
            SourceSpan::synthetic("app/main.py", "x = y")
        );
    }

    #[test]
    fn cache_round_trips_rich_ir() {
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            tool_version: "0.1.0".to_string(),
            files: vec![SourceFileRecord {
                file_id: "M_a".to_string(),
                path: "app/a.py".to_string(),
                hash: "abc".to_string(),
                line_count: 3,
                parse_status: "ok".to_string(),
            }],
            modules: vec![ModuleRecord {
                module_id: "M_a".to_string(),
                file_id: "M_a".to_string(),
                module_name: "app.a".to_string(),
                exports: vec!["foo".to_string()],
                imports: Vec::new(),
            }],
            scopes: Vec::new(),
            classes: Vec::new(),
            functions: Vec::new(),
            definitions: vec![Definition {
                def_id: "D_x".to_string(),
                place: Place::Local {
                    scope_id: "S_a".to_string(),
                    name: "x".to_string(),
                },
                def_kind: "assign".to_string(),
                scope_id: "S_a".to_string(),
                function_id: None,
                span: SourceSpan::synthetic("app/a.py", "x = 1"),
                expr: "1".to_string(),
                deps: Vec::new(),
            }],
            uses: Vec::new(),
            captures: Vec::new(),
            calls: Vec::new(),
            cfgs: Vec::new(),
            def_use_edges: Vec::new(),
            var_dependency_edges: Vec::new(),
            function_summaries: Vec::new(),
            diagnostics: Vec::new(),
            graph_index: Vec::new(),
        };

        let json = serde_json::to_string(&cache).unwrap();
        let decoded: AnalysisCache = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.schema_version, SCHEMA_VERSION);
        assert_eq!(decoded.definitions[0].def_kind, "assign");
    }
}
