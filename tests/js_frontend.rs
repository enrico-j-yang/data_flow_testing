use data_flow_analyzer::config::AnalyzeConfig;
use data_flow_analyzer::ir::Place;
use data_flow_analyzer::js_source::{
    discover_js_sources, extract_vue_script_units, syntax_for_unit, JavaScriptSyntax,
};
use data_flow_analyzer::lang::javascript::JavaScriptFrontend;
use data_flow_analyzer::lang::LanguageFrontend;
use data_flow_analyzer::source::{LineMarker, SourceUnit};
use std::fs;

fn parse_js_unit(path: &str, source: &str) -> data_flow_analyzer::ir::AnalysisCache {
    let unit = SourceUnit {
        absolute_path: path.into(),
        relative_path: path.to_string(),
        source_text: source.to_string(),
        original_path: None,
        line_markers: Vec::new(),
    };

    JavaScriptFrontend::new().parse_units(&[unit]).unwrap()
}

#[test]
fn javascript_frontend_records_files_and_modules() {
    let cache = parse_js_unit("src/main.ts", "export const value: number = 1;\n");

    assert_eq!(cache.files.len(), 1);
    assert_eq!(cache.files[0].path, "src/main.ts");
    assert_eq!(cache.files[0].parse_status, "ok");
    assert_eq!(cache.modules.len(), 1);
    assert_eq!(cache.modules[0].module_name, "src/main.ts");
}

#[test]
fn javascript_frontend_records_parse_diagnostics_for_broken_code() {
    let cache = parse_js_unit("src/broken.ts", "export const = ;\n");

    assert_eq!(cache.files[0].parse_status, "partial");
    assert!(cache
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.kind == "parse-error"));
}

#[test]
fn javascript_frontend_does_not_duplicate_diagnostics_inside_one_error_node() {
    let cache = parse_js_unit("src/broken.ts", "export const = ;\n");

    let parse_diagnostics = cache
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.kind == "parse-error")
        .collect::<Vec<_>>();

    assert_eq!(
        parse_diagnostics.len(),
        1,
        "expected one diagnostic for one malformed declaration, got {parse_diagnostics:?}"
    );
}

#[test]
fn javascript_frontend_maps_diagnostics_with_latest_line_marker() {
    let unit = SourceUnit {
        absolute_path: "src/Widget.vue?script=setup&lang=ts".into(),
        relative_path: "src/Widget.vue?script=setup&lang=ts".to_string(),
        source_text: "\n\nexport const = ;\n".to_string(),
        original_path: Some("src/Widget.vue".into()),
        line_markers: vec![
            LineMarker {
                generated_line: 1,
                original_file: "src/Widget.vue".to_string(),
                original_line: 100,
            },
            LineMarker {
                generated_line: 3,
                original_file: "src/Widget.vue".to_string(),
                original_line: 300,
            },
        ],
    };

    let cache = JavaScriptFrontend::new().parse_units(&[unit]).unwrap();
    let diagnostic = cache
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.kind == "parse-error")
        .expect("expected parse diagnostic");

    assert_eq!(diagnostic.span.line, 300);
    assert_eq!(diagnostic.span.end_line, 300);
}

#[test]
fn javascript_frontend_selects_jsx_and_tsx_parsers_from_parse_units() {
    let units = vec![
        SourceUnit {
            absolute_path: "src/view.tsx".into(),
            relative_path: "src/view.tsx".to_string(),
            source_text: "const view = <Widget />;\n".to_string(),
            original_path: None,
            line_markers: Vec::new(),
        },
        SourceUnit {
            absolute_path: "src/view.jsx".into(),
            relative_path: "src/view.jsx".to_string(),
            source_text: "const view = <Widget />;\n".to_string(),
            original_path: None,
            line_markers: Vec::new(),
        },
        SourceUnit {
            absolute_path: "src/value.ts".into(),
            relative_path: "src/value.ts".to_string(),
            source_text: "const value: number = 1;\n".to_string(),
            original_path: None,
            line_markers: Vec::new(),
        },
    ];

    let cache = JavaScriptFrontend::new().parse_units(&units).unwrap();
    let statuses = cache
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.parse_status.as_str()))
        .collect::<Vec<_>>();

    assert_eq!(
        statuses,
        vec![
            ("src/value.ts", "ok"),
            ("src/view.jsx", "ok"),
            ("src/view.tsx", "ok"),
        ]
    );
}

#[test]
fn javascript_frontend_merges_units_in_deterministic_path_order() {
    let units = vec![
        SourceUnit {
            absolute_path: "src/z.ts".into(),
            relative_path: "src/z.ts".to_string(),
            source_text: "export const z = 1;\n".to_string(),
            original_path: None,
            line_markers: Vec::new(),
        },
        SourceUnit {
            absolute_path: "src/a.ts".into(),
            relative_path: "src/a.ts".to_string(),
            source_text: "export const = ;\n".to_string(),
            original_path: None,
            line_markers: Vec::new(),
        },
        SourceUnit {
            absolute_path: "src/m.ts".into(),
            relative_path: "src/m.ts".to_string(),
            source_text: "export const m = 1;\n".to_string(),
            original_path: None,
            line_markers: Vec::new(),
        },
    ];

    let cache = JavaScriptFrontend::new().parse_units(&units).unwrap();
    let file_paths = cache
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();
    let module_paths = cache
        .modules
        .iter()
        .map(|module| module.module_name.as_str())
        .collect::<Vec<_>>();

    assert_eq!(file_paths, vec!["src/a.ts", "src/m.ts", "src/z.ts"]);
    assert_eq!(module_paths, vec!["src/a.ts", "src/m.ts", "src/z.ts"]);
    assert!(cache
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.file == "src/a.ts"));
}

#[test]
fn javascript_frontend_lowers_imports_functions_and_params() {
    let cache = parse_js_unit(
        "src/main.ts",
        "import value, { source as local } from './dep'\n\
         import type { Shape } from './types'\n\
         export function compute(input: number) { return local(input) }\n",
    );

    assert!(cache.modules[0].imports.iter().any(|import| {
        import.module == "./dep"
            && import.name.as_deref() == Some("default")
            && import.alias.as_deref() == Some("value")
            && import.resolution == "external"
    }));
    assert!(cache.modules[0].imports.iter().any(|import| {
        import.module == "./dep"
            && import.name.as_deref() == Some("source")
            && import.alias.as_deref() == Some("local")
            && import.level == 0
    }));
    assert!(!cache.modules[0]
        .imports
        .iter()
        .any(|import| import.module == "./types"));

    let compute = cache
        .functions
        .iter()
        .find(|function| function.qualified_name == "src/main.ts::compute")
        .expect("expected compute function");
    assert_eq!(compute.params, vec!["input"]);

    assert!(cache.definitions.iter().any(|definition| {
        definition.def_kind == "param"
            && definition.function_id.as_deref() == Some(compute.function_id.as_str())
            && matches!(
                &definition.place,
                Place::Local { name, .. } if name == "input"
            )
    }));
    assert!(cache.uses.iter().any(|usage| {
        usage.function_id.as_deref() == Some(compute.function_id.as_str())
            && usage.context == "return value"
    }));
}

#[test]
fn javascript_frontend_lowers_arrows_classes_methods_and_cfgs() {
    let cache = parse_js_unit(
        "src/widgets.ts",
        "const make = (value: number) => value + 1\n\
         class Box { read(input: number) { return make(input) } }\n",
    );

    let make = cache
        .functions
        .iter()
        .find(|function| function.qualified_name == "src/widgets.ts::make")
        .expect("expected make arrow function");
    assert_eq!(make.params, vec!["value"]);

    let method = cache
        .functions
        .iter()
        .find(|function| function.qualified_name == "src/widgets.ts::Box.read")
        .expect("expected Box.read method function");
    let class = cache
        .classes
        .iter()
        .find(|class| class.qualified_name == "src/widgets.ts::Box")
        .expect("expected Box class");
    assert!(class.methods.contains(&method.function_id));

    let cfg = cache
        .cfgs
        .iter()
        .find(|cfg| cfg.function_id == method.function_id)
        .expect("expected method cfg");
    assert!(cfg.blocks.iter().any(|block| block.block_kind == "Entry"));
    assert!(cfg.blocks.iter().any(|block| block.block_kind == "Exit"));
}

#[test]
fn js_source_discovery_skips_vendor_and_cache_dirs() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::create_dir_all(dir.path().join("src/generated/deep")).unwrap();
    fs::create_dir_all(dir.path().join("src/node_modules/nested")).unwrap();
    fs::create_dir_all(dir.path().join("node_modules/pkg")).unwrap();
    fs::create_dir_all(dir.path().join(".turbo/cache")).unwrap();
    fs::create_dir_all(dir.path().join("dist/assets")).unwrap();
    fs::write(dir.path().join("src/app.ts"), "export const value = 1;\n").unwrap();
    fs::write(
        dir.path().join("src/view.vue"),
        "<script setup lang=\"ts\">\nconst value = 1\n</script>\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("node_modules/pkg/index.ts"),
        "export const vendored = 1;\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("src/node_modules/nested/index.ts"),
        "export const nestedVendored = 1;\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("src/generated/deep/client.ts"),
        "export const generated = 1;\n",
    )
    .unwrap();
    fs::write(
        dir.path().join(".turbo/cache/generated.ts"),
        "export const cached = 1;\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("dist/assets/bundle.js"),
        "export const bundled = 1;\n",
    )
    .unwrap();

    let cfg = AnalyzeConfig {
        lang: "js-ts".to_string(),
        input: dir.path().to_path_buf(),
        out: dir.path().join("out"),
        exclude: vec!["src/generated/**".to_string()],
        ..AnalyzeConfig::default()
    };

    let units = discover_js_sources(&cfg).unwrap();
    let paths = units
        .iter()
        .map(|unit| unit.relative_path.clone())
        .collect::<Vec<_>>();

    assert_eq!(paths.len(), 2, "unexpected units: {paths:?}");
    assert!(paths.iter().any(|path| path == "src/app.ts"));
    assert!(paths
        .iter()
        .any(|path| path == "src/view.vue?script=setup&lang=ts"));
    assert!(!paths.iter().any(|path| path.contains("node_modules")));
    assert!(!paths.iter().any(|path| path.contains(".turbo")));
    assert!(!paths.iter().any(|path| path.contains("generated")));
    assert!(!paths.iter().any(|path| path.contains("dist")));
}

#[test]
fn vue_extractor_preserves_virtual_paths_and_line_markers() {
    let source = r#"
<template><div>{{ value }}</div></template>
<script>
export const normal = 1
</script>
<script setup lang="ts">
const value: number = normal
</script>
"#;

    let units = extract_vue_script_units(
        std::path::Path::new("src/Widget.vue"),
        "src/Widget.vue",
        source,
    )
    .unwrap();

    assert_eq!(units.len(), 2);
    assert_eq!(
        units[0].relative_path,
        "src/Widget.vue?script=normal&lang=js"
    );
    assert_eq!(
        units[1].relative_path,
        "src/Widget.vue?script=setup&lang=ts"
    );
    assert_eq!(
        units[1].original_path.as_deref(),
        Some(std::path::Path::new("src/Widget.vue"))
    );
    assert_eq!(units[1].line_markers[0].generated_line, 1);
    assert_eq!(units[1].line_markers[0].original_line, 7);
    assert!(units[1]
        .source_text
        .contains("const value: number = normal"));
}

#[test]
fn vue_extractor_starts_source_text_at_first_script_content_line() {
    let source = "<template />\n<script>\nexport const value = 1\n</script>\n";

    let units = extract_vue_script_units(
        std::path::Path::new("src/Widget.vue"),
        "src/Widget.vue",
        source,
    )
    .unwrap();

    assert_eq!(units.len(), 1);
    assert_eq!(units[0].line_markers[0].generated_line, 1);
    assert_eq!(units[0].line_markers[0].original_line, 3);
    assert_eq!(
        units[0].source_text.lines().next(),
        Some("export const value = 1")
    );
}

#[test]
fn vue_extractor_trims_crlf_after_opening_script_tag() {
    let source = "<template />\r\n<script>\r\nexport const value = 1\r\n</script>\r\n";

    let units = extract_vue_script_units(
        std::path::Path::new("src/Widget.vue"),
        "src/Widget.vue",
        source,
    )
    .unwrap();

    assert_eq!(units.len(), 1);
    assert_eq!(units[0].line_markers[0].generated_line, 1);
    assert_eq!(units[0].line_markers[0].original_line, 3);
    assert_eq!(
        units[0].source_text.lines().next(),
        Some("export const value = 1")
    );
}

#[test]
fn vue_extractor_counts_lone_cr_as_line_break_after_opening_script_tag() {
    let source = "<script>\rconst value = 1\r</script>";

    let units = extract_vue_script_units(
        std::path::Path::new("src/Widget.vue"),
        "src/Widget.vue",
        source,
    )
    .unwrap();

    assert_eq!(units.len(), 1);
    assert_eq!(units[0].line_markers[0].generated_line, 1);
    assert_eq!(units[0].line_markers[0].original_line, 2);
    assert_eq!(
        units[0].source_text.split_terminator('\r').next(),
        Some("const value = 1")
    );
}

#[test]
fn vue_extractor_tolerates_spaced_script_lang_attribute() {
    let source = r#"<script setup lang = "ts">
const value: number = 1
</script>
"#;

    let units = extract_vue_script_units(
        std::path::Path::new("src/Widget.vue"),
        "src/Widget.vue",
        source,
    )
    .unwrap();

    assert_eq!(units.len(), 1);
    assert_eq!(
        units[0].relative_path,
        "src/Widget.vue?script=setup&lang=ts"
    );
    assert_eq!(syntax_for_unit(&units[0]), JavaScriptSyntax::TypeScript);
}

#[test]
fn vue_extractor_ignores_gt_inside_quoted_script_attributes() {
    let source = r#"<script setup lang="ts" generic="T extends Foo<Bar>">
const real: T = value
</script>
"#;

    let units = extract_vue_script_units(
        std::path::Path::new("src/Widget.vue"),
        "src/Widget.vue",
        source,
    )
    .unwrap();

    assert_eq!(units.len(), 1);
    assert_eq!(
        units[0].relative_path,
        "src/Widget.vue?script=setup&lang=ts"
    );
    assert_eq!(units[0].line_markers[0].generated_line, 1);
    assert_eq!(units[0].line_markers[0].original_line, 2);
    assert_eq!(
        units[0].source_text.lines().next(),
        Some("const real: T = value")
    );
}

#[test]
fn vue_extractor_matches_script_tags_case_insensitively() {
    let source = r#"<SCRIPT lang="ts">
const value: number = 1
</SCRIPT>
"#;

    let units = extract_vue_script_units(
        std::path::Path::new("src/Widget.vue"),
        "src/Widget.vue",
        source,
    )
    .unwrap();

    assert_eq!(units.len(), 1);
    assert_eq!(
        units[0].relative_path,
        "src/Widget.vue?script=normal&lang=ts"
    );
    assert_eq!(syntax_for_unit(&units[0]), JavaScriptSyntax::TypeScript);
    assert_eq!(
        units[0].source_text.lines().next(),
        Some("const value: number = 1")
    );
}

#[test]
fn vue_extractor_preserves_tsx_and_jsx_lang_attributes() {
    let source = r#"<script setup lang = "TSX">
const view = <Widget />
</script>
<script lang='jsx'>
const view = <Widget />
</script>
"#;

    let units = extract_vue_script_units(
        std::path::Path::new("src/Widget.vue"),
        "src/Widget.vue",
        source,
    )
    .unwrap();

    assert_eq!(units.len(), 2);
    assert_eq!(
        units[0].relative_path,
        "src/Widget.vue?script=setup&lang=tsx"
    );
    assert_eq!(syntax_for_unit(&units[0]), JavaScriptSyntax::Tsx);
    assert_eq!(
        units[1].relative_path,
        "src/Widget.vue?script=normal&lang=jsx"
    );
    assert_eq!(syntax_for_unit(&units[1]), JavaScriptSyntax::Jsx);
}

#[test]
fn vue_extractor_ignores_scripts_inside_html_comments() {
    let source = r#"
<!--
<script>
const ignored = true
</script>
-->
<script>
const real = true
</script>
"#;

    let units = extract_vue_script_units(
        std::path::Path::new("src/Widget.vue"),
        "src/Widget.vue",
        source,
    )
    .unwrap();

    assert_eq!(units.len(), 1);
    assert_eq!(
        units[0].source_text.lines().next(),
        Some("const real = true")
    );
}

#[test]
fn vue_extractor_ignores_non_script_tag_prefixes() {
    let source = r#"
<scripture>not a script</scripture>
<script-template>also not a script</script-template>
<script>
const real = true
</script>
"#;

    let units = extract_vue_script_units(
        std::path::Path::new("src/Widget.vue"),
        "src/Widget.vue",
        source,
    )
    .unwrap();

    assert_eq!(units.len(), 1);
    assert_eq!(
        units[0].source_text.lines().next(),
        Some("const real = true")
    );
}

#[test]
fn syntax_classification_distinguishes_tsx_from_typescript() {
    let ts = data_flow_analyzer::source::SourceUnit {
        absolute_path: "src/app.ts".into(),
        relative_path: "src/app.ts".to_string(),
        source_text: String::new(),
        original_path: None,
        line_markers: Vec::new(),
    };
    let tsx = data_flow_analyzer::source::SourceUnit {
        absolute_path: "src/App.tsx".into(),
        relative_path: "src/App.tsx".to_string(),
        source_text: String::new(),
        original_path: None,
        line_markers: Vec::new(),
    };

    assert_eq!(syntax_for_unit(&ts), JavaScriptSyntax::TypeScript);
    assert_eq!(syntax_for_unit(&tsx), JavaScriptSyntax::Tsx);
}
