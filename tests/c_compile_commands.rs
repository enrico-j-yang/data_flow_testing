use data_flow_analyzer::cbuild::{
    configure_cmake_projects, discover_cmake_projects, merge_compile_commands, CProject,
};
use data_flow_analyzer::config::AnalyzeConfig;
use std::fs;
use std::process::Command;

#[test]
fn discover_cmake_projects_finds_cmake_roots() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("alpha")).unwrap();
    fs::create_dir_all(dir.path().join("beta/nested")).unwrap();
    fs::write(
        dir.path().join("alpha/CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.20)\nproject(alpha C)\nadd_executable(alpha main.c)\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("beta/CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.20)\nproject(beta C)\nadd_executable(beta main.c)\n",
    )
    .unwrap();

    let cfg = AnalyzeConfig {
        lang: "c".to_string(),
        input: dir.path().to_path_buf(),
        out: dir.path().join("out"),
        ..AnalyzeConfig::default()
    };

    let projects = discover_cmake_projects(&cfg).unwrap();
    let names = projects
        .iter()
        .map(|project| project.relative_name.clone())
        .collect::<Vec<_>>();

    assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);
}

#[test]
fn merge_compile_commands_deduplicates_and_sorts_entries() {
    let dir = tempfile::tempdir().unwrap();
    let left = dir.path().join("left.json");
    let right = dir.path().join("right.json");
    fs::write(
        &left,
        r#"[{"directory":"/tmp/b","file":"/tmp/b/b.c","arguments":["cc","-c","/tmp/b/b.c"]}]"#,
    )
    .unwrap();
    fs::write(
        &right,
        r#"[{"directory":"/tmp/a","file":"/tmp/a/a.c","arguments":["cc","-c","/tmp/a/a.c"]},{"directory":"/tmp/b","file":"/tmp/b/b.c","arguments":["cc","-c","/tmp/b/b.c"]}]"#,
    )
    .unwrap();

    let merged = merge_compile_commands(&[left, right], &dir.path().join("merged.json")).unwrap();

    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].file.to_string_lossy(), "/tmp/a/a.c");
    assert_eq!(merged[1].file.to_string_lossy(), "/tmp/b/b.c");
    assert!(dir.path().join("merged.json").exists());
}

#[test]
fn configure_cmake_projects_exports_compile_commands_for_simple_project() {
    if Command::new("cmake").arg("--version").output().is_err() {
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.20)\nproject(sample C)\nadd_executable(sample main.c)\n",
    )
    .unwrap();
    fs::write(dir.path().join("main.c"), "int main(void) { return 0; }\n").unwrap();

    let cfg = AnalyzeConfig {
        lang: "c".to_string(),
        input: dir.path().to_path_buf(),
        out: dir.path().join("out"),
        build_root: Some(dir.path().join("build")),
        ..AnalyzeConfig::default()
    };

    let projects = vec![CProject {
        source_dir: dir.path().to_path_buf(),
        relative_name: ".".to_string(),
    }];
    let configured = configure_cmake_projects(&projects, &cfg).unwrap();

    assert_eq!(configured.len(), 1);
    assert!(configured[0].compile_commands_path.exists());
}
