use assert_cmd::Command;
use data_flow_analyzer::cli::{AnalyzeLanguage, parse_analyze_language};

#[test]
fn js_family_language_aliases_are_recognized() {
    assert_eq!(
        parse_analyze_language("javascript").unwrap(),
        AnalyzeLanguage::JavaScript
    );
    assert_eq!(
        parse_analyze_language("js").unwrap(),
        AnalyzeLanguage::JavaScript
    );
    assert_eq!(
        parse_analyze_language("typescript").unwrap(),
        AnalyzeLanguage::TypeScript
    );
    assert_eq!(
        parse_analyze_language("ts").unwrap(),
        AnalyzeLanguage::TypeScript
    );
    assert_eq!(
        parse_analyze_language("js-ts").unwrap(),
        AnalyzeLanguage::JavaScriptTypeScript
    );
}

#[test]
fn unsupported_language_error_mentions_js_ts() {
    let dir = tempfile::tempdir().unwrap();
    let mut cmd = Command::cargo_bin("data-flow-analyzer").unwrap();
    cmd.args([
        "analyze",
        "--lang",
        "ruby",
        "--input",
        dir.path().to_str().unwrap(),
        "--out",
        dir.path().join("out").to_str().unwrap(),
    ])
    .assert()
    .failure()
    .stderr(predicates::str::contains("js-ts"))
    .stderr(predicates::str::contains("typescript"));
}
