# Rust Data Flow Analyzer C Support Design

## Background

The current analyzer supports Python only. Its downstream pipeline is already
language-neutral enough to reuse for another frontend: the CLI drives source
discovery, the frontend lowers code into the shared IR, the analyzer computes
CFG and data-flow facts, and the report layer emits HTML, JSON/CSV, DOT, and
optional SVG outputs.

The next step is to add C language support with the same user-visible outputs
and query flow as Python. The first real acceptance target is
`/mnt/d/repos/arcs_mini/modules/lschat/tests`, which contains multiple CMake
driven C test applications and relies on real compiler options, local headers,
system headers, macros, and conditional compilation.

## Confirmed Requirements

- Add `lang = "c"` support alongside existing Python support.
- Keep the report and export layout aligned with Python:
  - static HTML report
  - `analysis-cache.json`
  - CSV exports
  - DOT graphs
  - optional SVG renders when Graphviz `dot` is available
- Support the same core analysis stages for C:
  - reaching definitions
  - def-use edges
  - variable-dependency edges
  - baseline CFG per function
  - simple interprocedural summary propagation
- Keep the `paths` flow unchanged: read `analysis-cache.json` and emit
  `path-query.json`.
- Support analysis scope configuration.
- For C, use real compile context rather than handwritten include/define lists.
- The analyzer must generate `compile_commands.json` internally from the CMake
  projects under the input tree, not require the user to supply it manually.
- The implementation should be validated against
  `/mnt/d/repos/arcs_mini/modules/lschat/tests`.

## Non-Goals

- Do not implement compiler-grade semantic analysis for all C constructs.
- Do not implement full points-to or alias analysis.
- Do not fully model inline assembly, linker-time symbol rewriting, `volatile`
  effects, or preprocessor metaprogramming semantics.
- Do not require the user to learn a separate "build database generation"
  command; build database generation is part of `analyze --lang c`.
- Do not change the Python output schema or break Python behavior while adding
  C support.

## User Experience

### Analyze

The main C workflow stays within the existing `analyze` subcommand:

```text
data-flow-analyzer analyze --lang c --input /mnt/d/repos/arcs_mini/modules/lschat/tests --out /tmp/lschat-report
```

The analyzer will:

1. discover CMake-backed test applications under the input directory
2. configure each discovered project with `CMAKE_EXPORT_COMPILE_COMMANDS=ON`
3. collect and merge the generated compile commands
4. preprocess translation units using the real compile context
5. lower the resulting C code into the shared IR
6. run the existing analysis and report pipeline

### Paths

The `paths` subcommand remains unchanged:

```text
data-flow-analyzer paths --input /tmp/lschat-report/data/analysis-cache.json --function lsc_test::reset_host_transport_mocks
```

It will emit `data/path-query.json` next to the cache file exactly as it does
for Python.

## Proposed CLI and Config Changes

### CLI

Keep the existing `analyze` and `paths` commands and add C-only options to
`analyze`:

- `--build-root <path>`
  - directory used for generated CMake build trees and compile databases
- `--cmake-arg <arg>`
  - repeatable passthrough option for extra CMake configure arguments
- `--keep-preprocessed`
  - keep preprocessed `.i` files and argument snapshots for debugging

Python runs ignore these options.

### Config

Extend `AnalyzeConfig` with optional C-specific fields:

- `build_root: Option<PathBuf>`
- `cmake_args: Vec<String>`
- `keep_preprocessed: bool`
- `c_project_globs: Vec<String>`

Behavior:

- `lang = "python"` ignores the new fields.
- `lang = "c"` uses them, while still supporting the shared fields such as
  `input`, `out`, `exclude`, `top_n`, `max_loop_unroll`, `max_paths`, and
  `max_path_len`.
- `c_project_globs` defaults to a recursive search for directories containing
  `CMakeLists.txt` below the input root.

## Architecture

### High-Level Pipeline

```text
CLI/config
  -> source/build discovery
      -> python: discover .py files
      -> c: discover CMake projects
  -> compile context stage
      -> python: none
      -> c: configure CMake projects and merge compile_commands.json
  -> frontend input preparation
      -> python: raw source files
      -> c: preprocessed translation units + source map
  -> language frontend
      -> lang::python or lang::c
  -> unified IR
  -> CFG
  -> reaching-definitions / def-use / var-deps
  -> summary propagation
  -> report/export writers
  -> path queries
```

### New C-Specific Modules

- `src/lang/c.rs`
  - tree-sitter C parsing and lowering to shared IR
- `src/cbuild.rs`
  - CMake project discovery, configure orchestration, compile database merge
- `src/ccompile.rs`
  - compile command parsing, preprocessing orchestration, source-map metadata

These modules isolate the C build context concerns from the shared analysis
logic.

## Compile Database Generation

### Discovery

When `lang = "c"`, the analyzer scans the input tree for candidate projects:

- directories containing `CMakeLists.txt`
- optionally filtered by `c_project_globs`
- ignored when they are nested helper directories with no compilable C target
  entries after configure

For the LSChat acceptance tree, this will cover directories such as:

- `lsc_tests`
- `lsc_conn_tests`
- `session_request_tests`
- `session_text_tests`
- `session_voice_tests`
- `multi_session_tests`
- `session_voice_encode_tests`
- `stream_text/sync`

### Configure Phase

For each discovered project, run CMake configure with:

- `-DCMAKE_EXPORT_COMPILE_COMMANDS=ON`
- a deterministic build directory under `build_root` or a default directory
  beneath the report output
- any user-supplied `--cmake-arg` values

The implementation should preserve the host-test conventions already used by
the LSChat test tree. That means the analyzer must allow CMake to consume the
same variables that the host coverage runner uses, including a custom
`LISA_BASE` or other test-specific options when passed through `--cmake-arg`.

### Merge Phase

Each generated `compile_commands.json` is collected and normalized into a single
merged file stored at:

- `out/data/compile_commands.merged.json`

Normalization rules:

- canonicalize `directory` paths
- preserve either `command` or `arguments`
- deduplicate repeated entries by `(file, directory, normalized arguments)`
- sort deterministically by file path plus argument digest

The merged file becomes the canonical C analysis input manifest.

## Preprocessing Strategy

The analyzer should not parse raw `.c` files directly. Instead, each compile
command is converted into a preprocessing command that expands headers and
macros under the real compile context.

### Preprocessing Flow

For every compile command:

1. parse the original command or argument vector
2. extract the source file, include paths, defines, standard flags, forced
   includes, and working directory
3. invoke the compiler in preprocessing mode
4. capture the preprocessed output and a compact source mapping record
5. cache both artifacts for reuse

### Required Outputs

For each translation unit, store:

- preprocessed source text
- normalized argument snapshot
- source file identity and hash
- lightweight mapping metadata from preprocessed lines back to original files
  using preprocessor line markers

Keep these artifacts under a dedicated cache area inside the report output, for
example:

- `out/data/c-preprocessed/...`

If `keep_preprocessed` is false, the analyzer may keep only the minimal cached
artifacts needed for deterministic reruns and debugging metadata.

### Mapping Policy

The primary IR spans should still point back to original project files whenever
line-marker information makes that possible. When a node originates from an
external header or macro-expanded synthetic region that cannot be mapped cleanly
to project code, the analyzer should:

- retain the best available file and line information
- mark the record as external or synthetic where appropriate
- avoid fabricating precise local spans

## C Frontend Lowering

### Parser

Use `tree-sitter-c` for syntax parsing of preprocessed C translation units.

### Supported Constructs in the First Version

The first C frontend version should lower these constructs into the shared IR:

- function definitions and declarations
- local variable definitions
- global variable definitions
- assignments and compound assignments
- returns
- expression statements
- calls, including direct calls and basic function-pointer calls
- `if` / `else`
- `switch`
- `for`, `while`, `do while`
- `break`, `continue`, `goto`
- unary and binary expressions
- array indexing
- struct and union field access with `.` and `->`
- pointer dereference and address-of expressions

### IR Mapping

Reuse the existing `Place` family with these C mappings:

- scalar local/global variable
  - `Local` or `Global`
- struct or union field
  - `Attribute`
- array or pointer indexing
  - `Subscript`
- closure-related places
  - not used for C
- unresolved external state, unknown alias targets, or unsafe fallback
  - `External` or wildcard-style place keys

Specific lowering rules:

- `x = y` creates a definition on `x` and a use on `y`
- `obj.field = value` creates a definition on `Attribute(obj, field)` and a use
  on `value`
- `ptr->field = value` uses a conservative normalized base for `ptr` and lowers
  to `Attribute(base, field)`
- `arr[i] = v` creates a definition on `Subscript(arr, i)` and uses on `arr`,
  `i`, and `v`
- `*ptr = v` lowers to a conservative dereference target rather than inventing
  an exact pointee

### Declarations and Cross-File Resolution

Create a project-local declaration/definition index so that:

- declarations in headers can be attached to definitions in C files
- direct calls can resolve to project-local functions across translation units
- unresolved or external calls still produce call records marked as external

The first version only needs one project-global symbol table for plain C names.
It does not need C++-style overload handling.

## CFG and Data-Flow Reuse

The existing shared CFG and analysis layers remain the default implementation.
The C frontend should emit IR in the shapes those layers already expect.

### CFG

Per function, produce:

- one entry block
- one exit block
- sequential edges
- branch edges for conditionals
- case edges for `switch`
- loop back edges
- control transfer edges for `break`, `continue`, and `goto`

### Reaching Definitions and Def-Use

The current reaching-definitions engine should operate over C-generated defs and
uses with no language-specific fork unless gaps are uncovered during testing.

### Variable Dependencies

The current variable-dependency pass should consume C defs and uses the same way
it consumes Python data once C places are normalized consistently.

### Summary Propagation

Retain the current context-insensitive summary model:

- function parameters become summary inputs
- returned places become summary returns
- locally observed writes become summary writes
- call sites propagate argument uses to return-target defs and writes

For unresolved or external C calls, mark external effects conservatively instead
of pretending the call is pure.

## Reporting and Exports

The C report output must match the Python output layout and schema style:

- `index.html`
- `assets/report.css`
- `data/analysis-cache.json`
- `data/definitions.csv`
- `data/uses.csv`
- `data/def_use_edges.csv`
- `data/var_dependencies.csv`
- `data/function_summaries.csv`
- `data/parse_diagnostics.csv`
- `graphs/def_use_hotspots.dot`
- `graphs/variable_dependencies.dot`
- optional SVG variants when Graphviz `dot` is available

Additional C-specific helper artifacts may be written under `data/`, but they
must not replace the canonical shared outputs.

## Error Handling

### Configure/Compile Database Stage

- If no CMake projects are discovered, fail with a clear message that the C
  input tree did not contain analyzable CMake projects.
- If CMake configure fails for a project, report which project failed and the
  configure command used.
- If at least one project configures successfully, keep going by default only if
  the failure can be recorded as a diagnostic without making the analysis
  meaningless. Otherwise fail the run.

### Preprocessing Stage

- If a translation unit cannot be preprocessed, emit a diagnostic record tied to
  the source file.
- Keep successful translation units in the analysis.
- Respect `fail_on_parse_error`-style behavior for hard-fail versus partial
  output policy once that policy is generalized beyond Python parsing.

### Frontend Stage

- Syntax or lowering failures should be recorded in diagnostics with original
  file paths when possible.
- Partial analysis is acceptable when enough IR can still be produced to keep
  outputs useful.

## Testing Strategy

### Repository Unit and Integration Tests

Add focused tests for:

- compile command discovery and merged database generation
- preprocessing command normalization
- source-map extraction from line markers
- C IR lowering for locals, globals, field access, subscripts, loops, calls,
  `goto`, and unresolved pointer cases
- CFG construction on representative C snippets
- def-use and variable-dependency generation on representative C snippets
- summary propagation across multiple C translation units
- report generation for C-produced caches
- `paths` queries for C-produced caches

Suggested new test files:

- `tests/c_compile_commands.rs`
- `tests/c_frontend.rs`
- `tests/c_integration_report.rs`

### External Acceptance Test

Add an ignored integration test that exercises the real LSChat tree at:

- `/mnt/d/repos/arcs_mini/modules/lschat/tests`

This test should:

1. run `analyze --lang c` against that input
2. assert that the merged compile database is written
3. assert that the shared report outputs exist
4. run `paths` against the produced `analysis-cache.json`
5. assert that `path-query.json` exists

Mark it ignored because it depends on an external repository and host build
environment.

## Acceptance Criteria

The design is complete when all of the following are true:

- Python analysis still passes its current test suite.
- The analyzer accepts `--lang c`.
- `analyze --lang c` can discover and configure the LSChat test projects under
  `/mnt/d/repos/arcs_mini/modules/lschat/tests`.
- The analyzer emits `out/data/compile_commands.merged.json`.
- The analyzer produces C-backed:
  - `analysis-cache.json`
  - CSV exports
  - DOT graphs
  - HTML report
  - optional SVGs
- The analyzer computes at least baseline CFG, reaching definitions, def-use,
  variable dependencies, and interprocedural summary propagation for C code.
- The `paths` subcommand works on C-generated caches and writes
  `path-query.json`.
- Output structure and schema remain aligned with Python outputs so downstream
  consumers do not need a separate C-only report reader.

## Design Rationale

This design keeps the existing analyzer architecture intact:

- language-neutral IR, CFG, data-flow, report, and path query layers remain the
  shared center
- C-specific complexity lives at the boundary where it belongs: build context,
  preprocessing, and AST lowering
- users get one simple command for C analysis instead of a multi-step manual
  workflow

That tradeoff gives the project a practical C first version that is strong
enough for real test applications without prematurely rebuilding the analyzer
around a compiler frontend.
