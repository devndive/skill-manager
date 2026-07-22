use std::fs;

use skill_manager::{SelectRequest, prepare_interactive_select, select, select_interactively};

mod common;

use common::{TestRepository, git_environment_lock};

#[test]
fn interaction_model_preserves_existing_intent_and_marks_missing_skills() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("parent/SKILL.md", "# Parent\n");
    repository.write("parent/child/SKILL.md", "# Child\n");
    repository.write("removed/SKILL.md", "# Removed\n");
    repository.commit("add initial skills");
    let manifest_path = repository.path().parent().unwrap().join("skills.toml");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_path("parent/child")
            .select_path("removed"),
    )
    .unwrap();
    fs::remove_file(repository.path().join("removed/SKILL.md")).unwrap();
    repository.commit("remove selected skill");
    let original = fs::read_to_string(&manifest_path).unwrap();

    let interaction = prepare_interactive_select(
        SelectRequest::new(repository.path()).with_manifest_path(&manifest_path),
    )
    .unwrap();

    assert_eq!(
        interaction
            .prompt()
            .options
            .iter()
            .map(|option| (
                option.name.as_str(),
                option.path.as_str(),
                option.parent_path.as_deref(),
                option.nesting_depth,
                option.preselected,
            ))
            .collect::<Vec<_>>(),
        vec![
            ("parent", "parent", None, 0, false),
            ("child", "parent/child", Some("parent"), 1, true,),
        ]
    );
    assert_eq!(interaction.prompt().missing_paths, vec!["removed"]);
    assert_eq!(fs::read_to_string(manifest_path).unwrap(), original);
}

#[test]
fn interaction_model_warns_about_overlapping_parent_and_nested_skills() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("parent/SKILL.md", "# Parent\n");
    repository.write("parent/child/SKILL.md", "# Child\n");
    repository.write("separate/SKILL.md", "# Separate\n");
    repository.commit("add skills");

    let interaction = prepare_interactive_select(SelectRequest::new(repository.path())).unwrap();
    let overlaps = interaction.prompt().overlaps(&[
        "separate".to_owned(),
        "parent/child".to_owned(),
        "parent".to_owned(),
    ]);

    assert_eq!(
        overlaps
            .iter()
            .map(|overlap| (overlap.parent_path.as_str(), overlap.nested_path.as_str()))
            .collect::<Vec<_>>(),
        vec![("parent", "parent/child")]
    );
}

#[test]
fn interaction_model_labels_duplicate_names_by_path_and_indents_nested_skills() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("alpha/SKILL.md", "# Alpha\n");
    repository.write("alpha/child/SKILL.md", "# First child\n");
    repository.write("beta/child/SKILL.md", "# Second child\n");
    repository.commit("add duplicate names");

    let interaction = prepare_interactive_select(SelectRequest::new(repository.path())).unwrap();

    assert_eq!(
        interaction
            .prompt()
            .options
            .iter()
            .map(|option| option.terminal_label())
            .collect::<Vec<_>>(),
        vec![
            "alpha (alpha)",
            "  child (alpha/child; parent: alpha)",
            "child (beta/child)",
        ]
    );
}

#[test]
fn confirmed_interaction_replaces_existing_selection_and_removes_missing_paths() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("old/SKILL.md", "# Old\n");
    repository.write("removed/SKILL.md", "# Removed\n");
    repository.commit("add initial skills");
    let manifest_path = repository.path().parent().unwrap().join("skills.toml");
    select(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .select_all(),
    )
    .unwrap();
    fs::remove_file(repository.path().join("removed/SKILL.md")).unwrap();
    repository.write("new/SKILL.md", "# New\n");
    repository.commit("replace skill");

    let selection = select_interactively(
        SelectRequest::new(repository.path()).with_manifest_path(&manifest_path),
        |prompt| {
            assert_eq!(prompt.missing_paths, vec!["removed"]);
            Ok::<_, std::io::Error>(Some(vec!["new".to_owned()]))
        },
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        selection
            .skills
            .iter()
            .map(|skill| skill.path.as_str())
            .collect::<Vec<_>>(),
        vec!["new"]
    );
    let contents = fs::read_to_string(manifest_path).unwrap();
    assert!(contents.contains("skills = [\"new\"]"), "{contents}");
    assert!(!contents.contains("removed"), "{contents}");
}

#[test]
fn confirmed_empty_interaction_removes_the_source() {
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

    let selection = select_interactively(
        SelectRequest::new(repository.path()).with_manifest_path(&manifest_path),
        |_| Ok::<_, std::io::Error>(Some(Vec::new())),
    )
    .unwrap()
    .unwrap();

    assert!(selection.skills.is_empty());
    assert!(
        !fs::read_to_string(manifest_path)
            .unwrap()
            .contains("[[sources]]")
    );
}

#[test]
fn cancelled_interaction_leaves_the_manifest_unchanged() {
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
    let original = fs::read_to_string(&manifest_path).unwrap();

    let selection = select_interactively(
        SelectRequest::new(repository.path()).with_manifest_path(&manifest_path),
        |_| Ok::<_, std::io::Error>(None),
    )
    .unwrap();

    assert!(selection.is_none());
    assert_eq!(fs::read_to_string(manifest_path).unwrap(), original);
}

#[test]
fn prompt_failure_leaves_the_manifest_unchanged() {
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
    let original = fs::read_to_string(&manifest_path).unwrap();

    let error = select_interactively(
        SelectRequest::new(repository.path()).with_manifest_path(&manifest_path),
        |_| Err::<Option<Vec<String>>, _>(std::io::Error::other("terminal input failed")),
    )
    .unwrap_err();

    assert!(error.to_string().contains("terminal input failed"));
    assert_eq!(fs::read_to_string(manifest_path).unwrap(), original);
}

#[test]
fn discovery_failure_does_not_start_interaction_or_change_the_manifest() {
    let _lock = git_environment_lock();
    let repository = TestRepository::new("source-repository");
    repository.write("skill/SKILL.md", "# Skill\n");
    repository.commit("add skill");
    let manifest_path = repository.path().parent().unwrap().join("skills.toml");
    let original = "# existing\nmanifest_version = 1\n";
    fs::write(&manifest_path, original).unwrap();
    let mut interaction_started = false;

    let result = select_interactively(
        SelectRequest::new(repository.path())
            .with_manifest_path(&manifest_path)
            .with_revision("missing"),
        |_| {
            interaction_started = true;
            Ok::<_, std::io::Error>(None)
        },
    );

    assert!(result.is_err());
    assert!(!interaction_started);
    assert_eq!(fs::read_to_string(manifest_path).unwrap(), original);
}
