//! Each chat's time zone, which admins can set, and what follows from it:
//! whether it is day or night there, so pictures can match.

use chrono::{DateTime, Offset, Timelike, Utc};
use chrono_tz::{TZ_VARIANTS, Tz};
use dashmap::DashMap;
use std::sync::LazyLock;

use crate::{BotError, dao::mongo::chat_settings};

/// The zone of a chat that never set one: UTC+8 (东八区). `Etc/GMT-8` is
/// eight hours *ahead* of UTC; those names have the sign reversed.
pub const DEFAULT_TZ: Tz = Tz::Etc__GMTMinus8;

/// Night, when pictures are drawn dark: from 19:00 until 07:00 local time.
const DARK_FROM: u32 = 19;
const DARK_UNTIL: u32 = 7;

/// Zones offered as buttons, with the name shown on each.
pub const COMMON: &[(&str, Tz)] = &[
    ("东八区", Tz::Etc__GMTMinus8),
    ("东京", Tz::Asia__Tokyo),
    ("新加坡", Tz::Asia__Singapore),
    ("新德里", Tz::Asia__Kolkata),
    ("迪拜", Tz::Asia__Dubai),
    ("莫斯科", Tz::Europe__Moscow),
    ("柏林", Tz::Europe__Berlin),
    ("伦敦", Tz::Europe__London),
    ("纽约", Tz::America__New_York),
    ("洛杉矶", Tz::America__Los_Angeles),
    ("悉尼", Tz::Australia__Sydney),
    ("UTC", Tz::UTC),
];

/// Other Chinese names people type, besides those on the buttons.
const ALIASES: &[(&str, Tz)] = &[
    ("北京", Tz::Etc__GMTMinus8),
    ("上海", Tz::Etc__GMTMinus8),
    ("中国", Tz::Etc__GMTMinus8),
    ("香港", Tz::Asia__Hong_Kong),
    ("台北", Tz::Asia__Taipei),
    ("首尔", Tz::Asia__Seoul),
    ("日本", Tz::Asia__Tokyo),
    ("巴黎", Tz::Europe__Paris),
    ("芝加哥", Tz::America__Chicago),
];

/// Light or dark, for pictures that follow the chat's day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}

impl Appearance {
    /// Dark from [`DARK_FROM`] until [`DARK_UNTIL`] on the local clock.
    pub fn at<T: Timelike>(local: &T) -> Self {
        let hour = local.hour();
        if !(DARK_UNTIL..DARK_FROM).contains(&hour) {
            Appearance::Dark
        } else {
            Appearance::Light
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Appearance::Light => "浅色",
            Appearance::Dark => "深色",
        }
    }
}

/// Zones read so far, so a chat's is looked up in MongoDB once.
static CACHE: LazyLock<DashMap<i64, Tz>> = LazyLock::new(DashMap::new);

/// The chat's zone, or [`DEFAULT_TZ`] when it set none or MongoDB can't be
/// reached (that answer is not cached, so it is asked again next time).
pub async fn chat_timezone(chat_id: i64) -> Tz {
    if let Some(tz) = CACHE.get(&chat_id) {
        return *tz;
    }
    match chat_settings::get_timezone(chat_id).await {
        Ok(name) => {
            let tz = name
                .and_then(|name| name.parse::<Tz>().ok())
                .unwrap_or(DEFAULT_TZ);
            CACHE.insert(chat_id, tz);
            tz
        }
        Err(err) => {
            log::warn!("Couldn't read the time zone of chat {chat_id}: {err}");
            DEFAULT_TZ
        }
    }
}

pub async fn set_chat_timezone(chat_id: i64, tz: Tz) -> Result<(), BotError> {
    chat_settings::set_timezone(chat_id, tz.name()).await?;
    CACHE.insert(chat_id, tz);
    Ok(())
}

/// Whether pictures for this chat should be light or dark right now.
pub async fn chat_appearance(chat_id: i64) -> Appearance {
    let tz = chat_timezone(chat_id).await;
    Appearance::at(&Utc::now().with_timezone(&tz))
}

const NUMERALS: [&str; 14] = [
    "一", "二", "三", "四", "五", "六", "七", "八", "九", "十", "十一", "十二", "十三", "十四",
];

/// How the zone is shown: whole-hour zones as `东八区` / `西五区` (and
/// Asia/Shanghai, which is UTC+8 all year, as `东八区` too), UTC as `UTC`,
/// any other zone by its IANA name.
pub fn zone_name(tz: Tz) -> String {
    if tz == Tz::Asia__Shanghai {
        return "东八区".to_string();
    }
    let name = tz.name();
    if matches!(
        name,
        "UTC" | "Etc/UTC" | "Etc/GMT" | "Etc/GMT0" | "Etc/GMT+0" | "Etc/GMT-0"
    ) {
        return "UTC".to_string();
    }
    let Some(offset) = name.strip_prefix("Etc/GMT") else {
        return name.to_string();
    };
    let (side, hours) = match offset.split_at_checked(1) {
        Some(("-", hours)) => ("东", hours),
        Some(("+", hours)) => ("西", hours),
        _ => return name.to_string(),
    };
    match hours
        .parse::<usize>()
        .ok()
        .and_then(|h| NUMERALS.get(h.checked_sub(1)?))
    {
        Some(numeral) => format!("{side}{numeral}区"),
        None => name.to_string(),
    }
}

/// The `Etc/GMT` zone `hours` ahead of UTC (behind when negative).
fn etc_zone(hours: i32) -> Option<Tz> {
    match hours {
        0 => Some(Tz::UTC),
        1..=14 => format!("Etc/GMT-{hours}").parse().ok(),
        -12..=-1 => format!("Etc/GMT+{}", -hours).parse().ok(),
        _ => None,
    }
}

/// The zone's current UTC offset, like `UTC+08:00`.
pub fn utc_offset(tz: Tz, at: DateTime<Utc>) -> String {
    format!("UTC{}", at.with_timezone(&tz).offset().fix())
}

/// A zone from what someone typed: an IANA name in any case
/// (`asia/tokyo`), a whole-hour UTC offset (`+9`, `UTC-5`, `GMT+8`,
/// `+08:00`), a zone as it is shown (`东九区`, `西五区`), or a city name
/// in Chinese (`东京`).
pub fn parse_timezone(input: &str) -> Option<Tz> {
    let input = input.trim();
    if let Some(&(_, tz)) = COMMON
        .iter()
        .chain(ALIASES)
        .find(|(name, _)| *name == input)
    {
        return Some(tz);
    }
    if let Some(tz) = (-12..=14)
        .filter_map(etc_zone)
        .find(|&tz| zone_name(tz) == input)
    {
        return Some(tz);
    }
    if let Some(&tz) = TZ_VARIANTS
        .iter()
        .find(|tz| tz.name().eq_ignore_ascii_case(input))
    {
        return Some(tz);
    }
    let offset = ["UTC", "utc", "GMT", "gmt"]
        .iter()
        .find_map(|prefix| input.strip_prefix(prefix))
        .unwrap_or(input);
    whole_hour_zone(offset)
}

/// `+9`, `-5`, `+08:00` as the matching `Etc/GMT` zone.
fn whole_hour_zone(offset: &str) -> Option<Tz> {
    let (sign, rest) = match offset.as_bytes().first()? {
        b'+' => (1, &offset[1..]),
        b'-' => (-1, &offset[1..]),
        _ => (1, offset),
    };
    let (hours, minutes) = rest.split_once(':').unwrap_or((rest, "0"));
    let hours: i32 = hours.parse().ok()?;
    if minutes.parse::<u32>().ok()? != 0 {
        return None;
    }
    etc_zone(sign * hours)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn reads_names_offsets_and_cities() {
        assert_eq!(parse_timezone("Asia/Tokyo"), Some(Tz::Asia__Tokyo));
        assert_eq!(
            parse_timezone(" america/new_york "),
            Some(Tz::America__New_York)
        );
        assert_eq!(parse_timezone("东京"), Some(Tz::Asia__Tokyo));
        assert_eq!(parse_timezone("上海"), Some(Tz::Etc__GMTMinus8));
        assert_eq!(parse_timezone("东八区"), Some(DEFAULT_TZ));
        assert_eq!(parse_timezone("东九区"), Some(Tz::Etc__GMTMinus9));
        assert_eq!(parse_timezone("西五区"), Some(Tz::Etc__GMTPlus5));
        assert_eq!(parse_timezone("+8"), Some(DEFAULT_TZ));
        assert_eq!(parse_timezone("+9"), Some(Tz::Etc__GMTMinus9));
        assert_eq!(parse_timezone("UTC-5"), Some(Tz::Etc__GMTPlus5));
        assert_eq!(parse_timezone("GMT+8"), Some(Tz::Etc__GMTMinus8));
        assert_eq!(parse_timezone("+08:00"), Some(Tz::Etc__GMTMinus8));
        assert_eq!(parse_timezone("UTC"), Some(Tz::UTC));
        assert_eq!(parse_timezone("0"), Some(Tz::UTC));
        assert_eq!(parse_timezone("+5:30"), None);
        assert_eq!(parse_timezone("+15"), None);
        assert_eq!(parse_timezone("-13"), None);
        assert_eq!(parse_timezone("火星"), None);
    }

    #[test]
    fn etc_zones_are_as_far_ahead_as_typed() {
        let noon = Utc.with_ymd_and_hms(2026, 11, 6, 12, 0, 0).unwrap();
        assert_eq!(utc_offset(parse_timezone("+9").unwrap(), noon), "UTC+09:00");
        assert_eq!(utc_offset(parse_timezone("-5").unwrap(), noon), "UTC-05:00");
    }

    #[test]
    fn dark_from_seven_in_the_evening_until_seven_in_the_morning() {
        let at = |h| {
            Tz::Asia__Shanghai
                .with_ymd_and_hms(2026, 11, 6, h, 30, 0)
                .unwrap()
        };
        assert_eq!(Appearance::at(&at(6)), Appearance::Dark);
        assert_eq!(Appearance::at(&at(7)), Appearance::Light);
        assert_eq!(Appearance::at(&at(18)), Appearance::Light);
        assert_eq!(Appearance::at(&at(19)), Appearance::Dark);
        assert_eq!(Appearance::at(&at(0)), Appearance::Dark);
    }

    #[test]
    fn the_same_moment_is_day_in_one_zone_and_night_in_another() {
        // 12:00 UTC is 20:00 in Shanghai and 07:00 in New York (EST)
        let moment = Utc.with_ymd_and_hms(2026, 11, 6, 12, 0, 0).unwrap();
        let dark = Appearance::at(&moment.with_timezone(&Tz::Asia__Shanghai));
        let light = Appearance::at(&moment.with_timezone(&Tz::America__New_York));
        assert_eq!((dark, light), (Appearance::Dark, Appearance::Light));
        // New York keeps daylight saving: UTC-4 in summer, UTC-5 in winter
        let summer = Utc.with_ymd_and_hms(2026, 7, 1, 12, 0, 0).unwrap();
        assert_eq!(utc_offset(Tz::America__New_York, summer), "UTC-04:00");
        assert_eq!(utc_offset(Tz::America__New_York, moment), "UTC-05:00");
    }

    #[test]
    fn zones_are_shown_by_their_offset_in_chinese() {
        assert_eq!(zone_name(DEFAULT_TZ), "东八区");
        assert_eq!(zone_name(Tz::Asia__Shanghai), "东八区");
        assert_eq!(zone_name(Tz::Etc__GMTMinus9), "东九区");
        assert_eq!(zone_name(Tz::Etc__GMTMinus14), "东十四区");
        assert_eq!(zone_name(Tz::Etc__GMTPlus5), "西五区");
        assert_eq!(zone_name(Tz::Etc__GMTPlus12), "西十二区");
        assert_eq!(zone_name(Tz::UTC), "UTC");
        assert_eq!(zone_name(Tz::Etc__GMT), "UTC");
        assert_eq!(zone_name(Tz::America__New_York), "America/New_York");
        let noon = Utc.with_ymd_and_hms(2026, 11, 6, 12, 0, 0).unwrap();
        assert_eq!(utc_offset(DEFAULT_TZ, noon), "UTC+08:00");
    }

    #[test]
    fn button_zones_fit_in_callback_data() {
        for (_, tz) in COMMON {
            assert!(format!("tz {}", tz.name()).len() <= 64);
        }
    }
}
