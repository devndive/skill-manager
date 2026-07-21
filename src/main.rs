use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use skill_manager::{DiscoverRequest, Discovery, discover};

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Discover Skills in a local Source Repository.
    Discover {
        /// Path to the local Source Repository.
        local_source: PathBuf,
        /// Branch, tag, or commit to inspect instead of HEAD.
        #[arg(long = "ref", value_name = "REVISION")]
        reference: Option<String>,
        /// Emit the versioned JSON schema.
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Commands::Discover {
            local_source,
            reference,
            json,
        } => {
            let mut request = DiscoverRequest::new(local_source);
            if let Some(reference) = reference {
                request = request.with_revision(reference);
            }
            let discovery = discover(request)?;

            if json {
                println!("{}", serde_json::to_string_pretty(&discovery)?);
            } else {
                print_human(&discovery);
            }
        }
    }

    Ok(())
}

fn print_human(discovery: &Discovery) {
    println!("Source Repository: {}", discovery.source.path);
    println!("Requested revision: {}", discovery.requested_revision);
    println!("Resolved commit: {}", discovery.resolved_commit);
    println!("Skills:");
    for skill in &discovery.skills {
        if let Some(parent_path) = &skill.parent_path {
            println!("- {} ({}; parent: {parent_path})", skill.name, skill.path);
        } else {
            println!("- {} ({})", skill.name, skill.path);
        }
    }
}
