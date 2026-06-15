use crate::BotError;
use async_trait::async_trait;
use lazy_static::lazy_static;
use reqwest::Client;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};
use serde::de::DeserializeOwned;
use std::{collections::HashMap, fmt};

lazy_static! {
    static ref CLIENT: ClientWithMiddleware = {
        let retry_policy = ExponentialBackoff::builder().build_with_max_retries(2);
        ClientBuilder::new(Client::new())
            .with(RetryTransientMiddleware::new_with_policy(retry_policy))
            .build()
    };
}

#[async_trait]
pub trait MusicProvider: Sync {
    async fn search(&self, keyword: &str, limit: usize) -> Result<Vec<MusicSearchItem>, BotError>;

    async fn resolve(
        &self,
        keyword: &str,
        selected_id: Option<&str>,
    ) -> Result<MusicTrack, BotError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum MusicPlatform {
    AppleMusic,
    Bilibili,
    Kugou,
    Netease,
    Soda,
    Tencent,
}

impl MusicPlatform {
    pub fn id(self) -> &'static str {
        match self {
            MusicPlatform::AppleMusic => "applemusic",
            MusicPlatform::Bilibili => "bilibili",
            MusicPlatform::Kugou => "kugou",
            MusicPlatform::Netease => "netease",
            MusicPlatform::Soda => "soda",
            MusicPlatform::Tencent => "tencent",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MusicPlatform::AppleMusic => "Apple Music",
            MusicPlatform::Bilibili => "Bilibili",
            MusicPlatform::Kugou => "酷狗音乐",
            MusicPlatform::Netease => "网易云音乐",
            MusicPlatform::Soda => "汽水音乐",
            MusicPlatform::Tencent => "QQ音乐",
        }
    }

    pub fn from_alias(alias: &str) -> Option<Self> {
        match alias.trim().to_ascii_lowercase().as_str() {
            "am" | "apple" | "applemusic" | "apple-music" | "apple_music" | "苹果音乐" => {
                Some(MusicPlatform::AppleMusic)
            }
            "b" | "bili" | "bilibili" | "哔哩哔哩" | "b站" => Some(MusicPlatform::Bilibili),
            "kg" | "kugou" | "酷狗" | "酷狗音乐" => Some(MusicPlatform::Kugou),
            "163" | "netease" | "wyy" | "网易" | "网易云" | "网易云音乐" => {
                Some(MusicPlatform::Netease)
            }
            "qs" | "soda" | "qishui" | "汽水" | "汽水音乐" => Some(MusicPlatform::Soda),
            "qq" | "qqmusic" | "tencent" | "tx" | "qq音乐" | "腾讯" | "腾讯音乐" => {
                Some(MusicPlatform::Tencent)
            }
            _ => None,
        }
    }

    pub fn callback_code(self) -> &'static str {
        match self {
            MusicPlatform::AppleMusic => "a",
            MusicPlatform::Bilibili => "b",
            MusicPlatform::Kugou => "k",
            MusicPlatform::Netease => "n",
            MusicPlatform::Soda => "s",
            MusicPlatform::Tencent => "t",
        }
    }

    pub fn from_callback_code(code: &str) -> Option<Self> {
        match code {
            "a" => Some(MusicPlatform::AppleMusic),
            "b" => Some(MusicPlatform::Bilibili),
            "k" => Some(MusicPlatform::Kugou),
            "n" => Some(MusicPlatform::Netease),
            "s" => Some(MusicPlatform::Soda),
            "t" => Some(MusicPlatform::Tencent),
            _ => None,
        }
    }

    pub fn from_stored_default(value: &str) -> Self {
        Self::from_alias(value).unwrap_or_default()
    }
}

impl Default for MusicPlatform {
    fn default() -> Self {
        Self::Tencent
    }
}

impl fmt::Display for MusicPlatform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicQuery {
    pub platform: MusicPlatform,
    pub keyword: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicSearchItem {
    pub platform: MusicPlatform,
    pub id: String,
    pub song: String,
    pub singer: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicTrack {
    pub id: String,
    pub platform: MusicPlatform,
    pub song: String,
    pub singer: String,
    pub album: String,
    pub cover: String,
    pub link: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub duration: Option<u32>,
    pub bitrate: Option<u32>,
    pub format: Option<String>,
}

impl MusicTrack {
    pub fn file_name(&self) -> String {
        let ext = self.file_extension();
        format!("{} - {}.{}", self.song, self.singer, ext)
    }

    pub fn file_extension(&self) -> String {
        if let Some(format) = self
            .format
            .as_deref()
            .map(str::trim)
            .filter(|format| !format.is_empty())
        {
            return format.trim_start_matches('.').to_ascii_lowercase();
        }
        if self.url.starts_with("applemusic-wrapper://") {
            return "m4a".to_string();
        }
        self.url
            .split('?')
            .next()
            .and_then(|path| path.rsplit('.').next())
            .filter(|ext| ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric()))
            .unwrap_or("mp3")
            .to_ascii_lowercase()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicMedia {
    pub audio: Vec<u8>,
    pub cover: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DownloadProgress {
    pub written: u64,
    pub total: Option<u64>,
}

pub fn build_query(platform_alias: &str, keyword_parts: &[String]) -> Result<MusicQuery, BotError> {
    let platform = MusicPlatform::from_alias(platform_alias)
        .ok_or_else(|| BotError::Custom(format!("暂不支持音乐平台：{platform_alias}")))?;
    build_query_with_platform(platform, keyword_parts)
}

pub fn build_query_with_platform(
    platform: MusicPlatform,
    keyword_parts: &[String],
) -> Result<MusicQuery, BotError> {
    let keyword = keyword_parts.join(" ").trim().to_string();
    if keyword.is_empty() {
        return Err(BotError::Custom(
            "请输入歌曲名、链接或 ID，例如：/music qq 晴天".to_string(),
        ));
    }

    Ok(MusicQuery { platform, keyword })
}

pub fn build_legacy_query(args: &[String]) -> Result<MusicQuery, BotError> {
    build_legacy_query_with_default(MusicPlatform::default(), args)
}

pub fn build_legacy_query_with_default(
    default_platform: MusicPlatform,
    args: &[String],
) -> Result<MusicQuery, BotError> {
    if let Some((first, rest)) = args.split_first()
        && let Some(platform) = MusicPlatform::from_alias(first.trim_start_matches('#'))
    {
        return build_query_with_platform(platform, rest);
    }
    build_query_with_platform(default_platform, args)
}

pub async fn download_track_media(track: &MusicTrack) -> Result<MusicMedia, BotError> {
    download_track_media_with_cover(track, true).await
}

pub async fn download_track_media_with_cover(
    track: &MusicTrack,
    include_cover: bool,
) -> Result<MusicMedia, BotError> {
    let mut progress = |_| {};
    download_track_media_with_cover_progress(track, include_cover, &mut progress).await
}

pub async fn download_track_media_with_cover_progress<F>(
    track: &MusicTrack,
    include_cover: bool,
    progress: &mut F,
) -> Result<MusicMedia, BotError>
where
    F: FnMut(DownloadProgress) + Send,
{
    let cover_url = if include_cover && !track.cover.trim().is_empty() {
        Some(track.cover.clone())
    } else {
        None
    };
    let cover_download = async move {
        match cover_url {
            Some(url) => download_url(&url).await.unwrap_or_default(),
            None => Vec::new(),
        }
    };
    let (audio, cover) = tokio::join!(
        download_url_with_headers_progress(&track.url, &track.headers, progress),
        cover_download,
    );
    let audio = audio?;
    Ok(MusicMedia { audio, cover })
}

pub async fn download_url(url: &str) -> Result<Vec<u8>, BotError> {
    download_url_with_headers(url, &HashMap::new()).await
}

pub async fn download_url_with_headers(
    url: &str,
    headers: &HashMap<String, String>,
) -> Result<Vec<u8>, BotError> {
    let mut progress = |_| {};
    download_url_with_headers_progress(url, headers, &mut progress).await
}

pub async fn download_url_with_headers_progress<F>(
    url: &str,
    headers: &HashMap<String, String>,
    progress: &mut F,
) -> Result<Vec<u8>, BotError>
where
    F: FnMut(DownloadProgress) + Send,
{
    if url.starts_with("applemusic-wrapper://") || url.starts_with("applemusic-widevine://") {
        return super::applemusic::download_internal_url_with_progress(url, progress).await;
    }
    if url.starts_with("soda-download://") {
        return super::soda::download_internal_url_with_progress(url, progress).await;
    }

    let mut request = CLIENT.get(url);
    for (key, value) in headers {
        request = request.header(key, value);
    }
    let mut resp = request.send().await?;
    if !resp.status().is_success() {
        return Err(BotError::Custom(format!(
            "下载失败：HTTP {} {}",
            resp.status(),
            url
        )));
    }

    let mut buf = Vec::new();
    let total = resp.content_length();
    while let Some(chunk) = resp.chunk().await? {
        buf.extend_from_slice(&chunk);
        progress(DownloadProgress {
            written: buf.len() as u64,
            total,
        });
    }
    if buf.is_empty() {
        return Err(BotError::Custom("下载失败：文件为空".to_string()));
    }
    Ok(buf)
}

pub(crate) async fn get_json<T: DeserializeOwned>(url: &str) -> Result<T, BotError> {
    get_json_with_headers(url, &HashMap::new()).await
}

pub(crate) async fn get_json_with_headers<T: DeserializeOwned>(
    url: &str,
    headers: &HashMap<String, String>,
) -> Result<T, BotError> {
    let mut request = CLIENT.get(url);
    for (key, value) in headers {
        request = request.header(key, value);
    }
    let resp = request.send().await?;
    if !resp.status().is_success() {
        return Err(BotError::Custom(format!(
            "音乐接口请求失败：HTTP {}",
            resp.status()
        )));
    }
    Ok(resp.json().await?)
}

pub(crate) fn json_id_to_string(value: serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s,
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}
