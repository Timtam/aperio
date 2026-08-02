//! Row → domain-type conversions and small helpers shared between the
//! calendar and task modules.

use cal_core::{ColorSource, ContainerColor, SoundConfig};
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use rusqlite::{types::Type, Row};

use crate::{map_json_err, map_sql_err};

/// Parse an RFC3339 datetime that was stored as TEXT.
pub(crate) fn parse_utc(s: &str) -> cal_core::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| cal_core::Error::internal(format!("invalid datetime '{s}': {e}")))
}

/// Format a UTC datetime as RFC3339 (the canonical TEXT form for SQLite).
pub(crate) fn fmt_utc(t: &DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

/// Parse an ISO 8601 date stored as TEXT.
pub(crate) fn parse_date(s: &str) -> cal_core::Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| cal_core::Error::internal(format!("invalid date '{s}': {e}")))
}

pub(crate) fn fmt_date(d: &NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

pub(crate) fn parse_time(s: &str) -> cal_core::Result<NaiveTime> {
    NaiveTime::parse_from_str(s, "%H:%M:%S")
        .map_err(|e| cal_core::Error::internal(format!("invalid time '{s}': {e}")))
}

pub(crate) fn fmt_time(t: &NaiveTime) -> String {
    t.format("%H:%M:%S").to_string()
}

/// Decode JSON into `T`. Treats `NULL` columns as a missing value via
/// [`Option<T>`].
pub(crate) fn decode_json<T: serde::de::DeserializeOwned>(s: &str) -> cal_core::Result<T> {
    serde_json::from_str(s).map_err(map_json_err)
}

pub(crate) fn encode_json<T: serde::Serialize>(value: &T) -> cal_core::Result<String> {
    serde_json::to_string(value).map_err(map_json_err)
}

/// Read an optional TEXT column.
pub(crate) fn opt_text(row: &Row<'_>, idx: usize) -> cal_core::Result<Option<String>> {
    match row.get_ref(idx).map_err(map_sql_err)? {
        rusqlite::types::ValueRef::Null => Ok(None),
        rusqlite::types::ValueRef::Text(b) => Ok(Some(
            std::str::from_utf8(b)
                .map_err(|e| {
                    cal_core::Error::internal(format!("invalid utf8 in column {idx}: {e}"))
                })?
                .to_string(),
        )),
        other => Err(cal_core::Error::internal(format!(
            "expected TEXT at column {idx}, got {:?}",
            other.data_type()
        ))),
    }
}

/// Read a NOT NULL TEXT column.
pub(crate) fn req_text(row: &Row<'_>, idx: usize) -> cal_core::Result<String> {
    opt_text(row, idx)?
        .ok_or_else(|| cal_core::Error::internal(format!("unexpected NULL at column {idx}")))
}

/// Reassemble a [`ContainerColor`] from two persisted columns
/// (`hex` + `source`). Returns `None` if `hex` is NULL.
pub(crate) fn read_container_color(
    row: &Row<'_>,
    hex_idx: usize,
    source_idx: usize,
) -> cal_core::Result<Option<ContainerColor>> {
    let Some(hex) = opt_text(row, hex_idx)? else {
        return Ok(None);
    };
    let source_text = opt_text(row, source_idx)?.unwrap_or_else(|| "custom".to_string());
    let source = match source_text.as_str() {
        "native" => ColorSource::Native,
        "custom" => ColorSource::Custom,
        other => {
            return Err(cal_core::Error::internal(format!(
                "unknown color source '{other}'"
            )));
        }
    };
    Ok(Some(ContainerColor { hex, source }))
}

pub(crate) fn write_container_color(c: &Option<ContainerColor>) -> (Option<&str>, Option<&str>) {
    match c {
        Some(c) => {
            let src = match c.source {
                ColorSource::Native => "native",
                ColorSource::Custom => "custom",
            };
            (Some(c.hex.as_str()), Some(src))
        }
        None => (None, None),
    }
}

/// Decode a JSON-serialised [`SoundConfig`] from a nullable TEXT column.
pub(crate) fn read_sound(row: &Row<'_>, idx: usize) -> cal_core::Result<Option<SoundConfig>> {
    match opt_text(row, idx)? {
        None => Ok(None),
        Some(s) => decode_json(&s).map(Some),
    }
}

pub(crate) fn write_sound(s: &Option<SoundConfig>) -> cal_core::Result<Option<String>> {
    match s {
        None => Ok(None),
        Some(s) => encode_json(s).map(Some),
    }
}

/// Convenience: read a column that must be a BOOL stored as `INTEGER 0/1`.
pub(crate) fn read_bool(row: &Row<'_>, idx: usize) -> cal_core::Result<bool> {
    let raw: i64 = row.get(idx).map_err(|e| match e {
        rusqlite::Error::InvalidColumnType(_, _, t) => {
            cal_core::Error::internal(format!("expected INTEGER at column {idx}, got {:?}", t))
        }
        rusqlite::Error::FromSqlConversionFailure(_, _, _) => {
            cal_core::Error::internal(format!("bad integer at column {idx}"))
        }
        other => map_sql_err(other),
    })?;
    Ok(raw != 0)
}

/// SQLite stores enums as plain TEXT — keeping the string explicit at the
/// boundary makes it easier to grep for valid values.
pub(crate) fn unknown_enum(field: &str, raw: &str) -> cal_core::Error {
    cal_core::Error::internal(format!("unknown {field} value: '{raw}'"))
}

#[allow(dead_code)]
pub(crate) fn _ensure_type(actual: Type, expected: Type, idx: usize) -> cal_core::Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(cal_core::Error::internal(format!(
            "column {idx}: expected {:?}, got {:?}",
            expected, actual
        )))
    }
}
