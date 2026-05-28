use crate::source::SourceSpan;
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
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

impl AnalysisCache {
    pub fn imports(&self) -> Vec<&ImportRecord> {
        self.modules.iter().flat_map(|m| m.imports.iter()).collect()
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
        assert_eq!(SCHEMA_VERSION, 2);
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
        let module_span = SourceSpan::synthetic("app/a.py", "import math");
        let function_span = SourceSpan::synthetic("app/a.py", "def foo(value): return value");
        let call_span = SourceSpan::synthetic("app/a.py", "print(value)");
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
                imports: vec![ImportRecord {
                    import_id: "I_math".to_string(),
                    module: "math".to_string(),
                    name: Some("sqrt".to_string()),
                    alias: Some("sq".to_string()),
                    level: 0,
                    resolution: "resolved".to_string(),
                    span: module_span.clone(),
                }],
            }],
            scopes: vec![
                ScopeRecord {
                    scope_id: "S_module".to_string(),
                    scope_kind: "module".to_string(),
                    parent_scope_id: None,
                    owner_id: "M_a".to_string(),
                    span: module_span.clone(),
                },
                ScopeRecord {
                    scope_id: "S_fn".to_string(),
                    scope_kind: "function".to_string(),
                    parent_scope_id: Some("S_module".to_string()),
                    owner_id: "FN_foo".to_string(),
                    span: function_span.clone(),
                },
            ],
            classes: vec![ClassRecord {
                class_id: "C_box".to_string(),
                module_id: "M_a".to_string(),
                qualified_name: "app.a.Box".to_string(),
                base_exprs: vec!["Base".to_string()],
                resolved_bases: vec!["app.base.Base".to_string()],
                mro_status: "resolved".to_string(),
                methods: vec!["FN_foo".to_string()],
                span: function_span.clone(),
            }],
            functions: vec![FunctionRecord {
                function_id: "FN_foo".to_string(),
                module_id: "M_a".to_string(),
                class_id: Some("C_box".to_string()),
                qualified_name: "app.a.Box.foo".to_string(),
                kind: "method".to_string(),
                params: vec!["self".to_string(), "value".to_string()],
                scope_id: "S_fn".to_string(),
                span: function_span.clone(),
            }],
            definitions: vec![Definition {
                def_id: "D_x".to_string(),
                place: Place::Local {
                    scope_id: "S_fn".to_string(),
                    name: "x".to_string(),
                },
                def_kind: "assign".to_string(),
                scope_id: "S_fn".to_string(),
                function_id: Some("FN_foo".to_string()),
                span: function_span.clone(),
                expr: "value".to_string(),
                deps: vec![Place::Local {
                    scope_id: "S_fn".to_string(),
                    name: "value".to_string(),
                }],
            }],
            uses: vec![Use {
                use_id: "U_value".to_string(),
                place: Place::Local {
                    scope_id: "S_fn".to_string(),
                    name: "value".to_string(),
                },
                use_kind: "load".to_string(),
                scope_id: "S_fn".to_string(),
                function_id: Some("FN_foo".to_string()),
                span: function_span.clone(),
                context: "rhs".to_string(),
            }],
            captures: vec![CaptureRecord {
                capture_id: "CAP_x".to_string(),
                source_scope_id: "S_module".to_string(),
                target_function_id: "FN_foo".to_string(),
                place: Place::Global {
                    module_id: "M_a".to_string(),
                    name: "x".to_string(),
                },
                mode: "read".to_string(),
                span: function_span.clone(),
            }],
            calls: vec![CallRecord {
                call_id: "CALL_print".to_string(),
                function_id: Some("FN_foo".to_string()),
                callee_expr: "print".to_string(),
                candidate_function_ids: vec!["FN_builtin_print".to_string()],
                resolution: "builtin".to_string(),
                arg_use_ids: vec!["U_value".to_string()],
                return_target_def_id: Some("D_x".to_string()),
                span: call_span.clone(),
            }],
            cfgs: vec![CfgRecord {
                function_id: "FN_foo".to_string(),
                blocks: vec![
                    CfgBlock {
                        block_id: "B_entry".to_string(),
                        block_kind: "entry".to_string(),
                        statements: vec!["x = value".to_string()],
                        span: function_span.clone(),
                    },
                    CfgBlock {
                        block_id: "B_exit".to_string(),
                        block_kind: "exit".to_string(),
                        statements: vec!["return x".to_string()],
                        span: function_span.clone(),
                    },
                ],
                edges: vec![CfgEdge {
                    edge_id: "E_next".to_string(),
                    from_block_id: "B_entry".to_string(),
                    to_block_id: "B_exit".to_string(),
                    edge_kind: "fallthrough".to_string(),
                    label: "next".to_string(),
                }],
                entry_block_id: "B_entry".to_string(),
                exit_block_id: "B_exit".to_string(),
            }],
            def_use_edges: vec![DefUseEdge {
                edge_id: "DU_1".to_string(),
                def_id: "D_x".to_string(),
                use_id: "U_value".to_string(),
                place: Place::Local {
                    scope_id: "S_fn".to_string(),
                    name: "x".to_string(),
                },
                edge_kind: "direct".to_string(),
                path_summary: "B_entry -> B_exit".to_string(),
            }],
            var_dependency_edges: vec![VarDependencyEdge {
                edge_id: "VD_1".to_string(),
                source_place: Place::Local {
                    scope_id: "S_fn".to_string(),
                    name: "value".to_string(),
                },
                target_place: Place::Local {
                    scope_id: "S_fn".to_string(),
                    name: "x".to_string(),
                },
                source_id: "U_value".to_string(),
                target_id: "D_x".to_string(),
                dep_kind: "data".to_string(),
                span: function_span.clone(),
            }],
            function_summaries: vec![FunctionSummary {
                function_id: "FN_foo".to_string(),
                inputs: vec![Place::Local {
                    scope_id: "S_fn".to_string(),
                    name: "value".to_string(),
                }],
                returns: vec![Place::Local {
                    scope_id: "S_fn".to_string(),
                    name: "x".to_string(),
                }],
                yields: Vec::new(),
                writes: vec![Place::Attribute {
                    base: "self".to_string(),
                    attr: "value".to_string(),
                }],
                raises: vec![Place::External {
                    name: "ValueError".to_string(),
                }],
                external_effects: vec!["prints".to_string()],
                fixpoint_status: "stable".to_string(),
            }],
            diagnostics: vec![Diagnostic {
                diagnostic_id: "G_import".to_string(),
                severity: "warning".to_string(),
                kind: "import".to_string(),
                message: "using builtin resolution".to_string(),
                file: "app/a.py".to_string(),
                span: module_span.clone(),
            }],
            graph_index: vec![GraphRecord {
                graph_id: "CFG_FN_foo".to_string(),
                kind: "cfg".to_string(),
                dot_path: "graphs/foo.dot".to_string(),
                svg_path: Some("graphs/foo.svg".to_string()),
                html_path: Some("graphs/foo.html".to_string()),
            }],
        };

        let json = serde_json::to_string(&cache).unwrap();
        let decoded: AnalysisCache = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.schema_version, 2);
        assert_eq!(decoded.modules[0].imports[0].alias.as_deref(), Some("sq"));
        assert_eq!(decoded.scopes[1].parent_scope_id.as_deref(), Some("S_module"));
        assert_eq!(decoded.classes[0].resolved_bases, vec!["app.base.Base".to_string()]);
        assert_eq!(decoded.functions[0].class_id.as_deref(), Some("C_box"));
        assert_eq!(decoded.definitions[0].def_kind, "assign");
        assert_eq!(decoded.uses[0].context, "rhs");
        assert_eq!(decoded.captures[0].mode, "read");
        assert_eq!(decoded.calls[0].arg_use_ids, vec!["U_value".to_string()]);
        assert_eq!(decoded.cfgs[0].edges[0].label, "next");
        assert_eq!(decoded.def_use_edges[0].path_summary, "B_entry -> B_exit");
        assert_eq!(decoded.var_dependency_edges[0].dep_kind, "data");
        assert_eq!(
            decoded.function_summaries[0].external_effects,
            vec!["prints".to_string()]
        );
        assert_eq!(decoded.diagnostics[0].kind, "import");
        assert_eq!(decoded.graph_index[0].svg_path.as_deref(), Some("graphs/foo.svg"));
    }
}
