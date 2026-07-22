use std::process::ExitCode;

use clap::{Parser, Subcommand};
use skill_manager::{DiscoverRequest, Discovery, discover, install_cancellation_handler};

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Discover Skills in a local or public GitHub Source Repository.
    Discover {
        /// Local path or public GitHub URL for the Source Repository.
        source: String,
        /// Branch, tag, or commit to inspect instead of HEAD.
        #[arg(long = "ref", value_name = "REVISION", allow_hyphen_values = true)]
        reference: Option<String>,
        /// Emit the versioned JSON schema.
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    if let Err(error) = install_cancellation_handler() {
        eprintln!("error: could not install cancellation handler: {error}");
        return ExitCode::FAILURE;
    }

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
            source,
            reference,
            json,
        } => {
            let mut request = DiscoverRequest::new(source);
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
