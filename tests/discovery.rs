use std::fs;
#[cfg(unix)]
use std::path::PathBuf;

use skill_manager::{DiscoverError, DiscoverRequest, discover};
use tempfile::TempDir;

mod common;

use common::{TestRepository, git_environment_lock};

#[test]
fn library_discovers_a_root_skill_at_head() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("SKILL.md", "# Root skill\n");
    let commit = repository.commit("add root skill");

    let discovery = discover(DiscoverRequest::new(repository.path())).unwrap();

    assert_eq!(discovery.requested_revision, "HEAD");
    assert_eq!(discovery.resolved_commit, commit);
    assert_eq!(discovery.skills.len(), 1);
    assert_eq!(discovery.skills[0].name, "source-repository");
    assert_eq!(discovery.skills[0].path, ".");
    assert_eq!(discovery.skills[0].parent_path, None);
}

#[test]
fn library_discovers_nested_skills_with_duplicate_names_at_a_commit() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha\n");
    repository.write("alpha/child/SKILL.md", "# Child one\n");
    repository.write("beta/child/SKILL.md", "# Child two\n");
    let historical_commit = repository.commit("add nested skills");
    repository.write("future/SKILL.md", "# Future\n");
    repository.commit("add future skill");

    let discovery =
        discover(DiscoverRequest::new(repository.path()).with_revision(historical_commit.clone()))
            .unwrap();

    assert_eq!(discovery.schema_version, 1);
    assert_eq!(discovery.source.repository_type, "local");
    assert_eq!(discovery.source.path, repository.source_repository_path());
    assert_eq!(discovery.requested_revision, historical_commit);
    assert_eq!(
        discovery
            .skills
            .iter()
            .map(|skill| (
                skill.name.as_str(),
                skill.path.as_str(),
                skill.parent_path.as_deref()
            ))
            .collect::<Vec<_>>(),
        vec![
            ("alpha", "alpha", None),
            ("child", "alpha/child", Some("alpha")),
            ("child", "beta/child", None),
        ]
    );
    assert!(
        discovery
            .skills
            .iter()
            .all(|skill| skill.identity.source == discovery.source.path)
    );
    assert_eq!(discovery.skills[1].identity.path, "alpha/child");
}

#[test]
fn library_preserves_utf8_repository_paths_without_git_quoting() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("débogage\tavancé/SKILL.md", "# Advanced debugging\n");
    repository.commit("add skill with utf8 path");

    let discovery = discover(DiscoverRequest::new(repository.path())).unwrap();

    assert_eq!(discovery.skills[0].name, "débogage\tavancé");
    assert_eq!(discovery.skills[0].path, "débogage\tavancé");
    assert_eq!(discovery.skills[0].identity.path, "débogage\tavancé");
}

#[test]
fn library_reports_an_unavailable_source_repository() {
    let _lock = git_environment_lock();
    let directory = TempDir::new().unwrap();
    let missing_repository = directory.path().join("missing");

    let error = discover(DiscoverRequest::new(&missing_repository)).unwrap_err();

    assert!(matches!(
        error,
        DiscoverError::RepositoryUnavailable { repository, .. }
            if repository == missing_repository.to_string_lossy()
    ));
}

#[test]
fn library_resolves_branches_tags_and_commits() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("stable/SKILL.md", "# Stable\n");
    let stable_commit = repository.commit("add stable skill");
    repository.git(&["branch", "release"]);
    repository.git(&["tag", "v1"]);
    repository.write("next/SKILL.md", "# Next\n");
    repository.commit("add next skill");

    for revision in ["release", "v1", stable_commit.as_str()] {
        let discovery =
            discover(DiscoverRequest::new(repository.path()).with_revision(revision)).unwrap();

        assert_eq!(discovery.requested_revision, revision);
        assert_eq!(discovery.resolved_commit, stable_commit);
        assert_eq!(
            discovery
                .skills
                .iter()
                .map(|skill| skill.path.as_str())
                .collect::<Vec<_>>(),
            vec!["stable"]
        );
    }
}

#[test]
fn library_excludes_untracked_misnamed_symlinked_and_submodule_entries() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("kept/SKILL.md", "# Kept\n");
    repository.write("wrong-case/skill.md", "# Wrong case\n");
    repository.write("suffix/SKILL.md.bak", "# Wrong suffix\n");
    let commit = repository.commit("add tracked files");

    repository.write("symlink-target", "kept/SKILL.md");
    let symlink_blob = repository.git(&["hash-object", "-w", "symlink-target"]);
    repository.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        "120000",
        &symlink_blob,
        "linked/SKILL.md",
    ]);
    repository.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        "160000",
        &commit,
        "vendor",
    ]);
    repository.git(&["commit", "--quiet", "-m", "add excluded entries"]);
    repository.write("untracked/SKILL.md", "# Untracked\n");
    repository.write("vendor/SKILL.md", "# Inside submodule\n");

    let discovery = discover(DiscoverRequest::new(repository.path())).unwrap();

    assert_eq!(
        discovery
            .skills
            .iter()
            .map(|skill| skill.path.as_str())
            .collect::<Vec<_>>(),
        vec!["kept"]
    );
}

#[test]
fn library_reports_missing_revisions_and_repositories_without_skills() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("README.md", "# No Skills\n");
    repository.commit("add readme");

    let missing_revision =
        discover(DiscoverRequest::new(repository.path()).with_revision("missing")).unwrap_err();
    assert!(matches!(
        missing_revision,
        DiscoverError::RevisionUnavailable { revision, .. } if revision == "missing"
    ));

    let no_skills = discover(DiscoverRequest::new(repository.path())).unwrap_err();
    assert!(matches!(no_skills, DiscoverError::NoSkills(_)));
}

#[test]
fn library_preserves_trailing_whitespace_in_the_repository_path() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository ");
    repository.write("SKILL.md", "# Root\n");
    repository.commit("add root skill");

    let discovery = discover(DiscoverRequest::new(repository.path())).unwrap();

    assert_eq!(discovery.source.path, repository.source_repository_path());
    assert_eq!(discovery.skills[0].name, "source-repository ");
}

#[test]
fn library_resolves_a_tag_whose_name_begins_with_a_hyphen() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("SKILL.md", "# Root\n");
    let commit = repository.commit("add root skill");
    repository.git(&["update-ref", "refs/tags/--foo", "HEAD"]);

    let discovery =
        discover(DiscoverRequest::new(repository.path()).with_revision("--foo")).unwrap();

    assert_eq!(discovery.requested_revision, "--foo");
    assert_eq!(discovery.resolved_commit, commit);
}

#[test]
fn library_ignores_uncommitted_changes_to_tracked_skill_files() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("stable/SKILL.md", "# Stable\n");
    repository.commit("add stable skill");
    fs::remove_file(repository.path().join("stable/SKILL.md")).unwrap();
    repository.write("working-tree/SKILL.md", "# Untracked\n");

    let discovery = discover(DiscoverRequest::new(repository.path())).unwrap();

    assert_eq!(
        discovery
            .skills
            .iter()
            .map(|skill| skill.path.as_str())
            .collect::<Vec<_>>(),
        vec!["stable"]
    );
}

#[cfg(unix)]
#[test]
fn library_discovers_a_normalized_github_default_branch_and_cleans_the_clone() {
    let lock = git_environment_lock();
    let repository = TestRepository::new("remote-source");
    repository.write("SKILL.md", "# Root\n");
    repository.write("nested/SKILL.md", "# Nested\n");
    let commit = repository.commit("add remote skills");
    let fake_github = common::FakeGitHub::new(repository.path(), lock);

    let discovery = discover(DiscoverRequest::new(
        "https://GitHub.com/DevNDive/Skill-Manager.git/",
    ))
    .unwrap();

    assert_eq!(discovery.source.repository_type, "github");
    assert_eq!(
        discovery.source.path,
        "https://github.com/devndive/skill-manager"
    );
    assert_eq!(discovery.requested_revision, "HEAD");
    assert_eq!(discovery.resolved_commit, commit);
    assert_eq!(
        discovery
            .skills
            .iter()
            .map(|skill| (
                skill.name.as_str(),
                skill.path.as_str(),
                skill.parent_path.as_deref()
            ))
            .collect::<Vec<_>>(),
        vec![
            ("skill-manager", ".", None),
            ("nested", "nested", Some("."))
        ]
    );
    assert!(
        discovery
            .skills
            .iter()
            .all(|skill| skill.identity.source == discovery.source.path)
    );

    let commands = fake_github.commands();
    assert_eq!(
        &commands[0][..5],
        [
            "clone",
            "--filter=blob:none",
            "--no-checkout",
            "--quiet",
            "https://github.com/devndive/skill-manager",
        ]
    );
    let clone_path = PathBuf::from(commands[0].last().unwrap());
    assert!(!clone_path.exists());
}

#[cfg(unix)]
#[test]
fn library_resolves_explicit_github_revisions() {
    let lock = git_environment_lock();
    let repository = TestRepository::new("remote-source");
    repository.write("stable/SKILL.md", "# Stable\n");
    let stable_commit = repository.commit("add stable skill");
    repository.git(&["branch", "release"]);
    repository.git(&["tag", "v1"]);
    repository.write("future/SKILL.md", "# Future\n");
    repository.commit("add future skill");
    let _fake_github = common::FakeGitHub::new(repository.path(), lock);

    for revision in ["release", "v1", stable_commit.as_str()] {
        let discovery = discover(
            DiscoverRequest::new("https://github.com/devndive/skill-manager")
                .with_revision(revision),
        )
        .unwrap();

        assert_eq!(discovery.requested_revision, revision);
        assert_eq!(discovery.resolved_commit, stable_commit);
        assert_eq!(
            discovery
                .skills
                .iter()
                .map(|skill| skill.path.as_str())
                .collect::<Vec<_>>(),
            vec!["stable"]
        );
    }
}

#[test]
fn library_rejects_unsupported_and_malformed_source_repository_urls() {
    let _lock = git_environment_lock();

    for source in [
        "https://gitlab.com/devndive/skill-manager",
        "https://github.com/devndive",
        "https:/github.com/devndive/skill-manager",
        "git@github.com:devndive/skill-manager.git",
    ] {
        let error = discover(DiscoverRequest::new(source)).unwrap_err();

        assert!(
            matches!(error, DiscoverError::InvalidSourceUrl { ref url, .. } if url == source),
            "unexpected error for {source}: {error}"
        );
    }
}

#[cfg(unix)]
#[test]
fn library_cleans_the_temporary_clone_after_a_missing_github_revision() {
    let lock = git_environment_lock();
    let repository = TestRepository::new("remote-source");
    repository.write("SKILL.md", "# Root\n");
    repository.commit("add remote skill");
    let fake_github = common::FakeGitHub::new(repository.path(), lock);

    let error = discover(
        DiscoverRequest::new("https://github.com/devndive/skill-manager").with_revision("missing"),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        DiscoverError::RevisionUnavailable {
            repository,
            revision,
            ..
        } if repository == "https://github.com/devndive/skill-manager"
            && revision == "missing"
    ));
    let clone_path = PathBuf::from(fake_github.commands()[0].last().unwrap());
    assert!(!clone_path.exists());
}

#[cfg(unix)]
#[test]
fn library_reports_github_transport_failures_and_cleans_temporary_data() {
    let lock = git_environment_lock();
    let fake_github =
        common::FakeGitHub::failing_clone("repository not found or access denied", lock);

    let error = discover(DiscoverRequest::new("https://github.com/devndive/missing")).unwrap_err();

    assert!(matches!(
        error,
        DiscoverError::RepositoryUnavailable {
            repository,
            details,
        } if repository == "https://github.com/devndive/missing"
            && details == "repository not found or access denied"
    ));
    let clone_path = PathBuf::from(fake_github.commands()[0].last().unwrap());
    assert!(!clone_path.exists());
}
