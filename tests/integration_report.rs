use data_flow_analyzer::ir::{AnalysisCache, SCHEMA_VERSION};
use data_flow_analyzer::report::write_report;

#[test]
fn report_writes_index_cache_and_csv_headers() {
    let dir = tempfile::tempdir().unwrap();
    let cache = AnalysisCache {
        schema_version: SCHEMA_VERSION,
        tool_version: "0.1.0".to_string(),
        ..AnalysisCache::default()
    };

    write_report(&cache, dir.path(), 100).unwrap();

    assert!(dir.path().join("index.html").exists());
    assert!(dir.path().join("data/analysis-cache.json").exists());
    assert!(dir.path().join("graphs/def_use_hotspots.dot").exists());
    assert!(dir.path().join("data/def_use_edges.csv").exists());
    assert!(dir.path().join("data/parse_diagnostics.csv").exists());
    let definitions = std::fs::read_to_string(dir.path().join("data/definitions.csv")).unwrap();
    assert!(definitions.starts_with("def_id,file,path,line,col"));
}

#[test]
fn analyzer_processes_golden_fixtures() {
    for fixture in [
        "tests/fixtures/python_basic",
        "tests/fixtures/python_control_flow",
        "tests/fixtures/python_calls",
        "tests/fixtures/python_classes",
    ] {
        let out = tempfile::tempdir().unwrap();
        let mut cmd = assert_cmd::Command::cargo_bin("data-flow-analyzer").unwrap();
        cmd.args([
            "analyze",
            "--lang",
            "python",
            "--input",
            fixture,
            "--out",
            out.path().to_str().unwrap(),
        ])
        .assert()
        .success();

        let cache = std::fs::read_to_string(out.path().join("data/analysis-cache.json")).unwrap();
        assert!(cache.contains("\"schema_version\""));
        assert!(out.path().join("index.html").exists());
    }
}
