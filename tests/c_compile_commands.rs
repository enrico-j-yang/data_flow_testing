use data_flow_analyzer::cbuild::{
    CProject, CompileCommand, configure_cmake_projects, discover_cmake_projects,
    merge_compile_commands,
};
use data_flow_analyzer::ccompile::{build_preprocess_arguments, parse_line_markers};
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

#[test]
fn build_preprocess_arguments_rewrites_compile_invocation() {
    let command = CompileCommand {
        directory: std::path::PathBuf::from("/tmp/project"),
        file: std::path::PathBuf::from("/tmp/project/main.c"),
        arguments: vec![
            "cc".to_string(),
            "-Iinclude".to_string(),
            "-DVALUE=1".to_string(),
            "-c".to_string(),
            "main.c".to_string(),
            "-o".to_string(),
            "main.o".to_string(),
        ],
        command: None,
        output: Some(std::path::PathBuf::from("main.o")),
    };

    let args = build_preprocess_arguments(&command, std::path::Path::new("main.i")).unwrap();

    assert_eq!(args[0], "cc");
    assert!(args.iter().any(|arg| arg == "-E"));
    assert!(args.iter().any(|arg| arg == "-Iinclude"));
    assert!(args.iter().any(|arg| arg == "-DVALUE=1"));
    assert!(!args.iter().any(|arg| arg == "-c"));
    assert!(!args.iter().any(|arg| arg == "main.o"));
    let out_index = args.iter().position(|arg| arg == "-o").unwrap();
    assert_eq!(args[out_index + 1], "main.i");
}

#[test]
fn parse_line_markers_maps_original_files() {
    let text = "# 1 \"/tmp/project/main.c\"\nint main(void) {\n# 12 \"/tmp/project/include/value.h\"\n  return VALUE;\n# 3 \"/tmp/project/main.c\"\n}\n";

    let markers = parse_line_markers(text);

    assert_eq!(markers.len(), 3);
    assert_eq!(markers[0].original_file, "/tmp/project/main.c");
    assert_eq!(markers[0].original_line, 1);
    assert_eq!(markers[1].original_file, "/tmp/project/include/value.h");
    assert_eq!(markers[1].original_line, 12);
    assert_eq!(markers[2].original_file, "/tmp/project/main.c");
    assert_eq!(markers[2].original_line, 3);
}
