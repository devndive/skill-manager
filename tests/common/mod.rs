use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};

use tempfile::TempDir;

pub fn git_environment_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
}

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

    #[allow(dead_code)]
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

#[cfg(unix)]
#[allow(dead_code)]
pub struct FakeGitHub {
    _environment: Vec<EnvironmentVariable>,
    _directory: TempDir,
    log: PathBuf,
    _lock: MutexGuard<'static, ()>,
}

#[cfg(unix)]
#[allow(dead_code)]
impl FakeGitHub {
    pub fn new(repository: &Path, lock: MutexGuard<'static, ()>) -> Self {
        Self::with_clone_behavior(Some(repository), None, None, lock)
    }

    pub fn failing_clone(message: &str, lock: MutexGuard<'static, ()>) -> Self {
        Self::with_clone_behavior(None, Some(message), None, lock)
    }

    pub fn signal_clone(signal: &str, lock: MutexGuard<'static, ()>) -> Self {
        Self::with_clone_behavior(None, None, Some(signal), lock)
    }

    fn with_clone_behavior(
        repository: Option<&Path>,
        clone_error: Option<&str>,
        signal: Option<&str>,
        lock: MutexGuard<'static, ()>,
    ) -> Self {
        use std::os::unix::fs::PermissionsExt;

        let directory = TempDir::new().unwrap();
        let executable = directory.path().join("git");
        let log = directory.path().join("git.log");
        let real_git = Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap();
        assert!(real_git.status.success());
        let real_git = String::from_utf8(real_git.stdout)
            .unwrap()
            .trim()
            .to_owned();
        fs::write(
            &executable,
            r#"#!/bin/sh
for argument in "$@"; do
  printf '%s\t' "$argument" >> "$SKILL_MANAGER_TEST_GIT_LOG"
done
printf '\n' >> "$SKILL_MANAGER_TEST_GIT_LOG"

if [ "$1" = "clone" ]; then
  for destination do :; done
  if [ -n "$SKILL_MANAGER_TEST_CLONE_ERROR" ]; then
    printf '%s\n' "$SKILL_MANAGER_TEST_CLONE_ERROR" >&2
    exit 128
  fi
  if [ -n "$SKILL_MANAGER_TEST_SIGNAL" ]; then
    kill -"$SKILL_MANAGER_TEST_SIGNAL" "$PPID"
    sleep 5
    exit 130
  fi
  exec "$SKILL_MANAGER_TEST_REAL_GIT" clone --no-checkout --quiet "$SKILL_MANAGER_TEST_REMOTE" "$destination"
fi

exec "$SKILL_MANAGER_TEST_REAL_GIT" "$@"
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).unwrap();

        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![directory.path().to_path_buf()];
        paths.extend(std::env::split_paths(&path));
        let fake_path = std::env::join_paths(paths).unwrap();
        let environment = vec![
            EnvironmentVariable::set("PATH", fake_path),
            EnvironmentVariable::set("SKILL_MANAGER_TEST_GIT_LOG", &log),
            EnvironmentVariable::set("SKILL_MANAGER_TEST_REAL_GIT", real_git),
            EnvironmentVariable::set(
                "SKILL_MANAGER_TEST_REMOTE",
                repository.unwrap_or_else(|| Path::new("")),
            ),
            EnvironmentVariable::set(
                "SKILL_MANAGER_TEST_CLONE_ERROR",
                clone_error.unwrap_or_default(),
            ),
            EnvironmentVariable::set("SKILL_MANAGER_TEST_SIGNAL", signal.unwrap_or_default()),
        ];

        Self {
            _environment: environment,
            _directory: directory,
            log,
            _lock: lock,
        }
    }

    pub fn commands(&self) -> Vec<Vec<String>> {
        fs::read_to_string(&self.log)
            .unwrap()
            .lines()
            .map(|line| {
                line.split('\t')
                    .filter(|argument| !argument.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .collect()
    }
}

#[cfg(unix)]
struct EnvironmentVariable {
    name: &'static str,
    previous: Option<OsString>,
}

#[cfg(unix)]
impl EnvironmentVariable {
    fn set(name: &'static str, value: impl AsRef<OsStr>) -> Self {
        let previous = std::env::var_os(name);
        // All tests in this integration-test process serialize environment access with the lock.
        unsafe {
            std::env::set_var(name, value);
        }
        Self { name, previous }
    }
}

#[cfg(unix)]
impl Drop for EnvironmentVariable {
    fn drop(&mut self) {
        // The owning FakeGitHub still holds the process-wide test lock during restoration.
        unsafe {
            if let Some(previous) = &self.previous {
                std::env::set_var(self.name, previous);
            } else {
                std::env::remove_var(self.name);
            }
        }
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
