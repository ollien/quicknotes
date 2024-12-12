use chrono::{NaiveDate, TimeZone};
use io::Write;
use note::{Preamble, SerializeError};
use std::io;
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::Command,
};
use tempfile::NamedTempFile;
use thiserror::Error;

mod note;

#[derive(Error, Debug)]
pub enum MakeNoteError {
    #[error("could not create temporary file: {0}")]
    CreateTempfileError(io::Error),

    #[error("could not write preamble to file: {0}")]
    PreambleWriteError(io::Error),

    #[error("could not encode preamble to tempfile: {0}")]
    PreambleEncodeError(SerializeError),

    #[error("could not store note at {destination:?}. It still exists at {src:?}: {err}")]
    NoteStoreError {
        src: String,
        destination: String,
        #[source]
        err: io::Error,
    },

    // This should VERY RARELY happen. There are failsafes to make this as hard as possible.
    // You can see why at its usage, but tl;dr tempfile can fail to keep the file (on windows)
    #[error("could not store note. It was unable to be preserved ({keep_error}), and then could not be read for you ({read_error}).")]
    NoteLostError {
        #[source]
        keep_error: io::Error,
        read_error: io::Error,
    },

    #[error("could not spawn editor '{editor}': {err}")]
    EditorSpawnError {
        editor: String,
        #[source]
        err: io::Error,
    },
}

enum SaveAction {
    SaveNote,
    DiscardNote,
}

pub struct Config {
    pub notes_root: PathBuf,
    pub editor_command: String,
    pub note_extension: String,
}

impl Config {
    pub fn notes_directory_path(&self) -> PathBuf {
        self.notes_root.join(Path::new("notes"))
    }

    pub fn daily_directory_path(&self) -> PathBuf {
        self.notes_root.join(Path::new("daily"))
    }
}

pub fn make_note(config: &Config, title: String) -> Result<(), MakeNoteError> {
    let filename = note::filename_for_title(&title, &config.note_extension);
    let destination_path = config.notes_directory_path().join(filename);

    make_note_at(config, title, &destination_path)
}

pub fn make_or_open_daily(config: &Config, date: NaiveDate) -> Result<(), MakeNoteError> {
    let filename = note::filename_for_date(date, &config.note_extension);
    let destination_path = config.notes_directory_path().join(filename);
    let destination_exists = fs::metadata(&destination_path)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false);

    if destination_exists {
        // The editor will do this for us
        let _save_action =
            run_editor(&config.editor_command, &destination_path).map_err(|err| {
                MakeNoteError::EditorSpawnError {
                    editor: config.editor_command.clone(),
                    err,
                }
            })?;

        Ok(())
    } else {
        make_note_at(
            config,
            date.format("%Y-%m-%d").to_string(),
            &destination_path,
        )
    }
}

fn make_note_at(
    config: &Config,
    title: String,
    destination_path: &Path,
) -> Result<(), MakeNoteError> {
    let preamble = Preamble::new(title);
    let tempfile = NamedTempFile::new().map_err(MakeNoteError::CreateTempfileError)?;
    write_preamble(preamble, tempfile.path())?;

    run_editor(&config.editor_command, tempfile.path()).map_err(|err| {
        MakeNoteError::EditorSpawnError {
            editor: config.editor_command.clone(),
            err,
        }
    })?;

    store_note(tempfile, destination_path)
}

fn store_note(tempfile: NamedTempFile, destination: &Path) -> Result<(), MakeNoteError> {
    // copy, don't use tempfile.persist as it does not work across filesystems
    match std::fs::copy(tempfile.path(), destination) {
        Ok(_bytes) => Ok(()),
        Err(err) => {
            let tempfile_path = tempfile.path().to_path_buf();
            try_preserve_note(tempfile)?;

            Err(MakeNoteError::NoteStoreError {
                src: tempfile_path.display().to_string(),
                destination: destination.display().to_string(),
                err,
            })
        }
    }
}

fn try_preserve_note(tempfile: NamedTempFile) -> Result<(), MakeNoteError> {
    // Store the path in case the keep operation fails somehow
    let tempfile_path = tempfile.path().to_path_buf();

    match tempfile.keep() {
        Ok(_result) => Ok(()),
        Err(tempfile::PersistError {
            error: keep_error, ..
        }) => match fs::read_to_string(tempfile_path) {
            Ok(contents) => {
                eprintln!("Your note could not be saved due to an error. Here are its contents");
                println!("{contents}");
                Ok(())
            }
            Err(read_error) => Err(MakeNoteError::NoteLostError {
                keep_error,
                read_error,
            }),
        },
    }
}

fn write_preamble<Tz: TimeZone>(preamble: Preamble<Tz>, path: &Path) -> Result<(), MakeNoteError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(false)
        .open(path)
        .map_err(MakeNoteError::PreambleWriteError)?;

    let serialized_preamble = preamble
        .serialize()
        .map_err(MakeNoteError::PreambleEncodeError)?;

    write!(file, "{}\n\n", serialized_preamble).map_err(MakeNoteError::PreambleWriteError)
}

fn run_editor(editor: &str, path: &Path) -> io::Result<SaveAction> {
    let output = Command::new(editor).arg(path).spawn()?.wait()?;

    if output.success() {
        Ok(SaveAction::SaveNote)
    } else {
        Ok(SaveAction::DiscardNote)
    }
}
