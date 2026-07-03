# Data Flow Analyzer

Rust static data-flow analyzer. It targets Python and C via tree-sitter and produces:

- def-use analysis
- variable-dependency analysis
- DOT graph sources
- optional SVG renders
- a static HTML report
- machine-readable JSON/CSV exports

## Current Scope

The current release supports:

- Python parsing and lowering into a language-neutral IR
- C parsing of CMake-based projects with auto-generated compile databases
- definitions, uses, imports, captures, and normalized attribute/subscript places
- baseline CFG construction per function
- reaching-definitions analysis
- def-use edges
- variable-dependency edges
- cross-function summary propagation
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

## Analyze A C Codebase

The analyzer can also process C projects. It scans the input tree for
`CMakeLists.txt` files, runs CMake configure with
`CMAKE_EXPORT_COMPILE_COMMANDS=ON` for each project, merges the resulting
`compile_commands.json` files, preprocesses every translation unit with its
real compile context, and only then lowers the preprocessed C into the shared
IR. There is no manual "build the compile database" step.

Example:

```bash
cargo run -- analyze --lang c \
    --input /mnt/d/repos/arcs_mini/modules/lschat/tests \
    --out /tmp/lschat-report \
    --build-root /tmp/lschat-build
```

C-specific flags:

- `--build-root <path>`: directory used to host the per-project CMake build
  trees and the merged compile database. Defaults to
  `<out>/cmake-build` when omitted.
- `--cmake-arg <arg>`: repeatable. Forwarded verbatim to each `cmake`
  configure invocation. Use this to pass project-specific variables such as
  `-DLISA_BASE=/opt/lisa` or `-DDEFCONF_FILE=prj-linux.conf`.
- `--keep-preprocessed`: keep the per-unit `.i` files under
  `<out>/data/c-preprocessed/` after analysis. Useful when debugging
  preprocessor expansion.

C reports use the same layout as Python and additionally write
`data/compile_commands.merged.json` for downstream consumers.

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
