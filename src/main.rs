#![warn(clippy::all, clippy::pedantic)]

use std::collections::HashMap;
use std::fmt::Display;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::{env, process};

use anyhow::anyhow;
use chrono::{DateTime, FixedOffset, Local, NaiveDate, Timelike};
use chrono_english::Dialect;
use clap::builder::PossibleValuesParser;
use clap::{Arg, Command as ClapCommand};
use colored::Colorize;
use directories::{ProjectDirs, UserDirs};
use itertools::Itertools;
use nucleo_picker::error::PickError;
use nucleo_picker::nucleo::pattern::CaseMatching;
use nucleo_picker::{Picker, PickerOptions, Render};
use quicknotes::{open_note, CommandEditor, IndexedNote, NoteConfig};
use serde::{de, Deserialize, Deserializer};
use serde_derive::{Deserialize, Serialize};

trait UnwrapOrExit<T> {
    fn unwrap_or_exit(self, msg: &str) -> T;
}

#[derive(Clone, Debug)]
struct IndexEntry {
    path: PathBuf,
    note: IndexedNote,
    rendered_title_override: Option<String>,
}

struct IndexedNoteRenderer;

impl IndexEntry {
    fn new(path: PathBuf, note: IndexedNote) -> Self {
        Self {
            path,
            note,
            rendered_title_override: None,
        }
    }
}

impl Render<IndexEntry> for IndexedNoteRenderer {
    type Str<'a> = &'a str;

    fn render<'a>(&self, entry: &'a IndexEntry) -> Self::Str<'a> {
        match &entry.rendered_title_override {
            Some(title_override) => title_override,
            None => &entry.note.preamble.title,
        }
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

        Ok(ext.trim_start_matches('.').into())
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
        Some(("daily", submatches)) => run_daily(&note_config, &editor, submatches),
        Some(("index", _submatches)) => run_index(&note_config),
        Some(("open", submatches)) => run_open(&note_config, &editor, submatches),
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
        .subcommand(
            ClapCommand::new("daily")
                .arg(Arg::new("offset").required(false))
                .about("Open or create a daily note")
                .long_about(
                    concat!(
                        "Open a daily note, or create one one does not already exist.",
                        " Optionally, an offset can be supplied, which is a fuzzy date relative to today.",
                        " Acceptable formats include, but are not limited to,  \"2015-10-21\", \"yesterday\" \"3 days ago\""
                    )
                )
        )
        .subcommand(
            ClapCommand::new("index")
                .about("Index the notes directory")
                .long_about(
                    concat!(
                        "Scan the notes directory, and add the notes there to the index.",
                        " This generally should not be necessary, as opening a note adds it to the index automatically,",
                        " but if notes are edited outside of quicknotes or deleted, then this can be useful."
                    )
                )
        )
        .subcommand(
            ClapCommand::new("open")
            .arg(
                Arg::new("kind")
                    .value_parser(PossibleValuesParser::new(vec!["note", "daily", "all"]))
                    .default_value("note")
            )
            .about("Open an existing note")
            .long_about(
                concat!(
                    "Open an existing note.",
                    " Optionally, the type of note can be specified. Defaults to 'note'",
                    " (i.e. those created with quicknotes new).",
                )
            )
        )
}

fn run_new(config: &NoteConfig, editor: &CommandEditor, args: &clap::ArgMatches) {
    ensure_notes_dir_exists(config).unwrap_or_exit("could not create notes directory");

    let title = args
        .get_many::<String>("title")
        .unwrap_or_default()
        .join(" ");

    let path = quicknotes::make_note(config, editor, title, &Local::now())
        .unwrap_or_exit("could not create note");

    if path.is_none() {
        eprintln!("nothing was written in the note; note discarded");
    }
}

fn run_daily(config: &NoteConfig, editor: &CommandEditor, args: &clap::ArgMatches) {
    ensure_daily_dir_exists(config).unwrap_or_exit("could not create dailies directory");
    let now = Local::now();
    let note_date = args.get_one::<String>("offset").map_or_else(
        || now.date_naive(),
        |offset| {
            fuzzy_offset_from_date(now.date_naive(), offset)
                .unwrap_or_exit("could not parse daily note offset")
        },
    );

    let path = quicknotes::make_or_open_daily(config, editor, note_date, &now)
        .unwrap_or_exit("could not create daily note");

    if path.is_none() {
        eprintln!("nothing was written in the note; note discarded");
    }
}

fn run_index(config: &NoteConfig) {
    ensure_root_dir_exists(config).unwrap_or_exit("could not create root quicknotes directory");

    quicknotes::index_notes(config).unwrap_or_exit("could not index notes");
}

fn run_open(config: &NoteConfig, editor: &CommandEditor, args: &clap::ArgMatches) {
    ensure_root_dir_exists(config).unwrap_or_exit("could not create root quicknotes directory");

    let kind = args
        .get_one::<String>("kind")
        .expect("kind has a default value");

    let indexed_notes = match kind.as_str() {
        "all" => quicknotes::indexed_notes(config).unwrap_or_exit("couldn't load notes"),

        "note" => quicknotes::indexed_notes_with_kind(config, quicknotes::NoteKind::Note)
            .unwrap_or_exit("couldn't load notes"),

        "daily" => quicknotes::indexed_notes_with_kind(config, quicknotes::NoteKind::Daily)
            .unwrap_or_exit("couldn't load notes"),

        _ => unreachable!("invalid argument, should be caught by clap"),
    };

    let mut picker = PickerOptions::new()
        .highlight(true)
        .case_matching(CaseMatching::Smart)
        .picker(IndexedNoteRenderer);

    let picker_injector = picker.injector();

    for entry in build_index_entires(indexed_notes) {
        picker_injector.push(entry);
    }

    if let Some(selected_note) = pick(&mut picker).unwrap_or_exit("could not launch picker") {
        open_note(config, editor, selected_note.note.kind, &selected_note.path)
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

fn build_index_entires(entries: HashMap<PathBuf, IndexedNote>) -> Vec<IndexEntry> {
    entries
        .into_iter()
        .map(|(path, note)| IndexEntry::new(path, note))
        .into_group_map_by(|entry| entry.note.preamble.title.clone())
        .into_iter()
        .flat_map(|(title, entries)| {
            let length = entries.len();

            entries.into_iter().map(move |entry| {
                if length == 1 {
                    entry
                } else {
                    let overridden_title =
                        override_title_with_date(&title, entry.note.preamble.created_at);

                    IndexEntry {
                        rendered_title_override: Some(overridden_title),
                        ..entry
                    }
                }
            })
        })
        .collect::<Vec<_>>()
}

fn override_title_with_date(title: &str, created_at: DateTime<FixedOffset>) -> String {
    let formatted_date = created_at
        .format("(%Y-%m-%d %H:%M:%S)")
        .to_string()
        .bright_blue();

    format!("{title} {formatted_date}")
}

fn pick<T: Send + Sync + 'static, R: Render<T>>(
    picker: &mut Picker<T, R>,
) -> anyhow::Result<Option<&T>> {
    picker.pick().or_else(|err| {
        if let PickError::UserInterrupted = err {
            // A user hitting ctrl-c is no different than esc for this purpose
            Ok(None)
        } else {
            Err(err.into())
        }
    })
}

fn fuzzy_offset_from_date(date: NaiveDate, offset: &str) -> Result<NaiveDate, anyhow::Error> {
    // this will always be valid because 00:00:00 is a valid time
    let marker = date.and_hms_opt(0, 0, 0).unwrap().and_utc();
    let changed = chrono_english::parse_date_string(offset, marker, Dialect::Us)?;
    if changed.num_seconds_from_midnight() > 0 {
        return Err(anyhow!("invalid offset"));
    }

    Ok(changed.date_naive())
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use quicknotes::{Editor, NotePreamble};
    use serde::de::value::StrDeserializer;
    use serde::de::IntoDeserializer;

    use super::*;

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
    fn deserialize_extension_removes_dot_to_file_extension() {
        let deserializer: StrDeserializer<'static, serde::de::value::Error> =
            ".md".into_deserializer();
        let extension = OnDiskConfig::deserialize_extension(deserializer)
            .expect("failed to deserialize extension");

        assert_eq!(extension, "md");
    }

    #[test]
    fn deserialize_extension_preserves_lack_of_dot_in_file_extension() {
        let deserializer: StrDeserializer<'static, serde::de::value::Error> =
            "txt".into_deserializer();
        let extension = OnDiskConfig::deserialize_extension(deserializer)
            .expect("failed to deserialize extension");

        assert_eq!(extension, "txt");
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

    #[test]
    fn fuzzy_offset_from_date_allows_date_based_offsets() {
        let date =
            fuzzy_offset_from_date(NaiveDate::from_ymd_opt(2015, 10, 21).unwrap(), "2 days ago")
                .expect("could not convert from offset");

        assert_eq!(date, NaiveDate::from_ymd_opt(2015, 10, 19).unwrap());
    }

    #[test]
    fn fuzzy_offset_from_date_does_not_allow_time_based_offset() {
        let res = fuzzy_offset_from_date(
            NaiveDate::from_ymd_opt(2015, 10, 21).unwrap(),
            "3 hours ago",
        );

        assert!(
            res.is_err(),
            "should not have been able to perform this conversion"
        );
    }

    #[test]
    fn build_index_entries_does_not_override_titles_for_unique_notes() {
        let make_created_at = |day_offset: u32| {
            FixedOffset::east_opt(-7 * 60 * 60)
                .unwrap()
                .with_ymd_and_hms(2015, 10, 21 + day_offset, 7, 28, 0)
                .single()
                .unwrap()
        };

        let notes = HashMap::from([
            (
                PathBuf::from("/home/ferris/Documents/quicknotes/notes/abc.txt"),
                IndexedNote {
                    preamble: NotePreamble {
                        created_at: make_created_at(0),
                        title: "abc".to_string(),
                    },
                    kind: quicknotes::NoteKind::Note,
                },
            ),
            (
                PathBuf::from("/home/ferris/Documents/quicknotes/notes/def.txt"),
                IndexedNote {
                    preamble: NotePreamble {
                        created_at: make_created_at(1),
                        title: "def".to_string(),
                    },
                    kind: quicknotes::NoteKind::Note,
                },
            ),
            (
                PathBuf::from("/home/ferris/Documents/quicknotes/notes/xyz.txt"),
                IndexedNote {
                    preamble: NotePreamble {
                        created_at: make_created_at(2),
                        title: "xyz".to_string(),
                    },
                    kind: quicknotes::NoteKind::Note,
                },
            ),
        ]);

        let overrides = build_index_entires(notes)
            .into_iter()
            .map(|entry| entry.rendered_title_override)
            .collect::<Vec<_>>();

        assert!(overrides == vec![None, None, None]);
    }

    #[test]
    fn build_index_entries_overrides_titles_of_notes_with_matching_titles() {
        let make_created_at = |day_offset: u32| {
            FixedOffset::east_opt(-7 * 60 * 60)
                .unwrap()
                .with_ymd_and_hms(2015, 10, 21 + day_offset, 7, 28, 0)
                .single()
                .unwrap()
        };

        let notes = HashMap::from([
            (
                PathBuf::from("/home/ferris/Documents/quicknotes/notes/abc.txt"),
                IndexedNote {
                    preamble: NotePreamble {
                        created_at: make_created_at(0),
                        title: "abc".to_string(),
                    },
                    kind: quicknotes::NoteKind::Note,
                },
            ),
            (
                PathBuf::from("/home/ferris/Documents/quicknotes/notes/def.txt"),
                IndexedNote {
                    preamble: NotePreamble {
                        created_at: make_created_at(1),
                        title: "def".to_string(),
                    },
                    kind: quicknotes::NoteKind::Note,
                },
            ),
            (
                PathBuf::from("/home/ferris/Documents/quicknotes/notes/abc2.txt"),
                IndexedNote {
                    preamble: NotePreamble {
                        created_at: make_created_at(2),
                        title: "abc".to_string(),
                    },
                    kind: quicknotes::NoteKind::Note,
                },
            ),
        ]);

        let overrides = build_index_entires(notes)
            .into_iter()
            .map(|entry| (entry.path, entry.rendered_title_override))
            .collect::<HashMap<_, _>>();

        let expected = HashMap::from([
            (
                PathBuf::from("/home/ferris/Documents/quicknotes/notes/def.txt"),
                None,
            ),
            (
                PathBuf::from("/home/ferris/Documents/quicknotes/notes/abc.txt"),
                Some(override_title_with_date("abc", make_created_at(0))),
            ),
            (
                PathBuf::from("/home/ferris/Documents/quicknotes/notes/abc2.txt"),
                Some(override_title_with_date("abc", make_created_at(2))),
            ),
        ]);

        assert_eq!(overrides, expected);
    }

    #[test]
    fn title_override_starts_with_title() {
        let created_at = FixedOffset::east_opt(-7 * 60 * 60)
            .unwrap()
            .with_ymd_and_hms(2015, 10, 21, 7, 28, 0)
            .single()
            .unwrap();

        let title = override_title_with_date("abc", created_at);

        assert!(title.starts_with("abc "));
    }

    #[test]
    fn title_override_contains_the_date() {
        let created_at = FixedOffset::east_opt(-7 * 60 * 60)
            .unwrap()
            .with_ymd_and_hms(2015, 10, 21, 7, 28, 0)
            .single()
            .unwrap();

        let title = override_title_with_date("abc", created_at);

        assert!(title.contains("2015-10-21 07:28:00"));
    }
}
