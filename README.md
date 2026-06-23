# Data Flow Analyzer

Rust static data-flow analyzer. The current implementation targets Python via
tree-sitter and produces:

- def-use analysis
- variable-dependency analysis
- DOT graph sources
- optional SVG renders
- a static HTML report
- machine-readable JSON/CSV exports

## Current Scope

The current release supports:

- Python parsing and lowering into a language-neutral IR
- definitions, uses, imports, captures, and normalized attribute/subscript places
- baseline CFG construction per function
- reaching-definitions analysis
- def-use edges
- variable-dependency edges
- static report generation
- function-level def-clear path queries
- parse diagnostics while keeping partially parsed files in the output

## Build

Debug run:

```powershell
cargo run -- --help
```

Release build:

```powershell
cargo build --release
```

Release binary:

```text
target\release\data-flow-analyzer.exe
```

## Analyze A Codebase

Using `cargo run`:

```powershell
cargo run -- analyze --lang python --input D:\repos\asr_platform\app --out D:\tmp\dataflow_report
```

Using the release binary:

```powershell
target\release\data-flow-analyzer.exe analyze --lang python --input D:\repos\asr_platform\app --out D:\tmp\dataflow_report
```

Open the generated report:

```text
D:\tmp\dataflow_report\index.html
```

## Query Def-Clear Paths

The `paths` subcommand reads `analysis-cache.json` and writes a
function-scoped `path-query.json`.

Example:

```powershell
target\release\data-flow-analyzer.exe paths --input D:\tmp\dataflow_report\data\analysis-cache.json --function app.utils.text_utils::handle_str --max-loop-unroll 2
```

Notes:

- `--function` accepts either a function id or a qualified function name
- `--max-loop-unroll` defaults to `2`

Output:

```text
D:\tmp\dataflow_report\data\path-query.json
```

## Optional Config File

The `analyze` subcommand also accepts `--config <path>` with TOML content.

Example:

```toml
lang = "python"
input = "D:/repos/asr_platform/app"
out = "D:/tmp/dataflow_report"
top_n = 100
max_loop_unroll = 2
max_paths = 1000
max_path_len = 500
```

Example invocation:

```powershell
target\release\data-flow-analyzer.exe analyze --config .\analyzer.toml
```

## Output Layout

Top-level report files:

- `index.html`: static report entrypoint
- `assets/report.css`: report stylesheet

Graph outputs:

- `graphs/def_use_hotspots.dot`: DOT for def-use hotspot graph
- `graphs/def_use_hotspots.svg`: SVG render when Graphviz `dot` is available
- `graphs/variable_dependencies.dot`: DOT for variable-dependency graph
- `graphs/variable_dependencies.svg`: SVG render when Graphviz `dot` is available
- `graphs/variable_dependencies.graph.json`: machine-readable variable-dependency graph

Data exports:

- `data/analysis-cache.json`: canonical machine-readable cache for post-processing and `paths`
- `data/definitions.csv`
- `data/uses.csv`
- `data/def_use_edges.csv`
- `data/var_dependencies.csv`
- `data/function_summaries.csv`
- `data/parse_diagnostics.csv`
- `data/path-query.json`: emitted only after running `paths`

Notes:

- only `def_use_hotspots` and `variable_dependencies` are emitted as report graphs
- stale `module_dependencies` and `function_dependencies` artifacts are removed during report generation
- SVG output is optional; DOT is always emitted

## Example Workflow

```powershell
cargo build --release
target\release\data-flow-analyzer.exe analyze --lang python --input D:\repos\asr_platform\app --out D:\tmp\dataflow_report
target\release\data-flow-analyzer.exe paths --input D:\tmp\dataflow_report\data\analysis-cache.json --function app.services.result_service::ResultService.get_task_summary
```

Then inspect:

- `D:\tmp\dataflow_report\index.html`
- `D:\tmp\dataflow_report\graphs\variable_dependencies.graph.json`
- `D:\tmp\dataflow_report\data\path-query.json`

## Limitations

The analyzer is static and conservative. Dynamic Python features such as:

- `eval`
- reflection
- monkey patching
- metaclasses
- descriptors
- dependency injection
- runtime-generated imports

are reported as uncertain or external effects rather than fully resolved flows.
