//! The `/count` picture: how one member's messages in a group are spread
//! over the period and over the day, next to the whole group's.

use std::sync::OnceLock;

use charts_rs::{MultiChart, svg_to_webp};
use serde_json::{Value, json};

use crate::BotError;

/// How the trend chart groups the days of the period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    Day,
    Week,
    Month,
}

impl Bucket {
    fn noun(self) -> &'static str {
        match self {
            Bucket::Day => "每天",
            Bucket::Week => "每周",
            Bucket::Month => "每月",
        }
    }
}

/// One period of the trend chart.
#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    /// What the x axis shows, e.g. `11-03` or `2026-10`.
    pub label: String,
    pub user: u32,
    pub group: u32,
}

/// A member's activity in a group over a period.
#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    pub user_name: String,
    pub group_name: String,
    /// The period in words, e.g. `近 30 天` or `全部记录`.
    pub period: String,
    /// Messages in each hour of the day (Beijing time), 0 to 23.
    pub user_hours: [u32; 24],
    pub group_hours: [u32; 24],
    /// The period split into days, weeks or months, oldest first; empty
    /// when the period is too short for a trend.
    pub trend: Vec<Point>,
    pub bucket: Bucket,
}

impl Activity {
    pub fn user_total(&self) -> u32 {
        self.user_hours.iter().sum()
    }

    pub fn group_total(&self) -> u32 {
        self.group_hours.iter().sum()
    }

    /// The member's share of the group's messages, in percent.
    fn share(&self) -> f32 {
        percent(self.user_total(), self.group_total())
    }
}

const WIDTH: u32 = 900;
const FONT: &str = "Noto Sans SC";
const ACCENT: &str = "#E8590C";
const GROUP: &str = "#B8CBE4";
const MUTED: &str = "#868E96";

fn percent(part: u32, whole: u32) -> f32 {
    if whole == 0 {
        0.0
    } else {
        part as f32 * 100.0 / whole as f32
    }
}

/// Registers the bundled Noto Sans SC with charts-rs, once: without it the
/// raster has no glyphs for Chinese names (the server has no CJK fonts).
fn register_fonts() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        for file in ["NotoSansSC-Regular.otf", "NotoSansSC-Medium.otf"] {
            let path = format!("./data/fonts/{file}");
            match std::fs::read(&path) {
                Ok(data) => {
                    if let Err(err) = charts_rs::add_fonts(&[&data]) {
                        log::error!("charts-rs could not load {path}: {err}");
                    }
                }
                Err(err) => log::error!("could not read {path}: {err}"),
            }
        }
    });
}

/// Shares of a day's messages per hour, in percent, rounded to 0.1.
fn hour_shares(hours: &[u32; 24]) -> Vec<f32> {
    let total: u32 = hours.iter().sum();
    hours
        .iter()
        .map(|&n| (percent(n, total) * 10.0).round() / 10.0)
        .collect()
}

fn peak_hour(hours: &[u32; 24]) -> Option<usize> {
    let (hour, &count) = hours.iter().enumerate().max_by_key(|&(_, n)| *n)?;
    (count > 0).then_some(hour)
}

/// The trend chart: the group's messages as bars on the left axis and the
/// member's as a line on the right one, with its average and its peak.
fn trend_chart(activity: &Activity) -> Value {
    let labels: Vec<&str> = activity.trend.iter().map(|p| p.label.as_str()).collect();
    let group: Vec<u32> = activity.trend.iter().map(|p| p.group).collect();
    let user: Vec<u32> = activity.trend.iter().map(|p| p.user).collect();
    json!({
        "type": "bar",
        "width": WIDTH,
        "height": 330,
        "title_text": format!("{} 在「{}」的发言", activity.user_name, activity.group_name),
        "title_align": "left",
        "title_font_size": 22,
        "title_margin": {"left": 8, "top": 4},
        "sub_title_text": format!(
            "{} · 共 {} 条 · 全群 {} 条 · 占 {:.1}%",
            activity.period,
            activity.user_total(),
            activity.group_total(),
            activity.share()
        ),
        "sub_title_align": "left",
        "sub_title_font_color": MUTED,
        "sub_title_margin": {"left": 8},
        "legend_align": "right",
        "legend_category": "round_rect",
        "legend_margin": {"top": 40, "right": 12},
        "x_axis_data": labels,
        "x_axis_font_size": 12,
        "x_axis_label_overflow": "thin",
        "series_colors": [GROUP, ACCENT],
        "series_stroke_width": 2.5,
        "series_symbol": null,
        // 同一根轴：个人的线贴着全群的柱，占比一眼可见
        "y_axis_configs": [
            {
                "axis_formatter": "{t}",
                "axis_font_size": 12,
                "axis_split_number": 4,
                "axis_title": format!("{}条数", activity.bucket.noun())
            }
        ],
        "series_list": [
            {"name": "全群", "data": group},
            {
                "name": activity.user_name.clone(),
                "data": user,
                "category": "line",
                "smooth": true,
                "mark_lines": [{"category": "average"}],
                "mark_points": [{"category": "max"}]
            }
        ]
    })
}

/// The day chart: what share of the member's and of the group's messages
/// fall in each hour, so the two rhythms compare however large each is.
fn hours_chart(activity: &Activity, with_title: bool) -> Value {
    let mut chart = json!({
        "type": "bar",
        "width": WIDTH,
        "height": 330,
        "legend_align": "right",
        "legend_category": "round_rect",
        "legend_margin": {"top": 6, "right": 12},
        "x_axis_data": (0..24).map(|h| format!("{h}")).collect::<Vec<_>>(),
        "x_axis_font_size": 12,
        "x_axis_title": "一天中的时段（北京时间，时）",
        "series_colors": [GROUP, ACCENT],
        "series_stroke_width": 2.5,
        "series_symbol": null,
        "series_fill_opacity": 0.15,
        "series_label_formatter": "{c}%",
        "y_axis_configs": [
            {
                "axis_formatter": "{c}%",
                "axis_font_size": 12,
                "axis_split_number": 4,
                "axis_title": "占全天的比例"
            }
        ],
        "series_list": [
            {"name": "全群", "data": hour_shares(&activity.group_hours)},
            {
                "name": activity.user_name.clone(),
                "data": hour_shares(&activity.user_hours),
                "category": "line",
                "smooth": true,
                "fill": true,
                "mark_points": [{"category": "max"}]
            }
        ]
    });
    if with_title {
        let extra = json!({
            "title_text": format!("{} 在「{}」的发言时段", activity.user_name, activity.group_name),
            "title_align": "left",
            "title_font_size": 22,
            "title_margin": {"left": 8, "top": 4},
            "sub_title_text": format!(
                "{} · 共 {} 条 · 全群 {} 条 · 占 {:.1}%",
                activity.period,
                activity.user_total(),
                activity.group_total(),
                activity.share()
            ),
            "sub_title_align": "left",
            "sub_title_font_color": MUTED,
            "sub_title_margin": {"left": 8},
            "legend_margin": {"top": 40, "right": 12},
            "height": 380,
        });
        for (key, value) in extra.as_object().unwrap() {
            chart[key] = value.clone();
        }
    }
    chart
}

/// The `/count` picture as WebP.
pub fn paint(activity: &Activity) -> Result<Vec<u8>, BotError> {
    register_fonts();
    let mut charts = Vec::new();
    let has_trend = activity.trend.len() >= 2;
    if has_trend {
        charts.push(trend_chart(activity));
    }
    charts.push(hours_chart(activity, !has_trend));
    for chart in &mut charts {
        chart["font_family"] = json!(FONT);
    }
    let spec = json!({
        "theme": "light",
        "background_color": "#FFFFFF",
        "margin": {"left": 12, "top": 12, "right": 12, "bottom": 8},
        "gap": 18,
        "texts": [{
            "text": "按北京时间统计",
            "right": 14,
            "bottom": 6,
            "font_size": 12,
            "font_color": MUTED,
            "reserve": true
        }],
        "child_charts": charts,
    });
    let svg = MultiChart::from_json(&spec.to_string())?.svg()?;
    svg_to_webp(&svg).map_err(|e| BotError::Custom(format!("failed to convert svg to webp: {e}")))
}

/// A few lines to send with the picture: when the member is most active,
/// against the group, and how the latest period compares.
pub fn summary(activity: &Activity) -> String {
    let mut lines = Vec::new();
    match (
        peak_hour(&activity.user_hours),
        peak_hour(&activity.group_hours),
    ) {
        (Some(user), Some(group)) => {
            let share = percent(activity.user_hours[user], activity.user_total());
            lines.push(format!(
                "最活跃在 {user}–{} 时（占自己的 {share:.0}%），全群高峰在 {group}–{} 时",
                user + 1,
                group + 1
            ));
        }
        (None, _) => lines.push(format!("{}没有发言记录", activity.period)),
        _ => {}
    }
    if let Some(best) = activity.trend.iter().max_by_key(|p| p.user)
        && best.user > 0
        && activity.trend.len() >= 2
    {
        lines.push(format!("发言最多的是 {}，共 {} 条", best.label, best.user));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn activity(user: [u32; 24], group: [u32; 24], trend: Vec<Point>) -> Activity {
        Activity {
            user_name: "小明".into(),
            group_name: "测试群".into(),
            period: "近 30 天".into(),
            user_hours: user,
            group_hours: group,
            trend,
            bucket: Bucket::Day,
        }
    }

    #[test]
    fn shares_add_up_to_the_whole_day() {
        let mut hours = [0; 24];
        hours[9] = 1;
        hours[21] = 3;
        let shares = hour_shares(&hours);
        assert_eq!(shares[9], 25.0);
        assert_eq!(shares[21], 75.0);
        assert_eq!(hour_shares(&[0; 24]), vec![0.0; 24]);
    }

    #[test]
    fn summary_names_the_peaks() {
        let mut user = [0; 24];
        user[22] = 6;
        user[9] = 4;
        let mut group = [1; 24];
        group[20] = 9;
        let trend = vec![
            Point {
                label: "11-01".into(),
                user: 3,
                group: 10,
            },
            Point {
                label: "11-02".into(),
                user: 7,
                group: 12,
            },
        ];
        assert_eq!(
            summary(&activity(user, group, trend)),
            "最活跃在 22–23 时（占自己的 60%），全群高峰在 20–21 时\n发言最多的是 11-02，共 7 条"
        );
    }

    #[test]
    fn paints_with_and_without_a_trend() {
        let mut user = [0; 24];
        user[22] = 6;
        let trend = vec![
            Point {
                label: "11-01".into(),
                user: 3,
                group: 10,
            },
            Point {
                label: "11-02".into(),
                user: 3,
                group: 12,
            },
        ];
        assert!(!paint(&activity(user, [2; 24], trend)).unwrap().is_empty());
        assert!(
            !paint(&activity(user, [2; 24], Vec::new()))
                .unwrap()
                .is_empty()
        );
    }
}
