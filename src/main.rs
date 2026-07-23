use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use dialoguer::{Confirm, MultiSelect};
use skill_manager::{
    DiscoverRequest, Discovery, InteractiveSelectionPrompt, SelectRequest, SkillSelection,
    SkillSelectionList, SyncRequest, SynchronizationResult, discover, install_cancellation_handler,
    list_selections, prepare_source_removal, select, select_interactively, sync,
};

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
    /// Persist a Skill Selection without installing files.
    Select {
        /// Local path or public GitHub URL for the Source Repository.
        source: String,
        /// Branch, tag, or commit to inspect instead of HEAD.
        #[arg(long = "ref", value_name = "REVISION", allow_hyphen_values = true)]
        reference: Option<String>,
        /// Select every discovered Skill.
        #[arg(long, conflicts_with = "selected_paths")]
        all: bool,
        /// Select a discovered Skill by exact repository-relative path.
        #[arg(long = "select", value_name = "PATH")]
        selected_paths: Vec<String>,
        /// Skill Selection manifest to update.
        #[arg(long, value_name = "FILE", default_value = "skills.toml")]
        manifest: PathBuf,
        /// Emit the versioned JSON schema.
        #[arg(long)]
        json: bool,
    },
    /// List persisted Skill Selections without accessing their Source Repositories.
    List {
        /// Skill Selection manifest to inspect.
        #[arg(long, value_name = "FILE", default_value = "skills.toml")]
        manifest: PathBuf,
        /// Emit the versioned JSON schema.
        #[arg(long)]
        json: bool,
    },
    /// Remove a Source Repository from the Skill Selection manifest.
    Remove {
        /// Local path or public GitHub URL identifying the Source Repository.
        source: String,
        /// Skill Selection manifest to update.
        #[arg(long, value_name = "FILE", default_value = "skills.toml")]
        manifest: PathBuf,
        /// Remove without interactive confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Materialize selected Skills at their recorded commits.
    Sync {
        /// Skill Selection manifest to synchronize.
        #[arg(long, value_name = "FILE", default_value = "skills.toml")]
        manifest: PathBuf,
        /// Synchronization Destination instead of the manifest-relative default.
        #[arg(long, value_name = "DIRECTORY")]
        target: Option<PathBuf>,
        /// Replace drifted managed Skills when reconciliation supports them.
        #[arg(long)]
        force: bool,
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
                print_discovery_human(&discovery);
            }
        }
        Commands::Select {
            source,
            reference,
            all,
            selected_paths,
            manifest,
            json,
        } => {
            let mut request = SelectRequest::new(source).with_manifest_path(manifest);
            if let Some(reference) = reference {
                request = request.with_revision(reference);
            }
            let interactive = !all && selected_paths.is_empty();
            if all {
                request = request.select_all();
            } else {
                for path in selected_paths {
                    request = request.select_path(path);
                }
            }
            let selection = if interactive {
                if !io::stdin().is_terminal() {
                    return Err(io::Error::other(
                        "interactive selection requires a terminal; use --all or --select PATH for non-interactive use",
                    )
                    .into());
                }
                select_interactively(request, prompt_for_selection)?
            } else {
                Some(select(request)?)
            };

            if let Some(selection) = selection {
                if json {
                    println!("{}", serde_json::to_string_pretty(&selection)?);
                } else {
                    print_selection_human(&selection);
                }
            } else {
                println!("Skill Selection unchanged.");
            }
        }
        Commands::List { manifest, json } => {
            let selections = list_selections(manifest)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&selections)?);
            } else {
                print_list_human(&selections);
            }
        }
        Commands::Remove {
            source,
            manifest,
            yes,
        } => {
            let removal = prepare_source_removal(source, manifest)?;
            let confirmed = yes || confirm_source_removal(&removal.source().path)?;
            if confirmed {
                let removed = removal.confirm()?;
                println!("Removed Source Repository: {}", removed.source.path);
                println!("Manifest: {}", removed.manifest_path);
            } else {
                println!("Skill Selection unchanged.");
            }
        }
        Commands::Sync {
            manifest,
            target,
            force,
            json,
        } => {
            let mut request = SyncRequest::new(manifest);
            if let Some(target) = target {
                request = request.with_destination(target);
            }
            if force {
                request = request.with_force();
            }
            let result = sync(request)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                print_synchronization_human(&result);
            }
        }
    }

    Ok(())
}

fn confirm_source_removal(source: &str) -> io::Result<bool> {
    eprint!("Remove Source Repository '{source}' from the Skill Selection manifest? [y/N] ");
    io::stderr().flush()?;
    let mut response = String::new();
    io::stdin().read_line(&mut response)?;
    match response.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => Ok(true),
        "" | "n" | "no" => Ok(false),
        response => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("expected 'y' or 'n', received '{response}'"),
        )),
    }
}

fn prompt_for_selection(
    prompt: &InteractiveSelectionPrompt,
) -> Result<Option<Vec<String>>, dialoguer::Error> {
    if !prompt.missing_paths.is_empty() {
        eprintln!("Previously selected Skills missing from the resolved revision:");
        for path in &prompt.missing_paths {
            eprintln!("- {path} (missing)");
        }
        eprintln!("These paths will be removed only if you confirm the replacement.");
    }

    let labels = prompt
        .options
        .iter()
        .map(|option| option.terminal_label())
        .collect::<Vec<_>>();
    let defaults = prompt
        .options
        .iter()
        .map(|option| option.preselected)
        .collect::<Vec<_>>();
    let selected_paths = if labels.is_empty() {
        Vec::new()
    } else {
        MultiSelect::new()
            .with_prompt("Select Skills")
            .items(&labels)
            .defaults(&defaults)
            .interact()?
            .into_iter()
            .map(|index| prompt.options[index].path.clone())
            .collect::<Vec<_>>()
    };

    let overlaps = prompt.overlaps(&selected_paths);
    if !overlaps.is_empty() {
        eprintln!("warning: overlapping parent and Nested Skills are selected:");
        for overlap in overlaps {
            eprintln!("- {} contains {}", overlap.parent_path, overlap.nested_path);
        }
        eprintln!("Selecting both is allowed, but their contents may overlap.");
    }

    let confirmation = if selected_paths.is_empty() {
        "Confirm the empty selection and remove this Source Repository?"
    } else {
        "Replace this Source Repository's Skill Selection?"
    };
    Ok(Confirm::new()
        .with_prompt(confirmation)
        .default(false)
        .interact()?
        .then_some(selected_paths))
}

fn print_discovery_human(discovery: &Discovery) {
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

fn print_selection_human(skill_selection: &SkillSelection) {
    println!("Manifest: {}", skill_selection.manifest_path);
    println!("Source Repository: {}", skill_selection.source.path);
    println!("Requested revision: {}", skill_selection.requested_revision);
    println!("Resolved commit: {}", skill_selection.resolved_commit);
    if skill_selection.skills.is_empty() {
        println!("Skill Selection: none");
        return;
    }

    println!("Skill Selection:");
    for skill in &skill_selection.skills {
        println!("- {} ({})", skill.name, skill.path);
    }
}

fn print_list_human(selections: &SkillSelectionList) {
    println!("Manifest: {}", selections.manifest_path);
    for (index, source) in selections.sources.iter().enumerate() {
        if index > 0 {
            println!();
        }
        println!("Source Repository: {}", source.source.path);
        println!("Requested revision: {}", source.requested_revision);
        println!("Resolved commit: {}", source.resolved_commit);
        println!("Skills:");
        for skill in &source.skills {
            println!("- {} ({})", skill.name, skill.path);
        }
    }
}

fn print_synchronization_human(result: &SynchronizationResult) {
    println!("Manifest: {}", result.manifest_path);
    println!("Synchronization Destination: {}", result.destination);
    if result.created.is_empty() {
        println!("Created Materialized Skills: none");
        return;
    }

    println!("Created Materialized Skills:");
    for skill in &result.created {
        println!(
            "- {} ({}:{}; commit: {})",
            skill.name, skill.identity.source, skill.identity.path, skill.resolved_commit
        );
    }
}
