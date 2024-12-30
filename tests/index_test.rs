use chrono::{DateTime, FixedOffset, TimeZone};
use itertools::Itertools;
use quicknotes::NoteConfig;
use testutil::{AppendEditor, OverwriteEditor};

mod testutil;

fn test_time() -> DateTime<FixedOffset> {
    FixedOffset::east_opt(-7 * 60 * 60)
        .unwrap()
        .with_ymd_and_hms(2015, 10, 21, 7, 28, 0)
        .single()
        .unwrap()
}

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

#[test]
fn notes_are_added_to_the_index_when_they_are_created() {
    let roots = testutil::setup_filesystem();

    let config = NoteConfig {
        file_extension: ".txt".to_string(),
        root_dir: roots.note_root.path().to_owned(),
        temp_root_override: Some(roots.temp_root.path().to_owned()),
    };

    let mut editor = AppendEditor::new();
    editor.note_contents("hello, world!\n".to_string());

    quicknotes::make_note(&config, editor, "my cool note".to_string(), &test_time())
        .expect("could not write note");

    let notes = quicknotes::indexed_notes(&config).expect("could not read indexed notes");
    let cool_note_path = roots.note_root.path().join("notes/my-cool-note.txt");

    assert_eq!(
        notes
            .into_iter()
            .map(|(path, preamble)| (path, preamble.title))
            .collect::<Vec<_>>(),
        vec![(cool_note_path, "my cool note".to_string())]
    )
}

#[test]
fn opening_a_note_reindexes_it() {
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
    let mut overwrite_editor = OverwriteEditor::new();
    overwrite_editor.note_contents(textwrap::dedent(
        r#"
            ---
            title = "my super awesome note"
            created_at = 2015-10-22T07:28:00-07:00
            ---
            "#
        .trim_start_matches("\n"),
    ));

    quicknotes::open_note(&config, &overwrite_editor, &awesome_note_path)
        .expect("could not open note for editing");

    let notes = quicknotes::indexed_notes(&config).expect("could not read indexed notes");

    assert_eq!(
        notes
            .into_iter()
            .map(|(path, preamble)| (path, preamble.title))
            .sorted()
            .collect::<Vec<_>>(),
        vec![
            (awesome_note_path, "my super awesome note".to_string()),
            (cool_note_path, "my cool note".to_string()),
        ]
    )
}
