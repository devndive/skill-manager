use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::json;
use tempfile::TempDir;

#[test]
fn cli_lists_selected_skills_without_accessing_the_source_repository() {
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    fs::write(
        &manifest_path,
        "manifest_version = 1\n\n[[sources]]\ntype = \"local\"\npath = \"missing-source\"\nrequested_revision = \"release\"\nresolved_commit = \"0123456789abcdef\"\nskills = [\"beta/child\", \"alpha\"]\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .arg("list")
        .current_dir(manifest_directory.path())
        .env("PATH", "")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!(
            "Manifest: {}\nSource Repository: {}\nRequested revision: release\nResolved commit: 0123456789abcdef\nSkills:\n- alpha (alpha)\n- child (beta/child)\n",
            "skills.toml",
            fs::canonicalize(manifest_directory.path())
                .unwrap()
                .join("missing-source")
                .display(),
        )
    );
}

#[test]
fn cli_lists_multiple_sources_as_deterministically_ordered_json() {
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    fs::write(
        &manifest_path,
        "manifest_version = 1\n\n[[sources]]\ntype = \"github\"\npath = \"https://github.com/devndive/skill-manager\"\nrequested_revision = \"main\"\nresolved_commit = \"bbbb\"\nskills = [\"nested\", \".\"]\n\n[[sources]]\ntype = \"local\"\npath = \"../local-source\"\nrequested_revision = \"HEAD\"\nresolved_commit = \"aaaa\"\nskills = [\"zeta\", \"alpha\"]\n",
    )
    .unwrap();
    let local_source = fs::canonicalize(manifest_directory.path())
        .unwrap()
        .parent()
        .unwrap()
        .join("local-source");

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "list",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--json",
        ])
        .env("PATH", "")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        json!({
            "schema_version": 1,
            "manifest_path": manifest_path.to_str().unwrap(),
            "sources": [{
                "source": {
                    "type": "local",
                    "path": local_source.to_str().unwrap(),
                },
                "requested_revision": "HEAD",
                "resolved_commit": "aaaa",
                "skills": [{
                    "identity": {
                        "source": local_source.to_str().unwrap(),
                        "path": "alpha",
                    },
                    "name": "alpha",
                    "path": "alpha",
                }, {
                    "identity": {
                        "source": local_source.to_str().unwrap(),
                        "path": "zeta",
                    },
                    "name": "zeta",
                    "path": "zeta",
                }],
            }, {
                "source": {
                    "type": "github",
                    "path": "https://github.com/devndive/skill-manager",
                },
                "requested_revision": "main",
                "resolved_commit": "bbbb",
                "skills": [{
                    "identity": {
                        "source": "https://github.com/devndive/skill-manager",
                        "path": ".",
                    },
                    "name": "skill-manager",
                    "path": ".",
                }, {
                    "identity": {
                        "source": "https://github.com/devndive/skill-manager",
                        "path": "nested",
                    },
                    "name": "nested",
                    "path": "nested",
                }],
            }],
        })
    );
}

#[test]
fn cli_removes_a_canonical_github_source_without_accessing_it() {
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    let original = "# manifest comment\nmanifest_version = 1\n\n# remove this source\n[[sources]]\ntype = \"github\"\npath = \"https://github.com/devndive/skill-manager\"\nrequested_revision = \"main\"\nresolved_commit = \"bbbb\"\nskills = [\"nested\"]\n\n# keep this source\n[[sources]]\ntype = \"local\"\npath = \"kept-source\"\nrequested_revision = \"HEAD\"\nresolved_commit = \"aaaa\"\nskills = [\"kept\"] # keep formatting\n";
    fs::write(&manifest_path, original).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "remove",
            "https://GitHub.com/DevNDive/Skill-Manager.git/",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--yes",
        ])
        .env("PATH", "")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!(
            "Removed Source Repository: https://github.com/devndive/skill-manager\nManifest: {}\n",
            manifest_path.display()
        )
    );
    assert_eq!(
        fs::read_to_string(manifest_path).unwrap(),
        "# manifest comment\nmanifest_version = 1\n\n# keep this source\n[[sources]]\ntype = \"local\"\npath = \"kept-source\"\nrequested_revision = \"HEAD\"\nresolved_commit = \"aaaa\"\nskills = [\"kept\"] # keep formatting\n"
    );
}

#[test]
fn cli_confirms_removal_of_a_canonical_local_source() {
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    fs::write(
        &manifest_path,
        "manifest_version = 1\n\n[[sources]]\ntype = \"local\"\npath = \"source\"\nrequested_revision = \"HEAD\"\nresolved_commit = \"aaaa\"\nskills = [\"skill\"]\n",
    )
    .unwrap();
    let canonical_source = fs::canonicalize(manifest_directory.path())
        .unwrap()
        .join("source");

    let output = run_cli_with_stdin(
        &[
            "remove",
            "./missing/../source",
            "--manifest",
            manifest_path.to_str().unwrap(),
        ],
        manifest_directory.path(),
        "y\n",
    );

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        format!(
            "Remove Source Repository '{}' from the Skill Selection manifest? [y/N] ",
            canonical_source.display()
        )
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!(
            "Removed Source Repository: {}\nManifest: {}\n",
            canonical_source.display(),
            manifest_path.display()
        )
    );
    assert_eq!(
        fs::read_to_string(manifest_path).unwrap(),
        "manifest_version = 1\n"
    );
}

#[test]
fn cli_declines_removal_without_changing_the_manifest() {
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    let original = "# keep\nmanifest_version = 1\n\n[[sources]]\ntype = \"github\"\npath = \"https://github.com/devndive/skill-manager\"\nrequested_revision = \"main\"\nresolved_commit = \"bbbb\"\nskills = [\"nested\"]\n";
    fs::write(&manifest_path, original).unwrap();

    let output = run_cli_with_stdin(
        &[
            "remove",
            "https://github.com/devndive/skill-manager",
            "--manifest",
            manifest_path.to_str().unwrap(),
        ],
        manifest_directory.path(),
        "n\n",
    );

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "Remove Source Repository 'https://github.com/devndive/skill-manager' from the Skill Selection manifest? [y/N] "
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Skill Selection unchanged.\n"
    );
    assert_eq!(fs::read_to_string(manifest_path).unwrap(), original);
}

#[test]
fn cli_reports_a_missing_source_without_changing_the_manifest() {
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    let original = "manifest_version = 1\n\n[[sources]]\ntype = \"github\"\npath = \"https://github.com/devndive/skill-manager\"\nrequested_revision = \"main\"\nresolved_commit = \"bbbb\"\nskills = [\"nested\"]\n";
    fs::write(&manifest_path, original).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "remove",
            "https://github.com/devndive/missing",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--yes",
        ])
        .env("PATH", "")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("is not selected in manifest")
    );
    assert_eq!(fs::read_to_string(manifest_path).unwrap(), original);
}

#[test]
fn cli_reports_malformed_and_unsupported_manifests_without_success_output() {
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");

    for (contents, arguments, diagnostic) in [
        (
            "manifest_version = [\n",
            vec!["list", "--manifest", manifest_path.to_str().unwrap()],
            "could not parse Skill Selection manifest",
        ),
        (
            "# future\nmanifest_version = 2\nsources = []\n",
            vec![
                "remove",
                "https://github.com/devndive/skill-manager",
                "--manifest",
                manifest_path.to_str().unwrap(),
                "--yes",
            ],
            "uses unsupported manifest version 2",
        ),
    ] {
        fs::write(&manifest_path, contents).unwrap();

        let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
            .args(arguments)
            .env("PATH", "")
            .output()
            .unwrap();

        assert!(!output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains(diagnostic)
        );
        assert_eq!(fs::read_to_string(&manifest_path).unwrap(), contents);
    }
}

#[cfg(unix)]
#[test]
fn cli_remove_write_failure_leaves_the_manifest_unchanged() {
    use std::os::unix::fs::PermissionsExt;

    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    let original = "manifest_version = 1\n\n[[sources]]\ntype = \"github\"\npath = \"https://github.com/devndive/skill-manager\"\nrequested_revision = \"main\"\nresolved_commit = \"bbbb\"\nskills = [\"nested\"]\n";
    fs::write(&manifest_path, original).unwrap();
    fs::set_permissions(manifest_directory.path(), fs::Permissions::from_mode(0o555)).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "remove",
            "https://github.com/devndive/skill-manager",
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--yes",
        ])
        .env("PATH", "")
        .output()
        .unwrap();

    fs::set_permissions(manifest_directory.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("could not write Skill Selection manifest")
    );
    assert_eq!(fs::read_to_string(manifest_path).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn cli_cancelled_removal_leaves_the_manifest_unchanged() {
    use std::thread;
    use std::time::Duration;

    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    let original = "manifest_version = 1\n\n[[sources]]\ntype = \"github\"\npath = \"https://github.com/devndive/skill-manager\"\nrequested_revision = \"main\"\nresolved_commit = \"bbbb\"\nskills = [\"nested\"]\n";
    fs::write(&manifest_path, original).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args([
            "remove",
            "https://github.com/devndive/skill-manager",
            "--manifest",
            manifest_path.to_str().unwrap(),
        ])
        .env("PATH", "")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    thread::sleep(Duration::from_secs(1));

    assert_eq!(
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGINT) },
        0
    );
    child.stdin.take().unwrap().write_all(b"y\n").unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(!output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("discovery was cancelled"), "{stderr}");
    assert_eq!(fs::read_to_string(manifest_path).unwrap(), original);
}

fn run_cli_with_stdin(
    arguments: &[&str],
    working_directory: &std::path::Path,
    input: &str,
) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_skill-manager"))
        .args(arguments)
        .current_dir(working_directory)
        .env("PATH", "")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}
