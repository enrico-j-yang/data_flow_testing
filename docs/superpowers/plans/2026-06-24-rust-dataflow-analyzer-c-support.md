# C Language Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add first-version C support that auto-generates compile databases from CMake projects, preprocesses translation units with real compile context, reuses the shared IR/CFG/data-flow pipeline, and emits the same report artifacts as Python.

**Architecture:** Keep the current shared core (`ir`, `cfg`, `analysis`, `summaries`, `paths`, `report`) intact and add a C pipeline at the boundary: CMake project discovery, compile database generation, preprocessing, C AST lowering, and project-local symbol resolution. Python keeps using the existing flow, but the frontend input abstraction is widened so both Python and C can feed the same analysis cache builder.

**Tech Stack:** Rust, `clap`, `serde`, `serde_json`, `csv`, `toml`, `walkdir`, `globset`, `rayon`, `tree-sitter`, `tree-sitter-python`, `tree-sitter-c`, `shlex`, `tempfile`, CMake, a host C compiler, optional Graphviz `dot`.

---

## Scope Check

The approved spec describes one cohesive feature rather than several unrelated
subsystems. This plan breaks the work into vertical slices that stay buildable:
CLI/config scaffolding, compile database generation, preprocessing and shared
source units, C lowering, advanced C places and CFG, cross-file symbol
resolution, and end-to-end integration with reports and path queries.

## File Structure

- Modify `Cargo.toml`: add C frontend and compile command parsing dependencies.
- Modify `src/lib.rs`: export the new C-related modules.
- Modify `src/cli.rs`: accept C-specific analyze options and route `--lang c`.
- Modify `src/config.rs`: persist C build/preprocess options in config.
- Modify `src/lang/mod.rs`: switch the frontend trait to shared `SourceUnit`
  inputs and export `lang::c`.
- Modify `src/lang/python.rs`: parse `SourceUnit` instead of raw `SourceFile`.
- Modify `src/source.rs`: add `SourceUnit` and line-marker/source-map metadata.
- Create `src/cbuild.rs`: discover CMake projects, configure them, and merge
  `compile_commands.json`.
- Create `src/ccompile.rs`: normalize compile commands, build preprocess
  invocations, parse line markers, and prepare cached `SourceUnit`s.
- Create `src/lang/c.rs`: parse preprocessed C with tree-sitter and lower to
  shared IR plus baseline CFG.
- Create `src/csymbols.rs`: resolve project-local C declarations/definitions and
  fill call candidate sets for summary propagation.
- Modify `README.md`: document the C analyze workflow and build-root options.
- Create `tests/c_compile_commands.rs`: compile database and preprocessing unit
  tests.
- Create `tests/c_frontend.rs`: C lowering, place normalization, CFG, and
  summary propagation tests.
- Create `tests/c_integration_report.rs`: C CLI/report integration tests and the
  ignored LSChat acceptance test.
- Modify `tests/cli_smoke.rs`: cover new C analyze help flags.
- Modify `tests/config_discovery.rs`: cover C config parsing and overrides.
- Modify `tests/python_frontend.rs`: migrate helpers to `SourceUnit` so Python
  remains green while the trait changes.

---

### Task 1: Add C CLI Flags And Config Scaffolding

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/cli.rs`
- Modify: `src/config.rs`
- Modify: `tests/cli_smoke.rs`
- Modify: `tests/config_discovery.rs`

- [ ] **Step 1: Write the failing CLI/config tests**

Append to `tests/cli_smoke.rs`:

```rust
#[test]
fn analyze_help_mentions_c_build_flags() {
    let mut cmd = Command::cargo_bin("data-flow-analyzer").unwrap();
    cmd.args(["analyze", "--help"])
        .assert()
        .success()
        .stdout(contains("--build-root"))
        .stdout(contains("--cmake-arg"))
        .stdout(contains("--keep-preprocessed"));
}
```

Append to `tests/config_discovery.rs`:

```rust
#[test]
fn config_file_loads_c_build_options() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("dataflow.toml");
    fs::write(
        &cfg_path,
        r#"
lang = "c"
input = "tests"
out = "report"
build_root = "build/cmake"
cmake_args = ["-DLISA_BASE=/opt/lisa", "-DDEFCONF_FILE=prj-linux.conf"]
keep_preprocessed = true
c_project_globs = ["session_*", "stream_text/**"]
"#,
    )
    .unwrap();

    let cfg = AnalyzeConfig::from_toml_file(&cfg_path).unwrap();

    assert_eq!(cfg.lang, "c");
    assert_eq!(
        cfg.build_root,
        Some(cfg_path.parent().unwrap().join("build/cmake"))
    );
    assert_eq!(
        cfg.cmake_args,
        vec![
            "-DLISA_BASE=/opt/lisa".to_string(),
            "-DDEFCONF_FILE=prj-linux.conf".to_string(),
        ]
    );
    assert!(cfg.keep_preprocessed);
    assert_eq!(
        cfg.c_project_globs,
        vec!["session_*".to_string(), "stream_text/**".to_string()]
    );
}
```

- [ ] **Step 2: Run the tests and verify they fail**

Run: `cargo test --test cli_smoke analyze_help_mentions_c_build_flags --test config_discovery config_file_loads_c_build_options`

Expected: FAIL because the CLI does not expose those flags and `AnalyzeConfig`
does not yet parse the C-specific keys.

- [ ] **Step 3: Implement the minimal CLI/config changes**

Run:

```bash
cargo add tree-sitter-c shlex
```

Update the `Analyze` subcommand in `src/cli.rs`:

```rust
    Analyze {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        lang: Option<String>,
        #[arg(long)]
        input: Option<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        build_root: Option<PathBuf>,
        #[arg(long = "cmake-arg")]
        cmake_args: Vec<String>,
        #[arg(long, default_value_t = false)]
        keep_preprocessed: bool,
    },
```

Update the `run()` match arm in `src/cli.rs`:

```rust
        Some(Commands::Analyze {
            config,
            lang,
            input,
            out,
            build_root,
            cmake_args,
            keep_preprocessed,
        }) => run_analyze(
            config,
            lang,
            input,
            out,
            build_root,
            cmake_args,
            keep_preprocessed,
        ),
```

Extend `AnalyzeConfig` in `src/config.rs`:

```rust
    pub build_root: Option<PathBuf>,
    pub cmake_args: Vec<String>,
    pub keep_preprocessed: bool,
    pub c_project_globs: Vec<String>,
```

Extend `RawAnalyzeConfig` in `src/config.rs`:

```rust
    build_root: Option<PathBuf>,
    cmake_args: Option<Vec<String>>,
    keep_preprocessed: Option<bool>,
    c_project_globs: Option<Vec<String>>,
```

Set defaults in `impl Default for AnalyzeConfig`:

```rust
            build_root: None,
            cmake_args: Vec::new(),
            keep_preprocessed: false,
            c_project_globs: vec!["**/CMakeLists.txt".to_string()],
```

Update `apply_cli_overrides` in `src/config.rs`:

```rust
    pub fn apply_cli_overrides(
        &mut self,
        lang: Option<String>,
        input: Option<PathBuf>,
        out: Option<PathBuf>,
        build_root: Option<PathBuf>,
        cmake_args: Vec<String>,
        keep_preprocessed: bool,
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
        if let Some(build_root) = build_root {
            self.build_root = Some(build_root);
        }
        if !cmake_args.is_empty() {
            self.cmake_args = cmake_args;
        }
        if keep_preprocessed {
            self.keep_preprocessed = true;
        }
    }
```

Resolve `build_root` in `RawAnalyzeConfig::into_config`:

```rust
            build_root: self
                .build_root
                .map(|path| resolve_config_path(base_dir, path)),
            cmake_args: self.cmake_args.unwrap_or_default(),
            keep_preprocessed: self.keep_preprocessed.unwrap_or(false),
            c_project_globs: self.c_project_globs.unwrap_or_else(|| {
                vec!["**/CMakeLists.txt".to_string()]
            }),
```

- [ ] **Step 4: Run the tests and verify they pass**

Run: `cargo test --test cli_smoke analyze_help_mentions_c_build_flags --test config_discovery config_file_loads_c_build_options`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/cli.rs src/config.rs tests/cli_smoke.rs tests/config_discovery.rs
git commit -m "feat: add C analyze config scaffolding"
```

---

### Task 2: Discover CMake Projects And Merge Compile Commands

**Files:**
- Create: `src/cbuild.rs`
- Modify: `src/lib.rs`
- Create: `tests/c_compile_commands.rs`

- [ ] **Step 1: Write the failing compile database tests**

Create `tests/c_compile_commands.rs`:

```rust
use data_flow_analyzer::cbuild::{
    configure_cmake_projects, discover_cmake_projects, merge_compile_commands, CProject,
};
use data_flow_analyzer::config::AnalyzeConfig;
use std::fs;
use std::process::Command;

#[test]
fn discover_cmake_projects_finds_cmake_roots() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("alpha")).unwrap();
    fs::create_dir_all(dir.path().join("beta/nested")).unwrap();
    fs::write(
        dir.path().join("alpha/CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.20)\nproject(alpha C)\nadd_executable(alpha main.c)\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("beta/CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.20)\nproject(beta C)\nadd_executable(beta main.c)\n",
    )
    .unwrap();

    let cfg = AnalyzeConfig {
        lang: "c".to_string(),
        input: dir.path().to_path_buf(),
        out: dir.path().join("out"),
        ..AnalyzeConfig::default()
    };

    let projects = discover_cmake_projects(&cfg).unwrap();
    let names = projects
        .iter()
        .map(|project| project.relative_name.clone())
        .collect::<Vec<_>>();

    assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);
}

#[test]
fn merge_compile_commands_deduplicates_and_sorts_entries() {
    let dir = tempfile::tempdir().unwrap();
    let left = dir.path().join("left.json");
    let right = dir.path().join("right.json");
    fs::write(
        &left,
        r#"[{"directory":"/tmp/b","file":"/tmp/b/b.c","arguments":["cc","-c","/tmp/b/b.c"]}]"#,
    )
    .unwrap();
    fs::write(
        &right,
        r#"[{"directory":"/tmp/a","file":"/tmp/a/a.c","arguments":["cc","-c","/tmp/a/a.c"]},{"directory":"/tmp/b","file":"/tmp/b/b.c","arguments":["cc","-c","/tmp/b/b.c"]}]"#,
    )
    .unwrap();

    let merged = merge_compile_commands(&[left, right], &dir.path().join("merged.json")).unwrap();

    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].file.to_string_lossy(), "/tmp/a/a.c");
    assert_eq!(merged[1].file.to_string_lossy(), "/tmp/b/b.c");
    assert!(dir.path().join("merged.json").exists());
}

#[test]
fn configure_cmake_projects_exports_compile_commands_for_simple_project() {
    if Command::new("cmake").arg("--version").output().is_err() {
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.20)\nproject(sample C)\nadd_executable(sample main.c)\n",
    )
    .unwrap();
    fs::write(dir.path().join("main.c"), "int main(void) { return 0; }\n").unwrap();

    let cfg = AnalyzeConfig {
        lang: "c".to_string(),
        input: dir.path().to_path_buf(),
        out: dir.path().join("out"),
        build_root: Some(dir.path().join("build")),
        ..AnalyzeConfig::default()
    };

    let projects = vec![CProject {
        source_dir: dir.path().to_path_buf(),
        relative_name: ".".to_string(),
    }];
    let configured = configure_cmake_projects(&projects, &cfg).unwrap();

    assert_eq!(configured.len(), 1);
    assert!(configured[0].compile_commands_path.exists());
}
```

- [ ] **Step 2: Run the tests and verify they fail**

Run: `cargo test --test c_compile_commands discover_cmake_projects_finds_cmake_roots --test c_compile_commands merge_compile_commands_deduplicates_and_sorts_entries --test c_compile_commands configure_cmake_projects_exports_compile_commands_for_simple_project`

Expected: FAIL because `src/cbuild.rs` and its public API do not exist yet.

- [ ] **Step 3: Implement CMake discovery and compile database merge**

Create `src/cbuild.rs`:

```rust
use crate::config::AnalyzeConfig;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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
    let root = config.input.canonicalize()?;
    let mut projects = Vec::new();
    for entry in WalkDir::new(&root).min_depth(0).max_depth(6) {
        let entry = entry?;
        if !entry.file_type().is_file() || entry.file_name() != "CMakeLists.txt" {
            continue;
        }
        let source_dir = entry.path().parent().unwrap().to_path_buf();
        let rel = source_dir
            .strip_prefix(&root)
            .unwrap_or(&source_dir)
            .to_string_lossy()
            .replace('\\', "/");
        projects.push(CProject {
            source_dir,
            relative_name: if rel.is_empty() { ".".to_string() } else { rel },
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
        let mut cmd = Command::new("cmake");
        cmd.arg("-S")
            .arg(&project.source_dir)
            .arg("-B")
            .arg(&build_dir)
            .arg("-DCMAKE_EXPORT_COMPILE_COMMANDS=ON");
        for extra in &config.cmake_args {
            cmd.arg(extra);
        }
        let output = cmd.output().with_context(|| {
            format!("failed to configure cmake project {}", project.source_dir.display())
        })?;
        if !output.status.success() {
            bail!(
                "cmake configure failed for {}:\n{}",
                project.source_dir.display(),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let compile_commands_path = build_dir.join("compile_commands.json");
        configured.push(ConfiguredProject {
            project: project.clone(),
            build_dir,
            compile_commands_path,
        });
    }

    Ok(configured)
}

pub fn merge_compile_commands(paths: &[PathBuf], out_path: &Path) -> Result<Vec<CompileCommand>> {
    let mut merged = BTreeMap::<(String, String, String), CompileCommand>::new();
    for path in paths {
        let text = fs::read_to_string(path)?;
        let commands: Vec<CompileCommand> = serde_json::from_str(&text)?;
        for command in commands {
            let args_key = if command.arguments.is_empty() {
                command.command.clone().unwrap_or_default()
            } else {
                command.arguments.join("\u{1f}")
            };
            let key = (
                command.file.to_string_lossy().to_string(),
                command.directory.to_string_lossy().to_string(),
                args_key,
            );
            merged.entry(key).or_insert(command);
        }
    }

    let result = merged.into_values().collect::<Vec<_>>();
    let json = serde_json::to_string_pretty(&result)?;
    fs::write(out_path, json)?;
    Ok(result)
}
```

Export the module from `src/lib.rs`:

```rust
pub mod cbuild;
```

- [ ] **Step 4: Run the tests and verify they pass**

Run: `cargo test --test c_compile_commands`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/cbuild.rs src/lib.rs tests/c_compile_commands.rs
git commit -m "feat: generate merged C compile databases"
```

---

### Task 3: Introduce Shared Source Units And Preprocess Preparation

**Files:**
- Create: `src/ccompile.rs`
- Modify: `src/source.rs`
- Modify: `src/lang/mod.rs`
- Modify: `src/lang/python.rs`
- Modify: `tests/c_compile_commands.rs`
- Modify: `tests/python_frontend.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write the failing preprocessing and source-unit tests**

Append to `tests/c_compile_commands.rs`:

```rust
use data_flow_analyzer::cbuild::CompileCommand;
use data_flow_analyzer::ccompile::{build_preprocess_arguments, parse_line_markers};

#[test]
fn build_preprocess_arguments_rewrites_compile_invocation() {
    let command = CompileCommand {
        directory: std::path::PathBuf::from("/tmp/project"),
        file: std::path::PathBuf::from("/tmp/project/main.c"),
        arguments: vec![
            "cc".to_string(),
            "-Iinclude".to_string(),
            "-DVALUE=1".to_string(),
            "-c".to_string(),
            "main.c".to_string(),
            "-o".to_string(),
            "main.o".to_string(),
        ],
        command: None,
        output: Some(std::path::PathBuf::from("main.o")),
    };

    let args = build_preprocess_arguments(&command, std::path::Path::new("main.i")).unwrap();

    assert_eq!(args[0], "cc");
    assert!(args.iter().any(|arg| arg == "-E"));
    assert!(args.iter().any(|arg| arg == "-Iinclude"));
    assert!(args.iter().any(|arg| arg == "-DVALUE=1"));
    assert!(!args.iter().any(|arg| arg == "-c"));
    assert!(!args.iter().any(|arg| arg == "main.o"));
}

#[test]
fn parse_line_markers_maps_original_files() {
    let text = r#"# 1 "/tmp/project/main.c"
int main(void) {
# 12 "/tmp/project/include/value.h"
  return VALUE;
# 3 "/tmp/project/main.c"
}
"#;

    let markers = parse_line_markers(text);

    assert_eq!(markers.len(), 3);
    assert_eq!(markers[0].generated_line, 1);
    assert_eq!(markers[0].original_file, "/tmp/project/main.c");
    assert_eq!(markers[1].original_line, 12);
}
```

In `tests/python_frontend.rs`, change the helper to fail against the new trait:

```rust
fn parse_python(source_text: &str) -> AnalysisCache {
    let unit = data_flow_analyzer::source::SourceUnit {
        absolute_path: std::path::PathBuf::from("sample.py"),
        relative_path: "sample.py".to_string(),
        source_text: source_text.to_string(),
        original_path: None,
        line_markers: Vec::new(),
    };

    PythonFrontend::new().parse_units(&[unit]).unwrap()
}
```

- [ ] **Step 2: Run the tests and verify they fail**

Run: `cargo test --test c_compile_commands build_preprocess_arguments_rewrites_compile_invocation --test c_compile_commands parse_line_markers_maps_original_files --test python_frontend python_frontend_extracts_core_ir`

Expected: FAIL because `src/ccompile.rs`, `SourceUnit`, and `LanguageFrontend::parse_units`
do not exist yet.

- [ ] **Step 3: Implement shared source units and preprocess helpers**

Create `src/ccompile.rs`:

```rust
use crate::cbuild::CompileCommand;
use crate::source::{LineMarker, SourceUnit};
use anyhow::{bail, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub fn build_preprocess_arguments(
    command: &CompileCommand,
    output_path: &Path,
) -> Result<Vec<String>> {
    let raw_args = if command.arguments.is_empty() {
        command
            .command
            .as_deref()
            .and_then(shlex::split)
            .ok_or_else(|| anyhow::anyhow!("compile command is empty"))?
    } else {
        command.arguments.clone()
    };
    let compiler = raw_args
        .first()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("compile command is empty"))?;
    let mut args = vec![compiler, "-E".to_string(), "-dD".to_string()];
    let mut skip_next = false;

    for arg in raw_args.iter().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }
        match arg.as_str() {
            "-c" => {}
            "-o" => skip_next = true,
            _ => args.push(arg.clone()),
        }
    }

    args.push("-o".to_string());
    args.push(output_path.to_string_lossy().to_string());
    Ok(args)
}

pub fn parse_line_markers(text: &str) -> Vec<LineMarker> {
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let trimmed = line.trim_start();
            if !trimmed.starts_with('#') {
                return None;
            }
            let mut parts = shlex::split(trimmed.replace('#', "").trim()).ok()?;
            if parts.len() < 2 {
                return None;
            }
            let original_line = parts.remove(0).parse::<usize>().ok()?;
            let original_file = parts.remove(0);
            Some(LineMarker {
                generated_line: index + 1,
                original_file,
                original_line,
            })
        })
        .collect()
}

pub fn load_preprocessed_unit(
    source_path: &Path,
    preprocessed_path: &Path,
) -> Result<SourceUnit> {
    let source_text = fs::read_to_string(preprocessed_path)?;
    Ok(SourceUnit {
        absolute_path: source_path.to_path_buf(),
        relative_path: source_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
        source_text: source_text.clone(),
        original_path: Some(source_path.to_path_buf()),
        line_markers: parse_line_markers(&source_text),
    })
}
```

Update `src/source.rs`:

```rust
use std::path::PathBuf;

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
```

Update `src/lang/mod.rs`:

```rust
pub mod python;

use crate::ir::AnalysisCache;
use crate::source::SourceUnit;
use anyhow::Result;

pub trait LanguageFrontend {
    fn parse_units(&self, units: &[SourceUnit]) -> Result<AnalysisCache>;
}
```

Update the Python frontend entry point in `src/lang/python.rs`:

```rust
impl LanguageFrontend for PythonFrontend {
    fn parse_units(&self, units: &[SourceUnit]) -> Result<AnalysisCache> {
        let mut partials = units
            .iter()
            .map(|unit| -> Result<(String, AnalysisCache)> {
                let cache = self.parse_single_unit(unit)?;
                let key = cache
                    .files
                    .first()
                    .map(|record| record.path.clone())
                    .unwrap_or_else(|| unit.relative_path.clone());
                Ok((key, cache))
            })
            .collect::<Result<Vec<_>>>()?;
        partials.sort_by(|left, right| left.0.cmp(&right.0));

        let mut cache = AnalysisCache {
            tool_version: env!("CARGO_PKG_VERSION").to_string(),
            ..AnalysisCache::default()
        };

        for (_, partial) in partials {
            merge_analysis_cache(&mut cache, partial);
        }

        Ok(cache)
    }
}

impl PythonFrontend {
    fn parse_single_unit(&self, unit: &SourceUnit) -> Result<AnalysisCache> {
        let file = crate::fs::SourceFile {
            absolute_path: unit.absolute_path.clone(),
            relative_path: unit.relative_path.clone(),
        };
        self.parse_single_file(&file, &unit.source_text)
    }
}
```

Change the existing helper signature in `src/lang/python.rs` from:

```rust
fn parse_single_file(&self, file: &SourceFile) -> Result<AnalysisCache>
```

to:

```rust
fn parse_single_file(&self, file: &SourceFile, source: &str) -> Result<AnalysisCache>
```

and remove the current `fs::read_to_string(&file.absolute_path)` call so the
rest of the Python lowering logic reads from the provided `source` text.

Export the module from `src/lib.rs`:

```rust
pub mod ccompile;
```

- [ ] **Step 4: Run the tests and verify they pass**

Run: `cargo test --test c_compile_commands build_preprocess_arguments_rewrites_compile_invocation --test c_compile_commands parse_line_markers_maps_original_files --test python_frontend python_frontend_extracts_core_ir`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/ccompile.rs src/source.rs src/lang/mod.rs src/lang/python.rs src/lib.rs tests/c_compile_commands.rs tests/python_frontend.rs
git commit -m "refactor: share source units across language frontends"
```

---

### Task 4: Parse Basic C Translation Units Into Shared IR

**Files:**
- Create: `src/lang/c.rs`
- Modify: `src/lang/mod.rs`
- Create: `tests/c_frontend.rs`

- [ ] **Step 1: Write the failing basic C frontend tests**

Create `tests/c_frontend.rs`:

```rust
use data_flow_analyzer::ir::{AnalysisCache, Place};
use data_flow_analyzer::lang::c::CFrontend;
use data_flow_analyzer::lang::LanguageFrontend;
use data_flow_analyzer::source::SourceUnit;

fn parse_c(source_text: &str) -> AnalysisCache {
    let unit = SourceUnit {
        absolute_path: std::path::PathBuf::from("sample.c"),
        relative_path: "sample.c".to_string(),
        source_text: source_text.to_string(),
        original_path: None,
        line_markers: Vec::new(),
    };

    CFrontend::new().parse_units(&[unit]).unwrap()
}

#[test]
fn c_frontend_extracts_functions_params_and_returns() {
    let cache = parse_c(
        r#"
int add_one(int value) {
    int next = value + 1;
    return next;
}
"#,
    );

    let function = cache
        .functions
        .iter()
        .find(|item| item.qualified_name == "add_one")
        .unwrap();

    assert_eq!(function.params, vec!["value".to_string()]);
    assert!(cache.definitions.iter().any(|definition| {
        definition.function_id.as_deref() == Some(function.function_id.as_str())
            && definition.def_kind == "assign"
            && matches!(
                &definition.place,
                Place::Local { scope_id, name }
                    if scope_id == &function.scope_id && name == "next"
            )
    }));
    assert!(cache.uses.iter().any(|use_site| {
        use_site.function_id.as_deref() == Some(function.function_id.as_str())
            && use_site.context == "return value"
    }));
}

#[test]
fn c_frontend_records_global_and_local_assignments() {
    let cache = parse_c(
        r#"
int counter = 0;

int bump(int delta) {
    counter = counter + delta;
    return counter;
}
"#,
    );

    let module = cache.modules.first().unwrap();
    assert!(cache.definitions.iter().any(|definition| {
        definition.function_id.is_none()
            && matches!(
                &definition.place,
                Place::Global { module_id, name }
                    if module_id == &module.module_id && name == "counter"
            )
    }));
}
```

- [ ] **Step 2: Run the tests and verify they fail**

Run: `cargo test --test c_frontend c_frontend_extracts_functions_params_and_returns --test c_frontend c_frontend_records_global_and_local_assignments`

Expected: FAIL because `src/lang/c.rs` and `lang::c::CFrontend` do not exist yet.

- [ ] **Step 3: Implement the minimal C frontend**

Create `src/lang/c.rs`:

```rust
use crate::ids::stable_id;
use crate::ir::{
    AnalysisCache, Definition, FunctionRecord, ModuleRecord, Place, ScopeRecord, SourceFileRecord,
    SCHEMA_VERSION, Use,
};
use crate::lang::LanguageFrontend;
use crate::source::{SourceSpan, SourceUnit};
use anyhow::{Context, Result};
use tree_sitter::{Node, Parser};

pub struct CFrontend;

impl CFrontend {
    pub fn new() -> Self {
        Self
    }

    fn parser() -> Result<Parser> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_c::LANGUAGE.into())
            .context("failed to load tree-sitter-c")?;
        Ok(parser)
    }
}

impl LanguageFrontend for CFrontend {
    fn parse_units(&self, units: &[SourceUnit]) -> Result<AnalysisCache> {
        let mut parser = Self::parser()?;
        let mut cache = AnalysisCache::default();

        for unit in units {
            let tree = parser
                .parse(&unit.source_text, None)
                .context("c parse returned no tree")?;
            let file_id = stable_id("FILE", SCHEMA_VERSION, &[&unit.relative_path]);
            let module_id = stable_id("M", SCHEMA_VERSION, &[&unit.relative_path]);
            cache.files.push(SourceFileRecord {
                file_id: file_id.clone(),
                path: unit.relative_path.clone(),
                hash: stable_id("HASH", SCHEMA_VERSION, &[&unit.source_text]),
                line_count: unit.source_text.lines().count(),
                parse_status: "ok".to_string(),
            });
            cache.modules.push(ModuleRecord {
                module_id: module_id.clone(),
                file_id: file_id.clone(),
                module_name: unit
                    .relative_path
                    .trim_end_matches(".c")
                    .replace('/', "::"),
                exports: Vec::new(),
                imports: Vec::new(),
            });
            lower_translation_unit(
                &unit.relative_path,
                &unit.source_text,
                tree.root_node(),
                &module_id,
                &mut cache,
            );
        }

        Ok(cache)
    }
}

fn lower_translation_unit(
    path: &str,
    text: &str,
    root: Node,
    module_id: &str,
    cache: &mut AnalysisCache,
) {
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        match child.kind() {
            "function_definition" => lower_function(path, text, module_id, child, cache),
            "declaration" => lower_global(path, text, module_id, child, cache),
            _ => {}
        }
    }
}

fn lower_function(
    path: &str,
    text: &str,
    module_id: &str,
    node: Node,
    cache: &mut AnalysisCache,
) {
    let span = span_for(path, text, node);
    let function_name = node
        .child_by_field_name("declarator")
        .and_then(|decl| decl.utf8_text(text.as_bytes()).ok())
        .unwrap_or("anonymous")
        .split('(')
        .next()
        .unwrap_or("anonymous")
        .trim()
        .trim_start_matches('*')
        .to_string();
    let function_id = stable_id("F", SCHEMA_VERSION, &[module_id, &function_name, &span.snippet]);
    let scope_id = stable_id("S", SCHEMA_VERSION, &[&function_id, "function"]);

    cache.scopes.push(ScopeRecord {
        scope_id: scope_id.clone(),
        scope_kind: "function".to_string(),
        parent_scope_id: None,
        owner_id: function_id.clone(),
        span: span.clone(),
    });
    cache.functions.push(FunctionRecord {
        function_id: function_id.clone(),
        module_id: module_id.to_string(),
        class_id: None,
        qualified_name: function_name.clone(),
        kind: "function".to_string(),
        params: extract_c_params(node, text),
        scope_id: scope_id.clone(),
        span: span.clone(),
    });

    for param in extract_c_params(node, text) {
        cache.definitions.push(Definition {
            def_id: stable_id("D", SCHEMA_VERSION, &[&function_id, "param", &param]),
            place: Place::Local {
                scope_id: scope_id.clone(),
                name: param.clone(),
            },
            def_kind: "param".to_string(),
            scope_id: scope_id.clone(),
            function_id: Some(function_id.clone()),
            span: span.clone(),
            expr: param,
            deps: Vec::new(),
        });
    }

    lower_function_body(path, text, node, &function_id, &scope_id, cache);
}

fn extract_c_params(node: Node, text: &str) -> Vec<String> {
    let declarator = node.child_by_field_name("declarator");
    let Some(declarator) = declarator else { return Vec::new() };
    let mut cursor = declarator.walk();
    declarator
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "parameter_list")
        .flat_map(|list| {
            let mut inner = list.walk();
            list.named_children(&mut inner)
                .filter(|child| child.kind() == "parameter_declaration")
                .filter_map(|param| {
                    param.child_by_field_name("declarator")
                        .and_then(|decl| decl.utf8_text(text.as_bytes()).ok())
                        .map(|value| value.trim().trim_start_matches('*').to_string())
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn lower_global(path: &str, text: &str, module_id: &str, node: Node, cache: &mut AnalysisCache) {
    let snippet = node.utf8_text(text.as_bytes()).unwrap_or("").trim().to_string();
    if let Some((name, expr)) = snippet.split_once('=') {
        cache.definitions.push(Definition {
            def_id: stable_id("D", SCHEMA_VERSION, &[module_id, name.trim(), "global"]),
            place: Place::Global {
                module_id: module_id.to_string(),
                name: name
                    .split_whitespace()
                    .last()
                    .unwrap_or(name)
                    .trim()
                    .to_string(),
            },
            def_kind: "assign".to_string(),
            scope_id: module_id.to_string(),
            function_id: None,
            span: span_for(path, text, node),
            expr: expr.trim().trim_end_matches(';').to_string(),
            deps: Vec::new(),
        });
    }
}

fn lower_function_body(
    path: &str,
    text: &str,
    node: Node,
    function_id: &str,
    scope_id: &str,
    cache: &mut AnalysisCache,
) {
    let body = node.child_by_field_name("body");
    let Some(body) = body else { return };
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        let snippet = child.utf8_text(text.as_bytes()).unwrap_or("").trim().to_string();
        if child.kind() == "declaration" && snippet.contains('=') {
            let (left, right) = snippet.split_once('=').unwrap();
            let name = left.split_whitespace().last().unwrap().trim().to_string();
            cache.definitions.push(Definition {
                def_id: stable_id("D", SCHEMA_VERSION, &[function_id, &name, &snippet]),
                place: Place::Local {
                    scope_id: scope_id.to_string(),
                    name,
                },
                def_kind: "assign".to_string(),
                scope_id: scope_id.to_string(),
                function_id: Some(function_id.to_string()),
                span: span_for(path, text, child),
                expr: right.trim().trim_end_matches(';').to_string(),
                deps: Vec::new(),
            });
        }
        if child.kind() == "return_statement" {
            let value = snippet
                .trim_start_matches("return")
                .trim()
                .trim_end_matches(';')
                .to_string();
            cache.uses.push(Use {
                use_id: stable_id("U", SCHEMA_VERSION, &[function_id, "return", &value]),
                place: Place::Local {
                    scope_id: scope_id.to_string(),
                    name: value.clone(),
                },
                use_kind: "read".to_string(),
                scope_id: scope_id.to_string(),
                function_id: Some(function_id.to_string()),
                span: span_for(path, text, child),
                context: "return value".to_string(),
            });
        }
    }
}

fn span_for(path: &str, text: &str, node: Node) -> SourceSpan {
    let start = node.start_position();
    let end = node.end_position();
    SourceSpan {
        file: path.to_string(),
        line: start.row + 1,
        col: start.column + 1,
        end_line: end.row + 1,
        end_col: end.column + 1,
        snippet: node
            .utf8_text(text.as_bytes())
            .unwrap_or("")
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string(),
    }
}
```

Update `src/lang/mod.rs`:

```rust
pub mod c;
pub mod python;
```

Update `src/lib.rs`:

```rust
pub mod lang;
```

- [ ] **Step 4: Run the tests and verify they pass**

Run: `cargo test --test c_frontend c_frontend_extracts_functions_params_and_returns --test c_frontend c_frontend_records_global_and_local_assignments`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/lang/c.rs src/lang/mod.rs src/lib.rs tests/c_frontend.rs
git commit -m "feat: parse basic C translation units into IR"
```

---

### Task 5: Lower C Places, Calls, And Baseline CFG

**Files:**
- Modify: `src/lang/c.rs`
- Modify: `tests/c_frontend.rs`

- [ ] **Step 1: Write the failing advanced C frontend tests**

Append to `tests/c_frontend.rs`:

```rust
#[test]
fn c_frontend_normalizes_field_and_subscript_places() {
    let cache = parse_c(
        r#"
struct Item { int value; };

int read_item(struct Item *item, int index, int *values) {
    item->value = values[index];
    return item->value;
}
"#,
    );

    assert!(cache.definitions.iter().any(|definition| {
        matches!(
            &definition.place,
            Place::Attribute { base, attr }
                if base == "item" && attr == "value"
        )
    }));
    assert!(cache.uses.iter().any(|use_site| {
        matches!(
            &use_site.place,
            Place::Subscript { base, index }
                if base == "values" && index == "index"
        )
    }));
}

#[test]
fn c_frontend_records_calls_and_emits_cfg() {
    let cache = parse_c(
        r#"
int helper(int value) { return value; }

int run(int value, int (*fn_ptr)(int)) {
    if (value > 0) {
        value = helper(value);
    } else {
        value = fn_ptr(value);
    }
    while (value > 10) {
        value = value - 1;
    }
    return value;
}
"#,
    );

    let run_fn = cache
        .functions
        .iter()
        .find(|function| function.qualified_name == "run")
        .unwrap();
    let cfg = cache
        .cfgs
        .iter()
        .find(|cfg| cfg.function_id == run_fn.function_id)
        .unwrap();

    assert!(cache.calls.iter().any(|call| call.callee_expr == "helper"));
    assert!(cache.calls.iter().any(|call| call.callee_expr == "fn_ptr"));
    assert!(cfg.edges.iter().any(|edge| edge.edge_kind == "branch-true"));
    assert!(cfg.edges.iter().any(|edge| edge.edge_kind == "branch-false"));
    assert!(cfg.edges.iter().any(|edge| edge.edge_kind == "loop-back"));
}
```

- [ ] **Step 2: Run the tests and verify they fail**

Run: `cargo test --test c_frontend c_frontend_normalizes_field_and_subscript_places --test c_frontend c_frontend_records_calls_and_emits_cfg`

Expected: FAIL because the current C frontend only handles simple declarations
and return uses.

- [ ] **Step 3: Implement field/subscript/call lowering and CFG edges**

Add these helpers to `src/lang/c.rs`:

```rust
use crate::cfg::ControlFlowGraph;
use crate::ir::{CallRecord, CfgRecord};

fn normalize_c_lvalue(node_text: &str, scope_id: &str) -> Place {
    if let Some((base, attr)) = node_text.split_once("->") {
        return Place::Attribute {
            base: base.trim().trim_start_matches('*').to_string(),
            attr: attr.trim().to_string(),
        };
    }
    if let Some((base, attr)) = node_text.split_once('.') {
        return Place::Attribute {
            base: base.trim().to_string(),
            attr: attr.trim().to_string(),
        };
    }
    if let Some((base, index)) = node_text.split_once('[') {
        return Place::Subscript {
            base: base.trim().to_string(),
            index: index.trim_end_matches(']').trim().to_string(),
        };
    }
    Place::Local {
        scope_id: scope_id.to_string(),
        name: node_text.trim().trim_start_matches('*').to_string(),
    }
}

fn record_call(
    cache: &mut AnalysisCache,
    function_id: &str,
    span: SourceSpan,
    callee_expr: String,
    arg_use_ids: Vec<String>,
    return_target_def_id: Option<String>,
) {
    cache.calls.push(CallRecord {
        call_id: stable_id("CALL", SCHEMA_VERSION, &[function_id, &callee_expr, &span.snippet]),
        function_id: Some(function_id.to_string()),
        callee_expr,
        candidate_function_ids: Vec::new(),
        resolution: "unresolved".to_string(),
        arg_use_ids,
        return_target_def_id,
        span,
    });
}
```

Inside `lower_function`, create and store a CFG:

```rust
    let mut cfg = ControlFlowGraph::new(function_id.clone());
    let body_block = cfg.add_block("BasicBlock", span.clone());
    cfg.add_edge(&cfg.entry_block_id.clone(), &body_block, "sequence", "entry");
```

Inside `lower_function_body`, detect common constructs:

```rust
        if child.kind() == "expression_statement" && snippet.contains('=') {
            let (left, right) = snippet.split_once('=').unwrap();
            let place = normalize_c_lvalue(left.trim(), scope_id);
            let span = span_for(path, text, child);
            let def_id = stable_id("D", SCHEMA_VERSION, &[function_id, &snippet, "expr-assign"]);
            let deps = collect_rhs_places(right, scope_id);
            cache.definitions.push(Definition {
                def_id: def_id.clone(),
                place,
                def_kind: "assign".to_string(),
                scope_id: scope_id.to_string(),
                function_id: Some(function_id.to_string()),
                span: span.clone(),
                expr: right.trim().trim_end_matches(';').to_string(),
                deps: deps.iter().map(|item| item.place.clone()).collect(),
            });
            for dep in deps {
                cache.uses.push(dep);
            }
        }

        if child.kind() == "if_statement" {
            let then_block = cfg.add_block("Branch", span_for(path, text, child));
            let else_block = cfg.add_block("Branch", span_for(path, text, child));
            cfg.add_edge(&body_block, &then_block, "branch-true", "if");
            cfg.add_edge(&body_block, &else_block, "branch-false", "else");
        }

        if child.kind() == "while_statement" {
            let loop_block = cfg.add_block("Loop", span_for(path, text, child));
            cfg.add_edge(&body_block, &loop_block, "loop-enter", "while");
            cfg.add_edge(&loop_block, &loop_block, "loop-back", "while");
        }
```

Add a helper to collect RHS places:

```rust
fn collect_rhs_places(expr: &str, scope_id: &str) -> Vec<Use> {
    let mut uses = Vec::new();
    for token in expr
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '>' || ch == '.' || ch == '[' || ch == ']'))
        .filter(|token| !token.is_empty())
    {
        let place = normalize_c_lvalue(token, scope_id);
        uses.push(Use {
            use_id: stable_id("U", SCHEMA_VERSION, &[scope_id, expr, token]),
            place,
            use_kind: "read".to_string(),
            scope_id: scope_id.to_string(),
            function_id: None,
            span: SourceSpan::synthetic("<c-expr>", expr),
            context: "assign:rhs".to_string(),
        });
    }
    uses
}
```

Finalize each function CFG:

```rust
    cfg.add_edge(&body_block, &cfg.exit_block_id.clone(), "sequence", "exit");
    cache.cfgs.push(cfg.into_record());
```

- [ ] **Step 4: Run the tests and verify they pass**

Run: `cargo test --test c_frontend c_frontend_normalizes_field_and_subscript_places --test c_frontend c_frontend_records_calls_and_emits_cfg`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/lang/c.rs tests/c_frontend.rs
git commit -m "feat: lower C places calls and cfg"
```

---

### Task 6: Resolve Project-Local C Symbols And Reuse Summary Propagation

**Files:**
- Create: `src/csymbols.rs`
- Modify: `src/lib.rs`
- Modify: `tests/c_frontend.rs`

- [ ] **Step 1: Write the failing cross-file resolution and summary tests**

Append to `tests/c_frontend.rs`:

```rust
use data_flow_analyzer::analysis::{compute_def_use_edges, compute_var_dependencies};
use data_flow_analyzer::csymbols::resolve_c_symbols;
use data_flow_analyzer::summaries::{build_initial_summaries, propagate_call_summaries};

fn parse_c_units(units: &[(&str, &str)]) -> AnalysisCache {
    let units = units
        .iter()
        .map(|(path, text)| SourceUnit {
            absolute_path: std::path::PathBuf::from(path),
            relative_path: (*path).to_string(),
            source_text: (*text).to_string(),
            original_path: None,
            line_markers: Vec::new(),
        })
        .collect::<Vec<_>>();
    CFrontend::new().parse_units(&units).unwrap()
}

#[test]
fn c_symbol_resolution_links_project_local_calls() {
    let mut cache = parse_c_units(&[
        (
            "helper.c",
            r#"
int helper(int value) { return value + 1; }
"#,
        ),
        (
            "main.c",
            r#"
int helper(int value);
int run(int input) { return helper(input); }
"#,
        ),
    ]);

    resolve_c_symbols(&mut cache);

    let call = cache.calls.iter().find(|call| call.callee_expr == "helper").unwrap();
    assert_eq!(call.resolution, "project-local");
    assert_eq!(call.candidate_function_ids.len(), 1);
}

#[test]
fn c_summary_propagation_links_argument_to_return_target() {
    let mut cache = parse_c_units(&[
        (
            "helper.c",
            r#"
int helper(int value) { return value; }
"#,
        ),
        (
            "main.c",
            r#"
int helper(int value);
int run(int input) {
    int result = helper(input);
    return result;
}
"#,
        ),
    ]);

    resolve_c_symbols(&mut cache);
    compute_def_use_edges(&mut cache);
    compute_var_dependencies(&mut cache);
    build_initial_summaries(&mut cache);
    propagate_call_summaries(&mut cache);

    assert!(cache.function_summaries.iter().any(|summary| {
        let function = cache
            .functions
            .iter()
            .find(|item| item.function_id == summary.function_id)
            .unwrap();
        function.qualified_name == "run" && !summary.returns.is_empty()
    }));
    assert!(cache
        .var_dependency_edges
        .iter()
        .any(|edge| edge.dep_kind == "call-return"));
}
```

- [ ] **Step 2: Run the tests and verify they fail**

Run: `cargo test --test c_frontend c_symbol_resolution_links_project_local_calls --test c_frontend c_summary_propagation_links_argument_to_return_target`

Expected: FAIL because the C call records have no project-local symbol
resolution yet.

- [ ] **Step 3: Implement project-local C symbol resolution**

Create `src/csymbols.rs`:

```rust
use crate::ir::AnalysisCache;
use std::collections::BTreeMap;

pub fn resolve_c_symbols(cache: &mut AnalysisCache) {
    let mut functions_by_name = BTreeMap::<String, Vec<String>>::new();
    for function in &cache.functions {
        functions_by_name
            .entry(simple_name(&function.qualified_name))
            .or_default()
            .push(function.function_id.clone());
    }

    for call in &mut cache.calls {
        if let Some(candidates) = functions_by_name.get(&call.callee_expr) {
            call.candidate_function_ids = candidates.clone();
            call.resolution = "project-local".to_string();
        } else if call.callee_expr.contains("->") || call.callee_expr == "fn_ptr" {
            call.resolution = "indirect".to_string();
        } else {
            call.resolution = "external".to_string();
        }
    }
}

fn simple_name(name: &str) -> String {
    name.rsplit("::").next().unwrap_or(name).to_string()
}
```

Export the module from `src/lib.rs`:

```rust
pub mod csymbols;
```

In the future `run_analyze` will call `resolve_c_symbols(&mut cache)` before the
shared analysis stages. For this task, the unit tests invoke it directly.

- [ ] **Step 4: Run the tests and verify they pass**

Run: `cargo test --test c_frontend c_symbol_resolution_links_project_local_calls --test c_frontend c_summary_propagation_links_argument_to_return_target`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/csymbols.rs src/lib.rs tests/c_frontend.rs
git commit -m "feat: resolve project-local C symbols"
```

---

### Task 7: Wire Full C Analyze Flow, Report Outputs, And Acceptance Tests

**Files:**
- Modify: `src/cli.rs`
- Modify: `README.md`
- Create: `tests/c_integration_report.rs`

- [ ] **Step 1: Write the failing end-to-end C integration tests**

Create `tests/c_integration_report.rs`:

```rust
use assert_cmd::Command;
use std::fs;
use std::path::Path;

fn write_cmake_fixture(root: &Path) {
    fs::write(
        root.join("CMakeLists.txt"),
        r#"
cmake_minimum_required(VERSION 3.20)
project(c_report_fixture C)
add_executable(c_report_fixture main.c helper.c)
"#,
    )
    .unwrap();
    fs::write(root.join("helper.h"), "int helper(int value);\n").unwrap();
    fs::write(root.join("helper.c"), "int helper(int value) { return value + 1; }\n").unwrap();
    fs::write(
        root.join("main.c"),
        r#"
#include "helper.h"
int run(int input) {
    int result = helper(input);
    return result;
}
"#,
    )
    .unwrap();
}

#[test]
fn analyze_command_writes_report_for_c_fixture() {
    if std::process::Command::new("cmake")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("app");
    let out = dir.path().join("report");
    let build = dir.path().join("build");
    fs::create_dir_all(&input).unwrap();
    write_cmake_fixture(&input);

    let mut cmd = Command::cargo_bin("data-flow-analyzer").unwrap();
    cmd.args([
        "analyze",
        "--lang",
        "c",
        "--input",
        input.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--build-root",
        build.to_str().unwrap(),
    ])
    .assert()
    .success();

    assert!(out.join("index.html").exists());
    assert!(out.join("data/analysis-cache.json").exists());
    assert!(out.join("data/compile_commands.merged.json").exists());
}

#[test]
fn paths_command_writes_query_result_from_c_cache() {
    if std::process::Command::new("cmake")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("app");
    let out = dir.path().join("report");
    let build = dir.path().join("build");
    fs::create_dir_all(&input).unwrap();
    write_cmake_fixture(&input);

    let mut analyze = Command::cargo_bin("data-flow-analyzer").unwrap();
    analyze
        .args([
            "analyze",
            "--lang",
            "c",
            "--input",
            input.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--build-root",
            build.to_str().unwrap(),
        ])
        .assert()
        .success();

    let mut paths = Command::cargo_bin("data-flow-analyzer").unwrap();
    paths
        .args([
            "paths",
            "--input",
            out.join("data/analysis-cache.json").to_str().unwrap(),
            "--function",
            "run",
        ])
        .assert()
        .success();

    assert!(out.join("data/path-query.json").exists());
}

#[ignore]
#[test]
fn lschat_tests_can_be_analyzed_with_real_compile_context() {
    let input = Path::new("/mnt/d/repos/arcs_mini/modules/lschat/tests");
    if !input.exists() {
        return;
    }

    let out = tempfile::tempdir().unwrap();
    let build = tempfile::tempdir().unwrap();
    let mut cmd = Command::cargo_bin("data-flow-analyzer").unwrap();
    cmd.args([
        "analyze",
        "--lang",
        "c",
        "--input",
        input.to_str().unwrap(),
        "--out",
        out.path().to_str().unwrap(),
        "--build-root",
        build.path().to_str().unwrap(),
        "--cmake-arg",
        "-DLSCHAT_SKIP_GIT_VERSION=ON",
    ])
    .assert()
    .success();

    assert!(out.path().join("data/compile_commands.merged.json").exists());
    assert!(out.path().join("index.html").exists());
}
```

- [ ] **Step 2: Run the tests and verify they fail**

Run: `cargo test --test c_integration_report analyze_command_writes_report_for_c_fixture --test c_integration_report paths_command_writes_query_result_from_c_cache`

Expected: FAIL because `run_analyze` still only accepts Python and does not yet
invoke the C pipeline.

- [ ] **Step 3: Implement the end-to-end C analyze path and update the docs**

Update `run_analyze` in `src/cli.rs`:

```rust
fn run_analyze(
    config: Option<PathBuf>,
    lang: Option<String>,
    input: Option<PathBuf>,
    out: Option<PathBuf>,
    build_root: Option<PathBuf>,
    cmake_args: Vec<String>,
    keep_preprocessed: bool,
) -> Result<()> {
    let mut cfg = if let Some(config_path) = config {
        AnalyzeConfig::from_toml_file(&config_path)?
    } else {
        AnalyzeConfig::default()
    };
    cfg.apply_cli_overrides(
        lang,
        input,
        out,
        build_root,
        cmake_args,
        keep_preprocessed,
    );

    std::fs::create_dir_all(cfg.out.join("data"))?;

    let mut cache = match cfg.lang.as_str() {
        "python" => {
            let files = discover_sources(&cfg)?;
            let units = files
                .into_iter()
                .map(|file| crate::source::SourceUnit {
                    absolute_path: file.absolute_path.clone(),
                    relative_path: file.relative_path.clone(),
                    source_text: std::fs::read_to_string(&file.absolute_path)?,
                    original_path: None,
                    line_markers: Vec::new(),
                })
                .collect::<Result<Vec<_>, anyhow::Error>>()?;
            let mut cache = PythonFrontend::new().parse_units(&units)?;
            resolve_imports(&mut cache);
            cache
        }
        "c" => {
            let projects = crate::cbuild::discover_cmake_projects(&cfg)?;
            let configured = crate::cbuild::configure_cmake_projects(&projects, &cfg)?;
            let merged_path = cfg.out.join("data/compile_commands.merged.json");
            let preprocessed_dir = cfg.out.join("data/c-preprocessed");
            std::fs::create_dir_all(&preprocessed_dir)?;
            let compile_commands = crate::cbuild::merge_compile_commands(
                &configured
                    .iter()
                    .map(|project| project.compile_commands_path.clone())
                    .collect::<Vec<_>>(),
                &merged_path,
            )?;
            let units = compile_commands
                .iter()
                .map(|command| {
                    let preprocessed = preprocessed_dir.join(
                        command
                            .file
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .replace(".c", ".i"),
                    );
                    let args = crate::ccompile::build_preprocess_arguments(command, &preprocessed)?;
                    let output = std::process::Command::new(&args[0])
                        .args(&args[1..])
                        .current_dir(&command.directory)
                        .output()?;
                    if !output.status.success() {
                        anyhow::bail!(
                            "preprocess failed for {}: {}",
                            command.file.display(),
                            String::from_utf8_lossy(&output.stderr)
                        );
                    }
                    crate::ccompile::load_preprocessed_unit(&command.file, &preprocessed)
                })
                .collect::<Result<Vec<_>>>()?;
            let mut cache = crate::lang::c::CFrontend::new().parse_units(&units)?;
            crate::csymbols::resolve_c_symbols(&mut cache);
            cache
        }
        other => bail!("unsupported language '{}'", other),
    };

    compute_def_use_edges(&mut cache);
    compute_var_dependencies(&mut cache);
    build_initial_summaries(&mut cache);
    propagate_call_summaries(&mut cache);
    write_report(&cache, &cfg.out, cfg.top_n)?;
    println!("report written to {}", cfg.out.display());
    Ok(())
}
```

Update `README.md` by adding a C usage section:

```markdown
## Analyze A C Codebase

The analyzer can generate and merge `compile_commands.json` from CMake projects
automatically:

```bash
cargo run -- analyze --lang c --input /mnt/d/repos/arcs_mini/modules/lschat/tests --out /tmp/lschat-report --build-root /tmp/lschat-build
```

Useful flags:

- `--cmake-arg <arg>` to pass project-specific CMake configure values
- `--keep-preprocessed` to keep generated `.i` files under `data/c-preprocessed/`
```

- [ ] **Step 4: Run the integration tests and verify they pass**

Run: `cargo test --test c_integration_report analyze_command_writes_report_for_c_fixture --test c_integration_report paths_command_writes_query_result_from_c_cache`

Expected: PASS.

Run: `cargo test --test c_integration_report lschat_tests_can_be_analyzed_with_real_compile_context -- --ignored`

Expected: PASS on the target workstation with `/mnt/d/repos/arcs_mini/modules/lschat/tests`
and its required host build prerequisites present.

- [ ] **Step 5: Commit**

```bash
git add src/cli.rs README.md tests/c_integration_report.rs
git commit -m "feat: wire end-to-end C analysis pipeline"
```

---

## Self-Review Checklist

- Spec coverage:
  - `compile_commands.json` auto-generation: Task 2 and Task 7
  - preprocessing with real compile context: Task 3 and Task 7
  - shared report outputs: Task 7
  - CFG / reaching definitions / def-use / var-deps: Task 4 and Task 5, then reused in Task 6/7
  - summary propagation: Task 6
  - `paths` support: Task 7
  - LSChat acceptance input: Task 7 ignored test
- Placeholder scan:
  - no `TODO`, `TBD`, or “similar to previous task” language should remain
- Type consistency:
  - `AnalyzeConfig.build_root`, `AnalyzeConfig.cmake_args`, and `SourceUnit`
    names stay consistent across all tasks
  - `LanguageFrontend::parse_units` is the shared trait entry point in later tasks
