use log::warn;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    str::FromStr,
};

use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone};
use rusqlite::{Connection, Row};
use rusqlite_migration::{Migrations, M};
use thiserror::Error;

use crate::note::Preamble;

const DB_DATE_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.f";

#[derive(Error, Debug)]
#[error(transparent)]
pub struct MigrationError(#[from] rusqlite_migration::Error);

#[derive(Error, Debug)]
#[error(transparent)]
pub struct IndexError(#[from] rusqlite::Error);

#[derive(Error, Debug)]
pub enum InsertError {
    #[error("could not insert into index database: {0}")]
    DatabaseError(rusqlite::Error),

    #[error("cannot insert a non-utf-8 path to the database: {0}")]
    BadPath(PathBuf),
}

pub fn setup_database(connection: &mut Connection) -> Result<(), MigrationError> {
    migrations().to_latest(connection)?;

    Ok(())
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

pub fn add_note(
    connection: &mut Connection,
    preamble: &Preamble,
    path: &Path,
) -> Result<(), InsertError> {
    let path_string = path
        .to_str()
        .ok_or_else(|| InsertError::BadPath(path.to_owned()))?;

    connection
        .execute(
            "INSERT INTO notes VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(filepath) DO UPDATE SET
                    title=?2,
                    created_at=?3,
                    utc_offset_seconds=?4
            ;",
            (
                &path_string,
                &preamble.title,
                preamble.created_at.format(DB_DATE_FORMAT).to_string(),
                preamble.created_at.offset().local_minus_utc(),
            ),
        )
        .map(|_rows| ())
        .map_err(InsertError::DatabaseError)
}

pub fn all_notes(connection: &mut Connection) -> Result<HashMap<PathBuf, Preamble>, IndexError> {
    let mut query =
        connection.prepare("SELECT filepath, title, created_at, utc_offset_seconds FROM notes;")?;

    let notes = query
        .query_map([], |row| match unpack_row(row) {
            Err(QueryFailure::DatabaseFailure(err)) => Err(err),
            Err(QueryFailure::InvalidRow(msg)) => {
                // TODO: perhaps we want some kind of read-repair here.
                warn!("{msg}; skipping entry");

                Ok(None)
            }
            Ok((path, preamble)) => Ok(Some((path, preamble))),
        })?
        .filter_map(Result::transpose)
        .collect::<Result<HashMap<_, _>, _>>()?;

    Ok(notes)
}

fn unpack_row(row: &Row) -> Result<(PathBuf, Preamble), QueryFailure> {
    let raw_filepath: String = row.get(0)?;
    let title: String = row.get(1)?;
    let raw_created_at: String = row.get(2)?;
    let raw_utc_offset: i32 = row.get(3)?;

    let filepath = PathBuf::from_str(&raw_filepath).unwrap(); // infallible error type
    let created_at = datetime_from_database(&raw_created_at, raw_utc_offset)?;

    Ok((filepath, Preamble { title, created_at }))
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

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(
        "CREATE TABLE notes (
            filepath TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            created_at DATETIME NOT NULL,
            utc_offset_seconds INTEGER NOT NULL
        );",
    )])
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
            &PathBuf::from_str("/home/ferris/Documents/quicknotes/notes/hello-world.txt").unwrap(),
        )
        .unwrap();

        // ... then update
        add_note(
            &mut connection,
            &preamble2,
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
                preamble2
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

        let insert_result = add_note(&mut connection, &preamble, &path);

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
            Some(&preamble1)
        );

        assert_eq!(
            notes.get(
                &PathBuf::from_str("/home/ferris/Documents/quicknotes/notes/notes-notes-notes.txt")
                    .unwrap(),
            ),
            Some(&preamble2)
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
                    0
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
                valid_note_preamble
            )]
        );
    }
}
