use data_flow_analyzer::config::AnalyzeConfig;
use data_flow_analyzer::js_source::{
    JavaScriptSyntax, discover_js_sources, extract_vue_script_units, syntax_for_unit,
};
use std::fs;

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
    assert!(
        paths
            .iter()
            .any(|path| path == "src/view.vue?script=setup&lang=ts")
    );
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
    assert!(
        units[1]
            .source_text
            .contains("const value: number = normal")
    );
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
