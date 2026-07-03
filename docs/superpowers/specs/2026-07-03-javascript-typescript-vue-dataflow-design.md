# JavaScript TypeScript Vue Dataflow Support Design

## Background

The analyzer currently supports Python and C through language frontends that lower source code into a shared IR. The shared pipeline then computes CFG records, reaching definitions, def-use edges, variable-dependency edges, cross-function summaries, DOT/JSON/CSV exports, optional SVG output, HTML reports, and function-level `paths` queries.

The next feature is to add JavaScript and TypeScript support, including Vue single-file components, while keeping the user-visible output aligned with the existing C and Python outputs. The acceptance project is:

```text
D:\repos\temp\airi
```

The AIRI repository is a pnpm monorepo whose authored JS-family source is predominantly TypeScript and Vue SFC, with smaller amounts of JavaScript and MJS. Local scans with vendor/cache excludes do not show authored JSX, TSX, or CJS files, so AIRI acceptance will not exercise those parser paths. JSX, TSX, and CJS support remain in scope for grammar coverage and forward compatibility, and must be verified through synthetic fixtures.

## Confirmed Requirements

- Add one-shot JS-family analysis through:

```text
data-flow-analyzer analyze --lang js-ts --input D:\repos\temp\airi --out D:\tmp\airi-js-ts-dataflow
```

- Support these source extensions:
  - `.js`
  - `.jsx`
  - `.mjs`
  - `.cjs`
  - `.ts`
  - `.tsx`
  - `.vue`
- AIRI acceptance verifies `.ts`, `.vue`, `.js`, and `.mjs` in practice. `.jsx`, `.tsx`, and `.cjs` are covered by tests because the current AIRI checkout does not contain authored files with those extensions under the default JS-family excludes.
- Vue SFC support must include `<script>` and `<script setup>` blocks.
- Vue template analysis is not part of this first version.
- Keep report artifacts aligned with Python and C:
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
  - `graphs/variable_dependencies.graph.json`
  - optional SVG files when Graphviz `dot` is available
- Keep the existing `paths` command working against JS/TS-generated `analysis-cache.json`.
- Do not require the user to run a separate build, transpile, or TypeScript compile step.

## Non-Goals

- Full TypeScript type checking.
- Full ECMAScript runtime semantics.
- Full Vue template dataflow analysis.
- Complete inter-module bundler resolution for every alias convention in AIRI.
- Complete points-to, alias, prototype-chain, or decorator semantics.
- Executing project code or installing dependencies as part of analysis.

## User Workflow

The primary acceptance workflow is:

```powershell
cargo run -- analyze --lang js-ts --input D:\repos\temp\airi --out D:\tmp\airi-js-ts-dataflow
```

The released binary should support the same workflow:

```powershell
target\release\data-flow-analyzer.exe analyze --lang js-ts --input D:\repos\temp\airi --out D:\tmp\airi-js-ts-dataflow
```

Function path queries remain unchanged:

```powershell
target\release\data-flow-analyzer.exe paths --input D:\tmp\airi-js-ts-dataflow\data\analysis-cache.json --function <qualified-function-name>
```

## Architecture

Use the same boundary pattern as Python and C:

```text
CLI/config
  -> source discovery
  -> JS-family source unit preparation
      -> raw JS/TS/JSX/TSX files
      -> extracted Vue script blocks
  -> JavaScript/TypeScript frontend
  -> shared IR
  -> CFG
  -> reaching definitions / def-use / var-deps
  -> summary propagation
  -> report/export writers
  -> paths query
```

### New Language Frontend

Add `src/lang/javascript.rs`.

The frontend owns:

- parser selection for JavaScript, JSX, TypeScript, and TSX
- Vue SFC script-block extraction handoff
- lowering JS-family AST nodes to shared IR
- parse diagnostics for partial or failed files
- stable qualified names for functions, classes, methods, and Vue script blocks

The preferred parser stack is:

- `tree-sitter-javascript` for `.js`, `.jsx`, `.mjs`, `.cjs`, and non-TS Vue scripts
- `tree-sitter-typescript` for `.ts`, `.tsx`, and `lang="ts"` Vue scripts

Add these dependencies to `Cargo.toml`:

```toml
tree-sitter-javascript = "0.25.0"
tree-sitter-typescript = "0.23.2"
```

Parser entry points:

- use `tree_sitter_javascript::LANGUAGE` for `.js`, `.jsx`, `.mjs`, `.cjs`, and non-TS Vue script blocks
- use `tree_sitter_typescript::LANGUAGE_TYPESCRIPT` for `.ts` and Vue `lang="ts"` script blocks
- use `tree_sitter_typescript::LANGUAGE_TSX` for `.tsx`; TSX must not be parsed with `LANGUAGE_TYPESCRIPT`

If these crates are not already available locally, implementation should update `Cargo.toml` first, run the dependency resolution command, and request network approval if Cargo cannot fetch them inside the sandbox.

### Source Discovery

Add JS-family source discovery without changing Python discovery behavior. The JS/TS CLI branch should call a dedicated helper:

```rust
discover_js_sources(config: &AnalyzeConfig) -> Result<Vec<SourceUnit>>
```

This follows the C-style frontend path: the CLI prepares `Vec<SourceUnit>` and then calls `JavaScriptFrontend::parse_units(&units)`. Do not add a Python-style `parse_files(&[SourceFile])` wrapper for JS/TS because Vue extraction already produces virtual source units.

`AnalyzeConfig::default()` currently has Python-oriented excludes and does not switch defaults by language. To keep existing Python behavior stable, the `analyze_js_ts` branch must inject JS-family excludes by taking `config.exclude` and adding these defaults for JS-family discovery:

- `**/.git/**`
- `**/node_modules/**`
- `**/.pnpm-store/**`
- `**/.cache/**`
- `**/.turbo/**`
- `**/dist/**`
- `**/build/**`
- `**/coverage/**`
- generated package manager caches

`**/node_modules/**` covers pnpm's nested `node_modules/.pnpm` store. This keeps AIRI scanning focused on authored source files and avoids analyzing vendored packages or generated assets.

The helper should read raw JS/TS/JSX/TSX files directly into `SourceUnit`s and should expand Vue SFC files into one `SourceUnit` per supported script block.

### Vue SFC Handling

Add a small SFC extractor, either in `src/source.rs` or a focused helper module used by `src/lang/javascript.rs`.

For every `.vue` file:

- extract each `<script ...>` block
- classify `setup` scripts by presence of the `setup` attribute
- classify TypeScript scripts by `lang="ts"` or `lang='ts'`
- produce a `SourceUnit` for each script block
- keep the original `.vue` path in `original_path`
- assign a virtual `relative_path`, for example:

```text
components/Foo.vue?script=normal&lang=ts
components/Foo.vue?script=setup&lang=ts
```

The virtual path string is a stable ID contract because the analyzer's stable IDs include `relative_path`. Do not change the `?script=<normal|setup>&lang=<js|ts>` shape without a schema/version migration.

First-version Vue cross-block behavior is intentionally conservative. Normal `<script>` and `<script setup>` blocks from the same `.vue` file are separate `SourceUnit`s and separate modules. References across those blocks may appear unresolved or external in v1; the report should document this as a conservative limitation rather than merging the blocks incorrectly.

### Span And Path Mapping

For raw JS/TS files, `SourceSpan.file` should be the repository-relative path.

For Vue script blocks:

- `SourceUnit.relative_path` should be the stable virtual path, such as `components/Foo.vue?script=setup&lang=ts`
- `SourceUnit.original_path` should point to the original `.vue` file
- `SourceUnit.line_markers` should preserve the script-block start line in the `.vue` file
- `SourceSpan.file` should use the virtual `relative_path`
- `SourceSpan.line` and `end_line` should use original `.vue` line numbers when the block line offset is known

This gives stable IDs through the virtual path while keeping report line numbers useful for opening the original Vue file.

### Qualified Names

`paths` accepts a function ID or `FunctionRecord.qualified_name`, so JS/TS qualified names must be deterministic.

Use these formats:

- top-level named function: `<virtual-relative-path>::<name>`
- exported named function: `<virtual-relative-path>::<name>`
- default export without a name: `<virtual-relative-path>::default`
- class method: `<virtual-relative-path>::<ClassName>.<methodName>`
- object-literal method assigned to a binding: `<virtual-relative-path>::<binding>.<methodName>`
- arrow/function expression assigned to a local: `<virtual-relative-path>::<binding>`
- anonymous function without an obvious binding: `<virtual-relative-path>::<anonymous@line:col>`

Line and column fallback names must use the mapped `SourceSpan` location so anonymous IDs remain stable as long as source locations are stable.

### IR Lowering Scope

The first version should lower enough JavaScript-family constructs to be useful on AIRI and to match C/Python output expectations:

- modules and source files
- imports and exports
- function declarations
- function expressions
- arrow functions
- class declarations
- methods
- parameters
- variable declarations with `var`, `let`, and `const`
- assignments and compound assignments
- destructuring patterns for common object and array binding cases
- return statements
- call expressions
- member expressions as `Place::Attribute`
- subscript/computed member expressions as `Place::Subscript`
- `if` / `else`
- `switch`
- `for`, `for...of`, `for...in`, `while`, and `do...while`
- `try` / `catch` / `finally`
- `await` and `yield` as expression wrappers that still expose contained uses

Unsupported or ambiguous constructs should produce conservative diagnostics or `Place::Unknown` rather than failing the whole analysis.

### Import Records

The existing `resolve_imports` pass is Python-specific and depends on `__init__.py`, `__all__`, dotted Python modules, and Python relative import levels. The JS/TS CLI branch must not call `resolve_imports`, and v1 should not add a `resolve_js_imports` pass.

The JS/TS frontend should still emit `ImportRecord`s for report visibility:

- `module`: raw module specifier string, for example `"vue"`, `"./foo"`, or `"@/stores/app"`
- `name`: imported export name
  - default import: `Some("default")`
  - named import: `Some("<imported-name>")`
  - namespace import: `Some("*")`
  - side-effect import: `None`
- `alias`: local binding name when one exists
  - `import value from "./x"` -> `name = Some("default")`, `alias = Some("value")`
  - `import { source as local } from "./x"` -> `name = Some("source")`, `alias = Some("local")`
  - `import * as ns from "./x"` -> `name = Some("*")`, `alias = Some("ns")`
  - `import "./setup"` -> `name = None`, `alias = None`
- `level`: always `0` for JS/TS because the Python relative-import level model does not apply
- `resolution`: `"external"` for all JS/TS imports in v1

Any local binding created by an import should also produce a `Definition` with `def_kind = "import"` and conservative `Place::External` dependencies.

### Vue Script Setup Macros

Vue compiler macros are common in AIRI and should not be treated as ordinary unresolved runtime calls.

Handle these forms in v1:

- `const props = defineProps(...)` and `const props = withDefaults(defineProps(...), ...)` define local `props`
- `const { foo } = defineProps(...)` and `const { foo } = withDefaults(defineProps(...), ...)` define destructured local bindings such as `foo`
- `const emit = defineEmits(...)` defines local `emit`
- `const model = defineModel(...)` defines local `model`
- `defineExpose({...})` should collect ordinary uses from the object expression but should not create a runtime unresolved-call diagnostic for `defineExpose`

Call records for these macros may use `resolution = "compiler-macro"` or may be omitted if no downstream summary value is useful. Unsupported macro forms should produce conservative diagnostics rather than false definitions.

### CFG

The JS/TS frontend should initially match the Python baseline CFG style rather than trying to model a complete JavaScript CFG. For every `FunctionRecord`, including arrow functions, methods, and Vue script-related functions, emit:

- one entry block and one exit block per function
- one body/basic block when the function has a body
- an entry-to-body sequence edge
- a body-to-exit edge

Branches, loops, switches, and try/catch statements should still be walked for definitions and uses, but rich branch/loop/exception CFG edges are a later enhancement. This keeps `paths` working for all functions without over-promising control-flow precision in v1.

### Definitions and Uses

Definitions:

- imports define their local binding names
- parameters define local places
- `let` / `const` / `var` declarations define local places
- assignment left-hand sides define places
- destructuring pattern leaves define local or attribute/subscript places when recognizable
- class declarations define class-name locals
- function declarations define function-name locals

Uses:

- identifier reads
- member-expression bases
- computed member-expression bases and indices
- call arguments
- return values
- conditional and loop test expressions
- assignment right-hand sides

Call records:

- direct identifier calls keep `callee_expr` as the identifier
- member calls keep `callee_expr` as a normalized member path when possible
- unresolved dynamic calls remain unresolved or external

### Cross-Function Summaries

Reuse the existing summary pipeline. The JS/TS frontend only needs to create enough call records, argument use IDs, and return target definitions for current summary propagation to work.

First-version local resolution can be conservative:

- resolve calls to functions declared in the same module by simple name
- resolve class methods only when the receiver is syntactically obvious
- mark imports, package calls, and dynamic calls as external or unresolved

### Error Handling

Parsing and lowering should be best effort:

- one malformed file should not fail the entire AIRI scan
- parser errors should be recorded in `data/parse_diagnostics.csv`
- files with partial parse trees should still contribute any recoverable IR
- Vue blocks with malformed tags should produce diagnostics and continue scanning other files
- when `fail_on_parse_error = true`, JS/TS analysis should stop on the first parse error after recording enough context in the error message

### Performance

The AIRI tree is large enough that scanning must avoid obvious performance traps:

- skip vendor/cache/generated directories by default
- process independent files in parallel, following the Python frontend pattern
- keep source-unit preparation deterministic by sorting virtual paths
- avoid loading `node_modules`

## Testing Strategy

Use TDD for implementation.

### Unit and Frontend Tests

Add `tests/js_frontend.rs` for:

- JS/TS source discovery and default excludes
- Vue SFC script extraction with normal and setup scripts
- JS function declarations, parameters, declarations, assignments, and returns
- TypeScript syntax such as type annotations and interfaces without false dataflow definitions
- JSX/TSX parsing without treating JSX tags as normal variable reads unless explicit expressions require it
- member/subscript normalization
- destructuring definitions
- call records and return-target wiring
- baseline CFG for function-like constructs
- parse diagnostics for malformed JS/TS/Vue input

### Integration Tests

Add `tests/js_integration_report.rs` for CLI integration tests that write a temporary JS/TS/Vue fixture and assert:

- `analyze --lang js-ts` succeeds
- shared report artifacts exist
- `data/analysis-cache.json` contains JS/TS functions
- `paths` can query a function from that cache

Add `tests/js_cli.rs` for CLI help and language alias coverage:

- `--lang js-ts`
- `--lang javascript`
- `--lang js`
- `--lang typescript`
- `--lang ts`

### AIRI Acceptance

Add an ignored acceptance test for the local AIRI checkout:

```text
D:\repos\temp\airi
```

The test should:

1. skip when the path does not exist
2. run `analyze --lang js-ts`
3. assert shared report artifacts exist
4. assert the cache contains at least one file, function, definition, use, and graph output
5. run `paths` against a discovered function where feasible

This acceptance test must not claim to validate JSX, TSX, or CJS behavior because the current AIRI checkout does not contain authored files with those extensions under the JS-family excludes.

### Documentation

Update `README.md`:

- add JavaScript/TypeScript/Vue to Current Scope
- add an "Analyze A JS/TS Codebase" section with the AIRI-style `--lang js-ts` command
- document that Vue `<script>` and `<script setup>` blocks are analyzed while templates are not analyzed in v1

## Acceptance Criteria

The feature is complete when:

- existing Python tests still pass
- existing C tests still pass
- `--lang js-ts` is accepted by the CLI
- JS/TS/JSX/TSX/Vue source discovery works with sensible default excludes
- Vue `<script>` and `<script setup>` blocks are analyzed
- the frontend emits shared IR records for files, modules, functions, scopes, definitions, uses, calls, and CFGs
- every JS/TS function-like record has a baseline CFG compatible with `paths`
- shared def-use and variable-dependency analysis runs on JS/TS output
- reports for JS/TS contain the same canonical artifacts as Python and C
- `paths` works against JS/TS-generated `analysis-cache.json`
- the AIRI acceptance command completes on `D:\repos\temp\airi`
- JSX, TSX, and CJS parser paths are covered by synthetic tests rather than AIRI acceptance

## Design Rationale

This approach keeps the analyzer architecture stable. JavaScript, TypeScript, and Vue-specific complexity remains in the language boundary, while reports, data exports, path queries, and downstream consumers continue to rely on the existing shared IR schema. It also avoids requiring users to configure TypeScript projects, install dependencies, or run build tools before analysis.
