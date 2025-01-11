use std::io::{self, BufRead, BufReader, Read};

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, Offset, TimeZone, Timelike};
use itertools::Itertools;
use serde::{de, ser, Deserialize, Deserializer, Serialize, Serializer};
use serde_derive::{Deserialize, Serialize};
use thiserror::Error;
use toml::value::Datetime as TomlDateTime;

/// Holds metadata about the note. This metadata is stored in the first section of the note when
/// stored on disk.
#[derive(Deserialize, Serialize, PartialEq, Eq, Clone, Debug)]
pub struct Preamble {
    pub title: String,
    #[serde(
        serialize_with = "serialize_datetime",
        deserialize_with = "deserialize_datetime"
    )]
    pub created_at: DateTime<FixedOffset>,
}

impl Preamble {
    /// Serialize the preamble for being written to a note. It will be serialized
    /// as a TOML encoded string, between two `---`s. For example
    ///
    /// ```text
    /// ---
    /// title = "my cool note"
    /// created_at = 2015-10-21T07:28:00-07:00
    /// ---
    /// ```
    ///
    /// # Errors
    /// Returns an error if the data stored in the preamble is not serializable at TOML
    pub fn serialize(&self) -> Result<String, SerializeError> {
        let toml_preamble = toml::to_string_pretty(self).map_err(SerializeError)?;
        let serialized = format!("---\n{}\n---", toml_preamble.trim_end());

        Ok(serialized)
    }
}

#[derive(Error, Debug)]
#[error(transparent)]
pub struct SerializeError(toml::ser::Error);

impl Preamble {
    #[must_use]
    pub fn new(title: String, created_at: DateTime<FixedOffset>) -> Self {
        Self { title, created_at }
    }
}

pub fn filename_stem_for_title(title: &str) -> String {
    let base_name = title
        .to_lowercase()
        .split(' ')
        .map(remove_specials)
        .join("-");

    base_name
}

pub fn filename_stem_for_date(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

pub fn extract_preamble<R: Read>(reader: R) -> Result<Preamble, InvalidPreambleError> {
    let mut buffered_reader = BufReader::new(reader);
    ensure_preamble_fence(&mut buffered_reader)?;
    let toml = read_until_closing_fence(&mut buffered_reader)?;

    toml::from_str(&toml).map_err(InvalidPreambleError::DeserializeError)
}

#[derive(Error, Debug)]
pub enum InvalidPreambleError {
    #[error("preamble did not terminate")]
    UnterminatedFence(),

    #[error("'{0}' is not a valid fence")]
    MalformedFence(String),

    #[error("{0}")]
    DeserializeError(toml::de::Error),

    #[error(transparent)]
    IOError(io::Error),
}

fn ensure_preamble_fence<R: BufRead>(mut reader: R) -> Result<(), InvalidPreambleError> {
    let mut text = String::new();
    reader
        .read_line(&mut text)
        .map_err(InvalidPreambleError::IOError)?;

    if text == "---\n" {
        Ok(())
    } else {
        Err(InvalidPreambleError::MalformedFence(text.clone()))
    }
}

fn read_until_closing_fence<R: BufRead>(mut reader: R) -> Result<String, InvalidPreambleError> {
    let mut toml = String::new();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Err(err) => {
                return Err(InvalidPreambleError::IOError(err));
            }

            Ok(0) => {
                return Err(InvalidPreambleError::UnterminatedFence());
            }

            Ok(_n) if line == "---" || line == "---\n" => {
                return Ok(toml);
            }

            Ok(_n) => {
                toml += &line;
            }
        }
    }
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

fn deserialize_datetime<'a, D: Deserializer<'a>>(
    deserializer: D,
) -> Result<chrono::DateTime<FixedOffset>, D::Error> {
    let dt: TomlDateTime = Deserialize::deserialize(deserializer)?;
    let date = dt.date.ok_or(de::Error::custom("missing date"))?;
    let time = dt.time.ok_or(de::Error::custom("missing time"))?;
    let offset = dt
        .offset
        .ok_or(de::Error::custom("missing timezone offset"))?;
    let offset_minutes = match offset {
        toml::value::Offset::Z => 0,
        toml::value::Offset::Custom { minutes } => minutes,
    };
    let offset_seconds = offset_minutes * 60;

    FixedOffset::east_opt(offset_seconds.into())
        .ok_or_else(|| de::Error::custom("offset {offset_minutes} out of range"))?
        .with_ymd_and_hms(
            date.year.into(),
            date.month.into(),
            date.day.into(),
            time.hour.into(),
            time.minute.into(),
            time.second.into(),
        )
        // Take the later of the two times, arbitrarily
        .latest()
        .ok_or_else(|| de::Error::custom("timestamp {dt} is unresolvable"))?
        .with_nanosecond(time.nanosecond)
        .ok_or_else(|| de::Error::custom("timestamp {dt} is unresolvable"))
}

fn utc_offset_seconds<Tz: TimeZone>(dt: &DateTime<Tz>) -> i32 {
    dt.offset().fix().local_minus_utc()
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;
    use stringreader::StringReader;
    use test_case::test_case;

    use super::*;

    #[test]
    fn can_serialize_preamble_as_toml() {
        let preamble = Preamble {
            title: "Hello world".to_string(),
            created_at: FixedOffset::east_opt(-7 * 60 * 60)
                .unwrap()
                .with_ymd_and_hms(2015, 10, 21, 7, 28, 0)
                .single()
                .unwrap(),
        };

        assert_eq!(
            "---\ntitle = \"Hello world\"\ncreated_at = 2015-10-21T07:28:00-07:00\n---",
            preamble.serialize().unwrap()
        );
    }

    #[test_case("---\ntitle = \"Hello world\"\ncreated_at = 2015-10-21T07:28:00-07:00\n---"; "preamble alone")]
    #[test_case("---\ntitle = \"Hello world\"\ncreated_at = 2015-10-21T07:28:00-07:00\n---\nsick notes bro"; "preamble with data after it")]
    fn can_read_preamble(contents: &str) {
        let reader = StringReader::new(contents);

        let preamble = extract_preamble(reader).expect("failed to parse preamble");
        let expected = Preamble {
            title: "Hello world".to_string(),
            created_at: FixedOffset::east_opt(-7 * 60 * 60)
                .unwrap()
                .with_ymd_and_hms(2015, 10, 21, 7, 28, 0)
                .single()
                .unwrap(),
        };

        assert_eq!(preamble, expected);
    }

    #[test]
    fn filename_for_title_converts_to_lowercase() {
        assert_eq!("note", filename_stem_for_title("Note"));
    }

    #[test]
    fn filename_for_title_converts_spaces_to_dashes() {
        assert_eq!(
            "my-awesome-note",
            filename_stem_for_title("my awesome note")
        );
    }

    #[test]
    fn filename_for_title_removes_specials() {
        assert_eq!("im-a-note", filename_stem_for_title("i'm a note"));
    }

    #[test]
    fn filename_for_date_uses_date_in_simple_iso_format() {
        assert_eq!(
            "2015-10-21",
            filename_stem_for_date(NaiveDate::from_ymd_opt(2015, 10, 21).unwrap())
        );
    }
}
