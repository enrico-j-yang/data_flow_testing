use data_flow_analyzer::config::AnalyzeConfig;
use data_flow_analyzer::fs::discover_sources;
use std::fs;
#[cfg(windows)]
use std::process::Command;

#[test]
fn config_file_loads_defaults_and_cli_overrides() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("dataflow.toml");
    fs::write(
        &cfg_path,
        r#"
lang = "python"
input = "D:/repos/asr_platform/app"
out = "D:/tmp/dataflow_report"
max_loop_unroll = 2
top_n = 50
emit_full_dot = false
render_full_svg = false
fail_on_parse_error = false
parallelism = "auto"
exclude = ["**/__pycache__/**"]
stub_paths = ["stubs"]
"#,
    )
    .unwrap();

    let mut cfg = AnalyzeConfig::from_toml_file(&cfg_path).unwrap();
    cfg.apply_cli_overrides(None, None, Some(dir.path().join("override_out")));

    assert_eq!(cfg.lang, "python");
    assert_eq!(cfg.max_loop_unroll, 2);
    assert_eq!(cfg.top_n, 50);
    assert!(cfg.out.ends_with("override_out"));
}

#[test]
fn config_file_resolves_relative_paths_from_config_directory() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_dir = dir.path().join("config");
    fs::create_dir_all(&cfg_dir).unwrap();
    let cfg_path = cfg_dir.join("dataflow.toml");
    fs::write(
        &cfg_path,
        r#"
input = "project/app"
out = "reports/dataflow"
stub_paths = ["stubs"]
"#,
    )
    .unwrap();

    let cfg = AnalyzeConfig::from_toml_file(&cfg_path).unwrap();

    assert_eq!(cfg.input, cfg_dir.join("project/app"));
    assert_eq!(cfg.out, cfg_dir.join("reports/dataflow"));
    assert_eq!(cfg.stub_paths, vec![cfg_dir.join("stubs")]);
}

#[test]
fn config_file_preserves_explicit_empty_excludes() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("dataflow.toml");
    fs::write(
        &cfg_path,
        r#"
exclude = []
"#,
    )
    .unwrap();

    let cfg = AnalyzeConfig::from_toml_file(&cfg_path).unwrap();

    assert!(cfg.exclude.is_empty());
}

#[test]
fn discovery_skips_ignored_python_files() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("app/__pycache__")).unwrap();
    fs::create_dir_all(dir.path().join("app/routers")).unwrap();
    fs::write(dir.path().join("app/main.py"), "x = 1\n").unwrap();
    fs::write(dir.path().join("app/routers/tests.py"), "y = 2\n").unwrap();
    fs::write(dir.path().join("app/__pycache__/ignored.py"), "z = 3\n").unwrap();

    let cfg = AnalyzeConfig {
        input: dir.path().join("app"),
        exclude: vec!["**/__pycache__/**".to_string()],
        ..AnalyzeConfig::default()
    };

    let files = discover_sources(&cfg).unwrap();
    let paths: Vec<String> = files.iter().map(|f| f.relative_path.clone()).collect();

    assert_eq!(paths, vec!["main.py", "routers/tests.py"]);
}

#[cfg(windows)]
#[test]
fn discovery_surfaces_walkdir_traversal_errors() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app");
    let blocked = app.join("blocked");
    fs::create_dir_all(&blocked).unwrap();
    fs::write(blocked.join("hidden.py"), "y = 2\n").unwrap();
    fs::write(app.join("main.py"), "x = 1\n").unwrap();

    let whoami = Command::new("whoami").output().unwrap();
    assert!(
        whoami.status.success(),
        "whoami failed: {}",
        String::from_utf8_lossy(&whoami.stderr)
    );
    let identity = String::from_utf8(whoami.stdout).unwrap();
    let identity = identity.trim();

    let output = Command::new("icacls")
        .arg(&blocked)
        .args(["/inheritance:r", "/deny", &format!("{identity}:(OI)(CI)(RX)")])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "icacls deny failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cfg = AnalyzeConfig {
        input: app,
        exclude: Vec::new(),
        ..AnalyzeConfig::default()
    };

    let result = discover_sources(&cfg);

    let restore = Command::new("icacls")
        .arg(&blocked)
        .args(["/remove:d", identity])
        .output()
        .unwrap();
    assert!(
        restore.status.success(),
        "icacls restore failed: {}",
        String::from_utf8_lossy(&restore.stderr)
    );

    let err = result.unwrap_err();
    let message = err.to_string();

    assert!(message.contains("blocked"), "unexpected error: {message}");
}
