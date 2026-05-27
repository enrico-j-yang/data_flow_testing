use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn version_command_prints_binary_name() {
    let mut cmd = Command::cargo_bin("data-flow-analyzer").unwrap();
    cmd.arg("--version")
        .assert()
        .success()
        .stdout(contains("data-flow-analyzer"));
}
