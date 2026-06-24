use data_flow_analyzer::cbuild::{
    configure_cmake_projects, discover_cmake_projects, merge_compile_commands, CProject,
    CompileCommand,
};
use data_flow_analyzer::ccompile::{
    build_preprocess_arguments, load_preprocessed_unit, parse_line_markers,
};
use data_flow_analyzer::config::AnalyzeConfig;
use data_flow_analyzer::source::LineMarker;
use std::env;
use std::ffi::OsString;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

fn cmake_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct PathGuard {
    original: Option<OsString>,
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        match &self.original {
            Some(path) => unsafe {
                env::set_var("PATH", path);
            },
            None => unsafe {
                env::remove_var("PATH");
            },
        }
    }
}

fn prepend_path(path: &Path) -> PathGuard {
    // These tests stub `cmake` via PATH, so callers hold `cmake_test_lock` first.
    let original = env::var_os("PATH");
    let mut paths = vec![path.to_path_buf()];
    if let Some(existing) = &original {
        paths.extend(env::split_paths(existing));
    }

    let joined = env::join_paths(paths).unwrap();
    unsafe {
        env::set_var("PATH", joined);
    }

    PathGuard { original }
}

fn write_cmake_stub(dir: &Path, body: &str) {
    let stub_path = cmake_stub_path(dir);
    fs::write(&stub_path, cmake_stub_contents(body)).unwrap();
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(&stub_path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&stub_path, permissions).unwrap();
    }
}

#[cfg(windows)]
fn cmake_stub_path(dir: &Path) -> PathBuf {
    dir.join("cmake.bat")
}

#[cfg(not(windows))]
fn cmake_stub_path(dir: &Path) -> PathBuf {
    dir.join("cmake")
}

#[cfg(windows)]
fn cmake_stub_contents(body: &str) -> String {
    format!("@echo off\r\n{}\r\n", body)
}

#[cfg(not(windows))]
fn cmake_stub_contents(body: &str) -> String {
    format!("#!/bin/sh\nset -eu\n{}\n", body)
}

#[cfg(windows)]
fn stub_body_writing_compile_commands() -> &'static str {
    r#"set "build_dir="
:loop
if "%~1"=="" goto done
if "%~1"=="-B" (
  set "build_dir=%~2"
  shift
  shift
  goto loop
)
shift
goto loop
:done
if "%build_dir%"=="" exit /b 1
if not exist "%build_dir%" mkdir "%build_dir%"
> "%build_dir%\compile_commands.json" echo []"#
}

#[cfg(not(windows))]
fn stub_body_writing_compile_commands() -> &'static str {
    r#"build_dir=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-B" ]; then
    build_dir="$2"
    shift 2
    continue
  fi
  shift
done
mkdir -p "$build_dir"
printf '[]' > "$build_dir/compile_commands.json""#
}

#[cfg(windows)]
fn stub_body_missing_compile_commands() -> &'static str {
    "exit /b 0"
}

#[cfg(not(windows))]
fn stub_body_missing_compile_commands() -> &'static str {
    "exit 0"
}

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
fn discover_cmake_projects_finds_deeply_nested_cmake_roots() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir
        .path()
        .join("level1")
        .join("level2")
        .join("level3")
        .join("level4")
        .join("level5")
        .join("level6")
        .join("level7");
    fs::create_dir_all(&nested).unwrap();
    fs::write(
        nested.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.20)\nproject(deep C)\n",
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

    assert_eq!(
        names,
        vec!["level1/level2/level3/level4/level5/level6/level7".to_string()]
    );
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
    let _cmake_lock = cmake_test_lock().lock().unwrap();
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
fn configure_cmake_projects_uses_distinct_build_dirs_for_colliding_relative_names() {
    let _cmake_lock = cmake_test_lock().lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let stub_dir = dir.path().join("stub-bin");
    fs::create_dir_all(&stub_dir).unwrap();
    write_cmake_stub(&stub_dir, stub_body_writing_compile_commands());
    let _path_guard = prepend_path(&stub_dir);

    let nested = dir.path().join("a").join("b");
    let flattened = dir.path().join("a__b");
    fs::create_dir_all(&nested).unwrap();
    fs::create_dir_all(&flattened).unwrap();

    let cfg = AnalyzeConfig {
        lang: "c".to_string(),
        input: dir.path().to_path_buf(),
        out: dir.path().join("out"),
        build_root: Some(dir.path().join("build")),
        ..AnalyzeConfig::default()
    };
    let projects = vec![
        CProject {
            source_dir: nested,
            relative_name: "a/b".to_string(),
        },
        CProject {
            source_dir: flattened,
            relative_name: "a__b".to_string(),
        },
    ];

    let configured = configure_cmake_projects(&projects, &cfg).unwrap();

    assert_eq!(configured.len(), 2);
    assert_ne!(configured[0].build_dir, configured[1].build_dir);
    assert!(configured[0].compile_commands_path.exists());
    assert!(configured[1].compile_commands_path.exists());
}

#[test]
fn configure_cmake_projects_errors_when_compile_commands_are_missing() {
    let _cmake_lock = cmake_test_lock().lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let stub_dir = dir.path().join("stub-bin");
    fs::create_dir_all(&stub_dir).unwrap();
    write_cmake_stub(&stub_dir, stub_body_missing_compile_commands());
    let _path_guard = prepend_path(&stub_dir);

    let project_dir = dir.path().join("sample");
    fs::create_dir_all(&project_dir).unwrap();

    let cfg = AnalyzeConfig {
        lang: "c".to_string(),
        input: dir.path().to_path_buf(),
        out: dir.path().join("out"),
        build_root: Some(dir.path().join("build")),
        ..AnalyzeConfig::default()
    };
    let projects = vec![CProject {
        source_dir: project_dir.clone(),
        relative_name: "sample".to_string(),
    }];

    let err = configure_cmake_projects(&projects, &cfg).unwrap_err();

    assert!(err.to_string().contains("compile_commands.json"));
    assert!(err.to_string().contains(project_dir.to_string_lossy().as_ref()));
}

#[test]
fn build_preprocess_arguments_rewrites_compile_invocation() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("sample.i");
    let command = CompileCommand {
        directory: dir.path().to_path_buf(),
        file: dir.path().join("src").join("sample.c"),
        arguments: Vec::new(),
        command: Some(
            "clang -Iinclude -DMODE=1 -include config.h -std=c11 -c src/sample.c -o sample.o -MMD -MF sample.d"
                .to_string(),
        ),
        output: Some(dir.path().join("sample.o")),
    };

    let args = build_preprocess_arguments(&command, &output).unwrap();

    assert_eq!(
        args,
        vec![
            "clang".to_string(),
            "-E".to_string(),
            "-dD".to_string(),
            "-Iinclude".to_string(),
            "-DMODE=1".to_string(),
            "-include".to_string(),
            "config.h".to_string(),
            "-std=c11".to_string(),
            "src/sample.c".to_string(),
            "-o".to_string(),
            output.display().to_string(),
        ]
    );
}

#[test]
fn parse_line_markers_maps_original_files() {
    let preprocessed = r#"# 1 "src/main.c" 1
int root = 0;
# 1 "include/config.h" 1
#define FLAG 1
# 2 "src/main.c" 2
int value = FLAG;
"#;

    let markers = parse_line_markers(preprocessed);

    assert_eq!(
        markers,
        vec![
            LineMarker {
                generated_line: 1,
                original_file: "src/main.c".to_string(),
                original_line: 1,
            },
            LineMarker {
                generated_line: 3,
                original_file: "include/config.h".to_string(),
                original_line: 1,
            },
            LineMarker {
                generated_line: 5,
                original_file: "src/main.c".to_string(),
                original_line: 2,
            },
        ]
    );
}

#[test]
fn load_preprocessed_unit_preserves_full_preprocessed_text_and_source_identity() {
    let dir = tempfile::tempdir().unwrap();
    let source_path = dir.path().join("main.c");
    let preprocessed_path = dir.path().join("main.i");
    let preprocessed = r#"# 1 "main.c" 1
int root = 0;
# 7 "config.h" 1
#define FLAG 1
# 2 "main.c" 2
int value = FLAG;
"#;
    fs::write(&preprocessed_path, preprocessed).unwrap();

    let unit = load_preprocessed_unit(&source_path, &preprocessed_path).unwrap();

    assert_eq!(unit.absolute_path, source_path);
    assert_eq!(unit.relative_path, "main.c".to_string());
    assert_eq!(unit.original_path, Some(unit.absolute_path.clone()));
    assert_eq!(unit.source_text, preprocessed.to_string());
    assert_eq!(
        unit.line_markers,
        vec![
            LineMarker {
                generated_line: 1,
                original_file: "main.c".to_string(),
                original_line: 1,
            },
            LineMarker {
                generated_line: 3,
                original_file: "config.h".to_string(),
                original_line: 7,
            },
            LineMarker {
                generated_line: 5,
                original_file: "main.c".to_string(),
                original_line: 2,
            },
        ]
    );
}
