use super::provider::{
    MusicCollection, MusicLyrics, MusicPlatform, MusicProvider, MusicSearchItem, MusicTrack,
    get_json, json_id_to_string,
};
use crate::BotError;
use async_trait::async_trait;
use serde::Deserialize;
use std::collections::HashMap;

pub static NETEASE_PROVIDER: NeteaseProvider = NeteaseProvider;

pub struct NeteaseProvider;

#[async_trait]
impl MusicProvider for NeteaseProvider {
    async fn search(&self, keyword: &str, limit: usize) -> Result<Vec<MusicSearchItem>, BotError> {
        let url = format!(
            "https://music.163.com/api/search/get/web?csrf_token=&s={}&type=1&offset=0&total=true&limit={limit}",
            urlencoding::encode(keyword)
        );
        let response: NeteaseSearchResponse = get_json(&url).await?;
        Ok(response
            .result
            .and_then(|result| result.songs)
            .unwrap_or_default()
            .into_iter()
            .map(netease_song_to_item)
            .filter(|item| !item.id.is_empty())
            .collect())
    }

    async fn resolve(
        &self,
        keyword: &str,
        selected_id: Option<&str>,
    ) -> Result<MusicTrack, BotError> {
        let id = match selected_id {
            Some(id) => id.to_string(),
            None => {
                if let Some(id) = parse_netease_id(keyword) {
                    id
                } else {
                    self.search(keyword, 1)
                        .await?
                        .into_iter()
                        .next()
                        .ok_or_else(|| BotError::Custom("没有找到可下载的音乐".to_string()))?
                        .id
                }
            }
        };
        let (song, download_url) =
            tokio::try_join!(get_netease_song(&id), get_netease_download_url(&id))?;
        let singer = netease_artists(&song);
        let album = song.album.unwrap_or_default();
        Ok(MusicTrack {
            id: id.clone(),
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
        let response = get_netease_lyrics(&id).await?;
        let plain = response.lrc.map(|lyric| lyric.lyric).unwrap_or_default();
        let translation = response.tlyric.map(|lyric| lyric.lyric).unwrap_or_default();
        if plain.trim().is_empty() && translation.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(MusicLyrics { plain, translation }))
    }
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

async fn get_netease_song(id: &str) -> Result<NeteaseSong, BotError> {
    let url = format!(
        "https://music.163.com/api/song/detail?ids=[{}]",
        urlencoding::encode(id)
    );
    let response: NeteaseDetailResponse = get_json(&url).await?;
    response
        .songs
        .into_iter()
        .next()
        .ok_or_else(|| BotError::Custom("没有找到歌曲详情".to_string()))
}

async fn get_netease_download_url(id: &str) -> Result<String, BotError> {
    let url = format!(
        "https://music.163.com/api/song/enhance/player/url?id={}&ids=[{}]&br=320000",
        urlencoding::encode(id),
        urlencoding::encode(id)
    );
    let response: NeteaseUrlResponse = get_json(&url).await?;
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
    let response: NeteasePlaylistResponse = get_json(&url).await?;
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
    let response: NeteaseAlbumResponse = get_json(&url).await?;
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
    get_json(&url).await
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
}
