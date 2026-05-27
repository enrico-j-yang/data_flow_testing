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
