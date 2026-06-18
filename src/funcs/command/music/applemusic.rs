use super::provider::{
    DownloadProgress, MusicPlatform, MusicProvider, MusicSearchItem, MusicTrack,
};
use crate::{BotError, settings::SETTINGS};
use aes::cipher::{KeyIvInit, StreamCipher};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose};
use lazy_static::lazy_static;
use oxideav_mp4::cenc::{SencBox, SubsampleEntry, TencBox, parse_senc, parse_tenc};
use protobuf::Message;
use regex::Regex;
use reqwest::Client;
use rsa::{RsaPrivateKey, pkcs1::DecodeRsaPrivateKey};
use serde::Deserialize;
use std::{
    collections::HashMap,
    fs,
    io::Cursor,
    ops::Range,
    sync::RwLock,
    time::{Duration, Instant},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use url::Url;
use widevine::{
    Cdm, Device, KeyType, LicenseType, Pssh,
    device::{DeviceType, SecurityLevel},
};
use widevine_proto::license_protocol::{WidevinePsshData, widevine_pssh_data};

const APPLE_MUSIC_API: &str = "https://amp-api.music.apple.com";
const APPLE_MUSIC_ORIGIN: &str = "https://music.apple.com";
const APPLE_MUSIC_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/110.0.0.0 Safari/537.36";
const DEFAULT_ARTWORK_SIZE: usize = 1200;
const WEB_PLAYBACK_URL: &str = "https://play.itunes.apple.com/WebObjects/MZPlay.woa/wa/webPlayback";
const WEB_PLAYBACK_LICENSE_URL: &str =
    "https://play.itunes.apple.com/WebObjects/MZPlay.woa/wa/acquireWebPlaybackLicense";
const APPLE_WRAPPER_SCHEME: &str = "applemusic-wrapper://";
const APPLE_WIDEVINE_SCHEME: &str = "applemusic-widevine://";
const APPLE_WRAPPER_M3U8_PORT: u16 = 20020;
const APPLE_WRAPPER_DECRYPT_PORT: u16 = 10020;
const APPLE_PREFETCH_KEY_URI: &str = "skd://itunes.apple.com/P000000000/s1/e1";
const APPLE_WRAPPER_M3U8_TIMEOUT: Duration = Duration::from_secs(5);
const APPLE_WRAPPER_IO_TIMEOUT: Duration = Duration::from_secs(15);
type Aes128Ctr = ctr::Ctr128BE<aes::Aes128>;

// Public Widevine L3 test device from the source MusicBot-Go Apple Music provider.
// This is not Apple account data; users can override it with wv_client_id /
// wv_private_key file paths in settings.
const DEFAULT_WV_DEVICE_WVD_BASE64: &str = "V1ZEAgIDAATCMIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDZs7fK8XA2cgexsOXcxOMp0OyIFay5lY4ZXEicVYBUyn6d98ZiWq5Mqkm6sSvL3nKmtxEHeFiG8DmCGwAKJ1xY8MQ9WqpFnthct6/JQD5KfYnGm+05zDIUdtLCu43GmtY6QcJPvvg7gv/AlS3nHYLiIUKskHKPDSiY3y74Qd1q+8ftk+dvf8Rmn9yllr3/4c6S628EcG9o+nXwXwlCHNRD0zAu4MVOv5AsMHn5jnsl7a01gMrEPUr4R3klrQDg4qE8ojA/A+t7Se3mxyMttrctIAq0rGIaoW1Y7hTtN/Vit2Mm/aj2jxJ/ypydunO9DgrAWp6G4mcHv5buZ0knDXY9AgMBAAECggEAITfbA4xzotsjcWmcqWMhhm/qp5knE39HegD+AEMOX8rMhDXOfsrMQngNSxU5t7gbcdBd96lWjxR3Hu1sHtfrAd7aQSAXJjeU92+5uKTQLi3iKlIPv3LtJ9btdUDldRCao3hTaRYzS5LDN3gEDuGrJJwmzKB/oqCkxY9LbEVkRlBFSlhwEfjwTtio2uuVg/fJpD70nHnqQpLSa+/e9RdLsB9RtTwKIfAP0jcEbCzARS544YZV5dvG0UXyzwQKBgQD3sENwv9u36veokXR3G0wHlmeAlsXH1F2xuVh6lWluWvZR/GcW204+g1Iprhbfoe4Oy6fj12RqgAElZ21mzYfLqr7G8SCjO+ExoFUTeymdafMJPXJMxgsbTa4MSXR0+JODXXptUSVqzC2oxe+eels5MxN7siD2xaJRb8Yf1Ez43wKBgQDhAdv44DQF9zNtVA/0H6mW3iYC67gEObgY/CrXJc5ajcK3wiT4d5jWGo45ul/JAwDNr4R7+Q/pby9emISUauaJRtNcrhHOZ/7Jh46u+XcPzRsWrga0M0AhAPeOmPrOOVP4bPAjnTWBHHmI+Xh9JxxFJ2mh6uLsX0A21zoQQYXIYwKBgQCMzOdZhccaQvjsG1uQhbTvr0FBKPRfh0qHyCwS6zKW6CCUNJ5JsPtGsBIZ3XvlPsD8KitTatMLc56zK5tWUEn8riBrKRF7mYOHWXRjcaUTdfIRc5uxJveTWtIw6+TGxbPdflslH3bcwhrGkVaIyVdoKa/OplD01x5Rmu+Oknn7EwKBgAlrwoIRIRx+1TBmrKRUDw26D21f4TyMDiE9ra2Eb7dq6BQ5lMKyfzu3sOzJ2OjZr1btWma2buwfM6SKTkLIlw54YLEouKYjxI87lcXNvCZ1OAUjFDTHUJARMkxOK3InBFUKeqODGZJmVtBdYaYb1RswI0QcSZQMCOxC5rN3itpxAoGBANHVjaRqW+V5gauHBwDwtOciHStgR8C31jE2ZOLmoMp/lA+9Q/a/5Y59IHlyhSpHwsHB4cTlXwmAW3vh90sGplX+TbP90jSv4RBaalna48QLQqGT9Jv2JEN8ftq3c8y6yuTBkIZnxt4UQCvqJxdyVXTyc8I0YHpZyRpx/OkCPwDOBvQIARKaCwrdAwgCEhB55GvJxHmgCdW9IKt3MWtPGK+CwqUGIo4CMIIBCgKCAQEA2bO3yvFwNnIHsbDl3MTjKdDsiBWsuZWOGVxInFWAVMp+nffGYlquTKpJurEry95yprcRB3hYhvA5ghsACidcWPDEPVqqRZ7YXLevyUA+Sn2JxpvtOcwyFHbSwruNxprWOkHCT774O4L/wJUt5x2C4iFCrJByjw0omN8u+EHdavvH7ZPnb3/EZp/cpZa9/+HOkutvBHBvaPp18F8JQhzUQ9MwLuDFTr+QLDB5+Y57Je2tNYDKxD1K+Ed5Ja0A4OKhPKIwPwPre0nt5scjLba3LSAKtKxiGqFtWO4U7Tf1YrdjJv2o9o8Sf8qcnbpzvQ4KwFqehuJnB7+W7mdJJw12PQIDAQABKPAiSAFSqgEIARAAGoEBBBmGHQs+znCa66htomh9UrWTQOO+c2Gq84x8bDRmXO6bhpMC9qwyLJZ+GOj/7GYe2xG8rd6T0Q50gHV7ZzlrMW5JwAxLaftkbOoDxcFv5Q8zjoVR4a3ujWMOb8DKxpm3hTomLzBeXjV1YD2QX+Igru5OFKZv6aGykstijcBSK7aGIiAWbyJ1Tj5S9J9UHgm94n6Gp41uli41aSvGVuP0MONaBhKAAh6YNBzL5EpTDAarn02FrFhIfeHFIcYKSFxeeM0LQ55XmAF0lCXu6o33SWQXcnGeW9gXeaG7HullHmOKLgpo0br69lcSrbWKs0yehLHAiXO+kyi/vE69BZWbTQr7EOQ7bSdxMLUKVemYAAfINeYx5OvO8Iuju1zdRJtoiaUhYBf3axpmiPMnef2Bz5yMEoarPYCBywUKKBJnm+ku3P/ASTk/EgYono5aMTLgPkdkbSnYxjKEzzh9SHdM5+57ErVx5as58/EJvFI9GL3DQERoOaLG38riYhqVQVMEmxbG0TrDDfxSYUuS1agwzz5AueHF7qOa5Wm5yF0u2cw+eQCzpeMatAUKrgIIARIQaePomLssP7ijsygdhPiMFBiO1b6RBSKOAjCCAQoCggEBANj26bWJ8FEg6aQ+0NlOof4JlQFtvR4rQKfc0cV/wwVQPM8TP56Yr85O5v+E3EIkJqj/+k/kvy1E1Q8UOuu8IEyjtGf6JRv6YD/bJeInqK+pw30K72Lmo5TXKCif1JZlGy6MUUHyfFWt5jkvN6rTbzfOfUKDWy1xfi04dPu60/MUb9F4MVC3Q78YuXNXAHR3WyfdIow7hcuOFl2dyu0X2OWOmDvGMwiWdYlSrEOjtNCRPK0mZSXSRwztud0Et6sB0kUZpdzqhJjh43L8gYOW4sJBHcpkRCzL+Yt5XWCBDjgwWvtc4z6t37j/fXir915BuMeqNYW5WvyIhopur5HrdKECAwEAASjwIhKAAyKLYzDrP5CzVu+/EZK2J722lyCg7a4C3lm+/QINe+sWXCdJuj6pHvVG7YiVlZVJs3ldhIgPBzAHMeNjys2vd+Cxe1Im3Ljbl2M1C0Xn4naOsCF/dDabsQjlqiFkakPYTRyv51WP2jzHS4lHY1ZHwVWhVBJdmlp3cz/hDMEs22j+3P5QrJnBrtJz95NBzb5lwNLdkQ7eUZGwfzrgp/9wQa8lEEGpyklcp8b4i2oRbUAA+m2O6CDOZkTNGrnFMZ/qiv5tesqMKkFL0bVI7K8I0XWTnPqUbwqxeGBDEvoAMcyi6XG9QiKHI9jkymKZ1d9R0RDkcDYSo2I1U0g7io/TTuNbnXIB5OE6vLgnbbGzxF2QgKU4YBq4siXSUsDh+UzNb5V9CXnFjE/QTm26GfSH7zjgOyVTxkXtTV1TZCmsgCl99cP3CAAAKTbpGCDLx73rPyLzr/5hjJlCRz2Eh0IARGuARgIRsHLM42t1EOzUytL9yHWahXmkm/rcEgq9Aca8uRoWCgxjb21wYW55X25hbWUSBkdvb2dsZRohCgptb2RlbF9uYW1lEhNBT1NQIG9uIElBIEVtdWxhdG9yGhgKEWFyY2hpdGVjdHVyZV9uYW1lEgN4ODYaHgoLZGV2aWNlX25hbWUSD2dlbmVyaWNfeDg2X2FybRoiCgxwcm9kdWN0X25hbWUSEnNka19ncGhvbmVfeDg2X2FybRpkCgpidWlsZF9pbmZvElZnb29nbGUvc2RrX2dwaG9uZV94ODZfYXJtL2dlbmVyaWNfeDg2X2FybTo5L1BTUjEuMTgwNzIwLjEyMi82NzM2NzQyOnVzZXJkZWJ1Zy9kZXYta2V5cxoeChR3aWRldmluZV9jZG1fdmVyc2lvbhIGMTQuMC4wGiQKH29lbV9jcnlwdG9fc2VjdXJpdHlfcGF0Y2hfbGV2ZWwSATAyDhABIAAoDTAAQABIAFAA";

lazy_static! {
    static ref CLIENT: Client = {
        let timeout = apple_timeout();
        Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_else(|_| Client::new())
    };
    static ref DEVELOPER_TOKEN: RwLock<Option<String>> = RwLock::new(None);
    static ref JS_ASSET_PATTERN: Regex = Regex::new(r#"/assets/index[^"'\s]*\.js"#).unwrap();
    static ref TOKEN_PATTERN: Regex =
        Regex::new(r"eyJ[A-Za-z0-9_-]{40,}\.[A-Za-z0-9_-]{40,}\.[A-Za-z0-9_-]{40,}").unwrap();
    static ref APPLE_URL_PATTERN: Regex = Regex::new(r"https?://[^\s]+").unwrap();
    static ref APPLE_SONG_PATH_RE: Regex = Regex::new(r"^/[a-z]{2}/song/[^/]+/(\d{6,})$").unwrap();
    static ref ENHANCED_STREAM_INF_RE: Regex = Regex::new(r#"^#EXT-X-STREAM-INF:(.*)$"#).unwrap();
    static ref ENHANCED_AUDIO_RE: Regex = Regex::new(r#"AUDIO="([^"]*)""#).unwrap();
    static ref ENHANCED_CODEC_RE: Regex = Regex::new(r#"CODECS="([^"]*)""#).unwrap();
    static ref ENHANCED_BW_RE: Regex = Regex::new(r#"(^|,)BANDWIDTH=(\d+)"#).unwrap();
    static ref ENHANCED_AVG_BW_RE: Regex = Regex::new(r#"AVERAGE-BANDWIDTH=(\d+)"#).unwrap();
    static ref MEDIA_KEY_RE: Regex = Regex::new(r#"#EXT-X-KEY:[^\n]*URI="(skd://[^"]+)""#).unwrap();
    static ref MEDIA_MAP_RE: Regex = Regex::new(r#"URI="([^"]+)""#).unwrap();
    static ref APPLE_WRAPPER_TRACK_DECRYPT_SEMAPHORE: tokio::sync::Semaphore =
        tokio::sync::Semaphore::new(SETTINGS.music.applemusic.wrapper_track_concurrency.max(1));
    static ref APPLE_WRAPPER_DECRYPT_SEMAPHORE: tokio::sync::Semaphore =
        tokio::sync::Semaphore::new(
            SETTINGS
                .music
                .applemusic
                .wrapper_connection_concurrency
                .max(1)
        );
}

pub static APPLE_MUSIC_PROVIDER: AppleMusicProvider = AppleMusicProvider;

pub struct AppleMusicProvider;

#[async_trait]
impl MusicProvider for AppleMusicProvider {
    async fn search(&self, keyword: &str, limit: usize) -> Result<Vec<MusicSearchItem>, BotError> {
        ensure_apple_enabled()?;
        let limit = limit.clamp(1, 25);
        let url = apple_search_url(keyword, limit);
        let response: AppleMusicResponse = apple_get_json(&url, true).await?;
        Ok(response
            .results
            .and_then(|results| results.songs)
            .map(|songs| songs.data)
            .unwrap_or_default()
            .into_iter()
            .map(song_to_item)
            .filter(|item| !item.id.is_empty())
            .collect())
    }

    async fn resolve(
        &self,
        keyword: &str,
        selected_id: Option<&str>,
    ) -> Result<MusicTrack, BotError> {
        resolve_apple_track(keyword, selected_id, AppleMusicQuality::High).await
    }
}

pub(super) async fn resolve_with_quality(
    keyword: &str,
    selected_id: Option<&str>,
    quality: &str,
) -> Result<MusicTrack, BotError> {
    resolve_apple_track(keyword, selected_id, quality_from_str(quality)).await
}

async fn resolve_apple_track(
    keyword: &str,
    selected_id: Option<&str>,
    quality: AppleMusicQuality,
) -> Result<MusicTrack, BotError> {
    ensure_apple_enabled()?;
    let id = match selected_id {
        Some(id) => id.to_string(),
        None => parse_apple_track_id(keyword)
            .unwrap_or_default()
            .trim()
            .to_string(),
    };
    let id = if id.is_empty() {
        APPLE_MUSIC_PROVIDER
            .search(keyword, 1)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| BotError::Custom("没有找到可下载的音乐".to_string()))?
            .id
    } else {
        id
    };

    let (song, download_url) = tokio::try_join!(
        get_apple_song(&id),
        get_apple_download_url_with_quality(&id, quality)
    )?;
    let attrs = song.attributes;
    Ok(MusicTrack {
        id: song.id,
        platform: MusicPlatform::AppleMusic,
        song: attrs.name,
        singer: attrs.artist_name,
        album: attrs.album_name,
        cover: format_artwork_url(attrs.artwork.as_ref(), DEFAULT_ARTWORK_SIZE),
        link: attrs
            .url
            .filter(|url| !url.trim().is_empty())
            .unwrap_or_else(|| apple_track_url(&id)),
        url: download_url,
        headers: apple_download_headers(),
        duration: attrs
            .duration_in_millis
            .filter(|duration| *duration > 0)
            .map(|duration| ((duration + 500) / 1000) as u32),
        bitrate: None,
        format: Some("m4a".to_string()),
    })
}

#[derive(Deserialize)]
struct AppleMusicResponse {
    results: Option<AppleMusicSearchResults>,
    #[serde(default)]
    data: Vec<AppleMusicResource>,
}

#[derive(Deserialize)]
struct AppleMusicSearchResults {
    songs: Option<AppleMusicResourceList>,
}

#[derive(Deserialize)]
struct AppleMusicResourceList {
    #[serde(default)]
    data: Vec<AppleMusicResource>,
}

#[derive(Deserialize)]
struct AppleMusicResource {
    id: String,
    attributes: AppleMusicAttributes,
}

#[derive(Deserialize)]
struct AppleMusicAttributes {
    name: String,
    #[serde(default, rename = "artistName")]
    artist_name: String,
    #[serde(default, rename = "albumName")]
    album_name: String,
    #[serde(default, rename = "durationInMillis")]
    duration_in_millis: Option<u64>,
    artwork: Option<AppleMusicArtwork>,
    url: Option<String>,
    #[serde(default, rename = "extendedAssetUrls")]
    extended_asset_urls: Option<AppleMusicExtendedAssetUrls>,
}

#[derive(Deserialize)]
struct AppleMusicArtwork {
    url: String,
}

#[derive(Deserialize)]
struct AppleMusicExtendedAssetUrls {
    #[serde(default, rename = "enhancedHls")]
    enhanced_hls: String,
}

#[derive(Deserialize)]
struct WebPlaybackResponse {
    #[serde(default, rename = "songList")]
    song_list: Vec<WebPlaybackSong>,
}

#[derive(Deserialize)]
struct WebPlaybackSong {
    #[serde(default)]
    assets: Vec<WebPlaybackAsset>,
}

#[derive(Clone, Deserialize)]
struct WebPlaybackAsset {
    #[serde(rename = "URL")]
    url: String,
    flavor: Option<String>,
    #[serde(default)]
    metadata: WebPlaybackMetadata,
}

#[derive(Clone, Default, Deserialize)]
struct WebPlaybackMetadata {
    #[serde(default, rename = "bitRate")]
    bit_rate: i64,
}

async fn get_apple_song(id: &str) -> Result<AppleMusicResource, BotError> {
    let url = apple_song_url(id);
    let response: AppleMusicResponse = apple_get_json(&url, true).await?;
    response
        .data
        .into_iter()
        .next()
        .ok_or_else(|| BotError::Custom("没有找到 Apple Music 歌曲详情".to_string()))
}

async fn get_apple_download_url_with_quality(
    id: &str,
    quality: AppleMusicQuality,
) -> Result<String, BotError> {
    let media_user_token =
        normalize_media_user_token(SETTINGS.music.applemusic.media_user_token.trim());
    if media_user_token.is_empty() {
        return Err(BotError::Custom(
            "Apple Music 下载需要配置 music.applemusic.media_user_token；无损/Hi-Res 还需要 wrapper_host"
                .to_string(),
        ));
    }
    validate_apple_download_environment()?;
    let wrapper_host = apple_wrapper_host_opt();
    if should_prefer_apple_wrapper(quality, wrapper_host.is_some()) {
        return Ok(apple_wrapper_internal_url(
            wrapper_host.as_deref().unwrap(),
            id,
            quality,
        ));
    }
    Ok(apple_widevine_internal_url(id, quality))
}

async fn get_apple_webplayback_assets(
    id: &str,
    media_user_token: String,
) -> Result<Vec<WebPlaybackAsset>, BotError> {
    let developer_token = ensure_developer_token().await?;
    let response = CLIENT
        .post(WEB_PLAYBACK_URL)
        .header("Authorization", format!("Bearer {developer_token}"))
        .header("Content-Type", "application/json")
        .header("Origin", APPLE_MUSIC_ORIGIN)
        .header("User-Agent", APPLE_MUSIC_UA)
        .header("media-user-token", media_user_token)
        .json(&serde_json::json!({ "salableAdamId": id }))
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(BotError::Custom(format!(
            "Apple Music WebPlayback 请求失败：HTTP {}",
            response.status()
        )));
    }
    let response: WebPlaybackResponse = response.json().await?;
    Ok(response
        .song_list
        .into_iter()
        .flat_map(|song| song.assets)
        .collect())
}

fn should_prefer_apple_wrapper(quality: AppleMusicQuality, has_wrapper: bool) -> bool {
    has_wrapper
        && matches!(
            quality,
            AppleMusicQuality::Standard | AppleMusicQuality::Lossless | AppleMusicQuality::HiRes
        )
}

fn apple_wrapper_internal_url(host: &str, id: &str, quality: AppleMusicQuality) -> String {
    format!(
        "{APPLE_WRAPPER_SCHEME}{host}/{id}?quality={}",
        quality.as_str()
    )
}

fn apple_widevine_internal_url(id: &str, quality: AppleMusicQuality) -> String {
    format!("{APPLE_WIDEVINE_SCHEME}{id}?quality={}", quality.as_str())
}

pub(super) async fn download_internal_url_with_progress_stats<F>(
    url: &str,
    progress: &mut F,
) -> Result<(Vec<u8>, Option<Duration>), BotError>
where
    F: FnMut(DownloadProgress) + Send,
{
    if let Some(payload) = url.strip_prefix(APPLE_WIDEVINE_SCHEME) {
        let (track_id, query) = payload.split_once('?').unwrap_or((payload, ""));
        if track_id.trim().is_empty() {
            return Err(BotError::Custom(
                "Apple Music Widevine URL 缺少 track id".to_string(),
            ));
        }
        let quality = query
            .split('&')
            .filter_map(|part| part.split_once('='))
            .find_map(|(key, value)| (key == "quality").then_some(value))
            .map(quality_from_str)
            .unwrap_or(AppleMusicQuality::High);
        match download_via_widevine_with_progress(track_id, progress).await {
            Ok(data) => return Ok((data, None)),
            Err(err) if quality == AppleMusicQuality::High => {
                if let Some(host) = apple_wrapper_host_opt() {
                    return download_via_wrapper_with_progress_stats(
                        &host,
                        track_id,
                        AppleMusicQuality::High,
                        progress,
                    )
                    .await
                    .map_err(|wrapper_err| {
                        BotError::Custom(format!(
                            "Apple Music Widevine 解密失败：{err}；wrapper fallback 也失败：{wrapper_err}"
                        ))
                    });
                }
                return Err(err);
            }
            Err(err) => return Err(err),
        }
    }

    let payload = url
        .strip_prefix(APPLE_WRAPPER_SCHEME)
        .ok_or_else(|| BotError::Custom("Unknown Apple Music internal URL".to_string()))?;
    let (host, rest) = payload
        .split_once('/')
        .ok_or_else(|| BotError::Custom("Apple Music wrapper URL 缺少 track id".to_string()))?;
    let (track_id, query) = rest.split_once('?').unwrap_or((rest, ""));
    if host.trim().is_empty() || track_id.trim().is_empty() {
        return Err(BotError::Custom("Apple Music wrapper URL 无效".to_string()));
    }
    let quality = query
        .split('&')
        .filter_map(|part| part.split_once('='))
        .find_map(|(key, value)| (key == "quality").then_some(value))
        .map(quality_from_str)
        .unwrap_or(AppleMusicQuality::High);
    download_via_wrapper_with_progress_stats(host, track_id, quality, progress).await
}

#[cfg(test)]
async fn download_via_wrapper(
    host: &str,
    track_id: &str,
    quality: AppleMusicQuality,
) -> Result<Vec<u8>, BotError> {
    let mut progress = |_| {};
    download_via_wrapper_with_progress(host, track_id, quality, &mut progress).await
}

async fn download_via_widevine_with_progress<F>(
    track_id: &str,
    progress: &mut F,
) -> Result<Vec<u8>, BotError>
where
    F: FnMut(DownloadProgress) + Send,
{
    let media_user_token =
        normalize_media_user_token(SETTINGS.music.applemusic.media_user_token.trim());
    if media_user_token.is_empty() {
        return Err(BotError::Custom(
            "Apple Music Widevine 下载需要配置 media_user_token".to_string(),
        ));
    }
    let assets = get_apple_webplayback_assets(track_id, media_user_token.clone()).await?;
    let asset = select_widevine_asset(&assets)
        .ok_or_else(|| BotError::Custom("Apple Music 没有返回 Widevine AAC 资源".to_string()))?;
    let hls = download_text(&asset.url).await?;
    let widevine_media = parse_widevine_hls(&asset.url, &hls)?;
    let encrypted = download_bytes_with_progress(&widevine_media.mp4_url, progress).await?;
    let key = acquire_widevine_content_key(
        track_id,
        &widevine_media.kid,
        &widevine_media.uri_prefix,
        &widevine_media.kid_b64,
        &media_user_token,
    )
    .await?;
    decrypt_cenc_fmp4(encrypted, &key)
}

#[cfg(test)]
async fn download_via_wrapper_with_progress<F>(
    host: &str,
    track_id: &str,
    quality: AppleMusicQuality,
    progress: &mut F,
) -> Result<Vec<u8>, BotError>
where
    F: FnMut(DownloadProgress) + Send,
{
    download_via_wrapper_with_progress_stats(host, track_id, quality, progress)
        .await
        .map(|(audio, _)| audio)
}

async fn download_via_wrapper_with_progress_stats<F>(
    host: &str,
    track_id: &str,
    quality: AppleMusicQuality,
    progress: &mut F,
) -> Result<(Vec<u8>, Option<Duration>), BotError>
where
    F: FnMut(DownloadProgress) + Send,
{
    let mut master_url = String::new();
    if quality == AppleMusicQuality::HiRes
        && let Ok(device_url) = wrapper_get_m3u8_url(host, track_id).await
        && device_url.ends_with(".m3u8")
    {
        master_url = device_url;
    }
    if master_url.is_empty() {
        master_url = enhanced_hls_master_url(track_id).await?;
    }

    let master = download_text(&master_url).await?;
    let variants = parse_enhanced_hls_master(&master)?;
    let variant = select_variant_for_quality(&variants, quality)
        .ok_or_else(|| BotError::Custom("Apple Music enhancedHls 没有匹配的音质".to_string()))?;
    let media_url = absolute_hls_url(&master_url, &variant.uri);
    let media = parse_enhanced_hls_media(&media_url, &download_text(&media_url).await?)?;
    let prewarm_task = {
        let host = host.to_string();
        let track_id = track_id.to_string();
        let seg_keys = media.seg_keys.clone();
        tokio::spawn(async move {
            let _ = prewarm_wrapper_keys(&host, &track_id, &seg_keys).await;
        })
    };
    let encrypted = download_bytes_with_progress(&media.mp4_url, progress).await?;
    let _ = prewarm_task.await;
    let mut last_error = None;
    for attempt in 0..2 {
        match decrypt_fmp4_with_wrapper(host, track_id, encrypted.clone(), &media.seg_keys).await {
            Ok(data) => return Ok((data.audio, Some(data.decrypt_elapsed))),
            Err(e) if attempt == 0 && is_retryable_wrapper_error(&e) => {
                last_error = Some(e);
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_error.unwrap_or_else(|| BotError::Custom("Apple Music wrapper 解密失败".to_string())))
}

async fn prewarm_wrapper_keys(
    host: &str,
    track_id: &str,
    seg_keys: &[String],
) -> Result<(), BotError> {
    let mut seen = Vec::<(&str, &str)>::new();
    for key_uri in seg_keys {
        let adam = if key_uri == APPLE_PREFETCH_KEY_URI {
            "0"
        } else {
            track_id
        };
        if seen
            .iter()
            .any(|(seen_adam, seen_uri)| *seen_adam == adam && *seen_uri == key_uri)
        {
            continue;
        }
        seen.push((adam, key_uri));
        prewarm_wrapper_key(host, adam, key_uri).await?;
    }
    Ok(())
}

async fn prewarm_wrapper_key(host: &str, adam: &str, key_uri: &str) -> Result<(), BotError> {
    let addr = format!("{host}:{APPLE_WRAPPER_DECRYPT_PORT}");
    let stream = tokio::time::timeout(apple_timeout(), tokio::net::TcpStream::connect(&addr))
        .await
        .map_err(|_| BotError::Custom(format!("Apple Music wrapper prewarm 连接超时：{addr}")))??;
    stream.set_nodelay(true)?;
    let mut stream = stream;
    write_len_prefixed(&mut stream, adam).await?;
    write_len_prefixed(&mut stream, key_uri).await?;
    wrapper_write_all(&mut stream, &[0, 0, 0, 0, 0], "prewarm finish").await?;
    wrapper_flush(&mut stream, "prewarm finish").await?;
    Ok(())
}

fn select_widevine_asset(assets: &[WebPlaybackAsset]) -> Option<&WebPlaybackAsset> {
    assets
        .iter()
        .find(|asset| asset.flavor.as_deref() == Some("28:ctrp256"))
        .or_else(|| assets.iter().max_by_key(|asset| asset.metadata.bit_rate))
}

struct AppleWrapperDecryptOutput {
    audio: Vec<u8>,
    decrypt_elapsed: Duration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct WidevineHlsMedia {
    mp4_url: String,
    kid_b64: String,
    kid: [u8; 16],
    uri_prefix: String,
}

fn parse_widevine_hls(m3u8_url: &str, content: &str) -> Result<WidevineHlsMedia, BotError> {
    let mut mp4_url = String::new();
    let mut kid_b64 = String::new();
    let mut uri_prefix = String::new();
    for line in content.lines().map(str::trim) {
        if line.contains("#EXT-X-KEY")
            && let Some(uri) = extract_quoted_attr(line, "URI")
            && let Some((prefix, kid)) = uri.split_once(',')
        {
            uri_prefix = prefix.to_string();
            kid_b64 = kid.to_string();
        }
        if line.contains("#EXT-X-MAP")
            && let Some(uri) = extract_quoted_attr(line, "URI")
        {
            mp4_url = absolute_hls_url(m3u8_url, &uri);
        }
        if !line.starts_with('#')
            && !line.is_empty()
            && (line.ends_with(".mp4") || line.ends_with(".m4a") || line.ends_with(".m4s"))
            && mp4_url.is_empty()
        {
            mp4_url = absolute_hls_url(m3u8_url, line);
        }
    }
    if mp4_url.is_empty() {
        return Err(BotError::Custom(
            "Apple Music Widevine playlist 缺少 MP4 URL".to_string(),
        ));
    }
    let kid_vec = general_purpose::STANDARD
        .decode(kid_b64.as_bytes())
        .map_err(|e| BotError::Custom(format!("Apple Music Widevine KID 解码失败：{e}")))?;
    let kid: [u8; 16] = kid_vec
        .try_into()
        .map_err(|_| BotError::Custom("Apple Music Widevine KID 长度错误".to_string()))?;
    Ok(WidevineHlsMedia {
        mp4_url,
        kid_b64,
        kid,
        uri_prefix,
    })
}

fn extract_quoted_attr(line: &str, attr: &str) -> Option<String> {
    let marker = format!(r#"{attr}=""#);
    let start = line.find(&marker)? + marker.len();
    let end = line[start..].find('"')?;
    Some(line[start..start + end].to_string())
}

async fn acquire_widevine_content_key(
    track_id: &str,
    kid: &[u8; 16],
    uri_prefix: &str,
    kid_b64: &str,
    media_user_token: &str,
) -> Result<Vec<u8>, BotError> {
    let device = load_widevine_device()?;
    let pssh = build_widevine_pssh(kid)?;
    let request = Cdm::new(device)
        .open()
        .get_license_request(pssh, LicenseType::AUTOMATIC)
        .map_err(|e| BotError::Custom(format!("Apple Music Widevine challenge 生成失败：{e}")))?;
    let challenge = request
        .challenge()
        .map_err(|e| BotError::Custom(format!("Apple Music Widevine challenge 签名失败：{e}")))?;
    let developer_token = ensure_developer_token().await?;
    let response = CLIENT
        .post(WEB_PLAYBACK_LICENSE_URL)
        .header("Authorization", format!("Bearer {developer_token}"))
        .header("Content-Type", "application/json")
        .header("Origin", APPLE_MUSIC_ORIGIN)
        .header("User-Agent", APPLE_MUSIC_UA)
        .header("media-user-token", media_user_token)
        .json(&serde_json::json!({
            "challenge": general_purpose::STANDARD.encode(challenge),
            "key-system": "com.widevine.alpha",
            "uri": format!("{uri_prefix},{kid_b64}"),
            "adamId": track_id,
            "isLibrary": false,
            "user-initiated": true,
        }))
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(BotError::Custom(format!(
            "Apple Music Widevine license 请求失败：HTTP {}",
            response.status()
        )));
    }
    let response: WidevineLicenseResponse = response.json().await?;
    if response.status != 0 {
        return Err(BotError::Custom(format!(
            "Apple Music Widevine license 状态异常：{}",
            response.status
        )));
    }
    let license = general_purpose::STANDARD
        .decode(response.license.as_bytes())
        .map_err(|e| BotError::Custom(format!("Apple Music Widevine license 解码失败：{e}")))?;
    let keys = request
        .get_keys(&license)
        .map_err(|e| BotError::Custom(format!("Apple Music Widevine license 解析失败：{e}")))?;
    let key = keys
        .content_key(kid)
        .or_else(|_| keys.first_of_type(KeyType::CONTENT))
        .map_err(|e| BotError::Custom(format!("Apple Music Widevine content key 缺失：{e}")))?;
    Ok(key.key.clone())
}

#[derive(Deserialize)]
struct WidevineLicenseResponse {
    license: String,
    status: i32,
}

fn build_widevine_pssh(kid: &[u8; 16]) -> Result<Pssh, BotError> {
    let mut data = WidevinePsshData::new();
    data.key_ids.push(kid.to_vec());
    data.set_algorithm(widevine_pssh_data::Algorithm::AESCTR);
    let bytes = data
        .write_to_bytes()
        .map_err(|e| BotError::Custom(format!("Apple Music Widevine PSSH 序列化失败：{e}")))?;
    Pssh::from_bytes(&bytes)
        .map_err(|e| BotError::Custom(format!("Apple Music Widevine PSSH 解析失败：{e}")))
}

fn load_widevine_device() -> Result<Device, BotError> {
    let client_id_path = SETTINGS.music.applemusic.wv_client_id.trim();
    let private_key_path = SETTINGS.music.applemusic.wv_private_key.trim();
    if !client_id_path.is_empty() && !private_key_path.is_empty() {
        let client_id = fs::read(client_id_path).map_err(|e| {
            BotError::Custom(format!("Apple Music Widevine client_id 读取失败：{e}"))
        })?;
        let private_key = fs::read_to_string(private_key_path).map_err(|e| {
            BotError::Custom(format!("Apple Music Widevine private_key 读取失败：{e}"))
        })?;
        let private_key = RsaPrivateKey::from_pkcs1_pem(&private_key).map_err(|e| {
            BotError::Custom(format!("Apple Music Widevine private_key 解析失败：{e}"))
        })?;
        return Device::new(
            DeviceType::ANDROID,
            SecurityLevel::L3,
            private_key,
            &client_id,
        )
        .map_err(|e| BotError::Custom(format!("Apple Music Widevine device 初始化失败：{e}")));
    }

    let bytes = general_purpose::STANDARD
        .decode(DEFAULT_WV_DEVICE_WVD_BASE64.as_bytes())
        .map_err(|e| BotError::Custom(format!("Apple Music 默认 Widevine device 解码失败：{e}")))?;
    Device::read_wvd(Cursor::new(bytes))
        .map_err(|e| BotError::Custom(format!("Apple Music 默认 Widevine device 初始化失败：{e}")))
}

fn decrypt_cenc_fmp4(mut data: Vec<u8>, key: &[u8]) -> Result<Vec<u8>, BotError> {
    if key.len() != 16 {
        return Err(BotError::Custom(
            "Apple Music Widevine content key 长度错误".to_string(),
        ));
    }
    let tenc = parse_tenc_from_init(&data)?;
    if tenc.default_crypt_byte_block != 0 || tenc.default_skip_byte_block != 0 {
        return Err(BotError::Custom(
            "Apple Music Widevine native 解密暂不支持 pattern encryption".to_string(),
        ));
    }

    let mut pos = 0usize;
    while let Some(box_info) = read_mp4_box(&data, pos)? {
        if &box_info.typ != b"moof" {
            pos = box_info.end();
            continue;
        }
        let moof = box_info;
        let mdat = read_mp4_box(&data, moof.end())?
            .filter(|next| &next.typ == b"mdat")
            .ok_or_else(|| BotError::Custom("MP4 moof 后缺少 mdat".to_string()))?;
        let mut traf_pos = moof.payload_start();
        while let Some(traf) = read_mp4_box(&data, traf_pos)? {
            if traf.end() > moof.end() {
                break;
            }
            if &traf.typ == b"traf" {
                let parsed = parse_traf(&data, traf, &tenc)?;
                let start = parsed
                    .data_offset
                    .map(|offset| (moof.start as i64 + offset as i64) as usize)
                    .unwrap_or_else(|| mdat.payload_start());
                decrypt_cenc_samples(
                    &mut data,
                    start,
                    &parsed.samples,
                    parsed.senc.as_ref(),
                    &tenc,
                    key,
                )?;
            }
            traf_pos = traf.end();
        }
        pos = mdat.end();
    }

    remux_decrypted_fmp4_to_progressive(&data)
}

fn decrypt_cenc_samples(
    data: &mut [u8],
    sample_start: usize,
    samples: &[TrunSampleInfo],
    senc: Option<&SencBox>,
    tenc: &TencBox,
    key: &[u8],
) -> Result<(), BotError> {
    let mut pos = sample_start;
    for (index, sample) in samples.iter().enumerate() {
        let end = pos + sample.size as usize;
        if end > data.len() {
            return Err(BotError::Custom("MP4 sample 超出文件范围".to_string()));
        }
        let senc_sample = senc.and_then(|senc| senc.samples.get(index));
        let iv = cenc_sample_iv(senc_sample, tenc)?;
        let subsamples = senc_sample
            .map(|sample| sample.subsamples.as_slice())
            .unwrap_or(&[]);
        decrypt_cenc_sample(&mut data[pos..end], subsamples, key, &iv)?;
        pos = end;
    }
    Ok(())
}

fn cenc_sample_iv(
    sample: Option<&oxideav_mp4::cenc::SencSample>,
    tenc: &TencBox,
) -> Result<[u8; 16], BotError> {
    let iv = sample
        .map(|sample| sample.initialization_vector.as_slice())
        .filter(|iv| !iv.is_empty())
        .or(tenc.default_constant_iv.as_deref())
        .ok_or_else(|| BotError::Custom("Apple Music Widevine sample IV 缺失".to_string()))?;
    let mut out = [0u8; 16];
    match iv.len() {
        8 => out[..8].copy_from_slice(iv),
        16 => out.copy_from_slice(iv),
        len => {
            return Err(BotError::Custom(format!(
                "Apple Music Widevine sample IV 长度错误：{len}"
            )));
        }
    }
    Ok(out)
}

fn decrypt_cenc_sample(
    sample: &mut [u8],
    subsamples: &[SubsampleEntry],
    key: &[u8],
    iv: &[u8; 16],
) -> Result<(), BotError> {
    let mut cipher = Aes128Ctr::new_from_slices(key, iv)
        .map_err(|e| BotError::Custom(format!("Apple Music Widevine AES-CTR 初始化失败：{e}")))?;
    if subsamples.is_empty() {
        cipher.apply_keystream(sample);
        return Ok(());
    }

    let mut pos = 0usize;
    for subsample in subsamples {
        pos += subsample.bytes_of_clear_data as usize;
        let end = pos + subsample.bytes_of_protected_data as usize;
        if end > sample.len() {
            return Err(BotError::Custom(
                "MP4 subsample protected range 超出 sample".to_string(),
            ));
        }
        cipher.apply_keystream(&mut sample[pos..end]);
        pos = end;
    }
    Ok(())
}

fn is_retryable_wrapper_error(err: &BotError) -> bool {
    match err {
        BotError::Custom(message) => {
            message.contains("wrapper") || message.contains("decrypt") || message.contains("CKC")
        }
        BotError::IOError(_) => true,
        _ => false,
    }
}

async fn enhanced_hls_master_url(track_id: &str) -> Result<String, BotError> {
    let url = apple_song_url(track_id);
    let response: AppleMusicResponse = apple_get_json(&url, true).await?;
    let song = response
        .data
        .into_iter()
        .next()
        .ok_or_else(|| BotError::Custom("没有找到 Apple Music 歌曲详情".to_string()))?;
    song.attributes
        .extended_asset_urls
        .map(|urls| urls.enhanced_hls)
        .filter(|url| !url.trim().is_empty())
        .ok_or_else(|| BotError::Custom("Apple Music 没有返回 enhancedHls 资源".to_string()))
}

async fn wrapper_get_m3u8_url(host: &str, track_id: &str) -> Result<String, BotError> {
    let addr = format!("{host}:{APPLE_WRAPPER_M3U8_PORT}");
    let mut stream = tokio::time::timeout(
        APPLE_WRAPPER_M3U8_TIMEOUT,
        tokio::net::TcpStream::connect(&addr),
    )
    .await
    .map_err(|_| BotError::Custom(format!("Apple Music wrapper m3u8 连接超时：{addr}")))??;
    stream.set_nodelay(true)?;
    let id = track_id.as_bytes();
    if id.len() > u8::MAX as usize {
        return Err(BotError::Custom("Apple Music track id 过长".to_string()));
    }
    wrapper_write_all(&mut stream, &[id.len() as u8], "m3u8 track id length").await?;
    wrapper_write_all(&mut stream, id, "m3u8 track id").await?;
    let mut buf = Vec::new();
    tokio::time::timeout(APPLE_WRAPPER_M3U8_TIMEOUT, stream.read_to_end(&mut buf))
        .await
        .map_err(|_| BotError::Custom("Apple Music wrapper m3u8 读取超时".to_string()))??;
    let url = String::from_utf8_lossy(&buf).trim().to_string();
    if url.is_empty() || url == "\0" {
        return Err(BotError::Custom(
            "Apple Music wrapper 没有返回 m3u8".to_string(),
        ));
    }
    Ok(url)
}

async fn download_text(url: &str) -> Result<String, BotError> {
    let response = CLIENT
        .get(url)
        .header("User-Agent", APPLE_MUSIC_UA)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(BotError::Custom(format!(
            "Apple Music 下载 playlist 失败：HTTP {}",
            response.status()
        )));
    }
    Ok(response.text().await?)
}

async fn download_bytes_with_progress<F>(url: &str, progress: &mut F) -> Result<Vec<u8>, BotError>
where
    F: FnMut(DownloadProgress) + Send,
{
    let response = CLIENT
        .get(url)
        .header("User-Agent", APPLE_MUSIC_UA)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(BotError::Custom(format!(
            "Apple Music 下载媒体失败：HTTP {}",
            response.status()
        )));
    }
    let total = response.content_length();
    let mut response = response;
    let mut buf = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        buf.extend_from_slice(&chunk);
        progress(DownloadProgress {
            written: buf.len() as u64,
            total,
        });
    }
    Ok(buf)
}

async fn apple_get_json<T: serde::de::DeserializeOwned>(
    url: &str,
    retry: bool,
) -> Result<T, BotError> {
    let token = ensure_developer_token().await?;
    let response = CLIENT
        .get(url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Origin", APPLE_MUSIC_ORIGIN)
        .header("User-Agent", APPLE_MUSIC_UA)
        .send()
        .await?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED && retry {
        clear_developer_token();
        return Box::pin(apple_get_json(url, false)).await;
    }
    if !response.status().is_success() {
        return Err(BotError::Custom(format!(
            "Apple Music API 请求失败：HTTP {}",
            response.status()
        )));
    }
    Ok(response.json().await?)
}

async fn ensure_developer_token() -> Result<String, BotError> {
    if let Some(token) = DEVELOPER_TOKEN.read().unwrap().clone() {
        return Ok(token);
    }
    let token = fetch_developer_token().await?;
    *DEVELOPER_TOKEN.write().unwrap() = Some(token.clone());
    Ok(token)
}

async fn fetch_developer_token() -> Result<String, BotError> {
    let html = CLIENT
        .get(APPLE_MUSIC_ORIGIN)
        .header("User-Agent", APPLE_MUSIC_UA)
        .send()
        .await?
        .text()
        .await?;
    let js_path = JS_ASSET_PATTERN
        .find(&html)
        .ok_or_else(|| BotError::Custom("Apple Music 首页没有找到 JS bundle".to_string()))?
        .as_str();
    let js_url = format!("{APPLE_MUSIC_ORIGIN}{js_path}");
    let js = CLIENT
        .get(js_url)
        .header("User-Agent", APPLE_MUSIC_UA)
        .send()
        .await?
        .text()
        .await?;
    TOKEN_PATTERN
        .find(&js)
        .map(|value| value.as_str().to_string())
        .ok_or_else(|| BotError::Custom("Apple Music JS 中没有找到 developer token".to_string()))
}

fn clear_developer_token() {
    *DEVELOPER_TOKEN.write().unwrap() = None;
}

fn apple_search_url(keyword: &str, limit: usize) -> String {
    let mut url = Url::parse(&format!(
        "{APPLE_MUSIC_API}/v1/catalog/{}/search",
        apple_storefront()
    ))
    .unwrap();
    url.query_pairs_mut()
        .append_pair("term", keyword)
        .append_pair("types", "songs")
        .append_pair("limit", &limit.to_string())
        .append_pair("l", &apple_language());
    url.to_string()
}

fn apple_song_url(id: &str) -> String {
    let mut url = Url::parse(&format!(
        "{APPLE_MUSIC_API}/v1/catalog/{}/songs/{id}",
        apple_storefront()
    ))
    .unwrap();
    url.query_pairs_mut()
        .append_pair("include", "albums,artists")
        .append_pair("extend", "extendedAssetUrls")
        .append_pair("l", &apple_language());
    url.to_string()
}

fn song_to_item(song: AppleMusicResource) -> MusicSearchItem {
    MusicSearchItem {
        platform: MusicPlatform::AppleMusic,
        id: song.id,
        song: song.attributes.name,
        singer: if song.attributes.artist_name.trim().is_empty() {
            "未知歌手".to_string()
        } else {
            song.attributes.artist_name
        },
    }
}

fn parse_apple_track_id(text: &str) -> Option<String> {
    let text = text.trim();
    if text.chars().all(|c| c.is_ascii_digit()) && text.len() >= 6 {
        return Some(text.to_string());
    }
    if let Some((prefix, value)) = text.split_once(':')
        && MusicPlatform::from_alias(prefix).is_some()
        && value.len() >= 6
        && value.chars().all(|c| c.is_ascii_digit())
    {
        return Some(value.trim().to_string());
    }
    let url_text = APPLE_URL_PATTERN
        .find(text)
        .map(|value| {
            value
                .as_str()
                .trim_end_matches(&['.', ',', '!', '?', ')', ']', '}'][..])
        })
        .unwrap_or(text);
    let url = Url::parse(url_text).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    if host != "music.apple.com" && !host.ends_with(".music.apple.com") {
        return None;
    }
    for key in ["i", "songId"] {
        if let Some(id) = url
            .query_pairs()
            .find(|(query_key, _)| query_key == key)
            .map(|(_, value)| value.to_string())
            .filter(|value| value.len() >= 6 && value.chars().all(|c| c.is_ascii_digit()))
        {
            return Some(id);
        }
    }
    APPLE_SONG_PATH_RE
        .captures(url.path())
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_string())
}

fn format_artwork_url(artwork: Option<&AppleMusicArtwork>, size: usize) -> String {
    artwork
        .map(|artwork| {
            artwork
                .url
                .replace("{w}", &size.to_string())
                .replace("{h}", &size.to_string())
        })
        .unwrap_or_default()
}

fn apple_track_url(id: &str) -> String {
    format!("https://music.apple.com/song/{id}")
}

fn apple_storefront() -> String {
    SETTINGS
        .music
        .applemusic
        .storefront
        .trim()
        .to_string()
        .if_empty("us")
}

fn apple_language() -> String {
    SETTINGS
        .music
        .applemusic
        .language
        .trim()
        .to_string()
        .if_empty("en-US")
}

fn apple_timeout() -> Duration {
    Duration::from_secs(SETTINGS.music.applemusic.timeout.max(1))
}

fn ensure_apple_enabled() -> Result<(), BotError> {
    if SETTINGS.music.applemusic.enabled {
        Ok(())
    } else {
        Err(BotError::Custom("Apple Music provider 未启用".to_string()))
    }
}

fn validate_apple_download_environment() -> Result<(), BotError> {
    let config = &SETTINGS.music.applemusic;
    let has_widevine_override =
        !config.wv_client_id.trim().is_empty() || !config.wv_private_key.trim().is_empty();
    if has_widevine_override
        && (config.wv_client_id.trim().is_empty() || config.wv_private_key.trim().is_empty())
    {
        return Err(BotError::Custom(
            "Apple Music Widevine 覆盖配置需要同时填写 music.applemusic.wv_client_id 和 music.applemusic.wv_private_key"
                .to_string(),
        ));
    }
    Ok(())
}

fn apple_download_headers() -> HashMap<String, String> {
    [("User-Agent".to_string(), APPLE_MUSIC_UA.to_string())]
        .into_iter()
        .collect()
}

fn normalize_media_user_token(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    raw.split(';')
        .find_map(|part| {
            let (key, value) = part.trim().split_once('=')?;
            (key.trim() == "media-user-token").then(|| value.trim().to_string())
        })
        .unwrap_or_else(|| raw.to_string())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AppleMusicQuality {
    Standard,
    High,
    Lossless,
    HiRes,
}

impl AppleMusicQuality {
    fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::High => "high",
            Self::Lossless => "lossless",
            Self::HiRes => "hires",
        }
    }
}

fn quality_from_str(value: &str) -> AppleMusicQuality {
    match value.trim().to_ascii_lowercase().as_str() {
        "standard" | "std" | "128" => AppleMusicQuality::Standard,
        "lossless" | "alac" | "无损" => AppleMusicQuality::Lossless,
        "hires" | "hi-res" | "hi_res" | "高解析" => AppleMusicQuality::HiRes,
        _ => AppleMusicQuality::High,
    }
}

fn apple_wrapper_host_opt() -> Option<String> {
    let host = SETTINGS.music.applemusic.wrapper_host.trim();
    (!host.is_empty()).then(|| host.to_string())
}

#[derive(Clone, Debug)]
struct EnhancedHlsVariant {
    codecs: String,
    audio: String,
    avg_bw: i64,
    uri: String,
    sample_rate: i64,
    bit_depth: i64,
}

impl EnhancedHlsVariant {
    fn is_alac(&self) -> bool {
        self.codecs == "alac"
    }

    fn is_aac(&self) -> bool {
        self.codecs.starts_with("mp4a.40")
    }
}

#[derive(Clone, Debug)]
struct EnhancedHlsMedia {
    mp4_url: String,
    seg_keys: Vec<String>,
}

fn parse_enhanced_hls_master(content: &str) -> Result<Vec<EnhancedHlsVariant>, BotError> {
    let lines = content.lines().map(str::trim).collect::<Vec<_>>();
    let mut variants = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(captures) = ENHANCED_STREAM_INF_RE.captures(lines[i]) else {
            i += 1;
            continue;
        };
        let attrs = captures.get(1).map(|m| m.as_str()).unwrap_or_default();
        let mut variant = EnhancedHlsVariant {
            codecs: ENHANCED_CODEC_RE
                .captures(attrs)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string())
                .unwrap_or_default(),
            audio: ENHANCED_AUDIO_RE
                .captures(attrs)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string())
                .unwrap_or_default(),
            avg_bw: ENHANCED_AVG_BW_RE
                .captures(attrs)
                .or_else(|| ENHANCED_BW_RE.captures(attrs))
                .and_then(|c| c.get(if c.len() > 2 { 2 } else { 1 }))
                .and_then(|m| m.as_str().parse::<i64>().ok())
                .unwrap_or_default(),
            uri: String::new(),
            sample_rate: 0,
            bit_depth: 0,
        };
        let (sample_rate, bit_depth) = parse_alac_group_details(&variant.audio);
        variant.sample_rate = sample_rate;
        variant.bit_depth = bit_depth;
        i += 1;
        while i < lines.len() {
            let line = lines[i];
            i += 1;
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            variant.uri = line.to_string();
            break;
        }
        if !variant.uri.is_empty() {
            variants.push(variant);
        }
    }
    if variants.is_empty() {
        return Err(BotError::Custom(
            "Apple Music enhancedHls 没有 stream variant".to_string(),
        ));
    }
    variants.sort_by(|a, b| b.avg_bw.cmp(&a.avg_bw));
    Ok(variants)
}

fn parse_alac_group_details(group: &str) -> (i64, i64) {
    if !group.contains("alac") {
        return (0, 0);
    }
    let parts = group.split('-').collect::<Vec<_>>();
    if parts.len() < 2 {
        return (0, 0);
    }
    let bit_depth = parts
        .last()
        .and_then(|part| part.parse::<i64>().ok())
        .unwrap_or_default();
    let sample_rate = parts
        .get(parts.len().saturating_sub(2))
        .and_then(|part| part.parse::<i64>().ok())
        .unwrap_or_default();
    (sample_rate, bit_depth)
}

fn select_variant_for_quality(
    variants: &[EnhancedHlsVariant],
    quality: AppleMusicQuality,
) -> Option<EnhancedHlsVariant> {
    match quality {
        AppleMusicQuality::HiRes => variants
            .iter()
            .filter(|v| v.is_alac() && (v.bit_depth >= 24 || v.sample_rate > 44_100))
            .max_by_key(|v| (v.sample_rate, v.avg_bw))
            .cloned()
            .or_else(|| select_variant_for_quality(variants, AppleMusicQuality::Lossless)),
        AppleMusicQuality::Lossless => variants
            .iter()
            .filter(|v| v.is_alac())
            .max_by_key(|v| (v.sample_rate, v.avg_bw))
            .cloned(),
        AppleMusicQuality::High => best_aac_variant(variants, 256_000),
        AppleMusicQuality::Standard => best_aac_variant(variants, 128_000),
    }
}

fn best_aac_variant(variants: &[EnhancedHlsVariant], target: i64) -> Option<EnhancedHlsVariant> {
    variants
        .iter()
        .filter(|v| v.is_aac())
        .min_by_key(|v| (v.avg_bw - target).abs())
        .cloned()
}

fn parse_enhanced_hls_media(media_url: &str, content: &str) -> Result<EnhancedHlsMedia, BotError> {
    let mut current_key = String::new();
    let mut mp4_name = String::new();
    let mut seg_keys = Vec::new();
    for line in content.lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        if line.starts_with("#EXT-X-KEY") {
            if let Some(key) = MEDIA_KEY_RE
                .captures(line)
                .and_then(|captures| captures.get(1))
                .map(|m| m.as_str().to_string())
            {
                current_key = key;
            }
            continue;
        }
        if line.starts_with("#EXT-X-MAP") {
            if let Some(name) = MEDIA_MAP_RE
                .captures(line)
                .and_then(|captures| captures.get(1))
                .map(|m| m.as_str().to_string())
            {
                mp4_name = name;
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if mp4_name.is_empty() {
            mp4_name = line.to_string();
        }
        seg_keys.push(current_key.clone());
    }
    if mp4_name.is_empty() {
        return Err(BotError::Custom(
            "Apple Music media playlist 没有 mp4 segment".to_string(),
        ));
    }
    if seg_keys.is_empty() {
        return Err(BotError::Custom(
            "Apple Music media playlist 没有 segment key".to_string(),
        ));
    }
    Ok(EnhancedHlsMedia {
        mp4_url: absolute_hls_url(media_url, &mp4_name),
        seg_keys,
    })
}

fn absolute_hls_url(base_url: &str, value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        return value.to_string();
    }
    base_url
        .rsplit_once('/')
        .map(|(base, _)| format!("{base}/{value}"))
        .unwrap_or_else(|| value.to_string())
}

#[derive(Clone, Copy, Debug)]
struct Mp4Box {
    start: usize,
    header_size: usize,
    size: usize,
    typ: [u8; 4],
}

impl Mp4Box {
    fn payload_start(self) -> usize {
        self.start + self.header_size
    }

    fn end(self) -> usize {
        self.start + self.size
    }
}

fn read_mp4_box(data: &[u8], start: usize) -> Result<Option<Mp4Box>, BotError> {
    if start >= data.len() {
        return Ok(None);
    }
    if start + 8 > data.len() {
        return Err(BotError::Custom("MP4 box header truncated".to_string()));
    }
    let mut size = u32::from_be_bytes([
        data[start],
        data[start + 1],
        data[start + 2],
        data[start + 3],
    ]) as usize;
    let mut header_size = 8;
    let typ = [
        data[start + 4],
        data[start + 5],
        data[start + 6],
        data[start + 7],
    ];
    if size == 1 {
        if start + 16 > data.len() {
            return Err(BotError::Custom(
                "MP4 largesize header truncated".to_string(),
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
        size = data.len() - start;
    }
    if size < header_size || start + size > data.len() {
        return Err(BotError::Custom(format!(
            "MP4 box {} size invalid",
            String::from_utf8_lossy(&typ)
        )));
    }
    Ok(Some(Mp4Box {
        start,
        header_size,
        size,
        typ,
    }))
}

fn find_child_box(data: &[u8], start: usize, end: usize, typ: &[u8; 4]) -> Option<Mp4Box> {
    let mut pos = start;
    while pos + 8 <= end {
        let Ok(Some(box_info)) = read_mp4_box(data, pos) else {
            return None;
        };
        if &box_info.typ == typ {
            return Some(box_info);
        }
        pos = box_info.end();
    }
    None
}

fn find_box_recursive(data: &[u8], start: usize, end: usize, typ: &[u8; 4]) -> Option<Mp4Box> {
    let mut pos = start;
    while pos + 8 <= end {
        let Ok(Some(box_info)) = read_mp4_box(data, pos) else {
            return None;
        };
        if &box_info.typ == typ {
            return Some(box_info);
        }
        if let Some(child_start) = container_child_start(box_info)
            && let Some(found) = find_box_recursive(data, child_start, box_info.end(), typ)
        {
            return Some(found);
        }
        pos = box_info.end();
    }
    None
}

fn container_child_start(box_info: Mp4Box) -> Option<usize> {
    let payload_start = box_info.payload_start();
    match &box_info.typ {
        b"stsd" => Some(payload_start + 8),
        b"enca" | b"mp4a" | b"alac" => Some(payload_start + 28),
        b"encv" => Some(payload_start + 78),
        b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" | b"sinf" | b"schi" | b"moof" | b"traf" => {
            Some(payload_start)
        }
        _ => None,
    }
}

fn parse_tenc_from_init(data: &[u8]) -> Result<TencBox, BotError> {
    let tenc_box = find_box_recursive(data, 0, data.len(), b"tenc")
        .ok_or_else(|| BotError::Custom("Apple Music fMP4 没有 tenc box".to_string()))?;
    parse_tenc(&data[tenc_box.payload_start()..tenc_box.end()])
        .map_err(|e| BotError::Custom(format!("Apple Music tenc 解析失败：{e}")))
}

#[derive(Clone, Copy, Debug, Default)]
struct FragmentDefaults {
    default_sample_size: u32,
    default_sample_duration: u32,
}

#[derive(Clone, Copy, Debug, Default)]
struct TrunSampleInfo {
    size: u32,
    duration: u32,
}

#[derive(Clone, Debug, Default)]
struct ParsedTraf {
    data_offset: Option<i32>,
    samples: Vec<TrunSampleInfo>,
    senc: Option<SencBox>,
}

fn parse_tfhd_defaults(body: &[u8]) -> Result<FragmentDefaults, BotError> {
    if body.len() < 8 {
        return Err(BotError::Custom("MP4 tfhd truncated".to_string()));
    }
    let flags = u32::from_be_bytes([0, body[1], body[2], body[3]]);
    let mut off = 8usize;
    if flags & 0x000001 != 0 {
        off += 8;
    }
    if flags & 0x000002 != 0 {
        off += 4;
    }
    let default_sample_duration = if flags & 0x000008 != 0 {
        if off + 4 > body.len() {
            return Err(BotError::Custom(
                "MP4 tfhd default duration truncated".to_string(),
            ));
        }
        let value = u32::from_be_bytes([body[off], body[off + 1], body[off + 2], body[off + 3]]);
        off += 4;
        value
    } else {
        0
    };
    let default_sample_size = if flags & 0x000010 != 0 {
        if off + 4 > body.len() {
            return Err(BotError::Custom(
                "MP4 tfhd default size truncated".to_string(),
            ));
        }
        u32::from_be_bytes([body[off], body[off + 1], body[off + 2], body[off + 3]])
    } else {
        0
    };
    Ok(FragmentDefaults {
        default_sample_size,
        default_sample_duration,
    })
}

fn parse_trun_samples(
    body: &[u8],
    defaults: FragmentDefaults,
) -> Result<(Option<i32>, Vec<TrunSampleInfo>), BotError> {
    if body.len() < 8 {
        return Err(BotError::Custom("MP4 trun truncated".to_string()));
    }
    let flags = u32::from_be_bytes([0, body[1], body[2], body[3]]);
    let sample_count = u32::from_be_bytes([body[4], body[5], body[6], body[7]]) as usize;
    let mut off = 8usize;
    let data_offset = if flags & 0x000001 != 0 {
        if off + 4 > body.len() {
            return Err(BotError::Custom(
                "MP4 trun data_offset truncated".to_string(),
            ));
        }
        let value = i32::from_be_bytes([body[off], body[off + 1], body[off + 2], body[off + 3]]);
        off += 4;
        Some(value)
    } else {
        None
    };
    if flags & 0x000004 != 0 {
        off += 4;
    }
    let mut samples = Vec::with_capacity(sample_count);
    for _ in 0..sample_count {
        let duration = if flags & 0x000100 != 0 {
            if off + 4 > body.len() {
                return Err(BotError::Custom(
                    "MP4 trun sample duration truncated".to_string(),
                ));
            }
            let value =
                u32::from_be_bytes([body[off], body[off + 1], body[off + 2], body[off + 3]]);
            off += 4;
            value
        } else {
            defaults.default_sample_duration
        };
        let size = if flags & 0x000200 != 0 {
            if off + 4 > body.len() {
                return Err(BotError::Custom(
                    "MP4 trun sample size truncated".to_string(),
                ));
            }
            let value =
                u32::from_be_bytes([body[off], body[off + 1], body[off + 2], body[off + 3]]);
            off += 4;
            value
        } else {
            defaults.default_sample_size
        };
        if flags & 0x000400 != 0 {
            off += 4;
        }
        if flags & 0x000800 != 0 {
            off += 4;
        }
        if size == 0 {
            return Err(BotError::Custom("MP4 trun sample size 缺失".to_string()));
        }
        samples.push(TrunSampleInfo { size, duration });
    }
    Ok((data_offset, samples))
}

fn parse_traf(data: &[u8], traf: Mp4Box, tenc: &TencBox) -> Result<ParsedTraf, BotError> {
    let tfhd = find_child_box(data, traf.payload_start(), traf.end(), b"tfhd")
        .ok_or_else(|| BotError::Custom("MP4 traf 缺少 tfhd".to_string()))?;
    let defaults = parse_tfhd_defaults(&data[tfhd.payload_start()..tfhd.end()])?;
    let trun = find_child_box(data, traf.payload_start(), traf.end(), b"trun")
        .ok_or_else(|| BotError::Custom("MP4 traf 缺少 trun".to_string()))?;
    let (data_offset, samples) =
        parse_trun_samples(&data[trun.payload_start()..trun.end()], defaults)?;
    let senc = find_child_box(data, traf.payload_start(), traf.end(), b"senc")
        .map(|senc| {
            parse_senc(
                &data[senc.payload_start()..senc.end()],
                tenc.default_per_sample_iv_size,
            )
            .map_err(|e| BotError::Custom(format!("Apple Music senc 解析失败：{e}")))
        })
        .transpose()?;
    Ok(ParsedTraf {
        data_offset,
        samples,
        senc,
    })
}

async fn decrypt_fmp4_with_wrapper(
    host: &str,
    track_id: &str,
    mut data: Vec<u8>,
    seg_keys: &[String],
) -> Result<AppleWrapperDecryptOutput, BotError> {
    let tenc = parse_tenc_from_init(&data)?;
    let addr = format!("{host}:{APPLE_WRAPPER_DECRYPT_PORT}");
    let commands = collect_wrapper_decrypt_commands(&data, track_id, seg_keys, &tenc)?;

    let _track_permit = APPLE_WRAPPER_TRACK_DECRYPT_SEMAPHORE
        .acquire()
        .await
        .map_err(|_| BotError::Custom("Apple Music wrapper track semaphore closed".into()))?;
    let started = Instant::now();
    run_wrapper_decrypt_fragment_parallel(
        &addr,
        &commands,
        &mut data,
        SETTINGS
            .music
            .applemusic
            .wrapper_fragment_concurrency
            .max(1),
    )
    .await?;
    let decrypt_elapsed = started.elapsed();

    Ok(AppleWrapperDecryptOutput {
        audio: remux_decrypted_fmp4_to_progressive(&data)?,
        decrypt_elapsed,
    })
}

#[derive(Clone)]
enum AppleWrapperDecryptCommand {
    Key { adam: String, uri: String },
    Job(AppleWrapperDecryptJob),
}

#[derive(Clone)]
struct AppleWrapperDecryptJob {
    payload: Vec<u8>,
    ranges: Vec<Range<usize>>,
}

fn collect_wrapper_decrypt_commands(
    data: &[u8],
    track_id: &str,
    seg_keys: &[String],
    tenc: &TencBox,
) -> Result<Vec<AppleWrapperDecryptCommand>, BotError> {
    let mut commands = Vec::new();
    let mut pos = 0usize;
    let mut fragment_index = 0usize;
    while let Some(box_info) = read_mp4_box(data, pos)? {
        if &box_info.typ != b"moof" {
            pos = box_info.end();
            continue;
        }
        let moof = box_info;
        let mdat = read_mp4_box(data, moof.end())?
            .filter(|next| &next.typ == b"mdat")
            .ok_or_else(|| BotError::Custom("MP4 moof 后缺少 mdat".to_string()))?;
        let key_uri = seg_keys
            .get(fragment_index)
            .ok_or_else(|| BotError::Custom("Apple Music segment key 数量不足".to_string()))?;
        let adam = if key_uri == APPLE_PREFETCH_KEY_URI {
            "0"
        } else {
            track_id
        };
        commands.push(AppleWrapperDecryptCommand::Key {
            adam: adam.to_string(),
            uri: key_uri.to_string(),
        });

        let mut traf_pos = moof.payload_start();
        while let Some(traf) = read_mp4_box(data, traf_pos)? {
            if traf.end() > moof.end() {
                break;
            }
            if &traf.typ == b"traf" {
                let parsed = parse_traf(data, traf, tenc)?;
                let start = parsed
                    .data_offset
                    .map(|offset| (moof.start as i64 + offset as i64) as usize)
                    .unwrap_or_else(|| mdat.payload_start());
                collect_wrapper_decrypt_sample_jobs(
                    &mut commands,
                    data,
                    start,
                    &parsed.samples,
                    parsed.senc.as_ref(),
                    tenc,
                )?;
            }
            traf_pos = traf.end();
        }

        fragment_index += 1;
        pos = mdat.end();
    }
    Ok(commands)
}

#[derive(Clone)]
struct AppleWrapperDecryptFragmentGroup {
    adam: String,
    uri: String,
    jobs: Vec<AppleWrapperDecryptJob>,
}

struct AppleWrapperDecryptFragmentResult {
    jobs: Vec<AppleWrapperDecryptFragmentJobResult>,
}

struct AppleWrapperDecryptFragmentJobResult {
    ranges: Vec<Range<usize>>,
    decrypted: Vec<u8>,
}

fn group_wrapper_commands_by_fragment(
    commands: &[AppleWrapperDecryptCommand],
) -> Result<Vec<AppleWrapperDecryptFragmentGroup>, BotError> {
    let mut groups = Vec::new();
    let mut current: Option<AppleWrapperDecryptFragmentGroup> = None;
    for command in commands {
        match command {
            AppleWrapperDecryptCommand::Key { adam, uri } => {
                if let Some(group) = current.take() {
                    groups.push(group);
                }
                current = Some(AppleWrapperDecryptFragmentGroup {
                    adam: adam.clone(),
                    uri: uri.clone(),
                    jobs: Vec::new(),
                });
            }
            AppleWrapperDecryptCommand::Job(job) => {
                let group = current.as_mut().ok_or_else(|| {
                    BotError::Custom("Apple Music wrapper fragment 缺少 key".to_string())
                })?;
                group.jobs.push(job.clone());
            }
        }
    }
    if let Some(group) = current {
        groups.push(group);
    }
    Ok(groups)
}

async fn decrypt_fragment_group_over_wrapper(
    addr: String,
    group: AppleWrapperDecryptFragmentGroup,
) -> Result<AppleWrapperDecryptFragmentResult, BotError> {
    let _global_permit = APPLE_WRAPPER_DECRYPT_SEMAPHORE
        .acquire()
        .await
        .map_err(|_| BotError::Custom("Apple Music wrapper decrypt semaphore closed".into()))?;
    let stream = tokio::time::timeout(apple_timeout(), tokio::net::TcpStream::connect(&addr))
        .await
        .map_err(|_| BotError::Custom(format!("Apple Music wrapper decrypt 连接超时：{addr}")))??;
    stream.set_nodelay(true)?;
    let mut stream = stream;
    write_len_prefixed(&mut stream, &group.adam).await?;
    write_len_prefixed(&mut stream, &group.uri).await?;

    let mut decrypted = Vec::new();
    let mut out = Vec::with_capacity(group.jobs.len());
    for job in group.jobs {
        let len = u32::try_from(job.payload.len()).map_err(|_| {
            BotError::Custom("Apple Music wrapper decrypt payload 过大".to_string())
        })?;
        wrapper_write_all(&mut stream, &len.to_le_bytes(), "fragment decrypt length").await?;
        wrapper_write_all(&mut stream, &job.payload, "fragment decrypt payload").await?;
        wrapper_flush(&mut stream, "fragment decrypt").await?;
        decrypted.resize(job.payload.len(), 0);
        wrapper_read_exact(&mut stream, &mut decrypted, "fragment decrypt result").await?;
        out.push(AppleWrapperDecryptFragmentJobResult {
            ranges: job.ranges,
            decrypted: decrypted.clone(),
        });
    }
    wrapper_write_all(&mut stream, &[0, 0, 0, 0, 0], "fragment decrypt finish").await?;
    wrapper_flush(&mut stream, "fragment decrypt finish").await?;
    Ok(AppleWrapperDecryptFragmentResult { jobs: out })
}

async fn run_wrapper_decrypt_fragment_parallel(
    addr: &str,
    commands: &[AppleWrapperDecryptCommand],
    data: &mut [u8],
    parallelism: usize,
) -> Result<(), BotError> {
    let groups = group_wrapper_commands_by_fragment(commands)?;
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(parallelism.max(1)));
    let mut join_set = tokio::task::JoinSet::new();

    for group in groups {
        let addr = addr.to_string();
        let semaphore = semaphore.clone();
        join_set.spawn(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|_| BotError::Custom("Apple Music fragment semaphore closed".into()))?;
            decrypt_fragment_group_over_wrapper(addr, group).await
        });
    }

    while let Some(result) = join_set.join_next().await {
        let fragment = result.map_err(|err| {
            BotError::Custom(format!("Apple Music fragment decrypt task failed: {err}"))
        })??;
        for job in fragment.jobs {
            let mut src = 0usize;
            for range in job.ranges {
                let len = range.end - range.start;
                data[range].copy_from_slice(&job.decrypted[src..src + len]);
                src += len;
            }
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct ProgressiveSample {
    data: Vec<u8>,
    duration: u32,
    size: u32,
}

fn remux_decrypted_fmp4_to_progressive(data: &[u8]) -> Result<Vec<u8>, BotError> {
    let ftyp = find_child_box(data, 0, data.len(), b"ftyp")
        .ok_or_else(|| BotError::Custom("MP4 缺少 ftyp".to_string()))?;
    let moov = find_child_box(data, 0, data.len(), b"moov")
        .ok_or_else(|| BotError::Custom("MP4 缺少 moov".to_string()))?;
    let tenc = parse_tenc_from_init(data)?;
    let samples = collect_fragment_samples(data, &tenc)?;
    if samples.is_empty() {
        return Err(BotError::Custom("MP4 fragments 没有 samples".to_string()));
    }

    let movie_timescale = find_box_recursive(data, moov.payload_start(), moov.end(), b"mvhd")
        .and_then(|mvhd| parse_mvhd_timescale(&data[mvhd.payload_start()..mvhd.end()]))
        .unwrap_or(1000);
    let media_timescale = find_box_recursive(data, moov.payload_start(), moov.end(), b"mdhd")
        .and_then(|mdhd| parse_mdhd_timescale(&data[mdhd.payload_start()..mdhd.end()]))
        .unwrap_or(movie_timescale);
    let total_media_duration = samples
        .iter()
        .map(|sample| sample.duration as u64)
        .sum::<u64>();
    let total_movie_duration = if media_timescale > 0 {
        total_media_duration.saturating_mul(movie_timescale as u64) / media_timescale as u64
    } else {
        total_media_duration
    };

    let mut moov_bytes = rewrite_box_for_progressive(
        data,
        moov,
        &samples,
        total_media_duration,
        total_movie_duration,
    )?;
    let ftyp_bytes = data[ftyp.start..ftyp.end()].to_vec();
    let mdat_payload_offset = ftyp_bytes.len() + moov_bytes.len() + 8;
    patch_first_stco_offset(&mut moov_bytes, mdat_payload_offset as u32)?;

    let mdat_payload_size = samples
        .iter()
        .map(|sample| sample.data.len())
        .sum::<usize>();
    let mut out = Vec::with_capacity(ftyp_bytes.len() + moov_bytes.len() + 8 + mdat_payload_size);
    out.extend_from_slice(&ftyp_bytes);
    out.extend_from_slice(&moov_bytes);
    let mdat_size = 8usize + mdat_payload_size;
    out.extend_from_slice(&(mdat_size as u32).to_be_bytes());
    out.extend_from_slice(b"mdat");
    for sample in samples {
        out.extend_from_slice(&sample.data);
    }
    fix_alac_packets_in_progressive(&mut out)?;
    Ok(out)
}

fn collect_fragment_samples(
    data: &[u8],
    tenc: &TencBox,
) -> Result<Vec<ProgressiveSample>, BotError> {
    let mut samples = Vec::new();
    let mut pos = 0usize;
    while let Some(box_info) = read_mp4_box(data, pos)? {
        if &box_info.typ != b"moof" {
            pos = box_info.end();
            continue;
        }
        let moof = box_info;
        let mdat = read_mp4_box(data, moof.end())?
            .filter(|next| &next.typ == b"mdat")
            .ok_or_else(|| BotError::Custom("MP4 moof 后缺少 mdat".to_string()))?;
        let mut traf_pos = moof.payload_start();
        while let Some(traf) = read_mp4_box(data, traf_pos)? {
            if traf.end() > moof.end() {
                break;
            }
            if &traf.typ == b"traf" {
                let parsed = parse_traf(data, traf, tenc)?;
                let mut sample_pos = parsed
                    .data_offset
                    .map(|offset| (moof.start as i64 + offset as i64) as usize)
                    .unwrap_or_else(|| mdat.payload_start());
                for sample in parsed.samples {
                    let end = sample_pos + sample.size as usize;
                    if end > data.len() {
                        return Err(BotError::Custom("MP4 sample 超出文件范围".to_string()));
                    }
                    samples.push(ProgressiveSample {
                        data: data[sample_pos..end].to_vec(),
                        duration: sample.duration,
                        size: sample.size,
                    });
                    sample_pos = end;
                }
            }
            traf_pos = traf.end();
        }
        pos = mdat.end();
    }
    Ok(samples)
}

fn rewrite_box_for_progressive(
    data: &[u8],
    box_info: Mp4Box,
    samples: &[ProgressiveSample],
    total_media_duration: u64,
    total_movie_duration: u64,
) -> Result<Vec<u8>, BotError> {
    match &box_info.typ {
        b"moov" | b"trak" | b"mdia" | b"minf" => {
            let mut payload = Vec::new();
            let mut pos = box_info.payload_start();
            while let Some(child) = read_mp4_box(data, pos)? {
                if child.end() > box_info.end() {
                    break;
                }
                if (&child.typ == b"mvex" && &box_info.typ == b"moov")
                    || (&child.typ == b"edts" && &box_info.typ == b"trak")
                {
                    pos = child.end();
                    continue;
                }
                payload.extend_from_slice(&rewrite_box_for_progressive(
                    data,
                    child,
                    samples,
                    total_media_duration,
                    total_movie_duration,
                )?);
                pos = child.end();
            }
            Ok(build_box(&box_info.typ, payload))
        }
        b"stbl" => build_progressive_stbl(data, box_info, samples),
        b"mvhd" => {
            let mut payload = data[box_info.payload_start()..box_info.end()].to_vec();
            patch_mvhd_duration(&mut payload, total_movie_duration);
            Ok(build_box(b"mvhd", payload))
        }
        b"mdhd" => {
            let mut payload = data[box_info.payload_start()..box_info.end()].to_vec();
            patch_mdhd_duration(&mut payload, total_media_duration);
            Ok(build_box(b"mdhd", payload))
        }
        b"tkhd" => {
            let mut payload = data[box_info.payload_start()..box_info.end()].to_vec();
            patch_tkhd_duration(&mut payload, total_movie_duration);
            Ok(build_box(b"tkhd", payload))
        }
        _ => Ok(data[box_info.start..box_info.end()].to_vec()),
    }
}

fn build_progressive_stbl(
    data: &[u8],
    stbl: Mp4Box,
    samples: &[ProgressiveSample],
) -> Result<Vec<u8>, BotError> {
    let stsd = find_child_box(data, stbl.payload_start(), stbl.end(), b"stsd")
        .ok_or_else(|| BotError::Custom("MP4 stbl 缺少 stsd".to_string()))?;
    let mut payload = Vec::new();
    payload.extend_from_slice(&sanitize_stsd(data, stsd)?);
    payload.extend_from_slice(&build_stts(samples));
    payload.extend_from_slice(&build_stsc(samples.len() as u32));
    payload.extend_from_slice(&build_stsz(samples));
    payload.extend_from_slice(&build_stco(0));
    Ok(build_box(b"stbl", payload))
}

fn sanitize_stsd(data: &[u8], stsd: Mp4Box) -> Result<Vec<u8>, BotError> {
    let payload = &data[stsd.payload_start()..stsd.end()];
    if payload.len() < 16 {
        return Err(BotError::Custom("MP4 stsd truncated".to_string()));
    }
    let entry = read_mp4_box(data, stsd.payload_start() + 8)?
        .ok_or_else(|| BotError::Custom("MP4 stsd 缺少 sample entry".to_string()))?;
    let sanitized_entry = sanitize_sample_entry(data, entry)?;
    let mut out_payload = payload[..8].to_vec();
    out_payload.extend_from_slice(&sanitized_entry);
    Ok(build_box(b"stsd", out_payload))
}

fn sanitize_sample_entry(data: &[u8], entry: Mp4Box) -> Result<Vec<u8>, BotError> {
    let mut typ = entry.typ;
    if &entry.typ == b"enca" || &entry.typ == b"encv" {
        if let Some(frma) = find_box_recursive(data, entry.payload_start(), entry.end(), b"frma") {
            let body = &data[frma.payload_start()..frma.end()];
            if body.len() >= 4 {
                typ.copy_from_slice(&body[..4]);
            } else if &entry.typ == b"enca" {
                typ.copy_from_slice(b"alac");
            }
        } else if &entry.typ == b"enca" {
            typ.copy_from_slice(b"alac");
        }
    }

    let fixed = match &entry.typ {
        b"enca" | b"mp4a" | b"alac" => 28,
        b"encv" => 78,
        _ => entry.end().saturating_sub(entry.payload_start()),
    };
    let fixed_end = (entry.payload_start() + fixed).min(entry.end());
    let mut payload = data[entry.payload_start()..fixed_end].to_vec();
    let mut pos = fixed_end;
    while let Some(child) = read_mp4_box(data, pos)? {
        if child.end() > entry.end() {
            break;
        }
        if &child.typ != b"sinf" {
            payload.extend_from_slice(&data[child.start..child.end()]);
        }
        pos = child.end();
    }
    Ok(build_box(&typ, payload))
}

fn build_stts(samples: &[ProgressiveSample]) -> Vec<u8> {
    let mut entries: Vec<(u32, u32)> = Vec::new();
    for sample in samples {
        if let Some((count, duration)) = entries.last_mut()
            && *duration == sample.duration
        {
            *count += 1;
            continue;
        }
        entries.push((1, sample.duration));
    }
    let mut payload = vec![0, 0, 0, 0];
    payload.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for (count, duration) in entries {
        payload.extend_from_slice(&count.to_be_bytes());
        payload.extend_from_slice(&duration.to_be_bytes());
    }
    build_box(b"stts", payload)
}

fn build_stsc(sample_count: u32) -> Vec<u8> {
    let mut payload = vec![0, 0, 0, 0];
    payload.extend_from_slice(&1u32.to_be_bytes());
    payload.extend_from_slice(&1u32.to_be_bytes());
    payload.extend_from_slice(&sample_count.to_be_bytes());
    payload.extend_from_slice(&1u32.to_be_bytes());
    build_box(b"stsc", payload)
}

fn build_stsz(samples: &[ProgressiveSample]) -> Vec<u8> {
    let mut payload = vec![0, 0, 0, 0];
    payload.extend_from_slice(&0u32.to_be_bytes());
    payload.extend_from_slice(&(samples.len() as u32).to_be_bytes());
    for sample in samples {
        payload.extend_from_slice(&sample.size.to_be_bytes());
    }
    build_box(b"stsz", payload)
}

fn build_stco(offset: u32) -> Vec<u8> {
    let mut payload = vec![0, 0, 0, 0];
    payload.extend_from_slice(&1u32.to_be_bytes());
    payload.extend_from_slice(&offset.to_be_bytes());
    build_box(b"stco", payload)
}

fn build_box(typ: &[u8; 4], payload: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 8);
    out.extend_from_slice(&((payload.len() + 8) as u32).to_be_bytes());
    out.extend_from_slice(typ);
    out.extend_from_slice(&payload);
    out
}

fn parse_mvhd_timescale(payload: &[u8]) -> Option<u32> {
    match payload.first().copied()? {
        0 if payload.len() >= 20 => Some(u32::from_be_bytes([
            payload[12],
            payload[13],
            payload[14],
            payload[15],
        ])),
        _ if payload.len() >= 32 => Some(u32::from_be_bytes([
            payload[20],
            payload[21],
            payload[22],
            payload[23],
        ])),
        _ => None,
    }
}

fn parse_mdhd_timescale(payload: &[u8]) -> Option<u32> {
    parse_mvhd_timescale(payload)
}

fn patch_mvhd_duration(payload: &mut [u8], duration: u64) {
    match payload.first().copied() {
        Some(0) if payload.len() >= 20 => {
            payload[16..20].copy_from_slice(&(duration as u32).to_be_bytes());
        }
        Some(_) if payload.len() >= 32 => {
            payload[24..32].copy_from_slice(&duration.to_be_bytes());
        }
        _ => {}
    }
}

fn patch_mdhd_duration(payload: &mut [u8], duration: u64) {
    patch_mvhd_duration(payload, duration);
}

fn patch_tkhd_duration(payload: &mut [u8], duration: u64) {
    match payload.first().copied() {
        Some(0) if payload.len() >= 24 => {
            payload[20..24].copy_from_slice(&(duration as u32).to_be_bytes());
        }
        Some(_) if payload.len() >= 36 => {
            payload[28..36].copy_from_slice(&duration.to_be_bytes());
        }
        _ => {}
    }
}

fn patch_first_stco_offset(moov: &mut [u8], offset: u32) -> Result<(), BotError> {
    let stco = find_box_recursive(moov, 0, moov.len(), b"stco")
        .ok_or_else(|| BotError::Custom("progressive MP4 缺少 stco".to_string()))?;
    let patch = stco.payload_start() + 8;
    if patch + 4 > stco.end() {
        return Err(BotError::Custom(
            "progressive MP4 stco truncated".to_string(),
        ));
    }
    moov[patch..patch + 4].copy_from_slice(&offset.to_be_bytes());
    Ok(())
}

#[derive(Clone, Copy, Debug)]
struct AlacParams {
    max_samples_per_frame: u32,
    sample_size: u8,
    rice_history_mult: u8,
    rice_initial_history: u8,
    rice_limit: u8,
    channels: u8,
}

#[derive(Clone, Copy, Debug)]
struct AlacPacketLoc {
    offset: usize,
    size: usize,
}

#[derive(Debug)]
struct AlacBitReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> AlacBitReader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn left(&self) -> usize {
        self.buf.len() * 8 - self.pos
    }

    fn read(&mut self, n: usize) -> Option<u32> {
        if self.pos + n > self.buf.len() * 8 {
            return None;
        }
        let mut value = 0u32;
        for _ in 0..n {
            value = (value << 1) | ((self.buf[self.pos >> 3] >> (7 - (self.pos & 7))) & 1) as u32;
            self.pos += 1;
        }
        Some(value)
    }

    fn show(&mut self, n: usize) -> Option<u32> {
        let save = self.pos;
        let value = self.read(n);
        self.pos = save;
        value
    }

    fn skip(&mut self, n: usize) -> bool {
        if self.pos + n > self.buf.len() * 8 {
            return false;
        }
        self.pos += n;
        true
    }

    fn read_signed(&mut self, n: usize) -> Option<i32> {
        let value = self.read(n)?;
        if value & (1 << (n - 1)) != 0 {
            Some(value as i32 - (1i32 << n))
        } else {
            Some(value as i32)
        }
    }

    fn unary09(&mut self) -> Option<u32> {
        let mut count = 0u32;
        while count < 9 {
            if self.read(1)? == 0 {
                return Some(count);
            }
            count += 1;
        }
        Some(9)
    }
}

fn fix_alac_packets_in_progressive(data: &mut [u8]) -> Result<(), BotError> {
    let Some(params) = find_alac_params(data)? else {
        return Ok(());
    };
    let locs = find_progressive_packet_locations(data)?;
    for loc in locs {
        if loc.offset + loc.size > data.len() {
            continue;
        }
        let packet = &data[loc.offset..loc.offset + loc.size];
        let body_end = alac_find_body_end_bit(packet, &params);
        if body_end < 0 || body_end as usize == loc.size * 8 {
            continue;
        }
        let mut reader = AlacBitReader::new(packet);
        if !reader.skip(body_end as usize) {
            continue;
        }
        if reader.left() >= 3 && reader.show(3) == Some(7) {
            continue;
        }
        alac_patch_in_place(data, loc.offset, loc.size, body_end as usize);
    }
    Ok(())
}

fn find_alac_params(data: &[u8]) -> Result<Option<AlacParams>, BotError> {
    let Some(stsd) = find_box_recursive(data, 0, data.len(), b"stsd") else {
        return Ok(None);
    };
    if stsd.payload_start() + 16 > stsd.end() {
        return Ok(None);
    }
    let Some(entry) = read_mp4_box(data, stsd.payload_start() + 8)? else {
        return Ok(None);
    };
    if &entry.typ != b"alac" {
        return Ok(None);
    }
    let child_start = entry.payload_start() + 28;
    let Some(config) = find_child_box(data, child_start, entry.end(), b"alac") else {
        return Ok(None);
    };
    let body = &data[config.payload_start()..config.end()];
    if body.len() < 24 {
        return Err(BotError::Custom("ALAC config too short".to_string()));
    }
    Ok(Some(AlacParams {
        max_samples_per_frame: u32::from_be_bytes([body[4], body[5], body[6], body[7]]),
        sample_size: body[9],
        rice_history_mult: body[10],
        rice_initial_history: body[11],
        rice_limit: body[12],
        channels: body[13],
    }))
}

fn find_progressive_packet_locations(data: &[u8]) -> Result<Vec<AlacPacketLoc>, BotError> {
    let stsz = find_box_recursive(data, 0, data.len(), b"stsz")
        .ok_or_else(|| BotError::Custom("progressive MP4 缺少 stsz".to_string()))?;
    let stco = find_box_recursive(data, 0, data.len(), b"stco")
        .ok_or_else(|| BotError::Custom("progressive MP4 缺少 stco".to_string()))?;
    let stsz_body = &data[stsz.payload_start()..stsz.end()];
    if stsz_body.len() < 12 {
        return Err(BotError::Custom(
            "progressive MP4 stsz truncated".to_string(),
        ));
    }
    let default_size = u32::from_be_bytes([stsz_body[4], stsz_body[5], stsz_body[6], stsz_body[7]]);
    let sample_count =
        u32::from_be_bytes([stsz_body[8], stsz_body[9], stsz_body[10], stsz_body[11]]) as usize;
    let mut sizes = Vec::with_capacity(sample_count);
    if default_size == 0 {
        let mut pos = 12usize;
        for _ in 0..sample_count {
            if pos + 4 > stsz_body.len() {
                return Err(BotError::Custom(
                    "progressive MP4 stsz sizes truncated".to_string(),
                ));
            }
            sizes.push(u32::from_be_bytes([
                stsz_body[pos],
                stsz_body[pos + 1],
                stsz_body[pos + 2],
                stsz_body[pos + 3],
            ]) as usize);
            pos += 4;
        }
    } else {
        sizes.resize(sample_count, default_size as usize);
    }

    let stco_body = &data[stco.payload_start()..stco.end()];
    if stco_body.len() < 12 {
        return Err(BotError::Custom(
            "progressive MP4 stco truncated".to_string(),
        ));
    }
    let chunk_offset =
        u32::from_be_bytes([stco_body[8], stco_body[9], stco_body[10], stco_body[11]]) as usize;
    let mut offset = chunk_offset;
    Ok(sizes
        .into_iter()
        .map(|size| {
            let loc = AlacPacketLoc { offset, size };
            offset += size;
            loc
        })
        .collect())
}

fn alac_find_body_end_bit(packet: &[u8], params: &AlacParams) -> isize {
    let mut reader = AlacBitReader::new(packet);
    let mut channels_used = 0usize;
    let mut last_end = -1isize;
    while reader.left() >= 3 {
        let Some((channels, is_end)) = alac_scan_one_element(&mut reader, params) else {
            return -1;
        };
        if is_end {
            return reader.pos as isize;
        }
        last_end = reader.pos as isize;
        channels_used += channels;
        if channels_used >= params.channels as usize {
            return last_end;
        }
    }
    last_end
}

fn alac_scan_one_element(
    reader: &mut AlacBitReader<'_>,
    params: &AlacParams,
) -> Option<(usize, bool)> {
    let elem = reader.read(3)?;
    if elem == 7 {
        return Some((0, true));
    }
    if elem > 1 && elem != 3 {
        return None;
    }
    let channels = if elem == 1 { 2usize } else { 1usize };
    reader.skip(4).then_some(())?;
    reader.skip(12).then_some(())?;
    let has_size = reader.read(1)?;
    let extra_bits = (reader.read(2)? as usize) << 3;
    let bps = params.sample_size as isize - extra_bits as isize + channels as isize - 1;
    if !(1..=32).contains(&bps) {
        return None;
    }
    let is_compressed = reader.read(1)? == 0;
    let output_samples = if has_size != 0 {
        reader.read(32)?
    } else {
        params.max_samples_per_frame
    };
    if output_samples == 0 || output_samples > params.max_samples_per_frame {
        return None;
    }
    if is_compressed {
        reader.read(8)?;
        reader.read(8)?;
        let mut rhms = Vec::with_capacity(channels);
        for _ in 0..channels {
            reader.read(4)?;
            let lpc_quant = reader.read(4)?;
            let rhm = reader.read(3)?;
            let lpc_order = reader.read(5)?;
            if lpc_order >= params.max_samples_per_frame || lpc_quant == 0 {
                return None;
            }
            for _ in 0..lpc_order {
                reader.read_signed(16)?;
            }
            rhms.push(rhm);
        }
        if extra_bits != 0 {
            reader
                .skip(output_samples as usize * channels * extra_bits)
                .then_some(())?;
        }
        for rhm in rhms {
            let rhm_eff = (rhm * params.rice_history_mult as u32) / 4;
            alac_rice_decompress(
                reader,
                output_samples as usize,
                bps as usize,
                rhm_eff,
                params,
            )?;
        }
    } else {
        reader
            .skip(output_samples as usize * channels * params.sample_size as usize)
            .then_some(())?;
    }
    Some((channels, false))
}

fn alac_rice_decompress(
    reader: &mut AlacBitReader<'_>,
    nb_samples: usize,
    bps: usize,
    rhm_eff: u32,
    params: &AlacParams,
) -> Option<()> {
    let mut history = params.rice_initial_history as u32;
    let mut sign_mod = 0u32;
    let limit = params.rice_limit as usize;
    let mut i = 0usize;
    let mut iters = 0usize;
    while i < nb_samples {
        iters += 1;
        if iters > nb_samples * 4 + 100 || reader.left() == 0 {
            return None;
        }
        let mut k = alac_log2((history >> 9) + 3);
        if k > limit {
            k = limit;
        }
        let mut x = alac_decode_scalar(reader, k, bps)? + sign_mod;
        sign_mod = 0;
        if x > 0xffff {
            x = 0xffff;
        }
        history = history + x * rhm_eff - ((history * rhm_eff) >> 9);
        if history < 128 && i + 1 < nb_samples {
            let mut k2 = 7usize
                .saturating_sub(alac_log2(history))
                .saturating_add(((history + 16) >> 6) as usize);
            if k2 > limit {
                k2 = limit;
            }
            let block_size = alac_decode_scalar(reader, k2, 16)?;
            if block_size > 0 {
                i += (block_size as usize).min(nb_samples - i - 1);
            }
            if block_size <= 0xffff {
                sign_mod = 1;
            }
            history = 0;
        }
        i += 1;
    }
    Some(())
}

fn alac_decode_scalar(reader: &mut AlacBitReader<'_>, k: usize, bps: usize) -> Option<u32> {
    let mut x = reader.unary09()?;
    if x > 8 {
        return reader.read(bps);
    }
    if k != 1 {
        let extra_bits = reader.show(k)?;
        x = (x << k) - x;
        if extra_bits > 1 {
            x += extra_bits - 1;
            reader.skip(k).then_some(())?;
        } else {
            reader.skip(k - 1).then_some(())?;
        }
    }
    Some(x)
}

fn alac_log2(mut value: u32) -> usize {
    let mut result = 0usize;
    while value > 1 {
        value >>= 1;
        result += 1;
    }
    result
}

fn alac_patch_in_place(data: &mut [u8], offset: usize, size: usize, body_end_bit: usize) -> bool {
    let total_bits = size * 8;
    if body_end_bit + 3 > total_bits || offset + size > data.len() {
        return false;
    }
    for i in 0..3 {
        let bit_pos = body_end_bit + i;
        let byte_index = offset + (bit_pos >> 3);
        let mask = 1u8 << (7 - (bit_pos & 7));
        data[byte_index] |= mask;
    }
    let pad_start = body_end_bit + 3;
    let mut byte_index = offset + (pad_start >> 3);
    let bit_in_byte = pad_start & 7;
    if bit_in_byte != 0 {
        let keep = 0xffu8 << (8 - bit_in_byte);
        data[byte_index] &= keep;
        byte_index += 1;
    }
    for byte in &mut data[byte_index..offset + size] {
        *byte = 0;
    }
    true
}

async fn write_len_prefixed<W>(stream: &mut W, value: &str) -> Result<(), BotError>
where
    W: AsyncWrite + Unpin,
{
    let bytes = value.as_bytes();
    if bytes.len() > u8::MAX as usize {
        return Err(BotError::Custom(
            "Apple Music wrapper string 过长".to_string(),
        ));
    }
    wrapper_write_all(stream, &[bytes.len() as u8], "length-prefixed header").await?;
    wrapper_write_all(stream, bytes, "length-prefixed payload").await?;
    Ok(())
}

fn collect_wrapper_decrypt_sample_jobs(
    commands: &mut Vec<AppleWrapperDecryptCommand>,
    data: &[u8],
    sample_start: usize,
    samples: &[TrunSampleInfo],
    senc: Option<&SencBox>,
    tenc: &TencBox,
) -> Result<(), BotError> {
    let mut pos = sample_start;
    for (index, sample) in samples.iter().enumerate() {
        let end = pos + sample.size as usize;
        if end > data.len() {
            return Err(BotError::Custom("MP4 sample 超出文件范围".to_string()));
        }
        let subsamples = senc
            .and_then(|senc| senc.samples.get(index))
            .map(|sample| sample.subsamples.as_slice())
            .unwrap_or(&[]);
        collect_wrapper_decrypt_sample_job(
            commands,
            data,
            pos,
            sample.size as usize,
            subsamples,
            tenc,
        )?;
        pos = end;
    }
    Ok(())
}

fn collect_wrapper_decrypt_sample_job(
    commands: &mut Vec<AppleWrapperDecryptCommand>,
    data: &[u8],
    sample_start: usize,
    sample_size: usize,
    subsamples: &[SubsampleEntry],
    tenc: &TencBox,
) -> Result<(), BotError> {
    if subsamples.is_empty() {
        return collect_wrapper_decrypt_raw_job(commands, data, sample_start, sample_size, tenc);
    }
    let mut pos = 0usize;
    for subsample in subsamples {
        pos += subsample.bytes_of_clear_data as usize;
        let end = pos + subsample.bytes_of_protected_data as usize;
        if end > sample_size {
            return Err(BotError::Custom(
                "MP4 subsample protected range 超出 sample".to_string(),
            ));
        }
        collect_wrapper_decrypt_raw_job(commands, data, sample_start + pos, end - pos, tenc)?;
        pos = end;
    }
    Ok(())
}

fn collect_wrapper_decrypt_raw_job(
    commands: &mut Vec<AppleWrapperDecryptCommand>,
    data: &[u8],
    start: usize,
    size: usize,
    tenc: &TencBox,
) -> Result<(), BotError> {
    let decrypt_block_len = tenc.default_crypt_byte_block as usize * 16;
    let skip_block_len = tenc.default_skip_byte_block as usize * 16;
    if skip_block_len == 0 {
        let len = size & !0x0f;
        if len == 0 {
            return Ok(());
        }
        let end = start + len;
        if end > data.len() {
            return Err(BotError::Custom(
                "MP4 decrypt range 超出文件范围".to_string(),
            ));
        }
        commands.push(AppleWrapperDecryptCommand::Job(AppleWrapperDecryptJob {
            payload: data[start..end].to_vec(),
            ranges: std::iter::once(start..end).collect(),
        }));
        return Ok(());
    }

    if size < decrypt_block_len {
        return Ok(());
    }
    let mut ranges = Vec::new();
    let mut pos = 0usize;
    while size.saturating_sub(pos) >= decrypt_block_len {
        ranges.push(start + pos..start + pos + decrypt_block_len);
        pos += decrypt_block_len;
        if size.saturating_sub(pos) < skip_block_len {
            break;
        }
        pos += skip_block_len;
    }
    let total_len = ranges
        .iter()
        .map(|range| range.end - range.start)
        .sum::<usize>();
    if total_len == 0 {
        return Ok(());
    }
    let mut payload = Vec::with_capacity(total_len);
    for range in &ranges {
        if range.end > data.len() {
            return Err(BotError::Custom(
                "MP4 decrypt range 超出文件范围".to_string(),
            ));
        }
        payload.extend_from_slice(&data[range.clone()]);
    }
    commands.push(AppleWrapperDecryptCommand::Job(AppleWrapperDecryptJob {
        payload,
        ranges,
    }));
    Ok(())
}

async fn wrapper_write_all<W>(stream: &mut W, data: &[u8], context: &str) -> Result<(), BotError>
where
    W: AsyncWrite + Unpin,
{
    tokio::time::timeout(APPLE_WRAPPER_IO_TIMEOUT, stream.write_all(data))
        .await
        .map_err(|_| BotError::Custom(format!("Apple Music wrapper 写入超时：{context}")))??;
    Ok(())
}

async fn wrapper_flush<W>(stream: &mut W, context: &str) -> Result<(), BotError>
where
    W: AsyncWrite + Unpin,
{
    tokio::time::timeout(APPLE_WRAPPER_IO_TIMEOUT, stream.flush())
        .await
        .map_err(|_| BotError::Custom(format!("Apple Music wrapper flush 超时：{context}")))??;
    Ok(())
}

async fn wrapper_read_exact<S>(
    stream: &mut S,
    data: &mut [u8],
    context: &str,
) -> Result<(), BotError>
where
    S: AsyncRead + Unpin,
{
    tokio::time::timeout(APPLE_WRAPPER_IO_TIMEOUT, stream.read_exact(data))
        .await
        .map_err(|_| BotError::Custom(format!("Apple Music wrapper 读取超时：{context}")))??;
    Ok(())
}

trait EmptyDefault {
    fn if_empty(self, fallback: &str) -> String;
}

impl EmptyDefault for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.trim().is_empty() {
            fallback.to_string()
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::funcs::command::music::provider::download_track_media;

    #[test]
    fn parses_apple_music_url_query_song_id() {
        assert_eq!(
            parse_apple_track_id("https://music.apple.com/us/album/x/1440841360?i=1440841363"),
            Some("1440841363".to_string())
        );
    }

    #[test]
    fn parses_apple_prefixed_id() {
        assert_eq!(
            parse_apple_track_id("am:1440841363"),
            Some("1440841363".to_string())
        );
    }

    #[test]
    fn builds_apple_search_url() {
        let url = apple_search_url("hello", 5);
        assert!(url.contains(&format!(
            "amp-api.music.apple.com/v1/catalog/{}/search",
            apple_storefront()
        )));
        assert!(url.contains("types=songs"));
        assert!(url.contains("limit=5"));
    }

    #[test]
    fn extracts_media_user_token_from_cookie_string() {
        assert_eq!(
            normalize_media_user_token("foo=bar; media-user-token=token-value; baz=qux"),
            "token-value"
        );
        assert_eq!(normalize_media_user_token("token-only"), "token-only");
    }

    #[test]
    fn parses_user_quality_values() {
        assert_eq!(quality_from_str("standard"), AppleMusicQuality::Standard);
        assert_eq!(quality_from_str("lossless"), AppleMusicQuality::Lossless);
        assert_eq!(quality_from_str("hires"), AppleMusicQuality::HiRes);
        assert_eq!(AppleMusicQuality::Lossless.as_str(), "lossless");
    }

    #[test]
    fn apple_wrapper_route_matches_source_bot_priority() {
        assert!(should_prefer_apple_wrapper(
            AppleMusicQuality::Standard,
            true
        ));
        assert!(should_prefer_apple_wrapper(
            AppleMusicQuality::Lossless,
            true
        ));
        assert!(should_prefer_apple_wrapper(AppleMusicQuality::HiRes, true));
        assert!(!should_prefer_apple_wrapper(AppleMusicQuality::High, true));
        assert!(!should_prefer_apple_wrapper(
            AppleMusicQuality::Lossless,
            false
        ));
        assert_eq!(
            apple_wrapper_internal_url("127.0.0.1", "1624001324", AppleMusicQuality::Lossless),
            "applemusic-wrapper://127.0.0.1/1624001324?quality=lossless"
        );
    }

    #[test]
    fn parses_enhanced_hls_master_and_selects_lossless() {
        let master = r#"#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=64000,AVERAGE-BANDWIDTH=64000,CODECS="mp4a.40.2",AUDIO="audio-stereo-44100"
low.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=2116000,AVERAGE-BANDWIDTH=2116000,CODECS="alac",AUDIO="audio-alac-stereo-44100-16"
lossless.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=4600000,AVERAGE-BANDWIDTH=4600000,CODECS="alac",AUDIO="audio-alac-stereo-96000-24"
hires.m3u8
"#;
        let variants = parse_enhanced_hls_master(master).unwrap();
        assert_eq!(variants.len(), 3);
        assert_eq!(
            select_variant_for_quality(&variants, AppleMusicQuality::Lossless)
                .unwrap()
                .uri,
            "hires.m3u8"
        );
        assert_eq!(
            select_variant_for_quality(&variants, AppleMusicQuality::Standard)
                .unwrap()
                .uri,
            "low.m3u8"
        );
    }

    #[test]
    fn parses_enhanced_hls_media_keys_and_mp4() {
        let playlist = r#"#EXTM3U
#EXT-X-KEY:METHOD=SAMPLE-AES,URI="skd://itunes.apple.com/P000000000/s1/e1",KEYFORMAT="com.apple.streamingkeydelivery"
#EXT-X-KEY:METHOD=SAMPLE-AES,URI="data:text/plain;base64,AAAA",KEYFORMAT="urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed"
#EXT-X-MAP:URI="track.m4a",BYTERANGE="100@0"
#EXTINF:1,
track.m4a
#EXT-X-KEY:METHOD=SAMPLE-AES,URI="skd://itunes.apple.com/p1/c2",KEYFORMAT="com.apple.streamingkeydelivery"
#EXTINF:1,
track.m4a
"#;
        let media =
            parse_enhanced_hls_media("https://example.test/a/master.m3u8", playlist).unwrap();
        assert_eq!(media.mp4_url, "https://example.test/a/track.m4a");
        assert_eq!(
            media.seg_keys,
            vec![
                APPLE_PREFETCH_KEY_URI.to_string(),
                "skd://itunes.apple.com/p1/c2".to_string()
            ]
        );
    }

    #[test]
    fn finds_tenc_inside_stsd_enca_sample_entry() {
        fn mp4_box(typ: &[u8; 4], payload: Vec<u8>) -> Vec<u8> {
            let mut out = Vec::new();
            out.extend_from_slice(&((payload.len() + 8) as u32).to_be_bytes());
            out.extend_from_slice(typ);
            out.extend_from_slice(&payload);
            out
        }

        let mut tenc_payload = vec![0, 0, 0, 0, 0, 0, 1, 16];
        tenc_payload.extend_from_slice(&[7u8; 16]);
        let tenc = mp4_box(b"tenc", tenc_payload);
        let schi = mp4_box(b"schi", tenc);
        let sinf = mp4_box(b"sinf", schi);
        let mut enca_payload = vec![0u8; 28];
        enca_payload.extend_from_slice(&sinf);
        let enca = mp4_box(b"enca", enca_payload);
        let mut stsd_payload = vec![0, 0, 0, 0];
        stsd_payload.extend_from_slice(&1u32.to_be_bytes());
        stsd_payload.extend_from_slice(&enca);
        let stsd = mp4_box(b"stsd", stsd_payload);
        let stbl = mp4_box(b"stbl", stsd);
        let minf = mp4_box(b"minf", stbl);
        let mdia = mp4_box(b"mdia", minf);
        let trak = mp4_box(b"trak", mdia);
        let moov = mp4_box(b"moov", trak);

        let parsed = parse_tenc_from_init(&moov).unwrap();
        assert_eq!(parsed.default_per_sample_iv_size, 16);
        assert_eq!(parsed.default_kid, [7u8; 16]);
    }

    #[tokio::test]
    #[ignore = "requires live Apple Music credentials and wrapper"]
    async fn live_search_resolve_and_download_without_telegram() {
        let results = APPLE_MUSIC_PROVIDER.search("晴天", 1).await.unwrap();
        let first = results
            .first()
            .expect("Apple Music search returned no songs");
        let track = APPLE_MUSIC_PROVIDER
            .resolve(&first.id, Some(&first.id))
            .await
            .unwrap();
        let media = download_track_media(&track).await.unwrap();
        assert!(!media.audio.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires live Apple Music credentials, wrapper, and a lossless-capable track"]
    async fn live_wrapper_lossless_download_without_telegram() {
        let results = APPLE_MUSIC_PROVIDER.search("晴天", 1).await.unwrap();
        let first = results
            .first()
            .expect("Apple Music search returned no songs");
        let host = SETTINGS.music.applemusic.wrapper_host.trim().to_string();
        assert!(!host.is_empty(), "wrapper_host is required");
        let audio = download_via_wrapper(&host, &first.id, AppleMusicQuality::Lossless)
            .await
            .unwrap();
        assert!(!audio.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires live Apple Music credentials, wrapper, ffprobe, and a lossless-capable track"]
    async fn live_wrapper_lossless_output_is_probeable_without_telegram() {
        let results = APPLE_MUSIC_PROVIDER.search("晴天", 1).await.unwrap();
        let first = results
            .first()
            .expect("Apple Music search returned no songs");
        let host = SETTINGS.music.applemusic.wrapper_host.trim().to_string();
        assert!(!host.is_empty(), "wrapper_host is required");
        let audio = download_via_wrapper(&host, &first.id, AppleMusicQuality::Lossless)
            .await
            .unwrap();
        let path = std::env::temp_dir().join("bot-rs-applemusic-lossless.m4a");
        std::fs::write(&path, audio).unwrap();
        let output = std::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).trim().is_empty());
    }

    #[tokio::test]
    #[ignore = "requires live Apple Music credentials, wrapper, ffprobe, and ffmpeg"]
    async fn live_wrapper_lossless_specific_track_id_without_telegram() {
        let track_id =
            std::env::var("APPLE_MUSIC_TEST_ID").unwrap_or_else(|_| "1624001324".to_string());
        let track = APPLE_MUSIC_PROVIDER
            .resolve(&track_id, Some(&track_id))
            .await
            .unwrap();
        let media = download_track_media(&track).await.unwrap();
        let path = std::env::temp_dir().join(format!("bot-rs-applemusic-{track_id}.m4a"));
        std::fs::write(&path, &media.audio).unwrap();
        let output = std::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).trim().is_empty());
        eprintln!("{} -> {} bytes", track.file_name(), media.audio.len());
    }

    #[tokio::test]
    #[ignore = "requires live Apple Music credentials, ffprobe, and ffmpeg"]
    async fn live_high_specific_track_id_is_aac_without_telegram() {
        let track_id =
            std::env::var("APPLE_MUSIC_TEST_ID").unwrap_or_else(|_| "1624001324".to_string());
        let track = resolve_with_quality(&track_id, Some(&track_id), "high")
            .await
            .unwrap();
        assert!(!track.url.starts_with(APPLE_WRAPPER_SCHEME));
        let media = download_track_media(&track).await.unwrap();
        let path = std::env::temp_dir().join(format!("bot-rs-applemusic-high-{track_id}.m4a"));
        std::fs::write(&path, &media.audio).unwrap();
        let output = std::process::Command::new("ffprobe")
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
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("codec_name=aac"), "{stdout}");
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
        eprintln!(
            "{} -> {} bytes\n{stdout}",
            track.file_name(),
            media.audio.len()
        );
    }
}
