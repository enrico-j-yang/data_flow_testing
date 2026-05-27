use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

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
        None => {
            Cli::parse_from(["data-flow-analyzer", "--help"]);
            Ok(())
        }
    }
}
