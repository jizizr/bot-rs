use super::provider::{MusicPlatform, MusicProvider, MusicSearchItem, MusicTrack, get_json};
use crate::BotError;
use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;

pub static BILIBILI_PROVIDER: BilibiliProvider = BilibiliProvider;

pub struct BilibiliProvider;

#[async_trait]
impl MusicProvider for BilibiliProvider {
    async fn search(&self, keyword: &str, limit: usize) -> Result<Vec<MusicSearchItem>, BotError> {
        let url = format!(
            "https://api.bilibili.com/x/web-interface/search/type?search_type=video&keyword={}&page=1",
            urlencoding::encode(keyword)
        );
        let response: BilibiliSearchResponse = get_json(&url).await?;
        ensure_bilibili_ok(response.code, &response.message)?;
        Ok(response
            .data
            .map(|data| data.result)
            .unwrap_or_default()
            .into_iter()
            .take(limit)
            .filter_map(|item| {
                let bvid = item.bvid?;
                Some(MusicSearchItem {
                    platform: MusicPlatform::Bilibili,
                    id: bvid,
                    song: strip_html_tags(&item.title),
                    singer: item.author.unwrap_or_else(|| "未知 UP".to_string()),
                })
            })
            .collect())
    }

    async fn resolve(
        &self,
        keyword: &str,
        selected_id: Option<&str>,
    ) -> Result<MusicTrack, BotError> {
        let bvid = match selected_id {
            Some(id) => id.to_string(),
            None => parse_bilibili_id(keyword).unwrap_or_else(|| keyword.to_string()),
        };
        let view = get_video_view(&bvid).await?;
        let audio = get_video_audio(&view.bvid, view.cid).await?;
        let mut headers = HashMap::new();
        headers.insert(
            "Referer".to_string(),
            "https://www.bilibili.com/".to_string(),
        );
        headers.insert("User-Agent".to_string(), "Mozilla/5.0".to_string());
        Ok(MusicTrack {
            id: view.bvid.clone(),
            platform: MusicPlatform::Bilibili,
            song: view.title,
            singer: view
                .owner
                .map(|owner| owner.name)
                .unwrap_or_else(|| "未知 UP".to_string()),
            album: String::new(),
            cover: normalize_bilibili_image(&view.pic),
            link: format!("https://www.bilibili.com/video/{}", view.bvid),
            url: audio,
            headers,
            duration: view
                .duration
                .filter(|duration| *duration > 0)
                .map(|duration| duration as u32),
            bitrate: None,
            format: Some("m4a".to_string()),
        })
    }
}

#[derive(Deserialize)]
struct BilibiliSearchResponse {
    code: i64,
    #[serde(default)]
    message: String,
    data: Option<BilibiliSearchData>,
}

#[derive(Deserialize)]
struct BilibiliSearchData {
    #[serde(default)]
    result: Vec<BilibiliSearchItem>,
}

#[derive(Deserialize)]
struct BilibiliSearchItem {
    bvid: Option<String>,
    title: String,
    author: Option<String>,
}

#[derive(Deserialize)]
struct BilibiliViewResponse {
    code: i64,
    #[serde(default)]
    message: String,
    data: Option<BilibiliViewData>,
}

#[derive(Deserialize)]
struct BilibiliViewData {
    bvid: String,
    cid: i64,
    title: String,
    pic: String,
    duration: Option<i64>,
    owner: Option<BilibiliOwner>,
}

#[derive(Deserialize)]
struct BilibiliOwner {
    name: String,
}

#[derive(Deserialize)]
struct BilibiliPlayResponse {
    code: i64,
    #[serde(default)]
    message: String,
    data: Option<BilibiliPlayData>,
}

#[derive(Deserialize)]
struct BilibiliPlayData {
    dash: Option<BilibiliDash>,
}

#[derive(Deserialize)]
struct BilibiliDash {
    #[serde(default)]
    audio: Vec<BilibiliDashAudio>,
}

#[derive(Deserialize)]
struct BilibiliDashAudio {
    #[serde(rename = "baseUrl")]
    base_url: Option<String>,
    #[serde(default, rename = "backupUrl")]
    backup_url: Vec<String>,
    bandwidth: Option<i64>,
}

async fn get_video_view(bvid: &str) -> Result<BilibiliViewData, BotError> {
    let url = format!(
        "https://api.bilibili.com/x/web-interface/view?bvid={}",
        urlencoding::encode(bvid)
    );
    let response: BilibiliViewResponse = get_json(&url).await?;
    ensure_bilibili_ok(response.code, &response.message)?;
    response
        .data
        .ok_or_else(|| BotError::Custom("没有找到 Bilibili 视频信息".to_string()))
}

async fn get_video_audio(bvid: &str, cid: i64) -> Result<String, BotError> {
    let url = format!(
        "https://api.bilibili.com/x/player/playurl?bvid={}&cid={cid}&qn=16&fnval=16",
        urlencoding::encode(bvid)
    );
    let response: BilibiliPlayResponse = get_json(&url).await?;
    ensure_bilibili_ok(response.code, &response.message)?;
    response
        .data
        .and_then(|data| data.dash)
        .and_then(|dash| {
            dash.audio
                .into_iter()
                .max_by_key(|audio| audio.bandwidth.unwrap_or_default())
                .and_then(|audio| {
                    audio
                        .base_url
                        .or_else(|| audio.backup_url.into_iter().next())
                })
        })
        .ok_or_else(|| BotError::Custom("没有拿到 Bilibili 音频链接".to_string()))
}

fn ensure_bilibili_ok(code: i64, message: &str) -> Result<(), BotError> {
    if code == 0 {
        Ok(())
    } else {
        Err(BotError::Custom(format!(
            "Bilibili API 错误 {code}: {message}"
        )))
    }
}

fn parse_bilibili_id(text: &str) -> Option<String> {
    let text = text.trim();
    let re = Regex::new(r"(?i)(BV[a-zA-Z0-9]{10})").ok()?;
    re.captures(text)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_string())
}

fn strip_html_tags(text: &str) -> String {
    Regex::new(r"<[^>]+>")
        .map(|re| re.replace_all(text, "").into_owned())
        .unwrap_or_else(|_| text.to_string())
}

fn normalize_bilibili_image(url: &str) -> String {
    if url.starts_with("//") {
        format!("https:{url}")
    } else {
        url.to_string()
    }
}
