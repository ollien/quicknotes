use std::{
    fs::{self, OpenOptions},
    io::Write,
};

use chrono::{DateTime, FixedOffset, TimeZone};
use quicknotes::{Editor, NoteConfig};
use tempfile::{tempdir, TempDir};

struct FilesystemRoots {
    note_root: TempDir,
    temp_root: TempDir,
}

fn test_time() -> DateTime<FixedOffset> {
    FixedOffset::east_opt(-7 * 60 * 60)
        .unwrap()
        .with_ymd_and_hms(2015, 10, 21, 7, 28, 0)
        .single()
        .unwrap()
}

fn setup_filesystem() -> FilesystemRoots {
    let note_root = tempdir().expect("could not make temp dir for notes root");
    let temp_root = tempdir().expect("could not make temp dir for temp root");

    std::fs::create_dir(note_root.path().join("notes"))
        .expect("could not make notes dir for testing");
    std::fs::create_dir(note_root.path().join("daily"))
        .expect("could not make daily dir for testing");

    FilesystemRoots {
        note_root,
        temp_root,
    }
}

#[derive(Default)]
struct TestEditor {
    to_insert: Option<String>,
}

impl TestEditor {
    fn new() -> Self {
        TestEditor::default()
    }

    fn note_contents(&mut self, contents: String) {
        self.to_insert = Some(contents);
    }
}

impl Editor for TestEditor {
    fn name(&self) -> &str {
        "test_dir"
    }

    fn edit(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(to_insert) = self.to_insert.as_ref() {
            let mut file = OpenOptions::new()
                .write(true)
                .append(true)
                .open(path)
                .expect("could not open note file for editing");

            write!(file, "{to_insert}")?;
        }
        Ok(())
    }
}

#[test]
fn writes_notes_to_notes_directory() {
    let roots = setup_filesystem();
    let config = NoteConfig {
        file_extension: ".txt".to_string(),
        root_dir: roots.note_root.path().to_owned(),
        temp_root_override: Some(roots.temp_root.path().to_owned()),
    };

    let mut editor = TestEditor::new();
    editor.note_contents("hello, world!\n".to_string());

    quicknotes::make_note(&config, editor, "my cool note".to_string(), test_time())
        .expect("could not write note");

    let expected_note_path = roots.note_root.path().join("notes/my-cool-note.txt");
    let note_contents = fs::read_to_string(expected_note_path).expect("failed to open note");

    insta::assert_snapshot!(note_contents);
}

#[test]
fn writes_dailies_to_notes_directory() {
    let roots = setup_filesystem();
    let config = NoteConfig {
        file_extension: ".txt".to_string(),
        root_dir: roots.note_root.path().to_owned(),
        temp_root_override: Some(roots.temp_root.path().to_owned()),
    };

    let mut editor = TestEditor::new();
    editor.note_contents("today was a cool day\n".to_string());

    quicknotes::make_or_open_daily(&config, editor, test_time()).expect("could not write note");

    let expected_note_path = roots.note_root.path().join("daily/2015-10-21.txt");
    let note_contents = fs::read_to_string(expected_note_path).expect("failed to open note");

    insta::assert_snapshot!(note_contents);
}

#[test]
fn editing_an_existing_daily_alters_the_same_file() {
    let roots = setup_filesystem();
    let config = NoteConfig {
        file_extension: ".txt".to_string(),
        root_dir: roots.note_root.path().to_owned(),
        temp_root_override: Some(roots.temp_root.path().to_owned()),
    };

    let datetime = test_time();
    let mut editor = TestEditor::new();

    editor.note_contents("today was a cool day\n".to_string());
    quicknotes::make_or_open_daily(&config, &editor, datetime).expect("could not write note");

    editor.note_contents("I have more to say!\n".to_string());
    quicknotes::make_or_open_daily(&config, &editor, datetime).expect("could not write note");

    let expected_note_path = roots.note_root.path().join("daily/2015-10-21.txt");
    let note_contents = fs::read_to_string(expected_note_path).expect("failed to open note");

    insta::assert_snapshot!(note_contents);
}
