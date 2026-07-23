use std::fs;
use std::process::Command;

use serde_json::json;
use skill_manager::{SelectRequest, select};
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
            "Manifest: {}\nSynchronization Destination: {}\nCreated Skills:\n- alpha ({}:alpha; commit: {commit})\n",
            manifest_path.display(),
            destination.display(),
            fs::canonicalize(repository.path()).unwrap().display(),
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
    let source = fs::canonicalize(repository.path())
        .unwrap()
        .to_string_lossy()
        .into_owned();

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
