use assert_cmd::{Command, cargo::cargo_bin};
use predicates::str::contains;
use std::fs;
use tempfile::tempdir;

#[test]
fn version_command_prints_binary_name() {
    let mut cmd = Command::cargo_bin("data-flow-analyzer").unwrap();
    cmd.arg("--version")
        .assert()
        .success()
        .stdout(contains("data-flow-analyzer"));
}

#[test]
fn bare_invocation_prints_help_with_actual_binary_name() {
    let temp_dir = tempdir().unwrap();
    let original_binary = cargo_bin("data-flow-analyzer");
    let renamed_stem = "renamed-analyzer";
    let renamed_name = match original_binary.extension().and_then(|ext| ext.to_str()) {
        Some(ext) => format!("{renamed_stem}.{ext}"),
        None => renamed_stem.to_string(),
    };
    let renamed_binary = temp_dir.path().join(renamed_name);

    fs::copy(&original_binary, &renamed_binary).unwrap();

    let mut cmd = Command::new(&renamed_binary);
    cmd.assert()
        .success()
        .stdout(contains("Static def-use and dependency analyzer"))
        .stdout(contains(renamed_stem));
}

#[test]
fn analyze_command_writes_report_for_python_fixture() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("app");
    let out = dir.path().join("report");
    std::fs::create_dir_all(&input).unwrap();
    std::fs::write(
        input.join("main.py"),
        "def main():\n    x = 1\n    print(x)\n    return x\n\nmain()\n",
    )
    .unwrap();

    let mut cmd = Command::cargo_bin("data-flow-analyzer").unwrap();
    cmd.args([
        "analyze",
        "--lang",
        "python",
        "--input",
        input.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ])
    .assert()
    .success();

    assert!(out.join("index.html").exists());
    assert!(out.join("data/analysis-cache.json").exists());
}

#[test]
fn paths_command_writes_query_result_from_cache() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("app");
    let out = dir.path().join("report");
    std::fs::create_dir_all(&input).unwrap();
    std::fs::write(
        input.join("main.py"),
        "def main():\n    x = 1\n    print(x)\n    return x\n\nmain()\n",
    )
    .unwrap();

    let mut analyze = Command::cargo_bin("data-flow-analyzer").unwrap();
    analyze
        .args([
            "analyze",
            "--lang",
            "python",
            "--input",
            input.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
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
            "main",
        ])
        .assert()
        .success();

    assert!(out.join("data/path-query.json").exists());
}
