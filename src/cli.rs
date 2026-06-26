use crate::analysis::{compute_def_use_edges, compute_var_dependencies};
use crate::cbuild::{configure_cmake_projects, discover_cmake_projects, merge_compile_commands};
use crate::ccompile::{build_preprocess_arguments, load_preprocessed_unit};
use crate::config::AnalyzeConfig;
use crate::csymbols::resolve_c_symbols;
use crate::fs::discover_sources;
use crate::imports::resolve_imports;
use crate::ir::AnalysisCache;
use crate::lang::LanguageFrontend;
use crate::lang::c::CFrontend;
use crate::lang::python::PythonFrontend;
use crate::paths::{PathQueryOptions, query_function_paths};
use crate::report::write_report;
use crate::source::SourceUnit;
use crate::summaries::{build_initial_summaries, propagate_call_summaries};
use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
#[command(
    name = "data-flow-analyzer",
    version,
    about = "Static def-use and dependency analyzer"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    Analyze {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        lang: Option<String>,
        #[arg(long)]
        input: Option<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        build_root: Option<PathBuf>,
        #[arg(long = "cmake-arg")]
        cmake_args: Vec<String>,
        #[arg(long, default_value_t = false)]
        keep_preprocessed: bool,
    },
    Paths {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        function: String,
        #[arg(long, default_value_t = 2)]
        max_loop_unroll: usize,
    },
}

fn print_default_help() -> Result<()> {
    let mut command = Cli::command();

    if let Some(bin_name) = std::env::args_os()
        .next()
        .as_deref()
        .and_then(|arg0| Path::new(arg0).file_name())
    {
        command = command.bin_name(bin_name.to_string_lossy().into_owned());
    }

    command.print_help()?;
    println!();
    Ok(())
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Analyze {
            config,
            lang,
            input,
            out,
            build_root,
            cmake_args,
            keep_preprocessed,
        }) => run_analyze(
            config,
            lang,
            input,
            out,
            build_root,
            cmake_args,
            keep_preprocessed,
        ),
        Some(Commands::Paths {
            input,
            function,
            max_loop_unroll,
        }) => run_paths(input, function, max_loop_unroll),
        None => print_default_help(),
    }
}

fn run_analyze(
    config: Option<PathBuf>,
    lang: Option<String>,
    input: Option<PathBuf>,
    out: Option<PathBuf>,
    build_root: Option<PathBuf>,
    cmake_args: Vec<String>,
    keep_preprocessed: bool,
) -> Result<()> {
    let mut cfg = if let Some(config_path) = config {
        AnalyzeConfig::from_toml_file(&config_path)?
    } else {
        AnalyzeConfig::default()
    };
    cfg.apply_cli_overrides(lang, input, out, build_root, cmake_args, keep_preprocessed);

    fs::create_dir_all(cfg.out.join("data"))
        .with_context(|| format!("failed to create output directory {}", cfg.out.display()))?;

    let mut cache = match cfg.lang.as_str() {
        "python" => analyze_python(&cfg)?,
        "c" => analyze_c(&cfg)?,
        other => bail!(
            "unsupported language '{}'; supported languages are 'python' and 'c'",
            other
        ),
    };

    compute_def_use_edges(&mut cache);
    compute_var_dependencies(&mut cache);
    build_initial_summaries(&mut cache);
    propagate_call_summaries(&mut cache);
    write_report(&cache, &cfg.out, cfg.top_n)?;
    println!("report written to {}", cfg.out.display());
    Ok(())
}

fn analyze_python(cfg: &AnalyzeConfig) -> Result<AnalysisCache> {
    let files = discover_sources(cfg)?;
    let frontend = PythonFrontend::new();
    let mut cache = frontend.parse_files(&files)?;
    resolve_imports(&mut cache);
    Ok(cache)
}

fn analyze_c(cfg: &AnalyzeConfig) -> Result<AnalysisCache> {
    let projects = discover_cmake_projects(cfg)?;
    if projects.is_empty() {
        bail!(
            "no CMake projects found under {}",
            cfg.input.display()
        );
    }
    let configured = configure_cmake_projects(&projects, cfg)?;
    let merged_path = cfg.out.join("data/compile_commands.merged.json");
    let preprocessed_dir = cfg.out.join("data/c-preprocessed");
    fs::create_dir_all(&preprocessed_dir).with_context(|| {
        format!(
            "failed to create preprocessed cache directory {}",
            preprocessed_dir.display()
        )
    })?;

    let compile_commands = merge_compile_commands(
        &configured
            .iter()
            .map(|project| project.compile_commands_path.clone())
            .collect::<Vec<_>>(),
        &merged_path,
    )?;

    let units = compile_commands
        .iter()
        .enumerate()
        .filter_map(|(index, command)| match preprocess_compile_command(index, command, &preprocessed_dir) {
            Ok(unit) => Some(unit),
            Err(err) => {
                eprintln!(
                    "warning: failed to preprocess {}: {err:#}",
                    command.file.display()
                );
                None
            }
        })
        .collect::<Vec<_>>();

    if units.is_empty() {
        bail!("no C translation units could be preprocessed for analysis");
    }

    let mut cache = CFrontend::new().parse_units(&units)?;
    resolve_c_symbols(&mut cache);

    if !cfg.keep_preprocessed {
        // Best-effort cleanup of the per-unit preprocessed outputs.
        let _ = fs::remove_dir_all(&preprocessed_dir);
    }
    Ok(cache)
}

fn preprocess_compile_command(
    index: usize,
    command: &crate::cbuild::CompileCommand,
    preprocessed_dir: &Path,
) -> Result<SourceUnit> {
    let source_name = command
        .file
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| format!("unit-{index}"));
    let stem = Path::new(&source_name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| source_name.clone());
    let preprocessed = preprocessed_dir.join(format!("{stem}-{index}.i"));

    let args = build_preprocess_arguments(command, &preprocessed)?;
    let mut cmd = std::process::Command::new(&args[0]);
    cmd.args(&args[1..]).current_dir(&command.directory);
    let output = cmd
        .output()
        .with_context(|| format!("failed to spawn preprocessor for {}", command.file.display()))?;
    if !output.status.success() {
        bail!(
            "preprocess failed for {}: {}",
            command.file.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    load_preprocessed_unit(&command.file, &preprocessed)
}

fn run_paths(input: PathBuf, function: String, max_loop_unroll: usize) -> Result<()> {
    let text = fs::read_to_string(&input)
        .with_context(|| format!("failed to read analysis cache {}", input.display()))?;
    let cache: crate::ir::AnalysisCache = serde_json::from_str(&text)
        .with_context(|| format!("failed to parse analysis cache {}", input.display()))?;
    let function_id = cache
        .functions
        .iter()
        .find(|record| record.function_id == function || record.qualified_name == function)
        .map(|record| record.function_id.clone())
        .ok_or_else(|| anyhow::anyhow!("function '{}' not found in cache", function))?;

    let defaults = AnalyzeConfig::default();
    let result = query_function_paths(
        &cache,
        &function_id,
        None,
        None,
        PathQueryOptions {
            max_loop_unroll,
            max_paths: defaults.max_paths,
            max_path_len: defaults.max_path_len,
        },
    );
    let output_path = input
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("path-query.json");
    let json = serde_json::to_string_pretty(&result)?;
    fs::write(&output_path, json)
        .with_context(|| format!("failed to write {}", output_path.display()))?;
    println!("path query written to {}", output_path.display());
    Ok(())
}
