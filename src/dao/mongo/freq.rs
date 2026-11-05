use super::*;
use crate::analysis::freq::{Activity, Bucket, Point};
use chrono::{Datelike, Days, Duration as ChronoDuration, Months, NaiveDate, Utc};
use chrono_tz::Tz;
use futures::stream::TryStreamExt;
use serde::Deserialize;

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

const KEY_FORMAT: &str = "%Y-%m-%d";

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

#[derive(Deserialize)]
struct HourCount {
    #[serde(rename = "_id")]
    hour: i32,
    n: i64,
}

#[derive(Deserialize)]
struct BucketCount {
    /// The bucket's first day, `%Y-%m-%d` in Beijing time.
    #[serde(rename = "_id")]
    key: String,
    user: i64,
    group: i64,
}

#[derive(Deserialize)]
struct Facets {
    user_hours: Vec<HourCount>,
    group_hours: Vec<HourCount>,
    #[serde(default)]
    trend: Vec<BucketCount>,
}

fn hours(counts: &[HourCount]) -> [u32; 24] {
    let mut hours = [0; 24];
    for count in counts {
        if let Some(slot) = usize::try_from(count.hour)
            .ok()
            .and_then(|h| hours.get_mut(h))
        {
            *slot = count.n.max(0) as u32;
        }
    }
    hours
}

/// Every bucket from the one holding `from` to the one holding `to`, with
/// the counts found and zero for the rest.
fn fill_trend(
    bucket: Bucket,
    from: NaiveDate,
    to: NaiveDate,
    counts: &[BucketCount],
) -> Vec<Point> {
    let mut points = Vec::new();
    let mut start = bucket.start_of(from);
    while start <= to {
        let key = start.format(KEY_FORMAT).to_string();
        let found = counts.iter().find(|c| c.key == key);
        points.push(Point {
            label: bucket.label(start),
            user: found.map_or(0, |c| c.user.max(0) as u32),
            group: found.map_or(0, |c| c.group.max(0) as u32),
        });
        match bucket.next(start) {
            Some(next) => start = next,
            None => break,
        }
    }
    points
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
    let zone = tz.name();
    let now = Utc::now();
    let mut match_condition = doc! { "group_id": gid };
    if let Some(span) = duration.span() {
        match_condition.insert(
            "timestamp",
            doc! { "$gte": bson::DateTime::from_chrono(now - span) },
        );
    }
    let by_hour = doc! { "$hour": { "date": "$timestamp", "timezone": zone } };
    let mut facets = doc! {
        "user_hours": [
            { "$match": { "user_id": uid } },
            { "$group": { "_id": by_hour.clone(), "n": { "$sum": 1 } } }
        ],
        "group_hours": [
            { "$group": { "_id": by_hour, "n": { "$sum": 1 } } }
        ],
    };
    let bucket = duration.bucket();
    if let Some(bucket) = bucket {
        facets.insert(
            "trend",
            vec![doc! {
                "$group": {
                    "_id": {
                        "$dateToString": {
                            "format": KEY_FORMAT,
                            "timezone": zone,
                            "date": {
                                "$dateTrunc": {
                                    "date": "$timestamp",
                                    "unit": bucket.unit(),
                                    "timezone": zone,
                                    "startOfWeek": "monday"
                                }
                            }
                        }
                    },
                    "user": { "$sum": { "$cond": [{ "$eq": ["$user_id", uid] }, 1, 0] } },
                    "group": { "$sum": 1 }
                }
            }],
        );
    }
    let pipeline = vec![
        doc! { "$match": match_condition },
        doc! { "$facet": facets },
    ];
    let facets: Option<Facets> = botlog()
        .await
        .aggregate(pipeline)
        .await?
        .try_next()
        .await?
        .map(|doc| bson::deserialize_from_document(doc).map_err(mongodb::error::Error::from))
        .transpose()?;
    let facets = facets.unwrap_or(Facets {
        user_hours: Vec::new(),
        group_hours: Vec::new(),
        trend: Vec::new(),
    });

    let today = now.with_timezone(&tz).date_naive();
    let trend = match bucket {
        Some(bucket) => {
            let first_found = facets
                .trend
                .iter()
                .filter_map(|c| NaiveDate::parse_from_str(&c.key, KEY_FORMAT).ok())
                .min();
            let from = match duration.span() {
                Some(span) => Some((now - span).with_timezone(&tz).date_naive()),
                None => first_found,
            };
            from.map(|from| fill_trend(bucket, from, today, &facets.trend))
                .unwrap_or_default()
        }
        None => Vec::new(),
    };
    Ok(Activity {
        user_name,
        group_name,
        period: duration.words().to_string(),
        zone: crate::funcs::pkg::timezone::zone_name(tz),
        user_hours: hours(&facets.user_hours),
        group_hours: hours(&facets.group_hours),
        trend,
        bucket: bucket.unwrap_or(Bucket::Day),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, KEY_FORMAT).unwrap()
    }

    fn count(key: &str, user: i64, group: i64) -> BucketCount {
        BucketCount {
            key: key.into(),
            user,
            group,
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
        let counts = [count("2026-11-03", 2, 5), count("2026-11-05", 1, 4)];
        let trend = fill_trend(Bucket::Day, day("2026-11-03"), day("2026-11-06"), &counts);
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

        let counts = [count("2026-10-01", 1, 1), count("2026-12-01", 3, 9)];
        let months: Vec<_> =
            fill_trend(Bucket::Month, day("2026-10-20"), day("2026-12-02"), &counts)
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
    fn hours_ignore_out_of_range_values() {
        let counts = [
            HourCount { hour: 3, n: 7 },
            HourCount { hour: 24, n: 1 },
            HourCount { hour: -1, n: 1 },
        ];
        let hours = hours(&counts);
        assert_eq!(hours[3], 7);
        assert_eq!(hours.iter().sum::<u32>(), 7);
    }
}
