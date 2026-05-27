use data_flow_analyzer::config::AnalyzeConfig;
use data_flow_analyzer::fs::discover_sources;
use std::fs;

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
