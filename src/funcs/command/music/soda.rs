use super::provider::{
    DownloadProgress, MusicPlatform, MusicProvider, MusicSearchItem, MusicTrack,
    get_json_with_headers,
};
use crate::BotError;
use aes::cipher::{KeyIvInit, StreamCipher};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose};
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use url::Url;

const SODA_AID: &str = "386088";
const SODA_CHANNEL: &str = "pc_web";
const SODA_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";
const SODA_DOWNLOAD_SCHEME: &str = "soda-download://";
type Aes128Ctr = ctr::Ctr128BE<aes::Aes128>;

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
        resolve_soda_track(keyword, selected_id, "high").await
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
            .unwrap_or_else(|| String::new()),
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
    let track = details.track;
    let play_info_url = details
        .player_info_url
        .filter(|url| !url.trim().is_empty())
        .ok_or_else(|| BotError::Custom("汽水音乐没有返回播放信息链接".to_string()))?;
    let play_infos = get_soda_play_infos(&play_info_url).await?;
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
        duration: track
            .duration
            .filter(|duration| *duration > 0)
            .map(|duration| ((duration + 500) / 1000) as u32),
        bitrate: (play_info.bitrate > 0).then_some(play_info.bitrate as u32),
        format: Some(format),
    })
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
    #[serde(rename = "Format")]
    format: Option<String>,
    #[serde(rename = "Quality")]
    quality: Option<String>,
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

async fn get_soda_play_infos(url: &str) -> Result<Vec<SodaPlayInfo>, BotError> {
    let response: SodaPlayInfoResponse = get_soda_json(url).await?;
    let mut list = response.result.data.play_info_list;
    if list.is_empty() {
        return Err(BotError::Custom("汽水音乐没有返回播放清单".to_string()));
    }
    list.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| b.bitrate.cmp(&a.bitrate)));
    Ok(list)
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
        decrypt_soda_audio(&encrypted, play_auth).unwrap_or(encrypted)
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
    let client = reqwest::Client::new();
    let mut response = client
        .get(url)
        .header("User-Agent", SODA_USER_AGENT)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(BotError::Custom(format!(
            "汽水音乐下载失败：HTTP {}",
            response.status()
        )));
    }
    let total = response.content_length();
    let mut buf = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        buf.extend_from_slice(&chunk);
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

fn decrypt_soda_audio(file_data: &[u8], play_auth: &str) -> Result<Vec<u8>, BotError> {
    let hex_key = extract_soda_key(play_auth)?;
    let key = hex_decode(&hex_key)?;
    if key.len() != 16 {
        return Err(BotError::Custom(
            "汽水音乐 PlayAuth key 长度错误".to_string(),
        ));
    }
    let moov = find_soda_box(file_data, b"moov", 0, file_data.len())?;
    let (trak, stbl) = find_soda_audio_track(file_data, moov)?;
    let sample_ranges = parse_soda_sample_ranges(file_data, stbl)?;
    let senc = find_soda_box(file_data, b"senc", trak.payload_start(), trak.end())
        .or_else(|_| find_soda_box(file_data, b"senc", stbl.payload_start(), stbl.end()))
        .or_else(|_| find_soda_box(file_data, b"senc", moov.payload_start(), moov.end()))?;
    let samples = parse_soda_senc(&file_data[senc.payload_start()..senc.end()])?;
    let mut decrypted = file_data.to_vec();
    for (index, range) in sample_ranges.iter().enumerate() {
        if index >= samples.len() {
            break;
        }
        let end = range.offset + range.size;
        if end > decrypted.len() {
            return Err(BotError::Custom("汽水音乐 sample 超出文件范围".to_string()));
        }
        decrypt_soda_sample(&mut decrypted[range.offset..end], &key, &samples[index])?;
    }
    rewrite_soda_audio_sample_entries(&mut decrypted)?;
    Ok(decrypted)
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
    if size < header_size || start + size > end || start + size > data.len() {
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
        for chunk_index in chunk_start..chunk_end.min(chunk_offsets.len()) {
            let mut offset = chunk_offsets[chunk_index] as usize;
            for _ in 0..entry.samples_per_chunk {
                if sample_index >= sample_sizes.len() {
                    break;
                }
                let size = sample_sizes[sample_index] as usize;
                if offset + size > data.len() {
                    return Err(BotError::Custom("汽水音乐 MP4 sample 越界".to_string()));
                }
                ranges.push(SodaSampleRange { offset, size });
                offset += size;
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
        assert!(url.contains("api.qishui.com/luna/pc/search/track"));
        assert!(url.contains("aid=386088"));
        assert!(url.contains("channel=pc_web"));
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
            },
            SodaPlayInfo {
                main_play_url: Some("https://example.com/highest.m4a".to_string()),
                backup_play_url: None,
                play_auth: None,
                format: Some("mp4".to_string()),
                quality: Some("highest".to_string()),
                size: 2,
                bitrate: 320,
            },
        ];

        let selected = select_soda_play_info(&play_infos, "high").unwrap();
        assert_eq!(selected.quality.as_deref(), Some("highest"));
        assert_eq!(normalize_soda_format(selected.format.as_deref()), "m4a");
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
