use std::fs;

use skill_manager::{SelectError, SelectRequest, select};
use tempfile::TempDir;
use toml_edit::DocumentMut;

mod common;

use common::{TestRepository, git_environment_lock};

#[test]
fn library_selects_all_skills_into_a_new_manifest() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("SKILL.md", "# Root\n");
    repository.write("nested/SKILL.md", "# Nested\n");
    let commit = repository.commit("add skills");
    let manifest_path = repository.path().parent().unwrap().join("skills.toml");

    let selection = select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();

    assert_eq!(selection.schema_version, 1);
    assert_eq!(selection.resolved_commit, commit);
    assert_eq!(
        selection
            .skills
            .iter()
            .map(|skill| skill.path.as_str())
            .collect::<Vec<_>>(),
        vec![".", "nested"]
    );
    assert_eq!(
        fs::read_to_string(manifest_path).unwrap(),
        format!(
            "manifest_version = 1\n\n[[sources]]\ntype = \"local\"\npath = \"source-repository\"\nrequested_revision = \"HEAD\"\nresolved_commit = \"{commit}\"\nskills = [\".\", \"nested\"]\n"
        )
    );
}

#[test]
fn library_selects_duplicate_skill_names_by_exact_path() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/child/SKILL.md", "# First child\n");
    repository.write("beta/child/SKILL.md", "# Second child\n");
    let commit = repository.commit("add duplicate names");
    repository.git(&["tag", "v1"]);
    let manifest_path = repository.path().parent().unwrap().join("skills.toml");

    let selection = select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .with_revision("v1")
            .select_path("beta/child")
            .select_path("alpha/child"),
    )
    .unwrap();

    assert_eq!(
        selection
            .skills
            .iter()
            .map(|skill| (skill.name.as_str(), skill.path.as_str()))
            .collect::<Vec<_>>(),
        vec![("child", "alpha/child"), ("child", "beta/child")]
    );
    assert_eq!(
        fs::read_to_string(manifest_path).unwrap(),
        format!(
            "manifest_version = 1\n\n[[sources]]\ntype = \"local\"\npath = \"source-repository\"\nrequested_revision = \"v1\"\nresolved_commit = \"{commit}\"\nskills = [\"alpha/child\", \"beta/child\"]\n"
        )
    );
}

#[test]
fn library_rejects_missing_skill_paths_without_changing_the_manifest() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("kept/SKILL.md", "# Kept\n");
    repository.commit("add skill");
    let manifest_path = repository.path().parent().unwrap().join("skills.toml");
    let original = "# keep this comment\nmanifest_version = 1\n";
    fs::write(&manifest_path, original).unwrap();

    let error = select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_path("missing"),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        SelectError::InvalidSelection { paths, .. } if paths == vec!["missing"]
    ));
    assert_eq!(fs::read_to_string(manifest_path).unwrap(), original);
}

#[test]
fn library_adds_a_second_source_without_changing_the_first() {
    let _lock = git_environment_lock();
    let first = TestRepository::new("first-source");
    first.write("first/SKILL.md", "# First\n");
    let first_commit = first.commit("add first skill");
    let second = TestRepository::new("second-source");
    second.write("second/SKILL.md", "# Second\n");
    let second_commit = second.commit("add second skill");
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");

    select(
        SelectRequest::new(first.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    select(
        SelectRequest::new(second.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();

    let document = fs::read_to_string(manifest_path)
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();
    let sources = document["sources"].as_array_of_tables().unwrap();
    let first_source = sources.get(0).unwrap();
    let second_source = sources.get(1).unwrap();
    assert_eq!(sources.len(), 2);
    assert_eq!(first_source["requested_revision"].as_str(), Some("HEAD"));
    assert_eq!(
        first_source["resolved_commit"].as_str(),
        Some(first_commit.as_str())
    );
    assert_eq!(
        first_source["skills"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["first"]
    );
    assert_eq!(
        second_source["resolved_commit"].as_str(),
        Some(second_commit.as_str())
    );
    assert_eq!(
        second_source["skills"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["second"]
    );
}

#[test]
fn library_replaces_one_source_and_preserves_comments_and_other_sources() {
    let _lock = git_environment_lock();
    let first = TestRepository::new("first-source");
    first.write("old/SKILL.md", "# Old\n");
    first.commit("add old skill");
    let second = TestRepository::new("second-source");
    second.write("kept/SKILL.md", "# Kept\n");
    let second_commit = second.commit("add kept skill");
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");

    for repository in [&first, &second] {
        select(
            SelectRequest::new(repository.path())
                .with_manifest_path(&manifest_path)
                .select_all(),
        )
        .unwrap();
    }

    let commented = fs::read_to_string(&manifest_path)
        .unwrap()
        .replacen(
            "manifest_version = 1\n",
            "# manifest comment\nmanifest_version = 1\n",
            1,
        )
        .replacen("[[sources]]\n", "# first source\n[[sources]]\n", 1)
        .replacen(
            "requested_revision = \"HEAD\"",
            "requested_revision = \"HEAD\" # keep revision formatting",
            1,
        )
        .replacen(
            "skills = [\"old\"]",
            "skills = [\"old\"] # keep selection formatting",
            1,
        )
        .replace(
            "skills = [\"kept\"]",
            "skills = [\"kept\"] # keep this formatting",
        );
    fs::write(&manifest_path, commented).unwrap();

    first.write("new/SKILL.md", "# New\n");
    let first_commit = first.commit("add new skill");
    select(
        SelectRequest::new(first.path())
            .with_manifest_path(&manifest_path)
            .select_path("new"),
    )
    .unwrap();

    let contents = fs::read_to_string(manifest_path).unwrap();
    assert!(contents.contains("# manifest comment"));
    assert!(contents.contains("# first source"));
    assert!(contents.contains("requested_revision = \"HEAD\" # keep revision formatting"));
    assert!(contents.contains("skills = [\"new\"] # keep selection formatting"));
    assert!(contents.contains("skills = [\"kept\"] # keep this formatting"));
    let document = contents.parse::<DocumentMut>().unwrap();
    let sources = document["sources"].as_array_of_tables().unwrap();
    assert_eq!(sources.len(), 2);
    let first_source = sources.get(0).unwrap();
    let second_source = sources.get(1).unwrap();
    assert_eq!(
        first_source["resolved_commit"].as_str(),
        Some(first_commit.as_str())
    );
    assert_eq!(
        first_source["skills"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["new"]
    );
    assert_eq!(
        second_source["resolved_commit"].as_str(),
        Some(second_commit.as_str())
    );
    assert_eq!(
        second_source["skills"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["kept"]
    );
}

#[test]
fn library_removes_only_the_source_with_an_empty_selection() {
    let _lock = git_environment_lock();
    let removed = TestRepository::new("removed-source");
    removed.write("removed/SKILL.md", "# Removed\n");
    removed.commit("add removed skill");
    let kept = TestRepository::new("kept-source");
    kept.write("kept/SKILL.md", "# Kept\n");
    let kept_commit = kept.commit("add kept skill");
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");

    for repository in [&removed, &kept] {
        select(
            SelectRequest::new(repository.path())
                .with_manifest_path(&manifest_path)
                .select_all(),
        )
        .unwrap();
    }

    let selection =
        select(SelectRequest::new(removed.path()).with_manifest_path(&manifest_path)).unwrap();

    assert!(selection.skills.is_empty());
    let document = fs::read_to_string(manifest_path)
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();
    let sources = document["sources"].as_array_of_tables().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(
        sources.get(0).unwrap()["resolved_commit"].as_str(),
        Some(kept_commit.as_str())
    );
}

#[test]
fn library_rejects_unsupported_manifest_versions_without_changing_the_file() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("skill/SKILL.md", "# Skill\n");
    repository.commit("add skill");
    let manifest_path = repository.path().parent().unwrap().join("skills.toml");
    let original = "# future format\nmanifest_version = 2\nsources = []\n";
    fs::write(&manifest_path, original).unwrap();

    let error = select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        SelectError::UnsupportedManifestVersion { version: 2, .. }
    ));
    assert_eq!(fs::read_to_string(manifest_path).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn library_persists_the_canonical_github_repository_identity() {
    let lock = git_environment_lock();
    let repository = TestRepository::new("remote-source");
    repository.write("skill/SKILL.md", "# Skill\n");
    repository.commit("add skill");
    let _fake_github = common::FakeGitHub::new(repository.path(), lock);
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");

    select(
        SelectRequest::new("https://GitHub.com/DevNDive/Skill-Manager.git/")
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();

    let document = fs::read_to_string(manifest_path)
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();
    let source = document["sources"]
        .as_array_of_tables()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(source["type"].as_str(), Some("github"));
    assert_eq!(
        source["path"].as_str(),
        Some("https://github.com/devndive/skill-manager")
    );
}

#[cfg(unix)]
#[test]
fn library_write_failure_leaves_the_existing_manifest_unchanged() {
    use std::os::unix::fs::PermissionsExt;

    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("old/SKILL.md", "# Old\n");
    repository.commit("add old skill");
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    let original = fs::read_to_string(&manifest_path).unwrap();
    repository.write("new/SKILL.md", "# New\n");
    repository.commit("add new skill");

    fs::set_permissions(manifest_directory.path(), fs::Permissions::from_mode(0o555)).unwrap();
    let result = select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_path("new"),
    );
    fs::set_permissions(manifest_directory.path(), fs::Permissions::from_mode(0o755)).unwrap();

    assert!(matches!(result, Err(SelectError::ManifestWrite { .. })));
    assert_eq!(fs::read_to_string(manifest_path).unwrap(), original);
}

#[test]
fn library_can_add_a_source_after_the_last_source_was_removed() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("skill/SKILL.md", "# Skill\n");
    repository.commit("add skill");
    let manifest_directory = TempDir::new().unwrap();
    let manifest_path = manifest_directory.path().join("skills.toml");

    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    select(SelectRequest::new(repository.path()).with_manifest_path(&manifest_path)).unwrap();
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();

    let document = fs::read_to_string(manifest_path)
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();
    assert_eq!(document["sources"].as_array_of_tables().unwrap().len(), 1);
}

#[test]
fn library_parse_failure_leaves_the_existing_manifest_unchanged() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("skill/SKILL.md", "# Skill\n");
    repository.commit("add skill");
    let manifest_path = repository.path().parent().unwrap().join("skills.toml");
    let original = "manifest_version = [\n";
    fs::write(&manifest_path, original).unwrap();

    let result = select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    );

    assert!(matches!(result, Err(SelectError::ManifestParse { .. })));
    assert_eq!(fs::read_to_string(manifest_path).unwrap(), original);
}

#[test]
fn library_discovery_failure_leaves_the_existing_manifest_unchanged() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("skill/SKILL.md", "# Skill\n");
    repository.commit("add skill");
    let manifest_path = repository.path().parent().unwrap().join("skills.toml");
    let original = "# existing\nmanifest_version = 1\n";
    fs::write(&manifest_path, original).unwrap();

    let result = select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .with_revision("missing")
            .select_all(),
    );

    assert!(matches!(
        result,
        Err(SelectError::Discovery(
            skill_manager::DiscoverError::RevisionUnavailable { .. }
        ))
    ));
    assert_eq!(fs::read_to_string(manifest_path).unwrap(), original);
}

#[test]
fn library_rejects_invalid_source_entries_without_changing_the_manifest() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("skill/SKILL.md", "# Skill\n");
    repository.commit("add skill");
    let manifest_path = repository.path().parent().unwrap().join("skills.toml");
    let original = "manifest_version = 1\n\n[[sources]]\ntype = \"local\"\npath = \"other\"\nrequested_revision = \"HEAD\"\nresolved_commit = \"abc\"\n";
    fs::write(&manifest_path, original).unwrap();

    let result = select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    );

    assert!(matches!(result, Err(SelectError::InvalidManifest { .. })));
    assert_eq!(fs::read_to_string(manifest_path).unwrap(), original);
}

#[test]
fn library_consolidates_duplicate_entries_for_the_selected_source() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("old/SKILL.md", "# Old\n");
    repository.commit("add old skill");
    let manifest_path = repository.path().parent().unwrap().join("skills.toml");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    let duplicate = fs::read_to_string(&manifest_path).unwrap();
    let source_table = duplicate.split_once("\n[[sources]]").unwrap().1;
    fs::write(
        &manifest_path,
        format!("{duplicate}\n[[sources]]{source_table}"),
    )
    .unwrap();
    repository.write("new/SKILL.md", "# New\n");
    repository.commit("add new skill");

    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_path("new"),
    )
    .unwrap();

    let document = fs::read_to_string(manifest_path)
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();
    let sources = document["sources"].as_array_of_tables().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(
        sources.get(0).unwrap()["skills"]
            .as_array()
            .unwrap()
            .iter()
            .map(|skill| skill.as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["new"]
    );
}

#[test]
fn library_removes_a_source_after_its_last_skill_is_deleted() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("skill/SKILL.md", "# Skill\n");
    repository.commit("add skill");
    let manifest_path = repository.path().parent().unwrap().join("skills.toml");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    fs::remove_file(repository.path().join("skill/SKILL.md")).unwrap();
    repository.commit("remove last skill");

    let skill_selection =
        select(SelectRequest::new(repository.path()).with_manifest_path(&manifest_path)).unwrap();

    assert!(skill_selection.skills.is_empty());
    let document = fs::read_to_string(manifest_path)
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();
    assert!(document.get("sources").is_none());
}

#[test]
fn library_preserves_comments_on_retained_multiline_skill_entries() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("removed/SKILL.md", "# Removed\n");
    repository.write("retained/SKILL.md", "# Retained\n");
    repository.commit("add initial skills");
    let manifest_path = repository.path().parent().unwrap().join("skills.toml");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    let multiline = fs::read_to_string(&manifest_path).unwrap().replace(
        "skills = [\"removed\", \"retained\"]",
        "skills = [\n  \"removed\",\n  # keep retained comment\n  \"retained\",\n]",
    );
    fs::write(&manifest_path, multiline).unwrap();
    repository.write("new/SKILL.md", "# New\n");
    repository.commit("add new skill");

    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_path("retained")
            .select_path("new"),
    )
    .unwrap();

    let contents = fs::read_to_string(manifest_path).unwrap();
    assert!(
        contents.contains("# keep retained comment\n  \"retained\""),
        "{contents}"
    );
    assert!(contents.contains("\n  \"new\""), "{contents}");
    let document = contents.parse::<DocumentMut>().unwrap();
    let skills = document["sources"]
        .as_array_of_tables()
        .unwrap()
        .get(0)
        .unwrap()["skills"]
        .as_array()
        .unwrap();
    assert_eq!(
        skills
            .iter()
            .map(|skill| skill.as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["retained", "new"]
    );
}
