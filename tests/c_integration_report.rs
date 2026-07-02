use assert_cmd::Command;
use std::fs;
use std::path::Path;

fn write_cmake_fixture(root: &Path) {
    fs::write(
        root.join("CMakeLists.txt"),
        r#"
cmake_minimum_required(VERSION 3.20)
project(c_report_fixture C)
add_executable(c_report_fixture main.c helper.c)
"#,
    )
    .unwrap();
    fs::write(root.join("helper.h"), "int helper(int value);\n").unwrap();
    fs::write(
        root.join("helper.c"),
        "int helper(int value) { return value + 1; }\n",
    )
    .unwrap();
    fs::write(
        root.join("main.c"),
        r#"
#include "helper.h"
int run(int input) {
    int result = helper(input);
    return result;
}
"#,
    )
    .unwrap();
}

#[test]
fn analyze_command_writes_report_for_c_fixture() {
    if std::process::Command::new("cmake")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("cmake not available; skipping");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("app");
    let out = dir.path().join("report");
    let build = dir.path().join("build");
    fs::create_dir_all(&input).unwrap();
    write_cmake_fixture(&input);

    let mut cmd = Command::cargo_bin("data-flow-analyzer").unwrap();
    cmd.args([
        "analyze",
        "--lang",
        "c",
        "--input",
        input.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--build-root",
        build.to_str().unwrap(),
    ])
    .assert()
    .success();

    assert!(out.join("index.html").exists(), "index.html missing");
    assert!(
        out.join("data/analysis-cache.json").exists(),
        "analysis-cache.json missing"
    );
    assert!(
        out.join("data/compile_commands.merged.json").exists(),
        "merged compile_commands missing"
    );
}

#[test]
fn paths_command_writes_query_result_from_c_cache() {
    if std::process::Command::new("cmake")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("cmake not available; skipping");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("app");
    let out = dir.path().join("report");
    let build = dir.path().join("build");
    fs::create_dir_all(&input).unwrap();
    write_cmake_fixture(&input);

    let mut analyze = Command::cargo_bin("data-flow-analyzer").unwrap();
    analyze
        .args([
            "analyze",
            "--lang",
            "c",
            "--input",
            input.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--build-root",
            build.to_str().unwrap(),
        ])
        .assert()
        .success();

    let mut paths = Command::cargo_bin("data-flow-analyzer").unwrap();
    paths
        .args([
            "paths",
            "--input",
            out.join("data/analysis-cache.json").to_str().unwrap(),
            "--function",
            "run",
        ])
        .assert()
        .success();

    assert!(out.join("data/path-query.json").exists());
}

#[ignore]
#[test]
fn lschat_tests_can_be_analyzed_with_real_compile_context() {
    let input = Path::new("/mnt/d/repos/arcs_mini/modules/lschat/tests");
    if !input.exists() {
        eprintln!("LSChat tests not present at {}; skipping", input.display());
        return;
    }

    let out = tempfile::tempdir().unwrap();
    let build = tempfile::tempdir().unwrap();
    let mut cmd = Command::cargo_bin("data-flow-analyzer").unwrap();
    cmd.args([
        "analyze",
        "--lang",
        "c",
        "--input",
        input.to_str().unwrap(),
        "--out",
        out.path().to_str().unwrap(),
        "--build-root",
        build.path().to_str().unwrap(),
        "--cmake-arg",
        "-DLSCHAT_SKIP_GIT_VERSION=ON",
    ])
    .assert()
    .success();

    assert!(
        out.path()
            .join("data/compile_commands.merged.json")
            .exists()
    );
    assert!(out.path().join("index.html").exists());
}

#[test]
fn paths_query_path_uses_cache_parent_dir() {
    // Sanity check that the paths command always writes alongside its input.
    let dir = tempfile::tempdir().unwrap();
    let cache_path = dir.path().join("custom/sub/analysis-cache.json");
    fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    let minimal_cache = data_flow_analyzer::ir::AnalysisCache {
        functions: vec![data_flow_analyzer::ir::FunctionRecord {
            function_id: "F_dummy".to_string(),
            module_id: "M_dummy".to_string(),
            class_id: None,
            qualified_name: "dummy".to_string(),
            kind: "function".to_string(),
            params: Vec::new(),
            scope_id: "S_dummy".to_string(),
            span: data_flow_analyzer::source::SourceSpan::synthetic("dummy.c", ""),
        }],
        ..data_flow_analyzer::ir::AnalysisCache::default()
    };
    fs::write(&cache_path, serde_json::to_string(&minimal_cache).unwrap()).unwrap();

    let mut paths = Command::cargo_bin("data-flow-analyzer").unwrap();
    paths
        .args([
            "paths",
            "--input",
            cache_path.to_str().unwrap(),
            "--function",
            "dummy",
        ])
        .assert()
        .success();

    assert!(
        cache_path
            .parent()
            .unwrap()
            .join("path-query.json")
            .exists()
    );
}
