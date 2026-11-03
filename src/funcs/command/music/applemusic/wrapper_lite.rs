//! Complete-file wrapper-lite/Temari transport, ported from the Music API project.
//! Browser-only packet streams intentionally do not cross this boundary.
use super::{
    APPLE_MUSIC_UA, APPLE_WRAPPER_TRACK_DECRYPT_SEMAPHORE, AppleMusicQuality, BotError, CLIENT,
    DownloadProgress, SubsampleEntry, TencBox, absolute_hls_url, collect_wrapper_decrypt_commands,
    download_enhanced_hls_media_direct_or_range, download_text, group_wrapper_commands_by_fragment,
    parse_enhanced_hls_master, parse_enhanced_hls_media, parse_tenc_from_init, parse_traf,
    read_mp4_box, remux_decrypted_fmp4_to_progressive, select_variant_for_quality,
    validate_hls_master_url, validate_hls_media_duration,
};
use crate::settings::SETTINGS;
use lazy_static::lazy_static;
use serde::{Deserialize, de::DeserializeOwned};
use std::{
    collections::HashMap,
    sync::{
        Arc, RwLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use temari::{rounds::decrypt_par, template::template_from_json};
use url::Url;

const APPLE_WRAPPER_LITE_TIMEOUT: Duration = Duration::from_secs(10);
const APPLE_WRAPPER_LITE_CACHE_TTL: Duration = Duration::from_secs(15 * 60);

lazy_static! {
    static ref APPLE_WRAPPER_LITE_ENDPOINTS: Vec<AppleWrapperLiteEndpoint> =
        configured_wrapper_lite_endpoints();
    static ref APPLE_WRAPPER_LITE_NEXT_ENDPOINT: AtomicUsize = AtomicUsize::new(0);
    static ref APPLE_WRAPPER_LITE_TEMPLATES: RwLock<HashMap<(String, String), CachedWrapperLiteTemplate>> =
        RwLock::new(HashMap::new());
}
#[derive(Clone)]
struct CachedWrapperLiteTemplate {
    fetched_at: Instant,
    template: Arc<temari::rounds::Template>,
}

pub(super) fn enabled() -> bool {
    !APPLE_WRAPPER_LITE_ENDPOINTS.is_empty()
}

#[derive(Clone, Debug)]
struct AppleWrapperLiteEndpoint {
    base_url: String,
}

#[derive(Debug, Deserialize)]
struct AppleWrapperLiteResponse<T> {
    code: i64,
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct AppleWrapperLiteM3u8Data {
    #[serde(rename = "m3u8")]
    m3u8: String,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct AppleWrapperLiteKeyData {
    #[serde(default, rename = "adamId")]
    adam_id: String,
    #[serde(default, rename = "keyUri")]
    key_uri: String,
    #[serde(default, rename = "contentKey")]
    content_key: String,
    #[serde(default)]
    ctx: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    rcx: String,
    #[serde(default)]
    rax: String,
    #[serde(default)]
    rdx: String,
    #[serde(default)]
    r9: String,
    #[serde(default)]
    rbp: String,
}

fn configured_wrapper_lite_endpoints() -> Vec<AppleWrapperLiteEndpoint> {
    let config = &SETTINGS.music.applemusic;
    if !config.wrapper_lite_enabled {
        return Vec::new();
    }
    let mut specs = config
        .wrapper_lite_urls
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(config.wrapper_lite_url.as_str()))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string())
        .collect::<Vec<_>>();
    let mut seen = std::collections::HashSet::new();
    specs.retain(|spec| seen.insert(spec.clone()));
    specs
        .into_iter()
        .filter_map(|spec| {
            let base_url = if spec.starts_with("http://") || spec.starts_with("https://") {
                spec
            } else {
                format!("http://{spec}")
            };
            let parsed = Url::parse(&base_url).ok()?;
            if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
                return None;
            }
            Some(AppleWrapperLiteEndpoint { base_url })
        })
        .collect()
}

pub(super) async fn download_via_wrapper_lite_with_progress_stats<F>(
    track_id: &str,
    quality: AppleMusicQuality,
    expected_duration: Option<u32>,
    progress: &mut F,
) -> Result<(Vec<u8>, Option<Duration>), BotError>
where
    F: FnMut(DownloadProgress) + Send,
{
    if APPLE_WRAPPER_LITE_ENDPOINTS.is_empty() {
        return Err(BotError::Custom(
            "Apple Music wrapper-lite endpoint 未配置".to_string(),
        ));
    }
    let _track_permit = APPLE_WRAPPER_TRACK_DECRYPT_SEMAPHORE
        .acquire()
        .await
        .map_err(|_| BotError::Custom("Apple Music wrapper-lite track semaphore closed".into()))?;
    let start = APPLE_WRAPPER_LITE_NEXT_ENDPOINT.fetch_add(1, Ordering::Relaxed)
        % APPLE_WRAPPER_LITE_ENDPOINTS.len();
    let mut last_error = None;
    for offset in 0..APPLE_WRAPPER_LITE_ENDPOINTS.len() {
        let endpoint =
            &APPLE_WRAPPER_LITE_ENDPOINTS[(start + offset) % APPLE_WRAPPER_LITE_ENDPOINTS.len()];
        match download_via_wrapper_lite_endpoint(
            endpoint,
            track_id,
            quality,
            expected_duration,
            progress,
        )
        .await
        {
            Ok(result) => return Ok(result),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        BotError::Custom("Apple Music wrapper-lite 所有 endpoint 均失败".to_string())
    }))
}

async fn download_via_wrapper_lite_endpoint<F>(
    endpoint: &AppleWrapperLiteEndpoint,
    track_id: &str,
    quality: AppleMusicQuality,
    expected_duration: Option<u32>,
    progress: &mut F,
) -> Result<(Vec<u8>, Option<Duration>), BotError>
where
    F: FnMut(DownloadProgress) + Send,
{
    let master_url = wrapper_lite_m3u8_url(endpoint, track_id).await?;
    let master = download_text(&master_url).await?;
    let variants = parse_enhanced_hls_master(&master)?;
    let variant = select_variant_for_quality(&variants, quality)
        .ok_or_else(|| BotError::Custom("Apple Music enhancedHls 没有匹配的音质".to_string()))?;
    let media_url = absolute_hls_url(&master_url, &variant.uri);
    let media = parse_enhanced_hls_media(&media_url, &download_text(&media_url).await?)?;
    validate_hls_media_duration(media.duration_seconds, expected_duration)?;
    let mut encrypted = download_enhanced_hls_media_direct_or_range(&media, progress).await?;
    let tenc = parse_tenc_from_init(&encrypted)?;
    validate_wrapper_lite_cenc_layout(&encrypted, &tenc)?;

    let commands = collect_wrapper_decrypt_commands(&encrypted, track_id, &media.seg_keys, &tenc)?;
    let groups = group_wrapper_commands_by_fragment(commands)?;
    if groups.is_empty() {
        return Err(BotError::Custom(
            "Apple Music wrapper-lite 没有可解密的 MP4 fragment".to_string(),
        ));
    }

    let started = Instant::now();
    let batch_size = SETTINGS
        .music
        .applemusic
        .wrapper_lite_batch_size
        .clamp(1, 4096);
    for group in groups {
        let template = wrapper_lite_template(endpoint, &group.adam, &group.uri).await?;
        for jobs in group.jobs.chunks(batch_size) {
            if jobs.is_empty() {
                continue;
            }
            let payloads = jobs
                .iter()
                .map(|job| job.payload.clone())
                .collect::<Vec<_>>();
            let template = Arc::clone(&template);
            let plaintexts = tokio::task::spawn_blocking(move || {
                let samples = payloads.iter().map(Vec::as_slice).collect::<Vec<_>>();
                decrypt_par(template.as_ref(), &samples)
            })
            .await
            .map_err(|error| {
                BotError::Custom(format!("Apple Music wrapper-lite 解密任务失败：{error}"))
            })?;
            if plaintexts.len() != jobs.len() {
                return Err(BotError::Custom(
                    "Apple Music wrapper-lite 解密结果数量异常".to_string(),
                ));
            }
            for (job, plaintext) in jobs.iter().zip(plaintexts) {
                if plaintext.len() != job.payload.len()
                    || job
                        .ranges
                        .iter()
                        .map(|range| range.end - range.start)
                        .sum::<usize>()
                        != plaintext.len()
                {
                    return Err(BotError::Custom(
                        "Apple Music wrapper-lite 解密结果长度异常".to_string(),
                    ));
                }
                let mut source_offset = 0usize;
                for range in &job.ranges {
                    if range.end > encrypted.len()
                        || source_offset + (range.end - range.start) > plaintext.len()
                    {
                        return Err(BotError::Custom(
                            "Apple Music wrapper-lite 解密范围超出文件".to_string(),
                        ));
                    }
                    let length = range.end - range.start;
                    encrypted[range.clone()]
                        .copy_from_slice(&plaintext[source_offset..source_offset + length]);
                    source_offset += length;
                }
            }
        }
    }
    let decrypt_elapsed = started.elapsed();
    let audio =
        tokio::task::spawn_blocking(move || remux_decrypted_fmp4_to_progressive(&encrypted))
            .await
            .map_err(|error| {
                BotError::Custom(format!("Apple Music wrapper-lite remux 任务失败：{error}"))
            })??;
    Ok((audio, Some(decrypt_elapsed)))
}

async fn wrapper_lite_m3u8_url(
    endpoint: &AppleWrapperLiteEndpoint,
    track_id: &str,
) -> Result<String, BotError> {
    let data: AppleWrapperLiteM3u8Data =
        wrapper_lite_get_json(endpoint, "/m3u8", &[("adamId", track_id)]).await?;
    validate_hls_master_url(&data.m3u8, "wrapper-lite")
}

async fn wrapper_lite_template(
    endpoint: &AppleWrapperLiteEndpoint,
    adam_id: &str,
    key_uri: &str,
) -> Result<Arc<temari::rounds::Template>, BotError> {
    let cache_key = (endpoint.base_url.clone(), format!("{adam_id}\\0{key_uri}"));
    if let Ok(cache) = APPLE_WRAPPER_LITE_TEMPLATES.read()
        && let Some(cached) = cache.get(&cache_key)
        && cached.fetched_at.elapsed() < APPLE_WRAPPER_LITE_CACHE_TTL
    {
        return Ok(Arc::clone(&cached.template));
    }
    let data: AppleWrapperLiteKeyData =
        wrapper_lite_get_json(endpoint, "/key", &[("adamId", adam_id), ("uri", key_uri)]).await?;
    if data.ctx.trim().is_empty() || data.state.trim().is_empty() {
        return Err(BotError::Custom(
            "Apple Music wrapper-lite key 缺少 Temari ctx/state".to_string(),
        ));
    }
    let template_json = serde_json::json!({
        "ctx": data.ctx,
        "state": data.state,
        "rcx": data.rcx,
        "rax": data.rax,
        "rdx": data.rdx,
        "r9": data.r9,
        "rbp": data.rbp,
    })
    .to_string();
    let template = tokio::task::spawn_blocking(move || template_from_json(&template_json))
        .await
        .map_err(|_| BotError::Custom("Apple Music wrapper-lite 模板任务失败".into()))?
        .map_err(|_| BotError::Custom("Apple Music wrapper-lite 模板解析失败".into()))?;
    let template = Arc::new(template);
    if let Ok(mut cache) = APPLE_WRAPPER_LITE_TEMPLATES.write() {
        cache.retain(|_, value| value.fetched_at.elapsed() < APPLE_WRAPPER_LITE_CACHE_TTL);
        if cache.len() >= 64 {
            cache.clear();
        }
        cache.insert(
            cache_key,
            CachedWrapperLiteTemplate {
                fetched_at: Instant::now(),
                template: Arc::clone(&template),
            },
        );
    }
    Ok(template)
}

async fn wrapper_lite_get_json<T: DeserializeOwned>(
    endpoint: &AppleWrapperLiteEndpoint,
    path: &str,
    query: &[(&str, &str)],
) -> Result<T, BotError> {
    let url = format!(
        "{}/{}",
        endpoint.base_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    );
    let response = tokio::time::timeout(
        APPLE_WRAPPER_LITE_TIMEOUT,
        CLIENT
            .get(&url)
            .query(query)
            .header("User-Agent", APPLE_MUSIC_UA)
            .send(),
    )
    .await
    .map_err(|_| BotError::Custom(format!("Apple Music wrapper-lite 请求超时：{path}")))??;
    if !response.status().is_success() {
        return Err(BotError::Custom(format!(
            "Apple Music wrapper-lite {path} 请求失败：HTTP {}",
            response.status()
        )));
    }
    let envelope: AppleWrapperLiteResponse<T> =
        tokio::time::timeout(APPLE_WRAPPER_LITE_TIMEOUT, response.json())
            .await
            .map_err(|_| {
                BotError::Custom(format!("Apple Music wrapper-lite 响应超时：{path}"))
            })??;
    if envelope.code != 0 {
        return Err(BotError::Custom(format!(
            "Apple Music wrapper-lite {path} 失败：code {}",
            envelope.code
        )));
    }
    envelope
        .data
        .ok_or_else(|| BotError::Custom(format!("Apple Music wrapper-lite {path} 缺少 data")))
}

fn validate_wrapper_lite_cenc_layout(data: &[u8], tenc: &TencBox) -> Result<(), BotError> {
    if tenc.default_crypt_byte_block != 0 || tenc.default_skip_byte_block != 0 {
        return Err(BotError::Custom(
            "Apple Music wrapper-lite 暂不支持 pattern encryption，回退 raw wrapper".to_string(),
        ));
    }
    let mut pos = 0usize;
    while let Some(moof) = read_mp4_box(data, pos)? {
        if &moof.typ == b"moof" {
            let mut child = moof.payload_start();
            while let Some(traf) = read_mp4_box(data, child)? {
                if traf.end() > moof.end() {
                    break;
                }
                if &traf.typ == b"traf" {
                    let parsed = parse_traf(data, traf, tenc)?;
                    if let Some(senc) = &parsed.senc {
                        let samples = parsed.runs.iter().flat_map(|run| &run.samples);
                        if senc.samples.len() != samples.clone().count() {
                            return Err(BotError::Custom(
                                "Apple Music senc 与 trun sample 数量不一致".to_string(),
                            ));
                        }
                        for (sample, encryption) in samples.zip(&senc.samples) {
                            validate_wrapper_lite_subsamples(sample.size, &encryption.subsamples)?;
                        }
                    }
                }
                child = traf.end();
            }
        }
        pos = moof.end();
    }
    Ok(())
}

fn validate_wrapper_lite_subsamples(
    size: u32,
    subsamples: &[SubsampleEntry],
) -> Result<(), BotError> {
    if subsamples.is_empty() {
        return Ok(());
    }
    // Apple audio commonly declares a single protected span in `senc`, even
    // when it covers the whole sample. The existing scatter/gather collector
    // already preserves its clear prefix and non-block-aligned clear tail.
    // Temari resets its cipher state for each job. Keep multiple encrypted
    // spans disabled until the scheme-specific CBC chaining is explicit.
    if subsamples
        .iter()
        .filter(|entry| entry.bytes_of_protected_data > 0)
        .count()
        > 1
    {
        return Err(BotError::Custom(
            "Apple Music wrapper-lite 暂不支持同一 sample 多段加密，回退 raw wrapper".to_string(),
        ));
    }
    let mut covered = 0u64;
    for entry in subsamples {
        covered += u64::from(entry.bytes_of_clear_data) + u64::from(entry.bytes_of_protected_data);
        if covered > u64::from(size) {
            return Err(BotError::Custom(
                "Apple Music senc 范围超出 sample".to_string(),
            ));
        }
    }
    if covered != u64::from(size) {
        return Err(BotError::Custom(
            "Apple Music senc 未完整描述 sample".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        AppleWrapperLiteEndpoint, SubsampleEntry, download_via_wrapper_lite_endpoint,
        validate_wrapper_lite_subsamples,
    };
    use crate::funcs::command::music::{
        applemusic::{AppleMusicQuality, resolve_with_quality},
        provider::MusicMedia,
        telegram_audio,
    };

    #[test]
    fn validates_complete_single_encrypted_span_and_clear_tail() {
        let entry = |clear, protected| SubsampleEntry {
            bytes_of_clear_data: clear,
            bytes_of_protected_data: protected,
        };
        assert!(validate_wrapper_lite_subsamples(64, &[entry(4, 48), entry(12, 0)]).is_ok());
        assert!(validate_wrapper_lite_subsamples(64, &[entry(0, 32), entry(0, 32)]).is_err());
        assert!(validate_wrapper_lite_subsamples(64, &[entry(0, 65)]).is_err());
        assert!(validate_wrapper_lite_subsamples(64, &[entry(0, 63)]).is_err());
    }

    #[tokio::test]
    #[ignore = "requires live wrapper-lite, Apple catalog, ffmpeg/ffprobe; no Telegram"]
    async fn live_wrapper_lite_high_and_lossless_are_playable() {
        let endpoint = AppleWrapperLiteEndpoint {
            base_url: std::env::var("APPLE_MUSIC_TEST_WRAPPER_LITE_URL")
                .expect("set APPLE_MUSIC_TEST_WRAPPER_LITE_URL"),
        };
        let id = std::env::var("APPLE_MUSIC_TEST_ID").unwrap_or_else(|_| "1624001324".into());
        for quality in [AppleMusicQuality::High, AppleMusicQuality::Lossless] {
            let mut track = resolve_with_quality(&id, Some(&id), quality.as_str())
                .await
                .unwrap();
            let (audio, decrypt_elapsed) = download_via_wrapper_lite_endpoint(
                &endpoint,
                &id,
                quality,
                track.duration,
                &mut |_| {},
            )
            .await
            .unwrap();
            let original_size = audio.len();
            let mut media = MusicMedia {
                audio,
                cover: Vec::new(),
                decrypt_elapsed,
            };
            telegram_audio::prepare(&mut track, &mut media)
                .await
                .unwrap();
            assert_eq!(track.format.as_deref(), Some("m4a"));
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("telegram.m4a");
            tokio::fs::write(&path, &media.audio).await.unwrap();
            let probe = tokio::process::Command::new(
                &crate::settings::SETTINGS.music.telegram_audio.ffprobe,
            )
            .args([
                "-v",
                "error",
                "-select_streams",
                "a:0",
                "-show_entries",
                "stream=codec_name,sample_rate,channels,bits_per_raw_sample",
                "-of",
                "json",
            ])
            .arg(&path)
            .kill_on_drop(true)
            .output()
            .await
            .unwrap();
            assert!(probe.status.success());
            let metadata: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
            let stream = &metadata["streams"][0];
            let expected_codec = if quality == AppleMusicQuality::High {
                "aac"
            } else {
                "alac"
            };
            assert_eq!(stream["codec_name"].as_str(), Some(expected_codec));
            eprintln!(
                "{} {}: source={} bytes, Telegram={} bytes, duration={:?}, decrypt={:?}, encoding={stream}",
                id,
                quality.as_str(),
                original_size,
                media.audio.len(),
                track.duration,
                decrypt_elapsed
            );
        }
    }
}
