use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
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
        lang: Option<String>,
        #[arg(long)]
        input: Option<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
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
        Some(Commands::Analyze { .. }) => {
            println!("analyze command is available");
            Ok(())
        }
        Some(Commands::Paths { .. }) => {
            println!("paths command is available");
            Ok(())
        }
        None => print_default_help(),
    }
}
