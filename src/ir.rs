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
    pub definitions: Vec<Definition>,
    pub uses: Vec<Use>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Default for AnalysisCache {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            tool_version: String::new(),
            files: Vec::new(),
            definitions: Vec::new(),
            uses: Vec::new(),
            diagnostics: Vec::new(),
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

#[cfg(test)]
mod tests {
    use super::{
        AnalysisCache, Definition, Diagnostic, Place, SCHEMA_VERSION, SourceFileRecord, Use,
    };
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
            diagnostics: vec![Diagnostic {
                diagnostic_id: "G_deadbeef".to_string(),
                severity: "warning".to_string(),
                kind: "parse".to_string(),
                message: "example".to_string(),
                file: "app/main.py".to_string(),
                span,
            }],
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
}
