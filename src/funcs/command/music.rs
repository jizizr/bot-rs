use super::*;
use crate::dao::mongo::music_settings::{
    UserMusicSettings, get_user_settings, get_user_settings_or_default, save_user_settings,
};
use image::{DynamicImage, ImageFormat, imageops::FilterType};
use provider::{
    DownloadProgress, MusicMedia, MusicPlatform, MusicProvider, MusicQuery, MusicSearchItem,
    MusicTrack,
};
use std::{
    io::Cursor,
    time::{Duration, Instant},
};
use teloxide::types::MaybeInaccessibleMessage;
use tokio::{sync::mpsc, task::JoinHandle};
use url::Url;

mod applemusic;
mod bilibili;
mod kugou;
mod netease;
mod provider;
mod soda;
mod tencent;

const CALLBACK_PREFIX: &str = "music";
const CALLBACK_LIMIT: usize = 64;
const MUSIC_PLATFORM_OPTIONS: &[(&str, &str)] = &[
    ("soda", "汽水"),
    ("tencent", "QQ音乐"),
    ("netease", "网易云"),
    ("kugou", "酷狗"),
    ("bilibili", "Bilibili"),
    ("applemusic", "Apple Music"),
];
const MUSIC_QUALITY_OPTIONS: &[(&str, &str)] = &[
    ("standard", "标准"),
    ("high", "高品质"),
    ("lossless", "无损"),
    ("hires", "Hi-Res"),
];
const DOWNLOAD_PROGRESS_MIN_INTERVAL: Duration = Duration::from_secs(2);

cmd!(
    "/music",
    "获取音乐",
    MusicCmd ,
    {
        ///子命令：search 搜索候选，get 直接下载；省略时兼容旧用法
        #[command(subcommand)]
        command: Option<MusicSubcommand>,
        ///兼容旧用法：/music 歌名 或 /music qq 歌名
        #[arg(required = false, num_args = 1.., trailing_var_arg = true)]
        query: Vec<String>,
    }
);

#[derive(Clone, Debug, clap::Subcommand)]
enum MusicSubcommand {
    ///搜索音乐候选
    #[command(alias = "s")]
    Search(MusicSearchCmd),
    ///直接下载第一首或指定 ID/链接
    #[command(alias = "download", alias = "dl")]
    Get(MusicGetCmd),
    ///设置默认平台、Apple Music 音质和封面
    #[command(alias = "settings")]
    Setting,
}

#[derive(Clone, Debug, clap::Args)]
struct MusicSearchCmd {
    ///音乐平台：qq/tencent/netease/163/kugou/bilibili/soda/applemusic
    #[arg(short, long)]
    platform: Option<String>,
    ///音乐名、链接或 ID
    #[arg(required = true, num_args = 1.., trailing_var_arg = true)]
    query: Vec<String>,
}

#[derive(Clone, Debug, clap::Args)]
struct MusicGetCmd {
    ///音乐平台：qq/tencent/netease/163/kugou/bilibili/soda/applemusic
    #[arg(short, long)]
    platform: Option<String>,
    ///音乐名、链接或 ID
    #[arg(required = true, num_args = 1.., trailing_var_arg = true)]
    query: Vec<String>,
}

#[derive(Clone, Debug)]
enum MusicCallbackAction {
    SearchMore(MusicQuery),
    Select {
        platform: MusicPlatform,
        id: String,
        search_keyword: Option<String>,
    },
    Cover(MusicPlatform, String),
    Setting(MusicSettingAction),
}

#[derive(Clone, Debug)]
enum MusicSettingAction {
    Platform(String),
    Quality(String),
    Cover(bool),
}

impl MusicCallbackAction {
    fn encode(&self) -> Option<String> {
        let payload = match self {
            MusicCallbackAction::SearchMore(query) => format!(
                "{CALLBACK_PREFIX} s {} {}",
                query.platform.callback_code(),
                urlencoding::encode(&query.keyword)
            ),
            MusicCallbackAction::Select {
                platform,
                id,
                search_keyword,
            } => {
                let mut payload = format!("{CALLBACK_PREFIX} d {} {id}", platform.callback_code());
                if let Some(keyword) = search_keyword
                    .as_deref()
                    .map(str::trim)
                    .filter(|keyword| !keyword.is_empty())
                {
                    let with_keyword = format!("{payload} {}", urlencoding::encode(keyword));
                    if with_keyword.len() <= CALLBACK_LIMIT {
                        payload = with_keyword;
                    }
                }
                payload
            }
            MusicCallbackAction::Cover(platform, id) => {
                format!("{CALLBACK_PREFIX} c {} {id}", platform.callback_code())
            }
            MusicCallbackAction::Setting(action) => match action {
                MusicSettingAction::Platform(platform) => {
                    format!("{CALLBACK_PREFIX} setting platform {platform}")
                }
                MusicSettingAction::Quality(quality) => {
                    format!("{CALLBACK_PREFIX} setting quality {quality}")
                }
                MusicSettingAction::Cover(enabled) => format!(
                    "{CALLBACK_PREFIX} setting cover {}",
                    if *enabled { "on" } else { "off" }
                ),
            },
        };
        (payload.len() <= CALLBACK_LIMIT).then_some(payload)
    }

    fn decode(data: &str) -> Result<Self, BotError> {
        let payload = data
            .strip_prefix(CALLBACK_PREFIX)
            .and_then(|data| data.strip_prefix(' '))
            .unwrap_or(data);
        let mut parts = payload.splitn(3, ' ');
        let action = parts
            .next()
            .ok_or_else(|| BotError::Custom("Unknown music callback".to_string()))?;
        if action == "setting" {
            let setting_action = parts
                .next()
                .ok_or_else(|| BotError::Custom("Unknown music setting action".to_string()))?;
            let value = parts
                .next()
                .ok_or_else(|| BotError::Custom("Unknown music setting data".to_string()))?;
            return match setting_action {
                "platform" => Ok(MusicCallbackAction::Setting(MusicSettingAction::Platform(
                    value.to_string(),
                ))),
                "quality" => Ok(MusicCallbackAction::Setting(MusicSettingAction::Quality(
                    value.to_string(),
                ))),
                "cover" => Ok(MusicCallbackAction::Setting(MusicSettingAction::Cover(
                    value == "on",
                ))),
                _ => Err(BotError::Custom("Unknown music setting action".to_string())),
            };
        }
        let platform = parts
            .next()
            .and_then(MusicPlatform::from_callback_code)
            .ok_or_else(|| BotError::Custom("Unknown music platform".to_string()))?;
        let value = parts
            .next()
            .ok_or_else(|| BotError::Custom("Unknown music callback data".to_string()))?;

        match action {
            "s" => Ok(MusicCallbackAction::SearchMore(MusicQuery {
                platform,
                keyword: urlencoding::decode(value)
                    .map_err(|e| BotError::Custom(e.to_string()))?
                    .into_owned(),
            })),
            "d" => {
                let (id, search_keyword) = value.split_once(' ').map_or_else(
                    || (value.to_string(), None),
                    |(id, keyword)| {
                        (
                            id.to_string(),
                            urlencoding::decode(keyword)
                                .ok()
                                .map(|keyword| keyword.into_owned())
                                .filter(|keyword| !keyword.trim().is_empty()),
                        )
                    },
                );
                Ok(MusicCallbackAction::Select {
                    platform,
                    id,
                    search_keyword,
                })
            }
            "c" => Ok(MusicCallbackAction::Cover(platform, value.to_string())),
            _ => Err(BotError::Custom(
                "Unknown music callback action".to_string(),
            )),
        }
    }
}

fn provider_for(platform: MusicPlatform) -> &'static dyn MusicProvider {
    match platform {
        MusicPlatform::AppleMusic => &applemusic::APPLE_MUSIC_PROVIDER,
        MusicPlatform::Bilibili => &bilibili::BILIBILI_PROVIDER,
        MusicPlatform::Kugou => &kugou::KUGOU_PROVIDER,
        MusicPlatform::Netease => &netease::NETEASE_PROVIDER,
        MusicPlatform::Soda => &soda::SODA_PROVIDER,
        MusicPlatform::Tencent => &tencent::TENCENT_PROVIDER,
    }
}

async fn search_tracks(query: &MusicQuery, limit: usize) -> Result<Vec<MusicSearchItem>, BotError> {
    provider_for(query.platform)
        .search(&query.keyword, limit)
        .await
}

async fn resolve_track(
    query: &MusicQuery,
    selected_id: Option<&str>,
    settings: &UserMusicSettings,
) -> Result<MusicTrack, BotError> {
    if query.platform == MusicPlatform::AppleMusic {
        return applemusic::resolve_with_quality(&query.keyword, selected_id, &settings.quality)
            .await;
    }
    if query.platform == MusicPlatform::Soda {
        return soda::resolve_with_quality(&query.keyword, selected_id, &settings.quality).await;
    }
    provider_for(query.platform)
        .resolve(&query.keyword, selected_id)
        .await
}

fn settings_platform(settings: &UserMusicSettings) -> MusicPlatform {
    MusicPlatform::from_stored_default(&settings.default_platform)
}

fn user_id_from_message(msg: &Message) -> i64 {
    msg.from.as_ref().map(|user| user.id.0 as i64).unwrap_or(0)
}

fn music_settings_text(settings: &UserMusicSettings) -> String {
    format!(
        "音乐设置\n\n🎵 默认平台: {}\n🎧 默认音质: {}\n🖼️ 发送封面: {}\n\n点击下方按钮修改设置",
        platform_label(&settings.default_platform),
        quality_label(&settings.quality),
        if settings.send_cover {
            "开启"
        } else {
            "关闭"
        }
    )
}

fn music_settings_menu(settings: &UserMusicSettings) -> InlineKeyboardMarkup {
    let mut keyboard: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    for chunk in MUSIC_PLATFORM_OPTIONS.chunks(3) {
        keyboard.push(
            chunk
                .iter()
                .filter_map(|(value, label)| {
                    MusicCallbackAction::Setting(MusicSettingAction::Platform(value.to_string()))
                        .encode()
                        .map(|callback| {
                            InlineKeyboardButton::callback(
                                selected_label(label, settings.default_platform == *value),
                                callback,
                            )
                        })
                })
                .collect(),
        );
    }

    keyboard.push(
        MUSIC_QUALITY_OPTIONS
            .iter()
            .filter_map(|(value, label)| {
                MusicCallbackAction::Setting(MusicSettingAction::Quality(value.to_string()))
                    .encode()
                    .map(|callback| {
                        InlineKeyboardButton::callback(
                            selected_label(label, settings.quality == *value),
                            callback,
                        )
                    })
            })
            .collect(),
    );

    if let Some(callback) =
        MusicCallbackAction::Setting(MusicSettingAction::Cover(!settings.send_cover)).encode()
    {
        keyboard.push(vec![InlineKeyboardButton::callback(
            format!("发送封面 {}", if settings.send_cover { "✅" } else { "❌" }),
            callback,
        )]);
    }

    InlineKeyboardMarkup::new(keyboard)
}

fn selected_label(label: &str, selected: bool) -> String {
    if selected {
        format!("✅ {label}")
    } else {
        label.to_string()
    }
}

fn platform_label(value: &str) -> &'static str {
    MUSIC_PLATFORM_OPTIONS
        .iter()
        .find(|(id, _)| *id == value)
        .map(|(_, label)| *label)
        .unwrap_or("QQ音乐")
}

fn quality_label(value: &str) -> &'static str {
    MUSIC_QUALITY_OPTIONS
        .iter()
        .find(|(id, _)| *id == value)
        .map(|(_, label)| *label)
        .unwrap_or("高品质")
}

async fn send_music_settings(bot: &Bot, msg: &Message, settings: &UserMusicSettings) -> BotResult {
    bot.send_message(msg.chat.id, music_settings_text(settings))
        .reply_parameters(ReplyParameters::new(msg.id))
        .reply_markup(music_settings_menu(settings))
        .await?;
    Ok(())
}

async fn edit_music_settings(bot: &Bot, msg: &Message, settings: &UserMusicSettings) -> BotResult {
    bot.edit_message_text(msg.chat.id, msg.id, music_settings_text(settings))
        .reply_markup(music_settings_menu(settings))
        .await?;
    Ok(())
}

async fn save_music_settings(
    bot: &Bot,
    callback_id: &str,
    settings: &UserMusicSettings,
) -> BotResult {
    if let Err(e) = save_user_settings(settings).await {
        let _ = bot
            .answer_callback_query(callback_id.to_string())
            .text("保存音乐设置失败")
            .show_alert(true)
            .await;
        return Err(e);
    }
    Ok(())
}

async fn handle_setting_callback(
    bot: &Bot,
    callback_id: &str,
    msg: &Message,
    user_id: i64,
    action: MusicSettingAction,
) -> BotResult {
    let mut settings = match get_user_settings(user_id).await {
        Ok(settings) => settings,
        Err(e) => {
            let _ = bot
                .answer_callback_query(callback_id.to_string())
                .text("获取音乐设置失败")
                .show_alert(true)
                .await;
            return Err(e);
        }
    };

    let mut changed = false;
    let response_text = match action {
        MusicSettingAction::Platform(platform) => {
            if !MUSIC_PLATFORM_OPTIONS
                .iter()
                .any(|(value, _)| *value == platform)
            {
                bot.answer_callback_query(callback_id.to_string())
                    .text("不支持这个音乐平台")
                    .show_alert(true)
                    .await?;
                return Ok(());
            }
            if settings.default_platform != platform {
                settings.default_platform = platform.clone();
                changed = true;
            }
            format!("已切换到 {}", platform_label(&platform))
        }
        MusicSettingAction::Quality(quality) => {
            if !MUSIC_QUALITY_OPTIONS
                .iter()
                .any(|(value, _)| *value == quality)
            {
                bot.answer_callback_query(callback_id.to_string())
                    .text("不支持这个音质")
                    .show_alert(true)
                    .await?;
                return Ok(());
            }
            if settings.quality != quality {
                settings.quality = quality.clone();
                changed = true;
            }
            format!("音质已设置为 {}", quality_label(&quality))
        }
        MusicSettingAction::Cover(enabled) => {
            if settings.send_cover != enabled {
                settings.send_cover = enabled;
                changed = true;
            }
            if enabled {
                "已开启发送封面".to_string()
            } else {
                "已关闭发送封面".to_string()
            }
        }
    };

    if changed {
        save_music_settings(bot, callback_id, &settings).await?;
        bot.answer_callback_query(callback_id.to_string())
            .text(response_text)
            .await?;
        edit_music_settings(bot, msg, &settings).await?;
    } else {
        bot.answer_callback_query(callback_id.to_string()).await?;
    }
    Ok(())
}

async fn send_track(
    bot: &Bot,
    msg: &Message,
    status_msg: Option<&Message>,
    reply_to: Option<MessageId>,
    query: &MusicQuery,
    track: MusicTrack,
    media: MusicMedia,
) -> Result<(), BotError> {
    let caption = build_music_caption(&track, &media);
    let MusicMedia {
        audio,
        cover,
        decrypt_elapsed: _,
    } = media;
    let thumbnail_task = if cover.is_empty() {
        None
    } else {
        Some(tokio::task::spawn_blocking(move || {
            prepare_audio_thumbnail(&cover)
        }))
    };

    if let Some(status_msg) = status_msg {
        bot.edit_message_text(status_msg.chat.id, status_msg.id, "获取成功，正在上传...")
            .await?;
    }

    let mut request = bot
        .send_audio(
            msg.chat.id,
            InputFile::memory(audio).file_name(track.file_name()),
        )
        .caption(caption)
        .parse_mode(ParseMode::Html)
        .title(track.song.clone())
        .performer(track.singer.clone());
    if let Some(reply_to) = reply_to {
        request = request.reply_parameters(ReplyParameters::new(reply_to));
    }
    if let Some(duration) = track.duration.filter(|duration| *duration > 0) {
        request = request.duration(duration);
    }
    if let Some(markup) = track_menu(&track, Some(query)) {
        request = request.reply_markup(markup);
    }
    if let Some(cover) = match thumbnail_task {
        Some(task) => task.await?,
        None => None,
    } {
        request = request.thumbnail(InputFile::memory(cover));
    }
    request.send().await?;

    Ok(())
}

fn build_music_caption(track: &MusicTrack, media: &MusicMedia) -> String {
    let song = html_escape(&track.song);
    let singer = html_escape(&track.singer);
    let song_html = if track.link.trim().is_empty() {
        song
    } else {
        format!("<a href=\"{}\">{song}</a>", html_escape(&track.link))
    };
    let album_line = if track.album.trim().is_empty() {
        String::new()
    } else {
        format!("专辑: {}\n", html_escape(&track.album))
    };
    let mut info_parts = Vec::new();
    if !media.audio.is_empty() {
        info_parts.push(format_file_size(media.audio.len()));
    }
    let bitrate = track
        .bitrate
        .or_else(|| estimate_bitrate(media.audio.len(), track.duration));
    if let Some(bitrate) = bitrate.filter(|bitrate| *bitrate > 0) {
        info_parts.push(format!("{:.2}kbps", bitrate as f64 / 1000.0));
    }
    let decrypt_line = media
        .decrypt_elapsed
        .map(|elapsed| format!("解密: {}\n", format_duration(elapsed)))
        .unwrap_or_default();
    let info_line = if info_parts.is_empty() {
        String::new()
    } else {
        format!("{}\n", info_parts.join(" "))
    };
    let tags = format!(
        "#{} #{}",
        platform_tag(track.platform),
        html_escape(&track.file_extension())
    );
    format!(
        "<b>「{song_html}」- {singer}</b>\n{album_line}<blockquote>{info_line}{decrypt_line}{tags}\n</blockquote>"
    )
}

fn platform_tag(platform: MusicPlatform) -> &'static str {
    match platform {
        MusicPlatform::AppleMusic => "AppleMusic",
        MusicPlatform::Bilibili => "Bilibili",
        MusicPlatform::Kugou => "Kugou",
        MusicPlatform::Netease => "Netease",
        MusicPlatform::Soda => "Soda",
        MusicPlatform::Tencent => "QQMusic",
    }
}

fn format_file_size(bytes: usize) -> String {
    format!("{:.2}MB", bytes as f64 / 1024.0 / 1024.0)
}

fn format_duration(duration: Duration) -> String {
    if duration.as_secs() > 0 {
        format!("{:.2}s", duration.as_secs_f64())
    } else {
        format!("{}ms", duration.as_millis())
    }
}

fn estimate_bitrate(bytes: usize, duration: Option<u32>) -> Option<u32> {
    let duration = duration.filter(|duration| *duration > 0)? as u64;
    Some(((bytes as u64).saturating_mul(8) / duration) as u32)
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

async fn get_music(
    bot: &Bot,
    query: MusicQuery,
    msg: &Message,
    status_msg: &Message,
    settings: &UserMusicSettings,
) -> Result<(), BotError> {
    let track = resolve_track(&query, None, settings).await?;
    let (mut progress, progress_task) =
        download_progress_updater(bot.clone(), status_msg, track.song.clone());
    let media = provider::download_track_media_with_cover_progress(
        &track,
        settings.send_cover,
        &mut progress,
    )
    .await?;
    drop(progress);
    progress_task.abort();
    let _ = progress_task.await;
    send_track(
        bot,
        msg,
        Some(status_msg),
        Some(msg.id),
        &query,
        track,
        media,
    )
    .await
}

async fn get_music_gui(bot: Bot, msg: Message, query: MusicQuery) -> Result<(), BotError> {
    let tracks = search_tracks(&query, 5).await?;
    let text = if tracks.is_empty() {
        "没有找到可下载的音乐".to_string()
    } else {
        format!("选择你的音乐（{}）", query.platform.label())
    };

    let mut request = bot.send_message(msg.chat.id, text);
    if !tracks.is_empty() {
        request = request.reply_markup(search_menu(tracks, Some(&query.keyword)));
    }
    request.await?;
    Ok(())
}

async fn send_music_search(bot: &Bot, msg: &Message, query: MusicQuery) -> Result<(), BotError> {
    let tracks = search_tracks(&query, 5).await?;
    let text = if tracks.is_empty() {
        "没有找到可下载的音乐".to_string()
    } else {
        format!("选择你的音乐（{}）", query.platform.label())
    };

    let mut request = bot
        .send_message(msg.chat.id, text)
        .reply_parameters(ReplyParameters::new(msg.id));
    if !tracks.is_empty() {
        request = request.reply_markup(search_menu(tracks, Some(&query.keyword)));
    }
    request.await?;
    Ok(())
}

async fn get_music_cover(bot: &Bot, msg: &Message, url: &str) -> Result<(), BotError> {
    bot.send_photo(msg.chat.id, InputFile::url(Url::parse(url)?))
        .reply_parameters(ReplyParameters::new(msg.id))
        .send()
        .await?;
    Ok(())
}

async fn get_callback_cover(
    bot: Bot,
    msg: Message,
    platform: MusicPlatform,
    id: &str,
    settings: &UserMusicSettings,
) -> Result<(), BotError> {
    let query = MusicQuery {
        platform,
        keyword: id.to_string(),
    };
    let track = resolve_track(&query, Some(id), settings).await?;
    if track.cover.trim().is_empty() {
        return Err(BotError::Custom("这首歌没有可用封面".to_string()));
    }
    get_music_cover(&bot, &msg, &track.cover).await
}

async fn get_callback_music(
    bot: Bot,
    msg: Message,
    platform: MusicPlatform,
    id: &str,
    search_keyword: Option<&str>,
    settings: &UserMusicSettings,
) -> Result<(), BotError> {
    edit_callback_status(&bot, &msg, format!("正在从{}获取音乐...", platform.label())).await?;
    let query = MusicQuery {
        platform,
        keyword: id.to_string(),
    };
    let track = resolve_track(&query, Some(id), settings).await?;
    let (mut progress, progress_task) =
        download_progress_updater(bot.clone(), &msg, track.song.clone());
    let media = provider::download_track_media_with_cover_progress(
        &track,
        settings.send_cover,
        &mut progress,
    )
    .await?;
    drop(progress);
    progress_task.abort();
    let _ = progress_task.await;
    let done = format!("已发送：{} - {}", track.song, track.singer);
    let search_query = search_keyword
        .map(str::trim)
        .filter(|keyword| !keyword.is_empty())
        .map(|keyword| MusicQuery {
            platform,
            keyword: keyword.to_string(),
        });
    let query_for_menu = search_query.as_ref().unwrap_or(&query);
    let status_msg = message_has_text(&msg).then_some(&msg);
    send_track(
        &bot,
        &msg,
        status_msg,
        callback_reply_target(&msg),
        query_for_menu,
        track,
        media,
    )
    .await?;
    delete_callback_status(&bot, &msg, done).await?;
    Ok(())
}

fn download_progress_updater(
    bot: Bot,
    msg: &Message,
    title: String,
) -> (
    impl FnMut(DownloadProgress) + Send + 'static,
    JoinHandle<()>,
) {
    let chat_id = msg.chat.id;
    let message_id = msg.id;
    let edit_text = message_has_text(msg);
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let task = tokio::spawn(async move {
        let mut last_edited = String::new();
        while let Some(text) = rx.recv().await {
            if text == last_edited {
                continue;
            }
            let result = if edit_text {
                bot.edit_message_text(chat_id, message_id, text.clone())
                    .await
                    .map(|_| ())
            } else {
                bot.edit_message_caption(chat_id, message_id)
                    .caption(text.clone())
                    .await
                    .map(|_| ())
            };
            if result.is_ok() {
                last_edited = text;
            }
        }
    });

    let mut last_progress_text = String::new();
    let mut last_progress_at: Option<Instant> = None;
    let mut reported_complete = false;
    let progress = move |progress: DownloadProgress| {
        let now = Instant::now();
        let is_complete = progress
            .total
            .map(|total| total > 0 && progress.written >= total)
            .unwrap_or(false);
        if is_complete && reported_complete {
            return;
        }
        if !is_complete
            && last_progress_at
                .map(|last| now.duration_since(last) < DOWNLOAD_PROGRESS_MIN_INTERVAL)
                .unwrap_or(false)
        {
            return;
        }

        let text = format_download_progress(&title, progress);
        if text == last_progress_text {
            if is_complete {
                reported_complete = true;
            }
            return;
        }
        last_progress_text = text.clone();
        last_progress_at = Some(now);
        if is_complete {
            reported_complete = true;
        }
        let _ = tx.send(text);
    };

    (progress, task)
}

fn format_download_progress(title: &str, progress: DownloadProgress) -> String {
    let written_mb = bytes_to_mb(progress.written);
    match progress.total {
        Some(total) if total > 0 => {
            let total_mb = bytes_to_mb(total);
            let percent = (progress.written as f64 * 100.0 / total as f64).clamp(0.0, 100.0);
            format!(
                "正在下载：{title}\n进度：{percent:.2}% ({written_mb:.2} MB / {total_mb:.2} MB)"
            )
        }
        _ => format!("正在下载：{title}\n已下载：{written_mb:.2} MB"),
    }
}

fn bytes_to_mb(bytes: u64) -> f64 {
    bytes as f64 / 1024.0 / 1024.0
}

fn prepare_audio_thumbnail(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.is_empty() {
        return None;
    }
    let image = image::load_from_memory(bytes).ok()?;
    let image = DynamicImage::ImageRgb8(image.resize(320, 320, FilterType::Lanczos3).to_rgb8());
    for quality in [85, 75, 65, 55, 45, 35] {
        let mut out = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
        if encoder.encode_image(&image).is_ok() && out.len() <= 200 * 1024 {
            return Some(out);
        }
    }
    let mut out = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Jpeg)
        .ok()?;
    (out.len() <= 200 * 1024).then_some(out)
}

fn message_has_text(msg: &Message) -> bool {
    matches!(
        &msg.kind,
        MessageKind::Common(common) if matches!(common.media_kind, MediaKind::Text(_))
    )
}

fn callback_reply_target(msg: &Message) -> Option<MessageId> {
    if message_has_text(msg) {
        msg.reply_to_message().map(|msg| msg.id)
    } else {
        Some(msg.id)
    }
}

fn empty_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(Vec::<Vec<InlineKeyboardButton>>::new())
}

async fn edit_callback_status(
    bot: &Bot,
    msg: &Message,
    text: impl Into<String>,
) -> Result<(), BotError> {
    edit_callback_status_with_markup(bot, msg, text, None).await
}

async fn edit_callback_status_with_markup(
    bot: &Bot,
    msg: &Message,
    text: impl Into<String>,
    markup: Option<InlineKeyboardMarkup>,
) -> Result<(), BotError> {
    let text = text.into();
    if message_has_text(msg) {
        let mut request = bot.edit_message_text(msg.chat.id, msg.id, text);
        request = match markup {
            Some(markup) => request.reply_markup(markup),
            None => request.reply_markup(empty_keyboard()),
        };
        request.await?;
    } else {
        let mut request = bot.edit_message_caption(msg.chat.id, msg.id).caption(text);
        request = match markup {
            Some(markup) => request.reply_markup(markup),
            None => request.reply_markup(empty_keyboard()),
        };
        request.await?;
    }
    Ok(())
}

async fn delete_callback_status(
    bot: &Bot,
    msg: &Message,
    fallback_text: String,
) -> Result<(), BotError> {
    if message_has_text(msg) {
        let _ = bot.delete_message(msg.chat.id, msg.id).await;
    } else {
        let _ = edit_callback_status(bot, msg, fallback_text).await;
    }
    Ok(())
}

pub async fn music_callback(bot: Bot, q: CallbackQuery) -> Result<(), BotError> {
    if let Some(data) = q.data {
        let callback_id = q.id.clone();
        let user_id = q.from.id.0 as i64;
        let msg = match q.message {
            None => return Ok(()),
            Some(mbi_msg) => match mbi_msg {
                MaybeInaccessibleMessage::Inaccessible(_) => return Ok(()),
                MaybeInaccessibleMessage::Regular(msg) => msg,
            },
        };
        let action = match MusicCallbackAction::decode(&data) {
            Ok(action) => action,
            Err(e) => {
                let _ = bot
                    .answer_callback_query(callback_id)
                    .text(format!("{e}"))
                    .show_alert(true)
                    .await;
                return Err(e);
            }
        };
        let lock_flag = hashing((msg.chat.id, msg.id));
        if LIMITER.is_running(lock_flag) {
            bot.answer_callback_query(callback_id)
                .text("正在处理上一次请求...")
                .await?;
            return Ok(());
        }
        let _guard = Guard::new(&LIMITER, lock_flag);
        if !matches!(action, MusicCallbackAction::Setting(_)) {
            bot.answer_callback_query(callback_id.clone())
                .text("正在处理...")
                .await?;
        }

        let result = match action {
            MusicCallbackAction::SearchMore(query) => {
                get_music_gui(bot.clone(), msg.clone(), query).await
            }
            MusicCallbackAction::Cover(platform, id) => {
                let settings = get_user_settings_or_default(user_id).await;
                get_callback_cover(bot.clone(), msg.clone(), platform, &id, &settings).await
            }
            MusicCallbackAction::Select {
                platform,
                id,
                search_keyword,
            } => {
                let settings = get_user_settings_or_default(user_id).await;
                get_callback_music(
                    bot.clone(),
                    msg.clone(),
                    platform,
                    &id,
                    search_keyword.as_deref(),
                    &settings,
                )
                .await
            }
            MusicCallbackAction::Setting(action) => {
                handle_setting_callback(&bot, &callback_id, &msg, user_id, action).await
            }
        };
        if let Err(e) = result {
            if message_has_text(&msg) {
                let _ = edit_callback_status(&bot, &msg, format!("{e}")).await;
            }
            let _ = bot
                .answer_callback_query(callback_id)
                .text(format!("{e}"))
                .show_alert(true)
                .await;
            return Err(e);
        }
    }
    Ok(())
}

fn track_menu(
    track: &MusicTrack,
    fallback_query: Option<&MusicQuery>,
) -> Option<InlineKeyboardMarkup> {
    let mut buttons = Vec::new();
    if !track.cover.trim().is_empty()
        && !track.id.trim().is_empty()
        && let Some(callback) =
            MusicCallbackAction::Cover(track.platform, track.id.clone()).encode()
    {
        buttons.push(InlineKeyboardButton::callback("获取封面", callback));
    }

    let preferred_query = fallback_query
        .filter(|query| !is_direct_track_lookup(&query.keyword, track))
        .cloned()
        .unwrap_or_else(|| MusicQuery {
            platform: track.platform,
            keyword: track.song.clone(),
        });
    let fallback_song_query = MusicQuery {
        platform: track.platform,
        keyword: track.song.clone(),
    };
    let search_callback = MusicCallbackAction::SearchMore(preferred_query)
        .encode()
        .or_else(|| MusicCallbackAction::SearchMore(fallback_song_query).encode());
    if let Some(callback) = search_callback {
        buttons.push(InlineKeyboardButton::callback("搜索更多", callback));
    }

    (!buttons.is_empty()).then(|| InlineKeyboardMarkup::new([buttons]))
}

fn search_menu(tracks: Vec<MusicSearchItem>, search_keyword: Option<&str>) -> InlineKeyboardMarkup {
    let keyboard = tracks
        .into_iter()
        .filter_map(|track| {
            let callback = MusicCallbackAction::Select {
                platform: track.platform,
                id: track.id,
                search_keyword: search_keyword.map(str::to_string),
            }
            .encode()?;
            Some(vec![InlineKeyboardButton::callback(
                format!(
                    "[{}] {} | {}",
                    track.platform.label(),
                    track.song,
                    track.singer
                ),
                callback,
            )])
        })
        .collect::<Vec<_>>();
    InlineKeyboardMarkup::new(keyboard)
}

fn is_direct_track_lookup(keyword: &str, track: &MusicTrack) -> bool {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return true;
    }
    keyword == track.id
        || keyword.starts_with("http://")
        || keyword.starts_with("https://")
        || keyword
            .strip_prefix(track.platform.id())
            .and_then(|value| value.strip_prefix(':'))
            .map(|value| value == track.id)
            .unwrap_or(false)
}

pub async fn music(bot: &Bot, msg: &Message) -> BotResult {
    tokio::spawn(bot.send_chat_action(msg.chat.id, ChatAction::Typing).send());

    let settings = get_user_settings_or_default(user_id_from_message(msg)).await;
    let language_tag = Some("zh-CN");
    let music =
        MusicCmd::parse_i18n_from_bot(getor(msg).unwrap().split_whitespace(), language_tag)?;
    let action = MusicAction::from_cmd(music, &settings)?;

    let query = match action {
        MusicAction::Setting => return send_music_settings(bot, msg, &settings).await,
        MusicAction::Search(query) => return send_music_search(bot, msg, query).await,
        MusicAction::Get(query) => query,
    };
    let msg_bot = bot
        .send_message(
            msg.chat.id,
            format!("正在从{}获取音乐...", query.platform.label()),
        )
        .reply_parameters(ReplyParameters::new(msg.id))
        .await?;

    match get_music(bot, query, msg, &msg_bot, &settings).await {
        Ok(()) => {
            bot.delete_message(msg_bot.chat.id, msg_bot.id).await?;
        }
        Err(e) => {
            bot.edit_message_text(msg_bot.chat.id, msg_bot.id, format!("{e}"))
                .await?;
            return Err(e);
        }
    }
    Ok(())
}

enum MusicAction {
    Get(MusicQuery),
    Search(MusicQuery),
    Setting,
}

impl MusicAction {
    fn from_cmd(cmd: MusicCmd, settings: &UserMusicSettings) -> Result<Self, BotError> {
        let default_platform = settings_platform(settings);
        match cmd.command {
            Some(MusicSubcommand::Search(search)) => {
                let platform =
                    parse_optional_platform(search.platform.as_deref(), default_platform)?;
                Ok(Self::Search(provider::build_query_with_platform(
                    platform,
                    &search.query,
                )?))
            }
            Some(MusicSubcommand::Get(get)) => {
                let platform = parse_optional_platform(get.platform.as_deref(), default_platform)?;
                Ok(Self::Get(provider::build_query_with_platform(
                    platform, &get.query,
                )?))
            }
            Some(MusicSubcommand::Setting) => Ok(Self::Setting),
            None => Ok(Self::Get(provider::build_legacy_query_with_default(
                default_platform,
                &cmd.query,
            )?)),
        }
    }
}

fn parse_optional_platform(
    platform_alias: Option<&str>,
    default_platform: MusicPlatform,
) -> Result<MusicPlatform, BotError> {
    match platform_alias {
        Some(platform_alias) => MusicPlatform::from_alias(platform_alias)
            .ok_or_else(|| BotError::Custom(format!("暂不支持音乐平台：{platform_alias}"))),
        None => Ok(default_platform),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, ImageFormat, Rgb};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn test_settings(default_platform: &str) -> UserMusicSettings {
        UserMusicSettings {
            user_id: 1,
            default_platform: default_platform.to_string(),
            quality: "high".to_string(),
            send_cover: true,
        }
    }

    #[test]
    fn parses_platform_prefix() {
        let args = vec!["qq".to_string(), "晴天".to_string()];
        let query = provider::build_legacy_query(&args).unwrap();
        assert_eq!(query.platform, MusicPlatform::Tencent);
        assert_eq!(query.keyword, "晴天");
    }

    #[test]
    fn parses_platform_flag() {
        let cmd = MusicCmd::parse_i18n_from_bot(
            ["/music", "get", "-p", "qqmusic", "夜曲"],
            Some("zh-CN"),
        )
        .unwrap();
        let MusicAction::Get(query) =
            MusicAction::from_cmd(cmd, &test_settings("tencent")).unwrap()
        else {
            panic!("expected get action");
        };
        assert_eq!(query.platform, MusicPlatform::Tencent);
        assert_eq!(query.keyword, "夜曲");
    }

    #[test]
    fn parses_search_subcommand() {
        let cmd = MusicCmd::parse_i18n_from_bot(
            ["/music", "search", "--platform", "netease", "稻香"],
            Some("zh-CN"),
        )
        .unwrap();
        let MusicAction::Search(query) =
            MusicAction::from_cmd(cmd, &test_settings("tencent")).unwrap()
        else {
            panic!("expected search action");
        };
        assert_eq!(query.platform, MusicPlatform::Netease);
        assert_eq!(query.keyword, "稻香");
    }

    #[test]
    fn omitted_subcommand_platform_uses_user_default() {
        let cmd =
            MusicCmd::parse_i18n_from_bot(["/music", "search", "稻香"], Some("zh-CN")).unwrap();
        let MusicAction::Search(query) =
            MusicAction::from_cmd(cmd, &test_settings("netease")).unwrap()
        else {
            panic!("expected search action");
        };
        assert_eq!(query.platform, MusicPlatform::Netease);
        assert_eq!(query.keyword, "稻香");
    }

    #[test]
    fn parses_setting_subcommand() {
        let cmd = MusicCmd::parse_i18n_from_bot(["/music", "setting"], Some("zh-CN")).unwrap();
        let MusicAction::Setting = MusicAction::from_cmd(cmd, &test_settings("tencent")).unwrap()
        else {
            panic!("expected setting action");
        };
    }

    #[test]
    fn parses_netease_alias() {
        let args = vec!["网易云".to_string(), "稻香".to_string()];
        let query = provider::build_legacy_query(&args).unwrap();
        assert_eq!(query.platform, MusicPlatform::Netease);
        assert_eq!(query.keyword, "稻香");
    }

    #[test]
    fn parses_added_provider_aliases() {
        let kugou = provider::build_query("kugou", &["晴天".to_string()]).unwrap();
        assert_eq!(kugou.platform, MusicPlatform::Kugou);
        let bili = provider::build_query("bilibili", &["晴天".to_string()]).unwrap();
        assert_eq!(bili.platform, MusicPlatform::Bilibili);
        let soda = provider::build_query("汽水", &["晴天".to_string()]).unwrap();
        assert_eq!(soda.platform, MusicPlatform::Soda);
        let apple = provider::build_query("applemusic", &["晴天".to_string()]).unwrap();
        assert_eq!(apple.platform, MusicPlatform::AppleMusic);
    }

    #[test]
    fn callback_payload_is_short() {
        let query = MusicQuery {
            platform: MusicPlatform::Tencent,
            keyword: "晴天".to_string(),
        };
        let payload = MusicCallbackAction::SearchMore(query).encode().unwrap();
        assert!(payload.len() <= 64);
    }

    #[test]
    fn callback_payload_describes_select_action() {
        let payload = MusicCallbackAction::Select {
            platform: MusicPlatform::Tencent,
            id: "12345".to_string(),
            search_keyword: None,
        }
        .encode()
        .unwrap();
        assert_eq!(payload, "music d t 12345");
        let MusicCallbackAction::Select {
            platform,
            id,
            search_keyword,
        } = MusicCallbackAction::decode(&payload).unwrap()
        else {
            panic!("expected select action");
        };
        assert_eq!(platform, MusicPlatform::Tencent);
        assert_eq!(id, "12345");
        assert_eq!(search_keyword, None);
    }

    #[test]
    fn select_callback_can_preserve_search_keyword() {
        let payload = MusicCallbackAction::Select {
            platform: MusicPlatform::Tencent,
            id: "12345".to_string(),
            search_keyword: Some("晴天".to_string()),
        }
        .encode()
        .unwrap();
        assert_eq!(payload, "music d t 12345 %E6%99%B4%E5%A4%A9");
        let MusicCallbackAction::Select {
            platform,
            id,
            search_keyword,
        } = MusicCallbackAction::decode(&payload).unwrap()
        else {
            panic!("expected select action");
        };
        assert_eq!(platform, MusicPlatform::Tencent);
        assert_eq!(id, "12345");
        assert_eq!(search_keyword.as_deref(), Some("晴天"));
    }

    #[test]
    fn callback_decode_accepts_routed_payload_without_prefix() {
        let MusicCallbackAction::Select { platform, id, .. } =
            MusicCallbackAction::decode("d t 449205").unwrap()
        else {
            panic!("expected select action");
        };
        assert_eq!(platform, MusicPlatform::Tencent);
        assert_eq!(id, "449205");
    }

    #[test]
    fn callback_decode_accepts_added_provider_codes() {
        let MusicCallbackAction::Select { platform, id, .. } =
            MusicCallbackAction::decode("d b BV1BZbSzZEGT").unwrap()
        else {
            panic!("expected bilibili select action");
        };
        assert_eq!(platform, MusicPlatform::Bilibili);
        assert_eq!(id, "BV1BZbSzZEGT");

        let MusicCallbackAction::Select { platform, id, .. } =
            MusicCallbackAction::decode("d k ABCDEF0123456789ABCDEF0123456789").unwrap()
        else {
            panic!("expected kugou select action");
        };
        assert_eq!(platform, MusicPlatform::Kugou);
        assert_eq!(id, "ABCDEF0123456789ABCDEF0123456789");

        let MusicCallbackAction::Select { platform, id, .. } =
            MusicCallbackAction::decode("d s 739105056071").unwrap()
        else {
            panic!("expected soda select action");
        };
        assert_eq!(platform, MusicPlatform::Soda);
        assert_eq!(id, "739105056071");

        let MusicCallbackAction::Select { platform, id, .. } =
            MusicCallbackAction::decode("d a 1440841363").unwrap()
        else {
            panic!("expected apple music select action");
        };
        assert_eq!(platform, MusicPlatform::AppleMusic);
        assert_eq!(id, "1440841363");
    }

    #[test]
    fn callback_decode_accepts_setting_payload() {
        let payload =
            MusicCallbackAction::Setting(MusicSettingAction::Quality("lossless".to_string()))
                .encode()
                .unwrap();
        assert_eq!(payload, "music setting quality lossless");
        let MusicCallbackAction::Setting(MusicSettingAction::Quality(value)) =
            MusicCallbackAction::decode("setting quality lossless").unwrap()
        else {
            panic!("expected quality setting action");
        };
        assert_eq!(value, "lossless");
    }

    #[test]
    fn overlong_search_callback_is_not_rendered() {
        let query = MusicQuery {
            platform: MusicPlatform::Tencent,
            keyword: "很长很长很长很长很长很长的中文搜索词".to_string(),
        };
        assert!(MusicCallbackAction::SearchMore(query).encode().is_none());
    }

    #[test]
    fn apple_wrapper_track_uses_m4a_file_name() {
        let track = MusicTrack {
            id: "1624001324".to_string(),
            platform: MusicPlatform::AppleMusic,
            song: "稻香".to_string(),
            singer: "周杰伦".to_string(),
            album: "魔杰座".to_string(),
            cover: String::new(),
            link: "https://music.apple.com/song/1624001324".to_string(),
            url: "applemusic-wrapper://127.0.0.1/1624001324".to_string(),
            headers: Default::default(),
            duration: Some(223),
            bitrate: None,
            format: Some("m4a".to_string()),
        };
        assert_eq!(track.file_name(), "稻香 - 周杰伦.m4a");
    }

    #[test]
    fn builds_music_caption_like_source_bot() {
        let track = MusicTrack {
            id: "1624001324".to_string(),
            platform: MusicPlatform::AppleMusic,
            song: "稻香".to_string(),
            singer: "周杰伦".to_string(),
            album: "魔杰座".to_string(),
            cover: String::new(),
            link: "https://music.apple.com/song/1624001324".to_string(),
            url: "applemusic-wrapper://127.0.0.1/1624001324".to_string(),
            headers: Default::default(),
            duration: Some(2),
            bitrate: None,
            format: Some("m4a".to_string()),
        };
        let media = MusicMedia {
            audio: vec![0; 1024 * 1024],
            cover: Vec::new(),
            decrypt_elapsed: None,
        };
        assert_eq!(
            build_music_caption(&track, &media),
            "<b>「<a href=\"https://music.apple.com/song/1624001324\">稻香</a>」- 周杰伦</b>\n专辑: 魔杰座\n<blockquote>1.00MB 4194.30kbps\n#AppleMusic #m4a\n</blockquote>"
        );
    }

    #[test]
    fn builds_music_caption_with_decrypt_elapsed() {
        let track = MusicTrack {
            id: "1624001324".to_string(),
            platform: MusicPlatform::AppleMusic,
            song: "稻香".to_string(),
            singer: "周杰伦".to_string(),
            album: "魔杰座".to_string(),
            cover: String::new(),
            link: "https://music.apple.com/song/1624001324".to_string(),
            url: "applemusic-wrapper://127.0.0.1/1624001324".to_string(),
            headers: Default::default(),
            duration: Some(2),
            bitrate: None,
            format: Some("m4a".to_string()),
        };
        let media = MusicMedia {
            audio: vec![0; 1024 * 1024],
            cover: Vec::new(),
            decrypt_elapsed: Some(Duration::from_millis(7850)),
        };
        assert_eq!(
            build_music_caption(&track, &media),
            "<b>「<a href=\"https://music.apple.com/song/1624001324\">稻香</a>」- 周杰伦</b>\n专辑: 魔杰座\n<blockquote>1.00MB 4194.30kbps\n解密: 7.85s\n#AppleMusic #m4a\n</blockquote>"
        );
    }

    #[test]
    fn search_more_falls_back_to_song_for_direct_id_query() {
        let track = MusicTrack {
            id: "1624001324".to_string(),
            platform: MusicPlatform::AppleMusic,
            song: "稻香".to_string(),
            singer: "周杰伦".to_string(),
            album: "魔杰座".to_string(),
            cover: String::new(),
            link: "https://music.apple.com/song/1624001324".to_string(),
            url: "applemusic-wrapper://127.0.0.1/1624001324".to_string(),
            headers: Default::default(),
            duration: Some(223),
            bitrate: None,
            format: Some("m4a".to_string()),
        };
        let query = MusicQuery {
            platform: MusicPlatform::AppleMusic,
            keyword: "1624001324".to_string(),
        };
        let markup = track_menu(&track, Some(&query)).unwrap();
        let callback = markup.inline_keyboard[0]
            .iter()
            .find_map(|button| match &button.kind {
                teloxide::types::InlineKeyboardButtonKind::CallbackData(data)
                    if button.text == "搜索更多" =>
                {
                    Some(data.as_str())
                }
                _ => None,
            })
            .unwrap();
        let MusicCallbackAction::SearchMore(query) = MusicCallbackAction::decode(callback).unwrap()
        else {
            panic!("expected search more action");
        };
        assert_eq!(query.keyword, "稻香");
    }

    #[test]
    fn cover_is_resized_to_telegram_thumbnail() {
        let image = ImageBuffer::from_fn(1200, 1200, |x, y| {
            Rgb([(x % 255) as u8, (y % 255) as u8, ((x + y) % 255) as u8])
        });
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        let thumb = prepare_audio_thumbnail(&bytes).unwrap();
        assert!(thumb.len() <= 200 * 1024);
        assert_eq!(&thumb[..3], &[0xff, 0xd8, 0xff]);
        assert!(prepare_audio_thumbnail(&[]).is_none());
    }

    #[test]
    fn formats_download_progress_with_and_without_total() {
        assert_eq!(
            format_download_progress(
                "晴天",
                DownloadProgress {
                    written: 512 * 1024,
                    total: None,
                }
            ),
            "正在下载：晴天\n已下载：0.50 MB"
        );
        assert_eq!(
            format_download_progress(
                "晴天",
                DownloadProgress {
                    written: 1024 * 1024,
                    total: Some(4 * 1024 * 1024),
                }
            ),
            "正在下载：晴天\n进度：25.00% (1.00 MB / 4.00 MB)"
        );
    }

    #[tokio::test]
    async fn downloads_audio_bytes_without_telegram() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0; 1024];
            let _ = stream.read(&mut buf).await.unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nmusic",
                )
                .await
                .unwrap();
        });

        let bytes = provider::download_url(&format!("http://{addr}/track.mp3"))
            .await
            .unwrap();
        assert_eq!(bytes, b"music");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn downloads_audio_reports_progress_without_telegram() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0; 1024];
            let _ = stream.read(&mut buf).await.unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nmusic",
                )
                .await
                .unwrap();
            stream.write_all(b"bytes").await.unwrap();
        });

        let mut reports = Vec::new();
        let bytes = provider::download_url_with_headers_progress(
            &format!("http://{addr}/track.mp3"),
            &Default::default(),
            &mut |progress| reports.push(progress),
        )
        .await
        .unwrap();
        assert_eq!(bytes, b"musicbytes");
        assert_eq!(
            reports.last(),
            Some(&DownloadProgress {
                written: 10,
                total: Some(10),
            })
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn downloads_track_media_with_provider_headers_without_telegram() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0; 2048];
            let size = stream.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..size]);
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("x-music-token: test-token")
            );
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nmusic",
                )
                .await
                .unwrap();
        });

        let track = MusicTrack {
            id: "1".to_string(),
            platform: MusicPlatform::Bilibili,
            song: "song".to_string(),
            singer: "singer".to_string(),
            album: String::new(),
            cover: String::new(),
            link: "https://example.com".to_string(),
            url: format!("http://{addr}/track.m4a"),
            headers: [("x-music-token".to_string(), "test-token".to_string())]
                .into_iter()
                .collect(),
            duration: None,
            bitrate: None,
            format: Some("m4a".to_string()),
        };
        let media = provider::download_track_media(&track).await.unwrap();
        assert_eq!(media.audio, b"music");
        assert!(media.cover.is_empty());
        server.await.unwrap();
    }
}
