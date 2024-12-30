#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::missing_errors_doc)]

use chrono::{DateTime, TimeZone};
use index::{LookupError, MigrationError};
use io::Write;
use log::warn;
use note::{Preamble, SerializeError};
use rusqlite::Connection;
use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
};
use tempfile::{Builder as TempFileBuilder, NamedTempFile};
use thiserror::Error;
use walkdir::{DirEntry, WalkDir};

pub use edit::{CommandEditor, Editor};
pub use note::Preamble as NotePreamble;

mod edit;
mod index;
mod note;

pub struct NoteConfig {
    pub root_dir: PathBuf,
    pub file_extension: String,
    pub temp_root_override: Option<PathBuf>,
}

impl NoteConfig {
    #[must_use]
    pub fn notes_directory_path(&self) -> PathBuf {
        self.root_dir.join(Path::new("notes"))
    }

    #[must_use]
    pub fn daily_directory_path(&self) -> PathBuf {
        self.root_dir.join(Path::new("daily"))
    }

    #[must_use]
    pub fn index_db_path(&self) -> PathBuf {
        self.root_dir.join(Path::new(".index.sqlite3"))
    }
}

pub fn make_note<E: Editor, Tz: TimeZone>(
    config: &NoteConfig,
    editor: E,
    title: String,
    creation_time: &DateTime<Tz>,
) -> Result<(), MakeNoteError> {
    let filename = note::filename_for_title(&title, &config.file_extension);
    let destination_path = config.notes_directory_path().join(filename);

    make_note_at(config, editor, title, creation_time, &destination_path)
}

pub fn make_or_open_daily<E: Editor, Tz: TimeZone>(
    config: &NoteConfig,
    editor: E,
    creation_time: &DateTime<Tz>,
) -> Result<(), MakeNoteError> {
    let filename = note::filename_for_date(creation_time.date_naive(), &config.file_extension);
    let destination_path = config.daily_directory_path().join(filename);
    let destination_exists = ensure_note_exists(&destination_path)
        .map(|()| true)
        .or_else(|err| {
            if err.kind() == io::ErrorKind::NotFound {
                Ok(false)
            } else {
                Err(MakeNoteError::NoteLookupError {
                    destination: destination_path.display().to_string(),
                    err,
                })
            }
        })?;

    if destination_exists {
        open_note_in_editor(config, editor, &destination_path)?;

        Ok(())
    } else {
        make_note_at(
            config,
            editor,
            creation_time.date_naive().format("%Y-%m-%d").to_string(),
            creation_time,
            &destination_path,
        )
    }
}

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

    #[error("could not check if note exists at {destination:?}: {err}")]
    NoteLookupError {
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

    #[error(transparent)]
    EditorSpawnError(#[from] EditorSpawnError),

    #[error(transparent)]
    IndexOpenError(#[from] IndexOpenError),

    #[error(transparent)]
    IndexNoteError(#[from] IndexNoteError),

    #[error("could not store note at {destination:?} as a file exists with the same name. It still exists at {src:?}")]
    NoteClobberPreventedError { src: String, destination: String },
}

#[derive(Error, Debug)]
#[allow(clippy::enum_variant_names)]
pub enum IndexNoteError {
    #[error("could not open note at {0} for indexing: {1}")]
    OpenError(PathBuf, #[source] io::Error),

    #[error("could not read preamble from note at {0}: {1}")]
    PreambleError(PathBuf, #[source] note::InvalidPreambleError),

    #[error("could not index note at {0}: {1}")]
    IndexError(PathBuf, #[source] index::InsertError),
}

#[derive(Error, Debug)]
#[error("could not spawn editor '{editor}': {err}")]
pub struct EditorSpawnError {
    editor: String,
    #[source]
    err: io::Error,
}

impl From<OpenNoteInEditorError> for MakeNoteError {
    fn from(err: OpenNoteInEditorError) -> Self {
        match err {
            OpenNoteInEditorError::EditorSpawnError(err) => MakeNoteError::EditorSpawnError(err),
            OpenNoteInEditorError::IndexOpenError(err) => MakeNoteError::IndexOpenError(err),
            OpenNoteInEditorError::IndexNoteError(err) => MakeNoteError::IndexNoteError(err),
        }
    }
}

pub fn open_note<E: Editor>(
    config: &NoteConfig,
    editor: E,
    path: &Path,
) -> Result<(), OpenNoteError> {
    ensure_note_exists(path).map_err(OpenNoteError::LookupError)?;
    open_note_in_editor(config, editor, path)?;

    Ok(())
}

#[derive(Error, Debug)]
pub enum OpenNoteError {
    #[error(transparent)]
    LookupError(io::Error),

    #[error(transparent)]
    EditorSpawnError(EditorSpawnError),

    #[error(transparent)]
    IndexOpenError(IndexOpenError),

    #[error(transparent)]
    IndexNoteError(IndexNoteError),
}

impl From<OpenNoteInEditorError> for OpenNoteError {
    fn from(err: OpenNoteInEditorError) -> Self {
        match err {
            OpenNoteInEditorError::EditorSpawnError(err) => OpenNoteError::EditorSpawnError(err),
            OpenNoteInEditorError::IndexOpenError(err) => OpenNoteError::IndexOpenError(err),
            OpenNoteInEditorError::IndexNoteError(err) => OpenNoteError::IndexNoteError(err),
        }
    }
}

pub fn index_notes(config: &NoteConfig) -> Result<(), IndexNotesError> {
    // This is a bit of a hack, but is easier than trying to prune stale entries from
    // the index
    truncate_index_database(config).map_err(IndexNotesError::TruncateError)?;

    let mut connection = open_index_database(config).map_err(IndexNotesError::IndexOpenError)?;

    let file_iterator = WalkDir::new(config.notes_directory_path())
        .into_iter()
        .chain(WalkDir::new(config.daily_directory_path()));

    for entry_res in file_iterator {
        let Ok(entry) = unpack_walkdir_entry_result(entry_res) else {
            continue;
        };

        if !entry.file_type().is_file() {
            continue;
        }

        if let Err(err) = index_note(&mut connection, entry.path()) {
            warn!("{}", err);
        }
    }

    Ok(())
}

#[derive(Error, Debug)]
pub enum IndexNotesError {
    #[error("could not truncate old index database: {0}")]
    TruncateError(io::Error),

    #[error(transparent)]
    IndexOpenError(IndexOpenError),
}

#[derive(Error, Debug)]
pub enum IndexOpenError {
    #[error("could not open index database: {0}")]
    ConnectionOpenError(rusqlite::Error),

    #[error("could not setup index database: {0}")]
    MigrationError(MigrationError),
}

pub fn indexed_notes(config: &NoteConfig) -> Result<HashMap<PathBuf, Preamble>, IndexedNotesError> {
    let mut connection = open_index_database(config).map_err(IndexedNotesError::IndexOpenError)?;

    index::all_notes(&mut connection).map_err(IndexedNotesError::QueryError)
}

#[derive(Error, Debug)]
pub enum IndexedNotesError {
    #[error(transparent)]
    IndexOpenError(IndexOpenError),

    #[error("could not query index database: {0}")]
    QueryError(LookupError),
}

fn open_index_database(config: &NoteConfig) -> Result<Connection, IndexOpenError> {
    let mut connection =
        Connection::open(config.index_db_path()).map_err(IndexOpenError::ConnectionOpenError)?;

    index::setup_database(&mut connection).map_err(IndexOpenError::MigrationError)?;

    Ok(connection)
}

fn unpack_walkdir_entry_result(
    entry_res: Result<DirEntry, walkdir::Error>,
) -> Result<DirEntry, ()> {
    match entry_res {
        Ok(entry) => Ok(entry),
        Err(err) => {
            if let Some(path) = err.path() {
                warn!(
                    "Cannot traverse {}: {}",
                    path.display().to_string(),
                    io::Error::from(err)
                );
            } else {
                warn!("Cannot traverse notes: {}", io::Error::from(err));
            }

            Err(())
        }
    }
}

fn index_note(index_connection: &mut Connection, path: &Path) -> Result<(), IndexNoteError> {
    let mut file =
        File::open(path).map_err(|err| IndexNoteError::OpenError(path.to_owned(), err))?;

    let preamble = note::extract_preamble(&mut file)
        .map_err(|err| IndexNoteError::PreambleError(path.to_owned(), err))?;

    index::add_note(index_connection, &preamble, path)
        .map_err(|err| IndexNoteError::IndexError(path.to_owned(), err))
}

fn make_note_at<E: Editor, Tz: TimeZone>(
    config: &NoteConfig,
    editor: E,
    title: String,
    creation_time: &DateTime<Tz>,
    destination_path: &Path,
) -> Result<(), MakeNoteError> {
    let tempfile = make_tempfile(config)?;
    let preamble = Preamble::new(title, creation_time.fixed_offset());

    write_preamble(&preamble, tempfile.path())?;
    open_in_editor(editor, tempfile.path())?;
    store_note(tempfile, destination_path)?;

    let mut index_connection = open_index_database(config)?;
    index_note(&mut index_connection, destination_path)?;

    Ok(())
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

fn write_preamble(preamble: &Preamble, path: &Path) -> Result<(), MakeNoteError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(false)
        .open(path)
        .map_err(MakeNoteError::PreambleWriteError)?;

    let serialized_preamble = preamble
        .serialize()
        .map_err(MakeNoteError::PreambleEncodeError)?;

    write!(file, "{serialized_preamble}\n\n").map_err(MakeNoteError::PreambleWriteError)
}

fn ensure_note_exists(path: &Path) -> Result<(), io::Error> {
    fs::metadata(path).and_then(|metadata| {
        if metadata.is_dir() {
            Err(io::Error::new(
                io::ErrorKind::IsADirectory,
                "file is a directory",
            ))
        } else {
            Ok(())
        }
    })
}

fn open_note_in_editor<E: Editor>(
    config: &NoteConfig,
    editor: E,
    path: &Path,
) -> Result<(), OpenNoteInEditorError> {
    open_in_editor(editor, path)?;

    let mut index_connection = open_index_database(config)?;

    index_note(&mut index_connection, path)
        .or_else(|err| {
            let IndexNoteError::PreambleError(_path, err) = err else {
                return Err(err)
            };

            match index::delete_note(&mut index_connection, path) {
                Ok(()) => {
                    warn!("After editing, the note could not be reindexed. It has been removed from the index. Original error: {err}");
                    Ok(())
                }

                Err(delete_err) => {
                    warn!("After editing, the note could not be reindexed. There was a subsequent failure that prevented it from being removed from the index, so there is now a stale entry. You can fix this by running `quicknotes index`. Original error: {err}; Delete error: {delete_err}");
                    Ok(())
                }
            }
        })?;

    Ok(())
}

#[derive(Error, Debug)]
#[allow(clippy::enum_variant_names)]
enum OpenNoteInEditorError {
    #[error(transparent)]
    EditorSpawnError(#[from] EditorSpawnError),

    #[error(transparent)]
    IndexOpenError(#[from] IndexOpenError),

    #[error(transparent)]
    IndexNoteError(#[from] IndexNoteError),
}

fn open_in_editor<E: Editor>(editor: E, path: &Path) -> Result<(), EditorSpawnError> {
    editor.edit(path).map_err(|err| EditorSpawnError {
        editor: editor.name().to_owned(),
        err,
    })
}

fn truncate_index_database(config: &NoteConfig) -> Result<(), io::Error> {
    OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(config.index_db_path())
        .map(|_file| ())
        .or_else(|err| {
            if err.kind() == io::ErrorKind::NotFound {
                Ok(())
            } else {
                Err(err)
            }
        })
}
