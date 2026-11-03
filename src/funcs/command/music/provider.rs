//! Music sources through MHub's gateway. MHub owns every platform account,
//! implementation, download and the shared Telegram cache; the bot only
//! decides who may ask for what, and sends the resulting Telegram files.

use crate::{BotError, settings::SETTINGS};
use music_gateway::GatewayClient;
use std::{sync::OnceLock, time::Duration};

pub use music_gateway::{
    CacheInvalidateRequest, Collection as MusicCollection, JobState, Lyrics as MusicLyrics,
    Media as MusicMedia, MediaRequest, Platform as MusicPlatform, Quality,
    SearchItem as MusicSearchItem, Track as MusicTrack,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicQuery {
    pub platform: MusicPlatform,
    pub keyword: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DownloadProgress {
    pub written: u64,
    pub total: Option<u64>,
}

pub fn build_query_with_platform(
    platform: MusicPlatform,
    keyword_parts: &[String],
) -> Result<MusicQuery, BotError> {
    let keyword = keyword_parts.join(" ").trim().to_string();
    if keyword.is_empty() {
        return Err(BotError::Custom(
            "请输入歌曲名、链接或 ID，例如：/music 晴天".to_string(),
        ));
    }

    Ok(MusicQuery { platform, keyword })
}

/// The stored quality setting as a listening tier; lossless when unset.
pub fn parse_quality(value: &str) -> Quality {
    Quality::parse(value).unwrap_or(Quality::Lossless)
}

/// The shared gateway client. A missing or unreadable configuration is
/// reported on every request instead of taking the whole bot down.
pub fn gateway() -> Result<&'static GatewayClient, BotError> {
    static CLIENT: OnceLock<Result<GatewayClient, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            let config = &SETTINGS.music.gateway;
            let token = config.resolve_token()?;
            if token.is_empty() {
                return Err("未配置 music.gateway.token，无法使用音乐功能".to_string());
            }
            GatewayClient::new(&config.url, &token)
                .map(|client| {
                    client.with_media_timeout(Duration::from_secs(
                        config.media_timeout_seconds.max(60),
                    ))
                })
                .map_err(|err| err.to_string())
        })
        .as_ref()
        .map_err(|err| BotError::Custom(err.clone()))
}

pub async fn search(query: &MusicQuery, limit: usize) -> Result<Vec<MusicSearchItem>, BotError> {
    Ok(gateway()?
        .search(query.platform, &query.keyword, limit)
        .await?)
}

pub async fn collection(
    query: &MusicQuery,
    limit: usize,
) -> Result<Option<MusicCollection>, BotError> {
    if !query.platform.supports_collections() {
        return Ok(None);
    }
    Ok(gateway()?
        .collection(query.platform, &query.keyword, limit)
        .await?)
}

pub async fn lyrics(
    platform: MusicPlatform,
    track_id: &str,
) -> Result<Option<MusicLyrics>, BotError> {
    if !platform.supports_lyrics() {
        return Ok(None);
    }
    Ok(gateway()?.lyrics(platform, track_id).await?)
}

pub async fn resolve(
    query: &MusicQuery,
    selected_id: Option<&str>,
    quality: Quality,
) -> Result<MusicTrack, BotError> {
    Ok(gateway()?
        .resolve(&music_gateway::ResolveRequest {
            platform: query.platform,
            keyword: query.keyword.clone(),
            selected_id: selected_id.map(str::to_string),
            quality,
        })
        .await?)
}

/// Asks MHub to forget a cached file Telegram no longer accepts.
pub async fn invalidate(media: &MusicMedia) {
    let result = match gateway() {
        Ok(client) => client
            .invalidate_cache(&CacheInvalidateRequest {
                platform: media.track.platform,
                track_id: media.track.id.clone(),
                quality: media.quality,
                file_id: media.file_id.clone(),
            })
            .await
            .map_err(BotError::from),
        Err(err) => Err(err),
    };
    if let Err(err) = result {
        log::warn!("failed to invalidate music cache: {err}");
    }
}
