use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

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

        let started = Instant::now();
        let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
            .args([
                "discover",
                "https://github.com/devndive/skill-manager",
                "--json",
            ])
            .output()
            .unwrap();

        assert!(started.elapsed() < Duration::from_secs(2));
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

#[test]
fn cli_selects_all_skills_into_the_default_manifest() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha\n");
    repository.write("beta/SKILL.md", "# Beta\n");
    let commit = repository.commit("add skills");
    let working_directory = TempDir::new().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args(["select", repository.path().to_str().unwrap(), "--all"])
        .current_dir(working_directory.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!(
            "Manifest: skills.toml\nSource Repository: {}\nRequested revision: HEAD\nResolved commit: {commit}\nSkill Selection:\n- alpha (alpha)\n- beta (beta)\n",
            fs::canonicalize(repository.path()).unwrap().display()
        )
    );
    assert!(working_directory.path().join("skills.toml").is_file());
}

#[test]
fn cli_selects_repeated_exact_paths_into_a_custom_manifest_as_json() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/child/SKILL.md", "# First\n");
    repository.write("beta/child/SKILL.md", "# Second\n");
    let commit = repository.commit("add duplicate names");
    repository.git(&["tag", "v1"]);
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("selected.toml");
    let source = fs::canonicalize(repository.path())
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "select",
            repository.path().to_str().unwrap(),
            "--ref",
            "v1",
            "--select",
            "beta/child",
            "--select",
            "alpha/child",
            "--manifest",
            manifest_path.to_str().unwrap(),
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
            "manifest_path": manifest_path.to_str().unwrap(),
            "source": {
                "type": "local",
                "path": source,
            },
            "requested_revision": "v1",
            "resolved_commit": commit,
            "skills": [{
                "identity": {
                    "source": source,
                    "path": "alpha/child",
                },
                "name": "child",
                "path": "alpha/child",
                "parent_path": null,
            }, {
                "identity": {
                    "source": source,
                    "path": "beta/child",
                },
                "name": "child",
                "path": "beta/child",
                "parent_path": null,
            }],
        })
    );
    assert!(manifest_path.is_file());
}

#[test]
fn cli_invalid_selection_writes_only_a_diagnostic_and_keeps_the_manifest() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("kept/SKILL.md", "# Kept\n");
    repository.commit("add skill");
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    let original = "# existing manifest\nmanifest_version = 1\nsources = []\n";
    fs::write(&manifest_path, original).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "select",
            repository.path().to_str().unwrap(),
            "--select",
            "missing",
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
            .contains("Skill Selection contains paths not present")
    );
    assert_eq!(fs::read_to_string(manifest_path).unwrap(), original);
}

#[test]
fn cli_requires_a_terminal_when_no_non_interactive_selection_is_supplied() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("kept/SKILL.md", "# Kept\n");
    repository.commit("add skill");
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    let original = "# existing manifest\nmanifest_version = 1\nsources = []\n";
    fs::write(&manifest_path, original).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "select",
            repository.path().to_str().unwrap(),
            "--manifest",
            manifest_path.to_str().unwrap(),
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("interactive selection requires a terminal")
    );
    assert_eq!(fs::read_to_string(manifest_path).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn cli_cancelled_selection_does_not_create_a_manifest() {
    let lock = git_environment_lock();
    let fake_github = common::FakeGitHub::signal_clone("INT", lock);
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "select",
            "https://github.com/devndive/skill-manager",
            "--all",
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
            .contains("discovery was cancelled")
    );
    assert!(!manifest_path.exists());
    let clone_path = PathBuf::from(fake_github.commands()[0].last().unwrap());
    assert!(!clone_path.exists());
}
