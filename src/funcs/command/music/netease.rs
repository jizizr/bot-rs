use super::provider::{
    MusicPlatform, MusicProvider, MusicSearchItem, MusicTrack, get_json, json_id_to_string,
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

fn netease_song_to_item(song: NeteaseSong) -> MusicSearchItem {
    let singer = netease_artists(&song);
    MusicSearchItem {
        platform: MusicPlatform::Netease,
        id: json_id_to_string(song.id),
        song: song.name,
        singer,
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
    url.query_pairs()
        .find(|(key, _)| key == "id")
        .map(|(_, value)| value.to_string())
}
