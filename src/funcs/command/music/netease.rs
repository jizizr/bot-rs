use super::provider::{
    MusicCollection, MusicLyrics, MusicPlatform, MusicProvider, MusicSearchItem, MusicTrack,
    get_json_with_headers, json_id_to_string,
};
use crate::{BotError, settings::SETTINGS};
use async_trait::async_trait;
use serde::Deserialize;
use std::collections::HashMap;

pub static NETEASE_PROVIDER: NeteaseProvider = NeteaseProvider;

pub struct NeteaseProvider;

#[async_trait]
impl MusicProvider for NeteaseProvider {
    async fn search(&self, keyword: &str, limit: usize) -> Result<Vec<MusicSearchItem>, BotError> {
        match search_netease_native(keyword, limit).await {
            Ok(songs) if !songs.is_empty() => Ok(songs),
            _ => search_netease_by_vkeys(keyword, limit)
                .await
                .map(|songs| vkeys_netease_to_items(songs, limit)),
        }
    }

    async fn resolve(
        &self,
        keyword: &str,
        selected_id: Option<&str>,
    ) -> Result<MusicTrack, BotError> {
        resolve_with_quality(keyword, selected_id, "lossless").await
    }

    async fn collection(
        &self,
        keyword: &str,
        limit: usize,
    ) -> Result<Option<MusicCollection>, BotError> {
        let Some(collection) = parse_netease_collection_id(keyword) else {
            return Ok(None);
        };
        get_netease_collection(collection, limit).await.map(Some)
    }

    async fn lyrics(&self, track_id: &str) -> Result<Option<MusicLyrics>, BotError> {
        let id = parse_netease_id(track_id).unwrap_or_else(|| track_id.trim().to_string());
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
            return Ok(None);
        }
        match get_native_netease_lyrics(&id).await {
            Ok(Some(lyrics)) => Ok(Some(lyrics)),
            Ok(None) | Err(_) => get_vkeys_netease_lyrics(&id).await,
        }
    }
}

pub(super) async fn resolve_with_quality(
    keyword: &str,
    selected_id: Option<&str>,
    quality: &str,
) -> Result<MusicTrack, BotError> {
    let id = resolve_netease_id(keyword, selected_id).await?;
    if should_prefer_vkeys_netease_download(quality)
        && netease_cookie().is_none()
        && vkeys_enabled()
    {
        match get_vkeys_netease_track(&id, quality).await {
            Ok(track) => return Ok(track),
            Err(vkeys_error) => match resolve_netease_native(&id, quality).await {
                Ok(track) => return Ok(track),
                Err(_) => return Err(vkeys_error),
            },
        }
    }
    match resolve_netease_native(&id, quality).await {
        Ok(track) => Ok(track),
        Err(native_error) if vkeys_enabled() => match get_vkeys_netease_track(&id, quality).await {
            Ok(track) => Ok(track),
            Err(_) => Err(native_error),
        },
        Err(native_error) => Err(native_error),
    }
}

async fn resolve_netease_id(keyword: &str, selected_id: Option<&str>) -> Result<String, BotError> {
    if let Some(id) = selected_id {
        return Ok(id.to_string());
    }
    if let Some(id) = parse_netease_id(keyword) {
        return Ok(id);
    }
    NETEASE_PROVIDER
        .search(keyword, 1)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| BotError::Custom("没有找到可下载的音乐".to_string()))
        .map(|item| item.id)
}

async fn resolve_netease_native(id: &str, quality: &str) -> Result<MusicTrack, BotError> {
    let (song, download_url) =
        tokio::try_join!(get_netease_song(id), get_netease_download_url(id, quality))?;
    let singer = netease_artists(&song);
    let album = song.album.unwrap_or_default();
    Ok(MusicTrack {
        id: id.to_string(),
        platform: MusicPlatform::Netease,
        song: song.name,
        singer,
        album: album.name,
        cover: album.pic_url.unwrap_or_default(),
        link: format!("https://music.163.com/#/song?id={id}"),
        url: download_url,
        headers: HashMap::new(),
        duration: song
            .duration
            .filter(|duration| *duration > 0)
            .map(|duration| ((duration + 500) / 1000) as u32),
        bitrate: None,
        format: Some("mp3".to_string()),
    })
}

async fn search_netease_native(
    keyword: &str,
    limit: usize,
) -> Result<Vec<MusicSearchItem>, BotError> {
    let url = format!(
        "https://music.163.com/api/search/get/web?csrf_token=&s={}&type=1&offset=0&total=true&limit={limit}",
        urlencoding::encode(keyword)
    );
    let response: NeteaseSearchResponse = get_netease_json(&url).await?;
    Ok(response
        .result
        .and_then(|result| result.songs)
        .unwrap_or_default()
        .into_iter()
        .map(netease_song_to_item)
        .filter(|item| !item.id.is_empty())
        .collect())
}

async fn get_native_netease_lyrics(id: &str) -> Result<Option<MusicLyrics>, BotError> {
    let response = get_netease_lyrics(id).await?;
    let plain = response.lrc.map(|lyric| lyric.lyric).unwrap_or_default();
    let translation = response.tlyric.map(|lyric| lyric.lyric).unwrap_or_default();
    if plain.trim().is_empty() && translation.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(MusicLyrics { plain, translation }))
}

#[derive(Deserialize)]
struct NeteaseSearchResponse {
    result: Option<NeteaseSearchResult>,
}

#[derive(Deserialize)]
struct NeteaseSearchResult {
    songs: Option<Vec<NeteaseSong>>,
}

#[derive(Deserialize)]
struct NeteaseDetailResponse {
    songs: Vec<NeteaseSong>,
}

#[derive(Deserialize)]
struct NeteaseUrlResponse {
    data: Vec<NeteaseUrlData>,
}

#[derive(Deserialize)]
struct NeteaseUrlData {
    url: Option<String>,
}

#[derive(Deserialize)]
struct NeteasePlaylistResponse {
    playlist: Option<NeteasePlaylist>,
}

#[derive(Deserialize)]
struct NeteasePlaylist {
    id: serde_json::Value,
    name: String,
    #[serde(default)]
    tracks: Vec<NeteaseSong>,
}

#[derive(Deserialize)]
struct NeteaseAlbumResponse {
    album: Option<NeteaseAlbumInfo>,
    #[serde(default)]
    songs: Vec<NeteaseSong>,
}

#[derive(Deserialize)]
struct NeteaseAlbumInfo {
    id: serde_json::Value,
    name: String,
    artist: Option<NeteaseArtist>,
}

#[derive(Deserialize)]
struct NeteaseLyricResponse {
    lrc: Option<NeteaseLyricPart>,
    tlyric: Option<NeteaseLyricPart>,
}

#[derive(Deserialize)]
struct NeteaseLyricPart {
    #[serde(default)]
    lyric: String,
}

#[derive(Deserialize)]
struct NeteaseSong {
    id: serde_json::Value,
    name: String,
    #[serde(default)]
    artists: Vec<NeteaseArtist>,
    #[serde(default, rename = "duration")]
    duration: Option<u64>,
    album: Option<NeteaseAlbum>,
}

#[derive(Deserialize)]
struct NeteaseArtist {
    name: String,
}

#[derive(Default, Deserialize)]
struct NeteaseAlbum {
    #[serde(default)]
    name: String,
    #[serde(rename = "picUrl")]
    pic_url: Option<String>,
}

#[derive(Clone, Default, Deserialize)]
struct VkeysNeteaseTrack {
    #[serde(default)]
    id: serde_json::Value,
    #[serde(default)]
    song: String,
    #[serde(default)]
    singer: String,
    #[serde(default)]
    album: String,
    #[serde(default)]
    cover: String,
    #[serde(default)]
    interval: String,
    #[serde(default)]
    link: String,
    #[serde(default)]
    quality: String,
    #[serde(default)]
    size: String,
    #[serde(default)]
    kbps: String,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum VkeysNeteaseData {
    List(Vec<VkeysNeteaseTrack>),
    Track(Box<VkeysNeteaseTrack>),
}

#[derive(Deserialize)]
struct VkeysNeteaseLyricsData {
    #[serde(default)]
    lrc: String,
    #[serde(default)]
    tlyric: String,
}

async fn search_netease_by_vkeys(
    keyword: &str,
    limit: usize,
) -> Result<Vec<VkeysNeteaseTrack>, BotError> {
    if !vkeys_enabled() {
        return Err(BotError::Custom("vkeys 未启用".to_string()));
    }
    let mut url = vkeys_url("/v2/music/netease")?;
    append_vkeys_auth(&mut url);
    url.query_pairs_mut()
        .append_pair("word", keyword)
        .append_pair("page", "1")
        .append_pair("num", &limit.clamp(1, 50).to_string());
    let response: VkeysResponse<VkeysNeteaseData> = get_vkeys_json(url.as_str()).await?;
    response.ensure_success("vkeys 网易云搜索")?;
    match response.data {
        Some(VkeysNeteaseData::List(list)) => Ok(list),
        Some(VkeysNeteaseData::Track(track)) => Ok(vec![*track]),
        None => Ok(Vec::new()),
    }
}

async fn get_vkeys_netease_track(id: &str, quality: &str) -> Result<MusicTrack, BotError> {
    let mut url = vkeys_url("/v2/music/netease")?;
    append_vkeys_auth(&mut url);
    url.query_pairs_mut()
        .append_pair("id", id)
        .append_pair("quality", &netease_vkeys_quality(quality).to_string());
    let response: VkeysResponse<VkeysNeteaseTrack> = get_vkeys_json(url.as_str()).await?;
    response.ensure_success("vkeys 网易云下载")?;
    let track = response
        .data
        .ok_or_else(|| BotError::Custom("vkeys 网易云没有返回歌曲数据".to_string()))?;
    let download_url = track
        .url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| BotError::Custom("vkeys 网易云没有返回可下载直链".to_string()))?
        .to_string();
    Ok(MusicTrack {
        id: json_id_to_string(track.id).if_empty(|| id.to_string()),
        platform: MusicPlatform::Netease,
        song: track.song.if_empty(|| "未知歌曲".to_string()),
        singer: track.singer.if_empty(|| "未知歌手".to_string()),
        album: track.album,
        cover: track.cover,
        link: track
            .link
            .if_empty(|| format!("https://music.163.com/#/song?id={id}")),
        url: download_url.clone(),
        headers: HashMap::new(),
        duration: parse_vkeys_duration(&track.interval),
        bitrate: parse_vkeys_kbps(&track.kbps),
        format: vkeys_track_format(&download_url, &track.quality, &track.size),
    })
}

async fn get_vkeys_netease_lyrics(id: &str) -> Result<Option<MusicLyrics>, BotError> {
    if !vkeys_enabled() {
        return Ok(None);
    }
    let mut url = vkeys_url("/v2/music/netease/lyric")?;
    append_vkeys_auth(&mut url);
    url.query_pairs_mut().append_pair("id", id);
    let response: VkeysResponse<VkeysNeteaseLyricsData> = get_vkeys_json(url.as_str()).await?;
    response.ensure_success("vkeys 网易云歌词")?;
    let Some(data) = response.data else {
        return Ok(None);
    };
    if data.lrc.trim().is_empty() && data.tlyric.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(MusicLyrics {
        plain: data.lrc,
        translation: data.tlyric,
    }))
}

async fn get_netease_song(id: &str) -> Result<NeteaseSong, BotError> {
    let url = format!(
        "https://music.163.com/api/song/detail?ids=[{}]",
        urlencoding::encode(id)
    );
    let response: NeteaseDetailResponse = get_netease_json(&url).await?;
    response
        .songs
        .into_iter()
        .next()
        .ok_or_else(|| BotError::Custom("没有找到歌曲详情".to_string()))
}

async fn get_netease_download_url(id: &str, quality: &str) -> Result<String, BotError> {
    let url = format!(
        "https://music.163.com/api/song/enhance/player/url?id={}&ids=[{}]&br={}",
        urlencoding::encode(id),
        urlencoding::encode(id),
        netease_native_bitrate(quality)
    );
    let response: NeteaseUrlResponse = get_netease_json(&url).await?;
    response
        .data
        .into_iter()
        .find_map(|item| item.url)
        .filter(|url| !url.trim().is_empty())
        .ok_or_else(|| BotError::Custom("没有拿到可下载的音乐链接".to_string()))
}

async fn get_netease_collection(
    collection: NeteaseCollectionId,
    limit: usize,
) -> Result<MusicCollection, BotError> {
    match collection.kind {
        NeteaseCollectionKind::Playlist => get_netease_playlist(&collection.id, limit).await,
        NeteaseCollectionKind::Album => get_netease_album_collection(&collection.id, limit).await,
    }
}

async fn get_netease_playlist(id: &str, limit: usize) -> Result<MusicCollection, BotError> {
    let url = format!(
        "https://music.163.com/api/v6/playlist/detail?id={}",
        urlencoding::encode(id)
    );
    let response: NeteasePlaylistResponse = get_netease_json(&url).await?;
    let playlist = response
        .playlist
        .ok_or_else(|| BotError::Custom("没有找到网易云歌单".to_string()))?;
    let playlist_id = json_id_to_string(playlist.id);
    let items = playlist
        .tracks
        .into_iter()
        .take(limit.clamp(1, 50))
        .map(netease_song_to_item)
        .filter(|item| !item.id.trim().is_empty())
        .collect();
    Ok(MusicCollection {
        platform: MusicPlatform::Netease,
        id: playlist_id,
        title: playlist.name,
        items,
    })
}

async fn get_netease_album_collection(id: &str, limit: usize) -> Result<MusicCollection, BotError> {
    let url = format!(
        "https://music.163.com/api/album/{}",
        urlencoding::encode(id)
    );
    let response: NeteaseAlbumResponse = get_netease_json(&url).await?;
    let album = response
        .album
        .ok_or_else(|| BotError::Custom("没有找到网易云专辑".to_string()))?;
    let artist = album
        .artist
        .as_ref()
        .map(|artist| artist.name.trim())
        .filter(|name| !name.is_empty());
    let title = artist
        .map(|artist| format!("{} - {}", album.name, artist))
        .unwrap_or(album.name);
    let items = response
        .songs
        .into_iter()
        .take(limit.clamp(1, 50))
        .map(netease_song_to_item)
        .filter(|item| !item.id.trim().is_empty())
        .collect();
    Ok(MusicCollection {
        platform: MusicPlatform::Netease,
        id: json_id_to_string(album.id),
        title,
        items,
    })
}

async fn get_netease_lyrics(id: &str) -> Result<NeteaseLyricResponse, BotError> {
    let url = format!(
        "https://music.163.com/api/song/lyric?id={}&lv=-1&kv=-1&tv=-1",
        urlencoding::encode(id)
    );
    get_netease_json(&url).await
}

async fn get_netease_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T, BotError> {
    get_json_with_headers(url, &netease_headers()).await
}

#[derive(Deserialize)]
struct VkeysResponse<T> {
    code: i64,
    #[serde(default)]
    message: String,
    data: Option<T>,
}

impl<T> VkeysResponse<T> {
    fn ensure_success(&self, context: &str) -> Result<(), BotError> {
        if self.code == 0 || self.code == 200 {
            Ok(())
        } else {
            Err(BotError::Custom(format!("{context}失败：{}", self.message)))
        }
    }
}

async fn get_vkeys_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T, BotError> {
    get_json_with_headers(url, &HashMap::new()).await
}

fn vkeys_enabled() -> bool {
    SETTINGS.music.vkeys.enabled && !SETTINGS.music.vkeys.base_url.trim().is_empty()
}

fn vkeys_url(path: &str) -> Result<url::Url, BotError> {
    let base = SETTINGS.music.vkeys.base_url.trim().trim_end_matches('/');
    url::Url::parse(&format!("{base}/{}", path.trim_start_matches('/')))
        .map_err(|err| BotError::Custom(format!("vkeys API 地址无效：{err}")))
}

fn append_vkeys_auth(url: &mut url::Url) {
    let token = SETTINGS.music.vkeys.token.trim();
    if !token.is_empty() {
        url.query_pairs_mut().append_pair("token", token);
    }
}

fn netease_vkeys_quality(quality: &str) -> i64 {
    match quality.trim().to_ascii_lowercase().as_str() {
        "standard" | "std" | "128" => 2,
        "lossless" | "flac" | "无损" => 5,
        _ => 4,
    }
}

fn netease_native_bitrate(quality: &str) -> i64 {
    match quality.trim().to_ascii_lowercase().as_str() {
        "standard" | "std" | "128" => 128_000,
        "lossless" | "flac" | "无损" => 999_000,
        "hires" | "hi-res" | "master" => 1_999_000,
        _ => 320_000,
    }
}

fn should_prefer_vkeys_netease_download(quality: &str) -> bool {
    matches!(
        quality.trim().to_ascii_lowercase().as_str(),
        "lossless" | "flac" | "无损" | "hires" | "hi-res" | "master"
    )
}

fn parse_vkeys_kbps(value: &str) -> Option<u32> {
    let numeric = value
        .trim()
        .trim_end_matches("kbps")
        .trim()
        .parse::<f32>()
        .ok()?;
    (numeric > 0.0).then(|| numeric.round() as u32)
}

fn parse_vkeys_duration(value: &str) -> Option<u32> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(seconds) = value.parse::<u32>() {
        return (seconds > 0).then_some(seconds);
    }
    let (minutes, rest) = value.split_once('分')?;
    let seconds = rest.trim_end_matches('秒');
    let total = minutes.trim().parse::<u32>().ok()? * 60 + seconds.trim().parse::<u32>().ok()?;
    (total > 0).then_some(total)
}

fn vkeys_track_format(url: &str, quality: &str, size: &str) -> Option<String> {
    let ext = url
        .split('?')
        .next()
        .and_then(|path| path.rsplit('.').next())
        .map(str::trim)
        .filter(|ext| ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric()))
        .map(|ext| ext.to_ascii_lowercase());
    if ext.is_some() {
        return ext;
    }
    if quality.contains("无损") || size.to_ascii_lowercase().contains("flac") {
        Some("flac".to_string())
    } else {
        Some("mp3".to_string())
    }
}

trait IfEmpty {
    fn if_empty<F>(self, fallback: F) -> String
    where
        F: FnOnce() -> String;
}

impl IfEmpty for String {
    fn if_empty<F>(self, fallback: F) -> String
    where
        F: FnOnce() -> String,
    {
        if self.trim().is_empty() {
            fallback()
        } else {
            self
        }
    }
}

fn netease_headers() -> HashMap<String, String> {
    let mut headers = HashMap::from([
        ("Referer".to_string(), "https://music.163.com/".to_string()),
        (
            "User-Agent".to_string(),
            "Mozilla/5.0 AppleWebKit/537.36 Chrome/125 Safari/537.36".to_string(),
        ),
    ]);
    if let Some(cookie) = netease_cookie() {
        headers.insert("Cookie".to_string(), cookie);
    }
    headers
}

fn netease_cookie() -> Option<String> {
    let configured_cookie = SETTINGS.music.netease.cookie.trim();
    if configured_cookie.is_empty() {
        std::env::var("NETEASE_MUSIC_COOKIE")
            .ok()
            .map(|cookie| cookie.trim().to_string())
            .filter(|cookie| !cookie.is_empty())
    } else {
        Some(configured_cookie.to_string())
    }
}

fn netease_song_to_item(song: NeteaseSong) -> MusicSearchItem {
    let singer = netease_artists(&song);
    let cover = song
        .album
        .as_ref()
        .and_then(|album| album.pic_url.clone())
        .unwrap_or_default();
    MusicSearchItem {
        platform: MusicPlatform::Netease,
        id: json_id_to_string(song.id),
        song: song.name,
        singer,
        cover,
    }
}

fn vkeys_netease_to_item(song: VkeysNeteaseTrack) -> MusicSearchItem {
    MusicSearchItem {
        platform: MusicPlatform::Netease,
        id: json_id_to_string(song.id),
        song: song.song.if_empty(|| "未知歌曲".to_string()),
        singer: song.singer.if_empty(|| "未知歌手".to_string()),
        cover: song.cover,
    }
}

fn vkeys_netease_to_items(songs: Vec<VkeysNeteaseTrack>, limit: usize) -> Vec<MusicSearchItem> {
    songs
        .into_iter()
        .take(limit)
        .map(vkeys_netease_to_item)
        .filter(|item| !item.id.is_empty())
        .collect()
}

fn netease_artists(song: &NeteaseSong) -> String {
    let artists = song
        .artists
        .iter()
        .map(|artist| artist.name.as_str())
        .filter(|name| !name.trim().is_empty())
        .collect::<Vec<_>>();
    if artists.is_empty() {
        "未知歌手".to_string()
    } else {
        artists.join(" / ")
    }
}

fn parse_netease_id(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit()) {
        return Some(trimmed.to_string());
    }

    let url = url::Url::parse(trimmed).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    if !host.contains("163.com") {
        return None;
    }
    netease_url_path_and_query(&url)
        .1
        .and_then(|query| query_id(&query))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NeteaseCollectionKind {
    Playlist,
    Album,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NeteaseCollectionId {
    kind: NeteaseCollectionKind,
    id: String,
}

fn parse_netease_collection_id(text: &str) -> Option<NeteaseCollectionId> {
    let trimmed = text.trim();
    if let Some(id) = trimmed.strip_prefix("album:")
        && is_netease_numeric_id(id)
    {
        return Some(NeteaseCollectionId {
            kind: NeteaseCollectionKind::Album,
            id: id.trim().to_string(),
        });
    }
    if let Some(id) = trimmed.strip_prefix("playlist:")
        && is_netease_numeric_id(id)
    {
        return Some(NeteaseCollectionId {
            kind: NeteaseCollectionKind::Playlist,
            id: id.trim().to_string(),
        });
    }
    let url = url::Url::parse(trimmed).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    if !host.contains("163.com") {
        return None;
    }
    let (path, query) = netease_url_path_and_query(&url);
    let id = query.and_then(|query| query_id(&query))?;
    if path.contains("album") {
        return Some(NeteaseCollectionId {
            kind: NeteaseCollectionKind::Album,
            id,
        });
    }
    if path.contains("playlist") {
        return Some(NeteaseCollectionId {
            kind: NeteaseCollectionKind::Playlist,
            id,
        });
    }
    None
}

fn netease_url_path_and_query(url: &url::Url) -> (String, Option<String>) {
    if let Some(fragment) = url
        .fragment()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let fragment = fragment.trim_start_matches('/');
        let (path, query) = fragment.split_once('?').unwrap_or((fragment, ""));
        return (
            path.to_ascii_lowercase(),
            (!query.trim().is_empty()).then(|| query.to_string()),
        );
    }
    (
        url.path().to_ascii_lowercase(),
        url.query().map(ToString::to_string),
    )
}

fn query_id(query: &str) -> Option<String> {
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(key, _)| key == "id")
        .map(|(_, value)| value.to_string())
        .filter(|id| is_netease_numeric_id(id))
}

fn is_netease_numeric_id(value: &str) -> bool {
    !value.trim().is_empty() && value.trim().chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_netease_collection_urls() {
        assert_eq!(
            parse_netease_collection_id("https://music.163.com/#/playlist?id=123456"),
            Some(NeteaseCollectionId {
                kind: NeteaseCollectionKind::Playlist,
                id: "123456".to_string(),
            })
        );
        assert_eq!(
            parse_netease_collection_id("https://music.163.com/#/album?id=654321"),
            Some(NeteaseCollectionId {
                kind: NeteaseCollectionKind::Album,
                id: "654321".to_string(),
            })
        );
        assert_eq!(
            parse_netease_collection_id("https://music.163.com/#/song?id=449818741"),
            None
        );
    }

    #[test]
    fn maps_vkeys_netease_quality_from_global_quality() {
        assert_eq!(netease_vkeys_quality("standard"), 2);
        assert_eq!(netease_vkeys_quality("high"), 4);
        assert_eq!(netease_vkeys_quality("lossless"), 5);
        assert_eq!(netease_native_bitrate("standard"), 128_000);
        assert_eq!(netease_native_bitrate("high"), 320_000);
        assert_eq!(netease_native_bitrate("lossless"), 999_000);
    }

    #[test]
    fn parses_vkeys_netease_media_metadata() {
        assert_eq!(parse_vkeys_duration("4分29秒"), Some(269));
        assert_eq!(parse_vkeys_duration("183"), Some(183));
        assert_eq!(parse_vkeys_kbps("3002kbps"), Some(3002));
    }

    #[test]
    fn only_lossless_netease_prefers_vkeys_without_cookie() {
        assert!(!should_prefer_vkeys_netease_download("standard"));
        assert!(!should_prefer_vkeys_netease_download("high"));
        assert!(should_prefer_vkeys_netease_download("lossless"));
        assert!(should_prefer_vkeys_netease_download("hires"));
    }
}
