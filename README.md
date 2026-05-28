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
cargo run -- paths --input D:\tmp\dataflow_report\data\analysis-cache.json --function main --max-loop-unroll 2
```

The command writes:

```text
D:\tmp\dataflow_report\data\path-query.json
```

## Outputs

- `index.html`: static report entrypoint
- `graphs/*.dot`: Graphviz sources
- `graphs/*.svg`: rendered graphs when Graphviz `dot` is available
- `data/analysis-cache.json`: versioned cache for path queries and post-processing
- `data/*.csv`: tabular exports for definitions, uses, edges, summaries, and diagnostics

## Current Capabilities

- Parse Python modules into a language-neutral IR
- Track definitions, uses, imports, captures, and normalized attribute/subscript places
- Build baseline CFG records per function
- Compute reaching definitions and def-use edges
- Export DOT/SVG graphs and an HTML report
- Record parse diagnostics while keeping partially parsed files in the output

## Limitations

The analyzer is static and conservative. Dynamic Python features such as `eval`, reflection, monkey patching, metaclasses, descriptors, dependency injection, and runtime-generated imports are reported as uncertain or external effects rather than fully resolved flows.
