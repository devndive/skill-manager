use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use tempfile::TempDir;

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

struct TestRepository {
    _directory: TempDir,
    path: PathBuf,
}

impl TestRepository {
    fn new(name: &str) -> Self {
        let parent = TempDir::new().unwrap();
        let path = parent.path().join(name);
        fs::create_dir(&path).unwrap();
        git(&path, ["init", "--quiet"]);
        git(&path, ["config", "user.email", "test@example.com"]);
        git(&path, ["config", "user.name", "Test User"]);

        Self {
            _directory: parent,
            path,
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self, path: &str, contents: &str) {
        let path = self.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn commit(&self, message: &str) -> String {
        git(self.path(), ["add", "."]);
        git(self.path(), ["commit", "--quiet", "-m", message]);
        git(self.path(), ["rev-parse", "HEAD"])
    }

    fn git(&self, arguments: &[&str]) -> String {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(self.path())
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }
}

fn git<const N: usize>(directory: &Path, arguments: [&str; N]) -> String {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(directory)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
