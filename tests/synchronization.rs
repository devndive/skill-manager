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
