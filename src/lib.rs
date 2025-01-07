#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::enum_variant_names)]

use chrono::{DateTime, TimeZone};
use index::{LookupError as IndexLookupError, OpenError as IndexOpenError};
use io::Write;
use note::{Preamble, SerializeError};
use rusqlite::Connection;
use std::collections::HashMap;
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
};
use storage::{StoreNote, StoreNoteAt, StoreNoteError, StoreNoteIn};
use tempfile::{Builder as TempFileBuilder, NamedTempFile, TempPath};
use thiserror::Error;
use walkdir::{DirEntry, WalkDir};

pub use edit::{CommandEditor, Editor};
pub use index::{IndexedNote, NoteKind};
pub use note::Preamble as NotePreamble;

mod edit;
mod index;
mod note;
mod storage;

macro_rules! warning {
        ($($arg:tt)*) => {{
            use colored::Colorize;

            eprint!("{}: ", "warning".yellow());
            eprintln!($($arg)*)
        }};
    }

pub(crate) use warning;

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

/// Create a new note.
///
/// The note will be created in the notes directory, with a name as close to the given title as
/// possible, and then opened in the editor.
///
/// # Errors
///
/// Returns an error if there is an I/O failure creating the note, the editor fails to launch, or
/// if there is a problem adding the note to the index.
pub fn make_note<E: Editor, Tz: TimeZone>(
    config: &NoteConfig,
    editor: E,
    title: String,
    creation_time: &DateTime<Tz>,
) -> Result<PathBuf, MakeNoteError> {
    let filename_stem = note::filename_stem_for_title(&title);
    let store = StoreNoteIn {
        storage_directory: config.notes_directory_path(),
        preferred_file_stem: filename_stem,
        file_extension: config.file_extension.clone(),
    };

    let written_path =
        make_note_with_store(config, store, editor, title, creation_time, NoteKind::Note)?;

    Ok(written_path)
}

/// An error that occurred during a call to [`make_note`]. [errors section](`make_note#Errors`)
/// for more details.
#[derive(Error, Debug)]
#[error(transparent)]
pub struct MakeNoteError {
    #[from]
    inner: MakeNoteAtError,
}

/// Create or open a daily note for the given datetime.
///
/// This operates very similarly to [`make_note`], but the title of the note will be the
/// date part of the creation time. If one already exists, it will be opened instead of
/// creating a new one.
///
/// # Errors
///
/// Returns an error if there is an I/O failure creating the note, the editor fails to launch, or
/// if there is a problem adding the note to the index.
pub fn make_or_open_daily<E: Editor, Tz: TimeZone>(
    config: &NoteConfig,
    editor: E,
    creation_time: &DateTime<Tz>,
) -> Result<PathBuf, MakeOrOpenDailyNoteError> {
    let filename_stem = note::filename_stem_for_date(creation_time.date_naive());
    let destination_path = config
        .daily_directory_path()
        .join(filename_stem)
        .with_extension(&config.file_extension);

    let destination_exists = ensure_note_exists(&destination_path)
        .map(|()| true)
        .or_else(|err| {
            if err.kind() == io::ErrorKind::NotFound {
                Ok(false)
            } else {
                Err(InnerMakeOrOpenDailyNoteError::NoteLookupError {
                    destination: destination_path.display().to_string(),
                    err,
                })
            }
        })?;

    if destination_exists {
        open_existing_note_in_editor(config, editor, NoteKind::Daily, &destination_path)
            .map_err(InnerMakeOrOpenDailyNoteError::from)?;

        Ok(destination_path)
    } else {
        // We should be able to store the note with the date's name.
        //
        // Technically someone could come in and put a file there while we are
        // editing this note, but that is not behavior we really support.
        // That file will not be overwritten.
        //
        // Plus, the dailies directory is separate from the notes directory,
        // so without manual intervention, one cannot enter this scenario.
        let store = StoreNoteAt {
            destination: destination_path,
        };

        let actual_path = make_note_with_store(
            config,
            store,
            editor,
            creation_time.date_naive().format("%Y-%m-%d").to_string(),
            creation_time,
            NoteKind::Daily,
        )
        .map_err(InnerMakeOrOpenDailyNoteError::from)?;

        Ok(actual_path)
    }
}

/// An error that occurred during a call to [`make_or_open_daily`]. See its
/// [errors section](`make_or_open_daily#Errors`) for more details.
#[derive(Error, Debug)]
#[error(transparent)]
pub struct MakeOrOpenDailyNoteError {
    #[from]
    inner: InnerMakeOrOpenDailyNoteError,
}

#[derive(Error, Debug)]
enum InnerMakeOrOpenDailyNoteError {
    #[error("could not check if note exists at {destination:?}: {err}")]
    NoteLookupError {
        destination: String,
        #[source]
        err: io::Error,
    },

    #[error("could not open daily note: {0}")]
    OpenNoteError(#[from] OpenExistingNoteInEditorError),

    #[error("could not create new daily note: {0}")]
    MakeNoteAtError(#[from] MakeNoteAtError),
}

/// Open an existing note at the given path in the editor.
///
/// # Errors
///
/// Returns an error if there was an I/O problem locating the existing note, the editor
/// fails to launch, or there is a problem updating the note's entry in the index.
pub fn open_note<E: Editor>(
    config: &NoteConfig,
    editor: E,
    kind: NoteKind,
    path: &Path,
) -> Result<(), OpenNoteError> {
    open_existing_note(config, editor, kind, path)?;

    Ok(())
}

#[derive(Error, Debug)]
#[error(transparent)]
pub struct OpenNoteError {
    #[from]
    inner: OpenExistingNoteError,
}

/// Index all notes in the notes and dailies directories. This will also remove deleted files
/// from the index.
///
/// # Errors
///
/// Returns an error if there is a problem opening or the index.
///
/// Note that this will return `Ok` if there is a problem indexing an individual note, but a
/// warning will be printed to stderr.
pub fn index_notes(config: &NoteConfig) -> Result<(), IndexNotesError> {
    index_all_notes(config)?;

    Ok(())
}

#[derive(Error, Debug)]
#[error(transparent)]
pub struct IndexNotesError {
    #[from]
    inner: IndexAllNotesError,
}

/// Get all of the notes currently stored in the index, and metadata about them.
///
/// The returned `HashMap` maps from the path where the note to the metadata stored in its preamble.
///
/// # Errors
///
/// Returns an error if there was a problem opening or reading from the index.
pub fn indexed_notes(
    config: &NoteConfig,
) -> Result<HashMap<PathBuf, IndexedNote>, IndexedNotesError> {
    let notes = all_indexed_notes(config)?;

    Ok(notes)
}

#[derive(Error, Debug)]
#[error(transparent)]
pub struct IndexedNotesError {
    #[from]
    inner: AllIndexedNotesError,
}

fn make_note_with_store<E: Editor, Tz: TimeZone, S: StoreNote>(
    config: &NoteConfig,
    store: S,
    editor: E,
    title: String,
    creation_time: &DateTime<Tz>,
    kind: NoteKind,
) -> Result<PathBuf, MakeNoteAtError> {
    let tempfile = make_tempfile(config).map_err(MakeNoteAtError::CreateTempfileError)?;
    let preamble = Preamble::new(title, creation_time.fixed_offset());

    write_preamble(&preamble, &tempfile)?;
    open_in_editor(editor, &tempfile)?;

    let actual_destination_path = store
        .store(tempfile)
        .map_err(MakeNoteAtError::StoreNoteError)?;

    let mut index_connection = open_index_database(config)?;
    index_note(&mut index_connection, kind, &actual_destination_path)?;

    Ok(actual_destination_path)
}

#[derive(Error, Debug)]
#[error(transparent)]
enum MakeNoteAtError {
    #[error("could not create temporary file: {0}")]
    CreateTempfileError(io::Error),

    #[error("could not write preamble to file: {0}")]
    WritePreambleError(#[from] WritePreambleError),

    #[error(transparent)]
    StoreNoteError(StoreNoteError),

    #[error(transparent)]
    EditorSpawnError(#[from] OpenInEditorError),

    #[error(transparent)]
    IndexNoteError(#[from] IndexNoteError),

    #[error(transparent)]
    IndexOpenError(#[from] IndexOpenError),
}

fn make_tempfile(config: &NoteConfig) -> Result<TempPath, io::Error> {
    let mut builder = TempFileBuilder::new();
    let builder = builder.suffix(&config.file_extension);

    if let Some(temp_dir) = config.temp_root_override.as_ref() {
        builder
            .tempfile_in(temp_dir)
            .map(NamedTempFile::into_temp_path)
    } else {
        builder.tempfile().map(NamedTempFile::into_temp_path)
    }
}

fn write_preamble(preamble: &Preamble, path: &Path) -> Result<(), WritePreambleError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(false)
        .open(path)
        .map_err(WritePreambleError::OpenError)?;

    let serialized_preamble = preamble.serialize()?;

    write!(file, "{serialized_preamble}\n\n").map_err(WritePreambleError::WriteError)
}

#[derive(Error, Debug)]
#[error(transparent)]
enum WritePreambleError {
    OpenError(io::Error),
    EncodeError(#[from] SerializeError),
    WriteError(io::Error),
}

fn open_existing_note<E: Editor>(
    config: &NoteConfig,
    editor: E,
    kind: NoteKind,
    path: &Path,
) -> Result<(), OpenExistingNoteError> {
    ensure_note_exists(path).map_err(OpenExistingNoteError::LookupError)?;
    open_existing_note_in_editor(config, editor, kind, path)?;

    Ok(())
}

#[derive(Error, Debug)]
#[error(transparent)]
enum OpenExistingNoteError {
    #[error("could not open note: {0}")]
    LookupError(io::Error),

    #[error(transparent)]
    OpenNoteInEditorError(#[from] OpenExistingNoteInEditorError),
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

fn open_existing_note_in_editor<E: Editor>(
    config: &NoteConfig,
    editor: E,
    kind: NoteKind,
    path: &Path,
) -> Result<(), OpenExistingNoteInEditorError> {
    open_in_editor(editor, path)?;

    let mut index_connection = open_index_database(config)?;

    index_note(&mut index_connection, kind, path)
        .or_else(|err| {
            let IndexNoteError::PreambleError(err) = err else {
                return Err(err)
            };

            match index::delete_note(&mut index_connection, path) {
                Ok(()) => {
                    warning!("After editing, the note could not be reindexed. It has been removed from the index. Original error: {err}");
                    Ok(())
                }

                Err(delete_err) => {
                    warning!("After editing, the note could not be reindexed. There was a subsequent failure that prevented it from being removed from the index, so there is now a stale entry. You can fix this by running `quicknotes index`. Original error: {err}; Delete error: {delete_err}");
                    Ok(())
                }
            }
        })?;

    Ok(())
}

#[derive(Error, Debug)]
#[allow(clippy::enum_variant_names)]
enum OpenExistingNoteInEditorError {
    #[error(transparent)]
    EditorSpawnError(#[from] OpenInEditorError),

    #[error(transparent)]
    IndexOpenError(#[from] IndexOpenError),

    #[error(transparent)]
    IndexNoteError(#[from] IndexNoteError),
}

fn open_in_editor<E: Editor>(editor: E, path: &Path) -> Result<(), OpenInEditorError> {
    editor.edit(path).map_err(|err| OpenInEditorError {
        editor: editor.name().to_owned(),
        err,
    })
}

#[derive(Error, Debug)]
#[error("could not spawn editor '{editor}': {err}")]
struct OpenInEditorError {
    editor: String,
    #[source]
    err: io::Error,
}

fn index_all_notes(config: &NoteConfig) -> Result<(), IndexAllNotesError> {
    // This is a bit of a hack, but is easier than trying to prune stale entries from
    // the index
    reset_index_database(config)?;
    let mut connection = open_index_database(config)?;

    for (kind, path) in note_file_paths(config) {
        if let Err(err) = index_note(&mut connection, kind, &path) {
            warning!("could not index note at {}: {}", path.display(), err);
        }
    }

    Ok(())
}

#[derive(Error, Debug)]
enum IndexAllNotesError {
    #[error(transparent)]
    IndexResetError(#[from] index::ResetError),

    #[error(transparent)]
    IndexOpenError(#[from] IndexOpenError),
}

fn all_indexed_notes(
    config: &NoteConfig,
) -> Result<HashMap<PathBuf, IndexedNote>, AllIndexedNotesError> {
    let mut connection = open_index_database(config)?;
    let notes = index::all_notes(&mut connection)?;

    Ok(notes)
}

#[derive(Error, Debug)]
enum AllIndexedNotesError {
    #[error(transparent)]
    IndexOpenError(#[from] IndexOpenError),

    #[error("could not query index database: {0}")]
    QueryError(#[from] IndexLookupError),
}

fn reset_index_database(config: &NoteConfig) -> Result<(), index::ResetError> {
    index::reset(&config.index_db_path())
}

fn open_index_database(config: &NoteConfig) -> Result<Connection, IndexOpenError> {
    index::open(&config.index_db_path())
}

/// Get all note file paths in a best-effort fashion. If there is an error where some
/// notes cannot be read, warnings will be logged.
fn note_file_paths(config: &NoteConfig) -> impl Iterator<Item = (NoteKind, PathBuf)> {
    WalkDir::new(config.notes_directory_path())
        .into_iter()
        .map(|entry| (NoteKind::Note, entry))
        .chain(
            WalkDir::new(config.daily_directory_path())
                .into_iter()
                .map(|entry| (NoteKind::Daily, entry)),
        )
        .filter_map(|(note_kind, entry_res)| {
            // skip entires we can't read, so we can get the rest
            unpack_walkdir_entry_result(entry_res)
                .ok()
                .and_then(|entry| {
                    let isnt_dir = !entry.file_type().is_dir();
                    isnt_dir.then_some((note_kind, entry.into_path()))
                })
        })
}

fn unpack_walkdir_entry_result(
    entry_res: Result<DirEntry, walkdir::Error>,
) -> Result<DirEntry, ()> {
    match entry_res {
        Ok(entry) => Ok(entry),
        Err(err) => {
            if let Some(path) = err.path() {
                warning!(
                    "Cannot traverse {}: {}",
                    path.display().to_string(),
                    io::Error::from(err)
                );
            } else {
                warning!("Cannot traverse notes: {}", io::Error::from(err));
            }

            Err(())
        }
    }
}

fn index_note(
    index_connection: &mut Connection,
    kind: NoteKind,
    path: &Path,
) -> Result<(), IndexNoteError> {
    let mut file = File::open(path).map_err(IndexNoteError::OpenError)?;
    let preamble = note::extract_preamble(&mut file).map_err(IndexNoteError::PreambleError)?;

    index::add_note(index_connection, &preamble, kind, path).map_err(IndexNoteError::IndexError)
}

#[derive(Error, Debug)]
#[allow(clippy::enum_variant_names)]
enum IndexNoteError {
    #[error("could not open note for indexing: {0}")]
    OpenError(io::Error),

    #[error("could not read preamble from note: {0}")]
    PreambleError(note::InvalidPreambleError),

    #[error(transparent)]
    IndexError(index::InsertError),
}
