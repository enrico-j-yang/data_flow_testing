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
