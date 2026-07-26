use super::provider::{MusicLyrics, MusicPlatform, MusicProvider, MusicSearchItem, MusicTrack};
use crate::BotError;
use async_trait::async_trait;
use lazy_static::lazy_static;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, time::Duration};
use tokio::sync::Mutex;
use url::Url;

const INNER_TUBE_BASE_MUSIC: &str = "https://music.youtube.com/youtubei/v1";
const INNER_TUBE_BASE_VIDEO: &str = "https://www.youtube.com/youtubei/v1";
const WEB_REMIX_KEY: &str = "AIzaSyC9XL3ZjWddXya6X74dJoCTL-WEYFDNX30";
const WEB_REMIX_CLIENT_NAME: &str = "WEB_REMIX";
const WEB_REMIX_CLIENT_VERSION: &str = "1.20240101.01.00";
const ANDROID_VR_CLIENT_NAME: &str = "ANDROID_VR";
const ANDROID_VR_CLIENT_VERSION: &str = "1.62.27";
const ANDROID_VR_USER_AGENT: &str = "com.google.android.apps.youtube.vr.oculus/1.62.27 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip";
const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const GOOGLE_VIDEO_MAX_CHUNK: u64 = 1024 * 1024;

pub static YOUTUBE_MUSIC_PROVIDER: YouTubeMusicProvider = YouTubeMusicProvider;

lazy_static! {
    static ref CLIENT: Client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("youtube music http client");
    static ref VISITOR_DATA: Mutex<String> = Mutex::new(String::new());
}

pub struct YouTubeMusicProvider;

#[async_trait]
impl MusicProvider for YouTubeMusicProvider {
    async fn search(&self, keyword: &str, limit: usize) -> Result<Vec<MusicSearchItem>, BotError> {
        search_youtube_music(keyword, limit).await
    }

    async fn lyrics(&self, track_id: &str) -> Result<Option<MusicLyrics>, BotError> {
        let id = parse_youtube_music_id(track_id).unwrap_or_else(|| track_id.trim().to_string());
        if !is_youtube_video_id(&id) {
            return Ok(None);
        }
        get_youtube_music_lyrics(&id).await
    }

    async fn resolve(
        &self,
        keyword: &str,
        selected_id: Option<&str>,
    ) -> Result<MusicTrack, BotError> {
        resolve_youtube_music(keyword, selected_id).await
    }
}

async fn search_youtube_music(
    keyword: &str,
    limit: usize,
) -> Result<Vec<MusicSearchItem>, BotError> {
    if keyword.trim().is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.max(1);
    let payload = SearchRequest {
        context: web_context(),
        query: keyword.to_string(),
        params: Some("EgWKAQIIAWoMEAMQBBAJEAoQBRAV".to_string()),
    };
    let root = post_innertube(
        INNER_TUBE_BASE_MUSIC,
        "search",
        &payload,
        DEFAULT_USER_AGENT,
        None,
    )
    .await?;
    let mut seen = HashMap::new();
    let mut tracks = Vec::new();
    for item in collect_music_list_items(&root) {
        let Some(track) = track_from_list_item(item) else {
            continue;
        };
        if seen.insert(track.id.clone(), ()).is_some() {
            continue;
        }
        tracks.push(track);
        if tracks.len() >= limit {
            break;
        }
    }
    Ok(tracks)
}

async fn resolve_youtube_music(
    keyword: &str,
    selected_id: Option<&str>,
) -> Result<MusicTrack, BotError> {
    let selected = selected_id.and_then(parse_youtube_music_id);
    // An 11-char alphanumeric keyword (e.g. "Thunderbolt") is indistinguishable from a
    // video id, so remember when the id was only inferred from the bare keyword and allow
    // falling back to search if that "id" turns out not to resolve.
    let keyword_is_bare_id = selected.is_none() && is_youtube_video_id(keyword.trim());
    let id = match selected.or_else(|| parse_youtube_music_id(keyword)) {
        Some(id) => id,
        None => {
            search_youtube_music(keyword, 1)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| BotError::Custom("没有找到可下载的 YouTube Music".to_string()))?
                .id
        }
    };

    match resolve_youtube_music_by_id(&id).await {
        Err(err) if keyword_is_bare_id => {
            let Some(item) = search_youtube_music(keyword, 1)
                .await
                .ok()
                .and_then(|items| items.into_iter().next())
                .filter(|item| item.id != id)
            else {
                return Err(err);
            };
            resolve_youtube_music_by_id(&item.id).await.map_err(|_| err)
        }
        result => result,
    }
}

async fn resolve_youtube_music_by_id(id: &str) -> Result<MusicTrack, BotError> {
    let player = youtube_player(id).await?;
    ensure_playable(&player, id)?;
    let format = select_audio_format(
        player
            .streaming_data
            .adaptive_formats
            .iter()
            .chain(player.streaming_data.formats.iter()),
    )
    .ok_or_else(|| BotError::Custom("YouTube Music 没有返回可上传的 MP4/AAC 音频流".to_string()))?;

    let song = player.video_details.title.trim().to_string();
    let singer = clean_artist_name(&player.video_details.author);
    let duration = player
        .video_details
        .length_seconds
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|duration| *duration > 0);
    Ok(MusicTrack {
        id: id.to_string(),
        platform: MusicPlatform::YouTubeMusic,
        song: if song.is_empty() {
            "未知歌曲".to_string()
        } else {
            song
        },
        singer: if singer.is_empty() {
            "未知歌手".to_string()
        } else {
            singer
        },
        album: String::new(),
        cover: best_thumbnail(&player.video_details.thumbnail.thumbnails),
        link: format!("https://music.youtube.com/watch?v={id}"),
        url: format.url.clone(),
        headers: HashMap::from([("User-Agent".to_string(), ANDROID_VR_USER_AGENT.to_string())]),
        max_chunk_size: Some(GOOGLE_VIDEO_MAX_CHUNK),
        duration,
        // MusicTrack.bitrate 全链路按 bps 消费（caption 展示时才除以 1000）
        bitrate: best_bitrate(format),
        format: Some(format_from_mime(&format.mime_type).to_string()),
    })
}

async fn youtube_player(video_id: &str) -> Result<PlayerResponse, BotError> {
    let mut player = youtube_player_once(video_id, &visitor_data(false).await).await?;
    if is_bot_flagged(&player) {
        let refreshed = visitor_data(true).await;
        if !refreshed.is_empty() {
            player = youtube_player_once(video_id, &refreshed).await?;
        }
    }
    Ok(player)
}

async fn youtube_player_once(video_id: &str, visitor: &str) -> Result<PlayerResponse, BotError> {
    let mut context = android_vr_context();
    if !visitor.trim().is_empty() {
        context.client.visitor_data = Some(visitor.to_string());
    }
    context.third_party = Some(ThirdPartyInfo {
        embed_url: "https://www.youtube.com".to_string(),
    });
    context.user = Some(UserInfo {
        locked_safety_mode: false,
    });
    context.request = Some(RequestInfo {
        use_ssl: true,
        internal_experiment_flags: Vec::<String>::new(),
        consistency_token_jars: Vec::<String>::new(),
    });
    let payload = PlayerRequest {
        context,
        video_id: video_id.to_string(),
        content_check_ok: true,
        racy_check_ok: true,
        playback_context: PlaybackContextInfo {
            content_playback_context: ContentPlaybackContextInfo {
                html5_preference: "HTML5_PREF_WANTS".to_string(),
            },
        },
    };
    let mut headers =
        HashMap::from([("Origin".to_string(), "https://www.youtube.com".to_string())]);
    if !visitor.trim().is_empty() {
        headers.insert("X-Goog-Visitor-Id".to_string(), visitor.to_string());
    }
    let value = post_innertube(
        INNER_TUBE_BASE_VIDEO,
        "player",
        &payload,
        ANDROID_VR_USER_AGENT,
        Some(&headers),
    )
    .await?;
    Ok(serde_json::from_value(value)?)
}

async fn get_youtube_music_lyrics(video_id: &str) -> Result<Option<MusicLyrics>, BotError> {
    let next = post_innertube(
        INNER_TUBE_BASE_MUSIC,
        "next",
        &NextRequest {
            context: web_context(),
            video_id: video_id.to_string(),
        },
        DEFAULT_USER_AGENT,
        None,
    )
    .await?;
    let Some(browse_id) = find_lyrics_browse_id(&next) else {
        return Ok(None);
    };
    let browse = post_innertube(
        INNER_TUBE_BASE_MUSIC,
        "browse",
        &BrowseRequest {
            context: web_context(),
            browse_id,
        },
        DEFAULT_USER_AGENT,
        None,
    )
    .await?;
    let Some(plain) = find_lyrics_text(&browse).filter(|text| !text.trim().is_empty()) else {
        return Ok(None);
    };
    Ok(Some(MusicLyrics {
        plain,
        translation: String::new(),
    }))
}

async fn post_innertube<T: Serialize>(
    base: &str,
    endpoint: &str,
    payload: &T,
    user_agent: &str,
    extra_headers: Option<&HashMap<String, String>>,
) -> Result<Value, BotError> {
    let url = format!("{base}/{endpoint}?key={WEB_REMIX_KEY}&prettyPrint=false");
    let origin = extra_headers
        .and_then(|headers| headers.get("Origin"))
        .map(String::as_str)
        .unwrap_or("https://music.youtube.com");
    let mut request = CLIENT
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("User-Agent", user_agent)
        .header("Origin", origin)
        .header("X-Goog-Api-Format-Version", "1")
        .json(payload);
    if let Ok(cookie) = std::env::var("YOUTUBE_MUSIC_COOKIE")
        && !cookie.trim().is_empty()
    {
        request = request.header("Cookie", cookie.trim());
    }
    if let Some(headers) = extra_headers {
        for (key, value) in headers {
            if key.eq_ignore_ascii_case("Origin") {
                continue;
            }
            request = request.header(key, value);
        }
    }
    let resp = request.send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(BotError::Custom(format!(
            "YouTube Music API 请求失败：HTTP {status}"
        )));
    }
    Ok(resp.json().await?)
}

async fn visitor_data(force_refresh: bool) -> String {
    if !force_refresh {
        let cached = VISITOR_DATA.lock().await.clone();
        if !cached.is_empty() {
            return cached;
        }
    }
    let fetched = fetch_visitor_data().await;
    if !fetched.is_empty() {
        *VISITOR_DATA.lock().await = fetched.clone();
    }
    fetched
}

async fn fetch_visitor_data() -> String {
    let Ok(resp) = CLIENT
        .get("https://www.youtube.com/watch?v=dQw4w9WgXcQ&bpctr=9999999999&has_verified=1")
        .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/15.5 Safari/605.1.15")
        .header("Accept-Language", "en-us,en;q=0.5")
        .header("Cookie", "PREF=hl=en&tz=UTC; SOCS=CAI")
        .send()
        .await
    else {
        return String::new();
    };
    let Ok(html) = resp.text().await else {
        return String::new();
    };
    extract_visitor_data(&html)
}

fn extract_visitor_data(html: &str) -> String {
    let Some((_, rest)) = html.split_once("\"visitorData\":\"") else {
        return String::new();
    };
    let Some((token, _)) = rest.split_once('"') else {
        return String::new();
    };
    serde_json::from_str::<String>(&format!("\"{token}\"")).unwrap_or_else(|_| token.to_string())
}

fn web_context() -> InnerTubeContext {
    InnerTubeContext {
        client: ClientInfo {
            client_name: WEB_REMIX_CLIENT_NAME.to_string(),
            client_version: WEB_REMIX_CLIENT_VERSION.to_string(),
            hl: Some("en".to_string()),
            gl: Some("US".to_string()),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn android_vr_context() -> InnerTubeContext {
    InnerTubeContext {
        client: ClientInfo {
            client_name: ANDROID_VR_CLIENT_NAME.to_string(),
            client_version: ANDROID_VR_CLIENT_VERSION.to_string(),
            hl: Some("en".to_string()),
            gl: Some("US".to_string()),
            device_make: Some("Oculus".to_string()),
            device_model: Some("Quest 3".to_string()),
            os_name: Some("Android".to_string()),
            os_version: Some("12L".to_string()),
            android_sdk_version: Some(32),
            user_agent: Some(ANDROID_VR_USER_AGENT.to_string()),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn collect_music_list_items(root: &Value) -> Vec<&Value> {
    fn walk<'a>(node: &'a Value, out: &mut Vec<&'a Value>) {
        match node {
            Value::Object(map) => {
                if let Some(item) = map.get("musicResponsiveListItemRenderer") {
                    out.push(item);
                }
                for value in map.values() {
                    walk(value, out);
                }
            }
            Value::Array(values) => {
                for value in values {
                    walk(value, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out
}

fn track_from_list_item(item: &Value) -> Option<MusicSearchItem> {
    let id = find_video_id(item)?;
    let columns = item.get("flexColumns")?.as_array()?;
    let mut title = String::new();
    let mut singer = String::new();
    for (index, column) in columns.iter().enumerate() {
        let text = runs_text(
            column
                .get("musicResponsiveListItemFlexColumnRenderer")
                .and_then(|renderer| renderer.get("text"))
                .unwrap_or(&Value::Null),
        )
        .trim()
        .to_string();
        if text.is_empty() {
            continue;
        }
        if index == 0 || title.is_empty() {
            title = text;
            continue;
        }
        for part in text.split('•').map(str::trim) {
            if !part.is_empty() && !looks_like_duration(part) {
                singer = part.to_string();
                break;
            }
        }
        if !singer.is_empty() {
            break;
        }
    }
    if title.is_empty() {
        return None;
    }
    Some(MusicSearchItem {
        platform: MusicPlatform::YouTubeMusic,
        id,
        song: title,
        singer: if singer.is_empty() {
            "未知歌手".to_string()
        } else {
            singer
        },
        cover: find_thumbnail_url(item).unwrap_or_default(),
    })
}

fn runs_text(value: &Value) -> String {
    if let Some(text) = value.get("simpleText").and_then(Value::as_str) {
        return text.to_string();
    }
    let mut out = String::new();
    if let Some(runs) = value.get("runs").and_then(Value::as_array) {
        for run in runs {
            if let Some(text) = run.get("text").and_then(Value::as_str) {
                out.push_str(text);
            }
        }
    } else if let Some(text) = value.as_str() {
        out.push_str(text);
    }
    out
}

fn find_video_id(node: &Value) -> Option<String> {
    node.get("playlistItemData")
        .and_then(|data| data.get("videoId"))
        .and_then(Value::as_str)
        .filter(|id| is_youtube_video_id(id))
        .map(str::to_string)
        .or_else(|| find_video_id_deep(node))
}

fn find_video_id_deep(node: &Value) -> Option<String> {
    match node {
        Value::Object(map) => {
            if let Some(id) = map
                .get("videoId")
                .and_then(Value::as_str)
                .filter(|id| is_youtube_video_id(id))
            {
                return Some(id.to_string());
            }
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort();
            keys.into_iter()
                .find_map(|key| find_video_id_deep(&map[key]))
        }
        Value::Array(values) => values.iter().find_map(find_video_id_deep),
        _ => None,
    }
}

fn find_thumbnail_url(node: &Value) -> Option<String> {
    match node {
        Value::Object(map) => {
            if let Some(thumbnails) = map.get("thumbnails").and_then(Value::as_array) {
                let best = thumbnails
                    .iter()
                    .filter_map(|item| {
                        Some((
                            item.get("url")?.as_str()?.to_string(),
                            item.get("width")
                                .and_then(Value::as_u64)
                                .unwrap_or_default()
                                * item
                                    .get("height")
                                    .and_then(Value::as_u64)
                                    .unwrap_or_default(),
                        ))
                    })
                    .max_by_key(|(_, area)| *area)
                    .map(|(url, _)| url);
                if best.is_some() {
                    return best;
                }
            }
            map.values().find_map(find_thumbnail_url)
        }
        Value::Array(values) => values.iter().find_map(find_thumbnail_url),
        _ => None,
    }
}

fn find_lyrics_browse_id(node: &Value) -> Option<String> {
    match node {
        Value::Object(map) => {
            if let Some(id) = map
                .get("browseId")
                .and_then(Value::as_str)
                .filter(|id| id.starts_with("MPLYt"))
            {
                return Some(id.to_string());
            }
            map.values().find_map(find_lyrics_browse_id)
        }
        Value::Array(values) => values.iter().find_map(find_lyrics_browse_id),
        _ => None,
    }
}

fn find_lyrics_text(node: &Value) -> Option<String> {
    match node {
        Value::Object(map) => {
            if let Some(shelf) = map.get("musicDescriptionShelfRenderer") {
                let text = runs_text(shelf.get("description").unwrap_or(&Value::Null));
                if !text.trim().is_empty() {
                    return Some(text);
                }
            }
            map.values().find_map(find_lyrics_text)
        }
        Value::Array(values) => values.iter().find_map(find_lyrics_text),
        _ => None,
    }
}

fn parse_youtube_music_id(text: &str) -> Option<String> {
    let text = text.trim();
    if is_youtube_video_id(text) {
        return Some(text.to_string());
    }
    if let Some((prefix, value)) = text.split_once(':')
        && matches!(
            prefix.trim().to_ascii_lowercase().as_str(),
            "ytm" | "ytmusic" | "youtubemusic" | "youtube" | "yt"
        )
    {
        let value = value.trim();
        if is_youtube_video_id(value) {
            return Some(value.to_string());
        }
        if let Some(id) = text
            .split_whitespace()
            .find_map(|part| parse_youtube_url_part(part).and_then(video_id_from_youtube_url))
        {
            return Some(id);
        }
        return None;
    }
    if let Some(id) = text
        .split_whitespace()
        .find_map(|part| parse_youtube_url_part(part).and_then(video_id_from_youtube_url))
    {
        return Some(id);
    }
    let parsed = parse_youtube_url(text)?;
    video_id_from_youtube_url(parsed)
}

fn video_id_from_youtube_url(parsed: Url) -> Option<String> {
    let host = normalize_youtube_host(parsed.host_str()?);
    if host == "youtu.be" {
        return first_path_segment(parsed.path()).filter(|id| is_youtube_video_id(id));
    }
    if let Some(id) = parsed
        .query_pairs()
        .find_map(|(key, value)| (key == "v").then(|| value.to_string()))
        .filter(|id| is_youtube_video_id(id))
    {
        return Some(id);
    }
    let path = parsed.path().trim_matches('/');
    for prefix in ["shorts/", "embed/", "v/"] {
        if let Some(rest) = path.strip_prefix(prefix)
            && let Some(id) = first_path_segment(rest).filter(|id| is_youtube_video_id(id))
        {
            return Some(id);
        }
    }
    None
}

fn parse_youtube_url_part(text: &str) -> Option<Url> {
    let text = text.trim_matches(|ch: char| matches!(ch, ',' | '.' | '!' | '?' | ')' | ']' | '}'));
    if text.contains("youtube.com") || text.contains("youtu.be") {
        parse_youtube_url(text)
    } else {
        None
    }
}

fn parse_youtube_url(text: &str) -> Option<Url> {
    let mut raw = text.trim().to_string();
    if raw.is_empty() {
        return None;
    }
    if !raw.contains("://") {
        raw = format!("https://{raw}");
    }
    let parsed = Url::parse(&raw).ok()?;
    if is_youtube_host(&normalize_youtube_host(parsed.host_str()?)) {
        Some(parsed)
    } else {
        None
    }
}

fn normalize_youtube_host(host: &str) -> String {
    host.trim()
        .to_ascii_lowercase()
        .trim_start_matches("www.")
        .trim_start_matches("m.")
        .to_string()
}

fn is_youtube_host(host: &str) -> bool {
    matches!(host, "music.youtube.com" | "youtube.com" | "youtu.be")
}

fn first_path_segment(path: &str) -> Option<String> {
    path.trim_matches('/')
        .split('/')
        .find(|segment| !segment.is_empty())
        .map(str::to_string)
}

fn is_youtube_video_id(value: &str) -> bool {
    value.len() == 11
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn looks_like_duration(value: &str) -> bool {
    value.contains(':')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b':')
}

fn ensure_playable(player: &PlayerResponse, video_id: &str) -> Result<(), BotError> {
    let status = player.playability_status.status.trim();
    if status.is_empty() || status.eq_ignore_ascii_case("OK") {
        return Ok(());
    }
    if player
        .playability_status
        .reason
        .to_ascii_lowercase()
        .contains("not available")
    {
        return Err(BotError::Custom("YouTube Music 曲目不可用".to_string()));
    }
    Err(BotError::Custom(format!(
        "YouTube Music 无法播放 {video_id}：{}",
        player.playability_status.reason
    )))
}

fn is_bot_flagged(player: &PlayerResponse) -> bool {
    player
        .playability_status
        .status
        .eq_ignore_ascii_case("LOGIN_REQUIRED")
        || player
            .playability_status
            .reason
            .to_ascii_lowercase()
            .contains("not a bot")
}

fn select_audio_format<'a>(
    formats: impl Iterator<Item = &'a StreamFormat>,
) -> Option<&'a StreamFormat> {
    // Telegram sendAudio 只接受 MP3/M4A：只选 audio/mp4（AAC）流。
    // WebM/Opus 不经转码无法上传，与其发出坏文件不如明确报错（本项目不引入 ffmpeg）
    formats
        .filter(|format| is_mp4_audio(&format.mime_type))
        .filter(|format| !format.url.trim().is_empty())
        .max_by_key(|format| best_bitrate(format).unwrap_or_default())
}

fn is_mp4_audio(mime: &str) -> bool {
    // 容器必须是 audio/mp4 本身：video/mp4 的 codecs 里也含 "mp4a"（AAC 音轨），
    // audio/mp4a-latm 是 LATM 裸流而非 ISO-BMFF；再要求 codecs 为 AAC（mp4a），
    // 排除 ec-3/ac-3/dtse/opus-in-mp4 这类装在 mp4 容器里但多数客户端放不了的编码
    let mime = mime.trim().to_ascii_lowercase();
    let container = mime.split(';').next().unwrap_or_default().trim();
    container == "audio/mp4" && mime.contains("mp4a")
}

fn best_bitrate(format: &StreamFormat) -> Option<u32> {
    format
        .average_bitrate
        .filter(|bitrate| *bitrate > 0)
        .or(format.bitrate.filter(|bitrate| *bitrate > 0))
}

fn best_thumbnail(thumbnails: &[Thumbnail]) -> String {
    thumbnails
        .iter()
        .filter(|thumbnail| !thumbnail.url.trim().is_empty())
        .max_by_key(|thumbnail| {
            thumbnail.width.unwrap_or_default() * thumbnail.height.unwrap_or_default()
        })
        .map(|thumbnail| thumbnail.url.clone())
        .unwrap_or_default()
}

fn clean_artist_name(name: &str) -> String {
    name.trim()
        .strip_suffix("- Topic")
        .unwrap_or(name.trim())
        .trim()
        .to_string()
}

fn format_from_mime(mime: &str) -> &'static str {
    // mp4 分支必须先于 opus：audio/mp4; codecs="opus"/"iamf...Opus" 这类
    // mime 同时含两种关键字，被选中的 mp4 容器流扩展名必须是 m4a
    let mime = mime.to_ascii_lowercase();
    if mime.contains("mp4a") || mime.contains("m4a") || mime.contains("audio/mp4") {
        "m4a"
    } else if mime.contains("opus") {
        "opus"
    } else if mime.contains("webm") {
        "webm"
    } else if mime.contains("mpeg") || mime.contains("mp3") {
        "mp3"
    } else {
        "m4a"
    }
}

#[derive(Serialize)]
struct SearchRequest {
    context: InnerTubeContext,
    query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<String>,
}

#[derive(Serialize)]
struct PlayerRequest {
    context: InnerTubeContext,
    #[serde(rename = "videoId")]
    video_id: String,
    #[serde(rename = "contentCheckOk")]
    content_check_ok: bool,
    #[serde(rename = "racyCheckOk")]
    racy_check_ok: bool,
    #[serde(rename = "playbackContext")]
    playback_context: PlaybackContextInfo,
}

#[derive(Serialize)]
struct PlaybackContextInfo {
    #[serde(rename = "contentPlaybackContext")]
    content_playback_context: ContentPlaybackContextInfo,
}

#[derive(Serialize)]
struct ContentPlaybackContextInfo {
    #[serde(rename = "html5Preference")]
    html5_preference: String,
}

#[derive(Serialize)]
struct NextRequest {
    context: InnerTubeContext,
    #[serde(rename = "videoId")]
    video_id: String,
}

#[derive(Serialize)]
struct BrowseRequest {
    context: InnerTubeContext,
    #[serde(rename = "browseId")]
    browse_id: String,
}

#[derive(Default, Serialize)]
struct InnerTubeContext {
    client: ClientInfo,
    #[serde(skip_serializing_if = "Option::is_none", rename = "thirdParty")]
    third_party: Option<ThirdPartyInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user: Option<UserInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request: Option<RequestInfo>,
}

#[derive(Default, Serialize)]
struct ClientInfo {
    #[serde(rename = "clientName")]
    client_name: String,
    #[serde(rename = "clientVersion")]
    client_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    hl: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gl: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "deviceMake")]
    device_make: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "deviceModel")]
    device_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "osName")]
    os_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "osVersion")]
    os_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "androidSdkVersion")]
    android_sdk_version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "userAgent")]
    user_agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "visitorData")]
    visitor_data: Option<String>,
}

#[derive(Serialize)]
struct ThirdPartyInfo {
    #[serde(rename = "embedUrl")]
    embed_url: String,
}

#[derive(Serialize)]
struct UserInfo {
    #[serde(rename = "lockedSafetyMode")]
    locked_safety_mode: bool,
}

#[derive(Serialize)]
struct RequestInfo {
    #[serde(rename = "useSsl")]
    use_ssl: bool,
    #[serde(rename = "internalExperimentFlags")]
    internal_experiment_flags: Vec<String>,
    #[serde(rename = "consistencyTokenJars")]
    consistency_token_jars: Vec<String>,
}

#[derive(Deserialize)]
struct PlayerResponse {
    #[serde(default, rename = "playabilityStatus")]
    playability_status: PlayabilityStatus,
    #[serde(default, rename = "streamingData")]
    streaming_data: StreamingData,
    #[serde(default, rename = "videoDetails")]
    video_details: VideoDetails,
}

#[derive(Default, Deserialize)]
struct PlayabilityStatus {
    #[serde(default)]
    status: String,
    #[serde(default)]
    reason: String,
}

#[derive(Default, Deserialize)]
struct StreamingData {
    #[serde(default, rename = "formats")]
    formats: Vec<StreamFormat>,
    #[serde(default, rename = "adaptiveFormats")]
    adaptive_formats: Vec<StreamFormat>,
}

#[derive(Default, Deserialize)]
struct VideoDetails {
    #[serde(default)]
    title: String,
    #[serde(default, rename = "lengthSeconds")]
    length_seconds: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    thumbnail: ThumbnailList,
}

#[derive(Default, Deserialize)]
struct StreamFormat {
    #[serde(default)]
    url: String,
    #[serde(default, rename = "mimeType")]
    mime_type: String,
    #[serde(default)]
    bitrate: Option<u32>,
    #[serde(default, rename = "averageBitrate")]
    average_bitrate: Option<u32>,
}

#[derive(Default, Deserialize)]
struct ThumbnailList {
    #[serde(default)]
    thumbnails: Vec<Thumbnail>,
}

#[derive(Deserialize)]
struct Thumbnail {
    #[serde(default)]
    url: String,
    #[serde(default)]
    width: Option<u64>,
    #[serde(default)]
    height: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::{super::provider::download_track_media, *};

    #[test]
    fn parses_youtube_music_ids() {
        assert_eq!(
            parse_youtube_music_id("https://music.youtube.com/watch?v=dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
        assert_eq!(
            parse_youtube_music_id("https://youtu.be/dQw4w9WgXcQ?si=x"),
            Some("dQw4w9WgXcQ".to_string())
        );
        assert_eq!(
            parse_youtube_music_id("ytm:dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
        assert_eq!(
            parse_youtube_music_id("YT: https://www.youtube.com/watch?v=dQw4w9WgXcQ,"),
            Some("dQw4w9WgXcQ".to_string())
        );
        assert_eq!(parse_youtube_music_id("not-video"), None);
    }

    #[test]
    fn extracts_visitor_data_from_watch_html() {
        assert_eq!(
            extract_visitor_data(r#"x "visitorData":"abc\u003d" y"#),
            "abc="
        );
    }

    #[test]
    fn prefers_mp4_audio_over_higher_bitrate_opus() {
        // Telegram sendAudio 只接受 MP3/M4A：即使 opus 码率更高也要选 mp4
        let formats = [
            StreamFormat {
                url: "https://example.com/aac".to_string(),
                mime_type: "audio/mp4; codecs=\"mp4a.40.2\"".to_string(),
                bitrate: Some(128_000),
                average_bitrate: None,
            },
            StreamFormat {
                url: "https://example.com/opus".to_string(),
                mime_type: "audio/webm; codecs=\"opus\"".to_string(),
                bitrate: None,
                average_bitrate: Some(251_000),
            },
        ];
        let best = select_audio_format(formats.iter()).unwrap();
        assert_eq!(best.url, "https://example.com/aac");
        assert_eq!(format_from_mime(&best.mime_type), "m4a");
    }

    #[test]
    fn selects_best_mp4_bitrate_and_rejects_opus_only() {
        let mp4 = |url: &str, bitrate: u32| StreamFormat {
            url: url.to_string(),
            mime_type: "audio/mp4; codecs=\"mp4a.40.2\"".to_string(),
            bitrate: Some(bitrate),
            average_bitrate: None,
        };
        let formats = [
            mp4("https://example.com/low", 48_000),
            mp4("https://example.com/high", 128_000),
        ];
        assert_eq!(
            select_audio_format(formats.iter()).unwrap().url,
            "https://example.com/high"
        );

        // 没有 mp4 流时不兜底其他容器：WebM/Opus 无法通过 sendAudio 上传，
        // 返回 None 让调用方给出明确错误
        let opus_only = [StreamFormat {
            url: "https://example.com/opus".to_string(),
            mime_type: "audio/webm; codecs=\"opus\"".to_string(),
            bitrate: None,
            average_bitrate: Some(160_000),
        }];
        assert!(select_audio_format(opus_only.iter()).is_none());
    }

    #[test]
    fn rejects_video_mp4_even_with_aac_track() {
        // video/mp4 的 codecs 里也带 "mp4a"（AAC 音轨）且码率更高，
        // 但它是视频流，不能被当作 .m4a 上传
        let video = || StreamFormat {
            url: "https://example.com/video".to_string(),
            mime_type: "video/mp4; codecs=\"avc1.4d401f, mp4a.40.2\"".to_string(),
            bitrate: Some(1_500_000),
            average_bitrate: None,
        };
        let audio = StreamFormat {
            url: "https://example.com/audio".to_string(),
            mime_type: "audio/mp4; codecs=\"mp4a.40.2\"".to_string(),
            bitrate: Some(128_000),
            average_bitrate: None,
        };
        let mixed = [video(), audio];
        assert_eq!(
            select_audio_format(mixed.iter()).unwrap().url,
            "https://example.com/audio"
        );

        let video_only = [video()];
        assert!(select_audio_format(video_only.iter()).is_none());
    }

    #[test]
    fn rejects_non_aac_mp4_containers_and_latm() {
        let format = |url: &str, mime: &str, bitrate: u32| StreamFormat {
            url: url.to_string(),
            mime_type: mime.to_string(),
            bitrate: Some(bitrate),
            average_bitrate: None,
        };
        // ec-3（杜比）码率更高但多数 Telegram 客户端不能解码，不得选中
        let with_dolby = [
            format(
                "https://example.com/ec3",
                "audio/mp4; codecs=\"ec-3\"",
                384_000,
            ),
            format(
                "https://example.com/aac",
                "audio/mp4; codecs=\"mp4a.40.2\"",
                129_000,
            ),
        ];
        assert_eq!(
            select_audio_format(with_dolby.iter()).unwrap().url,
            "https://example.com/aac"
        );

        // opus-in-mp4（含 IAMF）与 LATM 裸流都不是可上传的 m4a
        let unusable = [
            format(
                "https://example.com/iamf",
                "audio/mp4; codecs=\"iamf.001.001.Opus\"",
                256_000,
            ),
            format("https://example.com/latm", "audio/mp4a-latm", 128_000),
        ];
        assert!(select_audio_format(unusable.iter()).is_none());
    }

    #[tokio::test]
    #[ignore = "requires live YouTube Music access"]
    async fn live_search_resolve_and_download_without_telegram() {
        let tracks = search_youtube_music("Lemon 米津玄師", 1).await.unwrap();
        let first = tracks.first().expect("search result");
        let track = resolve_youtube_music(&first.id, Some(&first.id))
            .await
            .unwrap();
        assert_eq!(track.platform, MusicPlatform::YouTubeMusic);
        assert_eq!(track.max_chunk_size, Some(GOOGLE_VIDEO_MAX_CHUNK));

        let media = download_track_media(&track).await.unwrap();
        assert!(!media.audio.is_empty());
    }
}
