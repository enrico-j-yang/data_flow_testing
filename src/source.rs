use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LineMarker {
    pub generated_line: usize,
    pub original_file: String,
    pub original_line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceUnit {
    pub absolute_path: PathBuf,
    pub relative_path: String,
    pub source_text: String,
    pub original_path: Option<PathBuf>,
    pub line_markers: Vec<LineMarker>,
}

#[cfg(test)]
mod tests {
    use super::SourceSpan;

    #[test]
    fn synthetic_span_uses_zero_offsets_and_label_snippet() {
        let span = SourceSpan::synthetic("generated.py", "<module>");

        assert_eq!(
            span,
            SourceSpan {
                file: "generated.py".to_string(),
                line: 0,
                col: 0,
                end_line: 0,
                end_col: 0,
                snippet: "<module>".to_string(),
            }
        );
    }
}
