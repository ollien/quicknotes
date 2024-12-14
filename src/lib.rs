use chrono::{NaiveDate, TimeZone};
use io::Write;
use note::{Preamble, SerializeError};
use std::io;
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
};
use tempfile::{Builder as TempFileBuilder, NamedTempFile};
use thiserror::Error;

pub use edit::{CommandEditor, Editor};

mod edit;
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

    #[error("could not store note at {destination:?} as a file exists with the same name. It still exists at {src:?}")]
    NoteClobberPreventedError { src: String, destination: String },
}

pub struct NoteConfig {
    pub root_dir: PathBuf,
    pub file_extension: String,
    pub temp_root_override: Option<PathBuf>,
}

impl NoteConfig {
    pub fn notes_directory_path(&self) -> PathBuf {
        self.root_dir.join(Path::new("notes"))
    }

    pub fn daily_directory_path(&self) -> PathBuf {
        self.root_dir.join(Path::new("daily"))
    }
}

pub fn make_note<E: Editor>(
    config: &NoteConfig,
    editor: E,
    title: String,
) -> Result<(), MakeNoteError> {
    let filename = note::filename_for_title(&title, &config.file_extension);
    let destination_path = config.notes_directory_path().join(filename);

    make_note_at(config, editor, title, &destination_path)
}

pub fn make_or_open_daily<E: Editor>(
    config: &NoteConfig,
    editor: E,
    date: NaiveDate,
) -> Result<(), MakeNoteError> {
    let filename = note::filename_for_date(date, &config.file_extension);
    let destination_path = config.daily_directory_path().join(filename);
    let destination_exists = fs::metadata(&destination_path)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false);

    if destination_exists {
        editor
            .edit(&destination_path)
            .map_err(|err| MakeNoteError::EditorSpawnError {
                editor: editor.name().to_owned(),
                err,
            })?;

        Ok(())
    } else {
        make_note_at(
            config,
            editor,
            date.format("%Y-%m-%d").to_string(),
            &destination_path,
        )
    }
}

fn make_note_at<E: Editor>(
    config: &NoteConfig,
    editor: E,
    title: String,
    destination_path: &Path,
) -> Result<(), MakeNoteError> {
    let tempfile = make_tempfile(config)?;
    let preamble = Preamble::new(title);

    write_preamble(preamble, tempfile.path())?;

    editor
        .edit(tempfile.path())
        .map_err(|err| MakeNoteError::EditorSpawnError {
            editor: editor.name().to_owned(),
            err,
        })?;

    store_note(tempfile, destination_path)
}

fn store_note(tempfile: NamedTempFile, destination: &Path) -> Result<(), MakeNoteError> {
    if let Err(err) = ensure_no_clobber(tempfile.path(), destination) {
        try_preserve_note(tempfile)?;

        return Err(err);
    }

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

fn make_tempfile(config: &NoteConfig) -> Result<NamedTempFile, MakeNoteError> {
    let mut builder = TempFileBuilder::new();
    let builder = builder.suffix(&config.file_extension);

    if let Some(temp_dir) = config.temp_root_override.as_ref() {
        builder
            .tempfile_in(temp_dir)
            .map_err(MakeNoteError::CreateTempfileError)
    } else {
        builder
            .tempfile()
            .map_err(MakeNoteError::CreateTempfileError)
    }
}

fn ensure_no_clobber(src: &Path, destination: &Path) -> Result<(), MakeNoteError> {
    if destination.exists() {
        Err(MakeNoteError::NoteClobberPreventedError {
            src: src.display().to_string(),
            destination: destination.display().to_string(),
        })
    } else {
        Ok(())
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
