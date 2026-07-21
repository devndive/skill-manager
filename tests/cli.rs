use std::fs;
use std::process::Command;

use serde_json::json;
use tempfile::TempDir;

mod common;

use common::TestRepository;

#[test]
fn cli_prints_deterministic_human_output() {
    let repository = TestRepository::new("source-repository");
    repository.write("SKILL.md", "# Root\n");
    repository.write("nested/SKILL.md", "# Nested\n");
    let commit = repository.commit("add skills");

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args(["discover", repository.path().to_str().unwrap()])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!(
            "Source Repository: {}\nRequested revision: HEAD\nResolved commit: {commit}\nSkills:\n- source-repository (.)\n- nested (nested; parent: .)\n",
            fs::canonicalize(repository.path()).unwrap().display()
        )
    );
}

#[test]
fn cli_json_follows_the_versioned_schema_for_a_tag() {
    let repository = TestRepository::new("source-repository");
    repository.write("nested/SKILL.md", "# Nested\n");
    let commit = repository.commit("add nested skill");
    repository.git(&["tag", "v1"]);
    repository.write("future/SKILL.md", "# Future\n");
    repository.commit("add future skill");
    let source = fs::canonicalize(repository.path())
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "discover",
            repository.path().to_str().unwrap(),
            "--ref",
            "v1",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        json!({
            "schema_version": 1,
            "source": {
                "type": "local",
                "path": source,
            },
            "requested_revision": "v1",
            "resolved_commit": commit,
            "skills": [{
                "identity": {
                    "source": source,
                    "path": "nested",
                },
                "name": "nested",
                "path": "nested",
                "parent_path": null,
            }],
        })
    );
}

#[test]
fn cli_reports_when_git_is_unavailable() {
    let repository = TempDir::new().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args(["discover", repository.path().to_str().unwrap()])
        .env("PATH", "")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("error: Git is required but could not be executed")
    );
}

#[test]
fn cli_accepts_a_hyphenated_revision_value() {
    let repository = TestRepository::new("source-repository");
    repository.write("SKILL.md", "# Root\n");
    let commit = repository.commit("add root skill");
    repository.git(&["update-ref", "refs/tags/--foo", "HEAD"]);

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "discover",
            repository.path().to_str().unwrap(),
            "--ref",
            "--foo",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["resolved_commit"],
        commit
    );
}
