use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Seek};
use std::path::{Path, PathBuf};

use itertools::Itertools;
use regex::Regex;
use sha2::{Digest, Sha256};
use tempfile::TempPath;
use thiserror::Error;

use crate::warning;

pub struct TempFileHandle {
    opened: BufReader<File>,
    path: TempPath,
}

/// Stores the given tempfile into a storage medium. This trait can not be implemented
/// by other modules, in order to avoid heap allocations for handling the error.
pub trait StoreNote: sealed::StoreNote {
    fn store(self, tempfile: TempFileHandle) -> Result<PathBuf, StoreNoteError>;
}

#[derive(Error, Debug)]
#[error(transparent)]
pub struct StoreNoteError {
    inner: InnerStoreNoteError,
}

#[derive(Error, Debug)]
#[error(transparent)]
enum InnerStoreNoteError {
    StoreNoteInError(#[from] StoreNoteInError),
    StoreNoteAtError(#[from] StoreNoteAtError),
}

mod sealed {
    pub trait StoreNote {}

    impl<T: super::StoreNote> StoreNote for T {}
}

// A [`StoreNote`] strategy which stores the note at the given destination,
// regardless of the underlying filesystem's contents. It will not overwrite
// files at the existing location.
pub struct StoreNoteAt {
    pub destination: PathBuf,
}

/// A [`StoreNote`] strategy which stores the note at the given directory, but
/// prevents clobbering existing filenames.
pub struct StoreNoteIn {
    pub storage_directory: PathBuf,
    pub preferred_file_stem: String,
    pub file_extension: String,
}

impl TempFileHandle {
    pub fn open(temppath: TempPath) -> Result<Self, io::Error> {
        let file = File::open(&temppath)?;

        Ok(Self {
            opened: BufReader::new(file),
            path: temppath,
        })
    }
}

impl StoreNote for StoreNoteAt {
    fn store(self, tempfile: TempFileHandle) -> Result<PathBuf, StoreNoteError> {
        self.do_store(tempfile)
            .map_err(|err| StoreNoteError { inner: err.into() })
    }
}

impl StoreNoteAt {
    fn do_store(self, mut tempfile: TempFileHandle) -> Result<PathBuf, StoreNoteAtError> {
        match copy_to_destination(&mut tempfile.opened, &self.destination) {
            Ok(()) => Ok(self.destination),

            Err(err) => {
                let tempfile_path = tempfile.path.display().to_string();
                try_preserve_note(tempfile)?;

                Err(StoreNoteAtError::CopyError {
                    err: err.into(),
                    destination: self.destination.display().to_string(),
                    src: tempfile_path,
                })
            }
        }
    }
}

#[derive(Error, Debug)]
enum StoreNoteAtError {
    #[error("could not store note at {destination}. It still exists at {src:?}: {err}")]
    CopyError {
        src: String,
        destination: String,
        #[source]
        err: io::Error,
    },

    #[error(transparent)]
    TryPreserveNoteError(#[from] TryPreserveNoteError),
}

impl StoreNote for StoreNoteIn {
    fn store(self, tempfile: TempFileHandle) -> Result<PathBuf, StoreNoteError> {
        self.do_store(tempfile)
            .map_err(|err| StoreNoteError { inner: err.into() })
    }
}

impl StoreNoteIn {
    fn do_store(self, mut tempfile: TempFileHandle) -> Result<PathBuf, StoreNoteInError> {
        let mut destination = self
            .storage_directory
            .join(self.preferred_file_stem)
            .with_extension(&self.file_extension);

        // This is a loop to prevent the race where we generate a new filename and
        // something else inserts it quickly. It is technically possible this loops
        // forever, but it is extremely unlikely.
        loop {
            match copy_to_destination(&mut tempfile.opened, &destination) {
                Ok(()) => return Ok(destination),

                Err(err @ CopyToDestinationError::FileSetupError(..))
                    if err.is_destination_exists() =>
                {
                    warning!(
                        "Note already exists at {}, generating new filename...",
                        destination.display()
                    );

                    match generate_unclobbered_destination(&destination) {
                        Ok(new_destination) => {
                            // Loop, and try to store
                            destination = new_destination;
                        }

                        Err(err) => {
                            let tempfile_path = tempfile.path.display().to_string();
                            try_preserve_note(tempfile)?;

                            return Err(StoreNoteInError::NoteClobberPreventionError {
                                err,
                                destination: destination.display().to_string(),
                                src: tempfile_path,
                            });
                        }
                    }
                }

                Err(err) => {
                    let tempfile_path = tempfile.path.display().to_string();
                    try_preserve_note(tempfile)?;

                    return Err(StoreNoteInError::CopyError {
                        err: err.into(),
                        destination: destination.display().to_string(),
                        src: tempfile_path,
                    });
                }
            }
        }
    }
}

#[derive(Error, Debug)]
enum StoreNoteInError {
    #[error("could not store note at {destination}. It still exists at {src:?}: {err}")]
    CopyError {
        src: String,
        destination: String,
        #[source]
        err: io::Error,
    },

    #[error("could not store note at {destination}; file exists with the same name, and could not generate new filename for note. It still exists at {src}: {err}")]
    NoteClobberPreventionError {
        src: String,
        destination: String,
        err: GenerateUnclobberedDestinationError,
    },

    #[error(transparent)]
    TryPreserveNoteError(#[from] TryPreserveNoteError),
}

fn copy_to_destination<R: Read>(mut src: R, to: &Path) -> Result<(), CopyToDestinationError> {
    let mut destination_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)
        .map_err(CopyToDestinationError::FileSetupError)?;

    io::copy(&mut src, &mut destination_file).map_err(CopyToDestinationError::CopyError)?;

    Ok(())
}

#[derive(Error, Debug)]
enum CopyToDestinationError {
    #[error(transparent)]
    FileSetupError(io::Error),

    #[error(transparent)]
    CopyError(io::Error),
}

impl From<CopyToDestinationError> for io::Error {
    fn from(value: CopyToDestinationError) -> io::Error {
        match value {
            CopyToDestinationError::FileSetupError(err)
            | CopyToDestinationError::CopyError(err) => err,
        }
    }
}

impl CopyToDestinationError {
    fn is_destination_exists(&self) -> bool {
        if let Self::FileSetupError(err) = self {
            err.kind() == io::ErrorKind::AlreadyExists
        } else {
            false
        }
    }
}

pub fn store_if_different<S: StoreNote>(
    storage: S,
    mut tempfile: TempFileHandle,
    against: &str,
) -> Result<Option<PathBuf>, StoreIfDifferentError> {
    match is_different(&mut tempfile, against) {
        Ok(false) => Ok(None),

        Ok(true) => {
            let path = storage
                .store(tempfile)
                .map_err(|err| StoreIfDifferentError(err.into()))?;

            Ok(Some(path))
        }

        Err(err) => {
            let path = tempfile.path.to_path_buf();
            try_preserve_note(tempfile).map_err(|err| StoreIfDifferentError(err.into()))?;

            Err(InnerStoreIfDifferentError::CheckFileError { path, err }.into())
        }
    }
}

#[derive(Error, Debug)]
#[error(transparent)]
pub struct StoreIfDifferentError(#[from] InnerStoreIfDifferentError);

#[derive(Error, Debug)]
pub enum InnerStoreIfDifferentError {
    #[error("could not check note before storing it; it still exists at {path}: {err}")]
    CheckFileError {
        path: PathBuf,

        #[source]
        err: io::Error,
    },

    #[error(transparent)]
    TryPreserveNoteError(#[from] TryPreserveNoteError),

    #[error(transparent)]
    StoreNoteError(#[from] StoreNoteError),
}

fn is_different(tempfile: &mut TempFileHandle, against: &str) -> Result<bool, io::Error> {
    let mut against_hasher = Sha256::new();
    against_hasher.update(against.as_bytes());
    let against_hash = against_hasher.finalize();

    let mut file_hasher = Sha256::new();
    io::copy(&mut tempfile.opened, &mut file_hasher)?;

    let file_hash = file_hasher.finalize();
    if against_hash == file_hash {
        return Ok(false);
    }

    tempfile.opened.rewind()?;

    Ok(true)
}

fn try_preserve_note(tempfile: TempFileHandle) -> Result<(), TryPreserveNoteError> {
    // Store the path in case the keep operation fails somehow
    let tempfile_path = tempfile.path.to_path_buf();

    match tempfile.path.keep() {
        Ok(_result) => Ok(()),
        Err(tempfile::PathPersistError {
            error: keep_error, ..
        }) => match fs::read_to_string(tempfile_path) {
            Ok(contents) => {
                warning!("Your note could not be saved due to an error. Here are its contents");
                eprintln!("{contents}");
                Ok(())
            }
            Err(read_error) => Err(TryPreserveNoteError {
                keep_error,
                read_error,
            }),
        },
    }
}

#[derive(Error, Debug)]
#[error("note was unable to be preserved ({keep_error}), and then could not be read for you ({read_error}).")]
pub struct TryPreserveNoteError {
    // This should VERY RARELY happen. There are failsafes to make this as hard as possible.
    // You can see why at its usage, but tl;dr tempfile can fail to keep the file (on windows)
    #[source]
    keep_error: io::Error,
    read_error: io::Error,
}

fn generate_unclobbered_destination(
    path: &Path,
) -> Result<PathBuf, GenerateUnclobberedDestinationError> {
    // These were already both generated from rust strings, so must be UTF-8
    let stem = path
        .file_stem()
        .expect("path is already a full filename")
        .to_str()
        .expect("filename must be UTF-8");

    let extension = path
        .extension()
        .expect("path is already a full filename")
        .to_str()
        .expect("file extension must be UTF-8");

    let dir = path.parent().expect("path is already a full path");
    let destination = find_next_destination_basename(dir, stem, extension)
        .map(|basename| path.with_file_name(basename))?;

    Ok(destination)
}

#[derive(Error, Debug)]
#[error("could not generate new filename for note: {0}")]
struct GenerateUnclobberedDestinationError(#[from] FindNextDestinationBasenameError);

fn find_next_destination_basename(
    dir: &Path,
    stem: &str,
    extension: &str,
) -> Result<String, FindNextDestinationBasenameError> {
    let pattern = Regex::new(&format!(
        r"{}-(\d+).{}",
        regex::escape(stem),
        regex::escape(extension)
    ))
    .unwrap();

    let r = fs::read_dir(dir).map_err(FindNextDestinationBasenameError::ReadDirError)?;
    let suffix_num = r
        .filter_map_ok(|entry| {
            let raw_file_name = entry.file_name();
            let file_name = raw_file_name.to_str()?;
            let captured_suffix = pattern.captures(file_name).and_then(|captures| {
                captures
                    .iter()
                    .nth(1)
                    .expect("pattern must have one capture group")
            });

            captured_suffix.map(|suffix| {
                suffix
                    .as_str()
                    .parse::<u32>()
                    .expect("pattern must guarantee we have a number")
            })
        })
        .try_fold(0, |acc, n_result| n_result.map(|n| acc.max(n)))
        .map(|max| max + 1)
        .map_err(FindNextDestinationBasenameError::ReadDirError)?;

    Ok(format!("{stem}-{suffix_num}.{extension}"))
}

#[derive(Error, Debug)]
enum FindNextDestinationBasenameError {
    #[error("could not read directory contents: {0}")]
    ReadDirError(io::Error),
}
