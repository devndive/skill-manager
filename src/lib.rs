use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use serde::Serialize;
use tempfile::{NamedTempFile, TempDir};
use thiserror::Error;
use toml_edit::{Array, ArrayOfTables, DocumentMut, Formatted, Item, Table, Value, value};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Discovery {
    pub schema_version: u32,
    pub source: SourceRepository,
    pub requested_revision: String,
    pub resolved_commit: String,
    pub skills: Vec<Skill>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceRepository {
    #[serde(rename = "type")]
    pub repository_type: &'static str,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Skill {
    pub identity: SkillIdentity,
    pub name: String,
    pub path: String,
    pub parent_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

#[derive(Debug, Clone)]
pub struct SelectRequest {
    discovery: DiscoverRequest,
    manifest_path: PathBuf,
    selection: RequestedSkillSelection,
}

#[derive(Debug, Clone)]
enum RequestedSkillSelection {
    Paths(Vec<String>),
    All,
}

impl SelectRequest {
    pub fn new(source: impl Into<DiscoverSource>) -> Self {
        Self {
            discovery: DiscoverRequest::new(source),
            manifest_path: PathBuf::from("skills.toml"),
            selection: RequestedSkillSelection::Paths(Vec::new()),
        }
    }

    pub fn with_revision(mut self, revision: impl Into<String>) -> Self {
        self.discovery = self.discovery.with_revision(revision);
        self
    }

    pub fn with_manifest_path(mut self, manifest_path: impl Into<PathBuf>) -> Self {
        self.manifest_path = manifest_path.into();
        self
    }

    pub fn select_all(mut self) -> Self {
        self.selection = RequestedSkillSelection::All;
        self
    }

    pub fn select_path(mut self, path: impl Into<String>) -> Self {
        let path = path.into();
        match &mut self.selection {
            RequestedSkillSelection::Paths(paths) => paths.push(path),
            RequestedSkillSelection::All => {
                self.selection = RequestedSkillSelection::Paths(vec![path]);
            }
        }
        self
    }
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct SkillSelection {
    pub schema_version: u32,
    pub manifest_path: String,
    pub source: SourceRepository,
    pub requested_revision: String,
    pub resolved_commit: String,
    pub skills: Vec<Skill>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillSelectionList {
    pub schema_version: u32,
    pub manifest_path: String,
    pub sources: Vec<ListedSourceRepository>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListedSourceRepository {
    pub source: SourceRepository,
    pub requested_revision: String,
    pub resolved_commit: String,
    pub skills: Vec<ListedSkill>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListedSkill {
    pub identity: SkillIdentity,
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedSourceRepository {
    pub manifest_path: String,
    pub source: SourceRepository,
}

#[derive(Debug)]
pub struct PreparedSourceRemoval {
    manifest_path: PathBuf,
    source: SourceRepository,
    document: DocumentMut,
}

impl PreparedSourceRemoval {
    pub fn source(&self) -> &SourceRepository {
        &self.source
    }

    pub fn confirm(mut self) -> Result<RemovedSourceRepository, SelectError> {
        let manifest_parent = self
            .manifest_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let canonical_parent =
            fs::canonicalize(manifest_parent).map_err(|source| SelectError::ManifestDirectory {
                path: display_path(manifest_parent),
                source,
            })?;
        let sources = self.document["sources"]
            .as_array_of_tables_mut()
            .expect("manifest sources were validated as an array of tables");
        for index in (0..sources.len()).rev() {
            if stored_source_matches(
                sources.get(index).expect("source index is in bounds"),
                &self.source,
                &canonical_parent,
            ) {
                sources.remove(index);
            }
        }

        ensure_selection_not_cancelled()?;
        write_manifest_atomic(&self.manifest_path, self.document.to_string().as_bytes())?;

        Ok(RemovedSourceRepository {
            manifest_path: display_path(&self.manifest_path),
            source: self.source,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractiveSkillOption {
    pub name: String,
    pub path: String,
    pub parent_path: Option<String>,
    pub nesting_depth: usize,
    pub preselected: bool,
}

impl InteractiveSkillOption {
    pub fn terminal_label(&self) -> String {
        let indentation = "  ".repeat(self.nesting_depth);
        if let Some(parent_path) = &self.parent_path {
            format!(
                "{indentation}{} ({}; parent: {parent_path})",
                self.name, self.path
            )
        } else {
            format!("{indentation}{} ({})", self.name, self.path)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractiveSelectionPrompt {
    pub options: Vec<InteractiveSkillOption>,
    pub missing_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillOverlap {
    pub parent_path: String,
    pub nested_path: String,
}

impl InteractiveSelectionPrompt {
    pub fn overlaps(&self, selected_paths: &[String]) -> Vec<SkillOverlap> {
        let selected_paths = selected_paths
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        self.options
            .iter()
            .filter(|parent| selected_paths.contains(parent.path.as_str()))
            .flat_map(|parent| {
                self.options
                    .iter()
                    .filter(|nested| selected_paths.contains(nested.path.as_str()))
                    .filter(|nested| parent.path != nested.path)
                    .filter(|nested| contains_path(&parent.path, &nested.path))
                    .map(|nested| SkillOverlap {
                        parent_path: parent.path.clone(),
                        nested_path: nested.path.clone(),
                    })
            })
            .collect()
    }
}

#[derive(Debug)]
pub struct PreparedInteractiveSelection {
    prompt: InteractiveSelectionPrompt,
    discovery: Discovery,
    manifest_path: PathBuf,
}

impl PreparedInteractiveSelection {
    pub fn prompt(&self) -> &InteractiveSelectionPrompt {
        &self.prompt
    }

    pub fn confirm(
        self,
        selected_paths: impl IntoIterator<Item = String>,
    ) -> Result<SkillSelection, SelectError> {
        let skills = select_skills(&self.discovery, selected_paths)?;
        persist_selection(self.discovery, self.manifest_path, skills)
    }
}

#[derive(Debug, Error)]
pub enum SelectError {
    #[error(transparent)]
    Discovery(#[from] DiscoverError),
    #[error(
        "Skill Selection contains paths not present in Source Repository '{repository}': {paths:?}"
    )]
    InvalidSelection {
        repository: String,
        paths: Vec<String>,
    },
    #[error("could not read Skill Selection manifest '{path}': {source}")]
    ManifestRead {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("could not parse Skill Selection manifest '{path}': {source}")]
    ManifestParse {
        path: String,
        #[source]
        source: toml_edit::TomlError,
    },
    #[error("Skill Selection manifest '{path}' is invalid: {details}")]
    InvalidManifest { path: String, details: String },
    #[error(
        "Skill Selection manifest '{path}' uses unsupported manifest version {version}; expected 1"
    )]
    UnsupportedManifestVersion { path: String, version: i64 },
    #[error("could not resolve Skill Selection manifest directory '{path}': {source}")]
    ManifestDirectory {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("could not write Skill Selection manifest '{path}': {source}")]
    ManifestWrite {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("invalid Source Repository '{input}': {details}")]
    InvalidRemovalSource { input: String, details: String },
    #[error("Source Repository '{input}' is not selected in manifest '{path}'")]
    SourceNotSelected { input: String, path: String },
}

#[derive(Debug, Error)]
pub enum InteractiveSelectError<E>
where
    E: std::error::Error + 'static,
{
    #[error(transparent)]
    Select(#[from] SelectError),
    #[error("interactive selection failed: {0}")]
    Interaction(#[source] E),
}

pub fn install_cancellation_handler() -> Result<(), ctrlc::Error> {
    CANCELLATION_REQUESTED.store(false, Ordering::SeqCst);
    ctrlc::set_handler(|| {
        CANCELLATION_REQUESTED.store(true, Ordering::SeqCst);
    })
}

pub fn discover(request: DiscoverRequest) -> Result<Discovery, DiscoverError> {
    discover_with_options(request, false)
}

fn discover_with_options(
    request: DiscoverRequest,
    allow_empty: bool,
) -> Result<Discovery, DiscoverError> {
    if cancellation_requested() {
        return Err(DiscoverError::Cancelled);
    }
    let requested_revision = request.revision.as_deref().unwrap_or("HEAD").to_owned();
    match request.source {
        DiscoverSource::Local(source) => discover_local(source, requested_revision, allow_empty),
        DiscoverSource::PublicGitHubUrl(source) => {
            discover_github(source, requested_revision, allow_empty)
        }
    }
}

pub fn select(request: SelectRequest) -> Result<SkillSelection, SelectError> {
    let allow_empty = matches!(&request.selection, RequestedSkillSelection::Paths(_));
    let discovery = discover_with_options(request.discovery, allow_empty)?;
    ensure_selection_not_cancelled()?;
    let skills = match request.selection {
        RequestedSkillSelection::All => discovery.skills.clone(),
        RequestedSkillSelection::Paths(paths) => select_skills(&discovery, paths)?,
    };

    persist_selection(discovery, request.manifest_path, skills)
}

pub fn list_selections(
    manifest_path: impl Into<PathBuf>,
) -> Result<SkillSelectionList, SelectError> {
    let manifest_path = manifest_path.into();
    let (canonical_parent, document) = load_manifest(&manifest_path, false)?;
    let sources = document["sources"]
        .as_array_of_tables()
        .expect("manifest sources were validated as an array of tables");
    let mut listed_sources = sources
        .iter()
        .map(|source| listed_source(source, &canonical_parent))
        .collect::<Vec<_>>();
    listed_sources.sort_by(|left, right| {
        left.source.path.cmp(&right.source.path).then_with(|| {
            left.source
                .repository_type
                .cmp(right.source.repository_type)
        })
    });

    Ok(SkillSelectionList {
        schema_version: 1,
        manifest_path: display_path(&manifest_path),
        sources: listed_sources,
    })
}

pub fn prepare_source_removal(
    source: impl Into<String>,
    manifest_path: impl Into<PathBuf>,
) -> Result<PreparedSourceRemoval, SelectError> {
    let source = source.into();
    let manifest_path = manifest_path.into();
    let (canonical_parent, document) = load_manifest(&manifest_path, false)?;
    let requested_source = removal_source_identity(&source)?;
    let sources = document["sources"]
        .as_array_of_tables()
        .expect("manifest sources were validated as an array of tables");
    if !sources
        .iter()
        .any(|entry| stored_source_matches(entry, &requested_source, &canonical_parent))
    {
        return Err(SelectError::SourceNotSelected {
            input: source,
            path: display_path(&manifest_path),
        });
    }

    Ok(PreparedSourceRemoval {
        manifest_path,
        source: requested_source,
        document,
    })
}

fn removal_source_identity(source: &str) -> Result<SourceRepository, SelectError> {
    if looks_like_repository_url(source) {
        let github =
            normalize_github_url(source).map_err(|error| SelectError::InvalidRemovalSource {
                input: source.to_owned(),
                details: error.to_string(),
            })?;
        return Ok(SourceRepository {
            repository_type: "github",
            path: github.identity,
        });
    }

    let current_directory =
        fs::canonicalize(".").map_err(|error| SelectError::InvalidRemovalSource {
            input: source.to_owned(),
            details: format!("could not resolve the current directory: {error}"),
        })?;
    Ok(SourceRepository {
        repository_type: "local",
        path: absolute_local_source_path(source, &current_directory),
    })
}

fn listed_source(source: &Table, manifest_parent: &Path) -> ListedSourceRepository {
    let repository_type = source["type"]
        .as_str()
        .expect("source type was validated as a string");
    let stored_path = source["path"]
        .as_str()
        .expect("source path was validated as a string");
    let source_path = if repository_type == "local" {
        absolute_local_source_path(stored_path, manifest_parent)
    } else {
        stored_path.to_owned()
    };
    let mut skills = source["skills"]
        .as_array()
        .expect("source skills were validated as an array")
        .iter()
        .map(|skill| {
            let path = skill
                .as_str()
                .expect("source skills were validated as strings")
                .to_owned();
            ListedSkill {
                identity: SkillIdentity {
                    source: source_path.clone(),
                    path: path.clone(),
                },
                name: skill_name(&source_path, &path),
                path,
            }
        })
        .collect::<Vec<_>>();
    skills.sort_by(|left, right| left.path.cmp(&right.path));
    skills.dedup_by(|left, right| left.path == right.path);

    ListedSourceRepository {
        source: SourceRepository {
            repository_type: match repository_type {
                "local" => "local",
                "github" => "github",
                _ => unreachable!("source type was validated"),
            },
            path: source_path,
        },
        requested_revision: source["requested_revision"]
            .as_str()
            .expect("requested revision was validated as a string")
            .to_owned(),
        resolved_commit: source["resolved_commit"]
            .as_str()
            .expect("resolved commit was validated as a string")
            .to_owned(),
        skills,
    }
}

fn skill_name(source_path: &str, skill_path: &str) -> String {
    if skill_path == "." {
        return Path::new(source_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(source_path)
            .to_owned();
    }

    skill_path
        .rsplit('/')
        .next()
        .unwrap_or(skill_path)
        .to_owned()
}

fn select_skills(
    discovery: &Discovery,
    selected_paths: impl IntoIterator<Item = String>,
) -> Result<Vec<Skill>, SelectError> {
    let paths = selected_paths.into_iter().collect::<BTreeSet<_>>();
    let discovered_paths = discovered_skill_paths(discovery);
    let missing = paths
        .iter()
        .filter(|path| !discovered_paths.contains(path.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(SelectError::InvalidSelection {
            repository: discovery.source.path.clone(),
            paths: missing,
        });
    }

    Ok(discovery
        .skills
        .iter()
        .filter(|skill| paths.contains(&skill.path))
        .cloned()
        .collect())
}

fn persist_selection(
    discovery: Discovery,
    manifest_path: PathBuf,
    skills: Vec<Skill>,
) -> Result<SkillSelection, SelectError> {
    let (canonical_parent, mut document) = load_manifest(&manifest_path, true)?;
    let sources = document["sources"]
        .as_array_of_tables_mut()
        .expect("manifest sources were validated as an array of tables");
    let matching_source_indices = sources
        .iter()
        .enumerate()
        .filter_map(|(index, source)| {
            source_matches(source, &discovery.source, &canonical_parent).then_some(index)
        })
        .collect::<Vec<_>>();
    if skills.is_empty() {
        for index in matching_source_indices.into_iter().rev() {
            sources.remove(index);
        }
    } else if let Some(&index) = matching_source_indices.first() {
        update_source_table(
            sources
                .get_mut(index)
                .expect("source index came from iterator"),
            &discovery,
            &skills,
            &canonical_parent,
        );
        for index in matching_source_indices.into_iter().skip(1).rev() {
            sources.remove(index);
        }
    } else {
        let mut source = Table::new();
        update_source_table(&mut source, &discovery, &skills, &canonical_parent);
        sources.push(source);
    }

    ensure_selection_not_cancelled()?;
    write_manifest_atomic(&manifest_path, document.to_string().as_bytes())?;

    Ok(SkillSelection {
        schema_version: 1,
        manifest_path: display_path(&manifest_path),
        source: discovery.source,
        requested_revision: discovery.requested_revision,
        resolved_commit: discovery.resolved_commit,
        skills,
    })
}

pub fn prepare_interactive_select(
    request: SelectRequest,
) -> Result<PreparedInteractiveSelection, SelectError> {
    let discovery = discover_with_options(request.discovery, true)?;
    let (canonical_parent, document) = load_manifest(&request.manifest_path, true)?;
    let sources = document["sources"]
        .as_array_of_tables()
        .expect("manifest sources were validated as an array of tables");
    let selected_paths = sources
        .iter()
        .filter(|source| source_matches(source, &discovery.source, &canonical_parent))
        .flat_map(|source| {
            source["skills"]
                .as_array()
                .expect("source skills were validated as an array")
                .iter()
                .map(|skill| {
                    skill
                        .as_str()
                        .expect("source skills were validated as strings")
                        .to_owned()
                })
        })
        .collect::<BTreeSet<_>>();
    let discovered_paths = discovered_skill_paths(&discovery);
    let options = discovery
        .skills
        .iter()
        .map(|skill| InteractiveSkillOption {
            name: skill.name.clone(),
            path: skill.path.clone(),
            parent_path: skill.parent_path.clone(),
            nesting_depth: discovery
                .skills
                .iter()
                .filter(|candidate| candidate.path != skill.path)
                .filter(|candidate| contains_path(&candidate.path, &skill.path))
                .count(),
            preselected: selected_paths.contains(&skill.path),
        })
        .collect();
    let missing_paths = selected_paths
        .into_iter()
        .filter(|path| !discovered_paths.contains(path.as_str()))
        .collect();

    Ok(PreparedInteractiveSelection {
        prompt: InteractiveSelectionPrompt {
            options,
            missing_paths,
        },
        discovery,
        manifest_path: request.manifest_path,
    })
}

fn discovered_skill_paths(discovery: &Discovery) -> BTreeSet<&str> {
    discovery
        .skills
        .iter()
        .map(|skill| skill.path.as_str())
        .collect()
}

pub fn select_interactively<E>(
    request: SelectRequest,
    interact: impl FnOnce(&InteractiveSelectionPrompt) -> Result<Option<Vec<String>>, E>,
) -> Result<Option<SkillSelection>, InteractiveSelectError<E>>
where
    E: std::error::Error + 'static,
{
    let prepared_selection = prepare_interactive_select(request)?;
    let selected_paths =
        interact(prepared_selection.prompt()).map_err(InteractiveSelectError::Interaction)?;
    selected_paths
        .map(|selected_paths| prepared_selection.confirm(selected_paths))
        .transpose()
        .map_err(InteractiveSelectError::Select)
}

fn load_manifest(
    manifest_path: &Path,
    create_if_missing: bool,
) -> Result<(PathBuf, DocumentMut), SelectError> {
    let manifest_parent = manifest_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let canonical_parent =
        fs::canonicalize(manifest_parent).map_err(|source| SelectError::ManifestDirectory {
            path: display_path(manifest_parent),
            source,
        })?;

    ensure_selection_not_cancelled()?;
    let manifest_display = display_path(manifest_path);
    let mut document = match fs::read_to_string(manifest_path) {
        Ok(contents) => {
            contents
                .parse::<DocumentMut>()
                .map_err(|source| SelectError::ManifestParse {
                    path: manifest_display.clone(),
                    source,
                })?
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound && create_if_missing => {
            let mut document = DocumentMut::new();
            document["manifest_version"] = value(1);
            document["sources"] = Item::ArrayOfTables(ArrayOfTables::new());
            document
        }
        Err(source) => {
            return Err(SelectError::ManifestRead {
                path: manifest_display,
                source,
            });
        }
    };
    match document.get("manifest_version").and_then(Item::as_integer) {
        Some(1) => {}
        Some(version) => {
            return Err(SelectError::UnsupportedManifestVersion {
                path: display_path(manifest_path),
                version,
            });
        }
        None => {
            return Err(SelectError::InvalidManifest {
                path: display_path(manifest_path),
                details: "missing integer 'manifest_version'".to_owned(),
            });
        }
    }
    if document.get("sources").is_none() {
        document["sources"] = Item::ArrayOfTables(ArrayOfTables::new());
    }
    let manifest_display = display_path(manifest_path);
    let sources = document["sources"]
        .as_array_of_tables_mut()
        .ok_or_else(|| SelectError::InvalidManifest {
            path: manifest_display.clone(),
            details: "'sources' must be an array of tables".to_owned(),
        })?;
    validate_sources(sources, &manifest_display)?;

    Ok((canonical_parent, document))
}

fn manifest_source_path(source: &SourceRepository, manifest_parent: &Path) -> String {
    if source.repository_type != "local" {
        return source.path.clone();
    }

    let source_path = Path::new(&source.path);
    pathdiff::diff_paths(source_path, manifest_parent)
        .unwrap_or_else(|| source_path.to_owned())
        .to_string_lossy()
        .into_owned()
}

fn absolute_local_source_path(stored_path: &str, manifest_parent: &Path) -> String {
    let stored_path = Path::new(stored_path);
    let absolute = if stored_path.is_absolute() {
        stored_path.to_owned()
    } else {
        manifest_parent.join(stored_path)
    };
    normalize_path_lexically(&absolute)
        .to_string_lossy()
        .into_owned()
}

fn normalize_path_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    normalized.pop();
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

fn source_matches(table: &Table, source: &SourceRepository, manifest_parent: &Path) -> bool {
    if table.get("type").and_then(Item::as_str) != Some(source.repository_type) {
        return false;
    }
    let Some(stored_path) = table.get("path").and_then(Item::as_str) else {
        return false;
    };
    if source.repository_type != "local" {
        return stored_path == source.path;
    }

    let stored_path = Path::new(stored_path);
    let stored_path = if stored_path.is_absolute() {
        stored_path.to_owned()
    } else {
        manifest_parent.join(stored_path)
    };
    fs::canonicalize(stored_path).is_ok_and(|path| path == Path::new(&source.path))
}

fn stored_source_matches(table: &Table, source: &SourceRepository, manifest_parent: &Path) -> bool {
    if table.get("type").and_then(Item::as_str) != Some(source.repository_type) {
        return false;
    }
    let Some(stored_path) = table.get("path").and_then(Item::as_str) else {
        return false;
    };
    if source.repository_type == "local" {
        absolute_local_source_path(stored_path, manifest_parent) == source.path
    } else {
        normalize_github_url(stored_path).is_ok_and(|github| github.identity == source.path)
    }
}

fn validate_sources(sources: &ArrayOfTables, manifest_path: &str) -> Result<(), SelectError> {
    for (index, source) in sources.iter().enumerate() {
        for field in ["type", "path", "requested_revision", "resolved_commit"] {
            if source.get(field).and_then(Item::as_str).is_none() {
                return Err(SelectError::InvalidManifest {
                    path: manifest_path.to_owned(),
                    details: format!(
                        "Source Repository entry {} is missing string '{field}'",
                        index + 1
                    ),
                });
            }
        }
        let repository_type = source["type"]
            .as_str()
            .expect("source type was validated as a string");
        if !matches!(repository_type, "local" | "github") {
            return Err(SelectError::InvalidManifest {
                path: manifest_path.to_owned(),
                details: format!(
                    "Source Repository entry {} has unsupported type '{repository_type}'",
                    index + 1
                ),
            });
        }
        let Some(skills) = source.get("skills").and_then(Item::as_array) else {
            return Err(SelectError::InvalidManifest {
                path: manifest_path.to_owned(),
                details: format!(
                    "Source Repository entry {} is missing array 'skills'",
                    index + 1
                ),
            });
        };
        if skills.iter().any(|skill| skill.as_str().is_none()) {
            return Err(SelectError::InvalidManifest {
                path: manifest_path.to_owned(),
                details: format!(
                    "Source Repository entry {} contains a non-string Skill path",
                    index + 1
                ),
            });
        }
    }
    Ok(())
}

fn update_source_table(
    table: &mut Table,
    discovery: &Discovery,
    skills: &[Skill],
    manifest_parent: &Path,
) {
    let selected_paths = updated_skill_paths(table, skills);
    set_table_value(table, "type", Value::from(discovery.source.repository_type));
    set_table_value(
        table,
        "path",
        Value::from(manifest_source_path(&discovery.source, manifest_parent)),
    );
    set_table_value(
        table,
        "requested_revision",
        Value::from(&discovery.requested_revision),
    );
    set_table_value(
        table,
        "resolved_commit",
        Value::from(&discovery.resolved_commit),
    );
    set_table_value(table, "skills", Value::Array(selected_paths));
}

fn updated_skill_paths(table: &Table, skills: &[Skill]) -> Array {
    let requested_paths = skills
        .iter()
        .map(|skill| skill.path.as_str())
        .collect::<BTreeSet<_>>();
    let mut selected_paths = table
        .get("skills")
        .and_then(Item::as_array)
        .cloned()
        .unwrap_or_default();
    let multiline_indentation = selected_paths
        .iter()
        .filter_map(|value| value.decor().prefix().and_then(|prefix| prefix.as_str()))
        .find(|prefix| prefix.contains('\n'))
        .map(multiline_indentation);
    let mut retained_paths = BTreeSet::new();
    let mut removed_indices = Vec::new();
    for index in 0..selected_paths.len() {
        let retained = selected_paths
            .get(index)
            .and_then(Value::as_str)
            .is_some_and(|path| {
                requested_paths.contains(path) && retained_paths.insert(path.to_owned())
            });
        if !retained {
            removed_indices.push(index);
        }
    }
    for index in removed_indices.into_iter().rev() {
        selected_paths.remove(index);
    }
    for skill in skills {
        if retained_paths.insert(skill.path.clone()) {
            if let Some(indentation) = &multiline_indentation {
                let mut path = Formatted::new(skill.path.clone());
                path.decor_mut().set_prefix(indentation);
                selected_paths.push_formatted(Value::String(path));
            } else {
                selected_paths.push(skill.path.as_str());
            }
        }
    }
    selected_paths
}

fn multiline_indentation(prefix: &str) -> String {
    let indentation = prefix.rsplit_once('\n').map_or("", |(_, suffix)| suffix);
    format!("\n{indentation}")
}

fn set_table_value(table: &mut Table, key: &str, mut new_value: Value) {
    if let Some(existing_value) = table.get(key).and_then(Item::as_value) {
        new_value.decor_mut().clone_from(existing_value.decor());
    }
    table[key] = Item::Value(new_value);
}

fn write_manifest_atomic(manifest_path: &Path, contents: &[u8]) -> Result<(), SelectError> {
    ensure_selection_not_cancelled()?;
    let parent = manifest_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let manifest = display_path(manifest_path);
    let mut temporary =
        NamedTempFile::new_in(parent).map_err(|source| SelectError::ManifestWrite {
            path: manifest.clone(),
            source,
        })?;
    temporary
        .write_all(contents)
        .and_then(|()| temporary.as_file_mut().sync_all())
        .map_err(|source| SelectError::ManifestWrite {
            path: manifest.clone(),
            source,
        })?;
    ensure_selection_not_cancelled()?;
    temporary
        .persist(manifest_path)
        .map_err(|error| SelectError::ManifestWrite {
            path: manifest,
            source: error.error,
        })?;
    Ok(())
}

fn ensure_selection_not_cancelled() -> Result<(), SelectError> {
    if cancellation_requested() {
        Err(DiscoverError::Cancelled.into())
    } else {
        Ok(())
    }
}

fn discover_local(
    source_path: PathBuf,
    requested_revision: String,
    allow_empty: bool,
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
    discover_repository(&repository_root, context, requested_revision, allow_empty)
}

fn discover_github(
    source_url: String,
    requested_revision: String,
    allow_empty: bool,
) -> Result<Discovery, DiscoverError> {
    let github = normalize_github_url(&source_url)?;
    let temporary = TempDir::new().map_err(|source| DiscoverError::TemporaryRepository {
        repository: github.identity.clone(),
        source,
    })?;
    let repository_root = temporary.path().join("repository.git");
    let mut clone = Command::new("git");
    clone
        .args(["clone", "--filter=blob:none", "--no-checkout", "--quiet"])
        .arg(&github.identity)
        .arg(&repository_root);
    let result = run_command(&mut clone)
        .map_err(|error| match error {
            CommandExecutionError::Unavailable(error) => DiscoverError::GitUnavailable(error),
            CommandExecutionError::Cancelled => DiscoverError::Cancelled,
        })
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
            discover_repository(&repository_root, context, requested_revision, allow_empty)
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
    allow_empty: bool,
) -> Result<Discovery, DiscoverError> {
    let resolved_commit = resolve_commit(
        repository_root,
        &requested_revision,
        context.resolve_remote_branch,
    )
    .map_err(|error| {
        revision_error(
            error,
            &context.identity,
            &requested_revision,
            "resolving the requested revision",
        )
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
    .map_err(|error| {
        revision_error(
            error,
            &context.identity,
            &requested_revision,
            "reading the requested repository tree",
        )
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

    if skill_paths.is_empty() && !allow_empty {
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

fn revision_error(
    error: GitFailure,
    repository: &str,
    revision: &str,
    operation: &'static str,
) -> DiscoverError {
    match error {
        GitFailure::Unavailable(error) => DiscoverError::GitUnavailable(error),
        GitFailure::Failed(details) => DiscoverError::RevisionUnavailable {
            repository: repository.to_owned(),
            revision: revision.to_owned(),
            details,
        },
        GitFailure::InvalidUtf8 => DiscoverError::InvalidGitOutput { operation },
        GitFailure::Cancelled => DiscoverError::Cancelled,
    }
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

enum CommandExecutionError {
    Unavailable(io::Error),
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
    let output = run_git(repository, arguments)?;
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

fn run_git<const N: usize>(repository: &Path, arguments: [&str; N]) -> Result<Output, GitFailure> {
    let mut command = Command::new("git");
    command.arg("-C").arg(repository).args(arguments);
    run_command(&mut command).map_err(|error| match error {
        CommandExecutionError::Unavailable(error) => GitFailure::Unavailable(error),
        CommandExecutionError::Cancelled => GitFailure::Cancelled,
    })
}

fn run_command(command: &mut Command) -> Result<Output, CommandExecutionError> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    configure_process_group(command);
    let mut child = command
        .spawn()
        .map_err(CommandExecutionError::Unavailable)?;
    let stdout = child.stdout.take().expect("stdout is configured as piped");
    let stderr = child.stderr.take().expect("stderr is configured as piped");
    let stdout_reader = thread::spawn(move || read_all(stdout));
    let stderr_reader = thread::spawn(move || read_all(stderr));

    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(CommandExecutionError::Unavailable)?
        {
            break status;
        }
        if cancellation_requested() {
            terminate_child(&mut child).map_err(CommandExecutionError::Unavailable)?;
            child.wait().map_err(CommandExecutionError::Unavailable)?;
            join_reader(stdout_reader)?;
            join_reader(stderr_reader)?;
            return Err(CommandExecutionError::Cancelled);
        }
        thread::sleep(Duration::from_millis(10));
    };

    let output = Output {
        status,
        stdout: join_reader(stdout_reader)?,
        stderr: join_reader(stderr_reader)?,
    };
    if cancellation_requested() {
        Err(CommandExecutionError::Cancelled)
    } else {
        Ok(output)
    }
}

fn read_all(mut reader: impl Read) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    reader.read_to_end(&mut output)?;
    Ok(output)
}

fn join_reader(
    reader: thread::JoinHandle<io::Result<Vec<u8>>>,
) -> Result<Vec<u8>, CommandExecutionError> {
    reader
        .join()
        .map_err(|_| {
            CommandExecutionError::Unavailable(io::Error::other(
                "Git output reader thread panicked",
            ))
        })?
        .map_err(CommandExecutionError::Unavailable)
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn terminate_child(child: &mut Child) -> io::Result<()> {
    // Each Git child is the leader of the dedicated process group created above.
    let result = unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    if result == 0 {
        return Ok(());
    }

    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

#[cfg(windows)]
fn terminate_child(child: &mut Child) -> io::Result<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }

    let process_id = child.id().to_string();
    let status = Command::new("taskkill")
        .args(["/PID", process_id.as_str(), "/T", "/F"])
        .status();
    if status.is_ok_and(|status| status.success()) {
        return Ok(());
    }

    let error = match child.kill() {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    if child.try_wait()?.is_some() {
        Ok(())
    } else {
        Err(error)
    }
}

#[cfg(not(any(unix, windows)))]
fn terminate_child(child: &mut Child) -> io::Result<()> {
    child.kill()
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
