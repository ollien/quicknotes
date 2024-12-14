#![warn(clippy::all, clippy::pedantic)]

use anyhow::anyhow;
use chrono::Local;
use clap::{Arg, Command as ClapCommand};
use colored::Colorize;
use directories::{ProjectDirs, UserDirs};
use itertools::Itertools;
use quicknotes::{CommandEditor, NoteConfig};
use serde::{Deserialize, Deserializer};
use serde_derive::{Deserialize, Serialize};
use std::fmt::Display;
use std::fs;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::{env, process};

trait UnwrapOrExit<T> {
    fn unwrap_or_exit(self, msg: &str) -> T;
}

#[derive(Serialize, Deserialize)]
struct OnDiskConfig {
    pub notes_root: PathBuf,
    #[serde(deserialize_with = "deserialize_extension")]
    pub note_file_extension: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub editor_command: Option<String>,
}

impl OnDiskConfig {
    fn unpack(self, fallback_editor_command: &str) -> (NoteConfig, CommandEditor) {
        let editor = CommandEditor::new(
            self.editor_command
                .unwrap_or_else(|| fallback_editor_command.to_owned()),
        );

        let note_config = NoteConfig {
            root_dir: self.notes_root,
            file_extension: ".txt".to_string(),
            temp_root_override: None,
        };

        (note_config, editor)
    }
}

impl<T, E: Display> UnwrapOrExit<T> for Result<T, E> {
    fn unwrap_or_exit(self, msg: &str) -> T {
        match self {
            Ok(value) => value,
            Err(err) => {
                eprintln!("{}: {msg} - {err}", "error".red());

                process::exit(1)
            }
        }
    }
}

fn main() {
    let command = cli_command();
    let (note_config, editor) = load_config()
        .unwrap_or_exit("could not load configuration file")
        .unpack(&fallback_editor());

    match command.get_matches().subcommand() {
        Some(("new", submatches)) => run_new(&note_config, &editor, submatches),
        Some(("daily", _submatches)) => run_daily(&note_config, &editor),
        _ => unreachable!(),
    }
}

fn cli_command() -> ClapCommand {
    ClapCommand::new("qn")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(ClapCommand::new("new").arg(Arg::new("title").num_args(1..).required(true)))
        .subcommand(ClapCommand::new("daily"))
}

fn run_new(config: &NoteConfig, editor: &CommandEditor, args: &clap::ArgMatches) {
    ensure_notes_dir_exists(config).unwrap_or_exit("could not create notes directory");

    let title = args
        .get_many::<String>("title")
        .unwrap_or_default()
        .join(" ");

    quicknotes::make_note(config, editor, title, Local::now())
        .unwrap_or_exit("could not create note");
}

fn run_daily(config: &NoteConfig, editor: &CommandEditor) {
    ensure_daily_dir_exists(config).unwrap_or_exit("could not create dailies directory");
    quicknotes::make_or_open_daily(config, editor, Local::now())
        .unwrap_or_exit("could not create daily note");
}

fn load_config() -> anyhow::Result<OnDiskConfig> {
    let config_file = config_file_path()?;
    match File::open(&config_file) {
        Ok(mut file_handle) => read_config_file(&mut file_handle)
            .map_err(|err| anyhow!("reading {}: {err}", config_file.display())),

        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            ensure_config_directory_exists()?;
            let config_file = config_file_path()?;
            eprintln!(
                "{}: no configuration found; generating one for you at {}",
                "warning".yellow(),
                config_file.display()
            );

            let config = write_default_config(&config_file)?;

            Ok(config)
        }

        Err(e) => Err(e.into()),
    }
}

fn read_config_file<R: Read>(file: &mut R) -> anyhow::Result<OnDiskConfig> {
    let mut raw_config = String::new();
    file.read_to_string(&mut raw_config)?;

    let config = toml::from_str(&raw_config)?;

    Ok(config)
}

fn write_default_config(config_file: &Path) -> anyhow::Result<OnDiskConfig> {
    let config = default_config()?;
    let serialized_config = toml::to_string_pretty(&config)?;
    let mut config_file_handle = File::create(config_file)?;
    write!(config_file_handle, "{serialized_config}")?;

    Ok(config)
}

fn default_config() -> anyhow::Result<OnDiskConfig> {
    let notes_root = default_notes_root()?;
    Ok(OnDiskConfig {
        notes_root,
        note_file_extension: ".md".to_string(),
        editor_command: None,
    })
}

fn fallback_editor() -> String {
    env::var("EDITOR").unwrap_or_else(|_err| "nano".to_string())
}

fn ensure_config_directory_exists() -> anyhow::Result<()> {
    let config_directory = config_directory_path()?;
    ensure_directory_exists(&config_directory)
}

fn ensure_notes_dir_exists(config: &NoteConfig) -> anyhow::Result<()> {
    ensure_directory_exists(&config.notes_directory_path())
}

fn ensure_daily_dir_exists(config: &NoteConfig) -> anyhow::Result<()> {
    ensure_directory_exists(&config.daily_directory_path())
}

fn ensure_directory_exists(path: &Path) -> anyhow::Result<()> {
    match fs::create_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(err) => Err(err.into()),
    }
}

fn config_file_path() -> anyhow::Result<PathBuf> {
    let dir = config_directory_path()?;

    Ok(dir.join(Path::new("config.toml")))
}

fn config_directory_path() -> anyhow::Result<PathBuf> {
    let project_dirs = project_dirs()?;

    Ok(project_dirs.config_dir().to_owned())
}

fn default_notes_root() -> anyhow::Result<PathBuf> {
    let user_dirs = user_dirs()?;
    user_dirs.document_dir().map_or_else(
        || Err(anyhow!("could not locate documents directory")),
        |path| Ok(path.join("quicknotes/")),
    )
}

fn user_dirs() -> anyhow::Result<UserDirs> {
    UserDirs::new().map_or_else(
        || Err(anyhow!("could not locate home directory for current user")),
        Ok,
    )
}

fn project_dirs() -> anyhow::Result<ProjectDirs> {
    // TODO: I guess this means you can't configure this if there is no home directory for
    // the current user. Not typical but is possible.
    ProjectDirs::from("com", "ollien", "quicknotes").map_or_else(
        || Err(anyhow!("could not locate home directory current user")),
        Ok,
    )
}

fn deserialize_extension<'a, D: Deserializer<'a>>(deserializer: D) -> Result<String, D::Error> {
    let ext: String = Deserialize::deserialize(deserializer)?;
    if ext.starts_with('.') {
        Ok(ext)
    } else {
        Ok(format!(".{ext}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quicknotes::Editor;

    #[test]
    fn on_disk_config_unpack_does_not_replace_configured_editor() {
        let disk_config = OnDiskConfig {
            notes_root: Path::new("/home/me/notes").to_owned(),
            note_file_extension: ".txt".to_string(),
            editor_command: Some("vim".to_string()),
        };

        let (_note_config, editor) = disk_config.unpack("emacs");

        assert_eq!(editor.name(), "vim");
    }

    #[test]
    fn on_disk_config_unpacksets_missing_editor() {
        let disk_config = OnDiskConfig {
            notes_root: Path::new("/home/me/notes").to_owned(),
            note_file_extension: ".txt".to_string(),
            editor_command: None,
        };

        let (_note_config, editor) = disk_config.unpack("vim");

        assert_eq!(editor.name(), "vim");
    }

    #[test]
    fn on_disk_config_unwrap_adds_dot_before_extension() {
        let disk_config = OnDiskConfig {
            notes_root: Path::new("/home/me/notes").to_owned(),
            note_file_extension: "txt".to_string(),
            editor_command: None,
        };

        let (note_config, _editor) = disk_config.unpack("vim");

        assert_eq!(note_config.file_extension, ".txt");
    }
}
