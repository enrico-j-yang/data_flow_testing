use crate::config::AnalyzeConfig;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CProject {
    pub source_dir: PathBuf,
    pub relative_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredProject {
    pub project: CProject,
    pub build_dir: PathBuf,
    pub compile_commands_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompileCommand {
    pub directory: PathBuf,
    pub file: PathBuf,
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub output: Option<PathBuf>,
}

pub fn discover_cmake_projects(config: &AnalyzeConfig) -> Result<Vec<CProject>> {
    let root = config
        .input
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", config.input.display()))?;

    let mut projects = Vec::new();
    for entry in WalkDir::new(&root).min_depth(0).max_depth(6) {
        let entry = entry?;
        if !entry.file_type().is_file() || entry.file_name() != OsStr::new("CMakeLists.txt") {
            continue;
        }
        let source_dir = entry
            .path()
            .parent()
            .context("CMakeLists.txt without a parent directory")?
            .to_path_buf();
        let relative_name = relative_name(&root, &source_dir);
        projects.push(CProject {
            source_dir,
            relative_name,
        });
    }

    projects.sort_by(|left, right| left.relative_name.cmp(&right.relative_name));
    Ok(projects)
}

pub fn configure_cmake_projects(
    projects: &[CProject],
    config: &AnalyzeConfig,
) -> Result<Vec<ConfiguredProject>> {
    let build_root = config
        .build_root
        .clone()
        .unwrap_or_else(|| config.out.join("cmake-build"));
    fs::create_dir_all(&build_root)?;

    let mut configured = Vec::new();
    for project in projects {
        let build_dir = build_root.join(project.relative_name.replace('/', "__"));
        fs::create_dir_all(&build_dir)?;

        let mut command = Command::new("cmake");
        command
            .arg("-S")
            .arg(&project.source_dir)
            .arg("-B")
            .arg(&build_dir)
            .arg("-DCMAKE_EXPORT_COMPILE_COMMANDS=ON");
        for extra in &config.cmake_args {
            command.arg(extra);
        }

        let output = command.output().with_context(|| {
            format!(
                "failed to configure cmake project {}",
                project.source_dir.display()
            )
        })?;
        if !output.status.success() {
            bail!(
                "cmake configure failed for {}:\n{}",
                project.source_dir.display(),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        configured.push(ConfiguredProject {
            project: project.clone(),
            build_dir: build_dir.clone(),
            compile_commands_path: build_dir.join("compile_commands.json"),
        });
    }

    Ok(configured)
}

pub fn merge_compile_commands(
    paths: &[PathBuf],
    out_path: &Path,
) -> Result<Vec<CompileCommand>> {
    let mut merged = BTreeMap::<(String, String, String), CompileCommand>::new();
    for path in paths {
        let text = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let commands: Vec<CompileCommand> = serde_json::from_str(&text)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        for command in commands {
            let args_key = if command.arguments.is_empty() {
                command.command.clone().unwrap_or_default()
            } else {
                command.arguments.join("\u{1f}")
            };
            let directory_key = canonicalize_for_key(&command.directory);
            let file_key = canonicalize_for_key(&command.file);
            let key = (file_key, directory_key, args_key);
            merged.entry(key).or_insert(command);
        }
    }

    let result = merged.into_values().collect::<Vec<_>>();
    let json = serde_json::to_string_pretty(&result)?;
    fs::write(out_path, json)
        .with_context(|| format!("failed to write {}", out_path.display()))?;
    Ok(result)
}

fn relative_name(root: &Path, source_dir: &Path) -> String {
    let rel = source_dir
        .strip_prefix(root)
        .unwrap_or(source_dir)
        .to_string_lossy()
        .replace('\\', "/");
    if rel.is_empty() {
        ".".to_string()
    } else {
        rel
    }
}

fn canonicalize_for_key(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}
