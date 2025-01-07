// each test file is its own crate, so just because something is used in one place doesn't make it dead
#![allow(dead_code)]

use std::fs::{self, OpenOptions};
use std::io::Write;

use quicknotes::Editor;
use tempfile::{tempdir, NamedTempFile, TempDir};

pub struct FilesystemRoots {
    pub note_root: TempDir,
    pub temp_root: TempDir,
}

#[derive(Default)]
pub struct AppendEditor {
    to_insert: Option<String>,
}

impl AppendEditor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn note_contents(&mut self, contents: String) {
        self.to_insert = Some(contents);
    }
}

impl Editor for AppendEditor {
    fn name(&self) -> &str {
        "test_append_editor"
    }

    fn edit(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(to_insert) = self.to_insert.as_ref() {
            let mut file = OpenOptions::new()
                .append(true)
                .open(path)
                .expect("could not open note file for editing");

            write!(file, "{to_insert}")?;
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct OverwriteEditor {
    to_insert: Option<String>,
}

impl OverwriteEditor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn note_contents(&mut self, contents: String) {
        self.to_insert = Some(contents);
    }
}

impl Editor for OverwriteEditor {
    fn name(&self) -> &str {
        "test_overwrite_editor"
    }

    fn edit(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(to_insert) = self.to_insert.as_ref() {
            let mut file = OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(path)
                .expect("could not open note file for editing");

            write!(file, "{to_insert}")?;
        }

        Ok(())
    }
}

pub struct SwappingEditor<E> {
    inner: E,
}

impl<E: Editor> SwappingEditor<E> {
    pub fn new(editor: E) -> Self {
        Self { inner: editor }
    }
}

impl<E: Editor> Editor for SwappingEditor<E> {
    fn name(&self) -> &str {
        "test_swapping_editor"
    }

    fn edit(&self, path: &std::path::Path) -> std::io::Result<()> {
        // Emulate something like vim, which will move the file into place. This breaks
        // implementations that depend on the original tempfiles inode
        let swap_path = NamedTempFile::new()?.into_temp_path();
        fs::copy(path, &swap_path)?;
        self.inner.edit(&swap_path)?;
        fs::rename(&swap_path, path)?;
        swap_path.keep()?;

        Ok(())
    }
}

pub fn setup_filesystem() -> FilesystemRoots {
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
