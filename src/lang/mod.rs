pub mod python;

use crate::fs::SourceFile;
use crate::ir::AnalysisCache;
use anyhow::Result;

pub trait LanguageFrontend {
    fn parse_files(&self, files: &[SourceFile]) -> Result<AnalysisCache>;
}
