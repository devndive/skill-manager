use std::fs;
use std::process::Command;

use serde_json::json;
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
