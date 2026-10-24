//! Время: формат как у DateTimeOffset в .NET («2026-10-20T08:57:03.9704511+00:00»), чтение — любой RFC 3339.

use chrono::{DateTime, SecondsFormat, TimeZone, Utc};

pub type Time = DateTime<Utc>;

pub fn now() -> Time {
    Utc::now()
}

/// «Не задано» — как DateTimeOffset.MinValue в .NET.
pub fn min() -> Time {
    Utc.with_ymd_and_hms(1, 1, 1, 0, 0, 0).single().expect("0001-01-01")
}

pub fn is_min(t: &Time) -> bool {
    *t == min()
}

pub fn format(t: &Time) -> String {
    t.to_rfc3339_opts(SecondsFormat::AutoSi, false)
}

pub fn parse(s: &str) -> Option<Time> {
    DateTime::parse_from_rfc3339(s.trim()).ok().map(|d| d.with_timezone(&Utc))
}

/// serde: обязательное время.
pub mod ser {
    use super::*;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(t: &Time, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format(t))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Time, D::Error> {
        let s = String::deserialize(d)?;
        Ok(parse(&s).unwrap_or_else(min))
    }
}

/// serde: необязательное время (null — нет).
pub mod opt {
    use super::*;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(t: &Option<Time>, s: S) -> Result<S::Ok, S::Error> {
        match t {
            Some(t) => s.serialize_str(&format(t)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Time>, D::Error> {
        let s: Option<String> = Option::deserialize(d)?;
        Ok(s.and_then(|s| parse(&s)))
    }
}

/// Тики .NET (100 нс с 0001-01-01) ↔ время: так хранится mtime в состоянии файлов.
pub const TICKS_AT_UNIX_EPOCH: i64 = 621_355_968_000_000_000;

pub fn to_ticks(t: &Time) -> i64 {
    TICKS_AT_UNIX_EPOCH + t.timestamp() * 10_000_000 + (t.timestamp_subsec_nanos() as i64) / 100
}

pub fn from_ticks(ticks: i64) -> Time {
    let rel = ticks - TICKS_AT_UNIX_EPOCH;
    let secs = rel.div_euclid(10_000_000);
    let nanos = (rel.rem_euclid(10_000_000) * 100) as u32;
    Utc.timestamp_opt(secs, nanos).single().unwrap_or_else(min)
}

pub fn from_system(t: std::time::SystemTime) -> Time {
    DateTime::<Utc>::from(t)
}

/// «12 с назад», «5 мин назад».
pub fn ago(t: &Time) -> String {
    let d = Utc::now().signed_duration_since(*t);
    let s = d.num_seconds().max(0);
    if s < 60 {
        format!("{} с назад", s)
    } else if s < 3600 {
        format!("{} мин назад", s / 60)
    } else if s < 48 * 3600 {
        format!("{} ч назад", s / 3600)
    } else {
        format!("{} дн назад", s / 86400)
    }
}

/// «3 ч 12 мин» / «7 мин» / «40 с».
pub fn skew_text(secs: i64) -> String {
    let s = secs.abs();
    if s >= 3600 {
        format!("{} ч {} мин", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{} мин", s / 60)
    } else {
        format!("{} с", s)
    }
}
