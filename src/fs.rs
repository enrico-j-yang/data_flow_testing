use crate::config::AnalyzeConfig;
use anyhow::{Context, Result};
use globset::{Glob, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceFile {
    pub absolute_path: PathBuf,
    pub relative_path: String,
}

pub fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub fn discover_sources(config: &AnalyzeConfig) -> Result<Vec<SourceFile>> {
    let root = config
        .input
        .canonicalize()
        .with_context(|| format!("input path does not exist: {}", config.input.display()))?;
    let mut builder = GlobSetBuilder::new();
    for pattern in &config.exclude {
        builder
            .add(Glob::new(pattern).with_context(|| format!("invalid exclude glob {pattern}"))?);
    }
    let excludes = builder.build()?;
    let mut files = Vec::new();

    for entry in WalkDir::new(&root).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let rel = path.strip_prefix(&root).unwrap_or(path);
        if excludes.is_match(rel) || path.extension().and_then(|s| s.to_str()) != Some("py") {
            continue;
        }
        files.push(SourceFile {
            absolute_path: path.to_path_buf(),
            relative_path: normalize_path(rel),
        });
    }

    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(files)
}
