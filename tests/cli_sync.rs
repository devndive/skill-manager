use std::fs;
use std::process::Command;
#[cfg(unix)]
use std::{path::Path, process::Output};

use serde_json::json;
#[cfg(unix)]
use skill_manager::SyncError;
use skill_manager::{SelectRequest, SyncRequest, select, sync};
use tempfile::TempDir;

mod common;

use common::{TestRepository, git_environment_lock};

#[test]
fn cli_sync_uses_the_manifest_relative_default_destination() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha\n");
    let commit = repository.commit("add skill");
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args(["sync", "--manifest", manifest_path.to_str().unwrap()])
        .current_dir(repository.path())
        .output()
        .unwrap();

    let destination = manifest_directory.path().join(".agents/skills");
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!(
            "Manifest: {}\nSynchronization Destination: {}\nCreated Materialized Skills:\n- alpha ({}:alpha; commit: {commit})\nUpdated Materialized Skills: none\nRemoved Materialized Skills: none\nUnchanged Materialized Skills: none\n",
            manifest_path.display(),
            destination.display(),
            repository.source_repository_path(),
        )
    );
    assert!(destination.join("alpha/SKILL.md").is_file());
}

#[test]
fn cli_sync_accepts_an_explicit_target_and_emits_versioned_json() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha\n");
    let commit = repository.commit("add skill");
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    let target = manifest_directory.path().join("custom");
    let source = repository.source_repository_path();

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "sync",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--target",
            target.to_str().unwrap(),
            "--force",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "");
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        result,
        json!({
            "schema_version": 1,
            "manifest_path": manifest_path.to_str().unwrap(),
            "destination": target.to_str().unwrap(),
            "created": [{
                "identity": {
                    "source": source,
                    "path": "alpha",
                },
                "name": "alpha",
                "resolved_commit": commit,
                "digest": result["created"][0]["digest"],
            }],
            "updated": [],
            "removed": [],
            "unchanged": [],
        })
    );
    assert!(target.join("alpha/SKILL.md").is_file());
    assert!(!manifest_directory.path().join(".agents/skills").exists());
}

#[test]
fn cli_sync_failure_writes_no_success_shaped_json() {
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    fs::write(
        &manifest_path,
        "manifest_version = 1\n\n[[sources]]\ntype = \"local\"\npath = \"missing\"\nrequested_revision = \"HEAD\"\nresolved_commit = \"0123456789abcdef\"\nskills = [\"alpha\"]\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "sync",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("resolved commit '0123456789abcdef' is unavailable")
    );
    assert!(!manifest_directory.path().join(".agents/skills").exists());
}

#[test]
fn cli_sync_reports_when_git_is_unavailable() {
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    fs::write(
        &manifest_path,
        "manifest_version = 1\n\n[[sources]]\ntype = \"local\"\npath = \".\"\nrequested_revision = \"HEAD\"\nresolved_commit = \"0123456789abcdef\"\nskills = [\"alpha\"]\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args(["sync", "--manifest", manifest_path.to_str().unwrap()])
        .env("PATH", "")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("Git is required but could not be executed")
    );
}

#[test]
fn cli_sync_json_reports_complete_reconciliation() {
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
    let human_destination = manifest_directory.path().join("human-skills");
    sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap();
    sync(SyncRequest::new(&manifest_path).with_destination(&human_destination)).unwrap();

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

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "sync",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--target",
            destination.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "");
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for (field, expected) in [
        ("created", "created"),
        ("updated", "alpha"),
        ("removed", "removed"),
        ("unchanged", "steady"),
    ] {
        assert_eq!(result[field][0]["name"], expected);
        assert_eq!(result[field].as_array().unwrap().len(), 1);
    }

    let human_output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "sync",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--target",
            human_destination.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(human_output.status.success());
    assert_eq!(String::from_utf8(human_output.stderr).unwrap(), "");
    let human_output = String::from_utf8(human_output.stdout).unwrap();
    for expected in [
        "Created Materialized Skills:\n- created (",
        "Updated Materialized Skills:\n- alpha (",
        "Removed Materialized Skills:\n- removed (",
        "Unchanged Materialized Skills:\n- steady (",
    ] {
        assert!(human_output.contains(expected), "missing '{expected}'");
    }
}

#[test]
fn cli_sync_drift_fails_without_force_and_is_replaced_with_force() {
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
    fs::write(destination.join("alpha/SKILL.md"), "# Local edit\n").unwrap();

    let failure = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "sync",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--target",
            destination.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();

    assert!(!failure.status.success());
    assert_eq!(String::from_utf8(failure.stdout).unwrap(), "");
    assert!(
        String::from_utf8(failure.stderr)
            .unwrap()
            .contains("Materialized Skill 'alpha' has drift")
    );
    assert_eq!(
        fs::read_to_string(destination.join("alpha/SKILL.md")).unwrap(),
        "# Local edit\n"
    );

    let forced = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "sync",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--target",
            destination.to_str().unwrap(),
            "--force",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(forced.status.success());
    let result: serde_json::Value = serde_json::from_slice(&forced.stdout).unwrap();
    assert_eq!(result["updated"][0]["name"], "alpha");
    assert_eq!(
        fs::read_to_string(destination.join("alpha/SKILL.md")).unwrap(),
        "# Alpha\n"
    );
}

#[test]
fn cli_sync_reports_unchanged_when_the_source_repository_is_offline() {
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
    fs::remove_dir_all(repository.path()).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "sync",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--target",
            destination.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["created"], json!([]));
    assert_eq!(result["updated"], json!([]));
    assert_eq!(result["removed"], json!([]));
    assert_eq!(result["unchanged"][0]["name"], "alpha");
}

#[test]
fn cli_sync_source_failure_leaves_the_complete_reconciliation_unchanged() {
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

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "sync",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--target",
            destination.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
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
fn cli_sync_recovers_an_interruption_after_old_entries_are_backed_up() {
    let _lock = git_environment_lock();
    assert_interrupted_sync_recovers("after-old-entries", Some("after-new-entries"));
}

#[cfg(unix)]
#[test]
fn cli_sync_recovers_interruptions_at_every_other_commit_phase() {
    let _lock = git_environment_lock();
    for point in [
        "after-journal",
        "after-new-entries",
        "after-old-state",
        "after-new-state",
    ] {
        assert_interrupted_sync_recovers(point, None);
    }
}

#[cfg(unix)]
fn assert_interrupted_sync_recovers(point: &str, recovery_interrupt: Option<&str>) {
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha v1\n");
    repository.write("removed/SKILL.md", "# Removed\n");
    repository.commit("add initial skills");
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
    fs::create_dir(destination.join("unmanaged")).unwrap();
    fs::write(destination.join("unmanaged/keep.txt"), "keep\n").unwrap();

    repository.write("alpha/SKILL.md", "# Alpha v2\n");
    repository.write("created/SKILL.md", "# Created\n");
    repository.commit("change skills");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_path("alpha")
            .select_path("created"),
    )
    .unwrap();

    interrupt_sync(&manifest_path, &destination, point, false);
    assert!(
        destination
            .join(".skill-manager-transaction/journal.json")
            .is_file()
    );

    let journal: serde_json::Value = serde_json::from_slice(
        &fs::read(destination.join(".skill-manager-transaction/journal.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(journal["journal_version"], 1);
    assert_eq!(journal["phase"], "committing");
    assert_eq!(journal["next_state_path"], "next-state.json");
    assert_eq!(journal["operations"].as_array().unwrap().len(), 3);

    if let Some(recovery_interrupt) = recovery_interrupt {
        interrupt_sync(&manifest_path, &destination, recovery_interrupt, false);
        assert!(
            destination
                .join(".skill-manager-transaction/journal.json")
                .is_file()
        );
    }

    sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap();

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
        fs::read_to_string(destination.join("unmanaged/keep.txt")).unwrap(),
        "keep\n"
    );
    assert!(!destination.join(".skill-manager-transaction").exists());
}

#[cfg(unix)]
fn interrupt_sync(manifest_path: &Path, destination: &Path, point: &str, force: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_skill-manager"));
    command.args([
        "sync",
        "--manifest",
        manifest_path.to_str().unwrap(),
        "--target",
        destination.to_str().unwrap(),
    ]);
    if force {
        command.arg("--force");
    }
    let output = command
        .env("SKILL_MANAGER_TEST_SYNC_INTERRUPT_AT", point)
        .output()
        .unwrap();
    assert!(!output.status.success());
    output
}

#[cfg(unix)]
#[test]
fn cli_sync_cancellation_during_staging_leaves_no_destination_or_temporary_changes() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha v1\n");
    repository.write("beta/SKILL.md", "# Beta v1\n");
    repository.commit("add initial skills");
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
    let original_state = fs::read(destination.join(".skill-manager-state.json")).unwrap();
    repository.write("alpha/SKILL.md", "# Alpha v2\n");
    repository.write("beta/SKILL.md", "# Beta v2\n");
    repository.commit("update skills");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    let temporary_root = TempDir::new().unwrap();

    let cancelled = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "sync",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--target",
            destination.to_str().unwrap(),
        ])
        .env("TMPDIR", temporary_root.path())
        .env("SKILL_MANAGER_TEST_SYNC_CANCEL_AT", "during-staging")
        .output()
        .unwrap();

    assert!(!cancelled.status.success());
    assert!(
        String::from_utf8(cancelled.stderr)
            .unwrap()
            .contains("Skill Synchronization was cancelled")
    );
    assert_eq!(
        fs::read_to_string(destination.join("alpha/SKILL.md")).unwrap(),
        "# Alpha v1\n"
    );
    assert_eq!(
        fs::read_to_string(destination.join("beta/SKILL.md")).unwrap(),
        "# Beta v1\n"
    );
    assert_eq!(
        fs::read(destination.join(".skill-manager-state.json")).unwrap(),
        original_state
    );
    assert!(!destination.join(".skill-manager-transaction").exists());
    assert_eq!(fs::read_dir(temporary_root.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn cli_sync_recovers_an_interrupted_first_materialization() {
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

    interrupt_sync(&manifest_path, &destination, "after-journal", false);
    assert!(
        destination
            .join(".skill-manager-transaction/journal.json")
            .is_file()
    );
    assert!(!destination.join("alpha").exists());

    sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap();

    assert_eq!(
        fs::read_to_string(destination.join("alpha/SKILL.md")).unwrap(),
        "# Alpha\n"
    );
    assert!(destination.join(".skill-manager-state.json").is_file());
    assert!(!destination.join(".skill-manager-transaction").exists());
}

#[cfg(unix)]
#[test]
fn cli_sync_cancellation_during_commit_restores_the_previous_state() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha v1\n");
    repository.write("removed/SKILL.md", "# Removed\n");
    repository.commit("add initial skills");
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
    fs::write(destination.join("keep.txt"), "unmanaged\n").unwrap();
    let original_state = fs::read(destination.join(".skill-manager-state.json")).unwrap();
    repository.write("alpha/SKILL.md", "# Alpha v2\n");
    repository.write("created/SKILL.md", "# Created\n");
    repository.commit("change skills");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_path("alpha")
            .select_path("created"),
    )
    .unwrap();

    let cancelled = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "sync",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--target",
            destination.to_str().unwrap(),
        ])
        .env("SKILL_MANAGER_TEST_SYNC_CANCEL_AT", "after-old-entries")
        .output()
        .unwrap();

    assert!(!cancelled.status.success());
    assert!(
        String::from_utf8(cancelled.stderr)
            .unwrap()
            .contains("Skill Synchronization was cancelled")
    );
    assert_eq!(
        fs::read_to_string(destination.join("alpha/SKILL.md")).unwrap(),
        "# Alpha v1\n"
    );
    assert_eq!(
        fs::read_to_string(destination.join("removed/SKILL.md")).unwrap(),
        "# Removed\n"
    );
    assert!(!destination.join("created").exists());
    assert_eq!(
        fs::read_to_string(destination.join("keep.txt")).unwrap(),
        "unmanaged\n"
    );
    assert_eq!(
        fs::read(destination.join(".skill-manager-state.json")).unwrap(),
        original_state
    );
    assert!(!destination.join(".skill-manager-transaction").exists());
}

#[cfg(unix)]
#[test]
fn cli_sync_cancellation_after_recreating_a_missing_skill_restores_the_missing_state() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha v1\n");
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
    let original_state = fs::read(destination.join(".skill-manager-state.json")).unwrap();
    fs::remove_dir_all(destination.join("alpha")).unwrap();
    repository.write("alpha/SKILL.md", "# Alpha v2\n");
    repository.commit("update skill");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();

    let cancelled = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "sync",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--target",
            destination.to_str().unwrap(),
        ])
        .env("SKILL_MANAGER_TEST_SYNC_CANCEL_AT", "after-new-entries")
        .output()
        .unwrap();

    assert!(!cancelled.status.success());
    assert!(!destination.join("alpha").exists());
    assert_eq!(
        fs::read(destination.join(".skill-manager-state.json")).unwrap(),
        original_state
    );
    assert!(!destination.join(".skill-manager-transaction").exists());
}

#[cfg(unix)]
#[test]
fn cli_sync_recovery_preserves_drift_created_after_interruption() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha v1\n");
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
    fs::write(destination.join("keep.txt"), "unmanaged\n").unwrap();
    repository.write("alpha/SKILL.md", "# Alpha v2\n");
    repository.commit("update skill");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    interrupt_sync(&manifest_path, &destination, "after-journal", false);
    fs::write(destination.join("alpha/SKILL.md"), "# Local edit\n").unwrap();

    let error = sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap_err();

    assert!(matches!(
        error,
        SyncError::DestinationChangedDuringSynchronization { .. }
    ));
    assert_eq!(
        fs::read_to_string(destination.join("alpha/SKILL.md")).unwrap(),
        "# Local edit\n"
    );
    assert_eq!(
        fs::read_to_string(destination.join("keep.txt")).unwrap(),
        "unmanaged\n"
    );
    assert!(
        destination
            .join(".skill-manager-transaction/journal.json")
            .is_file()
    );
}

#[cfg(unix)]
#[test]
fn cli_sync_recovery_preflights_all_entries_before_mutation() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha v1\n");
    repository.write("zeta/SKILL.md", "# Zeta v1\n");
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
    let original_state = fs::read(destination.join(".skill-manager-state.json")).unwrap();
    repository.write("alpha/SKILL.md", "# Alpha v2\n");
    repository.write("zeta/SKILL.md", "# Zeta v2\n");
    repository.commit("update skills");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    interrupt_sync(&manifest_path, &destination, "after-journal", false);
    fs::write(destination.join("zeta/SKILL.md"), "# Local zeta\n").unwrap();

    let error = sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap_err();

    assert!(matches!(
        error,
        SyncError::DestinationChangedDuringSynchronization { .. }
    ));
    assert_eq!(
        fs::read_to_string(destination.join("alpha/SKILL.md")).unwrap(),
        "# Alpha v1\n"
    );
    assert_eq!(
        fs::read_to_string(destination.join("zeta/SKILL.md")).unwrap(),
        "# Local zeta\n"
    );
    assert_eq!(
        fs::read(destination.join(".skill-manager-state.json")).unwrap(),
        original_state
    );
}

#[cfg(unix)]
#[test]
fn cli_sync_recovery_preserves_changed_unsupported_drift_from_a_forced_transaction() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha v1\n");
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
    fs::write(destination.join("alpha"), "first unsupported contents\n").unwrap();
    repository.write("alpha/SKILL.md", "# Alpha v2\n");
    repository.commit("update skill");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();

    interrupt_sync(&manifest_path, &destination, "after-journal", true);
    fs::write(destination.join("alpha"), "changed after interruption\n").unwrap();

    let error = sync(
        SyncRequest::new(&manifest_path)
            .with_destination(&destination)
            .with_force(),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        SyncError::DestinationChangedDuringSynchronization { .. }
    ));
    assert_eq!(
        fs::read_to_string(destination.join("alpha")).unwrap(),
        "changed after interruption\n"
    );
    assert!(
        destination
            .join(".skill-manager-transaction/journal.json")
            .is_file()
    );
}

#[cfg(unix)]
#[test]
fn cli_sync_recovery_rejects_a_next_state_inconsistent_with_the_journal() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha v1\n");
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
    fs::write(destination.join("keep.txt"), "unmanaged\n").unwrap();
    repository.write("alpha/SKILL.md", "# Alpha v2\n");
    repository.commit("update skill");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    interrupt_sync(&manifest_path, &destination, "after-journal", false);
    let next_state_path = destination.join(".skill-manager-transaction/next-state.json");
    let mut next_state: serde_json::Value =
        serde_json::from_slice(&fs::read(&next_state_path).unwrap()).unwrap();
    let mut unrelated = next_state["managed_skills"][0].clone();
    unrelated["name"] = json!("unrelated");
    next_state["managed_skills"]
        .as_array_mut()
        .unwrap()
        .push(unrelated);
    fs::write(
        &next_state_path,
        serde_json::to_vec_pretty(&next_state).unwrap(),
    )
    .unwrap();

    let error = sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap_err();

    assert!(matches!(error, SyncError::InvalidTransactionJournal { .. }));
    assert_eq!(
        fs::read_to_string(destination.join("alpha/SKILL.md")).unwrap(),
        "# Alpha v1\n"
    );
    assert_eq!(
        fs::read_to_string(destination.join("keep.txt")).unwrap(),
        "unmanaged\n"
    );
    assert!(
        destination
            .join(".skill-manager-transaction/journal.json")
            .is_file()
    );
}

#[cfg(unix)]
#[test]
fn cli_sync_rolling_back_rejects_a_corrupt_late_backup_before_restoring_any_entry() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha v1\n");
    repository.write("zeta/SKILL.md", "# Zeta v1\n");
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
    repository.write("alpha/SKILL.md", "# Alpha v2\n");
    repository.write("zeta/SKILL.md", "# Zeta v2\n");
    repository.commit("update skills");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    interrupt_sync(&manifest_path, &destination, "after-old-entries", false);
    let journal_path = destination.join(".skill-manager-transaction/journal.json");
    let mut journal: serde_json::Value =
        serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
    journal["phase"] = json!("rolling_back");
    fs::write(&journal_path, serde_json::to_vec_pretty(&journal).unwrap()).unwrap();
    fs::write(
        destination.join(".skill-manager-transaction/backup/alpha/SKILL.md"),
        "# Corrupt alpha\n",
    )
    .unwrap();

    let error = sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap_err();

    assert!(matches!(error, SyncError::InvalidTransactionJournal { .. }));
    assert!(!destination.join("alpha").exists());
    assert!(!destination.join("zeta").exists());
    assert!(
        destination
            .join(".skill-manager-transaction/backup/zeta/SKILL.md")
            .is_file()
    );
}

#[cfg(unix)]
#[test]
fn cli_sync_rejects_a_malformed_preparing_transaction_without_cleanup() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha v1\n");
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
    repository.write("alpha/SKILL.md", "# Alpha v2\n");
    repository.commit("update skill");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    interrupt_sync(&manifest_path, &destination, "after-journal", false);
    let journal_path = destination.join(".skill-manager-transaction/journal.json");
    let mut journal: serde_json::Value =
        serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
    journal["phase"] = json!("preparing");
    fs::write(&journal_path, serde_json::to_vec_pretty(&journal).unwrap()).unwrap();
    fs::remove_file(destination.join(".skill-manager-transaction/next-state.json")).unwrap();

    let error = sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap_err();

    assert!(matches!(error, SyncError::InvalidTransactionJournal { .. }));
    assert_eq!(
        fs::read_to_string(destination.join("alpha/SKILL.md")).unwrap(),
        "# Alpha v1\n"
    );
    assert!(journal_path.is_file());
}

#[cfg(unix)]
#[test]
fn cli_sync_rejects_a_preparing_phase_that_already_contains_backups() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha v1\n");
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
    repository.write("alpha/SKILL.md", "# Alpha v2\n");
    repository.commit("update skill");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    interrupt_sync(&manifest_path, &destination, "after-old-entries", false);
    let journal_path = destination.join(".skill-manager-transaction/journal.json");
    let mut journal: serde_json::Value =
        serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
    journal["phase"] = json!("preparing");
    fs::write(&journal_path, serde_json::to_vec_pretty(&journal).unwrap()).unwrap();

    let error = sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap_err();

    assert!(matches!(error, SyncError::InvalidTransactionJournal { .. }));
    assert!(!destination.join("alpha").exists());
    assert!(
        destination
            .join(".skill-manager-transaction/backup/alpha/SKILL.md")
            .is_file()
    );
    assert!(journal_path.is_file());
}

#[cfg(unix)]
#[test]
fn cli_sync_recovers_an_interruption_while_discarding_rollback_content() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha v1\n");
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
    repository.write("alpha/SKILL.md", "# Alpha v2\n");
    repository.commit("update skill");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();

    let interrupted = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "sync",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--target",
            destination.to_str().unwrap(),
        ])
        .env("SKILL_MANAGER_TEST_SYNC_CANCEL_AT", "after-new-entries")
        .env(
            "SKILL_MANAGER_TEST_SYNC_INTERRUPT_AT",
            "after-rollback-discard",
        )
        .output()
        .unwrap();

    assert!(!interrupted.status.success());
    assert!(!destination.join("alpha").exists());
    assert!(
        destination
            .join(".skill-manager-transaction/discarded/alpha/SKILL.md")
            .is_file()
    );
    fs::remove_dir_all(repository.path()).unwrap();

    let error = sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap_err();

    assert!(matches!(error, SyncError::CommitUnavailable { .. }));
    assert_eq!(
        fs::read_to_string(destination.join("alpha/SKILL.md")).unwrap(),
        "# Alpha v1\n"
    );
    assert!(!destination.join(".skill-manager-transaction").exists());
}

#[cfg(unix)]
#[test]
fn cli_sync_rejects_symlinked_transaction_directories_without_external_mutation() {
    use std::os::unix::fs::symlink;

    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha v1\n");
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
    repository.write("alpha/SKILL.md", "# Alpha v2\n");
    repository.commit("update skill");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    interrupt_sync(&manifest_path, &destination, "after-journal", false);
    let external = TempDir::new().unwrap();
    fs::write(external.path().join("keep.txt"), "external\n").unwrap();
    let backup = destination.join(".skill-manager-transaction/backup");
    fs::remove_dir(&backup).unwrap();
    symlink(external.path(), &backup).unwrap();

    let error = sync(SyncRequest::new(&manifest_path).with_destination(&destination)).unwrap_err();

    assert!(matches!(error, SyncError::InvalidTransactionJournal { .. }));
    assert_eq!(
        fs::read_to_string(destination.join("alpha/SKILL.md")).unwrap(),
        "# Alpha v1\n"
    );
    assert_eq!(
        fs::read_to_string(external.path().join("keep.txt")).unwrap(),
        "external\n"
    );
}
