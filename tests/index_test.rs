use itertools::Itertools;
use quicknotes::NoteConfig;

mod testutil;

#[test]
fn indexes_existing_files_on_disk() {
    let roots = testutil::setup_filesystem();
    let cool_note_path = roots
        .note_root
        .path()
        .join("notes")
        .join("my-cool-note.txt");

    std::fs::write(
        &cool_note_path,
        textwrap::dedent(
            r#"
            ---
            title = "my cool note"
            created_at = 2015-10-21T07:28:00-07:00
            ---
            "#
            .trim_start_matches("\n"),
        ),
    )
    .expect("could not write note");

    let awesome_note_path = roots
        .note_root
        .path()
        .join("notes")
        .join("my-awesome-note.txt");

    std::fs::write(
        &awesome_note_path,
        textwrap::dedent(
            r#"
            ---
            title = "my awesome note"
            created_at = 2015-10-22T07:28:00-07:00
            ---
            "#
            .trim_start_matches("\n"),
        ),
    )
    .expect("could not write note");

    let config = NoteConfig {
        file_extension: ".txt".to_string(),
        root_dir: roots.note_root.path().to_owned(),
        temp_root_override: Some(roots.temp_root.path().to_owned()),
    };

    quicknotes::index_notes(&config).expect("could not index notes");
    let notes = quicknotes::indexed_notes(&config).expect("could not read indexed notes");

    assert_eq!(
        notes
            .into_iter()
            .map(|(path, preamble)| (path, preamble.title))
            .sorted()
            .collect::<Vec<_>>(),
        vec![
            (awesome_note_path, "my awesome note".to_string()),
            (cool_note_path, "my cool note".to_string())
        ]
    )
}

#[test]
fn deleted_files_are_removed_from_the_index() {
    let roots = testutil::setup_filesystem();
    let cool_note_path = roots
        .note_root
        .path()
        .join("notes")
        .join("my-cool-note.txt");

    std::fs::write(
        &cool_note_path,
        textwrap::dedent(
            r#"
            ---
            title = "my cool note"
            created_at = 2015-10-21T07:28:00-07:00
            ---
            "#
            .trim_start_matches("\n"),
        ),
    )
    .expect("could not write note");

    let awesome_note_path = roots
        .note_root
        .path()
        .join("notes")
        .join("my-awesome-note.txt");

    std::fs::write(
        &awesome_note_path,
        textwrap::dedent(
            r#"
            ---
            title = "my awesome note"
            created_at = 2015-10-22T07:28:00-07:00
            ---
            "#
            .trim_start_matches("\n"),
        ),
    )
    .expect("could not write note");

    let config = NoteConfig {
        file_extension: ".txt".to_string(),
        root_dir: roots.note_root.path().to_owned(),
        temp_root_override: Some(roots.temp_root.path().to_owned()),
    };

    quicknotes::index_notes(&config).expect("could not index notes");
    std::fs::remove_file(&awesome_note_path).expect("could not remote note");
    quicknotes::index_notes(&config).expect("could not re-index notes");

    let notes = quicknotes::indexed_notes(&config).expect("could not read indexed notes");

    assert_eq!(
        notes
            .into_iter()
            .map(|(path, preamble)| (path, preamble.title))
            .collect::<Vec<_>>(),
        vec![(cool_note_path, "my cool note".to_string())]
    )
}
