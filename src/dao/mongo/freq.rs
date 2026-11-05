use super::*;
use crate::analysis::freq::{Activity, Bucket, Point};
use cached::proc_macro::cached;
use chrono::{Datelike, Days, Duration as ChronoDuration, Months, NaiveDate, Utc};
use chrono_tz::Tz;
use futures::stream::TryStreamExt;
use serde::Deserialize;
use std::{collections::HashMap, time::Duration as StdDuration};

#[derive(Debug, PartialEq, Clone, clap::ValueEnum)]
pub enum Duration {
    #[value(aliases = ["day", "d"])]
    Day,
    #[value(aliases = ["week", "w"])]
    Week,
    #[value(aliases = ["month", "m"])]
    Month,
    #[value(aliases = ["quarter", "q"])]
    Quarter,
    #[value(aliases = ["year", "y"])]
    Year,
    #[value(skip)]
    Invalid,
}

impl<T: Into<String>> From<T> for Duration {
    fn from(value: T) -> Self {
        match value.into().as_str() {
            "day" | "d" => Duration::Day,
            "week" | "w" => Duration::Week,
            "month" | "m" => Duration::Month,
            "quarter" | "q" => Duration::Quarter,
            "year" | "y" => Duration::Year,
            _ => Duration::Invalid,
        }
    }
}

impl Duration {
    /// How far back the period reaches; `None` is every record.
    fn span(&self) -> Option<ChronoDuration> {
        match self {
            Duration::Day => Some(ChronoDuration::days(1)),
            Duration::Week => Some(ChronoDuration::weeks(1)),
            Duration::Month => Some(ChronoDuration::days(30)),
            Duration::Quarter => Some(ChronoDuration::days(90)),
            Duration::Year => Some(ChronoDuration::days(365)),
            Duration::Invalid => None,
        }
    }

    /// How the trend chart groups the period; a single day has no trend.
    fn bucket(&self) -> Option<Bucket> {
        match self {
            Duration::Day => None,
            Duration::Week | Duration::Month => Some(Bucket::Day),
            Duration::Quarter | Duration::Year => Some(Bucket::Week),
            Duration::Invalid => Some(Bucket::Month),
        }
    }

    fn words(&self) -> &'static str {
        match self {
            Duration::Day => "近 24 小时",
            Duration::Week => "近 7 天",
            Duration::Month => "近 30 天",
            Duration::Quarter => "近 90 天",
            Duration::Year => "近一年",
            Duration::Invalid => "全部记录",
        }
    }
}

impl Bucket {
    fn unit(self) -> &'static str {
        match self {
            Bucket::Day => "day",
            Bucket::Week => "week",
            Bucket::Month => "month",
        }
    }

    /// The first day of the bucket holding `day`.
    fn start_of(self, day: NaiveDate) -> NaiveDate {
        match self {
            Bucket::Day => day,
            Bucket::Week => day - Days::new(day.weekday().num_days_from_monday() as u64),
            Bucket::Month => day.with_day(1).unwrap_or(day),
        }
    }

    fn next(self, start: NaiveDate) -> Option<NaiveDate> {
        match self {
            Bucket::Day => start.checked_add_days(Days::new(1)),
            Bucket::Week => start.checked_add_days(Days::new(7)),
            Bucket::Month => start.checked_add_months(Months::new(1)),
        }
    }

    fn label(self, start: NaiveDate) -> String {
        match self {
            Bucket::Day | Bucket::Week => start.format("%m-%d").to_string(),
            Bucket::Month => start.format("%Y-%m").to_string(),
        }
    }
}

/// Messages counted in one local hour of the day, within one trend bucket
/// when the period has a trend.
#[derive(Deserialize)]
struct Row {
    #[serde(rename = "_id")]
    slot: Slot,
    n: i64,
}

#[derive(Deserialize)]
struct Slot {
    /// Hour of the day on the chat's clock, 0 to 23.
    h: i32,
    /// The instant the trend bucket starts on the chat's clock.
    #[serde(default)]
    b: Option<bson::DateTime>,
}

/// Messages per hour of the day and per trend bucket, for one member or for
/// the whole group.
#[derive(Debug, Clone, Default, PartialEq)]
struct Counts {
    hours: [u32; 24],
    /// Keyed by the bucket's first day on the chat's clock.
    trend: HashMap<NaiveDate, u32>,
}

impl Counts {
    fn from_rows(rows: &[Row], tz: Tz) -> Self {
        let mut counts = Counts::default();
        for row in rows {
            let n = u32::try_from(row.n.max(0)).unwrap_or(u32::MAX);
            if let Some(slot) = usize::try_from(row.slot.h)
                .ok()
                .and_then(|h| counts.hours.get_mut(h))
            {
                *slot = slot.saturating_add(n);
            }
            if let Some(start) = row.slot.b {
                let day = start.to_chrono().with_timezone(&tz).date_naive();
                let total = counts.trend.entry(day).or_default();
                *total = total.saturating_add(n);
            }
        }
        counts
    }
}

/// Every bucket from the one holding `from` to the one holding `to`, with
/// the counts found and zero for the rest.
fn fill_trend(
    bucket: Bucket,
    from: NaiveDate,
    to: NaiveDate,
    user: &Counts,
    group: &Counts,
) -> Vec<Point> {
    let mut points = Vec::new();
    let mut start = bucket.start_of(from);
    while start <= to {
        points.push(Point {
            label: bucket.label(start),
            user: user.trend.get(&start).copied().unwrap_or(0),
            group: group.trend.get(&start).copied().unwrap_or(0),
        });
        match bucket.next(start) {
            Some(next) => start = next,
            None => break,
        }
    }
    points
}

/// A query that runs longer than this is cut off: a slow /count must not
/// hold the handler, or the database, for minutes.
const QUERY_LIMIT: StdDuration = StdDuration::from_secs(10);
/// MongoDB's code for a query cut off by `maxTimeMS`.
const MAX_TIME_EXPIRED: i32 = 50;

/// Counts messages in `gid` since `since` (every record when `None`), only
/// `uid`'s when given, by local hour and by trend bucket on the clock of `tz`.
///
/// The pipeline reads nothing but the `(group_id, user_id, timestamp)` index
/// (MongoDB bounds the timestamp per member even with `user_id` unset). It
/// first groups by local hour, one time zone conversion per message, and
/// only then folds the hours into hours of the day and buckets.
async fn count(
    gid: i64,
    uid: Option<i64>,
    since: Option<chrono::DateTime<Utc>>,
    bucket: Option<Bucket>,
    tz: Tz,
) -> Result<Counts, BotError> {
    if is_mongo_disabled() {
        return Err(BotError::Custom("统计服务暂时不可用，稍后再试".to_string()));
    }
    let zone = tz.name();
    let mut filter = doc! { "group_id": gid };
    if let Some(uid) = uid {
        filter.insert("user_id", uid);
    }
    if let Some(since) = since {
        filter.insert(
            "timestamp",
            doc! { "$gte": bson::DateTime::from_chrono(since) },
        );
    }
    let mut slot = doc! { "h": { "$hour": { "date": "$_id", "timezone": zone } } };
    if let Some(bucket) = bucket {
        slot.insert(
            "b",
            doc! {
                "$dateTrunc": {
                    "date": "$_id",
                    "unit": bucket.unit(),
                    "timezone": zone,
                    "startOfWeek": "monday"
                }
            },
        );
    }
    let pipeline = vec![
        doc! { "$match": filter },
        doc! { "$project": { "_id": 0, "timestamp": 1 } },
        doc! {
            "$group": {
                "_id": { "$dateTrunc": { "date": "$timestamp", "unit": "hour", "timezone": zone } },
                "n": { "$sum": 1 }
            }
        },
        doc! { "$group": { "_id": slot, "n": { "$sum": "$n" } } },
    ];
    let rows: Vec<Row> = async {
        botlog()
            .await
            .aggregate(pipeline)
            .max_time(QUERY_LIMIT)
            .await?
            .try_collect::<Vec<_>>()
            .await
    }
    .await
    .map_err(|e| match *e.kind {
        mongodb::error::ErrorKind::Command(ref c) if c.code == MAX_TIME_EXPIRED => {
            BotError::Custom("统计超时了，换个短一点的时间范围试试".to_string())
        }
        _ => BotError::from(e),
    })?
    .into_iter()
    .map(|doc| bson::deserialize_from_document(doc).map_err(mongodb::error::Error::from))
    .collect::<Result<_, _>>()?;
    Ok(Counts::from_rows(&rows, tz))
}

/// The whole group's counts, which every member's /count in the group shares:
/// kept for a minute, and members asking at once wait for one query.
#[cached(
    ttl_secs = 60,
    max_size = 256,
    sync_writes = "by_key",
    key = "String",
    convert = r#"{ format!("{gid}:{duration:?}:{}", tz.name()) }"#
)]
async fn group_counts(gid: i64, duration: Duration, tz: Tz) -> Result<Counts, BotError> {
    let since = duration.span().map(|span| Utc::now() - span);
    count(gid, None, since, duration.bucket(), tz).await
}

/// How one member's messages in a group spread over the period and over the
/// day, next to the whole group's, counted on the clock of `tz`.
pub async fn query_activity(
    gid: i64,
    uid: i64,
    duration: Option<Duration>,
    tz: Tz,
    user_name: String,
    group_name: String,
) -> Result<Activity, BotError> {
    let duration = duration.unwrap_or(Duration::Invalid);
    let now = Utc::now();
    let bucket = duration.bucket();
    let since = duration.span().map(|span| now - span);
    let (user, group) = tokio::try_join!(
        count(gid, Some(uid), since, bucket, tz),
        group_counts(gid, duration.clone(), tz),
    )?;

    let today = now.with_timezone(&tz).date_naive();
    let trend = match bucket {
        Some(bucket) => {
            let from = match since {
                Some(since) => Some(since.with_timezone(&tz).date_naive()),
                None => group.trend.keys().chain(user.trend.keys()).min().copied(),
            };
            from.map(|from| fill_trend(bucket, from, today, &user, &group))
                .unwrap_or_default()
        }
        None => Vec::new(),
    };
    Ok(Activity {
        user_name,
        group_name,
        period: duration.words().to_string(),
        zone: crate::funcs::pkg::timezone::zone_name(tz),
        user_hours: user.hours,
        group_hours: group.hours,
        trend,
        bucket: bucket.unwrap_or(Bucket::Day),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn counts(trend: &[(&str, u32)]) -> Counts {
        Counts {
            hours: [0; 24],
            trend: trend.iter().map(|&(d, n)| (day(d), n)).collect(),
        }
    }

    fn row(h: i32, b: Option<&str>, n: i64, tz: Tz) -> Row {
        Row {
            slot: Slot {
                h,
                b: b.map(|d| {
                    let start = day(d).and_hms_opt(0, 0, 0).unwrap();
                    bson::DateTime::from_chrono(
                        start.and_local_timezone(tz).unwrap().with_timezone(&Utc),
                    )
                }),
            },
            n,
        }
    }

    #[test]
    fn buckets_start_on_monday_and_the_first_of_the_month() {
        // 2026-11-05 is a Thursday
        assert_eq!(Bucket::Week.start_of(day("2026-11-05")), day("2026-11-02"));
        assert_eq!(Bucket::Week.start_of(day("2026-11-02")), day("2026-11-02"));
        assert_eq!(Bucket::Month.start_of(day("2026-11-05")), day("2026-11-01"));
        assert_eq!(Bucket::Month.label(day("2026-11-01")), "2026-11");
        assert_eq!(Bucket::Day.label(day("2026-11-05")), "11-05");
    }

    #[test]
    fn trend_fills_quiet_buckets_with_zero() {
        let user = counts(&[("2026-11-03", 2), ("2026-11-05", 1)]);
        let group = counts(&[("2026-11-03", 5), ("2026-11-05", 4)]);
        let trend = fill_trend(
            Bucket::Day,
            day("2026-11-03"),
            day("2026-11-06"),
            &user,
            &group,
        );
        let rows: Vec<_> = trend
            .iter()
            .map(|p| (p.label.as_str(), p.user, p.group))
            .collect();
        assert_eq!(
            rows,
            [
                ("11-03", 2, 5),
                ("11-04", 0, 0),
                ("11-05", 1, 4),
                ("11-06", 0, 0)
            ]
        );

        let user = counts(&[("2026-10-01", 1), ("2026-12-01", 3)]);
        let months: Vec<_> = fill_trend(
            Bucket::Month,
            day("2026-10-20"),
            day("2026-12-02"),
            &user,
            &Counts::default(),
        )
        .into_iter()
        .map(|p| (p.label, p.user))
        .collect();
        assert_eq!(
            months,
            [
                ("2026-10".to_string(), 1),
                ("2026-11".to_string(), 0),
                ("2026-12".to_string(), 3)
            ]
        );
    }

    #[test]
    fn rows_fold_into_hours_and_local_buckets() {
        // 东京的 11-02 零点是 UTC 11-01 15:00，桶仍要记在 11-02
        let tz: Tz = "Asia/Tokyo".parse().unwrap();
        let rows = [
            row(3, Some("2026-11-02"), 7, tz),
            row(3, Some("2026-11-09"), 2, tz),
            row(22, Some("2026-11-02"), 1, tz),
            row(24, None, 1, tz),
            row(-1, None, 1, tz),
        ];
        let counts = Counts::from_rows(&rows, tz);
        assert_eq!(counts.hours[3], 9);
        assert_eq!(counts.hours[22], 1);
        assert_eq!(counts.hours.iter().sum::<u32>(), 10);
        assert_eq!(counts.trend.get(&day("2026-11-02")), Some(&8));
        assert_eq!(counts.trend.get(&day("2026-11-09")), Some(&2));
        assert_eq!(counts.trend.len(), 2);
    }
}
