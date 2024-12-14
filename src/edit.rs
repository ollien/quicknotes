use std::{io, path::Path, process::Command};

pub trait Editor {
    fn name(&self) -> &str;
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
