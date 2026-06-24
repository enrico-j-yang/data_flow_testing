use crate::cbuild::CompileCommand;
use crate::fs::normalize_path;
use crate::source::{LineMarker, SourceUnit};
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::Path;

pub fn build_preprocess_arguments(
    command: &CompileCommand,
    output_path: &Path,
) -> Result<Vec<String>> {
    let tokens = command_tokens(command)?;
    if tokens.is_empty() {
        bail!(
            "compile command for {} is empty",
            command.file.to_string_lossy()
        );
    }

    let mut args = vec![tokens[0].clone(), "-E".to_string(), "-dD".to_string()];
    let mut saw_source = false;
    let mut index = 1;

    while index < tokens.len() {
        let token = &tokens[index];
        match token.as_str() {
            "-c" | "-S" | "-E" | "-dD" | "-MMD" | "-MD" | "-MG" | "-MP" | "-MM" | "-M" => {
                index += 1;
            }
            "-o" | "-MF" | "-MT" | "-MQ" | "-MJ" => {
                index += 2;
            }
            "-include" | "-imacros" | "-isystem" | "-iquote" | "-idirafter" | "-I" | "-D"
            | "-U" | "-x" => {
                let value = tokens.get(index + 1).with_context(|| {
                    format!("expected value after {token} for {}", command.file.display())
                })?;
                args.push(token.clone());
                args.push(value.clone());
                index += 2;
            }
            _ if is_single_token_passthrough_flag(token) => {
                args.push(token.clone());
                index += 1;
            }
            _ if is_inline_drop_flag(token) => {
                index += 1;
            }
            _ => {
                if !token.starts_with('-') {
                    saw_source = true;
                }
                args.push(token.clone());
                index += 1;
            }
        }
    }

    if !saw_source {
        args.push(command.file.to_string_lossy().into_owned());
    }

    args.push("-o".to_string());
    args.push(output_path.to_string_lossy().into_owned());
    Ok(args)
}

pub fn parse_line_markers(source: &str) -> Vec<LineMarker> {
    source
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let line = line.trim_end_matches('\r');
            parse_line_marker(line).map(|(original_line, original_file)| LineMarker {
                generated_line: index + 1,
                original_file,
                original_line,
            })
        })
        .collect()
}

pub fn load_preprocessed_unit(source_path: &Path, preprocessed_path: &Path) -> Result<SourceUnit> {
    let source_text = fs::read_to_string(preprocessed_path)
        .with_context(|| format!("failed to read {}", preprocessed_path.display()))?;
    let line_markers = parse_line_markers(&source_text);
    let relative_path = source_path
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| normalize_path(source_path));

    Ok(SourceUnit {
        absolute_path: source_path.to_path_buf(),
        relative_path,
        source_text,
        original_path: Some(source_path.to_path_buf()),
        line_markers,
    })
}

fn command_tokens(command: &CompileCommand) -> Result<Vec<String>> {
    if !command.arguments.is_empty() {
        return Ok(command.arguments.clone());
    }

    let shell_command = command
        .command
        .as_deref()
        .context("compile command is missing both arguments and command")?;
    shlex::split(shell_command).with_context(|| format!("failed to parse `{shell_command}`"))
}

fn is_single_token_passthrough_flag(token: &str) -> bool {
    matches!(token, "-nostdinc" | "-nostdinc++")
        || token.starts_with("-I")
        || token.starts_with("-D")
        || token.starts_with("-U")
        || token.starts_with("-std=")
}

fn is_inline_drop_flag(token: &str) -> bool {
    token.starts_with("-o")
        || token.starts_with("-MF")
        || token.starts_with("-MT")
        || token.starts_with("-MQ")
        || token.starts_with("-MJ")
}

fn parse_line_marker(line: &str) -> Option<(usize, String)> {
    let rest = line.trim_start().strip_prefix('#')?.trim_start();
    let rest = rest.strip_prefix("line").map(str::trim_start).unwrap_or(rest);
    let digit_count = rest.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digit_count == 0 {
        return None;
    }

    let original_line = rest[..digit_count].parse().ok()?;
    let rest = rest[digit_count..].trim_start();
    let quoted = rest.strip_prefix('"')?;
    let end_quote = quoted.find('"')?;
    let original_file = quoted[..end_quote].to_string();
    Some((original_line, original_file))
}
