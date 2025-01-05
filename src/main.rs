#![warn(clippy::all, clippy::pedantic)]

use anyhow::anyhow;
use chrono::Local;
use clap::{Arg, Command as ClapCommand};
use colored::Colorize;
use directories::{ProjectDirs, UserDirs};
use itertools::Itertools;
use nucleo_picker::nucleo::pattern::CaseMatching;
use nucleo_picker::{Picker, PickerOptions, Render};
use quicknotes::{open_note, CommandEditor, NoteConfig, NotePreamble};
use serde::{de, Deserialize, Deserializer};
use serde_derive::{Deserialize, Serialize};
use std::error::Error;
use std::fmt::Display;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::{env, process};

trait UnwrapOrExit<T> {
    fn unwrap_or_exit(self, msg: &str) -> T;
}

struct IndexedNote {
    path: PathBuf,
    preamble: NotePreamble,
}

struct IndexedNoteRenderer;

impl Render<IndexedNote> for IndexedNoteRenderer {
    type Str<'a> = &'a str;

    fn render<'a>(&self, note: &'a IndexedNote) -> Self::Str<'a> {
        &note.preamble.title
    }
}

#[derive(Serialize, Deserialize)]
struct OnDiskConfig {
    #[serde(deserialize_with = "OnDiskConfig::deserialize_notes_root")]
    pub notes_root: PathBuf,

    #[serde(deserialize_with = "OnDiskConfig::deserialize_extension")]
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
            file_extension: self.note_file_extension,
            temp_root_override: None,
        };

        (note_config, editor)
    }

    fn deserialize_extension<'a, D: Deserializer<'a>>(deserializer: D) -> Result<String, D::Error> {
        let ext: String = Deserialize::deserialize(deserializer)?;
        if ext.starts_with('.') {
            Ok(ext)
        } else {
            Ok(format!(".{ext}"))
        }
    }

    fn deserialize_notes_root<'a, D: Deserializer<'a>>(
        deserializer: D,
    ) -> Result<PathBuf, D::Error> {
        let notes_root: PathBuf = Deserialize::deserialize(deserializer)?;
        if notes_root.is_absolute() {
            Ok(notes_root)
        } else {
            Err(de::Error::custom("must be an absolute path"))
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
    let (note_config, editor) = load_config()
        .unwrap_or_exit("could not load configuration file")
        .unpack(&fallback_editor());

    match command.get_matches().subcommand() {
        Some(("new", submatches)) => run_new(&note_config, &editor, submatches),
        Some(("daily", _submatches)) => run_daily(&note_config, &editor),
        Some(("index", _submatches)) => run_index(&note_config),
        Some(("open", _submatches)) => run_open(&note_config, &editor),
        _ => unreachable!(),
    }
}

fn cli_command() -> ClapCommand {
    ClapCommand::new("qn")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            ClapCommand::new("new")
                .arg(Arg::new("title").num_args(1..).required(true))
                .about("Create a new note")
                .long_about(
                    concat!(
                        "Create a new note.",
                        " The title for the note can be entered into the shell directly, including spaces.")
                    ,
            )
        )
        .subcommand(ClapCommand::new("daily").about("Open or create today's daily note"))
        .subcommand(ClapCommand::new("index")
            .about("Index the notes directory")
            .long_about(concat!("Scan the notes directory, and add the notes there to the index.",
            " This generally should not be necessary, as opening a note adds it to the index automatically,",
            " but if notes are edited outside of quicknotes or deleted, then this can be useful.")))
        .subcommand(ClapCommand::new("open").about("Open an existing note"))
}

fn run_new(config: &NoteConfig, editor: &CommandEditor, args: &clap::ArgMatches) {
    ensure_notes_dir_exists(config).unwrap_or_exit("could not create notes directory");

    let title = args
        .get_many::<String>("title")
        .unwrap_or_default()
        .join(" ");

    quicknotes::make_note(config, editor, title, &Local::now())
        .unwrap_or_exit("could not create note");
}

fn run_daily(config: &NoteConfig, editor: &CommandEditor) {
    ensure_daily_dir_exists(config).unwrap_or_exit("could not create dailies directory");
    quicknotes::make_or_open_daily(config, editor, &Local::now())
        .unwrap_or_exit("could not create daily note");
}

fn run_index(config: &NoteConfig) {
    ensure_root_dir_exists(config).unwrap_or_exit("could not create root quicknotes directory");

    quicknotes::index_notes(config).unwrap_or_exit("could not index notes");
}

fn run_open(config: &NoteConfig, editor: &CommandEditor) {
    ensure_root_dir_exists(config).unwrap_or_exit("could not create root quicknotes directory");

    let indexed_notes = quicknotes::indexed_notes(config).unwrap_or_exit("couldn't load notes");
    let mut picker = PickerOptions::new()
        .highlight(true)
        .case_matching(CaseMatching::Smart)
        .picker(IndexedNoteRenderer);

    let picker_injector = picker.injector();

    indexed_notes
        .into_iter()
        .map(|(path, preamble)| IndexedNote { path, preamble })
        .for_each(|note| {
            picker_injector.push(note);
        });

    if let Some(selected_note) = pick(&mut picker).unwrap_or_exit("could not launch picker") {
        open_note(config, editor, &selected_note.path)
            .unwrap_or_exit("could not open selected file");
    }
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

fn ensure_root_dir_exists(config: &NoteConfig) -> anyhow::Result<()> {
    ensure_directory_exists(&config.root_dir)
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

fn pick<T: Send + Sync + 'static, R: Render<T>>(
    picker: &mut Picker<T, R>,
) -> Result<Option<&T>, io::Error> {
    remap_picker_result(picker.pick())
}

fn remap_picker_result<T>(result: Result<Option<T>, io::Error>) -> Result<Option<T>, io::Error> {
    match result {
        Ok(data) => Ok(data),
        Err(err) => {
            // There is no way to other way do this without producing a copy.
            // The library guarantees that this wil be the message for a keyboard interrupt.
            // So, while brittle, it does work.
            #[allow(deprecated)]
            if err.kind() == io::ErrorKind::Other && err.description() == "keyboard interrupt" {
                Ok(None)
            } else {
                Err(err)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quicknotes::Editor;
    use serde::de::{value::StrDeserializer, IntoDeserializer};

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
    fn on_disk_config_unpack_sets_missing_editor() {
        let disk_config = OnDiskConfig {
            notes_root: Path::new("/home/me/notes").to_owned(),
            note_file_extension: ".txt".to_string(),
            editor_command: None,
        };

        let (_note_config, editor) = disk_config.unpack("vim");

        assert_eq!(editor.name(), "vim");
    }

    #[test]
    fn on_disk_config_unpack_copies_file_extension() {
        let disk_config = OnDiskConfig {
            notes_root: Path::new("/home/me/notes").to_owned(),
            note_file_extension: ".md".to_string(),
            editor_command: None,
        };

        let (note_config, _editor) = disk_config.unpack("vim");

        assert_eq!(note_config.file_extension, ".md");
    }

    #[test]
    fn deserialize_extension_adds_dot_to_file_extension() {
        let deserializer: StrDeserializer<'static, serde::de::value::Error> =
            "md".into_deserializer();
        let extension = OnDiskConfig::deserialize_extension(deserializer)
            .expect("failed to deserialize extension");

        assert_eq!(extension, ".md");
    }

    #[test]
    fn deserialize_extension_preserves_dot_in_file_extension() {
        let deserializer: StrDeserializer<'static, serde::de::value::Error> =
            ".txt".into_deserializer();
        let extension = OnDiskConfig::deserialize_extension(deserializer)
            .expect("failed to deserialize extension");

        assert_eq!(extension, ".txt");
    }

    #[test]
    fn deserialize_notes_root_allows_absolute_paths() {
        let deserializer: StrDeserializer<'static, serde::de::value::Error> =
            "/home/ferris/Documents/quicknotes/".into_deserializer();

        let notes_root = OnDiskConfig::deserialize_notes_root(deserializer)
            .expect("failed to deserialize extension");

        assert_eq!(
            "/home/ferris/Documents/quicknotes/",
            notes_root.to_str().unwrap()
        );
    }

    #[test]
    fn deserialize_notes_root_does_not_allow_relative_paths() {
        let deserializer: StrDeserializer<'static, serde::de::value::Error> =
            "Documents/quicknotes/".into_deserializer();

        assert!(OnDiskConfig::deserialize_notes_root(deserializer).is_err());
    }
}
