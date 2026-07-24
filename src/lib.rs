use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::{NamedTempFile, TempDir};
use thiserror::Error;
use toml_edit::{Array, ArrayOfTables, DocumentMut, Formatted, Item, Table, Value, value};

static CANCELLATION_REQUESTED: AtomicBool = AtomicBool::new(false);
const DESTINATION_STATE_FILE: &str = ".skill-manager-state.json";
const TRANSACTION_DIRECTORY: &str = ".skill-manager-transaction";
const TRANSACTION_JOURNAL_FILE: &str = "journal.json";

#[derive(Debug, Clone)]
pub struct DiscoverRequest {
    source: DiscoverSource,
    revision: Option<String>,
}

fn require_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(format!("'{}' is not a directory", display_path(path))),
        Err(error) => Err(format!(
            "could not inspect '{}': {error}",
            display_path(path)
        )),
    }
}

fn require_regular_file(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(format!("'{}' is not a regular file", display_path(path))),
        Err(error) => Err(format!(
            "could not inspect '{}': {error}",
            display_path(path)
        )),
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone)]
pub struct SyncRequest {
    manifest_path: PathBuf,
    destination: Option<PathBuf>,
    force: bool,
}

impl SyncRequest {
    pub fn new(manifest_path: impl Into<PathBuf>) -> Self {
        Self {
            manifest_path: manifest_path.into(),
            destination: None,
            force: false,
        }
    }

    pub fn with_destination(mut self, destination: impl Into<PathBuf>) -> Self {
        self.destination = Some(destination.into());
        self
    }

    pub fn with_force(mut self) -> Self {
        self.force = true;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SynchronizationResult {
    pub schema_version: u32,
    pub manifest_path: String,
    pub destination: String,
    pub created: Vec<MaterializedSkill>,
    pub updated: Vec<MaterializedSkill>,
    pub removed: Vec<MaterializedSkill>,
    pub unchanged: Vec<MaterializedSkill>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterializedSkill {
    pub identity: SkillIdentity,
    pub name: String,
    pub resolved_commit: String,
    pub digest: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct DestinationState {
    state_version: u32,
    owner: String,
    managed_skills: Vec<MaterializedSkill>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TransactionJournal {
    journal_version: u32,
    owner: String,
    phase: TransactionPhase,
    destination_existed: bool,
    operations: Vec<TransactionOperation>,
    previous_state_path: Option<String>,
    backup_state_path: Option<String>,
    next_state_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TransactionPhase {
    Preparing,
    Committing,
    RollingBack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncTestPoint {
    DuringStaging,
    AfterJournal,
    AfterOldEntries,
    AfterNewEntries,
    AfterOldState,
    AfterNewState,
    AfterRollbackDiscard,
}

#[cfg(debug_assertions)]
impl SyncTestPoint {
    fn as_str(self) -> &'static str {
        match self {
            Self::DuringStaging => "during-staging",
            Self::AfterJournal => "after-journal",
            Self::AfterOldEntries => "after-old-entries",
            Self::AfterNewEntries => "after-new-entries",
            Self::AfterOldState => "after-old-state",
            Self::AfterNewState => "after-new-state",
            Self::AfterRollbackDiscard => "after-rollback-discard",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TransactionOperation {
    name: String,
    kind: TransactionOperationKind,
    destination_path: String,
    staged_path: Option<String>,
    backup_path: Option<String>,
    previous: TransactionEntryState,
    next_digest: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TransactionOperationKind {
    Create,
    Update,
    Remove,
}

impl TransactionOperationKind {
    fn has_staged_content(self) -> bool {
        self != Self::Remove
    }

    fn has_backup(self) -> bool {
        self != Self::Create
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "digest")]
enum TransactionEntryState {
    Missing,
    Digest(String),
    Unsupported(String),
}

#[derive(Debug, Error)]
pub enum SyncError {
    #[error(transparent)]
    Manifest(#[from] SelectError),
    #[error("Git is required but could not be executed: {0}")]
    GitUnavailable(#[source] io::Error),
    #[error(
        "Source Repository '{repository}' uses unsupported type '{repository_type}'; Skill Synchronization supports local Source Repositories only"
    )]
    UnsupportedSource {
        repository: String,
        repository_type: &'static str,
    },
    #[error(
        "selected Skills would use duplicate destination name '{name}': {first:?} and {second:?}"
    )]
    DuplicateDestinationName {
        name: String,
        first: SkillIdentity,
        second: SkillIdentity,
    },
    #[error("Synchronization Destination '{path}' exists but is not a directory")]
    DestinationNotDirectory { path: String },
    #[error("Synchronization Destination state '{path}' is invalid: {details}")]
    InvalidDestinationState { path: String, details: String },
    #[error("Synchronization transaction journal '{path}' is invalid: {details}")]
    InvalidTransactionJournal { path: String, details: String },
    #[error(
        "Materialized Skill '{skill}' has drift at '{path}'; rerun with --force to replace or remove this managed content"
    )]
    MaterializedSkillDrift { skill: String, path: String },
    #[error(
        "Synchronization Destination entry '{path}' changed while source content was being staged; retry synchronization"
    )]
    DestinationChangedDuringSynchronization { path: String },
    #[error(
        "Synchronization Destination entry '{path}' is unmanaged and cannot be overwritten, including with --force"
    )]
    UnmanagedCollision { path: String },
    #[error("could not inspect Synchronization Destination '{path}': {source}")]
    DestinationRead {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error(
        "resolved commit '{commit}' is unavailable in local Source Repository '{repository}': {details}"
    )]
    CommitUnavailable {
        repository: String,
        commit: String,
        details: String,
    },
    #[error(
        "selected Skill '{skill}' in Source Repository '{repository}' contains unsupported tracked entry '{entry}'"
    )]
    UnsupportedTrackedEntry {
        repository: String,
        skill: String,
        entry: String,
    },
    #[error("Git returned invalid data while materializing Skill '{skill}' from '{repository}'")]
    InvalidGitOutput { repository: String, skill: String },
    #[error("could not stage Materialized Skill '{skill}': {source}")]
    Staging {
        skill: String,
        #[source]
        source: io::Error,
    },
    #[error("could not write Synchronization Destination '{path}': {source}")]
    DestinationWrite {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("{operation}; additionally, rollback failed: {rollback}")]
    Rollback {
        operation: Box<SyncError>,
        rollback: io::Error,
    },
    #[error("Skill Synchronization was cancelled")]
    Cancelled,
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

        write_manifest_atomic_for_removal(
            &self.manifest_path,
            self.document.to_string().as_bytes(),
        )?;

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
    #[error("removal was cancelled")]
    RemovalCancelled,
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

pub fn sync(request: SyncRequest) -> Result<SynchronizationResult, SyncError> {
    if cancellation_requested() {
        return Err(SyncError::Cancelled);
    }

    let manifest_path = request.manifest_path;
    let destination = request.destination.unwrap_or_else(|| {
        manifest_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .join(".agents/skills")
    });
    let force = request.force;

    ensure_destination_directory(&destination)?;
    recover_transaction(&destination)?;
    let selections = list_selections(&manifest_path)?;

    let mut planned = Vec::new();
    let mut destination_names = BTreeMap::<String, SkillIdentity>::new();
    for source in selections.sources {
        if source.source.repository_type != "local" {
            return Err(SyncError::UnsupportedSource {
                repository: source.source.path,
                repository_type: source.source.repository_type,
            });
        }
        for skill in source.skills {
            if let Some(first) =
                destination_names.insert(skill.name.clone(), skill.identity.clone())
            {
                return Err(SyncError::DuplicateDestinationName {
                    name: skill.name,
                    first,
                    second: skill.identity,
                });
            }
            planned.push(PlannedMaterialization {
                source_path: PathBuf::from(&source.source.path),
                identity: skill.identity,
                name: skill.name,
                skill_path: skill.path,
                resolved_commit: source.resolved_commit.clone(),
            });
        }
    }
    planned.sort_by(|left, right| left.name.cmp(&right.name));

    let previous_state = load_destination_state(&destination)?;
    let mut recorded = previous_state
        .as_ref()
        .map(|state| {
            state
                .managed_skills
                .iter()
                .cloned()
                .map(|skill| (skill.name.clone(), skill))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let mut created_plans = Vec::new();
    let mut updated_plans = Vec::new();
    let mut removed = Vec::new();
    let mut unchanged = Vec::new();
    let mut observed_materialized_skills = BTreeMap::new();
    for materialization in planned {
        if let Some(recorded_skill) = recorded.remove(&materialization.name) {
            let path = destination.join(&materialization.name);
            let contents = inspect_materialized_skill(&recorded_skill, &path, force)?;
            let has_recorded_contents = contents.has_digest(&recorded_skill.digest);
            observed_materialized_skills.insert(materialization.name.clone(), contents);
            if has_recorded_contents
                && recorded_skill.identity == materialization.identity
                && recorded_skill.resolved_commit == materialization.resolved_commit
            {
                unchanged.push(recorded_skill);
            } else {
                updated_plans.push(materialization);
            }
        } else {
            ensure_unmanaged_entry_absent(&destination.join(&materialization.name))?;
            created_plans.push(materialization);
        }
    }
    for (_, recorded_skill) in recorded {
        let path = destination.join(&recorded_skill.name);
        let contents = inspect_materialized_skill(&recorded_skill, &path, force)?;
        observed_materialized_skills.insert(recorded_skill.name.clone(), contents);
        removed.push(recorded_skill);
    }

    let staged = TempDir::new().map_err(|source| SyncError::DestinationWrite {
        path: display_path(&destination),
        source,
    })?;
    let mut created = stage_materializations(&created_plans, staged.path())?;
    let mut updated = stage_materializations(&updated_plans, staged.path())?;
    created.sort_by(|left, right| left.name.cmp(&right.name));
    updated.sort_by(|left, right| left.name.cmp(&right.name));
    removed.sort_by(|left, right| left.name.cmp(&right.name));
    unchanged.sort_by(|left, right| left.name.cmp(&right.name));

    if previous_state.is_some()
        && created_plans.is_empty()
        && updated_plans.is_empty()
        && removed.is_empty()
    {
        return Ok(SynchronizationResult {
            schema_version: 1,
            manifest_path: display_path(&manifest_path),
            destination: display_path(&destination),
            created: Vec::new(),
            updated: Vec::new(),
            removed,
            unchanged,
        });
    }

    let mut managed_skills = Vec::with_capacity(created.len() + updated.len() + unchanged.len());
    managed_skills.extend(created.iter().cloned());
    managed_skills.extend(updated.iter().cloned());
    managed_skills.extend(unchanged.iter().cloned());
    managed_skills.sort_by(|left, right| left.name.cmp(&right.name));
    let state = serde_json::to_vec_pretty(&DestinationState {
        state_version: 1,
        owner: "skill-manager".to_owned(),
        managed_skills,
    })
    .expect("destination state contains only serializable values");
    commit_synchronization(
        &destination,
        staged.path(),
        &state,
        SynchronizationChanges {
            created: &created,
            updated: &updated,
            removed: &removed,
        },
        previous_state.as_ref(),
        &observed_materialized_skills,
    )?;

    Ok(SynchronizationResult {
        schema_version: 1,
        manifest_path: display_path(&manifest_path),
        destination: display_path(&destination),
        created,
        updated,
        removed,
        unchanged,
    })
}

fn recover_transaction(destination: &Path) -> Result<(), SyncError> {
    let Some(journal) = load_transaction_journal(destination)? else {
        return Ok(());
    };
    validate_transaction_journal(destination, &journal)?;
    match journal.phase {
        TransactionPhase::Preparing => {
            let (previous_state, _) =
                validate_transaction_artifacts_for_commit(destination, &journal)?;
            validate_preparing_destination(destination, &journal, previous_state.as_ref())?;
            cleanup_transaction(destination, &journal)
        }
        TransactionPhase::Committing => {
            complete_transaction_commit(destination, &journal)?;
            cleanup_transaction(destination, &journal)
        }
        TransactionPhase::RollingBack => {
            rollback_transaction(destination, &journal)?;
            cleanup_transaction(destination, &journal)
        }
    }
}

fn load_transaction_journal(destination: &Path) -> Result<Option<TransactionJournal>, SyncError> {
    let transaction_path = destination.join(TRANSACTION_DIRECTORY);
    let journal_path = transaction_path.join(TRANSACTION_JOURNAL_FILE);
    match fs::symlink_metadata(&transaction_path) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(invalid_transaction_journal(
                &journal_path,
                "transaction path is not a directory",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(SyncError::DestinationRead {
                path: display_path(&transaction_path),
                source,
            });
        }
    }
    require_regular_file(&journal_path)
        .map_err(|details| invalid_transaction_journal(&journal_path, details))?;
    let contents =
        fs::read(&journal_path).map_err(|source| SyncError::InvalidTransactionJournal {
            path: display_path(&journal_path),
            details: source.to_string(),
        })?;
    let journal: TransactionJournal = serde_json::from_slice(&contents).map_err(|error| {
        SyncError::InvalidTransactionJournal {
            path: display_path(&journal_path),
            details: error.to_string(),
        }
    })?;
    if journal.journal_version != 1 || journal.owner != "skill-manager" {
        return Err(SyncError::InvalidTransactionJournal {
            path: display_path(&journal_path),
            details: "expected journal_version 1 owned by 'skill-manager'".to_owned(),
        });
    }
    Ok(Some(journal))
}

struct PlannedMaterialization {
    source_path: PathBuf,
    identity: SkillIdentity,
    name: String,
    skill_path: String,
    resolved_commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MaterializedSkillContents {
    Missing,
    Digest(String),
    UnsupportedEntry { path: String, fingerprint: String },
}

impl MaterializedSkillContents {
    fn has_digest(&self, expected: &str) -> bool {
        matches!(self, Self::Digest(digest) if digest == expected)
    }

    fn is_drifted(&self, expected: &str) -> bool {
        !matches!(self, Self::Missing) && !self.has_digest(expected)
    }
}

fn inspect_materialized_skill(
    recorded_skill: &MaterializedSkill,
    path: &Path,
    force: bool,
) -> Result<MaterializedSkillContents, SyncError> {
    let contents = materialized_skill_contents(path)?;
    if contents.is_drifted(&recorded_skill.digest) && !force {
        return Err(SyncError::MaterializedSkillDrift {
            skill: recorded_skill.name.clone(),
            path: display_path(path),
        });
    }
    Ok(contents)
}

fn load_destination_state(destination: &Path) -> Result<Option<DestinationState>, SyncError> {
    let state_path = destination.join(DESTINATION_STATE_FILE);
    let contents = match fs::read(&state_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(SyncError::DestinationRead {
                path: display_path(&state_path),
                source,
            });
        }
    };
    parse_destination_state(&contents, &state_path).map(Some)
}

fn parse_destination_state(
    contents: &[u8],
    state_path: &Path,
) -> Result<DestinationState, SyncError> {
    let state: DestinationState =
        serde_json::from_slice(contents).map_err(|error| SyncError::InvalidDestinationState {
            path: display_path(state_path),
            details: error.to_string(),
        })?;
    if state.state_version != 1 || state.owner != "skill-manager" {
        return Err(SyncError::InvalidDestinationState {
            path: display_path(state_path),
            details: "expected state_version 1 owned by 'skill-manager'".to_owned(),
        });
    }
    let mut names = BTreeSet::new();
    for skill in &state.managed_skills {
        if !is_destination_name(&skill.name) {
            return Err(SyncError::InvalidDestinationState {
                path: display_path(state_path),
                details: format!(
                    "managed Skill name '{}' is not a safe top-level entry",
                    skill.name
                ),
            });
        }
        if !names.insert(skill.name.as_str()) {
            return Err(SyncError::InvalidDestinationState {
                path: display_path(state_path),
                details: "managed Skill names must be unique".to_owned(),
            });
        }
        if !is_hex_identifier(&skill.resolved_commit, &[40, 64]) {
            return Err(SyncError::InvalidDestinationState {
                path: display_path(state_path),
                details: format!(
                    "managed Skill '{}' has an invalid resolved commit",
                    skill.name
                ),
            });
        }
        let digest = skill.digest.strip_prefix("sha256:").unwrap_or_default();
        if !is_hex_identifier(digest, &[64]) {
            return Err(SyncError::InvalidDestinationState {
                path: display_path(state_path),
                details: format!("managed Skill '{}' has an invalid digest", skill.name),
            });
        }
    }
    Ok(state)
}

fn is_hex_identifier(value: &str, lengths: &[usize]) -> bool {
    lengths.contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn materialized_skill_contents(skill_path: &Path) -> Result<MaterializedSkillContents, SyncError> {
    let metadata = match fs::symlink_metadata(skill_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(MaterializedSkillContents::Missing);
        }
        Err(source) => {
            return Err(SyncError::DestinationRead {
                path: display_path(skill_path),
                source,
            });
        }
    };
    if !metadata.is_dir() {
        return Ok(MaterializedSkillContents::UnsupportedEntry {
            path: display_path(skill_path),
            fingerprint: destination_entry_fingerprint(skill_path)?,
        });
    }

    let mut files = Vec::new();
    let mut unsupported = None;
    collect_materialized_files(skill_path, skill_path, &mut files, &mut unsupported)?;
    if let Some(path) = unsupported {
        return Ok(MaterializedSkillContents::UnsupportedEntry {
            path,
            fingerprint: destination_entry_fingerprint(skill_path)?,
        });
    }

    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    for (relative, path, mode) in files {
        let contents = fs::read(&path).map_err(|source| SyncError::DestinationRead {
            path: display_path(&path),
            source,
        })?;
        update_materialized_digest(&mut hasher, &relative, mode, &contents);
    }
    Ok(MaterializedSkillContents::Digest(format!(
        "sha256:{:x}",
        hasher.finalize()
    )))
}

fn destination_entry_fingerprint(path: &Path) -> Result<String, SyncError> {
    let mut hasher = Sha256::new();
    update_destination_entry_fingerprint(&mut hasher, path, Path::new(""), path)?;
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn update_destination_entry_fingerprint(
    hasher: &mut Sha256,
    path: &Path,
    relative: &Path,
    root: &Path,
) -> Result<(), SyncError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| SyncError::DestinationRead {
        path: display_path(path),
        source,
    })?;
    update_fingerprint_field(hasher, &fingerprint_path_bytes(relative));
    if metadata.is_dir() {
        update_fingerprint_field(hasher, b"directory");
        let mut entries = fs::read_dir(path)
            .map_err(|source| SyncError::DestinationRead {
                path: display_path(path),
                source,
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| SyncError::DestinationRead {
                path: display_path(path),
                source,
            })?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let entry_path = entry.path();
            let relative = entry_path
                .strip_prefix(root)
                .expect("fingerprinted path is below its root");
            update_destination_entry_fingerprint(hasher, &entry_path, relative, root)?;
        }
    } else if metadata.is_file() {
        update_fingerprint_field(hasher, b"file");
        update_fingerprint_field(hasher, materialized_file_mode(&metadata).as_bytes());
        let contents = fs::read(path).map_err(|source| SyncError::DestinationRead {
            path: display_path(path),
            source,
        })?;
        update_fingerprint_field(hasher, &contents);
    } else if metadata.file_type().is_symlink() {
        update_fingerprint_field(hasher, b"symlink");
        let target = fs::read_link(path).map_err(|source| SyncError::DestinationRead {
            path: display_path(path),
            source,
        })?;
        update_fingerprint_field(hasher, &fingerprint_path_bytes(&target));
    } else {
        update_fingerprint_field(hasher, b"other");
    }
    Ok(())
}

fn update_fingerprint_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

#[cfg(unix)]
fn fingerprint_path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;

    path.as_os_str().as_bytes().to_vec()
}

#[cfg(windows)]
fn fingerprint_path_bytes(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;

    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(not(any(unix, windows)))]
fn fingerprint_path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().as_bytes().to_vec()
}

fn collect_materialized_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(String, PathBuf, &'static str)>,
    unsupported: &mut Option<String>,
) -> Result<bool, SyncError> {
    let entries = fs::read_dir(directory).map_err(|source| SyncError::DestinationRead {
        path: display_path(directory),
        source,
    })?;
    let mut has_entries = false;
    for entry in entries {
        has_entries = true;
        let entry = entry.map_err(|source| SyncError::DestinationRead {
            path: display_path(directory),
            source,
        })?;
        let path = entry.path();
        let metadata =
            fs::symlink_metadata(&path).map_err(|source| SyncError::DestinationRead {
                path: display_path(&path),
                source,
            })?;
        if metadata.is_dir() {
            if !collect_materialized_files(root, &path, files, unsupported)? {
                *unsupported = Some(display_path(&path));
            }
        } else if metadata.is_file() {
            let relative = path
                .strip_prefix(root)
                .expect("collected path is below root");
            let Some(relative) = digest_path(relative) else {
                *unsupported = Some(display_path(&path));
                continue;
            };
            files.push((relative, path, materialized_file_mode(&metadata)));
        } else {
            *unsupported = Some(display_path(&path));
        }
    }
    Ok(has_entries)
}

fn digest_path(path: &Path) -> Option<String> {
    path.components()
        .map(|component| match component {
            Component::Normal(component) => component.to_str(),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .map(|components| components.join("/"))
}

#[cfg(unix)]
fn materialized_file_mode(metadata: &fs::Metadata) -> &'static str {
    use std::os::unix::fs::PermissionsExt;

    if metadata.permissions().mode() & 0o111 == 0 {
        "100644"
    } else {
        "100755"
    }
}

#[cfg(not(unix))]
fn materialized_file_mode(_metadata: &fs::Metadata) -> &'static str {
    "100644"
}

fn ensure_destination_directory(destination: &Path) -> Result<(), SyncError> {
    match fs::metadata(destination) {
        Ok(metadata) if !metadata.is_dir() => {
            return Err(SyncError::DestinationNotDirectory {
                path: display_path(destination),
            });
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(SyncError::DestinationRead {
                path: display_path(destination),
                source,
            });
        }
    }
    Ok(())
}

fn validate_transaction_artifacts_for_rollback(
    destination: &Path,
    journal: &TransactionJournal,
) -> Result<(Option<DestinationState>, DestinationState), SyncError> {
    let transaction = destination.join(TRANSACTION_DIRECTORY);
    let journal_path = transaction.join(TRANSACTION_JOURNAL_FILE);
    validate_transaction_directories(&transaction, &journal_path)?;
    let states = read_transaction_states(destination, journal)?;
    validate_transaction_backups(destination, journal)?;
    Ok(states)
}

fn validate_transaction_directories(
    transaction: &Path,
    journal_path: &Path,
) -> Result<(), SyncError> {
    for directory in ["staged", "backup", "installing", "discarded"] {
        let path = transaction.join(directory);
        require_directory(&path)
            .map_err(|details| invalid_transaction_journal(journal_path, details))?;
    }
    Ok(())
}

fn read_transaction_states(
    destination: &Path,
    journal: &TransactionJournal,
) -> Result<(Option<DestinationState>, DestinationState), SyncError> {
    let transaction = destination.join(TRANSACTION_DIRECTORY);
    let journal_path = transaction.join(TRANSACTION_JOURNAL_FILE);
    let previous_state = journal
        .previous_state_path
        .as_ref()
        .map(|path| {
            read_transaction_state(
                &transaction,
                &journal_path,
                path,
                "previous destination state",
            )
        })
        .transpose()?;
    let next_state = read_transaction_state(
        &transaction,
        &journal_path,
        &journal.next_state_path,
        "next destination state",
    )?;
    validate_transaction_state_transition(
        &journal_path,
        journal,
        previous_state.as_ref(),
        &next_state,
    )?;
    if let (Some(previous_state), Some(backup_state_path)) =
        (&previous_state, &journal.backup_state_path)
    {
        let backup_state_path = transaction.join(backup_state_path);
        if entry_exists(&backup_state_path)? {
            let backup_state = read_transaction_state(
                &transaction,
                &journal_path,
                journal
                    .backup_state_path
                    .as_ref()
                    .expect("checked backup state path"),
                "backup destination state",
            )?;
            if &backup_state != previous_state {
                return Err(invalid_transaction_journal(
                    &journal_path,
                    "backup destination state does not match the recorded previous state",
                ));
            }
        }
    }
    Ok((previous_state, next_state))
}

fn read_transaction_state(
    transaction: &Path,
    journal_path: &Path,
    relative_path: &str,
    label: &str,
) -> Result<DestinationState, SyncError> {
    let state_path = transaction.join(relative_path);
    require_regular_file(&state_path)
        .map_err(|details| invalid_transaction_journal(journal_path, details))?;
    let contents = fs::read(&state_path).map_err(|error| {
        invalid_transaction_journal(journal_path, format!("could not read {label}: {error}"))
    })?;
    parse_destination_state(&contents, &state_path)
        .map_err(|error| invalid_transaction_journal(journal_path, error.to_string()))
}

fn validate_transaction_state_transition(
    journal_path: &Path,
    journal: &TransactionJournal,
    previous_state: Option<&DestinationState>,
    next_state: &DestinationState,
) -> Result<(), SyncError> {
    let previous = previous_state
        .map(|state| {
            state
                .managed_skills
                .iter()
                .map(|skill| (skill.name.as_str(), skill))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let next = next_state
        .managed_skills
        .iter()
        .map(|skill| (skill.name.as_str(), skill))
        .collect::<BTreeMap<_, _>>();
    let operations = journal
        .operations
        .iter()
        .map(|operation| (operation.name.as_str(), operation))
        .collect::<BTreeMap<_, _>>();

    for operation in &journal.operations {
        let previous_skill = previous.get(operation.name.as_str()).copied();
        let next_skill = next.get(operation.name.as_str()).copied();
        let valid = match operation.kind {
            TransactionOperationKind::Create => {
                previous_skill.is_none()
                    && next_skill.is_some_and(|skill| {
                        operation.next_digest.as_deref() == Some(skill.digest.as_str())
                    })
            }
            TransactionOperationKind::Update => {
                previous_skill.is_some()
                    && next_skill.is_some_and(|skill| {
                        operation.next_digest.as_deref() == Some(skill.digest.as_str())
                    })
            }
            TransactionOperationKind::Remove => previous_skill.is_some() && next_skill.is_none(),
        };
        if !valid {
            return Err(invalid_transaction_journal(
                journal_path,
                format!(
                    "operation for Materialized Skill '{}' does not match the recorded destination states",
                    operation.name
                ),
            ));
        }
    }

    let state_names = previous
        .keys()
        .chain(next.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    if state_names
        .into_iter()
        .filter(|name| !operations.contains_key(name))
        .any(|name| previous.get(name) != next.get(name))
    {
        return Err(invalid_transaction_journal(
            journal_path,
            "unchanged Materialized Skills differ between previous and next destination state",
        ));
    }
    Ok(())
}

fn ensure_unmanaged_entry_absent(path: &Path) -> Result<(), SyncError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(SyncError::UnmanagedCollision {
            path: display_path(path),
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(SyncError::DestinationRead {
            path: display_path(path),
            source,
        }),
    }
}

fn is_destination_name(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
        && name != DESTINATION_STATE_FILE
        && name != TRANSACTION_DIRECTORY
}

fn stage_materialized_skill(
    materialization: &PlannedMaterialization,
    destination: &Path,
) -> Result<String, SyncError> {
    let commit_expression = format!("{}^{{commit}}", materialization.resolved_commit);
    let commit = git_text(
        &materialization.source_path,
        [
            "rev-parse",
            "--verify",
            "--end-of-options",
            commit_expression.as_str(),
        ],
    )
    .map_err(|error| synchronization_git_error(error, materialization))?;
    if commit != materialization.resolved_commit {
        return Err(SyncError::CommitUnavailable {
            repository: materialization.identity.source.clone(),
            commit: materialization.resolved_commit.clone(),
            details: format!("Git resolved the value to unexpected commit '{commit}'"),
        });
    }

    let pathspec = format!(":(literal){}", materialization.skill_path);
    let tree = if materialization.skill_path == "." {
        git_bytes(
            &materialization.source_path,
            [
                "ls-tree",
                "-r",
                "-z",
                "--full-tree",
                materialization.resolved_commit.as_str(),
            ],
        )
    } else {
        git_bytes(
            &materialization.source_path,
            [
                "ls-tree",
                "-r",
                "-z",
                "--full-tree",
                materialization.resolved_commit.as_str(),
                "--",
                pathspec.as_str(),
            ],
        )
    }
    .map_err(|error| synchronization_git_error(error, materialization))?;

    fs::create_dir(destination).map_err(|source| SyncError::Staging {
        skill: materialization.name.clone(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut found_skill = false;
    for entry in tree
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let separator = entry
            .iter()
            .position(|byte| *byte == b'\t')
            .ok_or_else(|| invalid_materialization_git_output(materialization))?;
        let metadata = std::str::from_utf8(&entry[..separator])
            .map_err(|_| invalid_materialization_git_output(materialization))?;
        let path = std::str::from_utf8(&entry[separator + 1..])
            .map_err(|_| invalid_materialization_git_output(materialization))?;
        let mut metadata = metadata.split_whitespace();
        let mode = metadata
            .next()
            .ok_or_else(|| invalid_materialization_git_output(materialization))?;
        let object_type = metadata
            .next()
            .ok_or_else(|| invalid_materialization_git_output(materialization))?;
        let object = metadata
            .next()
            .ok_or_else(|| invalid_materialization_git_output(materialization))?;
        if !matches!(mode, "100644" | "100755") || object_type != "blob" {
            return Err(SyncError::UnsupportedTrackedEntry {
                repository: materialization.identity.source.clone(),
                skill: materialization.skill_path.clone(),
                entry: path.to_owned(),
            });
        }

        let relative = if materialization.skill_path == "." {
            path
        } else {
            path.strip_prefix(&materialization.skill_path)
                .and_then(|path| path.strip_prefix('/'))
                .ok_or_else(|| invalid_materialization_git_output(materialization))?
        };
        if relative == "SKILL.md" {
            found_skill = true;
        }
        let contents = git_bytes(&materialization.source_path, ["cat-file", "blob", object])
            .map_err(|error| synchronization_git_error(error, materialization))?;
        let output_path = destination.join(relative);
        fs::create_dir_all(output_path.parent().expect("a file has a parent"))
            .and_then(|()| fs::write(&output_path, &contents))
            .map_err(|source| SyncError::Staging {
                skill: materialization.name.clone(),
                source,
            })?;
        set_executable(&output_path, mode == "100755").map_err(|source| SyncError::Staging {
            skill: materialization.name.clone(),
            source,
        })?;

        update_materialized_digest(&mut hasher, relative, mode, &contents);
    }
    if !found_skill {
        return Err(SyncError::CommitUnavailable {
            repository: materialization.identity.source.clone(),
            commit: materialization.resolved_commit.clone(),
            details: format!(
                "selected Skill '{}' is absent at the recorded commit",
                materialization.skill_path
            ),
        });
    }

    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn stage_materializations(
    materializations: &[PlannedMaterialization],
    staged: &Path,
) -> Result<Vec<MaterializedSkill>, SyncError> {
    let mut staged_skills = Vec::with_capacity(materializations.len());
    for materialization in materializations {
        if cancellation_requested() {
            return Err(SyncError::Cancelled);
        }
        let digest =
            stage_materialized_skill(materialization, &staged.join(&materialization.name))?;
        staged_skills.push(MaterializedSkill {
            identity: materialization.identity.clone(),
            name: materialization.name.clone(),
            resolved_commit: materialization.resolved_commit.clone(),
            digest,
        });
        request_sync_cancellation_for_test(SyncTestPoint::DuringStaging);
    }
    Ok(staged_skills)
}

fn invalid_materialization_git_output(materialization: &PlannedMaterialization) -> SyncError {
    SyncError::InvalidGitOutput {
        repository: materialization.identity.source.clone(),
        skill: materialization.skill_path.clone(),
    }
}

fn synchronization_git_error(
    error: GitFailure,
    materialization: &PlannedMaterialization,
) -> SyncError {
    match error {
        GitFailure::Unavailable(error) => SyncError::GitUnavailable(error),
        GitFailure::Failed(details) => SyncError::CommitUnavailable {
            repository: materialization.identity.source.clone(),
            commit: materialization.resolved_commit.clone(),
            details,
        },
        GitFailure::InvalidUtf8 => invalid_materialization_git_output(materialization),
        GitFailure::Cancelled => SyncError::Cancelled,
    }
}

fn update_materialized_digest(hasher: &mut Sha256, path: &str, mode: &str, contents: &[u8]) {
    hasher.update((path.len() as u64).to_be_bytes());
    hasher.update(path.as_bytes());
    hasher.update((mode.len() as u64).to_be_bytes());
    hasher.update(mode.as_bytes());
    hasher.update((contents.len() as u64).to_be_bytes());
    hasher.update(contents);
}

#[cfg(unix)]
fn set_executable(path: &Path, executable: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)?.permissions();
    let mode = permissions.mode();
    permissions.set_mode(if executable {
        mode | 0o111
    } else {
        mode & !0o111
    });
    fs::set_permissions(path, permissions)
}

#[cfg(not(unix))]
fn set_executable(_path: &Path, _executable: bool) -> io::Result<()> {
    Ok(())
}

struct SynchronizationChanges<'a> {
    created: &'a [MaterializedSkill],
    updated: &'a [MaterializedSkill],
    removed: &'a [MaterializedSkill],
}

fn commit_synchronization(
    destination: &Path,
    staged: &Path,
    state: &[u8],
    changes: SynchronizationChanges<'_>,
    previous_state: Option<&DestinationState>,
    observed_materialized_skills: &BTreeMap<String, MaterializedSkillContents>,
) -> Result<(), SyncError> {
    let SynchronizationChanges {
        created,
        updated,
        removed,
    } = changes;

    if cancellation_requested() {
        return Err(SyncError::Cancelled);
    }
    ensure_destination_directory(destination)?;

    let destination_existed = entry_exists(destination)?;
    if destination_existed {
        for skill in created {
            ensure_unmanaged_entry_absent(&destination.join(&skill.name))?;
        }
        let current_state = load_destination_state(destination)?;
        if current_state.as_ref() != previous_state {
            return Err(SyncError::DestinationChangedDuringSynchronization {
                path: display_path(&destination.join(DESTINATION_STATE_FILE)),
            });
        }
        for (name, observed) in observed_materialized_skills {
            let path = destination.join(name);
            if materialized_skill_contents(&path)? != *observed {
                return Err(SyncError::DestinationChangedDuringSynchronization {
                    path: display_path(&path),
                });
            }
        }
    }

    let mut journal = transaction_journal(
        destination_existed,
        created,
        updated,
        removed,
        previous_state,
        observed_materialized_skills,
    );
    let prepare = prepare_transaction(destination, staged, state, previous_state, &journal);
    if let Err(operation) = prepare {
        return Err(if destination.join(TRANSACTION_DIRECTORY).exists() {
            cleanup_after_transaction_failure(destination, &journal, operation)
        } else {
            operation
        });
    }

    journal.phase = TransactionPhase::Committing;
    if let Err(source) = write_transaction_journal(destination, &journal) {
        let operation = destination_write_error(destination, source);
        return Err(cleanup_after_transaction_failure(
            destination,
            &journal,
            operation,
        ));
    }
    interrupt_sync_for_test(SyncTestPoint::AfterJournal);

    match complete_transaction_commit(destination, &journal) {
        Ok(()) => cleanup_transaction(destination, &journal),
        Err(operation) => {
            journal.phase = TransactionPhase::RollingBack;
            if let Err(source) = write_transaction_journal(destination, &journal) {
                return Err(SyncError::Rollback {
                    operation: Box::new(operation),
                    rollback: source,
                });
            }
            if let Err(rollback) = rollback_transaction(destination, &journal) {
                return Err(SyncError::Rollback {
                    operation: Box::new(operation),
                    rollback: sync_error_as_io(rollback),
                });
            }
            Err(cleanup_after_transaction_failure(
                destination,
                &journal,
                operation,
            ))
        }
    }
}

fn entry_exists(path: &Path) -> Result<bool, SyncError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(SyncError::DestinationRead {
            path: display_path(path),
            source,
        }),
    }
}

fn transaction_journal(
    destination_existed: bool,
    created: &[MaterializedSkill],
    updated: &[MaterializedSkill],
    removed: &[MaterializedSkill],
    previous_state: Option<&DestinationState>,
    observed_materialized_skills: &BTreeMap<String, MaterializedSkillContents>,
) -> TransactionJournal {
    let mut operations = Vec::with_capacity(created.len() + updated.len() + removed.len());
    operations.extend(created.iter().map(|skill| TransactionOperation {
        name: skill.name.clone(),
        kind: TransactionOperationKind::Create,
        destination_path: skill.name.clone(),
        staged_path: Some(format!("staged/{}", skill.name)),
        backup_path: None,
        previous: TransactionEntryState::Missing,
        next_digest: Some(skill.digest.clone()),
    }));
    operations.extend(updated.iter().map(|skill| {
        TransactionOperation {
            name: skill.name.clone(),
            kind: TransactionOperationKind::Update,
            destination_path: skill.name.clone(),
            staged_path: Some(format!("staged/{}", skill.name)),
            backup_path: Some(format!("backup/{}", skill.name)),
            previous: transaction_entry_state(
                observed_materialized_skills
                    .get(&skill.name)
                    .expect("updated Skills were observed before staging"),
            ),
            next_digest: Some(skill.digest.clone()),
        }
    }));
    operations.extend(removed.iter().map(|skill| {
        TransactionOperation {
            name: skill.name.clone(),
            kind: TransactionOperationKind::Remove,
            destination_path: skill.name.clone(),
            staged_path: None,
            backup_path: Some(format!("backup/{}", skill.name)),
            previous: transaction_entry_state(
                observed_materialized_skills
                    .get(&skill.name)
                    .expect("removed Skills were observed before staging"),
            ),
            next_digest: None,
        }
    }));
    operations.sort_by(|left, right| left.name.cmp(&right.name));

    TransactionJournal {
        journal_version: 1,
        owner: "skill-manager".to_owned(),
        phase: TransactionPhase::Preparing,
        destination_existed,
        operations,
        previous_state_path: previous_state
            .is_some()
            .then(|| "previous-state.json".to_owned()),
        backup_state_path: previous_state
            .is_some()
            .then(|| format!("backup/{DESTINATION_STATE_FILE}")),
        next_state_path: "next-state.json".to_owned(),
    }
}

fn transaction_entry_state(contents: &MaterializedSkillContents) -> TransactionEntryState {
    match contents {
        MaterializedSkillContents::Missing => TransactionEntryState::Missing,
        MaterializedSkillContents::Digest(digest) => TransactionEntryState::Digest(digest.clone()),
        MaterializedSkillContents::UnsupportedEntry { fingerprint, .. } => {
            TransactionEntryState::Unsupported(fingerprint.clone())
        }
    }
}

fn prepare_transaction(
    destination: &Path,
    staged: &Path,
    state: &[u8],
    previous_state: Option<&DestinationState>,
    journal: &TransactionJournal,
) -> Result<(), SyncError> {
    let destination_parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(destination_parent)
        .map_err(|source| destination_write_error(destination, source))?;
    let prepared = TempDir::new_in(destination_parent)
        .map_err(|source| destination_write_error(destination, source))?;
    let transaction = prepared.path().join(TRANSACTION_DIRECTORY);
    fs::create_dir(&transaction).map_err(|source| destination_write_error(destination, source))?;
    write_transaction_journal_at(&transaction, journal)
        .map_err(|source| destination_write_error(destination, source))?;
    fs::create_dir(transaction.join("staged"))
        .and_then(|()| fs::create_dir(transaction.join("backup")))
        .and_then(|()| fs::create_dir(transaction.join("installing")))
        .and_then(|()| fs::create_dir(transaction.join("discarded")))
        .map_err(|source| destination_write_error(destination, source))?;
    for operation in &journal.operations {
        if operation.staged_path.is_some() {
            copy_directory(
                &staged.join(&operation.name),
                &transaction.join("staged").join(&operation.name),
            )
            .map_err(|source| destination_write_error(destination, source))?;
            sync_tree(&transaction.join("staged").join(&operation.name))
                .map_err(|source| destination_write_error(destination, source))?;
        }
    }
    sync_directory(&transaction.join("staged"))
        .map_err(|source| destination_write_error(destination, source))?;
    write_file_durable(&transaction.join(&journal.next_state_path), state)
        .map_err(|source| destination_write_error(destination, source))?;
    if let (Some(previous_state), Some(previous_state_path)) =
        (previous_state, &journal.previous_state_path)
    {
        let contents = serde_json::to_vec_pretty(previous_state)
            .expect("previous destination state is serializable");
        write_file_durable(&transaction.join(previous_state_path), &contents)
            .map_err(|source| destination_write_error(destination, source))?;
    }
    sync_directory(&transaction).map_err(|source| destination_write_error(destination, source))?;
    sync_directory(prepared.path())
        .map_err(|source| destination_write_error(destination, source))?;
    if journal.destination_existed {
        ensure_destination_directory(destination)?;
        fs::rename(&transaction, destination.join(TRANSACTION_DIRECTORY))
            .map_err(|source| destination_write_error(destination, source))?;
        sync_directory(prepared.path())
            .and_then(|()| sync_directory(destination))
            .map_err(|source| destination_write_error(destination, source))?;
    } else {
        if entry_exists(destination)? {
            return Err(SyncError::DestinationChangedDuringSynchronization {
                path: display_path(destination),
            });
        }
        let prepared = prepared.keep();
        fs::rename(&prepared, destination)
            .map_err(|source| destination_write_error(destination, source))?;
        sync_directory(destination_parent)
            .map_err(|source| destination_write_error(destination, source))?;
    }
    Ok(())
}

fn validate_preparing_destination(
    destination: &Path,
    journal: &TransactionJournal,
    previous_state: Option<&DestinationState>,
) -> Result<(), SyncError> {
    let transaction = destination.join(TRANSACTION_DIRECTORY);
    let journal_path = transaction.join(TRANSACTION_JOURNAL_FILE);
    for directory in ["backup", "discarded"] {
        let directory = transaction.join(directory);
        let has_entries = fs::read_dir(&directory)
            .map_err(|error| {
                invalid_transaction_journal(
                    &journal_path,
                    format!("could not inspect '{}': {error}", display_path(&directory)),
                )
            })?
            .next()
            .is_some();
        if has_entries {
            return Err(invalid_transaction_journal(
                &journal_path,
                "preparing transaction contains commit or rollback data",
            ));
        }
    }
    for operation in &journal.operations {
        let destination_path = destination.join(&operation.destination_path);
        let pristine = match operation.kind {
            TransactionOperationKind::Create => !entry_exists(&destination_path)?,
            TransactionOperationKind::Update | TransactionOperationKind::Remove => {
                entry_matches_previous(&destination_path, &operation.previous)?
            }
        };
        if !pristine {
            return Err(invalid_transaction_journal(
                &journal_path,
                format!(
                    "preparing transaction destination '{}' does not match its previous content",
                    operation.name
                ),
            ));
        }
    }
    let state_path = destination.join(DESTINATION_STATE_FILE);
    let current_state = read_optional_file(&state_path)
        .map_err(|source| destination_write_error(destination, source))?;
    let current_state = current_state
        .as_deref()
        .map(|contents| parse_destination_state(contents, &state_path))
        .transpose()?;
    if current_state.as_ref() != previous_state {
        return Err(invalid_transaction_journal(
            &journal_path,
            "preparing transaction destination state does not match its previous state",
        ));
    }
    Ok(())
}

fn complete_transaction_commit(
    destination: &Path,
    journal: &TransactionJournal,
) -> Result<(), SyncError> {
    let (previous_state, next_state) =
        validate_transaction_artifacts_for_commit(destination, journal)?;
    validate_transaction_destination(destination, journal, previous_state.as_ref(), &next_state)?;
    let transaction = destination.join(TRANSACTION_DIRECTORY);
    let backup = transaction.join("backup");

    for operation in journal
        .operations
        .iter()
        .filter(|operation| operation.kind.has_backup())
    {
        if cancellation_requested() {
            return Err(SyncError::Cancelled);
        }
        let destination_path = destination.join(&operation.destination_path);
        let backup_path = transaction.join(
            operation
                .backup_path
                .as_ref()
                .expect("updated and removed operations have backups"),
        );
        if entry_exists(&backup_path)?
            || entry_matches_digest(&destination_path, operation.next_digest.as_deref())?
        {
            continue;
        }
        if !entry_matches_previous(&destination_path, &operation.previous)? {
            return Err(SyncError::DestinationChangedDuringSynchronization {
                path: display_path(&destination_path),
            });
        }
        if entry_exists(&destination_path)? {
            fs::rename(&destination_path, &backup_path)
                .map_err(|source| destination_write_error(destination, source))?;
            sync_directory(destination)
                .and_then(|()| sync_directory(&backup))
                .map_err(|source| destination_write_error(destination, source))?;
        }
    }
    sync_directory(&backup).map_err(|source| destination_write_error(destination, source))?;
    interrupt_sync_for_test(SyncTestPoint::AfterOldEntries);
    request_sync_cancellation_for_test(SyncTestPoint::AfterOldEntries);

    for operation in journal
        .operations
        .iter()
        .filter(|operation| operation.kind.has_staged_content())
    {
        if cancellation_requested() {
            return Err(SyncError::Cancelled);
        }
        let destination_path = destination.join(&operation.destination_path);
        if entry_matches_digest(&destination_path, operation.next_digest.as_deref())? {
            continue;
        }
        if entry_exists(&destination_path)? {
            return Err(SyncError::DestinationChangedDuringSynchronization {
                path: display_path(&destination_path),
            });
        }
        let installing_path = transaction.join("installing").join(&operation.name);
        remove_path_if_exists(&installing_path)
            .map_err(|source| destination_write_error(destination, source))?;
        copy_directory(
            &transaction.join(
                operation
                    .staged_path
                    .as_ref()
                    .expect("created and updated operations have staged content"),
            ),
            &installing_path,
        )
        .map_err(|source| destination_write_error(destination, source))?;
        sync_tree(&installing_path)
            .map_err(|source| destination_write_error(destination, source))?;
        fs::rename(&installing_path, &destination_path)
            .map_err(|source| destination_write_error(destination, source))?;
        sync_directory(
            installing_path
                .parent()
                .expect("installing path has a parent"),
        )
        .and_then(|()| sync_directory(destination))
        .map_err(|source| destination_write_error(destination, source))?;
    }
    sync_directory(destination).map_err(|source| destination_write_error(destination, source))?;
    interrupt_sync_for_test(SyncTestPoint::AfterNewEntries);
    request_sync_cancellation_for_test(SyncTestPoint::AfterNewEntries);
    if cancellation_requested() {
        return Err(SyncError::Cancelled);
    }

    let state_path = destination.join(DESTINATION_STATE_FILE);
    let next_state_path = transaction.join(&journal.next_state_path);
    let next_state = fs::read(&next_state_path)
        .map_err(|source| destination_write_error(destination, source))?;
    let current_state = read_optional_file(&state_path)
        .map_err(|source| destination_write_error(destination, source))?;
    if let Some(backup_state_path) = &journal.backup_state_path {
        let backup_state_path = transaction.join(backup_state_path);
        if !entry_exists(&backup_state_path)? && current_state.as_deref() != Some(&next_state) {
            let Some(current_state) = current_state.as_deref() else {
                return Err(SyncError::DestinationChangedDuringSynchronization {
                    path: display_path(&state_path),
                });
            };
            let current_state = parse_destination_state(current_state, &state_path)?;
            if previous_state.as_ref() != Some(&current_state) {
                return Err(SyncError::DestinationChangedDuringSynchronization {
                    path: display_path(&state_path),
                });
            }
            fs::rename(&state_path, &backup_state_path)
                .map_err(|source| destination_write_error(destination, source))?;
            sync_directory(destination)
                .and_then(|()| sync_directory(&backup))
                .map_err(|source| destination_write_error(destination, source))?;
        }
    } else if current_state.is_some() && current_state.as_deref() != Some(&next_state) {
        return Err(SyncError::DestinationChangedDuringSynchronization {
            path: display_path(&state_path),
        });
    }
    sync_directory(&backup).map_err(|source| destination_write_error(destination, source))?;
    interrupt_sync_for_test(SyncTestPoint::AfterOldState);
    request_sync_cancellation_for_test(SyncTestPoint::AfterOldState);
    if cancellation_requested() {
        return Err(SyncError::Cancelled);
    }

    if read_optional_file(&state_path)
        .map_err(|source| destination_write_error(destination, source))?
        .as_deref()
        != Some(&next_state)
    {
        write_file_atomic_durable(&state_path, &next_state)
            .map_err(|source| destination_write_error(destination, source))?;
    }
    sync_directory(destination).map_err(|source| destination_write_error(destination, source))?;
    interrupt_sync_for_test(SyncTestPoint::AfterNewState);
    Ok(())
}

fn validate_transaction_destination(
    destination: &Path,
    journal: &TransactionJournal,
    previous_state: Option<&DestinationState>,
    next_state: &DestinationState,
) -> Result<(), SyncError> {
    let transaction = destination.join(TRANSACTION_DIRECTORY);
    for operation in &journal.operations {
        let destination_path = destination.join(&operation.destination_path);
        let backup_exists = operation
            .backup_path
            .as_ref()
            .map(|path| entry_exists(&transaction.join(path)))
            .transpose()?
            .unwrap_or(false);
        let destination_exists = entry_exists(&destination_path)?;
        let valid = if backup_exists {
            match operation.kind {
                TransactionOperationKind::Create => false,
                TransactionOperationKind::Update => {
                    !destination_exists
                        || entry_matches_digest(
                            &destination_path,
                            operation.next_digest.as_deref(),
                        )?
                }
                TransactionOperationKind::Remove => !destination_exists,
            }
        } else {
            match operation.kind {
                TransactionOperationKind::Create => {
                    !destination_exists
                        || entry_matches_digest(
                            &destination_path,
                            operation.next_digest.as_deref(),
                        )?
                }
                TransactionOperationKind::Update => {
                    entry_matches_previous(&destination_path, &operation.previous)?
                        || entry_matches_digest(
                            &destination_path,
                            operation.next_digest.as_deref(),
                        )?
                }
                TransactionOperationKind::Remove => {
                    entry_matches_previous(&destination_path, &operation.previous)?
                }
            }
        };
        if !valid {
            return Err(SyncError::DestinationChangedDuringSynchronization {
                path: display_path(&destination_path),
            });
        }
    }

    let state_path = destination.join(DESTINATION_STATE_FILE);
    let current_state = read_optional_file(&state_path)
        .map_err(|source| destination_write_error(destination, source))?;
    let current_state = current_state
        .as_deref()
        .map(|contents| parse_destination_state(contents, &state_path))
        .transpose()?;
    let backup_state_exists = journal
        .backup_state_path
        .as_ref()
        .map(|path| entry_exists(&transaction.join(path)))
        .transpose()?
        .unwrap_or(false);
    let valid_state = if backup_state_exists {
        current_state
            .as_ref()
            .is_none_or(|current| current == next_state)
    } else {
        current_state.as_ref() == previous_state || current_state.as_ref() == Some(next_state)
    };
    if !valid_state {
        return Err(SyncError::DestinationChangedDuringSynchronization {
            path: display_path(&state_path),
        });
    }
    Ok(())
}

fn rollback_transaction(destination: &Path, journal: &TransactionJournal) -> Result<(), SyncError> {
    let transaction = destination.join(TRANSACTION_DIRECTORY);
    let backup = transaction.join("backup");
    let (previous_state, next_state) =
        validate_transaction_artifacts_for_rollback(destination, journal)?;
    validate_rollback_destination(destination, journal, previous_state.as_ref(), &next_state)?;
    for operation in journal.operations.iter().rev() {
        let destination_path = destination.join(&operation.destination_path);
        match operation.kind {
            TransactionOperationKind::Create => {
                discard_rollback_entry(destination, &transaction, operation)?;
            }
            TransactionOperationKind::Update | TransactionOperationKind::Remove => {
                let backup_path = transaction.join(
                    operation
                        .backup_path
                        .as_ref()
                        .expect("updated and removed operations have backups"),
                );
                if entry_exists(&backup_path)? {
                    if operation.kind == TransactionOperationKind::Update {
                        discard_rollback_entry(destination, &transaction, operation)?;
                    }
                    fs::rename(&backup_path, &destination_path)
                        .map_err(|source| destination_write_error(destination, source))?;
                    sync_directory(&backup)
                        .and_then(|()| sync_directory(destination))
                        .map_err(|source| destination_write_error(destination, source))?;
                } else if operation.kind == TransactionOperationKind::Update
                    && operation.previous == TransactionEntryState::Missing
                {
                    discard_rollback_entry(destination, &transaction, operation)?;
                }
            }
        }
    }

    let state_path = destination.join(DESTINATION_STATE_FILE);
    let next_state = fs::read(transaction.join(&journal.next_state_path))
        .map_err(|source| destination_write_error(destination, source))?;
    if let Some(backup_state_path) = &journal.backup_state_path {
        let backup_state_path = transaction.join(backup_state_path);
        if entry_exists(&backup_state_path)? {
            if let Some(current) = read_optional_file(&state_path)
                .map_err(|source| destination_write_error(destination, source))?
            {
                if current != next_state {
                    return Err(SyncError::DestinationChangedDuringSynchronization {
                        path: display_path(&state_path),
                    });
                }
                fs::remove_file(&state_path)
                    .map_err(|source| destination_write_error(destination, source))?;
                sync_directory(destination)
                    .map_err(|source| destination_write_error(destination, source))?;
            }
            fs::rename(&backup_state_path, &state_path)
                .map_err(|source| destination_write_error(destination, source))?;
            sync_directory(&backup)
                .and_then(|()| sync_directory(destination))
                .map_err(|source| destination_write_error(destination, source))?;
        } else {
            let Some(current) = read_optional_file(&state_path)
                .map_err(|source| destination_write_error(destination, source))?
            else {
                return Err(SyncError::DestinationChangedDuringSynchronization {
                    path: display_path(&state_path),
                });
            };
            let current = parse_destination_state(&current, &state_path)?;
            if previous_state.as_ref() != Some(&current) {
                return Err(SyncError::DestinationChangedDuringSynchronization {
                    path: display_path(&state_path),
                });
            }
        }
    } else if let Some(current) = read_optional_file(&state_path)
        .map_err(|source| destination_write_error(destination, source))?
    {
        if current != next_state {
            return Err(SyncError::DestinationChangedDuringSynchronization {
                path: display_path(&state_path),
            });
        }
        fs::remove_file(&state_path)
            .map_err(|source| destination_write_error(destination, source))?;
        sync_directory(destination)
            .map_err(|source| destination_write_error(destination, source))?;
    }
    sync_directory(&backup)
        .and_then(|()| sync_directory(destination))
        .map_err(|source| destination_write_error(destination, source))?;
    Ok(())
}

fn discard_rollback_entry(
    destination: &Path,
    transaction: &Path,
    operation: &TransactionOperation,
) -> Result<(), SyncError> {
    let destination_path = destination.join(&operation.destination_path);
    let discarded_directory = transaction.join("discarded");
    let discarded_path = discarded_directory.join(&operation.name);
    if entry_exists(&discarded_path)? {
        if !entry_matches_digest(&discarded_path, operation.next_digest.as_deref())?
            || entry_exists(&destination_path)?
        {
            return Err(SyncError::DestinationChangedDuringSynchronization {
                path: display_path(&destination_path),
            });
        }
        return Ok(());
    }
    if !entry_exists(&destination_path)? {
        return Ok(());
    }
    if !entry_matches_digest(&destination_path, operation.next_digest.as_deref())? {
        return Err(SyncError::DestinationChangedDuringSynchronization {
            path: display_path(&destination_path),
        });
    }
    fs::rename(&destination_path, &discarded_path)
        .map_err(|source| destination_write_error(destination, source))?;
    sync_directory(destination)
        .and_then(|()| sync_directory(&discarded_directory))
        .map_err(|source| destination_write_error(destination, source))?;
    interrupt_sync_for_test(SyncTestPoint::AfterRollbackDiscard);
    Ok(())
}

fn validate_rollback_destination(
    destination: &Path,
    journal: &TransactionJournal,
    previous_state: Option<&DestinationState>,
    next_state: &DestinationState,
) -> Result<(), SyncError> {
    let transaction = destination.join(TRANSACTION_DIRECTORY);
    let journal_path = transaction.join(TRANSACTION_JOURNAL_FILE);
    for operation in &journal.operations {
        let destination_path = destination.join(&operation.destination_path);
        let backup_exists = operation
            .backup_path
            .as_ref()
            .map(|path| entry_exists(&transaction.join(path)))
            .transpose()?
            .unwrap_or(false);
        let discarded_path = transaction.join("discarded").join(&operation.name);
        let discarded_exists = entry_exists(&discarded_path)?;
        if discarded_exists
            && (!operation.kind.has_staged_content()
                || !entry_matches_digest(&discarded_path, operation.next_digest.as_deref())?)
        {
            return Err(invalid_transaction_journal(
                &journal_path,
                format!(
                    "discarded Materialized Skill '{}' does not match its recorded content",
                    operation.name
                ),
            ));
        }
        let destination_exists = entry_exists(&destination_path)?;
        let valid = match operation.kind {
            TransactionOperationKind::Create if discarded_exists => !destination_exists,
            TransactionOperationKind::Create => {
                !destination_exists
                    || entry_matches_digest(&destination_path, operation.next_digest.as_deref())?
            }
            TransactionOperationKind::Update if discarded_exists && backup_exists => {
                !destination_exists
            }
            TransactionOperationKind::Update if discarded_exists => {
                entry_matches_previous(&destination_path, &operation.previous)?
            }
            TransactionOperationKind::Update if backup_exists => {
                !destination_exists
                    || entry_matches_digest(&destination_path, operation.next_digest.as_deref())?
            }
            TransactionOperationKind::Update
                if operation.previous == TransactionEntryState::Missing =>
            {
                !destination_exists
                    || entry_matches_digest(&destination_path, operation.next_digest.as_deref())?
            }
            TransactionOperationKind::Update => {
                entry_matches_previous(&destination_path, &operation.previous)?
            }
            TransactionOperationKind::Remove if discarded_exists => false,
            TransactionOperationKind::Remove if backup_exists => !destination_exists,
            TransactionOperationKind::Remove => {
                entry_matches_previous(&destination_path, &operation.previous)?
            }
        };
        if !valid {
            return Err(SyncError::DestinationChangedDuringSynchronization {
                path: display_path(&destination_path),
            });
        }
    }

    let state_path = destination.join(DESTINATION_STATE_FILE);
    let current_state = read_optional_file(&state_path)
        .map_err(|source| destination_write_error(destination, source))?;
    let current_state = current_state
        .as_deref()
        .map(|contents| parse_destination_state(contents, &state_path))
        .transpose()?;
    let backup_state_exists = journal
        .backup_state_path
        .as_ref()
        .map(|path| entry_exists(&transaction.join(path)))
        .transpose()?
        .unwrap_or(false);
    let valid_state = if backup_state_exists {
        current_state
            .as_ref()
            .is_none_or(|current| current == next_state)
    } else if previous_state.is_some() {
        current_state.as_ref() == previous_state
    } else {
        current_state.is_none() || current_state.as_ref() == Some(next_state)
    };
    if !valid_state {
        return Err(SyncError::DestinationChangedDuringSynchronization {
            path: display_path(&state_path),
        });
    }
    Ok(())
}

fn validate_transaction_journal(
    destination: &Path,
    journal: &TransactionJournal,
) -> Result<(), SyncError> {
    let journal_path = destination
        .join(TRANSACTION_DIRECTORY)
        .join(TRANSACTION_JOURNAL_FILE);
    if journal.next_state_path != "next-state.json"
        || journal.previous_state_path.as_deref()
            != journal
                .previous_state_path
                .as_ref()
                .map(|_| "previous-state.json")
        || journal.backup_state_path.as_deref()
            != journal
                .previous_state_path
                .as_ref()
                .map(|_| format!("backup/{DESTINATION_STATE_FILE}"))
                .as_deref()
    {
        return Err(invalid_transaction_journal(
            &journal_path,
            "transaction state paths are not valid",
        ));
    }
    let mut names = BTreeSet::new();
    for operation in &journal.operations {
        let expected_staged = format!("staged/{}", operation.name);
        let expected_backup = format!("backup/{}", operation.name);
        let valid_paths = operation.destination_path == operation.name
            && operation.staged_path.as_deref()
                == operation
                    .kind
                    .has_staged_content()
                    .then_some(expected_staged.as_str())
            && operation.backup_path.as_deref()
                == operation
                    .kind
                    .has_backup()
                    .then_some(expected_backup.as_str());
        let valid_shape = match operation.kind {
            TransactionOperationKind::Create => {
                operation.previous == TransactionEntryState::Missing
                    && operation.next_digest.is_some()
            }
            TransactionOperationKind::Update => operation.next_digest.is_some(),
            TransactionOperationKind::Remove => operation.next_digest.is_none(),
        };
        if !is_destination_name(&operation.name)
            || !names.insert(operation.name.as_str())
            || !valid_paths
            || !valid_shape
            || operation
                .next_digest
                .as_deref()
                .is_some_and(|digest| !valid_digest(digest))
            || matches!(
                &operation.previous,
                TransactionEntryState::Digest(digest) if !valid_digest(digest)
            )
            || matches!(
                &operation.previous,
                TransactionEntryState::Unsupported(fingerprint) if !valid_digest(fingerprint)
            )
        {
            return Err(invalid_transaction_journal(
                &journal_path,
                "transaction operation is not valid",
            ));
        }
    }
    Ok(())
}

fn validate_transaction_artifacts_for_commit(
    destination: &Path,
    journal: &TransactionJournal,
) -> Result<(Option<DestinationState>, DestinationState), SyncError> {
    let transaction = destination.join(TRANSACTION_DIRECTORY);
    let journal_path = transaction.join(TRANSACTION_JOURNAL_FILE);
    validate_transaction_directories(&transaction, &journal_path)?;
    let states = read_transaction_states(destination, journal)?;
    for operation in journal
        .operations
        .iter()
        .filter(|operation| operation.kind.has_staged_content())
    {
        let staged_path = transaction.join(
            operation
                .staged_path
                .as_ref()
                .expect("created and updated operations have staged content"),
        );
        if !entry_matches_digest(&staged_path, operation.next_digest.as_deref())? {
            return Err(invalid_transaction_journal(
                &journal_path,
                format!(
                    "staged Materialized Skill '{}' does not match its recorded digest",
                    operation.name
                ),
            ));
        }
    }
    validate_transaction_backups(destination, journal)?;
    Ok(states)
}

fn validate_transaction_backups(
    destination: &Path,
    journal: &TransactionJournal,
) -> Result<(), SyncError> {
    let transaction = destination.join(TRANSACTION_DIRECTORY);
    let journal_path = transaction.join(TRANSACTION_JOURNAL_FILE);
    for operation in journal
        .operations
        .iter()
        .filter(|operation| operation.kind.has_backup())
    {
        let backup_path = transaction.join(
            operation
                .backup_path
                .as_ref()
                .expect("updated and removed operations have backups"),
        );
        if entry_exists(&backup_path)?
            && !entry_matches_previous(&backup_path, &operation.previous)?
        {
            return Err(invalid_transaction_journal(
                &journal_path,
                format!(
                    "backup for Materialized Skill '{}' does not match its recorded content",
                    operation.name
                ),
            ));
        }
    }
    Ok(())
}

fn write_transaction_journal(destination: &Path, journal: &TransactionJournal) -> io::Result<()> {
    write_transaction_journal_at(&destination.join(TRANSACTION_DIRECTORY), journal)
}

fn write_transaction_journal_at(
    transaction: &Path,
    journal: &TransactionJournal,
) -> io::Result<()> {
    let contents = serde_json::to_vec_pretty(journal).expect("transaction journal is serializable");
    write_file_atomic_durable(&transaction.join(TRANSACTION_JOURNAL_FILE), &contents)
}

fn cleanup_transaction(destination: &Path, journal: &TransactionJournal) -> Result<(), SyncError> {
    cleanup_transaction_io(destination, journal)
        .map_err(|source| destination_write_error(destination, source))
}

fn cleanup_transaction_io(destination: &Path, journal: &TransactionJournal) -> io::Result<()> {
    let transaction = destination.join(TRANSACTION_DIRECTORY);
    if fs::symlink_metadata(&transaction).is_ok() {
        let destination_parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let cleanup = TempDir::new_in(destination_parent)?.keep();
        fs::remove_dir(&cleanup)?;
        fs::rename(&transaction, &cleanup)?;
        sync_directory(destination)?;
        remove_path_if_exists(&cleanup)?;
        sync_directory(destination_parent)?;
    }
    if !journal.destination_existed && fs::read_dir(destination)?.next().is_none() {
        fs::remove_dir(destination)?;
    }
    Ok(())
}

fn cleanup_after_transaction_failure(
    destination: &Path,
    journal: &TransactionJournal,
    operation: SyncError,
) -> SyncError {
    match cleanup_transaction_io(destination, journal) {
        Ok(()) => operation,
        Err(rollback) => SyncError::Rollback {
            operation: Box::new(operation),
            rollback,
        },
    }
}

fn entry_matches_previous(
    path: &Path,
    previous: &TransactionEntryState,
) -> Result<bool, SyncError> {
    match previous {
        TransactionEntryState::Missing => Ok(!entry_exists(path)?),
        TransactionEntryState::Digest(digest) => entry_matches_digest(path, Some(digest.as_str())),
        TransactionEntryState::Unsupported(fingerprint) => {
            if !entry_exists(path)? {
                return Ok(false);
            }
            Ok(destination_entry_fingerprint(path)? == *fingerprint)
        }
    }
}

fn entry_matches_digest(path: &Path, digest: Option<&str>) -> Result<bool, SyncError> {
    let Some(digest) = digest else {
        return Ok(false);
    };
    Ok(materialized_skill_contents(path)?.has_digest(digest))
}

fn valid_digest(digest: &str) -> bool {
    digest
        .strip_prefix("sha256:")
        .is_some_and(|value| is_hex_identifier(value, &[64]))
}

fn read_optional_file(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn remove_path_if_exists(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn write_file_durable(path: &Path, contents: &[u8]) -> io::Result<()> {
    fs::write(path, contents)?;
    fs::File::open(path)?.sync_all()
}

fn write_file_atomic_durable(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = NamedTempFile::new_in(parent)?;
    temporary.write_all(contents)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    sync_directory(parent)
}

fn sync_tree(path: &Path) -> io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            sync_tree(&path)?;
        } else {
            fs::File::open(&path)?.sync_all()?;
        }
    }
    sync_directory(path)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn destination_write_error(destination: &Path, source: io::Error) -> SyncError {
    SyncError::DestinationWrite {
        path: display_path(destination),
        source,
    }
}

fn invalid_transaction_journal(path: &Path, details: impl Into<String>) -> SyncError {
    SyncError::InvalidTransactionJournal {
        path: display_path(path),
        details: details.into(),
    }
}

fn sync_error_as_io(error: SyncError) -> io::Error {
    io::Error::other(error.to_string())
}

#[cfg(debug_assertions)]
fn interrupt_sync_for_test(point: SyncTestPoint) {
    if std::env::var("SKILL_MANAGER_TEST_SYNC_INTERRUPT_AT").as_deref() == Ok(point.as_str()) {
        std::process::exit(86);
    }
}

#[cfg(not(debug_assertions))]
fn interrupt_sync_for_test(_point: SyncTestPoint) {}

#[cfg(debug_assertions)]
fn request_sync_cancellation_for_test(point: SyncTestPoint) {
    if std::env::var("SKILL_MANAGER_TEST_SYNC_CANCEL_AT").as_deref() == Ok(point.as_str()) {
        CANCELLATION_REQUESTED.store(true, Ordering::SeqCst);
    }
}

#[cfg(not(debug_assertions))]
fn request_sync_cancellation_for_test(_point: SyncTestPoint) {}

fn copy_directory(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory(&source_path, &destination_path)?;
        } else {
            fs::copy(&source_path, &destination_path)?;
            fs::set_permissions(&destination_path, fs::metadata(&source_path)?.permissions())?;
        }
    }
    Ok(())
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
    write_manifest_atomic_with_cancellation(manifest_path, contents, ensure_selection_not_cancelled)
}

fn write_manifest_atomic_for_removal(
    manifest_path: &Path,
    contents: &[u8],
) -> Result<(), SelectError> {
    write_manifest_atomic_with_cancellation(manifest_path, contents, ensure_removal_not_cancelled)
}

fn write_manifest_atomic_with_cancellation(
    manifest_path: &Path,
    contents: &[u8],
    ensure_not_cancelled: fn() -> Result<(), SelectError>,
) -> Result<(), SelectError> {
    ensure_not_cancelled()?;
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
    ensure_not_cancelled()?;
    temporary
        .persist(manifest_path)
        .map_err(|error| SelectError::ManifestWrite {
            path: manifest,
            source: error.error,
        })?;
    Ok(())
}

fn ensure_removal_not_cancelled() -> Result<(), SelectError> {
    if cancellation_requested() {
        Err(SelectError::RemovalCancelled)
    } else {
        Ok(())
    }
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
