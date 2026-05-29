mod collect;
mod difft_json;
mod publish;
mod render;
mod verify;

use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "walkthrough")]
#[command(about = "Generate narrative walkthroughs of code changes with difftastic diffs")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Collect difft JSON for all changed files
    Collect {
        /// Output directory for JSON files
        #[arg(short, long, default_value = ".walkthrough_data")]
        output: PathBuf,

        /// Include whitespace-only changes (default: ignore them, i.e. pass -w to git diff)
        #[arg(long)]
        include_whitespace: bool,

        /// Arguments to pass to git diff (put after --)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        diff_args: Vec<String>,
    },

    /// Verify all chunks are referenced in a walkthrough
    Verify {
        /// Path to the walkthrough markdown file
        walkthrough: PathBuf,

        /// Directory containing difft JSON files
        #[arg(long, default_value = ".walkthrough_data")]
        data_dir: PathBuf,
    },

    /// Render walkthrough markdown to HTML with side-by-side diffs
    Render {
        /// Path to the walkthrough markdown file
        walkthrough: PathBuf,

        /// Directory containing difft JSON files
        #[arg(long, default_value = ".walkthrough_data")]
        data_dir: PathBuf,

        /// Output HTML file path
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Render without diff data (pure markdown mode)
        #[arg(long)]
        no_diff_data: bool,

        /// Render even if the collected data is stale (HEAD has moved since collect)
        #[arg(long)]
        allow_stale: bool,

        /// On stale data, automatically re-run collect (using the stored diff args) before rendering
        #[arg(long)]
        recollect: bool,
    },

    /// Publish walkthrough HTML to $WALKTHROUGH_PUBLISH_PATH
    Publish {
        /// HTML file to publish
        html: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Collect { output, include_whitespace, diff_args } => {
            collect::run(&diff_args, &output, !include_whitespace)
        }

        Commands::Verify {
            walkthrough,
            data_dir,
        } => verify::run(&walkthrough, &data_dir).map(|complete| {
            if !complete {
                process::exit(1);
            }
        }),

        Commands::Render {
            walkthrough,
            data_dir,
            output,
            no_diff_data,
            allow_stale,
            recollect,
        } => {
            let output_path = output.unwrap_or_else(|| walkthrough.with_extension("html"));
            render::run(&walkthrough, &data_dir, &output_path, no_diff_data, allow_stale, recollect)
        }

        Commands::Publish { html } => publish::run(&html),
    };

    if let Err(e) = result {
        eprintln!("Error: {:#}", e);
        process::exit(1);
    }
}
