use crate::cbuild::CompileCommand;
use crate::source::{LineMarker, SourceUnit};
use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::Path;

/// Translate a compile command into a preprocessing invocation. The returned
/// argument list keeps the original compiler driver but adds `-E -dD` and
/// drops `-c`/`-o <path>` so the preprocessed translation unit lands at
/// `output_path`.
pub fn build_preprocess_arguments(
    command: &CompileCommand,
    output_path: &Path,
) -> Result<Vec<String>> {
    let raw_args = if command.arguments.is_empty() {
        command
            .command
            .as_deref()
            .and_then(split_compile_command)
            .ok_or_else(|| anyhow!("compile command is empty"))?
    } else {
        command.arguments.clone()
    };
    let compiler = raw_args
        .first()
        .cloned()
        .map(normalize_tool_arg)
        .ok_or_else(|| anyhow!("compile command is empty"))?;
    let mut args = vec![compiler, "-E".to_string(), "-dD".to_string()];
    let mut skip_next = false;

    for arg in raw_args.iter().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }
        match arg.as_str() {
            "-c" => {}
            "-o" => skip_next = true,
            _ => args.push(normalize_tool_arg(arg.clone())),
        }
    }

    args.push("-o".to_string());
    args.push(normalize_tool_arg(
        output_path.to_string_lossy().to_string(),
    ));
    Ok(args)
}

#[cfg(not(windows))]
fn split_compile_command(command: &str) -> Option<Vec<String>> {
    shlex::split(command)
}

#[cfg(windows)]
fn split_compile_command(command: &str) -> Option<Vec<String>> {
    let args = split_windows_command_line(command);
    if args.is_empty() { None } else { Some(args) }
}

#[cfg(windows)]
fn split_windows_command_line(command: &str) -> Vec<String> {
    // CMake's compile_commands.json stores a Windows command line, not a POSIX shell line.
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut started = false;
    let mut backslashes = 0usize;

    for ch in command.chars() {
        match ch {
            '\\' => {
                backslashes += 1;
                started = true;
            }
            '"' => {
                current.extend(std::iter::repeat_n('\\', backslashes / 2));
                if backslashes.is_multiple_of(2) {
                    in_quotes = !in_quotes;
                    started = true;
                } else {
                    current.push('"');
                }
                backslashes = 0;
            }
            ch if ch.is_whitespace() && !in_quotes => {
                current.extend(std::iter::repeat_n('\\', backslashes));
                backslashes = 0;
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            ch => {
                current.extend(std::iter::repeat_n('\\', backslashes));
                backslashes = 0;
                current.push(ch);
                started = true;
            }
        }
    }

    current.extend(std::iter::repeat_n('\\', backslashes));
    if started {
        args.push(current);
    }
    args
}

#[cfg(not(windows))]
fn normalize_tool_arg(arg: String) -> String {
    arg
}

#[cfg(windows)]
fn normalize_tool_arg(arg: String) -> String {
    if let Some(rest) = arg.strip_prefix("//?/UNC/") {
        format!("//{rest}")
    } else if let Some(rest) = arg.strip_prefix("//?/") {
        rest.to_string()
    } else if let Some(rest) = arg.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{rest}")
    } else if let Some(rest) = arg.strip_prefix("\\\\?\\") {
        rest.to_string()
    } else {
        arg
    }
}

/// Parse the `# <line> "<file>"` preprocessor line markers emitted by GCC and
/// Clang in `-E` output. Each marker tells us where the next group of lines in
/// the preprocessed text originally came from.
pub fn parse_line_markers(text: &str) -> Vec<LineMarker> {
    let mut markers = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('#') {
            continue;
        }
        let rest = trimmed.trim_start_matches('#').trim();
        let Some(parts) = shlex::split(rest) else {
            continue;
        };
        if parts.len() < 2 {
            continue;
        }
        let Ok(original_line) = parts[0].parse::<usize>() else {
            continue;
        };
        let original_file = parts[1].clone();
        markers.push(LineMarker {
            generated_line: index + 1,
            original_file,
            original_line,
        });
    }
    markers
}

/// Construct a `SourceUnit` from a preprocessed translation unit on disk.
pub fn load_preprocessed_unit(source_path: &Path, preprocessed_path: &Path) -> Result<SourceUnit> {
    let source_text = fs::read_to_string(preprocessed_path).with_context(|| {
        format!(
            "failed to read preprocessed unit {}",
            preprocessed_path.display()
        )
    })?;
    let relative_path = source_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let line_markers = parse_line_markers(&source_text);
    Ok(SourceUnit {
        absolute_path: source_path.to_path_buf(),
        relative_path,
        source_text,
        original_path: Some(source_path.to_path_buf()),
        line_markers,
    })
}
