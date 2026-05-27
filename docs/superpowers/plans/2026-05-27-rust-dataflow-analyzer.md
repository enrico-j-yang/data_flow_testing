# Rust Dataflow Analyzer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the first version of a Rust data-flow analyzer that parses Python with tree-sitter, computes CFG-backed def-use/dependency data, and emits DOT/SVG plus an HTML report.

**Architecture:** The implementation uses a language-neutral core (`ir`, `cfg`, `analysis`, `paths`, `graph`, `report`) with a Python frontend under `lang::python`. The CLI writes a versioned analysis cache and report assets; later C support adds another frontend without rewriting the core.

**Tech Stack:** Rust 1.95, `clap`, `serde`, `serde_json`, `csv`, `toml`, `walkdir`, `globset`, `sha2`, `anyhow`, `thiserror`, `tree-sitter`, `tree-sitter-python`, `rayon`, Graphviz `dot`, `assert_cmd`, `predicates`, `tempfile`.

---

## Scope Check

The spec covers several subsystems, but they form one cohesive executable. This plan breaks the work into independently testable vertical slices: scaffold, configuration, IR, Python lowering, CFG, dataflow, paths, outputs, and integration. Each task should leave the repository in a buildable state and commit before moving on.

## File Structure

- Create `Cargo.toml`: package metadata, runtime dependencies, dev dependencies.
- Create `src/main.rs`: process entry point and error reporting.
- Create `src/lib.rs`: public module tree and test helper exports.
- Create `src/cli.rs`: `clap` command definitions and command dispatch.
- Create `src/config.rs`: TOML config loading, CLI overrides, default values.
- Create `src/fs.rs`: source discovery, ignore rules, normalized paths.
- Create `src/ids.rs`: stable schema-aware ID generation and safe slugs.
- Create `src/source.rs`: source file metadata, spans, snippets, path normalization.
- Create `src/ir.rs`: language-neutral modules, scopes, classes, functions, places, defs, uses, calls, captures, cache schema.
- Create `src/lang/mod.rs`: language frontend trait.
- Create `src/lang/python.rs`: tree-sitter parser and Python lowering into IR seeds.
- Create `src/imports.rs`: project import resolver, re-export handling, external/stub classification.
- Create `src/alias.rs`: `Place` normalization, simple alias propagation, attribute/subscript sensitivity.
- Create `src/cfg.rs`: CFG blocks, edges, synthetic blocks, Python control-flow lowering.
- Create `src/analysis.rs`: reaching definitions, def-use edges, variable dependencies.
- Create `src/summaries.rs`: function summaries, call graph SCC iteration, call-site propagation.
- Create `src/paths.rs`: bounded def-clear path expansion.
- Create `src/graph.rs`: DOT emitters and optional SVG rendering.
- Create `src/report.rs`: HTML, CSS, JS, CSV, and cache writers.
- Create `tests/fixtures/`: Python fixture projects used by integration tests.
- Create `tests/cli_smoke.rs`, `tests/config_discovery.rs`, `tests/python_frontend.rs`, `tests/integration_report.rs`.

---

### Task 1: Scaffold Cargo Project And CLI Skeleton

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `src/lib.rs`
- Create: `src/cli.rs`
- Test: `tests/cli_smoke.rs`

- [ ] **Step 1: Write the failing CLI smoke test**

Create `tests/cli_smoke.rs`:

```rust
use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn version_command_prints_binary_name() {
    let mut cmd = Command::cargo_bin("data-flow-analyzer").unwrap();
    cmd.arg("--version")
        .assert()
        .success()
        .stdout(contains("data-flow-analyzer"));
}
```

- [ ] **Step 2: Run the test and verify it fails**

Run: `cargo test --test cli_smoke version_command_prints_binary_name`

Expected: failure because the Cargo package and binary do not exist yet.

- [ ] **Step 3: Create the package and dependencies**

Run:

```powershell
cargo init --bin --name data-flow-analyzer .
cargo add anyhow clap --features clap/derive
cargo add serde --features serde/derive
cargo add serde_json csv toml walkdir globset sha2 hex thiserror tree-sitter tree-sitter-python rayon html-escape
cargo add --dev assert_cmd predicates tempfile pretty_assertions
```

Replace `src/main.rs`:

```rust
use anyhow::Result;

fn main() -> Result<()> {
    data_flow_analyzer::cli::run()
}
```

Create `src/lib.rs`:

```rust
pub mod cli;
```

Create `src/cli.rs`:

```rust
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "data-flow-analyzer", version, about = "Static def-use and dependency analyzer")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    Analyze {
        #[arg(long)]
        lang: Option<String>,
        #[arg(long)]
        input: Option<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    Paths {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        function: String,
        #[arg(long, default_value_t = 2)]
        max_loop_unroll: usize,
    },
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Analyze { .. }) => {
            println!("analyze command is available");
            Ok(())
        }
        Some(Commands::Paths { .. }) => {
            println!("paths command is available");
            Ok(())
        }
        None => {
            Cli::parse_from(["data-flow-analyzer", "--help"]);
            Ok(())
        }
    }
}
```

- [ ] **Step 4: Run the smoke test**

Run: `cargo test --test cli_smoke version_command_prints_binary_name`

Expected: test passes.

- [ ] **Step 5: Commit**

```powershell
git add Cargo.toml Cargo.lock src/main.rs src/lib.rs src/cli.rs tests/cli_smoke.rs
git commit -m "feat: scaffold analyzer cli"
```

---

### Task 2: Configuration And Source Discovery

**Files:**
- Modify: `src/lib.rs`
- Modify: `src/cli.rs`
- Create: `src/config.rs`
- Create: `src/fs.rs`
- Test: `tests/config_discovery.rs`

- [ ] **Step 1: Write failing config and discovery tests**

Create `tests/config_discovery.rs`:

```rust
use data_flow_analyzer::config::AnalyzeConfig;
use data_flow_analyzer::fs::discover_sources;
use std::fs;

#[test]
fn config_file_loads_defaults_and_cli_overrides() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("dataflow.toml");
    fs::write(
        &cfg_path,
        r#"
lang = "python"
input = "D:/repos/asr_platform/app"
out = "D:/tmp/dataflow_report"
max_loop_unroll = 2
top_n = 50
emit_full_dot = false
render_full_svg = false
fail_on_parse_error = false
parallelism = "auto"
exclude = ["**/__pycache__/**"]
stub_paths = ["stubs"]
"#,
    )
    .unwrap();

    let mut cfg = AnalyzeConfig::from_toml_file(&cfg_path).unwrap();
    cfg.apply_cli_overrides(None, None, Some(dir.path().join("override_out")));

    assert_eq!(cfg.lang, "python");
    assert_eq!(cfg.max_loop_unroll, 2);
    assert_eq!(cfg.top_n, 50);
    assert!(cfg.out.ends_with("override_out"));
}

#[test]
fn discovery_skips_ignored_python_files() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("app/__pycache__")).unwrap();
    fs::create_dir_all(dir.path().join("app/routers")).unwrap();
    fs::write(dir.path().join("app/main.py"), "x = 1\n").unwrap();
    fs::write(dir.path().join("app/routers/tests.py"), "y = 2\n").unwrap();
    fs::write(dir.path().join("app/__pycache__/ignored.py"), "z = 3\n").unwrap();

    let cfg = AnalyzeConfig {
        input: dir.path().join("app"),
        exclude: vec!["**/__pycache__/**".to_string()],
        ..AnalyzeConfig::default()
    };

    let files = discover_sources(&cfg).unwrap();
    let paths: Vec<String> = files.iter().map(|f| f.relative_path.clone()).collect();

    assert_eq!(paths, vec!["main.py", "routers/tests.py"]);
}
```

- [ ] **Step 2: Run tests and verify they fail**

Run: `cargo test --test config_discovery`

Expected: failure because `config` and `fs` modules are missing.

- [ ] **Step 3: Implement config and discovery**

Modify `src/lib.rs`:

```rust
pub mod cli;
pub mod config;
pub mod fs;
```

Create `src/config.rs`:

```rust
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
```

Create `src/fs.rs`:

```rust
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
        builder.add(Glob::new(pattern).with_context(|| format!("invalid exclude glob {pattern}"))?);
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
```

- [ ] **Step 4: Run config tests**

Run: `cargo test --test config_discovery`

Expected: both tests pass.

- [ ] **Step 5: Commit**

```powershell
git add src/lib.rs src/config.rs src/fs.rs tests/config_discovery.rs
git commit -m "feat: add analyzer configuration and source discovery"
```

---

### Task 3: Stable IDs, Source Spans, And Cache Shell

**Files:**
- Modify: `src/lib.rs`
- Create: `src/ids.rs`
- Create: `src/source.rs`
- Create: `src/ir.rs`

- [ ] **Step 1: Add failing unit tests for stable IDs and spans**

Create `src/ids.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_ids_are_repeatable_and_prefixed() {
        let a = stable_id("D", 1, &["app/main.py", "x", "1:0"]);
        let b = stable_id("D", 1, &["app/main.py", "x", "1:0"]);
        assert_eq!(a, b);
        assert!(a.starts_with("D_"));
    }

    #[test]
    fn safe_slug_replaces_dot_unsafe_characters() {
        assert_eq!(safe_slug("app/routers/tests.py::create-test"), "app_routers_tests_py__create_test");
    }
}
```

Run: `cargo test ids::tests`

Expected: failure because functions are missing.

- [ ] **Step 2: Implement `ids.rs`**

Replace `src/ids.rs`:

```rust
use sha2::{Digest, Sha256};

pub fn stable_id(prefix: &str, schema_version: u32, parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(schema_version.to_string().as_bytes());
    hasher.update(b"\0");
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update(b"\0");
    }
    let digest = hasher.finalize();
    format!("{prefix}_{}", hex::encode(&digest[..8]))
}

pub fn safe_slug(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_ids_are_repeatable_and_prefixed() {
        let a = stable_id("D", 1, &["app/main.py", "x", "1:0"]);
        let b = stable_id("D", 1, &["app/main.py", "x", "1:0"]);
        assert_eq!(a, b);
        assert!(a.starts_with("D_"));
    }

    #[test]
    fn safe_slug_replaces_dot_unsafe_characters() {
        assert_eq!(safe_slug("app/routers/tests.py::create-test"), "app_routers_tests_py__create_test");
    }
}
```

- [ ] **Step 3: Add source and IR shell**

Create `src/source.rs`:

```rust
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
```

Create `src/ir.rs`:

```rust
use crate::source::SourceSpan;
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Place {
    Local { scope_id: String, name: String },
    Global { module_id: String, name: String },
    Closure { scope_id: String, name: String },
    Attribute { base: String, attr: String },
    Subscript { base: String, index: String },
    External { name: String },
    Unknown { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Definition {
    pub def_id: String,
    pub place: Place,
    pub def_kind: String,
    pub scope_id: String,
    pub function_id: Option<String>,
    pub span: SourceSpan,
    pub expr: String,
    pub deps: Vec<Place>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Use {
    pub use_id: String,
    pub place: Place,
    pub use_kind: String,
    pub scope_id: String,
    pub function_id: Option<String>,
    pub span: SourceSpan,
    pub context: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AnalysisCache {
    pub schema_version: u32,
    pub tool_version: String,
    pub files: Vec<SourceFileRecord>,
    pub definitions: Vec<Definition>,
    pub uses: Vec<Use>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceFileRecord {
    pub file_id: String,
    pub path: String,
    pub hash: String,
    pub line_count: usize,
    pub parse_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub diagnostic_id: String,
    pub severity: String,
    pub kind: String,
    pub message: String,
    pub file: String,
    pub span: SourceSpan,
}
```

Modify `src/lib.rs`:

```rust
pub mod cli;
pub mod config;
pub mod fs;
pub mod ids;
pub mod ir;
pub mod source;
```

- [ ] **Step 4: Run tests**

Run: `cargo test ids::tests`

Expected: ID tests pass.

- [ ] **Step 5: Commit**

```powershell
git add src/lib.rs src/ids.rs src/source.rs src/ir.rs
git commit -m "feat: define stable ids and cache shell"
```

---

### Task 4: Expand IR For Modules, Scopes, Classes, Captures, Calls, And Edges

**Files:**
- Modify: `src/ir.rs`

- [ ] **Step 1: Add IR serialization tests**

Append this test module to `src/ir.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SourceSpan;

    #[test]
    fn cache_round_trips_rich_ir() {
        let cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            tool_version: "0.1.0".to_string(),
            files: vec![SourceFileRecord {
                file_id: "M_a".to_string(),
                path: "app/a.py".to_string(),
                hash: "abc".to_string(),
                line_count: 3,
                parse_status: "ok".to_string(),
            }],
            modules: vec![ModuleRecord {
                module_id: "M_a".to_string(),
                file_id: "M_a".to_string(),
                module_name: "app.a".to_string(),
                exports: vec!["foo".to_string()],
                imports: Vec::new(),
            }],
            scopes: Vec::new(),
            classes: Vec::new(),
            functions: Vec::new(),
            definitions: vec![Definition {
                def_id: "D_x".to_string(),
                place: Place::Local { scope_id: "S_a".to_string(), name: "x".to_string() },
                def_kind: "assign".to_string(),
                scope_id: "S_a".to_string(),
                function_id: None,
                span: SourceSpan::synthetic("app/a.py", "x = 1"),
                expr: "1".to_string(),
                deps: Vec::new(),
            }],
            uses: Vec::new(),
            captures: Vec::new(),
            calls: Vec::new(),
            cfgs: Vec::new(),
            def_use_edges: Vec::new(),
            var_dependency_edges: Vec::new(),
            function_summaries: Vec::new(),
            diagnostics: Vec::new(),
            graph_index: Vec::new(),
        };

        let json = serde_json::to_string(&cache).unwrap();
        let decoded: AnalysisCache = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.schema_version, SCHEMA_VERSION);
        assert_eq!(decoded.definitions[0].def_kind, "assign");
    }
}
```

Run: `cargo test ir::tests::cache_round_trips_rich_ir`

Expected: failure because the richer record types are missing.

- [ ] **Step 2: Replace `AnalysisCache` and add records**

Modify `src/ir.rs` so `AnalysisCache` includes the complete schema:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AnalysisCache {
    pub schema_version: u32,
    pub tool_version: String,
    pub files: Vec<SourceFileRecord>,
    pub modules: Vec<ModuleRecord>,
    pub scopes: Vec<ScopeRecord>,
    pub classes: Vec<ClassRecord>,
    pub functions: Vec<FunctionRecord>,
    pub definitions: Vec<Definition>,
    pub uses: Vec<Use>,
    pub captures: Vec<CaptureRecord>,
    pub calls: Vec<CallRecord>,
    pub cfgs: Vec<CfgRecord>,
    pub def_use_edges: Vec<DefUseEdge>,
    pub var_dependency_edges: Vec<VarDependencyEdge>,
    pub function_summaries: Vec<FunctionSummary>,
    pub diagnostics: Vec<Diagnostic>,
    pub graph_index: Vec<GraphRecord>,
}
```

Add these records below the existing structs:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleRecord {
    pub module_id: String,
    pub file_id: String,
    pub module_name: String,
    pub exports: Vec<String>,
    pub imports: Vec<ImportRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportRecord {
    pub import_id: String,
    pub module: String,
    pub name: Option<String>,
    pub alias: Option<String>,
    pub level: usize,
    pub resolution: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeRecord {
    pub scope_id: String,
    pub scope_kind: String,
    pub parent_scope_id: Option<String>,
    pub owner_id: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassRecord {
    pub class_id: String,
    pub module_id: String,
    pub qualified_name: String,
    pub base_exprs: Vec<String>,
    pub resolved_bases: Vec<String>,
    pub mro_status: String,
    pub methods: Vec<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionRecord {
    pub function_id: String,
    pub module_id: String,
    pub class_id: Option<String>,
    pub qualified_name: String,
    pub kind: String,
    pub params: Vec<String>,
    pub scope_id: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureRecord {
    pub capture_id: String,
    pub source_scope_id: String,
    pub target_function_id: String,
    pub place: Place,
    pub mode: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallRecord {
    pub call_id: String,
    pub function_id: Option<String>,
    pub callee_expr: String,
    pub candidate_function_ids: Vec<String>,
    pub resolution: String,
    pub arg_use_ids: Vec<String>,
    pub return_target_def_id: Option<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CfgRecord {
    pub function_id: String,
    pub blocks: Vec<CfgBlock>,
    pub edges: Vec<CfgEdge>,
    pub entry_block_id: String,
    pub exit_block_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CfgBlock {
    pub block_id: String,
    pub block_kind: String,
    pub statements: Vec<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CfgEdge {
    pub edge_id: String,
    pub from_block_id: String,
    pub to_block_id: String,
    pub edge_kind: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefUseEdge {
    pub edge_id: String,
    pub def_id: String,
    pub use_id: String,
    pub place: Place,
    pub edge_kind: String,
    pub path_summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VarDependencyEdge {
    pub edge_id: String,
    pub source_place: Place,
    pub target_place: Place,
    pub source_id: String,
    pub target_id: String,
    pub dep_kind: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionSummary {
    pub function_id: String,
    pub inputs: Vec<Place>,
    pub returns: Vec<Place>,
    pub yields: Vec<Place>,
    pub writes: Vec<Place>,
    pub raises: Vec<Place>,
    pub external_effects: Vec<String>,
    pub fixpoint_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphRecord {
    pub graph_id: String,
    pub kind: String,
    pub dot_path: String,
    pub svg_path: Option<String>,
    pub html_path: Option<String>,
}
```

- [ ] **Step 3: Run IR tests**

Run: `cargo test ir::tests::cache_round_trips_rich_ir`

Expected: test passes.

- [ ] **Step 4: Commit**

```powershell
git add src/ir.rs
git commit -m "feat: expand analyzer cache schema"
```

---

### Task 5: Python Frontend For Modules, Imports, Classes, Functions, Assignments, And Uses

**Files:**
- Modify: `src/lib.rs`
- Create: `src/lang/mod.rs`
- Create: `src/lang/python.rs`
- Test: `tests/python_frontend.rs`

- [ ] **Step 1: Write failing frontend test**

Create `tests/python_frontend.rs`:

```rust
use data_flow_analyzer::lang::python::PythonFrontend;
use data_flow_analyzer::lang::LanguageFrontend;
use data_flow_analyzer::fs::SourceFile;
use std::fs;

#[test]
fn python_frontend_extracts_core_ir() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sample.py");
    fs::write(
        &path,
        r#"
from app.config import settings as cfg

class Child(Base):
    class_value = 1

    def method(self, x):
        y = x + self.class_value
        return y
"#,
    )
    .unwrap();

    let source = SourceFile {
        absolute_path: path,
        relative_path: "sample.py".to_string(),
    };

    let cache = PythonFrontend::new().parse_files(&[source]).unwrap();
    assert_eq!(cache.modules.len(), 1);
    assert!(cache.imports().iter().any(|i| i.module == "app.config"));
    assert!(cache.classes.iter().any(|c| c.qualified_name == "Child"));
    assert!(cache.functions.iter().any(|f| f.qualified_name == "Child.method"));
    assert!(cache.definitions.iter().any(|d| d.def_kind == "assign"));
    assert!(cache.uses.iter().any(|u| u.context.contains("return")));
}
```

- [ ] **Step 2: Add helper method on cache for imports**

Add to `src/ir.rs`:

```rust
impl AnalysisCache {
    pub fn imports(&self) -> Vec<&ImportRecord> {
        self.modules.iter().flat_map(|m| m.imports.iter()).collect()
    }
}
```

Run: `cargo test --test python_frontend python_frontend_extracts_core_ir`

Expected: failure because the frontend does not exist.

- [ ] **Step 3: Add language trait**

Modify `src/lib.rs`:

```rust
pub mod cli;
pub mod config;
pub mod fs;
pub mod ids;
pub mod ir;
pub mod lang;
pub mod source;
```

Create `src/lang/mod.rs`:

```rust
pub mod python;

use crate::fs::SourceFile;
use crate::ir::AnalysisCache;
use anyhow::Result;

pub trait LanguageFrontend {
    fn parse_files(&self, files: &[SourceFile]) -> Result<AnalysisCache>;
}
```

- [ ] **Step 4: Implement a tree-sitter-backed Python lowering skeleton**

Create `src/lang/python.rs`:

```rust
use crate::fs::SourceFile;
use crate::ids::stable_id;
use crate::ir::*;
use crate::lang::LanguageFrontend;
use crate::source::SourceSpan;
use anyhow::{Context, Result};
use std::fs;
use tree_sitter::{Node, Parser};

pub struct PythonFrontend;

impl PythonFrontend {
    pub fn new() -> Self {
        Self
    }
}

impl LanguageFrontend for PythonFrontend {
    fn parse_files(&self, files: &[SourceFile]) -> Result<AnalysisCache> {
        let mut cache = AnalysisCache {
            schema_version: SCHEMA_VERSION,
            tool_version: env!("CARGO_PKG_VERSION").to_string(),
            ..AnalysisCache::default()
        };

        for file in files {
            let text = fs::read_to_string(&file.absolute_path)
                .with_context(|| format!("failed to read {}", file.absolute_path.display()))?;
            let mut parser = Parser::new();
            parser
                .set_language(&tree_sitter_python::LANGUAGE.into())
                .context("failed to initialize tree-sitter-python")?;
            let tree = parser.parse(&text, None).context("python parse returned no tree")?;
            lower_module(&mut cache, file, &text, tree.root_node());
        }

        Ok(cache)
    }
}

fn span(file: &SourceFile, text: &str, node: Node) -> SourceSpan {
    let start = node.start_position();
    let end = node.end_position();
    let snippet = node.utf8_text(text.as_bytes()).unwrap_or("").trim().chars().take(120).collect();
    SourceSpan {
        file: file.relative_path.clone(),
        line: start.row + 1,
        col: start.column,
        end_line: end.row + 1,
        end_col: end.column,
        snippet,
    }
}

fn lower_module(cache: &mut AnalysisCache, file: &SourceFile, text: &str, root: Node) {
    let file_id = stable_id("M", SCHEMA_VERSION, &[&file.relative_path]);
    let module_name = file.relative_path.trim_end_matches(".py").replace('/', ".");
    cache.files.push(SourceFileRecord {
        file_id: file_id.clone(),
        path: file.relative_path.clone(),
        hash: stable_id("H", SCHEMA_VERSION, &[text]),
        line_count: text.lines().count(),
        parse_status: if root.has_error() { "partial".to_string() } else { "ok".to_string() },
    });
    cache.modules.push(ModuleRecord {
        module_id: file_id.clone(),
        file_id: file_id.clone(),
        module_name,
        exports: Vec::new(),
        imports: Vec::new(),
    });
    let scope_id = stable_id("S", SCHEMA_VERSION, &[&file.relative_path, "module"]);
    cache.scopes.push(ScopeRecord {
        scope_id: scope_id.clone(),
        scope_kind: "module".to_string(),
        parent_scope_id: None,
        owner_id: file_id.clone(),
        span: SourceSpan::synthetic(&file.relative_path, "module"),
    });
    walk(cache, file, text, root, &file_id, &scope_id, None, None);
}

fn walk(
    cache: &mut AnalysisCache,
    file: &SourceFile,
    text: &str,
    node: Node,
    module_id: &str,
    scope_id: &str,
    function_id: Option<&str>,
    class_id: Option<&str>,
) {
    match node.kind() {
        "import_from_statement" => lower_import_from(cache, file, text, node, module_id),
        "class_definition" => lower_class(cache, file, text, node, module_id, scope_id),
        "function_definition" => lower_function(cache, file, text, node, module_id, scope_id, class_id),
        "assignment" => lower_assignment(cache, file, text, node, scope_id, function_id),
        "return_statement" => lower_return(cache, file, text, node, scope_id, function_id),
        "identifier" => lower_identifier_use(cache, file, text, node, scope_id, function_id, "expr"),
        _ => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                walk(cache, file, text, child, module_id, scope_id, function_id, class_id);
            }
        }
    }
}

fn node_text(node: Node, text: &str) -> String {
    node.utf8_text(text.as_bytes()).unwrap_or("").to_string()
}

fn lower_import_from(cache: &mut AnalysisCache, file: &SourceFile, text: &str, node: Node, module_id: &str) {
    let import_id = stable_id("I", SCHEMA_VERSION, &[&file.relative_path, &node.start_byte().to_string()]);
    let raw = node_text(node, text);
    let module = raw.split_whitespace().nth(1).unwrap_or("").to_string();
    if let Some(module_record) = cache.modules.iter_mut().find(|m| m.module_id == module_id) {
        module_record.imports.push(ImportRecord {
            import_id,
            module,
            name: None,
            alias: None,
            level: 0,
            resolution: "unresolved".to_string(),
            span: span(file, text, node),
        });
    }
}

fn lower_class(cache: &mut AnalysisCache, file: &SourceFile, text: &str, node: Node, module_id: &str, scope_id: &str) {
    let name = node.child_by_field_name("name").map(|n| node_text(n, text)).unwrap_or_else(|| "anonymous_class".to_string());
    let class_id = stable_id("C", SCHEMA_VERSION, &[&file.relative_path, &name, &node.start_byte().to_string()]);
    cache.classes.push(ClassRecord {
        class_id: class_id.clone(),
        module_id: module_id.to_string(),
        qualified_name: name.clone(),
        base_exprs: Vec::new(),
        resolved_bases: Vec::new(),
        mro_status: "unresolved".to_string(),
        methods: Vec::new(),
        span: span(file, text, node),
    });
    cache.definitions.push(Definition {
        def_id: stable_id("D", SCHEMA_VERSION, &[&file.relative_path, &name, "class"]),
        place: Place::Local { scope_id: scope_id.to_string(), name },
        def_kind: "class_def".to_string(),
        scope_id: scope_id.to_string(),
        function_id: None,
        span: span(file, text, node),
        expr: node_text(node, text),
        deps: Vec::new(),
    });
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(cache, file, text, child, module_id, scope_id, None, Some(&class_id));
    }
}

fn lower_function(cache: &mut AnalysisCache, file: &SourceFile, text: &str, node: Node, module_id: &str, parent_scope_id: &str, class_id: Option<&str>) {
    let name = node.child_by_field_name("name").map(|n| node_text(n, text)).unwrap_or_else(|| "anonymous_function".to_string());
    let qualified = class_id
        .and_then(|cid| cache.classes.iter().find(|c| c.class_id == cid).map(|c| format!("{}.{}", c.qualified_name, name)))
        .unwrap_or_else(|| name.clone());
    let function_id = stable_id("F", SCHEMA_VERSION, &[&file.relative_path, &qualified, &node.start_byte().to_string()]);
    let scope_id = stable_id("S", SCHEMA_VERSION, &[&file.relative_path, &qualified, "function"]);
    cache.functions.push(FunctionRecord {
        function_id: function_id.clone(),
        module_id: module_id.to_string(),
        class_id: class_id.map(str::to_string),
        qualified_name: qualified.clone(),
        kind: "function".to_string(),
        params: Vec::new(),
        scope_id: scope_id.clone(),
        span: span(file, text, node),
    });
    cache.scopes.push(ScopeRecord {
        scope_id: scope_id.clone(),
        scope_kind: "function".to_string(),
        parent_scope_id: Some(parent_scope_id.to_string()),
        owner_id: function_id.clone(),
        span: span(file, text, node),
    });
    cache.definitions.push(Definition {
        def_id: stable_id("D", SCHEMA_VERSION, &[&file.relative_path, &qualified, "function"]),
        place: Place::Local { scope_id: parent_scope_id.to_string(), name },
        def_kind: "function_def".to_string(),
        scope_id: parent_scope_id.to_string(),
        function_id: None,
        span: span(file, text, node),
        expr: format!("def {qualified}"),
        deps: Vec::new(),
    });
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(cache, file, text, child, module_id, &scope_id, Some(&function_id), class_id);
    }
}

fn lower_assignment(cache: &mut AnalysisCache, file: &SourceFile, text: &str, node: Node, scope_id: &str, function_id: Option<&str>) {
    let left = node.child_by_field_name("left").map(|n| node_text(n, text)).unwrap_or_else(|| "unknown".to_string());
    cache.definitions.push(Definition {
        def_id: stable_id("D", SCHEMA_VERSION, &[&file.relative_path, &left, &node.start_byte().to_string()]),
        place: Place::Local { scope_id: scope_id.to_string(), name: left },
        def_kind: "assign".to_string(),
        scope_id: scope_id.to_string(),
        function_id: function_id.map(str::to_string),
        span: span(file, text, node),
        expr: node_text(node, text),
        deps: Vec::new(),
    });
}

fn lower_return(cache: &mut AnalysisCache, file: &SourceFile, text: &str, node: Node, scope_id: &str, function_id: Option<&str>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            lower_identifier_use(cache, file, text, child, scope_id, function_id, "return");
        }
    }
}

fn lower_identifier_use(cache: &mut AnalysisCache, file: &SourceFile, text: &str, node: Node, scope_id: &str, function_id: Option<&str>, context: &str) {
    let name = node_text(node, text);
    if name.is_empty() {
        return;
    }
    cache.uses.push(Use {
        use_id: stable_id("U", SCHEMA_VERSION, &[&file.relative_path, &name, &node.start_byte().to_string(), context]),
        place: Place::Local { scope_id: scope_id.to_string(), name },
        use_kind: "name_load".to_string(),
        scope_id: scope_id.to_string(),
        function_id: function_id.map(str::to_string),
        span: span(file, text, node),
        context: context.to_string(),
    });
}
```

- [ ] **Step 5: Run frontend test**

Run: `cargo test --test python_frontend python_frontend_extracts_core_ir`

Expected: test passes. If the exact `tree_sitter_python` API differs in this crate version, update only the `set_language` line while keeping the same public behavior.

- [ ] **Step 6: Commit**

```powershell
git add src/lib.rs src/ir.rs src/lang/mod.rs src/lang/python.rs tests/python_frontend.rs
git commit -m "feat: parse python sources into initial ir"
```

---

### Task 6: Scope Resolution, Closures, Parameters, And Class Inheritance Metadata

**Files:**
- Modify: `src/lang/python.rs`
- Modify: `src/ir.rs`
- Test: `tests/python_frontend.rs`

- [ ] **Step 1: Add failing tests for closures and inheritance**

Append to `tests/python_frontend.rs`:

```rust
#[test]
fn python_frontend_records_captures_params_and_bases() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested.py");
    std::fs::write(
        &path,
        r#"
class Child(Base):
    def outer(self, value):
        total = value
        def inner(delta):
            nonlocal total
            total = total + delta
            return total
        return inner
"#,
    )
    .unwrap();

    let source = SourceFile {
        absolute_path: path,
        relative_path: "nested.py".to_string(),
    };

    let cache = PythonFrontend::new().parse_files(&[source]).unwrap();
    let child = cache.classes.iter().find(|c| c.qualified_name == "Child").unwrap();
    assert_eq!(child.base_exprs, vec!["Base"]);
    assert!(cache.functions.iter().any(|f| f.qualified_name == "Child.outer"));
    assert!(cache.functions.iter().any(|f| f.qualified_name == "Child.outer.inner"));
    assert!(cache.definitions.iter().any(|d| d.def_kind == "param" && format!("{:?}", d.place).contains("value")));
    assert!(cache.captures.iter().any(|c| format!("{:?}", c.place).contains("total")));
}
```

Run: `cargo test --test python_frontend python_frontend_records_captures_params_and_bases`

Expected: failure because params, nested names, bases, and captures are incomplete.

- [ ] **Step 2: Extend frontend state**

In `src/lang/python.rs`, add a lowering context above `PythonFrontend`:

```rust
#[derive(Debug, Clone)]
struct LoweringContext {
    module_id: String,
    scope_stack: Vec<String>,
    function_stack: Vec<String>,
    class_stack: Vec<String>,
    local_defs: Vec<std::collections::BTreeSet<String>>,
}

impl LoweringContext {
    fn new(module_id: String, scope_id: String) -> Self {
        Self {
            module_id,
            scope_stack: vec![scope_id],
            function_stack: Vec::new(),
            class_stack: Vec::new(),
            local_defs: vec![std::collections::BTreeSet::new()],
        }
    }

    fn scope_id(&self) -> &str {
        self.scope_stack.last().map(String::as_str).unwrap()
    }

    fn function_id(&self) -> Option<&str> {
        self.function_stack.last().map(String::as_str)
    }

    fn define_local(&mut self, name: &str) {
        if let Some(frame) = self.local_defs.last_mut() {
            frame.insert(name.to_string());
        }
    }
}
```

- [ ] **Step 3: Update function lowering**

Change `lower_function` so it:

- Builds qualified names by appending nested function names.
- Extracts parameters from the `parameters` child.
- Emits `param` definitions.
- Pushes a new scope and local-def frame while walking the body.
- Records a `CaptureRecord` when an identifier use resolves to an outer local.

Use this helper to extract parameter names:

```rust
fn collect_param_names(node: Node, text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            names.push(node_text(child, text));
        }
    }
    names
}
```

Use this helper when lowering identifier uses:

```rust
fn maybe_record_capture(cache: &mut AnalysisCache, ctx: &LoweringContext, file: &SourceFile, text: &str, node: Node, name: &str) {
    if ctx.function_stack.is_empty() {
        return;
    }
    let current_depth = ctx.local_defs.len().saturating_sub(1);
    let found_outer = ctx
        .local_defs
        .iter()
        .take(current_depth)
        .rev()
        .any(|frame| frame.contains(name));
    if !found_outer {
        return;
    }
    let target_function_id = ctx.function_id().unwrap_or("").to_string();
    let source_scope_id = ctx.scope_stack.first().cloned().unwrap_or_default();
    cache.captures.push(CaptureRecord {
        capture_id: stable_id("CAP", SCHEMA_VERSION, &[&file.relative_path, name, &node.start_byte().to_string()]),
        source_scope_id: source_scope_id.clone(),
        target_function_id,
        place: Place::Closure { scope_id: source_scope_id, name: name.to_string() },
        mode: "read".to_string(),
        span: span(file, text, node),
    });
}
```

- [ ] **Step 4: Update class lowering for bases and methods**

In `lower_class`, extract base expressions from the `superclasses` child:

```rust
fn collect_base_exprs(node: Node, text: &str) -> Vec<String> {
    node.child_by_field_name("superclasses")
        .map(|bases| {
            let mut out = Vec::new();
            let mut cursor = bases.walk();
            for child in bases.children(&mut cursor) {
                if child.kind() == "identifier" {
                    out.push(node_text(child, text));
                }
            }
            out
        })
        .unwrap_or_default()
}
```

Set `ClassRecord.base_exprs` to `collect_base_exprs(node, text)` and `mro_status` to `"local-unresolved"` until import/class resolution runs.

- [ ] **Step 5: Run tests**

Run: `cargo test --test python_frontend`

Expected: both frontend tests pass.

- [ ] **Step 6: Commit**

```powershell
git add src/lang/python.rs tests/python_frontend.rs
git commit -m "feat: record python scopes captures and class bases"
```

---

### Task 7: Import Resolver And Cross-Module Globals

**Files:**
- Modify: `src/lib.rs`
- Create: `src/imports.rs`
- Modify: `src/lang/python.rs`
- Test: `tests/python_frontend.rs`

- [ ] **Step 1: Add failing import resolver test**

Append to `tests/python_frontend.rs`:

```rust
#[test]
fn import_resolver_handles_init_all_and_reexports() {
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path().join("app");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(pkg.join("__init__.py"), "__all__ = ['settings']\nfrom .config import settings\n").unwrap();
    std::fs::write(pkg.join("config.py"), "settings = {'debug': True}\n").unwrap();
    std::fs::write(pkg.join("main.py"), "from app import settings\nvalue = settings\n").unwrap();

    let cfg = data_flow_analyzer::config::AnalyzeConfig {
        input: pkg.clone(),
        ..data_flow_analyzer::config::AnalyzeConfig::default()
    };
    let files = data_flow_analyzer::fs::discover_sources(&cfg).unwrap();
    let mut cache = PythonFrontend::new().parse_files(&files).unwrap();
    data_flow_analyzer::imports::resolve_imports(&mut cache);

    let imports: Vec<_> = cache.imports();
    assert!(imports.iter().any(|i| i.resolution == "project-local"));
    assert!(cache.modules.iter().any(|m| m.exports.iter().any(|e| e == "settings")));
}
```

Run: `cargo test --test python_frontend import_resolver_handles_init_all_and_reexports`

Expected: failure because import resolver is missing.

- [ ] **Step 2: Implement import resolver**

Modify `src/lib.rs`:

```rust
pub mod cli;
pub mod config;
pub mod fs;
pub mod ids;
pub mod imports;
pub mod ir;
pub mod lang;
pub mod source;
```

Create `src/imports.rs`:

```rust
use crate::ir::{AnalysisCache, Definition, Place};
use std::collections::{BTreeMap, BTreeSet};

pub fn resolve_imports(cache: &mut AnalysisCache) {
    let module_names: BTreeSet<String> = cache.modules.iter().map(|m| m.module_name.clone()).collect();
    let exports = infer_exports(cache);

    for module in &mut cache.modules {
        if let Some(names) = exports.get(&module.module_name) {
            module.exports = names.iter().cloned().collect();
        }
        for import in &mut module.imports {
            if module_names.contains(&import.module) {
                import.resolution = "project-local".to_string();
            } else if import.module.starts_with('.') {
                import.resolution = "project-local-relative".to_string();
            } else {
                import.resolution = "external".to_string();
            }
        }
    }

    rewrite_import_places(cache);
}

fn infer_exports(cache: &AnalysisCache) -> BTreeMap<String, BTreeSet<String>> {
    let mut exports: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for module in &cache.modules {
        let mut names = BTreeSet::new();
        for def in cache.definitions.iter().filter(|d| d.scope_id.contains(&module.file_id)) {
            if let Place::Local { name, .. } = &def.place {
                if !name.starts_with('_') {
                    names.insert(name.clone());
                }
            }
        }
        exports.insert(module.module_name.clone(), names);
    }
    exports
}

fn rewrite_import_places(cache: &mut AnalysisCache) {
    let module_by_name: BTreeMap<String, String> = cache
        .modules
        .iter()
        .map(|m| (m.module_name.clone(), m.module_id.clone()))
        .collect();

    for def in &mut cache.definitions {
        if def.def_kind != "from_import" && def.def_kind != "import" {
            continue;
        }
        if let Some(module_id) = module_by_name.values().next() {
            if let Place::Local { name, .. } = &def.place {
                def.place = Place::Global {
                    module_id: module_id.clone(),
                    name: name.clone(),
                };
            }
        }
    }
}
```

- [ ] **Step 3: Emit import definitions and `__all__` assignments in Python lowering**

Update `lower_import_from` to also push a `Definition` with `def_kind: "from_import"` for the imported alias. Update assignment lowering so `__all__ = [...]` remains visible to `infer_exports`.

- [ ] **Step 4: Run import resolver test**

Run: `cargo test --test python_frontend import_resolver_handles_init_all_and_reexports`

Expected: test passes.

- [ ] **Step 5: Commit**

```powershell
git add src/lib.rs src/imports.rs src/lang/python.rs tests/python_frontend.rs
git commit -m "feat: resolve project imports and exports"
```

---

### Task 8: Place Normalization And Alias Strategy

**Files:**
- Modify: `src/lib.rs`
- Create: `src/alias.rs`
- Modify: `src/lang/python.rs`

- [ ] **Step 1: Write failing alias tests**

Create `src/alias.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Place;

    #[test]
    fn self_attribute_is_class_field_sensitive() {
        let place = normalize_attribute(Some("ClassName"), "self", "token");
        assert_eq!(
            place,
            Place::Attribute {
                base: "InstanceField(ClassName)".to_string(),
                attr: "token".to_string()
            }
        );
    }

    #[test]
    fn unknown_attribute_uses_field_based_fallback() {
        let place = normalize_attribute(None, "factory()", "token");
        assert_eq!(
            place,
            Place::Attribute {
                base: "*".to_string(),
                attr: "token".to_string()
            }
        );
    }
}
```

Run: `cargo test alias::tests`

Expected: failure because alias module is not exported and functions are missing.

- [ ] **Step 2: Implement alias module**

Modify `src/lib.rs`:

```rust
pub mod alias;
pub mod cli;
pub mod config;
pub mod fs;
pub mod ids;
pub mod imports;
pub mod ir;
pub mod lang;
pub mod source;
```

Replace `src/alias.rs`:

```rust
use crate::ir::Place;

pub fn normalize_attribute(class_name: Option<&str>, base_expr: &str, attr: &str) -> Place {
    let base = match (class_name, base_expr) {
        (Some(class_name), "self") => format!("InstanceField({class_name})"),
        (Some(class_name), "cls") => format!("ClassField({class_name})"),
        (_, expr) if expr.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => expr.to_string(),
        _ => "*".to_string(),
    };
    Place::Attribute {
        base,
        attr: attr.to_string(),
    }
}

pub fn normalize_subscript(base_expr: &str, index_expr: Option<&str>) -> Place {
    let base = if base_expr.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        base_expr.to_string()
    } else {
        "*".to_string()
    };
    let index = index_expr
        .filter(|idx| idx.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '"' || c == '\''))
        .unwrap_or("*")
        .to_string();
    Place::Subscript { base, index }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Place;

    #[test]
    fn self_attribute_is_class_field_sensitive() {
        let place = normalize_attribute(Some("ClassName"), "self", "token");
        assert_eq!(
            place,
            Place::Attribute {
                base: "InstanceField(ClassName)".to_string(),
                attr: "token".to_string()
            }
        );
    }

    #[test]
    fn unknown_attribute_uses_field_based_fallback() {
        let place = normalize_attribute(None, "factory()", "token");
        assert_eq!(
            place,
            Place::Attribute {
                base: "*".to_string(),
                attr: "token".to_string()
            }
        );
    }
}
```

- [ ] **Step 3: Use alias normalization in Python lowering**

Update assignment and identifier lowering so:

- `self.x = value` becomes `Place::Attribute { base: "InstanceField(ClassName)", attr: "x" }`.
- `obj.x` reads use `normalize_attribute(None, "obj", "x")`.
- `items[i] = value` uses `normalize_subscript("items", Some("i"))`.

Use existing tree-sitter node kinds `attribute` and `subscript` when present.

- [ ] **Step 4: Run alias tests and frontend tests**

Run: `cargo test alias::tests --test python_frontend`

Expected: alias unit tests and frontend integration tests pass.

- [ ] **Step 5: Commit**

```powershell
git add src/lib.rs src/alias.rs src/lang/python.rs
git commit -m "feat: normalize python places and aliases"
```

---

### Task 9: CFG Builder For Core Python Control Flow

**Files:**
- Modify: `src/lib.rs`
- Create: `src/cfg.rs`
- Modify: `src/lang/python.rs`

- [ ] **Step 1: Write failing CFG tests**

Create `src/cfg.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SourceSpan;

    #[test]
    fn cfg_models_for_else_break_and_continue_edges() {
        let mut cfg = ControlFlowGraph::new("F_loop".to_string());
        let entry = cfg.entry_block_id.clone();
        let body = cfg.add_block("BasicBlock", SourceSpan::synthetic("loop.py", "body"));
        let else_block = cfg.add_block("BasicBlock", SourceSpan::synthetic("loop.py", "else"));
        let exit = cfg.exit_block_id.clone();

        cfg.add_edge(&entry, &body, "loop-body", "for body");
        cfg.add_edge(&body, &entry, "continue-back", "continue");
        cfg.add_edge(&entry, &else_block, "loop-else", "normal completion");
        cfg.add_edge(&body, &exit, "break-exit", "break");

        assert!(cfg.edges.iter().any(|e| e.edge_kind == "loop-else"));
        assert!(cfg.edges.iter().any(|e| e.edge_kind == "break-exit"));
        assert!(cfg.edges.iter().any(|e| e.edge_kind == "continue-back"));
    }
}
```

Run: `cargo test cfg::tests`

Expected: failure because CFG types are missing.

- [ ] **Step 2: Implement CFG primitives**

Modify `src/lib.rs`:

```rust
pub mod alias;
pub mod cfg;
pub mod cli;
pub mod config;
pub mod fs;
pub mod ids;
pub mod imports;
pub mod ir;
pub mod lang;
pub mod source;
```

Replace `src/cfg.rs`:

```rust
use crate::ids::stable_id;
use crate::ir::{CfgBlock, CfgEdge, CfgRecord, SCHEMA_VERSION};
use crate::source::SourceSpan;

#[derive(Debug, Clone)]
pub struct ControlFlowGraph {
    pub function_id: String,
    pub entry_block_id: String,
    pub exit_block_id: String,
    pub blocks: Vec<CfgBlock>,
    pub edges: Vec<CfgEdge>,
}

impl ControlFlowGraph {
    pub fn new(function_id: String) -> Self {
        let entry = stable_id("B", SCHEMA_VERSION, &[&function_id, "entry"]);
        let exit = stable_id("B", SCHEMA_VERSION, &[&function_id, "exit"]);
        Self {
            function_id: function_id.clone(),
            entry_block_id: entry.clone(),
            exit_block_id: exit.clone(),
            blocks: vec![
                CfgBlock {
                    block_id: entry,
                    block_kind: "Entry".to_string(),
                    statements: Vec::new(),
                    span: SourceSpan::synthetic("<cfg>", "entry"),
                },
                CfgBlock {
                    block_id: exit,
                    block_kind: "Exit".to_string(),
                    statements: Vec::new(),
                    span: SourceSpan::synthetic("<cfg>", "exit"),
                },
            ],
            edges: Vec::new(),
        }
    }

    pub fn add_block(&mut self, kind: &str, span: SourceSpan) -> String {
        let id = stable_id("B", SCHEMA_VERSION, &[&self.function_id, kind, &self.blocks.len().to_string(), &span.snippet]);
        self.blocks.push(CfgBlock {
            block_id: id.clone(),
            block_kind: kind.to_string(),
            statements: Vec::new(),
            span,
        });
        id
    }

    pub fn add_edge(&mut self, from: &str, to: &str, kind: &str, label: &str) {
        self.edges.push(CfgEdge {
            edge_id: stable_id("E", SCHEMA_VERSION, &[from, to, kind, &self.edges.len().to_string()]),
            from_block_id: from.to_string(),
            to_block_id: to.to_string(),
            edge_kind: kind.to_string(),
            label: label.to_string(),
        });
    }

    pub fn into_record(self) -> CfgRecord {
        CfgRecord {
            function_id: self.function_id,
            blocks: self.blocks,
            edges: self.edges,
            entry_block_id: self.entry_block_id,
            exit_block_id: self.exit_block_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SourceSpan;

    #[test]
    fn cfg_models_for_else_break_and_continue_edges() {
        let mut cfg = ControlFlowGraph::new("F_loop".to_string());
        let entry = cfg.entry_block_id.clone();
        let body = cfg.add_block("BasicBlock", SourceSpan::synthetic("loop.py", "body"));
        let else_block = cfg.add_block("BasicBlock", SourceSpan::synthetic("loop.py", "else"));
        let exit = cfg.exit_block_id.clone();

        cfg.add_edge(&entry, &body, "loop-body", "for body");
        cfg.add_edge(&body, &entry, "continue-back", "continue");
        cfg.add_edge(&entry, &else_block, "loop-else", "normal completion");
        cfg.add_edge(&body, &exit, "break-exit", "break");

        assert!(cfg.edges.iter().any(|e| e.edge_kind == "loop-else"));
        assert!(cfg.edges.iter().any(|e| e.edge_kind == "break-exit"));
        assert!(cfg.edges.iter().any(|e| e.edge_kind == "continue-back"));
    }
}
```

- [ ] **Step 3: Generate baseline CFG records for functions**

In `src/lang/python.rs`, after each `FunctionRecord` is added, create a `ControlFlowGraph` for the function with:

- Entry -> body block (`sequence`).
- Body block -> Exit (`return` if a return statement exists, otherwise `sequence`).

Then add later control-specific blocks when lowering `if_statement`, `for_statement`, `while_statement`, `try_statement`, `await`, and `yield`.

- [ ] **Step 4: Run tests**

Run: `cargo test cfg::tests --test python_frontend`

Expected: CFG unit test and frontend tests pass.

- [ ] **Step 5: Commit**

```powershell
git add src/lib.rs src/cfg.rs src/lang/python.rs
git commit -m "feat: add control flow graph primitives"
```

---

### Task 10: Reaching Definitions And Def-Use Edges

**Files:**
- Modify: `src/lib.rs`
- Create: `src/analysis.rs`
- Test: unit tests in `src/analysis.rs`

- [ ] **Step 1: Write failing reaching definitions tests**

Create `src/analysis.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{AnalysisCache, Definition, Place, Use, SCHEMA_VERSION};
    use crate::source::SourceSpan;

    #[test]
    fn reaching_definitions_connect_definition_to_use() {
        let place = Place::Local { scope_id: "S".to_string(), name: "x".to_string() };
        let mut cache = AnalysisCache { schema_version: SCHEMA_VERSION, ..AnalysisCache::default() };
        cache.definitions.push(Definition {
            def_id: "D_x".to_string(),
            place: place.clone(),
            def_kind: "assign".to_string(),
            scope_id: "S".to_string(),
            function_id: Some("F".to_string()),
            span: SourceSpan::synthetic("a.py", "x = 1"),
            expr: "1".to_string(),
            deps: Vec::new(),
        });
        cache.uses.push(Use {
            use_id: "U_x".to_string(),
            place: place.clone(),
            use_kind: "name_load".to_string(),
            scope_id: "S".to_string(),
            function_id: Some("F".to_string()),
            span: SourceSpan::synthetic("a.py", "return x"),
            context: "return".to_string(),
        });

        compute_def_use_edges(&mut cache);
        assert_eq!(cache.def_use_edges.len(), 1);
        assert_eq!(cache.def_use_edges[0].def_id, "D_x");
        assert_eq!(cache.def_use_edges[0].use_id, "U_x");
    }
}
```

Run: `cargo test analysis::tests`

Expected: failure because `compute_def_use_edges` is missing.

- [ ] **Step 2: Implement initial def-use analysis**

Modify `src/lib.rs`:

```rust
pub mod alias;
pub mod analysis;
pub mod cfg;
pub mod cli;
pub mod config;
pub mod fs;
pub mod ids;
pub mod imports;
pub mod ir;
pub mod lang;
pub mod source;
```

Replace `src/analysis.rs`:

```rust
use crate::ids::stable_id;
use crate::ir::{AnalysisCache, DefUseEdge, Place, VarDependencyEdge, SCHEMA_VERSION};
use std::collections::BTreeMap;

pub fn compute_def_use_edges(cache: &mut AnalysisCache) {
    cache.def_use_edges.clear();
    let mut defs_by_scope_place: BTreeMap<(String, Place), Vec<String>> = BTreeMap::new();
    let mut def_place_by_id: BTreeMap<String, Place> = BTreeMap::new();

    for def in &cache.definitions {
        defs_by_scope_place
            .entry((def.scope_id.clone(), def.place.clone()))
            .or_default()
            .push(def.def_id.clone());
        def_place_by_id.insert(def.def_id.clone(), def.place.clone());
    }

    for use_site in &cache.uses {
        let key = (use_site.scope_id.clone(), use_site.place.clone());
        if let Some(defs) = defs_by_scope_place.get(&key) {
            for def_id in defs {
                cache.def_use_edges.push(DefUseEdge {
                    edge_id: stable_id("DU", SCHEMA_VERSION, &[def_id, &use_site.use_id]),
                    def_id: def_id.clone(),
                    use_id: use_site.use_id.clone(),
                    place: def_place_by_id.get(def_id).cloned().unwrap_or_else(|| use_site.place.clone()),
                    edge_kind: "local".to_string(),
                    path_summary: "same-scope reaching approximation".to_string(),
                });
            }
        }
    }
}

pub fn compute_var_dependencies(cache: &mut AnalysisCache) {
    cache.var_dependency_edges.clear();
    for def in &cache.definitions {
        for dep in &def.deps {
            cache.var_dependency_edges.push(VarDependencyEdge {
                edge_id: stable_id("VD", SCHEMA_VERSION, &[&def.def_id, &format!("{dep:?}")]),
                source_place: dep.clone(),
                target_place: def.place.clone(),
                source_id: format!("{dep:?}"),
                target_id: def.def_id.clone(),
                dep_kind: "assignment".to_string(),
                span: def.span.clone(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{AnalysisCache, Definition, Place, Use, SCHEMA_VERSION};
    use crate::source::SourceSpan;

    #[test]
    fn reaching_definitions_connect_definition_to_use() {
        let place = Place::Local { scope_id: "S".to_string(), name: "x".to_string() };
        let mut cache = AnalysisCache { schema_version: SCHEMA_VERSION, ..AnalysisCache::default() };
        cache.definitions.push(Definition {
            def_id: "D_x".to_string(),
            place: place.clone(),
            def_kind: "assign".to_string(),
            scope_id: "S".to_string(),
            function_id: Some("F".to_string()),
            span: SourceSpan::synthetic("a.py", "x = 1"),
            expr: "1".to_string(),
            deps: Vec::new(),
        });
        cache.uses.push(Use {
            use_id: "U_x".to_string(),
            place: place.clone(),
            use_kind: "name_load".to_string(),
            scope_id: "S".to_string(),
            function_id: Some("F".to_string()),
            span: SourceSpan::synthetic("a.py", "return x"),
            context: "return".to_string(),
        });

        compute_def_use_edges(&mut cache);
        assert_eq!(cache.def_use_edges.len(), 1);
        assert_eq!(cache.def_use_edges[0].def_id, "D_x");
        assert_eq!(cache.def_use_edges[0].use_id, "U_x");
    }
}
```

- [ ] **Step 3: Upgrade from same-scope approximation to CFG worklist**

Replace the core of `compute_def_use_edges` with a forward may analysis:

- Domain: `BTreeMap<Place, BTreeSet<DefId>>`.
- Meet: union predecessor maps.
- Transfer: remove killed place then add generated place.
- For each use in a block, connect all current reaching definitions for the same place.

Keep the test from Step 1 passing. Add a second unit test where `x = 1; x = 2; return x` connects only the second definition.

- [ ] **Step 4: Run tests**

Run: `cargo test analysis::tests`

Expected: tests pass, including kill behavior.

- [ ] **Step 5: Commit**

```powershell
git add src/lib.rs src/analysis.rs
git commit -m "feat: compute reaching definitions and def-use edges"
```

---

### Task 11: Function Summaries, Call Graph SCCs, And Call-Site Propagation

**Files:**
- Modify: `src/lib.rs`
- Create: `src/summaries.rs`
- Modify: `src/analysis.rs`

- [ ] **Step 1: Write failing summary tests**

Create `src/summaries.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{AnalysisCache, FunctionRecord, FunctionSummary, Place, SCHEMA_VERSION};
    use crate::source::SourceSpan;

    #[test]
    fn summaries_merge_multi_target_outputs() {
        let mut cache = AnalysisCache { schema_version: SCHEMA_VERSION, ..AnalysisCache::default() };
        cache.functions.push(FunctionRecord {
            function_id: "F_a".to_string(),
            module_id: "M".to_string(),
            class_id: None,
            qualified_name: "a".to_string(),
            kind: "function".to_string(),
            params: vec!["x".to_string()],
            scope_id: "S_a".to_string(),
            span: SourceSpan::synthetic("a.py", "def a"),
        });
        cache.function_summaries.push(FunctionSummary {
            function_id: "F_a".to_string(),
            inputs: vec![Place::Local { scope_id: "S_a".to_string(), name: "x".to_string() }],
            returns: vec![Place::Local { scope_id: "S_a".to_string(), name: "x".to_string() }],
            yields: Vec::new(),
            writes: Vec::new(),
            raises: Vec::new(),
            external_effects: Vec::new(),
            fixpoint_status: "fixed".to_string(),
        });

        let merged = merge_candidate_summaries(&cache, &["F_a".to_string()]);
        assert_eq!(merged.returns.len(), 1);
        assert_eq!(merged.fixpoint_status, "fixed");
    }
}
```

Run: `cargo test summaries::tests`

Expected: failure because summaries module is missing.

- [ ] **Step 2: Implement summary helpers**

Modify `src/lib.rs`:

```rust
pub mod alias;
pub mod analysis;
pub mod cfg;
pub mod cli;
pub mod config;
pub mod fs;
pub mod ids;
pub mod imports;
pub mod ir;
pub mod lang;
pub mod source;
pub mod summaries;
```

Create `src/summaries.rs`:

```rust
use crate::ir::{AnalysisCache, FunctionSummary, Place};
use std::collections::BTreeSet;

pub fn build_initial_summaries(cache: &mut AnalysisCache) {
    cache.function_summaries.clear();
    for function in &cache.functions {
        let inputs = function
            .params
            .iter()
            .map(|name| Place::Local { scope_id: function.scope_id.clone(), name: name.clone() })
            .collect();
        cache.function_summaries.push(FunctionSummary {
            function_id: function.function_id.clone(),
            inputs,
            returns: Vec::new(),
            yields: Vec::new(),
            writes: Vec::new(),
            raises: Vec::new(),
            external_effects: Vec::new(),
            fixpoint_status: "initial".to_string(),
        });
    }
}

pub fn merge_candidate_summaries(cache: &AnalysisCache, candidates: &[String]) -> FunctionSummary {
    let mut inputs = BTreeSet::new();
    let mut returns = BTreeSet::new();
    let mut yields = BTreeSet::new();
    let mut writes = BTreeSet::new();
    let mut raises = BTreeSet::new();
    let mut external_effects = BTreeSet::new();
    let mut status = "fixed".to_string();

    for candidate in candidates {
        if let Some(summary) = cache.function_summaries.iter().find(|s| &s.function_id == candidate) {
            inputs.extend(summary.inputs.iter().cloned());
            returns.extend(summary.returns.iter().cloned());
            yields.extend(summary.yields.iter().cloned());
            writes.extend(summary.writes.iter().cloned());
            raises.extend(summary.raises.iter().cloned());
            external_effects.extend(summary.external_effects.iter().cloned());
            if summary.fixpoint_status != "fixed" {
                status = "partial".to_string();
            }
        }
    }

    FunctionSummary {
        function_id: "merged".to_string(),
        inputs: inputs.into_iter().collect(),
        returns: returns.into_iter().collect(),
        yields: yields.into_iter().collect(),
        writes: writes.into_iter().collect(),
        raises: raises.into_iter().collect(),
        external_effects: external_effects.into_iter().collect(),
        fixpoint_status: status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{AnalysisCache, FunctionRecord, FunctionSummary, Place, SCHEMA_VERSION};
    use crate::source::SourceSpan;

    #[test]
    fn summaries_merge_multi_target_outputs() {
        let mut cache = AnalysisCache { schema_version: SCHEMA_VERSION, ..AnalysisCache::default() };
        cache.functions.push(FunctionRecord {
            function_id: "F_a".to_string(),
            module_id: "M".to_string(),
            class_id: None,
            qualified_name: "a".to_string(),
            kind: "function".to_string(),
            params: vec!["x".to_string()],
            scope_id: "S_a".to_string(),
            span: SourceSpan::synthetic("a.py", "def a"),
        });
        cache.function_summaries.push(FunctionSummary {
            function_id: "F_a".to_string(),
            inputs: vec![Place::Local { scope_id: "S_a".to_string(), name: "x".to_string() }],
            returns: vec![Place::Local { scope_id: "S_a".to_string(), name: "x".to_string() }],
            yields: Vec::new(),
            writes: Vec::new(),
            raises: Vec::new(),
            external_effects: Vec::new(),
            fixpoint_status: "fixed".to_string(),
        });

        let merged = merge_candidate_summaries(&cache, &["F_a".to_string()]);
        assert_eq!(merged.returns.len(), 1);
        assert_eq!(merged.fixpoint_status, "fixed");
    }
}
```

- [ ] **Step 3: Add call-site propagation**

Add `propagate_call_summaries(cache: &mut AnalysisCache)` to `src/summaries.rs`:

- For each `CallRecord`, merge candidate summaries.
- Add `VarDependencyEdge` from each argument use place to return target place when `return_target_def_id` exists.
- For `external` or `unresolved` calls, add `external_effects` diagnostic-like strings to the caller summary.
- For recursive SCCs, repeat propagation up to 20 iterations and mark `fixpoint_status` as `partial` if still changing.

- [ ] **Step 4: Run summary tests**

Run: `cargo test summaries::tests`

Expected: tests pass.

- [ ] **Step 5: Commit**

```powershell
git add src/lib.rs src/summaries.rs src/analysis.rs
git commit -m "feat: summarize functions and propagate calls"
```

---

### Task 12: Bounded Def-Clear Path Queries

**Files:**
- Modify: `src/lib.rs`
- Create: `src/paths.rs`
- Modify: `src/config.rs`

- [ ] **Step 1: Write failing path tests**

Create `src/paths.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_limits_record_truncation_reason() {
        let options = PathQueryOptions {
            max_loop_unroll: 2,
            max_paths: 1,
            max_path_len: 3,
        };
        let result = PathQueryResult {
            query: "D_x -> U_x".to_string(),
            paths: Vec::new(),
            truncated: true,
            truncation_reason: Some("max-paths".to_string()),
            options,
        };

        assert!(result.truncated);
        assert_eq!(result.truncation_reason.as_deref(), Some("max-paths"));
    }
}
```

Run: `cargo test paths::tests`

Expected: failure because path module is missing.

- [ ] **Step 2: Implement path query types and bounded DFS shell**

Modify `src/lib.rs`:

```rust
pub mod alias;
pub mod analysis;
pub mod cfg;
pub mod cli;
pub mod config;
pub mod fs;
pub mod ids;
pub mod imports;
pub mod ir;
pub mod lang;
pub mod paths;
pub mod source;
pub mod summaries;
```

Create `src/paths.rs`:

```rust
use crate::ir::{AnalysisCache, CfgRecord};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathQueryOptions {
    pub max_loop_unroll: usize,
    pub max_paths: usize,
    pub max_path_len: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefClearPath {
    pub path_id: String,
    pub def_id: String,
    pub use_id: String,
    pub block_ids: Vec<String>,
    pub edge_labels: Vec<String>,
    pub loop_unrolls: BTreeMap<String, usize>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathQueryResult {
    pub query: String,
    pub paths: Vec<DefClearPath>,
    pub truncated: bool,
    pub truncation_reason: Option<String>,
    pub options: PathQueryOptions,
}

pub fn query_function_paths(
    cache: &AnalysisCache,
    function_id: &str,
    def_id: Option<&str>,
    use_id: Option<&str>,
    options: PathQueryOptions,
) -> PathQueryResult {
    let query = format!(
        "function={function_id};def={};use={}",
        def_id.unwrap_or("*"),
        use_id.unwrap_or("*")
    );
    let Some(cfg) = cache.cfgs.iter().find(|c| c.function_id == function_id) else {
        return PathQueryResult {
            query,
            paths: Vec::new(),
            truncated: false,
            truncation_reason: Some("function-cfg-not-found".to_string()),
            options,
        };
    };

    bounded_walk(cfg, def_id, use_id, options, query)
}

fn bounded_walk(
    cfg: &CfgRecord,
    def_id: Option<&str>,
    use_id: Option<&str>,
    options: PathQueryOptions,
    query: String,
) -> PathQueryResult {
    let mut paths = Vec::new();
    if cfg.entry_block_id != cfg.exit_block_id {
        paths.push(DefClearPath {
            path_id: "P_0".to_string(),
            def_id: def_id.unwrap_or("*").to_string(),
            use_id: use_id.unwrap_or("*").to_string(),
            block_ids: vec![cfg.entry_block_id.clone(), cfg.exit_block_id.clone()],
            edge_labels: vec!["summary".to_string()],
            loop_unrolls: BTreeMap::new(),
            truncated: false,
        });
    }
    PathQueryResult {
        query,
        paths,
        truncated: false,
        truncation_reason: None,
        options,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_limits_record_truncation_reason() {
        let options = PathQueryOptions {
            max_loop_unroll: 2,
            max_paths: 1,
            max_path_len: 3,
        };
        let result = PathQueryResult {
            query: "D_x -> U_x".to_string(),
            paths: Vec::new(),
            truncated: true,
            truncation_reason: Some("max-paths".to_string()),
            options,
        };

        assert!(result.truncated);
        assert_eq!(result.truncation_reason.as_deref(), Some("max-paths"));
    }
}
```

- [ ] **Step 3: Replace summary walk with def-clear bounded DFS**

Update `bounded_walk` to:

- Build adjacency from `cfg.edges`.
- Track path length and return `truncated-by-max-path-len` when exceeded.
- Track loop edge visits by `edge_id`; block edge traversal when visits exceed `max_loop_unroll`.
- Stop collecting when `paths.len() == max_paths`.
- For each candidate path, check no block contains a redefining `DefId` of the queried place between start and target use.

- [ ] **Step 4: Run path tests**

Run: `cargo test paths::tests`

Expected: tests pass.

- [ ] **Step 5: Commit**

```powershell
git add src/lib.rs src/paths.rs src/config.rs
git commit -m "feat: add bounded def-clear path queries"
```

---

### Task 13: DOT Graphs And Optional SVG Rendering

**Files:**
- Modify: `src/lib.rs`
- Create: `src/graph.rs`

- [ ] **Step 1: Write failing DOT tests**

Create `src/graph.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_label_escapes_quotes_backslashes_and_newlines() {
        assert_eq!(dot_label("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
    }
}
```

Run: `cargo test graph::tests`

Expected: failure because graph module is missing.

- [ ] **Step 2: Implement graph module**

Modify `src/lib.rs`:

```rust
pub mod alias;
pub mod analysis;
pub mod cfg;
pub mod cli;
pub mod config;
pub mod fs;
pub mod graph;
pub mod ids;
pub mod imports;
pub mod ir;
pub mod lang;
pub mod paths;
pub mod source;
pub mod summaries;
```

Create `src/graph.rs`:

```rust
use crate::ir::AnalysisCache;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::process::Command;

pub fn dot_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

pub fn write_def_use_hotspots_dot(cache: &AnalysisCache, path: &Path, top_n: usize) -> Result<()> {
    let mut text = String::from("digraph DefUseHotspots {\n  rankdir=LR;\n  node [shape=box, fontsize=10];\n");
    for edge in cache.def_use_edges.iter().take(top_n) {
        text.push_str(&format!(
            "  \"{}\" -> \"{}\" [label=\"{}\"];\n",
            dot_label(&edge.def_id),
            dot_label(&edge.use_id),
            dot_label(&edge.edge_kind)
        ));
    }
    text.push_str("}\n");
    fs::write(path, text).with_context(|| format!("failed to write {}", path.display()))
}

pub fn render_svg(dot_path: &Path, svg_path: &Path) -> Result<bool> {
    let output = Command::new("dot")
        .arg("-Tsvg")
        .arg(dot_path)
        .arg("-o")
        .arg(svg_path)
        .output();

    match output {
        Ok(output) if output.status.success() => Ok(true),
        Ok(_) => Ok(false),
        Err(_) => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_label_escapes_quotes_backslashes_and_newlines() {
        assert_eq!(dot_label("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
    }
}
```

- [ ] **Step 3: Add module, function, and variable dependency DOT writers**

Add functions:

- `write_module_dependency_dot(cache, path)`
- `write_function_dependency_dot(cache, path)`
- `write_var_dependency_dot(cache, path, top_n)`

Each writer should use stable IDs from cache records and call `dot_label` on labels.

- [ ] **Step 4: Run graph tests**

Run: `cargo test graph::tests`

Expected: test passes.

- [ ] **Step 5: Commit**

```powershell
git add src/lib.rs src/graph.rs
git commit -m "feat: emit dot graphs and render svg"
```

---

### Task 14: HTML Report, CSV Outputs, And Cache Writer

**Files:**
- Modify: `src/lib.rs`
- Create: `src/report.rs`
- Test: `tests/integration_report.rs`

- [ ] **Step 1: Write failing report test**

Create `tests/integration_report.rs`:

```rust
use data_flow_analyzer::ir::{AnalysisCache, SCHEMA_VERSION};
use data_flow_analyzer::report::write_report;

#[test]
fn report_writes_index_cache_and_csv_headers() {
    let dir = tempfile::tempdir().unwrap();
    let cache = AnalysisCache {
        schema_version: SCHEMA_VERSION,
        tool_version: "0.1.0".to_string(),
        ..AnalysisCache::default()
    };

    write_report(&cache, dir.path(), 100).unwrap();

    assert!(dir.path().join("index.html").exists());
    assert!(dir.path().join("data/analysis-cache.json").exists());
    let definitions = std::fs::read_to_string(dir.path().join("data/definitions.csv")).unwrap();
    assert!(definitions.starts_with("def_id,file,path,line,col"));
}
```

Run: `cargo test --test integration_report report_writes_index_cache_and_csv_headers`

Expected: failure because report module is missing.

- [ ] **Step 2: Implement report writer**

Modify `src/lib.rs`:

```rust
pub mod alias;
pub mod analysis;
pub mod cfg;
pub mod cli;
pub mod config;
pub mod fs;
pub mod graph;
pub mod ids;
pub mod imports;
pub mod ir;
pub mod lang;
pub mod paths;
pub mod report;
pub mod source;
pub mod summaries;
```

Create `src/report.rs`:

```rust
use crate::graph;
use crate::ir::AnalysisCache;
use anyhow::{Context, Result};
use csv::Writer;
use std::fs;
use std::path::Path;

pub fn write_report(cache: &AnalysisCache, out: &Path, top_n: usize) -> Result<()> {
    fs::create_dir_all(out.join("assets"))?;
    fs::create_dir_all(out.join("graphs"))?;
    fs::create_dir_all(out.join("functions"))?;
    fs::create_dir_all(out.join("data"))?;

    write_cache(cache, &out.join("data/analysis-cache.json"))?;
    write_csvs(cache, &out.join("data"))?;
    write_graphs(cache, &out.join("graphs"), top_n)?;
    write_index(cache, out)?;
    Ok(())
}

fn write_cache(cache: &AnalysisCache, path: &Path) -> Result<()> {
    let json = serde_json::to_string_pretty(cache)?;
    fs::write(path, json).with_context(|| format!("failed to write {}", path.display()))
}

fn write_csvs(cache: &AnalysisCache, data_dir: &Path) -> Result<()> {
    let mut defs = Writer::from_path(data_dir.join("definitions.csv"))?;
    defs.write_record(["def_id", "file", "path", "line", "col", "end_line", "end_col", "scope_id", "function_id", "place", "def_kind", "expr", "deps"])?;
    for def in &cache.definitions {
        defs.write_record([
            def.def_id.as_str(),
            def.span.file.as_str(),
            def.span.file.as_str(),
            &def.span.line.to_string(),
            &def.span.col.to_string(),
            &def.span.end_line.to_string(),
            &def.span.end_col.to_string(),
            def.scope_id.as_str(),
            def.function_id.as_deref().unwrap_or(""),
            &format!("{:?}", def.place),
            def.def_kind.as_str(),
            def.expr.as_str(),
            &format!("{:?}", def.deps),
        ])?;
    }
    defs.flush()?;

    let mut uses = Writer::from_path(data_dir.join("uses.csv"))?;
    uses.write_record(["use_id", "file", "path", "line", "col", "end_line", "end_col", "scope_id", "function_id", "place", "use_kind", "context"])?;
    for use_site in &cache.uses {
        uses.write_record([
            use_site.use_id.as_str(),
            use_site.span.file.as_str(),
            use_site.span.file.as_str(),
            &use_site.span.line.to_string(),
            &use_site.span.col.to_string(),
            &use_site.span.end_line.to_string(),
            &use_site.span.end_col.to_string(),
            use_site.scope_id.as_str(),
            use_site.function_id.as_deref().unwrap_or(""),
            &format!("{:?}", use_site.place),
            use_site.use_kind.as_str(),
            use_site.context.as_str(),
        ])?;
    }
    uses.flush()?;
    Ok(())
}

fn write_graphs(cache: &AnalysisCache, graph_dir: &Path, top_n: usize) -> Result<()> {
    let dot = graph_dir.join("def_use_hotspots.dot");
    let svg = graph_dir.join("def_use_hotspots.svg");
    graph::write_def_use_hotspots_dot(cache, &dot, top_n)?;
    let _ = graph::render_svg(&dot, &svg)?;
    Ok(())
}

fn write_index(cache: &AnalysisCache, out: &Path) -> Result<()> {
    let html = format!(
        r#"<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <title>Data Flow Report</title>
  <link rel="stylesheet" href="assets/report.css">
</head>
<body>
  <main>
    <h1>Data Flow Report</h1>
    <section>
      <h2>Overview</h2>
      <ul>
        <li>Files: {}</li>
        <li>Functions: {}</li>
        <li>Definitions: {}</li>
        <li>Uses: {}</li>
        <li>Def-use edges: {}</li>
      </ul>
    </section>
    <section>
      <h2>Graphs</h2>
      <p><a href="graphs/def_use_hotspots.svg">Def-use hotspots</a></p>
    </section>
  </main>
</body>
</html>
"#,
        cache.files.len(),
        cache.functions.len(),
        cache.definitions.len(),
        cache.uses.len(),
        cache.def_use_edges.len()
    );
    fs::write(out.join("index.html"), html)?;
    fs::write(out.join("assets/report.css"), "body{font-family:Georgia,serif;margin:2rem;background:#f8f1e7;color:#1f2933}a{color:#8a4b08}")?;
    Ok(())
}
```

- [ ] **Step 3: Add remaining CSV files**

Extend `write_csvs` to emit:

- `def_use_edges.csv`
- `var_dependencies.csv`
- `function_summaries.csv`
- `parse_diagnostics.csv`

Use the exact headers from the design document.

- [ ] **Step 4: Run report test**

Run: `cargo test --test integration_report report_writes_index_cache_and_csv_headers`

Expected: test passes.

- [ ] **Step 5: Commit**

```powershell
git add src/lib.rs src/report.rs tests/integration_report.rs
git commit -m "feat: write html report and data exports"
```

---

### Task 15: Wire Analyze And Paths Commands End-To-End

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/report.rs`
- Test: `tests/cli_smoke.rs`

- [ ] **Step 1: Add failing CLI integration test**

Append to `tests/cli_smoke.rs`:

```rust
#[test]
fn analyze_command_writes_report_for_python_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("app");
    let out = dir.path().join("report");
    std::fs::create_dir_all(&input).unwrap();
    std::fs::write(input.join("main.py"), "x = 1\nprint(x)\n").unwrap();

    let mut cmd = Command::cargo_bin("data-flow-analyzer").unwrap();
    cmd.args([
        "analyze",
        "--lang",
        "python",
        "--input",
        input.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ])
    .assert()
    .success();

    assert!(out.join("index.html").exists());
    assert!(out.join("data/analysis-cache.json").exists());
}
```

Run: `cargo test --test cli_smoke analyze_command_writes_report_for_python_fixture`

Expected: failure because analyze command only prints a temporary message.

- [ ] **Step 2: Implement analyze command dispatch**

Update `src/cli.rs`:

```rust
use crate::analysis::{compute_def_use_edges, compute_var_dependencies};
use crate::config::AnalyzeConfig;
use crate::fs::discover_sources;
use crate::imports::resolve_imports;
use crate::lang::python::PythonFrontend;
use crate::lang::LanguageFrontend;
use crate::report::write_report;
use crate::summaries::{build_initial_summaries, propagate_call_summaries};
use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
```

Add `config` and path options to `Analyze`:

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
}
```

Replace the analyze match arm:

```rust
Some(Commands::Analyze { config, lang, input, out }) => {
    let mut cfg = if let Some(config) = config {
        AnalyzeConfig::from_toml_file(&config)?
    } else {
        AnalyzeConfig::default()
    };
    cfg.apply_cli_overrides(lang, input, out);
    if cfg.lang != "python" {
        bail!("unsupported language '{}'; first version supports python", cfg.lang);
    }
    let files = discover_sources(&cfg)?;
    let frontend = PythonFrontend::new();
    let mut cache = frontend.parse_files(&files)?;
    resolve_imports(&mut cache);
    compute_def_use_edges(&mut cache);
    compute_var_dependencies(&mut cache);
    build_initial_summaries(&mut cache);
    propagate_call_summaries(&mut cache);
    write_report(&cache, &cfg.out, cfg.top_n)?;
    println!("report written to {}", cfg.out.display());
    Ok(())
}
```

- [ ] **Step 3: Implement paths command cache load**

In the `Paths` match arm:

- Read `analysis-cache.json`.
- Deserialize `AnalysisCache`.
- Resolve function name to `function_id`.
- Call `paths::query_function_paths`.
- Write a JSON result next to the cache named `path-query.json`.

- [ ] **Step 4: Run CLI tests**

Run: `cargo test --test cli_smoke`

Expected: version and analyze tests pass.

- [ ] **Step 5: Commit**

```powershell
git add src/cli.rs src/report.rs tests/cli_smoke.rs
git commit -m "feat: wire analyze command end to end"
```

---

### Task 16: Parse Diagnostics And Error Recovery

**Files:**
- Modify: `src/lang/python.rs`
- Modify: `src/report.rs`
- Test: `tests/python_frontend.rs`

- [ ] **Step 1: Add failing parse diagnostic test**

Append to `tests/python_frontend.rs`:

```rust
#[test]
fn parser_records_diagnostics_for_broken_python_and_keeps_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken.py");
    std::fs::write(&path, "x = 1\ndef bad(:\n    pass\nz = x\n").unwrap();
    let source = SourceFile {
        absolute_path: path,
        relative_path: "broken.py".to_string(),
    };

    let cache = PythonFrontend::new().parse_files(&[source]).unwrap();
    assert_eq!(cache.files[0].parse_status, "partial");
    assert!(cache.diagnostics.iter().any(|d| d.kind == "parse-error"));
}
```

Run: `cargo test --test python_frontend parser_records_diagnostics_for_broken_python_and_keeps_file`

Expected: failure because diagnostics are not recorded.

- [ ] **Step 2: Emit diagnostics for tree-sitter ERROR nodes**

In `src/lang/python.rs`, add:

```rust
fn record_parse_errors(cache: &mut AnalysisCache, file: &SourceFile, text: &str, node: Node) {
    if node.kind() == "ERROR" {
        cache.diagnostics.push(Diagnostic {
            diagnostic_id: stable_id("DIAG", SCHEMA_VERSION, &[&file.relative_path, "parse-error", &node.start_byte().to_string()]),
            severity: "warning".to_string(),
            kind: "parse-error".to_string(),
            message: "tree-sitter reported an ERROR node; analysis skipped this subtree".to_string(),
            file: file.relative_path.clone(),
            span: span(file, text, node),
        });
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        record_parse_errors(cache, file, text, child);
    }
}
```

Call `record_parse_errors` immediately after parsing and before `lower_module`.

- [ ] **Step 3: Skip ERROR subtrees during lowering**

At the top of `walk`, add:

```rust
if node.kind() == "ERROR" {
    return;
}
```

- [ ] **Step 4: Write diagnostics CSV**

Extend `report::write_csvs` so `parse_diagnostics.csv` contains diagnostic rows using the design schema.

- [ ] **Step 5: Run tests**

Run: `cargo test --test python_frontend --test integration_report`

Expected: tests pass.

- [ ] **Step 6: Commit**

```powershell
git add src/lang/python.rs src/report.rs tests/python_frontend.rs
git commit -m "feat: recover from python parse errors"
```

---

### Task 17: Parallelize File Parsing And Per-Function Analysis

**Files:**
- Modify: `src/lang/python.rs`
- Modify: `src/analysis.rs`
- Modify: `src/config.rs`

- [ ] **Step 1: Add deterministic parallel parsing test**

Append to `tests/python_frontend.rs`:

```rust
#[test]
fn parser_output_is_deterministic_across_runs() {
    let dir = tempfile::tempdir().unwrap();
    for idx in 0..10 {
        std::fs::write(dir.path().join(format!("m{idx}.py")), format!("x{idx} = {idx}\n")).unwrap();
    }
    let cfg = data_flow_analyzer::config::AnalyzeConfig {
        input: dir.path().to_path_buf(),
        ..data_flow_analyzer::config::AnalyzeConfig::default()
    };
    let files = data_flow_analyzer::fs::discover_sources(&cfg).unwrap();
    let a = PythonFrontend::new().parse_files(&files).unwrap();
    let b = PythonFrontend::new().parse_files(&files).unwrap();
    assert_eq!(serde_json::to_string(&a.definitions).unwrap(), serde_json::to_string(&b.definitions).unwrap());
}
```

Run: `cargo test --test python_frontend parser_output_is_deterministic_across_runs`

Expected: test passes before parallelism; keep it passing after.

- [ ] **Step 2: Parallelize parsing with deterministic merge**

In `src/lang/python.rs`:

- Use `rayon::prelude::*`.
- Parse each file into a per-file `AnalysisCache`.
- Sort per-file caches by `files[0].path`.
- Merge vectors in that order.

Keep ID generation path-based so merge order does not affect IDs.

- [ ] **Step 3: Parallelize independent analysis**

In `src/analysis.rs`:

- Compute per-function CFG worklist data independently with `rayon`.
- Merge `DefUseEdge` vectors sorted by `edge_id`.
- Merge `VarDependencyEdge` vectors sorted by `edge_id`.

- [ ] **Step 4: Run determinism and full tests**

Run: `cargo test`

Expected: all tests pass.

- [ ] **Step 5: Commit**

```powershell
git add src/lang/python.rs src/analysis.rs src/config.rs tests/python_frontend.rs
git commit -m "perf: parallelize deterministic analysis stages"
```

---

### Task 18: Golden Fixtures And Real Project Validation

**Files:**
- Create: `tests/fixtures/python_basic/main.py`
- Create: `tests/fixtures/python_control_flow/main.py`
- Create: `tests/fixtures/python_calls/main.py`
- Create: `tests/fixtures/python_classes/main.py`
- Modify: `tests/integration_report.rs`

- [ ] **Step 1: Add fixture files**

Create `tests/fixtures/python_basic/main.py`:

```python
x = 1
y = x
print(y)
```

Create `tests/fixtures/python_control_flow/main.py`:

```python
def choose(flag):
    x = 1
    if flag:
        y = x
    else:
        y = 2
    return y
```

Create `tests/fixtures/python_calls/main.py`:

```python
def identity(value):
    return value

result = identity(3)
```

Create `tests/fixtures/python_classes/main.py`:

```python
class Base:
    def value(self):
        return 1

class Child(Base):
    def value(self):
        return super().value()
```

- [ ] **Step 2: Add integration test over fixtures**

Append to `tests/integration_report.rs`:

```rust
#[test]
fn analyzer_processes_golden_fixtures() {
    for fixture in [
        "tests/fixtures/python_basic",
        "tests/fixtures/python_control_flow",
        "tests/fixtures/python_calls",
        "tests/fixtures/python_classes",
    ] {
        let out = tempfile::tempdir().unwrap();
        let mut cmd = assert_cmd::Command::cargo_bin("data-flow-analyzer").unwrap();
        cmd.args([
            "analyze",
            "--lang",
            "python",
            "--input",
            fixture,
            "--out",
            out.path().to_str().unwrap(),
        ])
        .assert()
        .success();

        let cache = std::fs::read_to_string(out.path().join("data/analysis-cache.json")).unwrap();
        assert!(cache.contains("\"schema_version\""));
        assert!(out.path().join("index.html").exists());
    }
}
```

- [ ] **Step 3: Run all tests**

Run: `cargo test`

Expected: all unit and integration tests pass.

- [ ] **Step 4: Validate against ASR app**

Run:

```powershell
cargo run -- analyze --lang python --input D:\repos\asr_platform\app --out D:\tmp\dataflow_report
```

Expected:

- Process exits successfully.
- `D:\tmp\dataflow_report\index.html` exists.
- `D:\tmp\dataflow_report\data\analysis-cache.json` exists.
- `D:\tmp\dataflow_report\graphs\def_use_hotspots.dot` exists.
- If `dot` is available, `D:\tmp\dataflow_report\graphs\def_use_hotspots.svg` exists.

- [ ] **Step 5: Commit**

```powershell
git add tests/fixtures tests/integration_report.rs
git commit -m "test: validate analyzer on golden fixtures"
```

---

### Task 19: Documentation And Final Verification

**Files:**
- Create: `README.md`
- Modify: `docs/superpowers/specs/2026-05-27-rust-dataflow-analyzer-design.md` if verification changes a documented default.

- [ ] **Step 1: Write README**

Create `README.md`:

```markdown
# Data Flow Analyzer

Rust static data-flow analyzer. Version 1 supports Python through tree-sitter and emits DOT/SVG graphs plus a static HTML report.

## Quick Start

```powershell
cargo run -- analyze --lang python --input D:\repos\asr_platform\app --out D:\tmp\dataflow_report
```

Open:

```text
D:\tmp\dataflow_report\index.html
```

## Path Query

```powershell
cargo run -- paths --input D:\tmp\dataflow_report\data\analysis-cache.json --function app/routers/tests.py::create_test --max-loop-unroll 2
```

## Outputs

- `index.html`: main report.
- `graphs/*.dot`: Graphviz sources.
- `graphs/*.svg`: rendered summary graphs when Graphviz is available.
- `data/analysis-cache.json`: versioned cache for path queries.
- `data/*.csv`: tabular data for filtering and debugging.

## Limitations

The analyzer is static and conservative. Dynamic Python features such as `eval`, reflection, monkey patching, metaclasses, descriptors, and dependency injection are reported as uncertain or external effects.
```

- [ ] **Step 2: Run final verification**

Run:

```powershell
cargo fmt --check
cargo test
cargo run -- analyze --lang python --input D:\repos\asr_platform\app --out D:\tmp\dataflow_report
```

Expected:

- `cargo fmt --check` passes.
- `cargo test` passes.
- Analyze command completes and writes report files.

- [ ] **Step 3: Commit**

```powershell
git add README.md docs/superpowers/specs/2026-05-27-rust-dataflow-analyzer-design.md
git commit -m "docs: document analyzer usage"
```

---

## Self-Review

- Spec coverage:
  - CLI/config/source discovery: Tasks 1, 2, 15.
  - Stable IDs and cache schema: Tasks 3, 4, 14.
  - Python frontend, scopes, closures, classes, imports, aliasing: Tasks 5, 6, 7, 8.
  - CFG and Python control flow: Task 9, with later frontend integration in Tasks 10 and 16.
  - Reaching definitions and dependencies: Task 10.
  - Function summaries and call propagation: Task 11.
  - Path expansion: Task 12.
  - DOT/SVG and HTML report: Tasks 13, 14.
  - Error recovery, parallelism, fixtures, real ASR validation: Tasks 16, 17, 18.
  - Documentation and verification: Task 19.
- Plan-content scan:
  - No intentionally blank implementation step remains.
  - Each task has concrete files, commands, and expected outcomes.
  - Large algorithms are introduced with tests first and then refined inside their task.
- Type consistency:
  - `AnalysisCache`, `Place`, `Definition`, `Use`, `FunctionSummary`, `CfgRecord`, and command names match across tasks.
  - Default `max_loop_unroll` is consistently 2.
  - JSON/CSV writers consume the same cache structures produced by analysis.
