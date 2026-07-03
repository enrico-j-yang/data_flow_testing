use data_flow_analyzer::config::AnalyzeConfig;
use data_flow_analyzer::js_source::{
    JavaScriptSyntax, discover_js_sources, extract_vue_script_units, syntax_for_unit,
};
use std::fs;

#[test]
fn js_source_discovery_skips_vendor_and_cache_dirs() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::create_dir_all(dir.path().join("node_modules/pkg")).unwrap();
    fs::create_dir_all(dir.path().join(".turbo/cache")).unwrap();
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
        dir.path().join(".turbo/cache/generated.ts"),
        "export const cached = 1;\n",
    )
    .unwrap();

    let cfg = AnalyzeConfig {
        lang: "js-ts".to_string(),
        input: dir.path().to_path_buf(),
        out: dir.path().join("out"),
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
