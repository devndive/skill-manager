use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tempfile::TempDir;
use thiserror::Error;

static CANCELLATION_REQUESTED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone)]
pub struct DiscoverRequest {
    source: DiscoverSource,
    revision: Option<String>,
}

#[derive(Debug, Clone)]
pub enum DiscoverSource {
    Local(PathBuf),
    PublicGitHubUrl(String),
}

impl From<PathBuf> for DiscoverSource {
    fn from(source: PathBuf) -> Self {
        Self::Local(source)
    }
}

impl From<&Path> for DiscoverSource {
    fn from(source: &Path) -> Self {
        Self::Local(source.to_owned())
    }
}

impl From<&PathBuf> for DiscoverSource {
    fn from(source: &PathBuf) -> Self {
        Self::Local(source.to_owned())
    }
}

impl From<String> for DiscoverSource {
    fn from(source: String) -> Self {
        if looks_like_repository_url(&source) {
            Self::PublicGitHubUrl(source)
        } else {
            Self::Local(source.into())
        }
    }
}

impl From<&str> for DiscoverSource {
    fn from(source: &str) -> Self {
        source.to_owned().into()
    }
}

impl DiscoverRequest {
    pub fn new(source: impl Into<DiscoverSource>) -> Self {
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
    #[error("unsupported or malformed Source Repository URL '{url}': {details}")]
    InvalidSourceUrl { url: String, details: String },
    #[error(
        "could not create a temporary repository for Source Repository '{repository}': {source}"
    )]
    TemporaryRepository {
        repository: String,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "could not remove temporary repository data for Source Repository '{repository}': {source}"
    )]
    TemporaryCleanup {
        repository: String,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "{discovery}; additionally, temporary repository data for Source Repository '{repository}' could not be removed: {cleanup}"
    )]
    CleanupAfterFailure {
        repository: String,
        discovery: Box<DiscoverError>,
        cleanup: std::io::Error,
    },
    #[error("discovery was cancelled")]
    Cancelled,
}

pub fn install_cancellation_handler() -> Result<(), ctrlc::Error> {
    CANCELLATION_REQUESTED.store(false, Ordering::SeqCst);
    ctrlc::set_handler(|| {
        CANCELLATION_REQUESTED.store(true, Ordering::SeqCst);
    })
}

pub fn discover(request: DiscoverRequest) -> Result<Discovery, DiscoverError> {
    if cancellation_requested() {
        return Err(DiscoverError::Cancelled);
    }
    let requested_revision = request.revision.as_deref().unwrap_or("HEAD").to_owned();
    match request.source {
        DiscoverSource::Local(source) => discover_local(source, requested_revision),
        DiscoverSource::PublicGitHubUrl(source) => discover_github(source, requested_revision),
    }
}

fn discover_local(
    source_path: PathBuf,
    requested_revision: String,
) -> Result<Discovery, DiscoverError> {
    let source = display_path(&source_path);
    let repository_root =
        git_text(&source_path, ["rev-parse", "--show-toplevel"]).map_err(|error| match error {
            GitFailure::Unavailable(error) => DiscoverError::GitUnavailable(error),
            GitFailure::Failed(details) => DiscoverError::RepositoryUnavailable {
                repository: source.clone(),
                details,
            },
            GitFailure::InvalidUtf8 => DiscoverError::InvalidGitOutput {
                operation: "locating the Source Repository",
            },
            GitFailure::Cancelled => DiscoverError::Cancelled,
        })?;
    let repository_root = PathBuf::from(repository_root);
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
        })?
        .to_owned();

    let context = RepositoryContext {
        repository_type: "local",
        identity: repository_path,
        name: repository_name,
        resolve_remote_branch: false,
    };
    discover_repository(&repository_root, context, requested_revision)
}

fn discover_github(
    source_url: String,
    requested_revision: String,
) -> Result<Discovery, DiscoverError> {
    let github = normalize_github_url(&source_url)?;
    let temporary = TempDir::new().map_err(|source| DiscoverError::TemporaryRepository {
        repository: github.identity.clone(),
        source,
    })?;
    let repository_root = temporary.path().join("repository.git");
    let result = Command::new("git")
        .args(["clone", "--filter=blob:none", "--no-checkout", "--quiet"])
        .arg(&github.identity)
        .arg(&repository_root)
        .output()
        .map_err(DiscoverError::GitUnavailable)
        .and_then(|output| {
            if cancellation_requested() {
                Err(DiscoverError::Cancelled)
            } else if output.status.success() {
                Ok(())
            } else {
                Err(DiscoverError::RepositoryUnavailable {
                    repository: github.identity.clone(),
                    details: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
                })
            }
        })
        .and_then(|()| {
            let context = RepositoryContext {
                repository_type: "github",
                identity: github.identity.clone(),
                name: github.repository_name,
                resolve_remote_branch: true,
            };
            discover_repository(&repository_root, context, requested_revision)
        })
        .and_then(|discovery| {
            if cancellation_requested() {
                Err(DiscoverError::Cancelled)
            } else {
                Ok(discovery)
            }
        });
    let cleanup = temporary.close();
    match (result, cleanup) {
        (Ok(discovery), Ok(())) => Ok(discovery),
        (Err(error), Ok(())) => Err(error),
        (Err(discovery), Err(cleanup)) => Err(DiscoverError::CleanupAfterFailure {
            repository: github.identity,
            discovery: Box::new(discovery),
            cleanup,
        }),
        (Ok(_), Err(source)) => Err(DiscoverError::TemporaryCleanup {
            repository: github.identity,
            source,
        }),
    }
}

fn discover_repository(
    repository_root: &Path,
    context: RepositoryContext,
    requested_revision: String,
) -> Result<Discovery, DiscoverError> {
    let resolved_commit = resolve_commit(
        repository_root,
        &requested_revision,
        context.resolve_remote_branch,
    )
    .map_err(|error| match error {
        GitFailure::Unavailable(error) => DiscoverError::GitUnavailable(error),
        GitFailure::Failed(details) => DiscoverError::RevisionUnavailable {
            repository: context.identity.clone(),
            revision: requested_revision.clone(),
            details,
        },
        GitFailure::InvalidUtf8 => DiscoverError::InvalidGitOutput {
            operation: "resolving the requested revision",
        },
        GitFailure::Cancelled => DiscoverError::Cancelled,
    })?;

    let tree = git_bytes(
        repository_root,
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
            repository: context.identity.clone(),
            revision: requested_revision.clone(),
            details,
        },
        GitFailure::InvalidUtf8 => unreachable!("raw Git output does not decode text"),
        GitFailure::Cancelled => DiscoverError::Cancelled,
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
                    .map_err(|_| DiscoverError::InvalidSkillPath(context.identity.clone()))?
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
        return Err(DiscoverError::NoSkills(context.identity));
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
                context.name.clone()
            } else {
                path.rsplit('/').next().unwrap_or(path).to_owned()
            };

            Skill {
                identity: SkillIdentity {
                    source: context.identity.clone(),
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
            repository_type: context.repository_type,
            path: context.identity,
        },
        requested_revision,
        resolved_commit,
        skills,
    })
}

struct RepositoryContext {
    repository_type: &'static str,
    identity: String,
    name: String,
    resolve_remote_branch: bool,
}

fn resolve_commit(
    repository: &Path,
    requested_revision: &str,
    resolve_remote_branch: bool,
) -> Result<String, GitFailure> {
    let revision_expression = format!("{requested_revision}^{{commit}}");
    match git_text(
        repository,
        [
            "rev-parse",
            "--verify",
            "--end-of-options",
            revision_expression.as_str(),
        ],
    ) {
        Err(GitFailure::Failed(_)) if resolve_remote_branch && requested_revision != "HEAD" => {
            let remote_expression = format!("refs/remotes/origin/{requested_revision}^{{commit}}");
            git_text(
                repository,
                [
                    "rev-parse",
                    "--verify",
                    "--end-of-options",
                    remote_expression.as_str(),
                ],
            )
        }
        result => result,
    }
}

struct GitHubRepository {
    identity: String,
    repository_name: String,
}

fn normalize_github_url(source: &str) -> Result<GitHubRepository, DiscoverError> {
    let Some((scheme, remainder)) = source.split_once("://") else {
        return Err(invalid_source_url(
            source,
            "expected an https:// GitHub URL",
        ));
    };
    if !scheme.eq_ignore_ascii_case("https") {
        return Err(invalid_source_url(
            source,
            "only public https://github.com URLs are supported",
        ));
    }
    let Some((host, path)) = remainder.split_once('/') else {
        return Err(invalid_source_url(
            source,
            "expected an owner and repository name",
        ));
    };
    if !host.eq_ignore_ascii_case("github.com") {
        return Err(invalid_source_url(
            source,
            "only public github.com repositories are supported",
        ));
    }
    if path.contains(['?', '#']) {
        return Err(invalid_source_url(
            source,
            "query strings and fragments are not supported",
        ));
    }

    let path = path.trim_end_matches('/');
    let path = path
        .strip_suffix(".git")
        .or_else(|| path.strip_suffix(".GIT"))
        .unwrap_or(path);
    let mut segments = path.split('/');
    let owner = segments.next().unwrap_or_default();
    let repository = segments.next().unwrap_or_default();
    if owner.is_empty() || repository.is_empty() || segments.next().is_some() {
        return Err(invalid_source_url(
            source,
            "expected exactly one owner and repository name",
        ));
    }
    if !is_github_name(owner, false) || !is_github_name(repository, true) {
        return Err(invalid_source_url(
            source,
            "owner or repository name contains unsupported characters",
        ));
    }

    let owner = owner.to_ascii_lowercase();
    let repository_name = repository.to_ascii_lowercase();
    Ok(GitHubRepository {
        identity: format!("https://github.com/{owner}/{repository_name}"),
        repository_name,
    })
}

fn is_github_name(value: &str, allow_dot_and_underscore: bool) -> bool {
    value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric()
            || byte == b'-'
            || (allow_dot_and_underscore && matches!(byte, b'.' | b'_'))
    })
}

fn invalid_source_url(source: &str, details: &str) -> DiscoverError {
    DiscoverError::InvalidSourceUrl {
        url: source.to_owned(),
        details: details.to_owned(),
    }
}

fn looks_like_repository_url(source: &str) -> bool {
    source.contains("://")
        || source.starts_with("git@")
        || ["http:", "https:", "ssh:", "git:"].iter().any(|scheme| {
            source
                .get(..scheme.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme))
        })
}

enum GitFailure {
    Unavailable(std::io::Error),
    Failed(String),
    InvalidUtf8,
    Cancelled,
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
    if cancellation_requested() {
        return Err(GitFailure::Cancelled);
    }
    if !output.status.success() {
        return Err(GitFailure::Failed(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }

    Ok(output.stdout)
}

fn cancellation_requested() -> bool {
    CANCELLATION_REQUESTED.load(Ordering::SeqCst)
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
