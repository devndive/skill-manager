use std::fs;

use serde_json::json;
use skill_manager::{SelectRequest, SyncError, SyncRequest, select, sync};
use tempfile::TempDir;

mod common;

use common::{TestRepository, git_environment_lock};

#[test]
fn library_materializes_selected_local_skills_at_the_recorded_commit() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("parent/SKILL.md", "# Parent\n");
    repository.write("parent/config.json", "{\"version\":1}\n");
    repository.write("parent/nested/SKILL.md", "# Nested\n");
    repository.write("parent/nested/details.txt", "recorded\n");
    let recorded_commit = repository.commit("add selected skills");
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    let original_manifest = fs::read_to_string(&manifest_path).unwrap();

    repository.write("parent/config.json", "{\"version\":2}\n");
    repository.write("parent/nested/details.txt", "current worktree\n");
    repository.commit("change selected skills");

    let destination = manifest_directory.path().join("custom-skills");
    let result = sync(
        SyncRequest::new(&manifest_path)
            .with_destination(&destination)
            .with_force(),
    )
    .unwrap();

    assert_eq!(result.schema_version, 1);
    assert_eq!(result.manifest_path, manifest_path.to_string_lossy());
    assert_eq!(result.destination, destination.to_string_lossy());
    assert_eq!(
        result
            .created
            .iter()
            .map(|skill| (skill.name.as_str(), skill.identity.path.as_str()))
            .collect::<Vec<_>>(),
        vec![("nested", "parent/nested"), ("parent", "parent")]
    );
    assert_eq!(
        fs::read_to_string(destination.join("parent/config.json")).unwrap(),
        "{\"version\":1}\n"
    );
    assert_eq!(
        fs::read_to_string(destination.join("parent/nested/details.txt")).unwrap(),
        "recorded\n"
    );
    assert_eq!(
        fs::read_to_string(destination.join("nested/details.txt")).unwrap(),
        "recorded\n"
    );
    assert_eq!(
        fs::read_to_string(&manifest_path).unwrap(),
        original_manifest
    );

    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(destination.join(".skill-manager-state.json")).unwrap())
            .unwrap();
    assert_eq!(state["state_version"], 1);
    assert_eq!(state["owner"], "skill-manager");
    assert_eq!(
        state["managed_skills"],
        json!([
            {
                "name": "nested",
                "identity": {
                    "source": fs::canonicalize(repository.path()).unwrap(),
                    "path": "parent/nested",
                },
                "resolved_commit": recorded_commit,
                "digest": result.created[0].digest,
            },
            {
                "name": "parent",
                "identity": {
                    "source": fs::canonicalize(repository.path()).unwrap(),
                    "path": "parent",
                },
                "resolved_commit": result.created[1].resolved_commit,
                "digest": result.created[1].digest,
            },
        ])
    );
    assert!(
        result
            .created
            .iter()
            .all(|skill| skill.digest.starts_with("sha256:"))
    );

    let second_destination = manifest_directory.path().join("second-target");
    let second =
        sync(SyncRequest::new(&manifest_path).with_destination(&second_destination)).unwrap();
    assert_eq!(
        second
            .created
            .iter()
            .map(|skill| (&skill.name, &skill.digest))
            .collect::<Vec<_>>(),
        result
            .created
            .iter()
            .map(|skill| (&skill.name, &skill.digest))
            .collect::<Vec<_>>()
    );
}

#[test]
fn library_rejects_duplicate_destination_names_before_destination_changes() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/review/SKILL.md", "# First\n");
    repository.write("beta/review/SKILL.md", "# Second\n");
    repository.commit("add duplicate names");
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    let destination = manifest_directory.path().join("skills");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("keep.txt"), "unmanaged\n").unwrap();

    let error = sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap_err();

    assert!(matches!(
        error,
        SyncError::DuplicateDestinationName { name, .. } if name == "review"
    ));
    assert_eq!(
        fs::read_to_string(destination.join("keep.txt")).unwrap(),
        "unmanaged\n"
    );
    assert_eq!(
        fs::read_dir(&destination).unwrap().count(),
        1,
        "the destination must remain unchanged"
    );
}

#[test]
fn library_protects_unmanaged_destination_entries_even_with_force() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Managed candidate\n");
    repository.commit("add skill");
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    let destination = manifest_directory.path().join("skills");
    fs::create_dir(&destination).unwrap();
    fs::create_dir(destination.join("alpha")).unwrap();
    fs::write(destination.join("alpha/user.txt"), "keep me\n").unwrap();

    let error = sync(
        SyncRequest::new(&manifest_path)
            .with_destination(&destination)
            .with_force(),
    )
    .unwrap_err();

    assert!(matches!(error, SyncError::UnmanagedCollision { .. }));
    assert_eq!(
        fs::read_to_string(destination.join("alpha/user.txt")).unwrap(),
        "keep me\n"
    );
    assert!(!destination.join(".skill-manager-state.json").exists());
}

#[test]
fn library_reports_clean_materialized_skills_unchanged_without_source_access() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha\n");
    repository.commit("add skill");
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    let destination = manifest_directory.path().join("skills");
    let initial = sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap();

    fs::remove_dir_all(repository.path()).unwrap();

    let result = sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap();

    assert!(result.created.is_empty());
    assert!(result.updated.is_empty());
    assert!(result.removed.is_empty());
    assert_eq!(result.unchanged, initial.created);
    assert_eq!(
        fs::read_to_string(destination.join("alpha/SKILL.md")).unwrap(),
        "# Alpha\n"
    );
}

#[test]
fn library_reconciles_manifest_changes_and_classifies_every_materialized_skill() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha v1\n");
    repository.write("removed/SKILL.md", "# Removed\n");
    repository.commit("add initial skills");
    let steady_repository = TestRepository::new("steady-repository");
    steady_repository.write("steady/SKILL.md", "# Steady\n");
    steady_repository.commit("add steady skill");
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    select(
        SelectRequest::new(steady_repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    let destination = manifest_directory.path().join("skills");
    sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap();

    repository.write("alpha/SKILL.md", "# Alpha v2\n");
    repository.write("created/SKILL.md", "# Created\n");
    repository.commit("change selected skills");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_path("alpha")
            .select_path("created"),
    )
    .unwrap();

    let result = sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap();

    assert_eq!(
        result
            .created
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        vec!["created"]
    );
    assert_eq!(
        result
            .updated
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha"]
    );
    assert_eq!(
        result
            .removed
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        vec!["removed"]
    );
    assert_eq!(
        result
            .unchanged
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        vec!["steady"]
    );
    assert_eq!(
        fs::read_to_string(destination.join("alpha/SKILL.md")).unwrap(),
        "# Alpha v2\n"
    );
    assert_eq!(
        fs::read_to_string(destination.join("created/SKILL.md")).unwrap(),
        "# Created\n"
    );
    assert!(!destination.join("removed").exists());
    assert_eq!(
        fs::read_to_string(destination.join("steady/SKILL.md")).unwrap(),
        "# Steady\n"
    );
}

#[test]
fn library_rejects_materialized_skill_drift_without_destination_changes() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha\n");
    repository.write("beta/SKILL.md", "# Beta v1\n");
    repository.commit("add skills");
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    let destination = manifest_directory.path().join("skills");
    sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap();
    fs::write(destination.join("alpha/SKILL.md"), "# Local edit\n").unwrap();
    let original_state = fs::read(destination.join(".skill-manager-state.json")).unwrap();

    repository.write("beta/SKILL.md", "# Beta v2\n");
    repository.commit("update beta");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();

    let error = sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap_err();

    assert!(matches!(
        error,
        SyncError::MaterializedSkillDrift { skill, path }
            if skill == "alpha" && path.ends_with("skills/alpha")
    ));
    assert_eq!(
        fs::read_to_string(destination.join("alpha/SKILL.md")).unwrap(),
        "# Local edit\n"
    );
    assert_eq!(
        fs::read_to_string(destination.join("beta/SKILL.md")).unwrap(),
        "# Beta v1\n"
    );
    assert_eq!(
        fs::read(destination.join(".skill-manager-state.json")).unwrap(),
        original_state
    );
}

#[test]
fn library_force_replaces_and_removes_only_recorded_materialized_skills() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha\n");
    repository.write("removed/SKILL.md", "# Removed\n");
    repository.commit("add skills");
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    let destination = manifest_directory.path().join("skills");
    sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap();
    fs::write(destination.join("alpha/SKILL.md"), "# Local alpha\n").unwrap();
    fs::write(destination.join("removed/SKILL.md"), "# Local removed\n").unwrap();
    fs::create_dir(destination.join("unmanaged")).unwrap();
    fs::write(destination.join("unmanaged/keep.txt"), "keep\n").unwrap();
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_path("alpha"),
    )
    .unwrap();

    let result = sync(
        SyncRequest::new(&manifest_path)
            .with_destination(&destination)
            .with_force(),
    )
    .unwrap();

    assert!(result.created.is_empty());
    assert_eq!(
        result
            .updated
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha"]
    );
    assert_eq!(
        result
            .removed
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        vec!["removed"]
    );
    assert!(result.unchanged.is_empty());
    assert_eq!(
        fs::read_to_string(destination.join("alpha/SKILL.md")).unwrap(),
        "# Alpha\n"
    );
    assert!(!destination.join("removed").exists());
    assert_eq!(
        fs::read_to_string(destination.join("unmanaged/keep.txt")).unwrap(),
        "keep\n"
    );
}

#[test]
fn library_recreates_a_missing_materialized_skill_without_force() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha\n");
    repository.commit("add skill");
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    let destination = manifest_directory.path().join("skills");
    sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap();
    fs::remove_dir_all(destination.join("alpha")).unwrap();

    let result = sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap();

    assert!(result.created.is_empty());
    assert_eq!(
        result
            .updated
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha"]
    );
    assert!(result.removed.is_empty());
    assert!(result.unchanged.is_empty());
    assert_eq!(
        fs::read_to_string(destination.join("alpha/SKILL.md")).unwrap(),
        "# Alpha\n"
    );
}

#[test]
fn library_stages_all_source_content_before_reconciliation_mutation() {
    let _lock = git_environment_lock();
    let alpha_repository = TestRepository::new("alpha-repository");
    alpha_repository.write("alpha/SKILL.md", "# Alpha v1\n");
    alpha_repository.commit("add alpha");
    let zeta_repository = TestRepository::new("zeta-repository");
    zeta_repository.write("zeta/SKILL.md", "# Zeta v1\n");
    zeta_repository.commit("add zeta");
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    select(
        SelectRequest::new(alpha_repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    select(
        SelectRequest::new(zeta_repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    let destination = manifest_directory.path().join("skills");
    sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap();

    alpha_repository.write("alpha/SKILL.md", "# Alpha v2\n");
    alpha_repository.commit("update alpha");
    select(
        SelectRequest::new(alpha_repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    zeta_repository.write("zeta/SKILL.md", "# Zeta v2\n");
    zeta_repository.commit("update zeta");
    select(
        SelectRequest::new(zeta_repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    fs::remove_dir_all(zeta_repository.path()).unwrap();
    let original_state = fs::read(destination.join(".skill-manager-state.json")).unwrap();

    let error = sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap_err();

    assert!(
        matches!(error, SyncError::CommitUnavailable { repository, .. }
        if repository.ends_with("zeta-repository"))
    );
    assert_eq!(
        fs::read_to_string(destination.join("alpha/SKILL.md")).unwrap(),
        "# Alpha v1\n"
    );
    assert_eq!(
        fs::read_to_string(destination.join("zeta/SKILL.md")).unwrap(),
        "# Zeta v1\n"
    );
    assert_eq!(
        fs::read(destination.join(".skill-manager-state.json")).unwrap(),
        original_state
    );
}

#[cfg(unix)]
#[test]
fn library_staging_failure_leaves_the_complete_reconciliation_unchanged() {
    use std::os::unix::fs::symlink;

    let _lock = git_environment_lock();
    let alpha_repository = TestRepository::new("alpha-repository");
    alpha_repository.write("alpha/SKILL.md", "# Alpha v1\n");
    alpha_repository.commit("add alpha");
    let zeta_repository = TestRepository::new("zeta-repository");
    zeta_repository.write("zeta/SKILL.md", "# Zeta v1\n");
    zeta_repository.commit("add zeta");
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    for repository in [&alpha_repository, &zeta_repository] {
        select(
            SelectRequest::new(repository.path())
                .with_manifest_path(&manifest_path)
                .select_all(),
        )
        .unwrap();
    }
    let destination = manifest_directory.path().join("skills");
    sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap();

    alpha_repository.write("alpha/SKILL.md", "# Alpha v2\n");
    alpha_repository.commit("update alpha");
    select(
        SelectRequest::new(alpha_repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    symlink(
        "SKILL.md",
        zeta_repository.path().join("zeta/unsupported-link"),
    )
    .unwrap();
    zeta_repository.commit("add unsupported entry");
    select(
        SelectRequest::new(zeta_repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    let original_state = fs::read(destination.join(".skill-manager-state.json")).unwrap();

    let error = sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap_err();

    assert!(
        matches!(error, SyncError::UnsupportedTrackedEntry { skill, .. }
        if skill == "zeta")
    );
    assert_eq!(
        fs::read_to_string(destination.join("alpha/SKILL.md")).unwrap(),
        "# Alpha v1\n"
    );
    assert_eq!(
        fs::read_to_string(destination.join("zeta/SKILL.md")).unwrap(),
        "# Zeta v1\n"
    );
    assert_eq!(
        fs::read(destination.join(".skill-manager-state.json")).unwrap(),
        original_state
    );
}

#[cfg(unix)]
#[test]
fn library_filesystem_failure_leaves_materialized_skills_and_state_unchanged() {
    use std::os::unix::fs::PermissionsExt;

    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha v1\n");
    repository.commit("add alpha");
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    let destination = manifest_directory.path().join("skills");
    sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap();
    repository.write("alpha/SKILL.md", "# Alpha v2\n");
    repository.commit("update alpha");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    let original_state = fs::read(destination.join(".skill-manager-state.json")).unwrap();
    let original_permissions = fs::metadata(manifest_directory.path())
        .unwrap()
        .permissions();
    let mut read_only = original_permissions.clone();
    read_only.set_mode(0o555);
    fs::set_permissions(manifest_directory.path(), read_only).unwrap();

    let result = sync(SyncRequest::new(&manifest_path).with_destination(&destination));

    fs::set_permissions(manifest_directory.path(), original_permissions).unwrap();
    let error = result.unwrap_err();
    assert!(matches!(error, SyncError::DestinationWrite { .. }));
    assert_eq!(
        fs::read_to_string(destination.join("alpha/SKILL.md")).unwrap(),
        "# Alpha v1\n"
    );
    assert_eq!(
        fs::read(destination.join(".skill-manager-state.json")).unwrap(),
        original_state
    );
}

#[test]
fn library_detects_added_materialized_skill_content_as_drift() {
    assert_content_drift(|skill| fs::write(skill.join("local.txt"), "local\n").unwrap());
}

#[test]
fn library_detects_removed_materialized_skill_content_as_drift() {
    assert_content_drift(|skill| fs::remove_file(skill.join("SKILL.md")).unwrap());
}

#[test]
fn library_detects_an_added_empty_directory_as_materialized_skill_drift() {
    assert_content_drift(|skill| fs::create_dir(skill.join("local-empty")).unwrap());
}

#[test]
fn library_force_does_not_accept_invalid_state_as_managed_ownership() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Source alpha\n");
    let commit = repository.commit("add skill");
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    let destination = manifest_directory.path().join("skills");
    fs::create_dir_all(destination.join("alpha")).unwrap();
    fs::write(destination.join("alpha/user.txt"), "keep\n").unwrap();
    fs::write(
        destination.join(".skill-manager-state.json"),
        serde_json::to_vec_pretty(&json!({
            "state_version": 1,
            "owner": "skill-manager",
            "managed_skills": [{
                "identity": {
                    "source": fs::canonicalize(repository.path()).unwrap(),
                    "path": "alpha",
                },
                "name": "alpha",
                "resolved_commit": commit,
                "digest": "not-a-digest",
            }],
        }))
        .unwrap(),
    )
    .unwrap();

    let error = sync(
        SyncRequest::new(&manifest_path)
            .with_destination(&destination)
            .with_force(),
    )
    .unwrap_err();

    assert!(matches!(error, SyncError::InvalidDestinationState { .. }));
    assert_eq!(
        fs::read_to_string(destination.join("alpha/user.txt")).unwrap(),
        "keep\n"
    );
}

fn assert_content_drift(mutate: impl FnOnce(&std::path::Path)) {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha\n");
    repository.commit("add skill");
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    let destination = manifest_directory.path().join("skills");
    sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap();
    mutate(&destination.join("alpha"));

    let error = sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap_err();

    assert!(matches!(
        error,
        SyncError::MaterializedSkillDrift { skill, .. } if skill == "alpha"
    ));
}
