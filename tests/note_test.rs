use std::fs::{self, OpenOptions};

use chrono::{DateTime, FixedOffset, TimeZone};
use quicknotes::NoteConfig;
use testutil::{AppendEditor, SwappingEditor};

mod testutil;

fn test_time() -> DateTime<FixedOffset> {
    FixedOffset::east_opt(-7 * 60 * 60)
        .unwrap()
        .with_ymd_and_hms(2015, 10, 21, 7, 28, 0)
        .single()
        .unwrap()
}

#[test]
fn writes_notes_to_notes_directory() {
    let roots = testutil::setup_filesystem();
    let config = NoteConfig {
        file_extension: "txt".to_string(),
        root_dir: roots.note_root.path().to_owned(),
        temp_root_override: Some(roots.temp_root.path().to_owned()),
    };

    let mut editor = AppendEditor::new();
    editor.note_contents("hello, world!\n".to_string());

    let stored_path =
        quicknotes::make_note(&config, editor, "my cool note".to_string(), &test_time())
            .expect("could not write note")
            .expect("file has contents, so path should have been returned");

    let expected_note_path = roots.note_root.path().join("notes/my-cool-note.txt");

    assert_eq!(stored_path, expected_note_path);

    let note_contents = fs::read_to_string(expected_note_path).expect("failed to open note");
    insta::assert_snapshot!(note_contents);
}

#[test]
fn writes_dailies_to_notes_directory() {
    let roots = testutil::setup_filesystem();
    let config = NoteConfig {
        file_extension: "txt".to_string(),
        root_dir: roots.note_root.path().to_owned(),
        temp_root_override: Some(roots.temp_root.path().to_owned()),
    };

    let mut editor = AppendEditor::new();
    editor.note_contents("today was a cool day\n".to_string());

    let stored_path = quicknotes::make_or_open_daily(&config, editor, &test_time())
        .expect("could not write note")
        .expect("file has contents, so path should have been returned");

    let expected_note_path = roots.note_root.path().join("daily/2015-10-21.txt");

    assert_eq!(stored_path, expected_note_path);
    let note_contents = fs::read_to_string(expected_note_path).expect("failed to open note");

    insta::assert_snapshot!(note_contents);
}

#[test]
fn writes_notes_to_notes_directory_even_if_inode_changes() {
    let roots = testutil::setup_filesystem();
    let config = NoteConfig {
        file_extension: "txt".to_string(),
        root_dir: roots.note_root.path().to_owned(),
        temp_root_override: Some(roots.temp_root.path().to_owned()),
    };

    let mut append_editor = AppendEditor::new();
    append_editor.note_contents("hello, world!\n".to_string());
    let editor = SwappingEditor::new(append_editor);

    let stored_path =
        quicknotes::make_note(&config, editor, "my cool note".to_string(), &test_time())
            .expect("could not write note")
            .expect("file has contents, so path should have been returned");

    let expected_note_path = roots.note_root.path().join("notes/my-cool-note.txt");
    assert_eq!(stored_path, expected_note_path);

    let note_contents = fs::read_to_string(expected_note_path).expect("failed to open note");
    insta::assert_snapshot!(note_contents);
}

#[test]
fn editing_an_existing_daily_alters_the_same_file() {
    let roots = testutil::setup_filesystem();
    let config = NoteConfig {
        file_extension: "txt".to_string(),
        root_dir: roots.note_root.path().to_owned(),
        temp_root_override: Some(roots.temp_root.path().to_owned()),
    };

    let datetime = test_time();
    let mut editor = AppendEditor::new();

    editor.note_contents("today was a cool day\n".to_string());
    quicknotes::make_or_open_daily(&config, &editor, &datetime).expect("could not write note");

    editor.note_contents("I have more to say!\n".to_string());
    quicknotes::make_or_open_daily(&config, &editor, &datetime).expect("could not write note");

    let expected_note_path = roots.note_root.path().join("daily/2015-10-21.txt");
    let note_contents = fs::read_to_string(expected_note_path).expect("failed to open note");

    insta::assert_snapshot!(note_contents);
}

#[test]
fn opening_two_notes_with_the_same_name_prevents_clobbering() {
    let roots = testutil::setup_filesystem();

    let config = NoteConfig {
        file_extension: "txt".to_string(),
        root_dir: roots.note_root.path().to_owned(),
        temp_root_override: Some(roots.temp_root.path().to_owned()),
    };

    let mut editor = AppendEditor::new();

    editor.note_contents("hello, world!\n".to_string());
    let note_path =
        quicknotes::make_note(&config, editor, "my cool note".to_string(), &test_time())
            .expect("could not write note")
            .expect("file has contents, so path should have been returned");

    let original_note_contents = fs::read_to_string(&note_path).expect("failed to open note");

    let mut editor = AppendEditor::new();
    editor.note_contents("oh no\n".to_string());
    let second_note_result =
        quicknotes::make_note(&config, editor, "my cool note".to_string(), &test_time());

    let upd_note_path = second_note_result
        .expect("failed to write note")
        .expect("file has contents, so path should have been returned");

    let upd_original_location_contents =
        fs::read_to_string(&note_path).expect("failed to open note");

    assert_eq!(
        upd_original_location_contents, original_note_contents,
        "original note contents changed"
    );

    let upd_note_contents = fs::read_to_string(&upd_note_path).expect("failed to open note");
    insta::assert_snapshot!(upd_note_contents);
}

#[test]
fn opening_two_notes_with_the_same_name_prevents_clobbering_even_if_collision_exists_on_disk() {
    let roots = testutil::setup_filesystem();

    let config = NoteConfig {
        file_extension: "txt".to_string(),
        root_dir: roots.note_root.path().to_owned(),
        temp_root_override: Some(roots.temp_root.path().to_owned()),
    };

    let mut editor = AppendEditor::new();

    editor.note_contents("hello, world!\n".to_string());
    let note_path =
        quicknotes::make_note(&config, editor, "my cool note".to_string(), &test_time())
            .expect("could not write note")
            .expect("file has contents, so path should have been returned");

    let original_note_contents = fs::read_to_string(&note_path).expect("failed to open note");

    // precondition for setting up rest of test
    assert_eq!(
        note_path
            .file_name()
            .map(|s| s.to_str().expect("filename is not valid unicode")),
        Some("my-cool-note.txt")
    );

    // Create dummy files for the clobber repair to collide with
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(dbg!(note_path.with_file_name("my-cool-note-1.txt")))
        .expect("could not create dummy note");

    OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(note_path.with_file_name("my-cool-note-2.txt"))
        .expect("could not create dummy note");

    OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(note_path.with_file_name("my-cool-note-3.txt"))
        .expect("could not create dummy note");

    let mut editor = AppendEditor::new();
    editor.note_contents("oh no\n".to_string());
    let second_note_result =
        quicknotes::make_note(&config, editor, "my cool note".to_string(), &test_time());

    let upd_note_path = second_note_result
        .expect("failed to write note")
        .expect("file has contents, so path should have been returned");

    // The new note should not be 1-3
    assert_eq!(
        upd_note_path,
        note_path.with_file_name("my-cool-note-4.txt")
    );

    let upd_original_location_contents =
        fs::read_to_string(&note_path).expect("failed to open note");

    assert_eq!(
        upd_original_location_contents, original_note_contents,
        "original note contents changed"
    );

    let upd_note_contents = fs::read_to_string(&upd_note_path).expect("failed to open note");
    insta::assert_snapshot!(upd_note_contents);
}

#[test]
fn writing_nothing_to_file_results_in_no_file_written() {
    let roots = testutil::setup_filesystem();
    let config = NoteConfig {
        file_extension: "txt".to_string(),
        root_dir: roots.note_root.path().to_owned(),
        temp_root_override: Some(roots.temp_root.path().to_owned()),
    };

    let stored_path = quicknotes::make_note(
        &config,
        AppendEditor::new(),
        "my cool note".to_string(),
        &test_time(),
    )
    .expect("could not write note");

    assert_eq!(stored_path, None);

    let contents = fs::read_dir(roots.note_root).expect("could not read notes dir");
    assert!(contents.into_iter().next().is_none());
}
