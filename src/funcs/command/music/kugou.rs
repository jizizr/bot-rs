use super::provider::{
    MusicPlatform, MusicProvider, MusicSearchItem, MusicTrack, get_json, json_id_to_string,
};
use crate::BotError;
use async_trait::async_trait;
use serde::Deserialize;
use std::collections::HashMap;

pub static KUGOU_PROVIDER: KugouProvider = KugouProvider;

pub struct KugouProvider;

#[async_trait]
impl MusicProvider for KugouProvider {
    async fn search(&self, keyword: &str, limit: usize) -> Result<Vec<MusicSearchItem>, BotError> {
        let url = kugou_search_url(keyword, limit);
        let response: KugouSearchResponse = get_json(&url).await?;
        Ok(response
            .data
            .map(|data| data.lists)
            .unwrap_or_default()
            .into_iter()
            .take(limit)
            .filter_map(|item| {
                let hash = first_non_empty([
                    item.file_hash.as_deref(),
                    item.hq_file_hash.as_deref(),
                    item.sq_file_hash.as_deref(),
                ])?;
                Some(MusicSearchItem {
                    platform: MusicPlatform::Kugou,
                    id: encode_kugou_id(hash, item.album_id_string().as_deref()),
                    song: item.song_name,
                    singer: item.singer_name.unwrap_or_else(|| "未知歌手".to_string()),
                    cover: String::new(),
                })
            })
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
                if is_kugou_encoded_id(keyword) || is_hash(keyword) {
                    keyword.to_string()
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
        let (hash, album_id) = decode_kugou_id(&id);
        let info = get_kugou_play_info(&hash, album_id.as_deref()).await?;
        let url = first_non_empty([
            info.url.as_deref(),
            info.play_url.as_deref(),
            info.backup_url.as_ref().and_then(|backup| {
                backup
                    .values()
                    .find_map(|urls| urls.first().map(String::as_str))
            }),
        ])
        .ok_or_else(|| {
            BotError::Custom("酷狗没有返回可下载直链，可能需要登录或该歌曲不可试听".to_string())
        })?
        .to_string();
        let song = first_non_empty([info.song_name.as_deref(), info.file_name.as_deref()])
            .unwrap_or("未知歌曲")
            .to_string();
        let singer = info.author_name.unwrap_or_else(|| {
            split_kugou_file_name(&song)
                .0
                .unwrap_or_else(|| "未知歌手".to_string())
        });
        let song = split_kugou_file_name(&song).1.unwrap_or(song);
        Ok(MusicTrack {
            id,
            platform: MusicPlatform::Kugou,
            song,
            singer,
            album: String::new(),
            cover: normalize_kugou_cover(
                first_non_empty([info.album_img.as_deref(), info.img_url.as_deref()])
                    .unwrap_or_default(),
            ),
            link: format!("https://www.kugou.com/song/#hash={hash}"),
            url,
            headers: HashMap::new(),
            duration: None,
            bitrate: None,
            format: None,
        })
    }
}

#[derive(Deserialize)]
struct KugouSearchResponse {
    data: Option<KugouSearchData>,
}

#[derive(Deserialize)]
struct KugouSearchData {
    #[serde(default)]
    lists: Vec<KugouSearchItem>,
}

#[derive(Deserialize)]
struct KugouSearchItem {
    #[serde(rename = "FileHash")]
    file_hash: Option<String>,
    #[serde(rename = "HQFileHash")]
    hq_file_hash: Option<String>,
    #[serde(rename = "SQFileHash")]
    sq_file_hash: Option<String>,
    #[serde(rename = "SongName")]
    song_name: String,
    #[serde(rename = "SingerName")]
    singer_name: Option<String>,
    #[serde(rename = "AlbumID")]
    album_id: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct KugouPlayInfo {
    url: Option<String>,
    play_url: Option<String>,
    backup_url: Option<HashMap<String, Vec<String>>>,
    #[serde(rename = "songName")]
    song_name: Option<String>,
    #[serde(rename = "fileName")]
    file_name: Option<String>,
    author_name: Option<String>,
    #[serde(rename = "imgUrl")]
    img_url: Option<String>,
    album_img: Option<String>,
}

async fn get_kugou_play_info(
    hash: &str,
    album_id: Option<&str>,
) -> Result<KugouPlayInfo, BotError> {
    let mut url = format!(
        "https://m.kugou.com/app/i/getSongInfo.php?cmd=playInfo&hash={}",
        urlencoding::encode(hash)
    );
    if let Some(album_id) = album_id.filter(|id| !id.trim().is_empty()) {
        url.push_str("&album_id=");
        url.push_str(&urlencoding::encode(album_id));
    }
    get_json(&url).await
}

fn kugou_search_url(keyword: &str, limit: usize) -> String {
    format!(
        "http://songsearch.kugou.com/song_search_v2?keyword={}&platform=WebFilter&format=json&page=1&pagesize={}",
        urlencoding::encode(keyword),
        limit.max(1)
    )
}

fn encode_kugou_id(hash: &str, album_id: Option<&str>) -> String {
    let hash = hash.trim().to_ascii_uppercase();
    match album_id.and_then(|id| {
        let id = id.trim();
        (!id.is_empty()).then_some(id)
    }) {
        Some(album_id) => format!("{hash}:{album_id}"),
        None => hash,
    }
}

fn decode_kugou_id(id: &str) -> (String, Option<String>) {
    let mut parts = id.splitn(2, ':');
    let hash = parts.next().unwrap_or_default().trim().to_ascii_uppercase();
    let album_id = parts
        .next()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned);
    (hash, album_id)
}

fn is_kugou_encoded_id(text: &str) -> bool {
    let (hash, _) = decode_kugou_id(text);
    is_hash(&hash)
}

fn is_hash(text: &str) -> bool {
    let text = text.trim();
    text.len() == 32 && text.chars().all(|c| c.is_ascii_hexdigit())
}

fn first_non_empty<'a>(items: impl IntoIterator<Item = Option<&'a str>>) -> Option<&'a str> {
    items
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|item| !item.is_empty())
}

fn normalize_kugou_cover(url: &str) -> String {
    url.replace("{size}", "400")
}

fn split_kugou_file_name(file_name: &str) -> (Option<String>, Option<String>) {
    file_name
        .split_once(" - ")
        .map(|(singer, song)| (Some(singer.to_string()), Some(song.to_string())))
        .unwrap_or((None, None))
}

fn album_id_to_string(value: Option<serde_json::Value>) -> Option<String> {
    value.map(json_id_to_string).filter(|id| !id.is_empty())
}

impl KugouSearchItem {
    fn album_id_string(&self) -> Option<String> {
        album_id_to_string(self.album_id.clone())
    }
}
