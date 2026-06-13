use super::provider::{
    MusicPlatform, MusicProvider, MusicSearchItem, MusicTrack, get_json_with_headers,
};
use crate::BotError;
use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use url::Url;

const SODA_AID: &str = "386088";
const SODA_CHANNEL: &str = "pc_web";
const SODA_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";

pub static SODA_PROVIDER: SodaProvider = SodaProvider;

pub struct SodaProvider;

#[async_trait]
impl MusicProvider for SodaProvider {
    async fn search(&self, keyword: &str, limit: usize) -> Result<Vec<MusicSearchItem>, BotError> {
        let url = soda_search_url(keyword);
        let response: SodaSearchResponse = get_soda_json(&url).await?;
        Ok(response
            .result_groups
            .into_iter()
            .next()
            .map(|group| group.data)
            .unwrap_or_default()
            .into_iter()
            .take(limit)
            .filter_map(|item| soda_track_to_item(item.entity.track))
            .collect())
    }

    async fn resolve(
        &self,
        keyword: &str,
        selected_id: Option<&str>,
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
                .unwrap_or_else(|| String::new()),
        };
        let id = if id.is_empty() {
            self.search(keyword, 1)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| BotError::Custom("没有找到可下载的音乐".to_string()))?
                .id
        } else {
            id
        };

        let details = get_soda_track_details(&id).await?;
        let track = details.track;
        let play_info_url = details
            .player_info_url
            .filter(|url| !url.trim().is_empty())
            .ok_or_else(|| BotError::Custom("汽水音乐没有返回播放信息链接".to_string()))?;
        let play_info = get_soda_play_info(&play_info_url).await?;
        let url = first_non_empty([
            play_info.main_play_url.as_deref(),
            play_info.backup_play_url.as_deref(),
        ])
        .ok_or_else(|| BotError::Custom("汽水音乐没有返回可下载直链".to_string()))?
        .to_string();

        let mut headers = soda_headers();
        if let Some(auth) = play_info.play_auth.filter(|auth| !auth.trim().is_empty()) {
            headers.insert("X-Soda-Play-Auth".to_string(), auth);
        }

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
            url,
            headers,
            duration: track
                .duration
                .filter(|duration| *duration > 0)
                .map(|duration| ((duration + 500) / 1000) as u32),
            bitrate: (play_info.bitrate > 0).then_some(play_info.bitrate as u32),
            format: None,
        })
    }
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
    track: SodaTrack,
}

#[derive(Deserialize)]
struct SodaTrackResponse {
    track_info: Option<SodaTrack>,
    track: Option<SodaTrack>,
    track_player: Option<SodaTrackPlayer>,
}

#[derive(Deserialize)]
struct SodaTrackPlayer {
    url_player_info: Option<String>,
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

#[derive(Clone, Deserialize)]
struct SodaTrack {
    id: String,
    name: String,
    #[serde(default)]
    duration: Option<u64>,
    #[serde(default)]
    artists: Vec<SodaArtist>,
    #[serde(default)]
    album: SodaAlbum,
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
}

#[derive(Deserialize)]
struct SodaPlayInfo {
    #[serde(rename = "MainPlayUrl")]
    main_play_url: Option<String>,
    #[serde(rename = "BackupPlayUrl")]
    backup_play_url: Option<String>,
    #[serde(rename = "PlayAuth")]
    play_auth: Option<String>,
    #[serde(default, rename = "Size")]
    size: i64,
    #[serde(default, rename = "Bitrate")]
    bitrate: i64,
}

struct SodaTrackDetails {
    track: SodaTrack,
    player_info_url: Option<String>,
}

async fn get_soda_track_details(id: &str) -> Result<SodaTrackDetails, BotError> {
    let url = soda_track_v2_url(id);
    let response: SodaTrackResponse = get_soda_json(&url).await?;
    let track = response
        .track_info
        .or(response.track)
        .ok_or_else(|| BotError::Custom("没有找到汽水音乐歌曲详情".to_string()))?;
    let player_info_url = response
        .track_player
        .and_then(|player| player.url_player_info);
    Ok(SodaTrackDetails {
        track,
        player_info_url,
    })
}

async fn get_soda_play_info(url: &str) -> Result<SodaPlayInfo, BotError> {
    let response: SodaPlayInfoResponse = get_soda_json(url).await?;
    response
        .result
        .data
        .play_info_list
        .into_iter()
        .max_by_key(|item| (item.size, item.bitrate))
        .ok_or_else(|| BotError::Custom("汽水音乐没有返回播放清单".to_string()))
}

async fn get_soda_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T, BotError> {
    get_json_with_headers(url, &soda_headers()).await
}

fn soda_search_url(keyword: &str) -> String {
    let mut url = Url::parse("https://api.qishui.com/luna/pc/search/track").unwrap();
    url.query_pairs_mut()
        .append_pair("q", keyword)
        .append_pair("cursor", "0")
        .append_pair("search_method", "input")
        .append_pair("aid", SODA_AID)
        .append_pair("device_platform", "web")
        .append_pair("channel", SODA_CHANNEL);
    url.to_string()
}

fn soda_track_v2_url(id: &str) -> String {
    let mut url = Url::parse("https://api.qishui.com/luna/pc/track_v2").unwrap();
    url.query_pairs_mut()
        .append_pair("track_id", id)
        .append_pair("media_type", "track")
        .append_pair("aid", SODA_AID)
        .append_pair("device_platform", "web")
        .append_pair("channel", SODA_CHANNEL);
    url.to_string()
}

fn soda_track_to_item(track: SodaTrack) -> Option<MusicSearchItem> {
    (!track.id.trim().is_empty()).then(|| MusicSearchItem {
        platform: MusicPlatform::Soda,
        id: track.id.clone(),
        song: track.name.clone(),
        singer: soda_artists(&track),
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

fn is_soda_numeric_id(value: &str) -> bool {
    !value.trim().is_empty() && value.len() >= 8 && value.chars().all(|c| c.is_ascii_digit())
}

fn soda_headers() -> HashMap<String, String> {
    let mut headers: HashMap<String, String> = [
        ("User-Agent".to_string(), SODA_USER_AGENT.to_string()),
        (
            "Accept".to_string(),
            "application/json, text/plain, */*".to_string(),
        ),
    ]
    .into_iter()
    .collect();
    if let Ok(cookie) = std::env::var("SODA_COOKIE")
        && !cookie.trim().is_empty()
    {
        headers.insert("Cookie".to_string(), cookie.trim().to_string());
    }
    headers
}

fn first_non_empty<'a>(items: impl IntoIterator<Item = Option<&'a str>>) -> Option<&'a str> {
    items
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|item| !item.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(url.contains("api.qishui.com/luna/pc/search/track"));
        assert!(url.contains("aid=386088"));
        assert!(url.contains("channel=pc_web"));
    }
}
