use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyzeConfig {
    pub lang: String,
    pub input: PathBuf,
    pub out: PathBuf,
    pub max_loop_unroll: usize,
    pub max_paths: usize,
    pub max_path_len: usize,
    pub top_n: usize,
    pub emit_full_dot: bool,
    pub render_full_svg: bool,
    pub fail_on_parse_error: bool,
    pub parallelism: String,
    pub exclude: Vec<String>,
    pub stub_paths: Vec<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
struct RawAnalyzeConfig {
    lang: Option<String>,
    input: Option<PathBuf>,
    out: Option<PathBuf>,
    max_loop_unroll: Option<usize>,
    max_paths: Option<usize>,
    max_path_len: Option<usize>,
    top_n: Option<usize>,
    emit_full_dot: Option<bool>,
    render_full_svg: Option<bool>,
    fail_on_parse_error: Option<bool>,
    parallelism: Option<String>,
    exclude: Option<Vec<String>>,
    stub_paths: Option<Vec<PathBuf>>,
}

impl Default for AnalyzeConfig {
    fn default() -> Self {
        Self {
            lang: "python".to_string(),
            input: PathBuf::from("."),
            out: PathBuf::from("report"),
            max_loop_unroll: 2,
            max_paths: 1000,
            max_path_len: 500,
            top_n: 100,
            emit_full_dot: false,
            render_full_svg: false,
            fail_on_parse_error: false,
            parallelism: "auto".to_string(),
            exclude: vec![
                "**/.venv/**".to_string(),
                "**/venv/**".to_string(),
                "**/__pycache__/**".to_string(),
                "**/.pytest_cache/**".to_string(),
                "**/site-packages/**".to_string(),
                "**/build/**".to_string(),
                "**/dist/**".to_string(),
            ],
            stub_paths: Vec::new(),
        }
    }
}

impl AnalyzeConfig {
    pub fn from_toml_file(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        let raw: RawAnalyzeConfig = toml::from_str(&text)
            .with_context(|| format!("failed to parse config file {}", path.display()))?;
        let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
        Ok(raw.into_config(base_dir))
    }

    pub fn apply_cli_overrides(
        &mut self,
        lang: Option<String>,
        input: Option<PathBuf>,
        out: Option<PathBuf>,
    ) {
        if let Some(lang) = lang {
            self.lang = lang;
        }
        if let Some(input) = input {
            self.input = input;
        }
        if let Some(out) = out {
            self.out = out;
        }
    }

    pub fn parallelism_threads(&self) -> Option<usize> {
        match self.parallelism.trim() {
            "" | "auto" => None,
            value => value.parse::<usize>().ok().filter(|threads| *threads > 0),
        }
    }
}

impl RawAnalyzeConfig {
    fn into_config(self, base_dir: &Path) -> AnalyzeConfig {
        let defaults = AnalyzeConfig::default();

        AnalyzeConfig {
            lang: self.lang.unwrap_or(defaults.lang),
            input: resolve_config_path(base_dir, self.input.unwrap_or(defaults.input)),
            out: resolve_config_path(base_dir, self.out.unwrap_or(defaults.out)),
            max_loop_unroll: self.max_loop_unroll.unwrap_or(defaults.max_loop_unroll),
            max_paths: self.max_paths.unwrap_or(defaults.max_paths),
            max_path_len: self.max_path_len.unwrap_or(defaults.max_path_len),
            top_n: self.top_n.unwrap_or(defaults.top_n),
            emit_full_dot: self.emit_full_dot.unwrap_or(defaults.emit_full_dot),
            render_full_svg: self.render_full_svg.unwrap_or(defaults.render_full_svg),
            fail_on_parse_error: self
                .fail_on_parse_error
                .unwrap_or(defaults.fail_on_parse_error),
            parallelism: self.parallelism.unwrap_or(defaults.parallelism),
            exclude: self.exclude.unwrap_or(defaults.exclude),
            stub_paths: self
                .stub_paths
                .unwrap_or(defaults.stub_paths)
                .into_iter()
                .map(|path| resolve_config_path(base_dir, path))
                .collect(),
        }
    }
}

fn resolve_config_path(base_dir: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    }
}
