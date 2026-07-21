use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

pub struct TestRepository {
    _directory: TempDir,
    path: PathBuf,
}

impl TestRepository {
    pub fn new(name: &str) -> Self {
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

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write(&self, path: &str, contents: &str) {
        let path = self.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    pub fn commit(&self, message: &str) -> String {
        git(self.path(), ["add", "."]);
        git(self.path(), ["commit", "--quiet", "-m", message]);
        git(self.path(), ["rev-parse", "HEAD"])
    }

    pub fn git(&self, arguments: &[&str]) -> String {
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
