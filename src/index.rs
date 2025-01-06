use std::{
    collections::HashMap,
    fs::OpenOptions,
    io,
    path::{Path, PathBuf},
    str::FromStr,
};

use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone};
use rusqlite::{Connection, Row};
use rusqlite_migration::{Migrations, M};
use thiserror::Error;

use crate::{note::Preamble, warning};

const DB_DATE_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.f";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexedNote {
    pub preamble: Preamble,
    pub kind: NoteKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteKind {
    Note,
    Daily,
}

impl NoteKind {
    fn to_sql_enum(self) -> String {
        match self {
            Self::Note => "note".to_string(),
            Self::Daily => "daily".to_string(),
        }
    }

    fn try_from_sql_enum(sql_enum: &str) -> Result<Self, InvalidNoteKindString> {
        match sql_enum {
            "note" => Ok(Self::Note),
            "daily" => Ok(Self::Daily),
            _ => Err(InvalidNoteKindString(sql_enum.to_owned())),
        }
    }
}

#[derive(Error, Debug)]
#[error("invalid note kind '{0}'")]
struct InvalidNoteKindString(String);

pub fn open(path: &Path) -> Result<Connection, OpenError> {
    let mut connection = Connection::open(path).map_err(OpenError::ConnectionOpenError)?;

    setup_database(&mut connection).map_err(OpenError::MigrationError)?;

    Ok(connection)
}

#[derive(Error, Debug)]
pub enum OpenError {
    #[error("could not open index: {0}")]
    ConnectionOpenError(rusqlite::Error),

    #[error("could not setup index: {0}")]
    MigrationError(MigrationError),
}

#[derive(Error, Debug)]
#[error(transparent)]
pub struct MigrationError(#[from] rusqlite_migration::Error);

pub fn reset(path: &Path) -> Result<(), ResetError> {
    OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .map(|_file| ())
        .or_else(|err| {
            if err.kind() == io::ErrorKind::NotFound {
                Ok(())
            } else {
                Err(ResetError(err))
            }
        })
}

#[derive(Error, Debug)]
#[error("could not reset index database: {0}")]
pub struct ResetError(io::Error);

pub fn add_note(
    connection: &mut Connection,
    preamble: &Preamble,
    kind: NoteKind,
    path: &Path,
) -> Result<(), InsertError> {
    let path_string = path
        .to_str()
        .ok_or_else(|| InsertError::BadPath(path.to_owned()))?;

    connection
        .execute(
            "INSERT INTO notes VALUES (?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(filepath) DO UPDATE SET
                    title=?2,
                    created_at=?3,
                    utc_offset_seconds=?4,
                    kind=?5
            ;",
            (
                &path_string,
                &preamble.title,
                preamble.created_at.format(DB_DATE_FORMAT).to_string(),
                preamble.created_at.offset().local_minus_utc(),
                kind.to_sql_enum(),
            ),
        )
        .map(|_rows| ())
        .map_err(InsertError::DatabaseError)
}

#[derive(Error, Debug)]
pub enum InsertError {
    #[error("could not insert into index database: {0}")]
    DatabaseError(rusqlite::Error),

    #[error("cannot insert a non-utf-8 path to the database: {0}")]
    BadPath(PathBuf),
}

pub fn all_notes(
    connection: &mut Connection,
) -> Result<HashMap<PathBuf, IndexedNote>, LookupError> {
    let mut query = connection
        .prepare("SELECT filepath, title, created_at, utc_offset_seconds, kind FROM notes;")?;

    let notes = query
        .query_map([], |row| match unpack_row(row) {
            Err(QueryFailure::DatabaseFailure(err)) => Err(err),
            Err(QueryFailure::InvalidRow(msg)) => {
                // TODO: perhaps we want some kind of read-repair here.
                warning!("{msg}; skipping entry");

                Ok(None)
            }
            Ok((path, preamble)) => Ok(Some((path, preamble))),
        })?
        .filter_map(Result::transpose)
        .collect::<Result<HashMap<_, _>, _>>()?;

    Ok(notes)
}

#[derive(Error, Debug)]
#[error(transparent)]
pub struct LookupError(#[from] rusqlite::Error);

pub fn delete_note(connection: &mut Connection, path: &Path) -> Result<(), DeleteError> {
    let path_string = path
        .to_str()
        .ok_or_else(|| DeleteError::BadPath(path.to_owned()))?;

    connection
        .execute("DELETE FROM notes WHERE filepath = ?;", (&path_string,))
        .map(|_affected| ())
        .map_err(DeleteError::DatabaseError)
}

#[derive(Error, Debug)]
pub enum DeleteError {
    #[error("could not delete from index database: {0}")]
    DatabaseError(rusqlite::Error),

    #[error("cannot delete a non-utf-8 path from the database: {0}")]
    BadPath(PathBuf),
}

fn setup_database(connection: &mut Connection) -> Result<(), MigrationError> {
    migrations().to_latest(connection)?;

    Ok(())
}

fn unpack_row(row: &Row) -> Result<(PathBuf, IndexedNote), QueryFailure> {
    let raw_filepath: String = row.get(0)?;
    let title: String = row.get(1)?;
    let raw_created_at: String = row.get(2)?;
    let raw_utc_offset: i32 = row.get(3)?;
    let raw_kind: String = row.get(4)?;

    let filepath = PathBuf::from_str(&raw_filepath).unwrap(); // infallible error type
    let created_at = datetime_from_database(&raw_created_at, raw_utc_offset)?;
    let kind = NoteKind::try_from_sql_enum(&raw_kind)
        .map_err(|err| QueryFailure::InvalidRow(err.to_string()))?;

    Ok((
        filepath,
        IndexedNote {
            kind,
            preamble: Preamble { title, created_at },
        },
    ))
}

fn datetime_from_database(
    timestamp: &str,
    utc_offset_seconds: i32,
) -> Result<DateTime<FixedOffset>, QueryFailure> {
    let offset = FixedOffset::east_opt(utc_offset_seconds).ok_or_else(|| {
        QueryFailure::InvalidRow(format!("Invalid UTC offset \"{utc_offset_seconds}\""))
    })?;

    NaiveDateTime::parse_from_str(timestamp, DB_DATE_FORMAT)
        .map_err(|err| QueryFailure::InvalidRow(format!("Invalid date \"{timestamp}\", {err}")))
        .and_then(|datetime| {
            offset
                .from_local_datetime(&datetime)
                .single()
                .ok_or_else(|| {
                    QueryFailure::InvalidRow(format!(
                        "Invalid date \"{timestamp} at offset {utc_offset_seconds}\""
                    ))
                })
        })
}

enum QueryFailure {
    InvalidRow(String),
    DatabaseFailure(rusqlite::Error),
}

impl From<rusqlite::Error> for QueryFailure {
    fn from(error: rusqlite::Error) -> Self {
        Self::DatabaseFailure(error)
    }
}

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(
            "CREATE TABLE notes (
                filepath TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                created_at DATETIME NOT NULL,
                utc_offset_seconds INTEGER NOT NULL
            );",
        ),
        // Add the notes column. This migration is imperfect, because it classifies everything as 'note',
        // but given the index can be recreated, this is no big deal. Plus, I am the only one who used
        // the version without this :)
        M::up(
            r"
            CREATE TEMPORARY TABLE intermediate_notes AS
                SELECT
                    filepath, title, created_at, utc_offset_seconds, 'note'
                FROM notes;
            DROP TABLE notes;
            CREATE TABLE notes (
                filepath TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                created_at DATETIME NOT NULL,
                utc_offset_seconds INTEGER NOT NULL,
                kind CHECK (kind IN ('note', 'daily')) NOT NULL
            );
            INSERT INTO notes SELECT * FROM intermediate_notes;
            DROP TABLE intermediate_notes;
        ",
        ),
    ])
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, str::FromStr};

    use chrono::{FixedOffset, TimeZone};

    use super::*;

    #[test]
    pub fn migrations_valid() {
        migrations()
            .validate()
            .expect("failed to validate migrations");
    }

    #[test]
    pub fn can_insert_note() {
        let mut connection = Connection::open_in_memory().expect("could not open test database");
        setup_database(&mut connection).expect("could not setup test database");

        let preamble = Preamble {
            title: "Hello world".to_string(),
            created_at: FixedOffset::east_opt(-7 * 60 * 60)
                .unwrap()
                .with_ymd_and_hms(2015, 10, 21, 7, 28, 0)
                .single()
                .unwrap(),
        };

        add_note(
            &mut connection,
            &preamble,
            NoteKind::Note,
            &PathBuf::from_str("/home/ferris/Documents/quicknotes/notes/hello-world.txt").unwrap(),
        )
        .unwrap();
    }

    #[test]
    pub fn can_update_note_by_reinserting() {
        let mut connection = Connection::open_in_memory().expect("could not open test database");
        setup_database(&mut connection).expect("could not setup test database");

        let preamble1 = Preamble {
            title: "Hello world".to_string(),
            created_at: FixedOffset::east_opt(-7 * 60 * 60)
                .unwrap()
                .with_ymd_and_hms(2015, 10, 21, 7, 28, 0)
                .single()
                .unwrap(),
        };

        let preamble2 = Preamble {
            title: "Hello world!!".to_string(),
            ..preamble1
        };

        // insert the first note
        add_note(
            &mut connection,
            &preamble1,
            NoteKind::Note,
            &PathBuf::from_str("/home/ferris/Documents/quicknotes/notes/hello-world.txt").unwrap(),
        )
        .unwrap();

        // ... then update
        add_note(
            &mut connection,
            &preamble2,
            NoteKind::Note,
            &PathBuf::from_str("/home/ferris/Documents/quicknotes/notes/hello-world.txt").unwrap(),
        )
        .expect("Failed to update note");

        let notes = all_notes(&mut connection)
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>();

        // should only have the one note, which is the valid one
        assert_eq!(
            notes,
            [(
                PathBuf::from_str("/home/ferris/Documents/quicknotes/notes/hello-world.txt")
                    .unwrap(),
                IndexedNote {
                    preamble: preamble2,
                    kind: NoteKind::Note
                }
            )]
        );
    }

    #[test]
    pub fn cannot_insert_note_with_invalid_utf8_path() {
        let mut connection = Connection::open_in_memory().expect("could not open test database");

        let preamble = Preamble {
            title: "Hello world".to_string(),
            created_at: FixedOffset::east_opt(-7 * 60 * 60)
                .unwrap()
                .with_ymd_and_hms(2015, 10, 21, 7, 28, 0)
                .single()
                .unwrap(),
        };

        // construct an invalid path (this is platform dependent)
        #[cfg(unix)]
        let path = {
            use std::ffi::OsStr;
            use std::os::unix::ffi::OsStrExt;
            PathBuf::from(OsStr::from_bytes(&[0xFF, 0xFF]))
        };

        // construct an invalid path (this is platform dependent)
        #[cfg(windows)]
        let path = {
            use std::ffi::OsString;
            use std::os::windows::ffi::OsStringExt;
            PathBuf::from(OsString::from_wide(&[0xD800]))
        };

        #[cfg(not(any(unix, windows)))]
        panic!("Cannot run test on neither windows or unix");

        let insert_result = add_note(&mut connection, &preamble, NoteKind::Note, &path);

        assert!(insert_result.is_err());
    }

    #[test]
    pub fn can_select_inserted_notes() {
        let mut connection = Connection::open_in_memory().expect("could not open test database");
        setup_database(&mut connection).expect("could not setup test database");

        let preamble1 = Preamble {
            title: "Hello world".to_string(),
            created_at: FixedOffset::east_opt(-7 * 60 * 60)
                .unwrap()
                .with_ymd_and_hms(2015, 10, 21, 7, 28, 0)
                .single()
                .unwrap(),
        };

        add_note(
            &mut connection,
            &preamble1,
            NoteKind::Note,
            &PathBuf::from_str("/home/ferris/Documents/quicknotes/notes/hello-world.txt").unwrap(),
        )
        .unwrap();

        let preamble2 = Preamble {
            title: "notes notes notes".to_string(),
            created_at: FixedOffset::east_opt(-7 * 60 * 60)
                .unwrap()
                .with_ymd_and_hms(2015, 10, 21, 7, 28, 0)
                .single()
                .unwrap(),
        };

        add_note(
            &mut connection,
            &preamble2,
            NoteKind::Note,
            &PathBuf::from_str("/home/ferris/Documents/quicknotes/notes/notes-notes-notes.txt")
                .unwrap(),
        )
        .unwrap();

        let notes = all_notes(&mut connection).expect("Failed to query notes");

        assert_eq!(
            notes.get(
                &PathBuf::from_str("/home/ferris/Documents/quicknotes/notes/hello-world.txt")
                    .unwrap(),
            ),
            Some(&IndexedNote {
                preamble: preamble1,
                kind: NoteKind::Note
            })
        );

        assert_eq!(
            notes.get(
                &PathBuf::from_str("/home/ferris/Documents/quicknotes/notes/notes-notes-notes.txt")
                    .unwrap(),
            ),
            Some(&IndexedNote {
                preamble: preamble2,
                kind: NoteKind::Note
            })
        );
    }

    #[test]
    pub fn select_all_skips_notes_with_malformed_timestamps() {
        let mut connection = Connection::open_in_memory().expect("could not open test database");
        setup_database(&mut connection).expect("could not setup test database");

        let valid_note_preamble = Preamble {
            title: "This note is valid".to_string(),
            created_at: FixedOffset::east_opt(-7 * 60 * 60)
                .unwrap()
                .with_ymd_and_hms(2015, 10, 21, 7, 28, 0)
                .single()
                .unwrap(),
        };

        add_note(
            &mut connection,
            &valid_note_preamble,
            NoteKind::Note,
            &PathBuf::from_str("/home/ferris/Documents/quicknotes/notes/this-note-is-valid.txt")
                .unwrap(),
        )
        .unwrap();

        connection
            .execute(
                r#"INSERT INTO notes VALUES (
                    "/home/ferris/Documents/quicknotes/notes/this-note-is-not-valid.txt",
                    "This note is not valid",
                    "malformed timestamp",
                    0,
                    'note'
                )"#,
                [],
            )
            .unwrap();

        let notes = all_notes(&mut connection)
            .expect("Failed to query notes")
            .into_iter()
            .collect::<Vec<_>>();

        // should only have the one note, which is the valid one
        assert_eq!(
            notes,
            [(
                PathBuf::from_str("/home/ferris/Documents/quicknotes/notes/this-note-is-valid.txt")
                    .unwrap(),
                IndexedNote {
                    preamble: valid_note_preamble,
                    kind: NoteKind::Note
                }
            )]
        );
    }

    #[test]
    pub fn delete_note_is_idempotent() {
        let mut connection = Connection::open_in_memory().expect("could not open test database");
        setup_database(&mut connection).expect("could not setup test database");

        // if this is ok, the test passes
        delete_note(&mut connection, Path::new("/does/not/exist")).expect("could not delete note");
    }

    #[test]
    pub fn delete_note_removes_notes_from_database() {
        let mut connection = Connection::open_in_memory().expect("could not open test database");
        setup_database(&mut connection).expect("could not setup test database");

        connection
            .execute(
                r#"INSERT INTO notes VALUES (
                    "/home/ferris/Documents/quicknotes/notes/my-cool-note.txt",
                    "Hello, world!",
                    "2015-10-22T07:28:00.000",
                    0,
                    'note'
                )"#,
                [],
            )
            .unwrap();

        let notes = all_notes(&mut connection)
            .expect("Failed to query notes")
            .into_iter()
            .collect::<Vec<_>>();

        // Prove the note is there
        assert!(!notes.is_empty());

        delete_note(
            &mut connection,
            Path::new("/home/ferris/Documents/quicknotes/notes/my-cool-note.txt"),
        )
        .expect("could not delete note");

        let notes = all_notes(&mut connection)
            .expect("Failed to query notes")
            .into_iter()
            .collect::<Vec<_>>();

        // Prove the note is now gone
        assert!(notes.is_empty());
    }
}
