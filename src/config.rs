use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
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
        let mut cfg: AnalyzeConfig = toml::from_str(&text)
            .with_context(|| format!("failed to parse config file {}", path.display()))?;
        cfg.fill_missing_defaults();
        Ok(cfg)
    }

    fn fill_missing_defaults(&mut self) {
        let defaults = AnalyzeConfig::default();
        if self.lang.is_empty() {
            self.lang = defaults.lang;
        }
        if self.max_loop_unroll == 0 {
            self.max_loop_unroll = defaults.max_loop_unroll;
        }
        if self.max_paths == 0 {
            self.max_paths = defaults.max_paths;
        }
        if self.max_path_len == 0 {
            self.max_path_len = defaults.max_path_len;
        }
        if self.top_n == 0 {
            self.top_n = defaults.top_n;
        }
        if self.exclude.is_empty() {
            self.exclude = defaults.exclude;
        }
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
}
