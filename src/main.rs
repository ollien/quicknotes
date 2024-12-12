#![warn(clippy::all, clippy::pedantic)]

use anyhow::anyhow;
use clap::{Arg, Command as ClapCommand};
use colored::Colorize;
use directories::{ProjectDirs, UserDirs};
use itertools::Itertools;
use quicknotes::Config;
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
    pub note_file_extension: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub editor_command: Option<String>,
}

impl OnDiskConfig {
    fn into_full_config(self, fallback_editor: String) -> Config {
        Config {
            notes_root: self.notes_root,
            editor_command: self.editor_command.unwrap_or(fallback_editor),
            note_extension: ".txt".to_string(),
        }
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
    let config = load_config()
        .unwrap_or_exit("could not load configuration file")
        .into_full_config(fallback_editor());

    match command.get_matches().subcommand() {
        Some(("new", submatches)) => run_new(&config, submatches),
        Some(("daily", _submatches)) => run_daily(&config),
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

fn run_new(config: &Config, args: &clap::ArgMatches) {
    ensure_notes_dir_exists(config).unwrap_or_exit("could not create notes directory");

    let title = args
        .get_many::<String>("title")
        .unwrap_or_default()
        .join(" ");

    quicknotes::make_note(config, title).unwrap_or_exit("could not create note");
}

fn run_daily(config: &Config) {
    ensure_daily_dir_exists(config).unwrap_or_exit("could not create dailies directory");
    let today = chrono::Local::now().date_naive();

    quicknotes::make_or_open_daily(config, today).unwrap_or_exit("could not create daily note");
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

fn ensure_notes_dir_exists(config: &Config) -> anyhow::Result<()> {
    ensure_directory_exists(&config.notes_directory_path())
}

fn ensure_daily_dir_exists(config: &Config) -> anyhow::Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_disk_config_into_full_config_does_not_replace_configured_editor() {
        let disk_config = OnDiskConfig {
            notes_root: Path::new("/home/me/notes").to_owned(),
            note_file_extension: ".txt".to_string(),
            editor_command: Some("vim".to_string()),
        };

        let config = disk_config.into_full_config("emacs".to_string());

        assert_eq!(config.editor_command, "vim");
    }

    #[test]
    fn on_disk_config_into_full_config_sets_missing_editor() {
        let disk_config = OnDiskConfig {
            notes_root: Path::new("/home/me/notes").to_owned(),
            note_file_extension: ".txt".to_string(),
            editor_command: None,
        };

        let config = disk_config.into_full_config("vim".to_string());

        assert_eq!(config.editor_command, "vim");
    }
}
