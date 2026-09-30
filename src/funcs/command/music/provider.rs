use crate::{BotError, settings::SETTINGS};
use async_trait::async_trait;
use lazy_static::lazy_static;
use reqwest::{
    Client,
    header::{CONTENT_RANGE, RANGE},
};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};
use serde::{Deserialize, de::DeserializeOwned};
use std::{collections::HashMap, fmt, time::Duration};

/// 单次音频下载的硬上限（Telegram 本地 Bot API 上传上限即 2 GiB）：
/// 防止异常服务端让流式/分段下载无界增长耗尽内存
const MAX_AUDIO_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;

lazy_static! {
    // read_timeout (per-read) instead of a total timeout so large audio downloads are not capped.
    pub(crate) static ref CLIENT: ClientWithMiddleware = {
        let retry_policy = ExponentialBackoff::builder().build_with_max_retries(2);
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(30))
            .build()
            .expect("music http client");
        ClientBuilder::new(client)
            .with(RetryTransientMiddleware::new_with_policy(retry_policy))
            .build()
    };
}

#[async_trait]
pub trait MusicProvider: Sync {
    async fn search(&self, keyword: &str, limit: usize) -> Result<Vec<MusicSearchItem>, BotError>;

    async fn collection(
        &self,
        _keyword: &str,
        _limit: usize,
    ) -> Result<Option<MusicCollection>, BotError> {
        Ok(None)
    }

    async fn lyrics(&self, _track_id: &str) -> Result<Option<MusicLyrics>, BotError> {
        Ok(None)
    }

    async fn resolve(
        &self,
        keyword: &str,
        selected_id: Option<&str>,
    ) -> Result<MusicTrack, BotError>;
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum MusicPlatform {
    AppleMusic,
    Bilibili,
    Kugou,
    Netease,
    Soda,
    #[default]
    Tencent,
    YouTubeMusic,
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
            MusicPlatform::YouTubeMusic => "youtubemusic",
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
            MusicPlatform::YouTubeMusic => "YouTube Music",
        }
    }

    /// Whether the platform can serve lyrics at all: Bilibili and Kugou never
    /// override the default `lyrics()` (which returns `Ok(None)`), so showing
    /// them a lyrics button would always dead-end in an alert. Keep this in
    /// sync if a provider gains a `lyrics()` implementation.
    pub fn supports_lyrics(self) -> bool {
        !matches!(self, MusicPlatform::Bilibili | MusicPlatform::Kugou)
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
            "yt" | "ytm" | "youtube" | "youtubemusic" | "youtube-music" | "youtube_music" => {
                Some(MusicPlatform::YouTubeMusic)
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
            MusicPlatform::YouTubeMusic => "y",
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
            "y" => Some(MusicPlatform::YouTubeMusic),
            _ => None,
        }
    }

    pub fn from_stored_default(value: &str) -> Self {
        Self::from_alias(value).unwrap_or_default()
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
    pub cover: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicCollection {
    pub platform: MusicPlatform,
    pub id: String,
    pub title: String,
    pub items: Vec<MusicSearchItem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicLyrics {
    pub plain: String,
    pub translation: String,
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
    pub max_chunk_size: Option<u64>,
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
    pub decrypt_elapsed: Option<Duration>,
}

struct MusicAudio {
    bytes: Vec<u8>,
    decrypt_elapsed: Option<Duration>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DownloadProgress {
    pub written: u64,
    pub total: Option<u64>,
}

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
pub async fn download_track_media(track: &MusicTrack) -> Result<MusicMedia, BotError> {
    download_track_media_with_cover(track, true).await
}

#[cfg(test)]
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
        download_audio_with_headers_progress(
            &track.url,
            &track.headers,
            track.max_chunk_size,
            progress
        ),
        cover_download,
    );
    let audio = audio?;
    Ok(MusicMedia {
        audio: audio.bytes,
        cover,
        decrypt_elapsed: audio.decrypt_elapsed,
    })
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
    download_audio_with_headers_progress(url, headers, None, progress)
        .await
        .map(|audio| audio.bytes)
}

async fn download_audio_with_headers_progress<F>(
    url: &str,
    headers: &HashMap<String, String>,
    max_chunk_size: Option<u64>,
    progress: &mut F,
) -> Result<MusicAudio, BotError>
where
    F: FnMut(DownloadProgress) + Send,
{
    if url.starts_with("applemusic-wrapper://") || url.starts_with("applemusic-widevine://") {
        let (bytes, decrypt_elapsed) =
            super::applemusic::download_internal_url_with_progress_stats(url, progress).await?;
        return Ok(MusicAudio {
            bytes,
            decrypt_elapsed,
        });
    }
    if url.starts_with("soda-download://") {
        return Ok(MusicAudio {
            bytes: super::soda::download_internal_url_with_progress(url, progress).await?,
            decrypt_elapsed: None,
        });
    }
    if let Some(max_chunk_size) = max_chunk_size.filter(|size| *size > 0) {
        return Ok(MusicAudio {
            bytes: download_bounded_range_with_headers_progress(
                url,
                headers,
                max_chunk_size,
                progress,
            )
            .await?,
            decrypt_elapsed: None,
        });
    }

    let mut request = CLIENT.get(url);
    for (key, value) in headers {
        request = request.header(key, value);
    }
    let mut resp = request.send().await?;
    if resp.status() == reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(BotError::Custom(format!(
            "下载失败：未请求分段却收到 206 Partial Content {url}"
        )));
    }
    if !resp.status().is_success() {
        return Err(BotError::Custom(format!(
            "下载失败：HTTP {} {}",
            resp.status(),
            url
        )));
    }

    let mut buf = Vec::new();
    let total = resp.content_length();
    // 声明长度已超上限：不可能上传，直接拒绝，不必边下边等撞缓冲上限
    if total.is_some_and(|total| total > MAX_AUDIO_DOWNLOAD_BYTES) {
        return Err(BotError::Custom(oversize_message(url)));
    }
    while let Some(chunk) = resp.chunk().await? {
        buf.extend_from_slice(&chunk);
        if buf.len() as u64 > MAX_AUDIO_DOWNLOAD_BYTES {
            return Err(BotError::Custom(oversize_message(url)));
        }
        progress(DownloadProgress {
            written: buf.len() as u64,
            total,
        });
    }
    if buf.is_empty() {
        return Err(BotError::Custom("下载失败：文件为空".to_string()));
    }
    Ok(MusicAudio {
        bytes: buf,
        decrypt_elapsed: None,
    })
}

async fn download_bounded_range_with_headers_progress<F>(
    url: &str,
    headers: &HashMap<String, String>,
    max_chunk_size: u64,
    progress: &mut F,
) -> Result<Vec<u8>, BotError>
where
    F: FnMut(DownloadProgress) + Send,
{
    let mut buf = Vec::new();
    let mut offset = 0u64;
    let mut total = None;
    loop {
        let end = offset.saturating_add(max_chunk_size).saturating_sub(1);
        let mut request = CLIENT
            .get(url)
            .header(RANGE, format!("bytes={offset}-{end}"));
        for (key, value) in headers {
            request = request.header(key, value);
        }
        let resp = request.send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE && !buf.is_empty() {
            // 416 只说明起点越界，不代表下载完整：必须核对完整长度，
            // 否则服务端提前 416 会静默产出截断文件
            let reported = resp
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(parse_unsatisfied_range_total);
            // 已由合法 206 校验过的完整长度优先于 416 自报的长度：
            // 否则服务端用一个更小的 */total 就能把截断文件伪装成完整文件
            let complete_total = match (total, reported) {
                (Some(known), Some(reported)) if known != reported => {
                    return Err(BotError::Custom(format!(
                        "下载失败：416 声明完整长度 {reported}，与此前的 {known} 不一致 {url}"
                    )));
                }
                (Some(known), _) => Some(known),
                (None, reported) => reported,
            };
            return match complete_total {
                Some(expected) if buf.len() as u64 == expected => Ok(buf),
                Some(expected) => Err(BotError::Custom(format!(
                    "下载失败：文件不完整（已收到 {} / 共 {expected} 字节）{url}",
                    buf.len()
                ))),
                None => Err(BotError::Custom(format!(
                    "下载失败：服务端提前返回 416 且无法校验完整长度 {url}"
                ))),
            };
        }
        if !status.is_success() {
            return Err(BotError::Custom(format!("下载失败：HTTP {status} {url}")));
        }

        let content_range = resp
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_content_range);

        // 200 表示服务端忽略了 Range、返回的是完整对象：正文本身就是完整文件，
        // 绝不能拼接到已有前缀之后（首段 206 后中途改回 200 会产出损坏文件）
        if status == reqwest::StatusCode::OK {
            let body =
                read_body_capped(resp, MAX_AUDIO_DOWNLOAD_BYTES, &oversize_message(url)).await?;
            if body.is_empty() {
                return Err(BotError::Custom("下载失败：文件为空".to_string()));
            }
            progress(DownloadProgress {
                written: body.len() as u64,
                total: Some(body.len() as u64),
            });
            return Ok(body);
        }

        // 206 必须携带合法的单区间 Content-Range（RFC 9110）：缺失或无法解析时
        // 既无法验证推进也无法核对完整性，接受它就是静默截断路径
        if status != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(BotError::Custom(format!(
                "下载失败：Range 请求收到意外状态 HTTP {status} {url}"
            )));
        }
        let Some((range_start, range_end, range_total)) = content_range else {
            return Err(BotError::Custom(format!(
                "下载失败：206 响应缺少合法 Content-Range {url}"
            )));
        };
        // 服务端返回的区间必须从当前 offset 开始且向前推进；否则（重复旧区间、
        // 倒退区间）继续拼接会生成损坏文件，total 为 * 时甚至会无限循环。
        // range_end == u64::MAX 会让区间长度/下一起点溢出（debug panic、
        // release 下 offset 饱和不再推进），一并用 checked 算术拒绝
        let (Some(declared), Some(next_offset)) = (
            range_end
                .checked_sub(range_start)
                .and_then(|len| len.checked_add(1)),
            range_end.checked_add(1),
        ) else {
            return Err(BotError::Custom(format!(
                "下载失败：服务端返回异常区间 {range_start}-{range_end} {url}"
            )));
        };
        if range_start != offset || range_end < range_start {
            return Err(BotError::Custom(format!(
                "下载失败：服务端返回异常区间 {range_start}-{range_end}（期望起点 {offset}）{url}"
            )));
        }
        // 跨响应的完整长度必须一致，否则拼出的文件来自两个不同版本
        if let (Some(known), Some(reported_total)) = (total, range_total)
            && known != reported_total
        {
            return Err(BotError::Custom(format!(
                "下载失败：服务端声明的完整长度前后不一致（{known} -> {reported_total}）{url}"
            )));
        }
        // 一旦见过具体 total 就保留：后续响应降级为 * 不应丢失已知完整长度
        total = range_total.or(total);
        // 只要完整长度已知，每个分段（包括自报 * 的）都必须落在边界内：
        // bytes 0-4/3 自相矛盾，0-4/10 之后的 5-14/* 越界拼接，
        // 都会让 buf.len() >= total 把错误数据判定为成功
        if let Some(known) = total
            && range_end >= known
        {
            return Err(BotError::Custom(format!(
                "下载失败：区间 {range_start}-{range_end} 超出声明的完整长度 {known} {url}"
            )));
        }
        // 完整长度已确定超过上限：不可能上传，首段就拒绝，
        // 不必占着下载槽和内存把 2 GiB 缓冲填满才失败
        if total.is_some_and(|known| known > MAX_AUDIO_DOWNLOAD_BYTES) {
            return Err(BotError::Custom(oversize_message(url)));
        }
        // 读取正文前先算内存预算，避免超大响应整块读入后才发现超限
        if buf.len() as u64 + declared > MAX_AUDIO_DOWNLOAD_BYTES {
            return Err(BotError::Custom(oversize_message(url)));
        }
        // 响应体长度必须与 Content-Range 声明一致，否则按声明推进 offset 会跳过数据
        let length_mismatch = format!("下载失败：响应体长度与声明区间长度 {declared} 不一致 {url}");
        let chunk = read_body_capped(resp, declared, &length_mismatch).await?;
        if chunk.len() as u64 != declared {
            return Err(BotError::Custom(length_mismatch));
        }
        offset = next_offset;
        buf.extend_from_slice(&chunk);
        progress(DownloadProgress {
            written: buf.len() as u64,
            total,
        });

        // 已知完整长度且已收满时结束；total 为 * 时继续请求，
        // 由（经过完整性校验的）416 分支判定结束
        if total
            .map(|total| total > 0 && buf.len() as u64 >= total)
            .unwrap_or(false)
        {
            break;
        }
    }

    if buf.is_empty() {
        return Err(BotError::Custom("下载失败：文件为空".to_string()));
    }
    Ok(buf)
}

fn oversize_message(url: &str) -> String {
    format!("下载失败：文件超过大小上限 {MAX_AUDIO_DOWNLOAD_BYTES} 字节 {url}")
}

/// 流式读取响应体并施加上限：先用 Content-Length 快速拒绝，再逐块累加校验。
/// 不能用 `Response::bytes()` —— 那会在任何检查之前把整个响应读进内存。
async fn read_body_capped(
    mut resp: reqwest::Response,
    limit: u64,
    over_limit: &str,
) -> Result<Vec<u8>, BotError> {
    if resp.content_length().is_some_and(|len| len > limit) {
        return Err(BotError::Custom(over_limit.to_string()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if body.len() as u64 + chunk.len() as u64 > limit {
            return Err(BotError::Custom(over_limit.to_string()));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// 解析 416 响应的 `Content-Range: bytes */total`，返回完整长度。
fn parse_unsatisfied_range_total(value: &str) -> Option<u64> {
    let value = value.trim().strip_prefix("bytes ")?;
    let (range, total) = value.split_once('/')?;
    if range.trim() != "*" {
        return None;
    }
    total.trim().parse().ok()
}

fn parse_content_range(value: &str) -> Option<(u64, u64, Option<u64>)> {
    let value = value.trim().strip_prefix("bytes ")?;
    let (range, total) = value.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let start = start.trim().parse().ok()?;
    let end = end.trim().parse().ok()?;
    let total = match total.trim() {
        "*" => None,
        value => value.parse().ok(),
    };
    Some((start, end, total))
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

pub(crate) fn first_non_empty<'a>(
    items: impl IntoIterator<Item = Option<&'a str>>,
) -> Option<&'a str> {
    items
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|item| !item.is_empty())
}

#[derive(Deserialize)]
pub(crate) struct VkeysResponse<T> {
    pub(crate) code: i64,
    #[serde(default)]
    pub(crate) message: String,
    pub(crate) data: Option<T>,
}

impl<T> VkeysResponse<T> {
    pub(crate) fn ensure_success(&self, context: &str) -> Result<(), BotError> {
        if self.code == 0 || self.code == 200 {
            Ok(())
        } else {
            Err(BotError::Custom(format!("{context}失败：{}", self.message)))
        }
    }
}

pub(crate) fn vkeys_enabled() -> bool {
    SETTINGS.music.vkeys.enabled && !SETTINGS.music.vkeys.base_url.trim().is_empty()
}

pub(crate) fn vkeys_url(path: &str) -> Result<url::Url, BotError> {
    let base = SETTINGS.music.vkeys.base_url.trim().trim_end_matches('/');
    url::Url::parse(&format!("{base}/{}", path.trim_start_matches('/')))
        .map_err(|err| BotError::Custom(format!("vkeys API 地址无效：{err}")))
}

pub(crate) fn append_vkeys_auth(url: &mut url::Url) {
    let token = SETTINGS.music.vkeys.token.trim();
    if !token.is_empty() {
        url.query_pairs_mut().append_pair("token", token);
    }
}

pub(crate) fn parse_vkeys_kbps(value: &str) -> Option<u32> {
    let numeric = value
        .trim()
        .trim_end_matches("kbps")
        .trim()
        .parse::<f32>()
        .ok()?;
    (numeric > 0.0).then(|| numeric.round() as u32)
}

pub(crate) fn parse_vkeys_duration(value: &str) -> Option<u32> {
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

pub(crate) fn vkeys_track_format(url: &str, quality: &str, size: &str) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supports_lyrics_matches_provider_overrides() {
        // Only Bilibili and Kugou keep the default lyrics() -> Ok(None).
        assert!(!MusicPlatform::Bilibili.supports_lyrics());
        assert!(!MusicPlatform::Kugou.supports_lyrics());
        assert!(MusicPlatform::AppleMusic.supports_lyrics());
        assert!(MusicPlatform::Netease.supports_lyrics());
        assert!(MusicPlatform::Soda.supports_lyrics());
        assert!(MusicPlatform::Tencent.supports_lyrics());
        assert!(MusicPlatform::YouTubeMusic.supports_lyrics());
    }
}
