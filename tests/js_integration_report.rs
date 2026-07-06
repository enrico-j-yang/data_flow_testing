use assert_cmd::Command;
use std::fs;
use std::path::Path;

#[test]
fn analyze_command_writes_report_for_js_ts_vue_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("app");
    let out = dir.path().join("report");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("dep.ts"), "export const source = 1\n").unwrap();
    fs::write(
        input.join("main.ts"),
        "import { source } from './dep'\nexport function run(input: number) { const next = source + input; return next }\n",
    )
    .unwrap();
    fs::write(
        input.join("Widget.vue"),
        "<script setup lang=\"ts\">\nconst props = defineProps<{ title: string }>()\nconst value = props.title\n</script>\n",
    )
    .unwrap();

    let mut cmd = Command::cargo_bin("data-flow-analyzer").unwrap();
    cmd.args([
        "analyze",
        "--lang",
        "js-ts",
        "--input",
        input.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ])
    .assert()
    .success();

    assert!(out.join("index.html").exists());
    assert!(out.join("data/analysis-cache.json").exists());
    assert!(out.join("data/definitions.csv").exists());
    assert!(out.join("data/uses.csv").exists());
    assert!(out.join("graphs/variable_dependencies.dot").exists());

    let cache_text = fs::read_to_string(out.join("data/analysis-cache.json")).unwrap();
    assert!(cache_text.contains("main.ts::run"));
    assert!(cache_text.contains("Widget.vue?script=setup&lang=ts"));
}

#[test]
fn paths_command_writes_query_result_from_js_ts_cache() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("app");
    let out = dir.path().join("report");
    fs::create_dir_all(&input).unwrap();
    fs::write(
        input.join("main.ts"),
        "export function run(input: number) { const next = input + 1; return next }\n",
    )
    .unwrap();

    Command::cargo_bin("data-flow-analyzer")
        .unwrap()
        .args([
            "analyze",
            "--lang",
            "js-ts",
            "--input",
            input.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .assert()
        .success();

    Command::cargo_bin("data-flow-analyzer")
        .unwrap()
        .args([
            "paths",
            "--input",
            out.join("data/analysis-cache.json").to_str().unwrap(),
            "--function",
            "main.ts::run",
        ])
        .assert()
        .success();

    assert!(out.join("data/path-query.json").exists());
}

#[ignore]
#[test]
fn airi_js_ts_vue_tree_can_be_analyzed() {
    let input = Path::new("D:/repos/temp/airi");
    if !input.exists() {
        eprintln!("AIRI checkout not present at {}; skipping", input.display());
        return;
    }

    let out = tempfile::tempdir().unwrap();
    Command::cargo_bin("data-flow-analyzer")
        .unwrap()
        .args([
            "analyze",
            "--lang",
            "js-ts",
            "--input",
            input.to_str().unwrap(),
            "--out",
            out.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    assert!(out.path().join("index.html").exists());
    assert!(out.path().join("data/analysis-cache.json").exists());
    assert!(
        out.path()
            .join("graphs/variable_dependencies.graph.json")
            .exists()
    );
    let cache_text = fs::read_to_string(out.path().join("data/analysis-cache.json")).unwrap();
    assert!(cache_text.contains("\"files\""));
    assert!(cache_text.contains("\"functions\""));
    assert!(cache_text.contains("\"definitions\""));
    assert!(cache_text.contains("\"uses\""));
}
