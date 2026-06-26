pub mod python;

use crate::ir::AnalysisCache;
use crate::source::SourceUnit;
use anyhow::Result;

pub trait LanguageFrontend {
    fn parse_units(&self, units: &[SourceUnit]) -> Result<AnalysisCache>;
}
