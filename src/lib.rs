use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct DiscoverRequest {
    source: PathBuf,
    revision: Option<String>,
}

impl DiscoverRequest {
    pub fn new(source: impl Into<PathBuf>) -> Self {
        Self {
            source: source.into(),
            revision: None,
        }
    }

    pub fn with_revision(mut self, revision: impl Into<String>) -> Self {
        self.revision = Some(revision.into());
        self
    }
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Discovery {
    pub schema_version: u32,
    pub source: SourceRepository,
    pub requested_revision: String,
    pub resolved_commit: String,
    pub skills: Vec<Skill>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct SourceRepository {
    #[serde(rename = "type")]
    pub repository_type: &'static str,
    pub path: String,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Skill {
    pub identity: SkillIdentity,
    pub name: String,
    pub path: String,
    pub parent_path: Option<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct SkillIdentity {
    pub source: String,
    pub path: String,
}

#[derive(Debug, Error)]
pub enum DiscoverError {
    #[error("Git is required but could not be executed: {0}")]
    GitUnavailable(#[source] std::io::Error),
    #[error(
        "Source Repository '{repository}' is unavailable or is not a Git repository: {details}"
    )]
    RepositoryUnavailable { repository: String, details: String },
    #[error("revision '{revision}' is unavailable in Source Repository '{repository}': {details}")]
    RevisionUnavailable {
        repository: String,
        revision: String,
        details: String,
    },
    #[error("Git returned invalid UTF-8 while {operation}")]
    InvalidGitOutput { operation: &'static str },
    #[error("a tracked Skill path in Source Repository '{0}' is not valid UTF-8")]
    InvalidSkillPath(String),
    #[error("Source Repository path '{0}' is not valid UTF-8")]
    InvalidRepositoryPath(String),
    #[error("Source Repository '{0}' has no tracked Skills at the requested revision")]
    NoSkills(String),
}

pub fn discover(request: DiscoverRequest) -> Result<Discovery, DiscoverError> {
    let requested_revision = request.revision.as_deref().unwrap_or("HEAD").to_owned();
    let source = display_path(&request.source);
    let repository_root = git_text(&request.source, ["rev-parse", "--show-toplevel"]).map_err(
        |error| match error {
            GitFailure::Unavailable(error) => DiscoverError::GitUnavailable(error),
            GitFailure::Failed(details) => DiscoverError::RepositoryUnavailable {
                repository: source.clone(),
                details,
            },
            GitFailure::InvalidUtf8 => DiscoverError::InvalidGitOutput {
                operation: "locating the Source Repository",
            },
        },
    )?;
    let repository_root = PathBuf::from(repository_root);
    let revision_expression = format!("{requested_revision}^{{commit}}");
    let resolved_commit = git_text(
        &repository_root,
        [
            "rev-parse",
            "--verify",
            "--end-of-options",
            revision_expression.as_str(),
        ],
    )
    .map_err(|error| match error {
        GitFailure::Unavailable(error) => DiscoverError::GitUnavailable(error),
        GitFailure::Failed(details) => DiscoverError::RevisionUnavailable {
            repository: source.clone(),
            revision: requested_revision.clone(),
            details,
        },
        GitFailure::InvalidUtf8 => DiscoverError::InvalidGitOutput {
            operation: "resolving the requested revision",
        },
    })?;

    let tree = git_bytes(
        &repository_root,
        [
            "ls-tree",
            "-r",
            "-z",
            "--full-tree",
            resolved_commit.as_str(),
        ],
    )
    .map_err(|error| match error {
        GitFailure::Unavailable(error) => DiscoverError::GitUnavailable(error),
        GitFailure::Failed(details) => DiscoverError::RevisionUnavailable {
            repository: source.clone(),
            revision: requested_revision.clone(),
            details,
        },
        GitFailure::InvalidUtf8 => unreachable!("raw Git output does not decode text"),
    })?;

    let repository_path = repository_root
        .to_str()
        .ok_or_else(|| {
            DiscoverError::InvalidRepositoryPath(display_path(repository_root.as_path()))
        })?
        .to_owned();
    let repository_name = repository_root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            DiscoverError::InvalidRepositoryPath(display_path(repository_root.as_path()))
        })?;
    let mut skill_paths = Vec::new();
    for entry in tree
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let Some(separator) = entry.iter().position(|byte| *byte == b'\t') else {
            continue;
        };
        let metadata = std::str::from_utf8(&entry[..separator]).map_err(|_| {
            DiscoverError::InvalidGitOutput {
                operation: "reading repository tree metadata",
            }
        })?;
        let path = &entry[separator + 1..];
        let mut metadata = metadata.split_whitespace();
        let Some(mode) = metadata.next() else {
            continue;
        };
        let Some(object_type) = metadata.next() else {
            continue;
        };
        let is_regular_file = matches!(mode, "100644" | "100755") && object_type == "blob";
        if !is_regular_file {
            continue;
        }

        let skill_path = if path == b"SKILL.md" {
            Some(".".to_owned())
        } else if path.ends_with(b"/SKILL.md") {
            let directory = &path[..path.len() - b"/SKILL.md".len()];
            Some(
                std::str::from_utf8(directory)
                    .map_err(|_| DiscoverError::InvalidSkillPath(source.clone()))?
                    .to_owned(),
            )
        } else {
            None
        };
        if let Some(skill_path) = skill_path {
            skill_paths.push(skill_path);
        }
    }
    skill_paths.sort();

    if skill_paths.is_empty() {
        return Err(DiscoverError::NoSkills(source));
    }

    let skills = skill_paths
        .iter()
        .map(|path| {
            let parent_path = skill_paths
                .iter()
                .filter(|candidate| candidate.as_str() != path)
                .filter(|candidate| contains_path(candidate, path))
                .max_by_key(|candidate| candidate.len())
                .cloned();
            let name = if path == "." {
                repository_name.to_owned()
            } else {
                path.rsplit('/').next().unwrap_or(path).to_owned()
            };

            Skill {
                identity: SkillIdentity {
                    source: repository_path.clone(),
                    path: path.clone(),
                },
                name,
                path: path.clone(),
                parent_path,
            }
        })
        .collect();

    Ok(Discovery {
        schema_version: 1,
        source: SourceRepository {
            repository_type: "local",
            path: repository_path,
        },
        requested_revision,
        resolved_commit,
        skills,
    })
}

enum GitFailure {
    Unavailable(std::io::Error),
    Failed(String),
    InvalidUtf8,
}

fn git_text<const N: usize>(repository: &Path, arguments: [&str; N]) -> Result<String, GitFailure> {
    String::from_utf8(git_bytes(repository, arguments)?)
        .map(strip_line_ending)
        .map_err(|_| GitFailure::InvalidUtf8)
}

fn git_bytes<const N: usize>(
    repository: &Path,
    arguments: [&str; N],
) -> Result<Vec<u8>, GitFailure> {
    let output = run_git(repository, arguments).map_err(GitFailure::Unavailable)?;
    if !output.status.success() {
        return Err(GitFailure::Failed(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }

    Ok(output.stdout)
}

fn run_git<const N: usize>(repository: &Path, arguments: [&str; N]) -> std::io::Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .output()
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn contains_path(parent: &str, child: &str) -> bool {
    parent == "."
        || child
            .strip_prefix(parent)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn strip_line_ending(mut output: String) -> String {
    if output.ends_with('\n') {
        output.pop();
        if output.ends_with('\r') {
            output.pop();
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    use tempfile::TempDir;

    use super::{DiscoverRequest, discover};

    #[test]
    fn public_api_discovers_the_committed_tree() {
        let directory = TempDir::new().unwrap();
        let repository = directory.path().join("source-repository");
        fs::create_dir(&repository).unwrap();
        git(&repository, &["init", "--quiet"]);
        git(&repository, &["config", "user.email", "test@example.com"]);
        git(&repository, &["config", "user.name", "Test User"]);
        fs::write(repository.join("SKILL.md"), "# Root\n").unwrap();
        git(&repository, &["add", "."]);
        git(&repository, &["commit", "--quiet", "-m", "add root skill"]);
        fs::remove_file(repository.join("SKILL.md")).unwrap();

        let discovery = discover(DiscoverRequest::new(&repository)).unwrap();

        assert_eq!(discovery.skills[0].path, ".");
    }

    fn git(repository: &std::path::Path, arguments: &[&str]) {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(repository)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
