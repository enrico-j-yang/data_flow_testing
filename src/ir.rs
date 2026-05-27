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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AnalysisCache {
    pub schema_version: u32,
    pub tool_version: String,
    pub files: Vec<SourceFileRecord>,
    pub definitions: Vec<Definition>,
    pub uses: Vec<Use>,
    pub diagnostics: Vec<Diagnostic>,
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
