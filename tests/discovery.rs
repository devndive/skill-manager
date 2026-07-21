use std::fs;

use skill_manager::{DiscoverError, DiscoverRequest, discover};
use tempfile::TempDir;

mod common;

use common::TestRepository;

#[test]
fn library_discovers_a_root_skill_at_head() {
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
    assert_eq!(
        discovery.source.path,
        fs::canonicalize(repository.path())
            .unwrap()
            .to_str()
            .unwrap()
    );
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
    let repository = TestRepository::new("source-repository ");
    repository.write("SKILL.md", "# Root\n");
    repository.commit("add root skill");

    let discovery = discover(DiscoverRequest::new(repository.path())).unwrap();

    assert_eq!(
        discovery.source.path,
        fs::canonicalize(repository.path())
            .unwrap()
            .to_str()
            .unwrap()
    );
    assert_eq!(discovery.skills[0].name, "source-repository ");
}

#[test]
fn library_resolves_a_tag_whose_name_begins_with_a_hyphen() {
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
