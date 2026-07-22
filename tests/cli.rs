use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::json;
use tempfile::TempDir;

mod common;

use common::{TestRepository, git_environment_lock};

#[test]
fn cli_prints_deterministic_human_output() {
    let _lock = git_environment_lock();
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
    let _lock = git_environment_lock();
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
    let _lock = git_environment_lock();
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
fn cli_reports_when_git_is_unavailable_for_github() {
    let _lock = git_environment_lock();

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args(["discover", "https://github.com/devndive/skill-manager"])
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
    let _lock = git_environment_lock();
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

#[cfg(unix)]
#[test]
fn cli_json_uses_the_normalized_github_skill_identity() {
    let lock = git_environment_lock();
    let repository = TestRepository::new("remote-source");
    repository.write("SKILL.md", "# Root\n");
    repository.write("nested/SKILL.md", "# Nested\n");
    let commit = repository.commit("add remote skills");
    let _fake_github = common::FakeGitHub::new(repository.path(), lock);

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "discover",
            "https://GitHub.com/DevNDive/Skill-Manager.git",
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
                "type": "github",
                "path": "https://github.com/devndive/skill-manager",
            },
            "requested_revision": "HEAD",
            "resolved_commit": commit,
            "skills": [{
                "identity": {
                    "source": "https://github.com/devndive/skill-manager",
                    "path": ".",
                },
                "name": "skill-manager",
                "path": ".",
                "parent_path": null,
            }, {
                "identity": {
                    "source": "https://github.com/devndive/skill-manager",
                    "path": "nested",
                },
                "name": "nested",
                "path": "nested",
                "parent_path": ".",
            }],
        })
    );
}

#[cfg(unix)]
#[test]
fn cli_prints_deterministic_human_output_for_github() {
    let lock = git_environment_lock();
    let repository = TestRepository::new("remote-source");
    repository.write("SKILL.md", "# Root\n");
    repository.write("nested/SKILL.md", "# Nested\n");
    let commit = repository.commit("add remote skills");
    let _fake_github = common::FakeGitHub::new(repository.path(), lock);

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args(["discover", "https://github.com/devndive/skill-manager"])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!(
            "Source Repository: https://github.com/devndive/skill-manager\nRequested revision: HEAD\nResolved commit: {commit}\nSkills:\n- skill-manager (.)\n- nested (nested; parent: .)\n"
        )
    );
}

#[cfg(unix)]
#[test]
fn cli_remote_revision_failures_never_print_success_output() {
    let lock = git_environment_lock();
    let repository = TestRepository::new("remote-source");
    repository.write("SKILL.md", "# Root\n");
    repository.commit("add remote skill");
    let _fake_github = common::FakeGitHub::new(repository.path(), lock);

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "discover",
            "https://github.com/devndive/skill-manager",
            "--ref",
            "missing",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("revision 'missing' is unavailable")
    );
}

#[cfg(unix)]
#[test]
fn cli_cleans_the_temporary_clone_when_remote_discovery_is_cancelled() {
    for signal in ["INT", "TERM"] {
        let lock = git_environment_lock();
        let fake_github = common::FakeGitHub::signal_clone(signal, lock);

        let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
            .args([
                "discover",
                "https://github.com/devndive/skill-manager",
                "--json",
            ])
            .output()
            .unwrap();

        assert!(!output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("discovery was cancelled")
        );
        let clone_path = PathBuf::from(fake_github.commands()[0].last().unwrap());
        assert!(!clone_path.exists());
    }
}
