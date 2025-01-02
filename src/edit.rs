use std::{io, path::Path, process::Command};

/// A text editor that can edit a given note. There is no requirement about the editor itself,
/// just that it can edit a file at a given path.
pub trait Editor {
    fn name(&self) -> &str;
    /// Edit the given note
    ///
    /// # Errors
    ///
    /// Returns an error if the editor had a problem editing the note.
    fn edit(&self, path: &Path) -> io::Result<()>;
}

impl<E: Editor> Editor for &E {
    fn name(&self) -> &str {
        (*self).name()
    }

    fn edit(&self, path: &Path) -> io::Result<()> {
        (*self).edit(path)
    }
}

/// An editor that runs a command to launch. This is useful for CLI tools such as `vim`.
pub struct CommandEditor {
    command: String,
}

impl CommandEditor {
    #[must_use]
    pub fn new(command: String) -> Self {
        Self { command }
    }
}

impl Editor for CommandEditor {
    fn name(&self) -> &str {
        &self.command
    }

    fn edit(&self, path: &Path) -> io::Result<()> {
        Command::new(&self.command)
            .arg(path)
            .spawn()?
            .wait()
            .map(|_output| ())
    }
}
