use super::provider::{
    CLIENT, DownloadProgress, MusicLyrics, MusicPlatform, MusicProvider, MusicSearchItem,
    MusicTrack, first_non_empty,
};
use crate::{BotError, settings::SETTINGS};
use aes::cipher::{KeyIvInit, StreamCipher};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose};
use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use url::Url;

lazy_static! {
    static ref SODA_LYRIC_LINE_RE: Regex = Regex::new(r"^\[(\d+),(\d+)\](.*)$").unwrap();
    static ref SODA_LYRIC_WORD_RE: Regex = Regex::new(r"<[^>]+>").unwrap();
}

const SODA_AID: &str = "386088";
const SODA_CHANNEL: &str = "xiaomi_8478_64";
const SODA_SEARCH_USER_AGENT: &str = "com.luna.music/100198030 (Linux; U; Android 15; zh_CN_#Hans; ABR-AL80; Build/V417IR;tt-ok/3.12.13.19)";
const SODA_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";
const SODA_DOWNLOAD_SCHEME: &str = "soda-download://";
const MAX_SODA_AUDIO_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;
type Aes128Ctr = ctr::Ctr128BE<aes::Aes128>;

pub static SODA_PROVIDER: SodaProvider = SodaProvider;

pub struct SodaProvider;

#[async_trait]
impl MusicProvider for SodaProvider {
    async fn search(&self, keyword: &str, limit: usize) -> Result<Vec<MusicSearchItem>, BotError> {
        let url = soda_search_url(keyword);
        let response: SodaSearchResponse = get_soda_json(&url, soda_search_headers()).await?;
        let mut seen = HashSet::with_capacity(limit);
        Ok(response
            .result_groups
            .into_iter()
            .flat_map(|group| group.data)
            .filter_map(|item| soda_track_to_item(item.entity.track))
            .filter(|item| seen.insert(item.id.clone()))
            .take(limit)
            .collect())
    }

    async fn resolve(
        &self,
        keyword: &str,
        selected_id: Option<&str>,
    ) -> Result<MusicTrack, BotError> {
        resolve_soda_track(keyword, selected_id, "high").await
    }

    async fn lyrics(&self, track_id: &str) -> Result<Option<MusicLyrics>, BotError> {
        let id = parse_soda_track_id(track_id).unwrap_or_else(|| track_id.trim().to_string());
        if id.is_empty() {
            return Ok(None);
        }
        let plain = parse_soda_lyric(&get_soda_track_details(&id).await?.lyric);
        if plain.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(MusicLyrics {
            plain,
            translation: String::new(),
        }))
    }
}

pub(super) async fn resolve_with_quality(
    keyword: &str,
    selected_id: Option<&str>,
    quality: &str,
) -> Result<MusicTrack, BotError> {
    resolve_soda_track(keyword, selected_id, quality).await
}

async fn resolve_soda_track(
    keyword: &str,
    selected_id: Option<&str>,
    quality: &str,
) -> Result<MusicTrack, BotError> {
    let id = match selected_id {
        Some(id) => id.to_string(),
        None => parse_soda_track_id(keyword)
            .or_else(|| {
                keyword
                    .trim()
                    .chars()
                    .all(|c| c.is_ascii_digit())
                    .then(|| keyword.trim().to_string())
            })
            .unwrap_or_default(),
    };
    let id = if id.is_empty() {
        SODA_PROVIDER
            .search(keyword, 1)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| BotError::Custom("没有找到可下载的音乐".to_string()))?
            .id
    } else {
        id
    };

    let details = get_soda_track_details(&id).await?;
    if details
        .media_id
        .as_deref()
        .zip(details.preview_id.as_deref())
        .is_some_and(|(media_id, preview_id)| {
            !media_id.trim().is_empty() && media_id.trim() == preview_id.trim()
        })
    {
        return Err(BotError::Custom("汽水音乐只返回了试听音频".to_string()));
    }
    let track = details.track;
    let play_info_url = details
        .player_info_url
        .filter(|url| !url.trim().is_empty())
        .ok_or_else(|| BotError::Custom("汽水音乐没有返回播放信息链接".to_string()))?;
    let mut play_infos = get_soda_play_infos(&play_info_url).await?;
    let mut saw_incomplete_duration = false;
    play_infos.retain(|item| {
        if soda_play_info_is_shorter_than_track(track.duration.unwrap_or_default(), item.duration) {
            saw_incomplete_duration = true;
            false
        } else {
            true
        }
    });
    if play_infos.is_empty() && saw_incomplete_duration {
        return Err(BotError::Custom("汽水音乐只返回了较短的试听流".to_string()));
    }
    let play_info = select_soda_play_info(&play_infos, quality)
        .ok_or_else(|| BotError::Custom("汽水音乐没有匹配的音质".to_string()))?;
    let url = first_non_empty([
        play_info.main_play_url.as_deref(),
        play_info.backup_play_url.as_deref(),
    ])
    .ok_or_else(|| BotError::Custom("汽水音乐没有返回可下载直链".to_string()))?
    .to_string();
    let backup_url = play_info
        .backup_play_url
        .as_deref()
        .map(str::trim)
        .filter(|backup| !backup.is_empty() && *backup != url);
    let format = normalize_soda_format(play_info.format.as_deref());
    let bitrate_kbps = if play_info.bitrate > 0 {
        play_info.bitrate
    } else if play_info.duration > 0.0 && play_info.size > 0 {
        (play_info.size as f64 * 8.0 / play_info.duration / 1_000.0).round() as i64
    } else {
        0
    };
    let download_url = soda_internal_download_url(
        &url,
        backup_url,
        play_info.play_auth.as_deref().unwrap_or_default(),
        &format,
    );

    let song = track.name.clone();
    let singer = soda_artists(&track);
    let cover = soda_cover_url(&track.album.url_cover);
    let link = soda_track_url(&track.id);

    Ok(MusicTrack {
        id: track.id.clone(),
        platform: MusicPlatform::Soda,
        song,
        singer,
        album: track.album.name.clone(),
        cover,
        link,
        url: download_url,
        headers: HashMap::new(),
        max_chunk_size: None,
        duration: track
            .duration
            .filter(|duration| *duration > 0)
            .map(|duration| ((duration + 500) / 1000) as u32),
        bitrate: (bitrate_kbps > 0)
            .then_some(bitrate_kbps as u32)
            .map(|value| value.saturating_mul(1_000)),
        format: Some(format),
    })
}

fn soda_play_info_is_shorter_than_track(
    track_duration_ms: u64,
    play_duration_seconds: f64,
) -> bool {
    if track_duration_ms == 0 || play_duration_seconds <= 0.0 {
        return false;
    }
    let catalog_seconds = track_duration_ms as f64 / 1_000.0;
    let tolerance = (catalog_seconds * 0.05).min(3.0);
    catalog_seconds - play_duration_seconds > tolerance
}

#[derive(Deserialize)]
struct SodaSearchResponse {
    #[serde(default)]
    result_groups: Vec<SodaResultGroup>,
}

#[derive(Deserialize)]
struct SodaResultGroup {
    #[serde(default)]
    data: Vec<SodaSearchEntry>,
}

#[derive(Deserialize)]
struct SodaSearchEntry {
    entity: SodaSearchEntity,
}

#[derive(Deserialize)]
struct SodaSearchEntity {
    #[serde(default)]
    track: SodaTrack,
}

#[derive(Deserialize)]
struct SodaTrackResponse {
    track_info: Option<SodaTrack>,
    track: Option<SodaTrack>,
    track_player: Option<SodaTrackPlayer>,
    #[serde(default)]
    lyric: Option<SodaLyric>,
    #[serde(default)]
    seo_track: Option<SodaSeoTrack>,
}

#[derive(Default, Deserialize)]
struct SodaSeoTrack {
    #[serde(default)]
    track: Option<SodaTrack>,
    #[serde(default)]
    lyric: Option<SodaLyric>,
}

#[derive(Deserialize)]
struct SodaTrackPlayer {
    #[serde(default)]
    url_player_info: Option<String>,
    #[serde(default)]
    media_id: Option<String>,
}

#[derive(Default, Deserialize)]
struct SodaLyric {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct SodaPlayInfoResponse {
    #[serde(rename = "Result")]
    result: SodaPlayInfoResult,
}

#[derive(Deserialize)]
struct SodaPlayInfoResult {
    #[serde(rename = "Data")]
    data: SodaPlayInfoData,
}

#[derive(Deserialize)]
struct SodaPlayInfoData {
    #[serde(default, rename = "PlayInfoList")]
    play_info_list: Vec<SodaPlayInfo>,
}

#[derive(Clone, Default, Deserialize)]
struct SodaTrack {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    duration: Option<u64>,
    #[serde(default)]
    artists: Vec<SodaArtist>,
    #[serde(default)]
    album: SodaAlbum,
    #[serde(default)]
    preview: Option<SodaPreview>,
}

#[derive(Clone, Deserialize)]
struct SodaPreview {
    #[serde(default)]
    vid: Option<String>,
}

#[derive(Clone, Deserialize)]
struct SodaArtist {
    name: String,
}

#[derive(Clone, Default, Deserialize)]
struct SodaAlbum {
    #[serde(default)]
    name: String,
    #[serde(default)]
    url_cover: SodaCover,
}

#[derive(Clone, Default, Deserialize)]
struct SodaCover {
    #[serde(default)]
    urls: Vec<String>,
    #[serde(default)]
    uri: String,
    #[serde(default)]
    template_prefix: String,
}

#[derive(Deserialize)]
struct SodaPlayInfo {
    #[serde(rename = "MainPlayUrl")]
    main_play_url: Option<String>,
    #[serde(rename = "BackupPlayUrl")]
    backup_play_url: Option<String>,
    #[serde(rename = "PlayAuth")]
    play_auth: Option<String>,
    #[serde(rename = "Format")]
    format: Option<String>,
    #[serde(rename = "Quality")]
    quality: Option<String>,
    #[serde(default, rename = "Size")]
    size: i64,
    #[serde(default, rename = "Bitrate")]
    bitrate: i64,
    #[serde(default, rename = "Duration")]
    duration: f64,
}

struct SodaTrackDetails {
    track: SodaTrack,
    player_info_url: Option<String>,
    media_id: Option<String>,
    preview_id: Option<String>,
    lyric: String,
}

async fn get_soda_track_details(id: &str) -> Result<SodaTrackDetails, BotError> {
    let url = soda_track_detail_url(id);
    let response: SodaTrackResponse = get_soda_json(&url, soda_detail_headers()).await?;
    let seo_track = response.seo_track;
    let track = response
        .track_info
        .or(response.track)
        .or_else(|| seo_track.as_ref().and_then(|item| item.track.clone()))
        .ok_or_else(|| BotError::Custom("没有找到汽水音乐歌曲详情".to_string()))?;
    let player = response.track_player;
    let player_info_url = player
        .as_ref()
        .and_then(|player| player.url_player_info.clone());
    let preview_id = track
        .preview
        .as_ref()
        .and_then(|preview| preview.vid.clone());
    let lyric = response
        .lyric
        .and_then(|lyric| lyric.content)
        .filter(|lyric| !lyric.trim().is_empty())
        .or_else(|| {
            seo_track
                .as_ref()
                .and_then(|item| item.lyric.as_ref())
                .and_then(|lyric| lyric.content.clone())
        })
        .unwrap_or_default();
    Ok(SodaTrackDetails {
        track,
        player_info_url,
        media_id: player.as_ref().and_then(|player| player.media_id.clone()),
        preview_id,
        lyric,
    })
}

async fn get_soda_play_infos(url: &str) -> Result<Vec<SodaPlayInfo>, BotError> {
    let response: SodaPlayInfoResponse = get_soda_json(url, soda_play_info_headers()).await?;
    let mut list = response.result.data.play_info_list;
    if list.is_empty() {
        return Err(BotError::Custom("汽水音乐没有返回播放清单".to_string()));
    }
    list.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| b.bitrate.cmp(&a.bitrate)));
    for item in &mut list {
        if item.bitrate > 10_000 {
            item.bitrate /= 1_000;
        }
        item.quality = item
            .quality
            .take()
            .map(|quality| quality.trim().to_ascii_lowercase());
    }
    Ok(list)
}

async fn get_soda_json<T: DeserializeOwned>(
    url: &str,
    headers: HashMap<String, String>,
) -> Result<T, BotError> {
    let has_cookie = headers.contains_key("Cookie");
    let mut value = request_soda_json(url, &headers).await?;
    if has_cookie && soda_status_code(&value) == Some(1_000_006) {
        let mut anonymous_headers = headers;
        anonymous_headers.remove("Cookie");
        value = request_soda_json(url, &anonymous_headers).await?;
    }
    ensure_soda_response(&value)?;
    serde_json::from_value(value)
        .map_err(|err| BotError::Custom(format!("汽水音乐响应解析失败：{err}")))
}

fn soda_search_url(keyword: &str) -> String {
    let mut url = Url::parse("https://api.qishui.com/luna/search/track").unwrap();
    for (key, value) in soda_search_params(keyword) {
        url.query_pairs_mut().append_pair(key, &value);
    }
    url.to_string()
}

fn soda_track_detail_url(id: &str) -> String {
    let mut url = Url::parse("https://beta-luna.douyin.com/luna/h5/seo_track").unwrap();
    url.query_pairs_mut()
        .append_pair("track_id", id)
        .append_pair("device_platform", "web");
    url.to_string()
}

fn soda_search_params(keyword: &str) -> Vec<(&'static str, String)> {
    vec![
        ("device_platform", "android".to_string()),
        ("os", "android".to_string()),
        ("ssmix", "a".to_string()),
        ("cdid", "46556f98-1720-4248-83da-62b74b60b46a".to_string()),
        ("channel", SODA_CHANNEL.to_string()),
        ("aid", SODA_AID.to_string()),
        ("app_name", "luna".to_string()),
        ("version_code", "100198030".to_string()),
        ("version_name", "19.8.0".to_string()),
        ("manifest_version_code", "100198030".to_string()),
        ("update_version_code", "100198030".to_string()),
        ("resolution", "1080*1920".to_string()),
        ("dpi", "480".to_string()),
        ("device_type", "ABR-AL80".to_string()),
        ("device_brand", "HUAWEI".to_string()),
        ("language", "zh".to_string()),
        ("os_api", "35".to_string()),
        ("os_version", "15".to_string()),
        ("ac", "wifi".to_string()),
        ("device_model", "ABR-AL80".to_string()),
        ("save_power", "0".to_string()),
        ("font_size", "1.00".to_string()),
        ("luna_first_launch_apk_type", "normal_apk".to_string()),
        ("diversion_channel_name", "xiaomi_8478_64".to_string()),
        ("is_car_play", "0".to_string()),
        ("battery", "0.99".to_string()),
        ("network_speed", "10156".to_string()),
        ("hybrid_version_code", "100198030".to_string()),
        ("tz_name", "Asia/Shanghai".to_string()),
        ("tz_offset", "28800".to_string()),
        ("luna_register_time", "1784311292".to_string()),
        (
            "diversion_category_level_two",
            "Xiaomi%E5%95%86%E5%BA%97-%E8%87%AA%E7%84%B6".to_string(),
        ),
        ("package", "com.luna.music".to_string()),
        ("charge", "0".to_string()),
        ("luna_apk_type", "normal_apk".to_string()),
        ("output_device_type", "Phone".to_string()),
        ("volume", "1.00".to_string()),
        ("brightness", "0.08".to_string()),
        ("need_personal_recommend", "1".to_string()),
        ("is_teen_mode", "0".to_string()),
        ("sim_region", "cn".to_string()),
        (
            "diversion_category_level_one",
            "%E5%8E%82%E5%95%86%E5%95%86%E5%BA%97-%E8%87%AA%E7%84%B6".to_string(),
        ),
        ("android_device_type", "default".to_string()),
        ("iid", "2204957404569386".to_string()),
        ("device_id", "2204957404565290".to_string()),
        ("count", "20".to_string()),
        ("q", keyword.to_string()),
        ("cursor", "0".to_string()),
        (
            "_rticket",
            chrono::Utc::now().timestamp_millis().to_string(),
        ),
    ]
}

fn select_soda_play_info<'a>(
    play_infos: &'a [SodaPlayInfo],
    quality: &str,
) -> Option<&'a SodaPlayInfo> {
    let requested = normalize_quality(quality);
    play_infos.iter().max_by_key(|item| {
        let label = item
            .quality
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let mut rank = match label.as_str() {
            "lossless" => 6000,
            "hi_res" => 5000,
            "spatial" => 4500,
            "highest" => 4000,
            "high" => 3000,
            "higher" => 2000,
            "medium" => 1000,
            _ => item.bitrate as i32,
        };
        match requested {
            "lossless" if label == "lossless" => rank += 1_000_000,
            "hires" if label == "lossless" => rank += 1_000_000,
            "hires" if label == "hi_res" => rank += 900_000,
            "hires" if label == "spatial" => rank += 800_000,
            "high" if label == "highest" => rank += 1_000_000,
            "high" if label == "high" || label == "hi_res" || label == "spatial" => rank += 800_000,
            "standard" if label == "higher" => rank += 1_000_000,
            "standard" if label == "medium" => rank += 900_000,
            _ => {}
        }
        (rank, item.size, item.bitrate)
    })
}

fn normalize_quality(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "standard" | "std" | "128" => "standard",
        "lossless" | "flac" | "无损" => "lossless",
        "hires" | "hi-res" | "hi_res" | "高解析" => "hires",
        _ => "high",
    }
}

fn normalize_soda_format(format: Option<&str>) -> String {
    let format = format
        .map(str::trim)
        .filter(|format| !format.is_empty())
        .unwrap_or("m4a")
        .to_ascii_lowercase();
    if format == "mp4" {
        "m4a".to_string()
    } else {
        format
    }
}

fn soda_internal_download_url(
    url: &str,
    backup_url: Option<&str>,
    play_auth: &str,
    format: &str,
) -> String {
    let mut internal = Url::parse("soda-download://track").unwrap();
    {
        let mut query = internal.query_pairs_mut();
        query.append_pair("url", url);
        if let Some(backup_url) = backup_url {
            query.append_pair("backup", backup_url);
        }
        if !play_auth.trim().is_empty() {
            query.append_pair("auth", play_auth.trim());
        }
        query.append_pair("format", format);
    }
    internal.to_string()
}

fn soda_track_to_item(track: SodaTrack) -> Option<MusicSearchItem> {
    (!track.id.trim().is_empty()).then(|| MusicSearchItem {
        platform: MusicPlatform::Soda,
        id: track.id.clone(),
        song: track.name.clone(),
        singer: soda_artists(&track),
        cover: soda_cover_url(&track.album.url_cover),
    })
}

fn soda_artists(track: &SodaTrack) -> String {
    let artists = track
        .artists
        .iter()
        .map(|artist| artist.name.trim())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    if artists.is_empty() {
        "未知歌手".to_string()
    } else {
        artists.join(" / ")
    }
}

fn soda_cover_url(cover: &SodaCover) -> String {
    let mut base = cover
        .urls
        .first()
        .map(|url| url.trim().to_string())
        .unwrap_or_default();
    let uri = cover.uri.trim();
    if !cover.template_prefix.trim().is_empty() && !cover.uri.trim().is_empty() {
        return format!(
            "https://p3-luna.douyinpic.com/img/{}~{}-resize:960:960.png",
            cover.uri.trim(),
            cover.template_prefix.trim()
        );
    }
    if base.is_empty() {
        return String::new();
    }
    if !uri.is_empty() && !base.contains(uri) {
        base.push_str(uri);
    }
    if !base.contains('~') {
        base.push_str("~c5_375x375.jpg");
    }
    base
}

fn soda_track_url(id: &str) -> String {
    format!("https://music.douyin.com/qishui/share/track?track_id={id}")
}

fn parse_soda_track_id(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let url = Url::parse(trimmed).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    let known_host = [
        "qishui.com",
        "qishui.douyin.com",
        "music.douyin.com",
        "bubble.qishui.com",
        "douyin.com",
    ]
    .into_iter()
    .any(|suffix| host == suffix || host.ends_with(&format!(".{suffix}")));
    if !known_host {
        return None;
    }

    let path = url.path().trim_matches('/');
    for pattern in [r"^track/(\d+)$", r"^song/(\d+)$", r"^qishui/share/track$"] {
        let re = Regex::new(pattern).ok()?;
        if let Some(id) = re
            .captures(path)
            .and_then(|captures| captures.get(1))
            .map(|value| value.as_str().to_string())
        {
            return Some(id);
        }
    }

    let query_id = url
        .query_pairs()
        .find(|(key, _)| key == "track_id" || key == "trackId" || key == "id")
        .map(|(_, value)| value.to_string());
    if query_id.as_deref().is_some_and(is_soda_numeric_id) {
        return query_id;
    }

    parse_soda_fragment(url.fragment()?)
}

fn parse_soda_fragment(fragment: &str) -> Option<String> {
    let fragment = fragment.trim().trim_start_matches('#');
    let (path, query) = fragment.split_once('?')?;
    let path = path.trim_matches('/');
    if !path.contains("track") && !path.contains("song") {
        return None;
    }
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(key, _)| key == "track_id" || key == "trackId" || key == "id")
        .map(|(_, value)| value.to_string())
        .filter(|id| is_soda_numeric_id(id))
}

fn parse_soda_lyric(raw: &str) -> String {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let captures = SODA_LYRIC_LINE_RE.captures(line)?;
            let start_ms = captures.get(1)?.as_str().parse::<u64>().ok()?;
            let text = captures
                .get(3)
                .map(|value| {
                    SODA_LYRIC_WORD_RE
                        .replace_all(value.as_str(), "")
                        .to_string()
                })
                .unwrap_or_default();
            let minutes = start_ms / 60_000;
            let seconds = (start_ms % 60_000) / 1_000;
            let centis = (start_ms % 1_000) / 10;
            Some(format!("[{minutes:02}:{seconds:02}.{centis:02}]{text}"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_soda_numeric_id(value: &str) -> bool {
    !value.trim().is_empty() && value.len() >= 8 && value.chars().all(|c| c.is_ascii_digit())
}

fn soda_headers(
    user_agent: &str,
    content_type: Option<&str>,
    with_cookie: bool,
) -> HashMap<String, String> {
    let mut headers: HashMap<String, String> = [
        ("User-Agent".to_string(), user_agent.to_string()),
        (
            "Accept".to_string(),
            "application/json, text/plain, */*".to_string(),
        ),
    ]
    .into_iter()
    .collect();
    if let Some(content_type) = content_type {
        headers.insert("Content-Type".to_string(), content_type.to_string());
    }
    let cookie = SETTINGS.music.soda.cookie.trim();
    let cookie = if cookie.is_empty() {
        std::env::var("SODA_COOKIE").unwrap_or_default()
    } else {
        cookie.to_string()
    };
    if with_cookie && !cookie.trim().is_empty() {
        headers.insert("Cookie".to_string(), cookie.trim().to_string());
    }
    headers
}

fn soda_search_headers() -> HashMap<String, String> {
    soda_headers(
        SODA_SEARCH_USER_AGENT,
        Some("application/json; charset=UTF-8"),
        true,
    )
}

fn soda_detail_headers() -> HashMap<String, String> {
    soda_headers(SODA_USER_AGENT, None, true)
}

fn soda_play_info_headers() -> HashMap<String, String> {
    soda_headers(SODA_USER_AGENT, None, false)
}

async fn request_soda_json(
    url: &str,
    headers: &HashMap<String, String>,
) -> Result<Value, BotError> {
    let mut request = CLIENT.get(url);
    for (key, value) in headers {
        request = request.header(key, value);
    }
    let response = request
        .send()
        .await
        .map_err(|_| BotError::Custom("汽水音乐接口请求失败".to_string()))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|err| BotError::Request(err.without_url()))?;
    if !status.is_success() {
        return Err(BotError::Custom(format!(
            "汽水音乐接口请求失败：HTTP {status}"
        )));
    }
    serde_json::from_str(&body)
        .map_err(|err| BotError::Custom(format!("汽水音乐响应解析失败：{err}")))
}

fn soda_status_code(value: &Value) -> Option<i64> {
    value.get("status_code").and_then(Value::as_i64)
}

fn ensure_soda_response(value: &Value) -> Result<(), BotError> {
    if let Some(code) = soda_status_code(value)
        && code != 0
    {
        return Err(BotError::Custom(format!(
            "汽水音乐接口错误：status_code={code}"
        )));
    }
    if let Some(code) = value
        .get("status_info")
        .and_then(|info| info.get("code"))
        .and_then(Value::as_i64)
        && code != 0
    {
        return Err(BotError::Custom(format!(
            "汽水音乐接口错误：status_info.code={code}"
        )));
    }
    Ok(())
}

pub(super) async fn download_internal_url_with_progress<F>(
    url: &str,
    progress: &mut F,
) -> Result<Vec<u8>, BotError>
where
    F: FnMut(DownloadProgress) + Send,
{
    let payload = url
        .strip_prefix(SODA_DOWNLOAD_SCHEME)
        .ok_or_else(|| BotError::Custom("Unknown Soda internal URL".to_string()))?;
    let parsed = Url::parse(&format!("{SODA_DOWNLOAD_SCHEME}{payload}"))
        .map_err(|e| BotError::Custom(format!("汽水音乐内部下载 URL 无效：{e}")))?;
    let raw_url = parsed
        .query_pairs()
        .find(|(key, _)| key == "url")
        .map(|(_, value)| value.into_owned())
        .ok_or_else(|| BotError::Custom("汽水音乐内部下载 URL 缺少媒体地址".to_string()))?;
    let backup_url = parsed
        .query_pairs()
        .find(|(key, _)| key == "backup")
        .map(|(_, value)| value.into_owned());
    let play_auth = parsed
        .query_pairs()
        .find(|(key, _)| key == "auth")
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default();
    let format = parsed
        .query_pairs()
        .find(|(key, _)| key == "format")
        .map(|(_, value)| value.into_owned())
        .unwrap_or_else(|| "m4a".to_string());

    let mut last_error = None;
    let mut urls = vec![raw_url];
    if let Some(backup_url) = backup_url.filter(|url| !url.trim().is_empty()) {
        urls.push(backup_url);
    }
    for candidate in urls {
        match download_and_decrypt_soda_once(&candidate, &play_auth, &format, progress).await {
            Ok(data) => return Ok(data),
            Err(err) => last_error = Some(err),
        }
    }
    Err(last_error.unwrap_or_else(|| BotError::Custom("汽水音乐下载失败".to_string())))
}

async fn download_and_decrypt_soda_once<F>(
    url: &str,
    play_auth: &str,
    format: &str,
    progress: &mut F,
) -> Result<Vec<u8>, BotError>
where
    F: FnMut(DownloadProgress) + Send,
{
    let encrypted = download_soda_media(url, progress).await?;
    let mut data = if play_auth.trim().is_empty() {
        encrypted
    } else {
        // None means the file carries no senc box (never encrypted): deliver it as-is.
        // A real decryption failure must propagate instead of shipping unplayable bytes.
        match decrypt_soda_audio(&encrypted, play_auth)? {
            Some(decrypted) => decrypted,
            None => encrypted,
        }
    };
    if matches!(format.trim().to_ascii_lowercase().as_str(), "m4a" | "mp4") {
        rewrite_soda_audio_sample_entries(&mut data)?;
    }
    progress(DownloadProgress {
        written: data.len() as u64,
        total: Some(data.len() as u64),
    });
    Ok(data)
}

async fn download_soda_media<F>(url: &str, progress: &mut F) -> Result<Vec<u8>, BotError>
where
    F: FnMut(DownloadProgress) + Send,
{
    let mut response = CLIENT
        .get(url)
        .header("User-Agent", SODA_USER_AGENT)
        .send()
        .await
        .map_err(|_| BotError::Custom("汽水音乐媒体请求失败".to_string()))?;
    if response.status() == reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(BotError::Custom(
            "汽水音乐下载失败：未请求分段却收到 206 Partial Content".to_string(),
        ));
    }
    if !response.status().is_success() {
        return Err(BotError::Custom(format!(
            "汽水音乐下载失败：HTTP {}",
            response.status()
        )));
    }
    let total = response.content_length();
    if total.is_some_and(|total| total > MAX_SODA_AUDIO_DOWNLOAD_BYTES) {
        return Err(BotError::Custom(
            "汽水音乐下载失败：文件超过 2 GiB 上限".to_string(),
        ));
    }
    let mut buf = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| BotError::Request(err.without_url()))?
    {
        buf.extend_from_slice(&chunk);
        if buf.len() as u64 > MAX_SODA_AUDIO_DOWNLOAD_BYTES {
            return Err(BotError::Custom(
                "汽水音乐下载失败：文件超过 2 GiB 上限".to_string(),
            ));
        }
        progress(DownloadProgress {
            written: buf.len() as u64,
            total,
        });
    }
    if buf.is_empty() {
        return Err(BotError::Custom("汽水音乐下载失败：文件为空".to_string()));
    }
    Ok(buf)
}

/// Returns `Ok(None)` when the file is not an encrypted MP4 (no moov/audio trak/senc box),
/// meaning the original bytes are playable as-is; any error after encryption is established
/// is a real decryption failure and must not be swallowed.
fn decrypt_soda_audio(file_data: &[u8], play_auth: &str) -> Result<Option<Vec<u8>>, BotError> {
    let Ok(moov) = find_soda_box(file_data, b"moov", 0, file_data.len()) else {
        return Ok(None);
    };
    let Ok((trak, stbl)) = find_soda_audio_track(file_data, moov) else {
        return Ok(None);
    };
    let Ok(senc) = find_soda_box(file_data, b"senc", trak.payload_start(), trak.end())
        .or_else(|_| find_soda_box(file_data, b"senc", stbl.payload_start(), stbl.end()))
        .or_else(|_| find_soda_box(file_data, b"senc", moov.payload_start(), moov.end()))
    else {
        return Ok(None);
    };
    let hex_key = extract_soda_key(play_auth)?;
    let key = hex_decode(&hex_key)?;
    if key.len() != 16 {
        return Err(BotError::Custom(
            "汽水音乐 PlayAuth key 长度错误".to_string(),
        ));
    }
    let sample_ranges = parse_soda_sample_ranges(file_data, stbl)?;
    let samples = parse_soda_senc(&file_data[senc.payload_start()..senc.end()])?;
    let mut decrypted = file_data.to_vec();
    for (index, range) in sample_ranges.iter().enumerate() {
        if index >= samples.len() {
            break;
        }
        let end = range
            .offset
            .checked_add(range.size)
            .filter(|end| *end <= decrypted.len())
            .ok_or_else(|| BotError::Custom("汽水音乐 sample 超出文件范围".to_string()))?;
        decrypt_soda_sample(&mut decrypted[range.offset..end], &key, &samples[index])?;
    }
    rewrite_soda_audio_sample_entries(&mut decrypted)?;
    Ok(Some(decrypted))
}

fn extract_soda_key(play_auth: &str) -> Result<String, BotError> {
    let data = general_purpose::STANDARD
        .decode(play_auth.trim().as_bytes())
        .map_err(|e| BotError::Custom(format!("汽水音乐 PlayAuth 解码失败：{e}")))?;
    if data.len() < 3 {
        return Err(BotError::Custom("汽水音乐 PlayAuth 数据过短".to_string()));
    }
    let padding_len = (data[0] ^ data[1] ^ data[2]).saturating_sub(48) as usize;
    if data.len() < padding_len + 2 {
        return Err(BotError::Custom(
            "汽水音乐 PlayAuth padding 无效".to_string(),
        ));
    }
    let inner = &data[1..data.len() - padding_len];
    let decoded = decrypt_soda_inner(inner);
    if decoded.is_empty() {
        return Err(BotError::Custom(
            "汽水音乐 PlayAuth 内层解密失败".to_string(),
        ));
    }
    let skip = decode_soda_base36(decoded[0]);
    let end = 1 + (data.len() - padding_len - 2).saturating_sub(skip);
    if end > decoded.len() || end < 1 {
        return Err(BotError::Custom(
            "汽水音乐 PlayAuth key 范围无效".to_string(),
        ));
    }
    String::from_utf8(decoded[1..end].to_vec())
        .map_err(|e| BotError::Custom(format!("汽水音乐 PlayAuth key 非 UTF-8：{e}")))
}

fn decrypt_soda_inner(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut buff = Vec::with_capacity(bytes.len() + 2);
    buff.extend_from_slice(&[0xFA, 0x55]);
    buff.extend_from_slice(bytes);
    for (i, byte) in bytes.iter().enumerate() {
        let mut value = (*byte ^ buff[i]) as i32 - bitcount_soda(i) as i32 - 21;
        while value < 0 {
            value += 255;
        }
        out.push(value as u8);
    }
    out
}

fn bitcount_soda(n: usize) -> u32 {
    (n as u32).count_ones()
}

fn decode_soda_base36(byte: u8) -> usize {
    match byte {
        b'0'..=b'9' => (byte - b'0') as usize,
        b'a'..=b'z' => (byte - b'a') as usize + 10,
        _ => 0xff,
    }
}

fn hex_decode(value: &str) -> Result<Vec<u8>, BotError> {
    let value = value.trim();
    if !value.len().is_multiple_of(2) {
        return Err(BotError::Custom("hex 长度错误".to_string()));
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16)
                .map_err(|e| BotError::Custom(format!("hex 解码失败：{e}")))
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
struct SodaMp4Box {
    start: usize,
    header_size: usize,
    size: usize,
    typ: [u8; 4],
}

impl SodaMp4Box {
    fn payload_start(self) -> usize {
        self.start + self.header_size
    }

    fn end(self) -> usize {
        self.start + self.size
    }
}

#[derive(Clone, Copy, Debug)]
struct SodaSampleRange {
    offset: usize,
    size: usize,
}

#[derive(Clone, Debug)]
struct SodaSencSample {
    iv: Vec<u8>,
    subsamples: Vec<SodaSencSubsample>,
}

#[derive(Clone, Copy, Debug)]
struct SodaSencSubsample {
    clear_bytes: usize,
    encrypted_bytes: usize,
}

fn read_soda_box(data: &[u8], start: usize, end: usize) -> Result<Option<SodaMp4Box>, BotError> {
    if start >= end || start >= data.len() {
        return Ok(None);
    }
    if start + 8 > end || start + 8 > data.len() {
        return Err(BotError::Custom(
            "汽水音乐 MP4 box header truncated".to_string(),
        ));
    }
    let mut size = u32::from_be_bytes([
        data[start],
        data[start + 1],
        data[start + 2],
        data[start + 3],
    ]) as usize;
    let typ = [
        data[start + 4],
        data[start + 5],
        data[start + 6],
        data[start + 7],
    ];
    let mut header_size = 8usize;
    if size == 1 {
        if start + 16 > end || start + 16 > data.len() {
            return Err(BotError::Custom(
                "汽水音乐 MP4 largesize truncated".to_string(),
            ));
        }
        size = u64::from_be_bytes([
            data[start + 8],
            data[start + 9],
            data[start + 10],
            data[start + 11],
            data[start + 12],
            data[start + 13],
            data[start + 14],
            data[start + 15],
        ]) as usize;
        header_size = 16;
    } else if size == 0 {
        size = end - start;
    }
    // checked_add: size comes from untrusted (possibly 64-bit largesize) bytes and must not
    // wrap in release builds where overflow-checks are off.
    let box_end = start.checked_add(size);
    if size < header_size || box_end.is_none_or(|box_end| box_end > end || box_end > data.len()) {
        return Err(BotError::Custom(format!(
            "汽水音乐 MP4 box {} size invalid",
            String::from_utf8_lossy(&typ)
        )));
    }
    Ok(Some(SodaMp4Box {
        start,
        header_size,
        size,
        typ,
    }))
}

fn find_soda_box(
    data: &[u8],
    typ: &[u8; 4],
    start: usize,
    end: usize,
) -> Result<SodaMp4Box, BotError> {
    let mut pos = start;
    while pos + 8 <= end {
        let Some(box_info) = read_soda_box(data, pos, end)? else {
            break;
        };
        if &box_info.typ == typ {
            return Ok(box_info);
        }
        pos = box_info.end();
    }
    Err(BotError::Custom(format!(
        "汽水音乐 MP4 缺少 {} box",
        String::from_utf8_lossy(typ)
    )))
}

fn find_soda_audio_track(
    data: &[u8],
    moov: SodaMp4Box,
) -> Result<(SodaMp4Box, SodaMp4Box), BotError> {
    let mut first_trak = None;
    let mut pos = moov.payload_start();
    while pos + 8 <= moov.end() {
        let Some(box_info) = read_soda_box(data, pos, moov.end())? else {
            break;
        };
        if &box_info.typ == b"trak" {
            first_trak.get_or_insert(box_info);
            if is_soda_audio_track(data, box_info)
                && let Ok(stbl) = find_soda_track_stbl(data, box_info)
            {
                return Ok((box_info, stbl));
            }
        }
        pos = box_info.end();
    }
    let trak = first_trak.ok_or_else(|| BotError::Custom("汽水音乐 MP4 缺少 trak".to_string()))?;
    let stbl = find_soda_track_stbl(data, trak)?;
    Ok((trak, stbl))
}

fn find_soda_track_stbl(data: &[u8], trak: SodaMp4Box) -> Result<SodaMp4Box, BotError> {
    let mdia = find_soda_box(data, b"mdia", trak.payload_start(), trak.end())?;
    let minf = find_soda_box(data, b"minf", mdia.payload_start(), mdia.end())?;
    find_soda_box(data, b"stbl", minf.payload_start(), minf.end())
}

fn is_soda_audio_track(data: &[u8], trak: SodaMp4Box) -> bool {
    if let Ok(mdia) = find_soda_box(data, b"mdia", trak.payload_start(), trak.end())
        && let Ok(hdlr) = find_soda_box(data, b"hdlr", mdia.payload_start(), mdia.end())
    {
        let body = &data[hdlr.payload_start()..hdlr.end()];
        if body.len() >= 12 && &body[8..12] == b"soun" {
            return true;
        }
    }
    let Ok(stbl) = find_soda_track_stbl(data, trak) else {
        return false;
    };
    let Ok(stsd) = find_soda_box(data, b"stsd", stbl.payload_start(), stbl.end()) else {
        return false;
    };
    let body = &data[stsd.payload_start()..stsd.end()];
    body.windows(4)
        .any(|window| window == b"enca" || window == b"mp4a")
}

fn parse_soda_sample_ranges(
    data: &[u8],
    stbl: SodaMp4Box,
) -> Result<Vec<SodaSampleRange>, BotError> {
    let stsz = find_soda_box(data, b"stsz", stbl.payload_start(), stbl.end())?;
    let sample_sizes = parse_soda_stsz(&data[stsz.payload_start()..stsz.end()]);
    if sample_sizes.is_empty() {
        return Err(BotError::Custom("汽水音乐 MP4 stsz 为空".to_string()));
    }
    let stsc = find_soda_box(data, b"stsc", stbl.payload_start(), stbl.end())?;
    let chunk_map = parse_soda_stsc(&data[stsc.payload_start()..stsc.end()]);
    if chunk_map.is_empty() {
        return Err(BotError::Custom("汽水音乐 MP4 stsc 为空".to_string()));
    }
    let chunk_offsets = find_soda_box(data, b"stco", stbl.payload_start(), stbl.end())
        .map(|stco| parse_soda_stco(&data[stco.payload_start()..stco.end()]))
        .or_else(|_| {
            find_soda_box(data, b"co64", stbl.payload_start(), stbl.end())
                .map(|co64| parse_soda_co64(&data[co64.payload_start()..co64.end()]))
        })?;
    if chunk_offsets.is_empty() {
        return Err(BotError::Custom(
            "汽水音乐 MP4 chunk offset 为空".to_string(),
        ));
    }
    let mut ranges = Vec::with_capacity(sample_sizes.len());
    let mut sample_index = 0usize;
    for (map_index, entry) in chunk_map.iter().enumerate() {
        if entry.first_chunk == 0 || entry.samples_per_chunk == 0 {
            continue;
        }
        let chunk_start = entry.first_chunk as usize - 1;
        let mut chunk_end = chunk_offsets.len();
        if let Some(next) = chunk_map.get(map_index + 1)
            && next.first_chunk > 0
        {
            chunk_end = next.first_chunk as usize - 1;
        }
        for chunk_offset in chunk_offsets
            .iter()
            .take(chunk_end.min(chunk_offsets.len()))
            .skip(chunk_start)
        {
            let mut offset = *chunk_offset as usize;
            for _ in 0..entry.samples_per_chunk {
                if sample_index >= sample_sizes.len() {
                    break;
                }
                let size = sample_sizes[sample_index] as usize;
                let end = offset
                    .checked_add(size)
                    .filter(|end| *end <= data.len())
                    .ok_or_else(|| BotError::Custom("汽水音乐 MP4 sample 越界".to_string()))?;
                ranges.push(SodaSampleRange { offset, size });
                offset = end;
                sample_index += 1;
            }
        }
    }
    if sample_index != sample_sizes.len() {
        return Err(BotError::Custom(
            "汽水音乐 MP4 sample layout 不完整".to_string(),
        ));
    }
    Ok(ranges)
}

#[derive(Clone, Copy, Debug)]
struct SodaChunkMapEntry {
    first_chunk: u32,
    samples_per_chunk: u32,
}

fn parse_soda_stsz(data: &[u8]) -> Vec<u32> {
    if data.len() < 12 {
        return Vec::new();
    }
    let fixed_size = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let count = u32::from_be_bytes([data[8], data[9], data[10], data[11]]) as usize;
    if fixed_size != 0 {
        return vec![fixed_size; count];
    }
    (0..count)
        .filter_map(|index| {
            let off = 12 + index * 4;
            (off + 4 <= data.len()).then(|| {
                u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
            })
        })
        .collect()
}

fn parse_soda_stsc(data: &[u8]) -> Vec<SodaChunkMapEntry> {
    if data.len() < 8 {
        return Vec::new();
    }
    let count = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
    (0..count)
        .filter_map(|index| {
            let off = 8 + index * 12;
            (off + 12 <= data.len()).then(|| SodaChunkMapEntry {
                first_chunk: u32::from_be_bytes([
                    data[off],
                    data[off + 1],
                    data[off + 2],
                    data[off + 3],
                ]),
                samples_per_chunk: u32::from_be_bytes([
                    data[off + 4],
                    data[off + 5],
                    data[off + 6],
                    data[off + 7],
                ]),
            })
        })
        .collect()
}

fn parse_soda_stco(data: &[u8]) -> Vec<u64> {
    if data.len() < 8 {
        return Vec::new();
    }
    let count = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
    (0..count)
        .filter_map(|index| {
            let off = 8 + index * 4;
            (off + 4 <= data.len()).then(|| {
                u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]) as u64
            })
        })
        .collect()
}

fn parse_soda_co64(data: &[u8]) -> Vec<u64> {
    if data.len() < 8 {
        return Vec::new();
    }
    let count = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
    (0..count)
        .filter_map(|index| {
            let off = 8 + index * 8;
            (off + 8 <= data.len()).then(|| {
                u64::from_be_bytes([
                    data[off],
                    data[off + 1],
                    data[off + 2],
                    data[off + 3],
                    data[off + 4],
                    data[off + 5],
                    data[off + 6],
                    data[off + 7],
                ])
            })
        })
        .collect()
}

fn parse_soda_senc(data: &[u8]) -> Result<Vec<SodaSencSample>, BotError> {
    if data.len() < 8 {
        return Err(BotError::Custom("汽水音乐 MP4 senc 过短".to_string()));
    }
    let flags = u32::from_be_bytes([0, data[1], data[2], data[3]]);
    let sample_count = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
    let has_subsamples = flags & 0x02 != 0;
    let iv_size = infer_soda_senc_iv_size(&data[8..], sample_count, has_subsamples)
        .ok_or_else(|| BotError::Custom("汽水音乐 MP4 senc IV size 无法识别".to_string()))?;
    let mut samples = Vec::with_capacity(sample_count);
    let mut pos = 8usize;
    for _ in 0..sample_count {
        if pos + iv_size > data.len() {
            break;
        }
        let iv = data[pos..pos + iv_size].to_vec();
        pos += iv_size;
        let mut subsamples = Vec::new();
        if has_subsamples {
            if pos + 2 > data.len() {
                break;
            }
            let sub_count = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
            pos += 2;
            for _ in 0..sub_count {
                if pos + 6 > data.len() {
                    break;
                }
                subsamples.push(SodaSencSubsample {
                    clear_bytes: u16::from_be_bytes([data[pos], data[pos + 1]]) as usize,
                    encrypted_bytes: u32::from_be_bytes([
                        data[pos + 2],
                        data[pos + 3],
                        data[pos + 4],
                        data[pos + 5],
                    ]) as usize,
                });
                pos += 6;
            }
        }
        samples.push(SodaSencSample { iv, subsamples });
    }
    Ok(samples)
}

fn infer_soda_senc_iv_size(
    data: &[u8],
    sample_count: usize,
    has_subsamples: bool,
) -> Option<usize> {
    [8usize, 16]
        .into_iter()
        .find(|candidate| {
            soda_senc_consumed_bytes(data, sample_count, *candidate, has_subsamples)
                .is_some_and(|consumed| consumed == data.len())
        })
        .or_else(|| {
            [8usize, 16].into_iter().find(|candidate| {
                soda_senc_consumed_bytes(data, sample_count, *candidate, has_subsamples).is_some()
            })
        })
}

fn soda_senc_consumed_bytes(
    data: &[u8],
    sample_count: usize,
    iv_size: usize,
    has_subsamples: bool,
) -> Option<usize> {
    let mut pos = 0usize;
    for _ in 0..sample_count {
        if pos + iv_size > data.len() {
            return None;
        }
        pos += iv_size;
        if has_subsamples {
            if pos + 2 > data.len() {
                return None;
            }
            let sub_count = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
            pos += 2;
            if pos + sub_count * 6 > data.len() {
                return None;
            }
            pos += sub_count * 6;
        }
    }
    Some(pos)
}

fn decrypt_soda_sample(
    sample: &mut [u8],
    key: &[u8],
    senc: &SodaSencSample,
) -> Result<(), BotError> {
    let mut iv = [0u8; 16];
    match senc.iv.len() {
        8 => iv[..8].copy_from_slice(&senc.iv),
        16 => iv.copy_from_slice(&senc.iv),
        len => {
            return Err(BotError::Custom(format!(
                "汽水音乐 sample IV 长度错误：{len}"
            )));
        }
    }
    let mut cipher = Aes128Ctr::new_from_slices(key, &iv)
        .map_err(|e| BotError::Custom(format!("汽水音乐 AES-CTR 初始化失败：{e}")))?;
    if senc.subsamples.is_empty() {
        cipher.apply_keystream(sample);
        return Ok(());
    }
    let mut pos = 0usize;
    for subsample in &senc.subsamples {
        pos += subsample.clear_bytes;
        let end = pos + subsample.encrypted_bytes;
        if end > sample.len() {
            return Err(BotError::Custom(
                "汽水音乐 subsample encrypted range 越界".to_string(),
            ));
        }
        cipher.apply_keystream(&mut sample[pos..end]);
        pos = end;
    }
    Ok(())
}

fn rewrite_soda_audio_sample_entries(data: &mut [u8]) -> Result<(), BotError> {
    let Ok(moov) = find_soda_box(data, b"moov", 0, data.len()) else {
        return Ok(());
    };
    let Ok((_, stbl)) = find_soda_audio_track(data, moov) else {
        return Ok(());
    };
    let Ok(stsd) = find_soda_box(data, b"stsd", stbl.payload_start(), stbl.end()) else {
        return Ok(());
    };
    if stsd.payload_start() + 8 > stsd.end() {
        return Ok(());
    }
    let mut pos = stsd.payload_start() + 8;
    while pos + 8 <= stsd.end() {
        let Some(entry) = read_soda_box(data, pos, stsd.end())? else {
            break;
        };
        if &entry.typ == b"enca" {
            let mut replacement = *b"mp4a";
            if let Ok(frma) = find_soda_box(data, b"frma", entry.payload_start(), entry.end())
                && frma.payload_start() + 4 <= frma.end()
            {
                replacement.copy_from_slice(&data[frma.payload_start()..frma.payload_start() + 4]);
            }
            data[entry.start + 4..entry.start + 8].copy_from_slice(&replacement);
        }
        pos = entry.end();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{super::provider::download_track_media, *};

    #[test]
    fn parses_soda_share_url() {
        assert_eq!(
            parse_soda_track_id(
                "https://music.douyin.com/qishui/share/track?track_id=739105056071"
            ),
            Some("739105056071".to_string())
        );
    }

    #[test]
    fn builds_soda_search_url_like_pc_web_client() {
        let url = soda_search_url("晴天");
        assert!(url.contains("api.qishui.com/luna/search/track"));
        assert!(url.contains("aid=386088"));
        assert!(url.contains("channel=xiaomi_8478_64"));
        assert!(url.contains("device_platform=android"));
    }

    #[test]
    fn selects_soda_high_quality_like_source_bot() {
        let play_infos = vec![
            SodaPlayInfo {
                main_play_url: Some("https://example.com/higher.m4a".to_string()),
                backup_play_url: None,
                play_auth: None,
                format: Some("m4a".to_string()),
                quality: Some("higher".to_string()),
                size: 1,
                bitrate: 128,
                duration: 0.0,
            },
            SodaPlayInfo {
                main_play_url: Some("https://example.com/highest.m4a".to_string()),
                backup_play_url: None,
                play_auth: None,
                format: Some("mp4".to_string()),
                quality: Some("highest".to_string()),
                size: 2,
                bitrate: 320,
                duration: 0.0,
            },
        ];

        let selected = select_soda_play_info(&play_infos, "high").unwrap();
        assert_eq!(selected.quality.as_deref(), Some("highest"));
        assert_eq!(normalize_soda_format(selected.format.as_deref()), "m4a");
    }

    #[test]
    fn parses_soda_lyric_to_lrc() {
        let raw = "[1234,1000]<0,500,0>你<500,500,0>好";
        assert_eq!(parse_soda_lyric(raw), "[00:01.23]你好");
    }

    #[test]
    fn uses_soda_cover_template_from_h5_metadata() {
        let cover: SodaCover = serde_json::from_str(
            r#"{"urls":["https://p3-luna.douyinpic.com/img/"],"uri":"tos-cn/cover","template_prefix":"tplv-example"}"#,
        )
        .unwrap();
        assert_eq!(
            soda_cover_url(&cover),
            "https://p3-luna.douyinpic.com/img/tos-cn/cover~tplv-example-resize:960:960.png"
        );
    }

    #[test]
    fn filters_only_materially_short_soda_streams() {
        assert!(soda_play_info_is_shorter_than_track(180_000, 30.0));
        assert!(!soda_play_info_is_shorter_than_track(2_000, 1.91));
        assert!(!soda_play_info_is_shorter_than_track(180_000, 0.0));
    }

    #[tokio::test]
    #[ignore = "requires live Soda Music access, ffprobe, and ffmpeg"]
    async fn live_soda_high_download_is_playable_without_telegram() {
        let keyword = std::env::var("SODA_TEST_QUERY").unwrap_or_else(|_| "晴天".to_string());
        let track = resolve_with_quality(&keyword, None, "high").await.unwrap();
        assert!(track.url.starts_with(SODA_DOWNLOAD_SCHEME));
        let media = download_track_media(&track).await.unwrap();
        let path = std::env::temp_dir().join(format!(
            "bot-rs-soda-{}.{}",
            track.id,
            track.file_extension()
        ));
        std::fs::write(&path, &media.audio).unwrap();
        let probe = std::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_name:format=duration",
                "-of",
                "default=noprint_wrappers=1",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            probe.status.success(),
            "{}",
            String::from_utf8_lossy(&probe.stderr)
        );
        let stdout = String::from_utf8_lossy(&probe.stdout);
        assert!(stdout.contains("codec_name="), "{stdout}");
        assert!(stdout.contains("duration="), "{stdout}");
        let decode = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-t", "5", "-i"])
            .arg(&path)
            .args(["-f", "null", "-"])
            .output()
            .unwrap();
        assert!(
            decode.status.success(),
            "{}",
            String::from_utf8_lossy(&decode.stderr)
        );
        eprintln!("{} -> {} bytes", track.file_name(), media.audio.len());
    }
}
