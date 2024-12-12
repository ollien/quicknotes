use chrono::{DateTime, Datelike, TimeZone, Timelike};
use chrono::{Local, Offset};
use itertools::Itertools;
use serde::Serialize;
use serde::{ser, Serializer};
use serde_derive::Serialize;
use thiserror::Error;
use toml::value::Datetime as TomlDateTime;

#[derive(Serialize)]
pub struct Preamble<Tz: TimeZone> {
    pub title: String,
    #[serde(serialize_with = "serialize_datetime")]
    pub created_at: DateTime<Tz>,
}

#[derive(Error, Debug)]
#[error(transparent)]
pub struct SerializeError(toml::ser::Error);

impl<Tz: TimeZone> Preamble<Tz> {
    pub fn serialize(&self) -> Result<String, SerializeError> {
        let toml_preamble = toml::to_string_pretty(self).map_err(SerializeError)?;
        let serialized = format!("---\n{}\n---", toml_preamble.trim_end());

        Ok(serialized)
    }
}

impl Preamble<Local> {
    pub fn new(title: String) -> Self {
        Self {
            title,
            created_at: chrono::Local::now(),
        }
    }
}

pub fn filename_for_title(title: &str, extension: &str) -> String {
    let base_name = title
        .to_lowercase()
        .split(' ')
        .map(remove_specials)
        .join("-");

    base_name + extension
}

fn remove_specials(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_ascii() || c.is_ascii_alphanumeric())
        .join("")
}

fn serialize_datetime<S: Serializer, T: TimeZone>(
    dt: &DateTime<T>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let toml_datetime = toml_datetime::<_, S>(dt)?;

    toml_datetime.serialize(serializer)
}

fn toml_datetime<Tz: TimeZone, S: Serializer>(dt: &DateTime<Tz>) -> Result<TomlDateTime, S::Error> {
    let utc_offset_minutes = utc_offset_seconds(dt)
        .try_into()
        .map(|seconds: i16| seconds / 60)
        .map_err(|_err| ser::Error::custom("utc offset must fit into a u16"))?;

    let converted = TomlDateTime {
        date: Some(toml::value::Date {
            year: dt
                .year()
                .try_into()
                .map_err(|_err| ser::Error::custom("year must fit into a u16"))?,
            month: dt
                .month()
                .try_into()
                .map_err(|_err| ser::Error::custom("month must fit into a u8"))?,
            day: dt
                .day()
                .try_into()
                .map_err(|_err| ser::Error::custom("day must fit into a u8"))?,
        }),
        time: Some(toml::value::Time {
            hour: dt
                .hour()
                .try_into()
                .map_err(|_err| ser::Error::custom("hour must fit into a u8"))?,
            minute: dt
                .minute()
                .try_into()
                .map_err(|_err| ser::Error::custom("minute must fit into a u8"))?,
            second: dt
                .second()
                .try_into()
                .map_err(|_err| ser::Error::custom("second must fit into a u8"))?,

            nanosecond: dt.nanosecond(),
        }),
        offset: Some(toml::value::Offset::Custom {
            minutes: utc_offset_minutes,
        }),
    };

    Ok(converted)
}

fn utc_offset_seconds<Tz: TimeZone>(dt: &DateTime<Tz>) -> i32 {
    dt.offset().fix().local_minus_utc()
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::*;

    #[test]
    fn can_serialize_preamble_as_toml() {
        let preamble = Preamble {
            title: "Hello world".to_string(),
            created_at: DateTime::from_timestamp(1445437680, 0)
                .unwrap()
                .with_timezone(&FixedOffset::east_opt(-7 * 60 * 60).unwrap()),
        };

        assert_eq!(
            "---\ntitle = \"Hello world\"\ncreated_at = 2015-10-21T07:28:00-07:00\n---",
            preamble.serialize().unwrap()
        );
    }

    #[test]
    fn filename_for_title_converts_to_lowercase() {
        assert_eq!("note.txt", filename_for_title("Note", ".txt"))
    }

    #[test]
    fn filename_for_title_converts_spaces_to_dashes() {
        assert_eq!(
            "my-awesome-note.txt",
            filename_for_title("my awesome note", ".txt")
        )
    }

    #[test]
    fn filename_for_title_removes_specials() {
        assert_eq!("im-a-note.txt", filename_for_title("i'm a note", ".txt"))
    }
}
