use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceSpan {
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub snippet: String,
}

impl SourceSpan {
    pub fn synthetic(file: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            file: file.into(),
            line: 0,
            col: 0,
            end_line: 0,
            end_col: 0,
            snippet: label.into(),
        }
    }
}
