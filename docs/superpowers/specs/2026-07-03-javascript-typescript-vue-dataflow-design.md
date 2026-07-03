# JavaScript TypeScript Vue Dataflow Support Design

## Background

The analyzer currently supports Python and C through language frontends that lower source code into a shared IR. The shared pipeline then computes CFG records, reaching definitions, def-use edges, variable-dependency edges, cross-function summaries, DOT/JSON/CSV exports, optional SVG output, HTML reports, and function-level `paths` queries.

The next feature is to add JavaScript and TypeScript support, including Vue single-file components, while keeping the user-visible output aligned with the existing C and Python outputs. The acceptance project is:

```text
D:\repos\temp\airi
```

The AIRI repository is a pnpm monorepo with many JavaScript, TypeScript, JSX, TSX, and Vue SFC files. A practical acceptance run needs one command that scans all supported JS-family files in that tree.

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

If dependency resolution is unavailable in the local environment, the implementation may first add the frontend behind tests and then request approval to fetch crates.

### Source Discovery

Add JS-family source discovery without changing Python discovery behavior.

Default JS-family excludes should include:

- `.git`
- `node_modules`
- `.pnpm-store`
- `.cache`
- `.turbo`
- `dist`
- `build`
- `coverage`
- generated package manager caches

This keeps AIRI scanning focused on authored source files and avoids analyzing vendored packages or generated assets.

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

Line numbers should map back to the original `.vue` file by preserving a line offset. Existing `SourceSpan` can continue to store a single file and line; the frontend should report spans against the `.vue` path with original line numbers whenever practical.

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

### CFG

The JS/TS frontend should emit baseline CFG records compatible with existing path queries:

- one entry block and one exit block per function
- sequence edges for ordinary statements
- branch edges for `if`, conditional expressions, and `switch`
- loop-enter and loop-back edges for loops
- exception edges for `try` / `catch` when feasible

This does not need to be a perfect JavaScript CFG in the first version. It needs to provide stable function-level structure for the existing `paths` command and report views.

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

### Performance

The AIRI tree is large enough that scanning must avoid obvious performance traps:

- skip vendor/cache/generated directories by default
- process independent files in parallel, following the Python frontend pattern
- keep source-unit preparation deterministic by sorting virtual paths
- avoid loading `node_modules`

## Testing Strategy

Use TDD for implementation.

### Unit and Frontend Tests

Add tests for:

- JS/TS source discovery and default excludes
- Vue SFC script extraction with normal and setup scripts
- JS function declarations, parameters, declarations, assignments, and returns
- TypeScript syntax such as type annotations and interfaces without false dataflow definitions
- JSX/TSX parsing without treating JSX tags as normal variable reads unless explicit expressions require it
- member/subscript normalization
- destructuring definitions
- call records and return-target wiring
- baseline CFG for branches and loops
- parse diagnostics for malformed JS/TS/Vue input

### Integration Tests

Add CLI integration tests that write a temporary JS/TS/Vue fixture and assert:

- `analyze --lang js-ts` succeeds
- shared report artifacts exist
- `data/analysis-cache.json` contains JS/TS functions
- `paths` can query a function from that cache

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

## Acceptance Criteria

The feature is complete when:

- existing Python tests still pass
- existing C tests still pass
- `--lang js-ts` is accepted by the CLI
- JS/TS/JSX/TSX/Vue source discovery works with sensible default excludes
- Vue `<script>` and `<script setup>` blocks are analyzed
- the frontend emits shared IR records for files, modules, functions, scopes, definitions, uses, calls, and CFGs
- shared def-use and variable-dependency analysis runs on JS/TS output
- reports for JS/TS contain the same canonical artifacts as Python and C
- `paths` works against JS/TS-generated `analysis-cache.json`
- the AIRI acceptance command completes on `D:\repos\temp\airi`

## Design Rationale

This approach keeps the analyzer architecture stable. JavaScript, TypeScript, and Vue-specific complexity remains in the language boundary, while reports, data exports, path queries, and downstream consumers continue to rely on the existing shared IR schema. It also avoids requiring users to configure TypeScript projects, install dependencies, or run build tools before analysis.

