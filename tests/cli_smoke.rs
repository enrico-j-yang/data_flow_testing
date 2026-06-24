use clap::Parser;
use data_flow_analyzer::cli::{Cli, Commands};
use assert_cmd::{Command, cargo::cargo_bin};
use predicates::str::contains;
use std::fs;
use std::path::PathBuf;
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
fn analyze_help_mentions_c_build_flags() {
    let mut cmd = Command::cargo_bin("data-flow-analyzer").unwrap();
    cmd.args(["analyze", "--help"])
        .assert()
        .success()
        .stdout(contains("--build-root"))
        .stdout(contains("--cmake-arg"))
        .stdout(contains("--keep-preprocessed"));
}

#[test]
fn analyze_command_accepts_c_build_flags() {
    let dir = tempdir().unwrap();
    let input = dir.path().join("app");
    let out = dir.path().join("report");
    let build_root = dir.path().join("build");
    std::fs::create_dir_all(&input).unwrap();
    std::fs::create_dir_all(&build_root).unwrap();
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
        "--build-root",
        build_root.to_str().unwrap(),
        "--cmake-arg",
        "-DFIRST=1",
        "--cmake-arg",
        "-DSECOND=2",
        "--keep-preprocessed",
    ])
    .assert()
    .success();

    assert!(out.join("index.html").exists());
    assert!(out.join("data/analysis-cache.json").exists());
}

#[test]
fn analyze_command_parses_c_build_flags() {
    let cli = Cli::try_parse_from([
        "data-flow-analyzer",
        "analyze",
        "--build-root",
        "build/cmake",
        "--cmake-arg",
        "-DFIRST=1",
        "--cmake-arg",
        "-DSECOND=2",
        "--keep-preprocessed",
    ])
    .unwrap();

    match cli.command {
        Some(Commands::Analyze {
            build_root,
            cmake_args,
            keep_preprocessed,
            ..
        }) => {
            assert_eq!(build_root, Some(PathBuf::from("build/cmake")));
            assert_eq!(cmake_args, vec!["-DFIRST=1", "-DSECOND=2"]);
            assert!(keep_preprocessed);
        }
        other => panic!("expected analyze command, got {other:?}"),
    }
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
