//! Preserve AAC/ALAC packets in standard M4A, never the web-only packet stream.
use super::provider::{MusicMedia, MusicPlatform, MusicTrack};
use crate::{
    BotError,
    settings::{SETTINGS, TelegramAudio},
};
use lazy_static::lazy_static;
use serde::Deserialize;
use std::{path::Path, process::Output, time::Duration};
use tokio::process::Command;

const MAX_AUDIO_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const CACHE_SUFFIX: &str = ":tg-m4a-v2";

lazy_static! {
    static ref SLOTS: tokio::sync::Semaphore =
        tokio::sync::Semaphore::new(SETTINGS.music.telegram_audio.concurrency.clamp(1, 4));
}

pub(super) fn cache_quality(platform: MusicPlatform, quality: &str) -> String {
    if platform == MusicPlatform::AppleMusic {
        format!("{quality}{CACHE_SUFFIX}")
    } else {
        quality.to_string()
    }
}

#[derive(Deserialize)]
struct Probe {
    streams: Vec<AudioStream>,
}

#[derive(Deserialize)]
struct AudioStream {
    codec_name: String,
    profile: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u32>,
    bits_per_raw_sample: Option<String>,
    duration: Option<String>,
    bit_rate: Option<String>,
}

impl AudioStream {
    fn supported_for_m4a(&self) -> bool {
        matches!(self.codec_name.as_str(), "aac" | "alac")
            && self.channels.is_some_and(|channels| channels > 0)
            && self
                .sample_rate
                .as_deref()
                .and_then(|x| x.parse::<u32>().ok())
                .is_some_and(|rate| rate > 0)
    }

    fn same_encoding_as(&self, source: &Self) -> bool {
        self.codec_name == source.codec_name
            && self.profile == source.profile
            && self.sample_rate == source.sample_rate
            && self.channels == source.channels
            && self.bits_per_raw_sample == source.bits_per_raw_sample
    }

    fn duration_seconds(&self) -> Option<f64> {
        self.duration
            .as_deref()?
            .parse::<f64>()
            .ok()
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
    }
}

async fn run(command: &mut Command, config: &TelegramAudio) -> Result<Output, BotError> {
    let result = tokio::time::timeout(
        Duration::from_secs(config.timeout_seconds.max(1)),
        command.kill_on_drop(true).output(),
    )
    .await
    .map_err(|_| BotError::Custom("音频封装或校验超时，未上传文件".into()))?
    .map_err(|error| {
        BotError::Custom(format!(
            "无法运行音频处理工具，请检查 music.telegram_audio 配置：{error}"
        ))
    })?;
    if !result.status.success() {
        // Do not expose file paths or media URLs in a user-facing FFmpeg error.
        return Err(BotError::Custom(
            "音频封装或解码校验失败，未上传文件".into(),
        ));
    }
    Ok(result)
}

async fn probe(path: &Path, config: &TelegramAudio) -> Result<AudioStream, BotError> {
    let result = run(
        Command::new(&config.ffprobe)
            .args([
                "-v",
                "error",
                "-protocol_whitelist",
                "file,pipe",
                "-select_streams",
                "a:0",
                "-show_entries",
                "stream=codec_name,profile,sample_rate,channels,bits_per_raw_sample,duration,bit_rate",
                "-of",
                "json",
            ])
            .arg(path),
        config,
    )
    .await?;
    let data: Probe = serde_json::from_slice(&result.stdout)?;
    data.streams
        .into_iter()
        .next()
        .ok_or_else(|| BotError::Custom("文件不包含有效音轨，未上传文件".into()))
}

pub(super) async fn prepare(
    track: &mut MusicTrack,
    media: &mut MusicMedia,
) -> Result<(), BotError> {
    if track.platform != MusicPlatform::AppleMusic {
        return Ok(());
    }
    let _permit = SLOTS
        .acquire()
        .await
        .map_err(|_| BotError::Custom("音频处理队列已关闭".into()))?;
    prepare_with_config(track, media, &SETTINGS.music.telegram_audio).await
}

async fn prepare_with_config(
    track: &mut MusicTrack,
    media: &mut MusicMedia,
    config: &TelegramAudio,
) -> Result<(), BotError> {
    if media.audio.is_empty() || media.audio.len() as u64 > MAX_AUDIO_BYTES {
        return Err(BotError::Custom("音频为空或超过 Telegram 大小上限".into()));
    }
    let directory = tempfile::tempdir()?;
    let input = directory.path().join("source.m4a");
    let output = directory.path().join("telegram.m4a");
    tokio::fs::write(&input, &media.audio).await?;
    let source = probe(&input, config).await?;
    if !source.supported_for_m4a() {
        return Err(BotError::Custom(
            "音频不是有效的 AAC/ALAC，不自动转码降质".into(),
        ));
    }
    let mut remux = Command::new(&config.ffmpeg);
    remux
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-xerror",
            "-err_detect",
            "explode",
            "-protocol_whitelist",
            "file,pipe",
            "-i",
        ])
        .arg(&input)
        .args([
            "-map",
            "0:a:0",
            "-vn",
            "-sn",
            "-dn",
            "-map_metadata",
            "-1",
            "-c:a",
            "copy",
        ]);
    remux
        .args([
            "-metadata",
            &format!("title={}", track.song),
            "-metadata",
            &format!("artist={}", track.singer),
            "-metadata",
            &format!("album={}", track.album),
            "-movflags",
            "+faststart",
            "-f",
            "mp4",
            "-fs",
            &MAX_AUDIO_BYTES.to_string(),
        ])
        .arg(&output);
    run(&mut remux, config).await?;
    let info = probe(&output, config).await?;
    let duration = info
        .duration_seconds()
        .ok_or_else(|| BotError::Custom("音频时长无效，未上传文件".into()))?;
    if !info.same_encoding_as(&source) {
        return Err(BotError::Custom(
            "音频封装前后编码参数不一致，未上传文件".into(),
        ));
    }
    if !duration_matches(
        track
            .duration
            .map(f64::from)
            .or_else(|| source.duration_seconds()),
        duration,
    ) {
        return Err(BotError::Custom("音频时长不完整，未上传文件".into()));
    }
    // A valid MP4 header does not prove decryption was correct. Decode the
    // complete AAC/ALAC output before publishing/cache insertion.
    run(
        Command::new(&config.ffmpeg)
            .args([
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-xerror",
                "-err_detect",
                "explode",
                "-protocol_whitelist",
                "file,pipe",
                "-i",
            ])
            .arg(&output)
            .args(["-map", "0:a:0", "-f", "null", "-"]),
        config,
    )
    .await?;
    let size = tokio::fs::metadata(&output).await?.len();
    if size == 0 || size >= MAX_AUDIO_BYTES {
        return Err(BotError::Custom("音频为空或超过 Telegram 大小上限".into()));
    }
    media.audio = tokio::fs::read(output).await?;
    track.format = Some("m4a".into());
    track.duration = Some(duration.round() as u32);
    track.bitrate = info.bit_rate.as_deref().and_then(|x| x.parse().ok());
    Ok(())
}

fn duration_matches(expected: Option<f64>, actual: f64) -> bool {
    expected.is_none_or(|expected| (expected - actual).abs() <= 2.0_f64.max(expected * 0.02))
}

#[cfg(test)]
mod tests {
    use super::{
        AudioStream, MusicMedia, MusicPlatform, MusicTrack, TelegramAudio, cache_quality,
        duration_matches, prepare_with_config, probe, run,
    };
    use tokio::process::Command;

    #[test]
    fn isolates_validated_m4a_from_old_and_transcoded_cache() {
        assert_eq!(
            cache_quality(MusicPlatform::AppleMusic, "lossless"),
            "lossless:tg-m4a-v2"
        );
        assert_eq!(
            cache_quality(MusicPlatform::Tencent, "lossless"),
            "lossless"
        );
    }

    #[test]
    fn rejects_preview_duration() {
        assert!(!duration_matches(Some(223.0), 30.0));
        assert!(duration_matches(Some(223.0), 222.9));
    }

    #[test]
    fn accepts_aac_and_hires_alac_without_lowering_quality() {
        let mut stream: AudioStream = serde_json::from_str(
            r#"{"codec_name":"aac","profile":"LC","sample_rate":"48000","channels":2}"#,
        )
        .unwrap();
        assert!(stream.supported_for_m4a());
        stream.codec_name = "alac".into();
        stream.sample_rate = Some("192000".into());
        stream.bits_per_raw_sample = Some("24".into());
        assert!(stream.supported_for_m4a());
        stream.codec_name = "opus".into();
        assert!(!stream.supported_for_m4a());
    }

    #[tokio::test]
    #[ignore = "requires ffmpeg and ffprobe; offline, no Telegram"]
    async fn preserves_alac_and_aac_packets_without_transcoding() {
        let config = TelegramAudio::default();
        let dir = tempfile::tempdir().unwrap();
        for (codec, rate, sample_format) in [
            ("alac", "44100", "s16p"),
            ("alac", "96000", "s32p"),
            ("aac", "44100", "fltp"),
        ] {
            let source = dir.path().join(format!("{codec}-{rate}.m4a"));
            run(
                Command::new(&config.ffmpeg)
                    .args([
                        "-nostdin",
                        "-v",
                        "error",
                        "-f",
                        "lavfi",
                        "-i",
                        "sine=frequency=440:duration=3",
                        "-c:a",
                        codec,
                        "-ar",
                        rate,
                        "-ac",
                        "2",
                        "-sample_fmt",
                        sample_format,
                    ])
                    .arg(&source),
                &config,
            )
            .await
            .unwrap();
            let mut track = MusicTrack {
                id: "test".into(),
                platform: MusicPlatform::AppleMusic,
                song: "Test".into(),
                singer: "Test".into(),
                album: String::new(),
                cover: String::new(),
                link: String::new(),
                url: String::new(),
                headers: Default::default(),
                max_chunk_size: None,
                duration: Some(3),
                bitrate: None,
                format: Some("m4a".into()),
            };
            let mut media = MusicMedia {
                audio: tokio::fs::read(&source).await.unwrap(),
                cover: vec![1, 2, 3],
                decrypt_elapsed: None,
            };
            prepare_with_config(&mut track, &mut media, &config)
                .await
                .unwrap();
            let output = dir.path().join(format!("{codec}-{rate}-telegram.m4a"));
            tokio::fs::write(&output, &media.audio).await.unwrap();
            let info = probe(&output, &config).await.unwrap();
            assert!(info.supported_for_m4a());
            assert_eq!(info.codec_name, codec);
            assert_eq!(info.sample_rate.as_deref(), Some(rate));
            assert_eq!(info.channels, Some(2));
            if codec == "alac" {
                assert_eq!(
                    info.bits_per_raw_sample.as_deref(),
                    Some(if sample_format == "s32p" { "24" } else { "16" })
                );
            }
            assert_eq!(media.cover, [1, 2, 3]);
            assert_eq!(track.duration, Some(3));
            let mut hashes = Vec::new();
            for path in [&source, &output] {
                let hash = run(
                    Command::new(&config.ffmpeg)
                        .args(["-nostdin", "-v", "error", "-i"])
                        .arg(path)
                        .args([
                            "-map", "0:a:0", "-c:a", "copy", "-f", "hash", "-hash", "sha256", "-",
                        ]),
                    &config,
                )
                .await
                .unwrap();
                hashes.push(hash.stdout);
            }
            assert_eq!(
                hashes[0], hashes[1],
                "AAC/ALAC packets must not be re-encoded"
            );
            track.duration = Some(223);
            assert!(
                prepare_with_config(&mut track, &mut media, &config)
                    .await
                    .is_err(),
                "preview duration must not be accepted"
            );
            media.audio = b"not an audio file".to_vec();
            assert!(
                prepare_with_config(&mut track, &mut media, &config)
                    .await
                    .is_err()
            );
        }
    }
}
