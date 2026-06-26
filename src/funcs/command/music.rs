use super::*;
use crate::{
    dao::{
        mongo::{
            music_favorites::{
                FAVORITE_SCOPE_GROUP, FAVORITE_SCOPE_USER, MusicFavorite, is_favorited,
                list_favorites, remove_favorite, upsert_favorite,
            },
            music_settings::{
                UserMusicSettings, get_user_settings, get_user_settings_or_default,
                save_user_settings,
            },
        },
        mysql::music_cache::{
            MusicCache, delete_cache as delete_music_cache, find_cache as find_music_cache,
            upsert_cache as upsert_music_cache,
        },
    },
    settings::SETTINGS,
};
use ferrous_opencc::{OpenCC, config::BuiltinConfig};
use image::{DynamicImage, ImageFormat, imageops::FilterType};
use lazy_static::lazy_static;
use provider::{
    DownloadProgress, MusicCollection, MusicMedia, MusicPlatform, MusicProvider, MusicQuery,
    MusicSearchItem, MusicTrack,
};
use std::{
    io::Cursor,
    time::{Duration, Instant},
};
use teloxide::types::{
    ChatId, ChosenInlineResult, InlineQuery, InputMedia, InputMediaAudio, InputMediaDocument,
    MaybeInaccessibleMessage,
};
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
const MUSIC_LYRIC_SCRIPT_OPTIONS: &[(&str, &str)] =
    &[("simplified", "简体"), ("traditional", "繁体")];
const DOWNLOAD_PROGRESS_MIN_INTERVAL: Duration = Duration::from_secs(2);

lazy_static! {
    static ref LYRIC_TO_SIMPLIFIED: OpenCC =
        OpenCC::from_config(BuiltinConfig::T2s).expect("builtin OpenCC t2s config");
    static ref LYRIC_TO_TRADITIONAL: OpenCC =
        OpenCC::from_config(BuiltinConfig::S2t).expect("builtin OpenCC s2t config");
}

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
    ///查看收藏列表
    #[command(alias = "fav", alias = "favorites")]
    Favorite(MusicFavoriteCmd),
    ///展开歌单或专辑
    #[command(alias = "playlist", alias = "album")]
    Collection(MusicCollectionCmd),
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

#[derive(Clone, Debug, clap::Args)]
struct MusicFavoriteCmd {
    ///收藏范围：user 个人收藏，group 群收藏
    #[arg(value_enum, default_value_t = MusicFavoriteScopeArg::User)]
    scope: MusicFavoriteScopeArg,
}

#[derive(Clone, Debug, clap::Args)]
struct MusicCollectionCmd {
    ///音乐平台：qq/tencent/netease/163/kugou/bilibili/soda/applemusic
    #[arg(short, long)]
    platform: Option<String>,
    ///歌单或专辑链接/ID
    #[arg(required = true, num_args = 1.., trailing_var_arg = true)]
    query: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
enum MusicFavoriteScopeArg {
    #[value(alias = "u", alias = "me", alias = "personal")]
    User,
    #[value(alias = "g", alias = "chat", alias = "group")]
    Group,
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
    Lyrics(MusicPlatform, String, Option<MusicLyricScript>),
    InlineLyrics(MusicPlatform, String, MusicLyricScript),
    InlineSend {
        platform: MusicPlatform,
        id: String,
        quality: String,
        requester_id: i64,
    },
    Favorite(MusicFavoriteAction),
    Setting(MusicSettingAction),
    Close,
}

#[derive(Clone, Debug)]
enum MusicFavoriteAction {
    Toggle {
        scope: FavoriteScope,
        platform: MusicPlatform,
        id: String,
        chat_id: Option<i64>,
    },
    AskRemove {
        scope: FavoriteScope,
        platform: MusicPlatform,
        id: String,
        chat_id: Option<i64>,
    },
    Remove {
        scope: FavoriteScope,
        platform: MusicPlatform,
        id: String,
        chat_id: Option<i64>,
    },
    List {
        scope: FavoriteScope,
        chat_id: Option<i64>,
    },
    Close,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FavoriteScope {
    User,
    Group,
}

impl FavoriteScope {
    fn callback_code(self) -> &'static str {
        match self {
            FavoriteScope::User => "u",
            FavoriteScope::Group => "g",
        }
    }

    fn from_callback_code(code: &str) -> Option<Self> {
        match code {
            "u" => Some(FavoriteScope::User),
            "g" => Some(FavoriteScope::Group),
            _ => None,
        }
    }

    fn storage_key(self) -> &'static str {
        match self {
            FavoriteScope::User => FAVORITE_SCOPE_USER,
            FavoriteScope::Group => FAVORITE_SCOPE_GROUP,
        }
    }

    fn label(self) -> &'static str {
        match self {
            FavoriteScope::User => "个人收藏",
            FavoriteScope::Group => "群收藏",
        }
    }
}

impl From<MusicFavoriteScopeArg> for FavoriteScope {
    fn from(value: MusicFavoriteScopeArg) -> Self {
        match value {
            MusicFavoriteScopeArg::User => FavoriteScope::User,
            MusicFavoriteScopeArg::Group => FavoriteScope::Group,
        }
    }
}

#[derive(Clone, Debug)]
enum MusicSettingAction {
    Platform(String),
    Quality(String),
    Cover(bool),
    LyricScript(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MusicLyricScript {
    Simplified,
    Traditional,
}

impl MusicLyricScript {
    fn callback_code(self) -> &'static str {
        match self {
            MusicLyricScript::Simplified => "s",
            MusicLyricScript::Traditional => "t",
        }
    }

    fn from_callback_code(value: &str) -> Option<Self> {
        match value {
            "s" => Some(MusicLyricScript::Simplified),
            "t" => Some(MusicLyricScript::Traditional),
            _ => None,
        }
    }

    fn from_stored_value(value: &str) -> Self {
        match value.trim() {
            "traditional" => MusicLyricScript::Traditional,
            _ => MusicLyricScript::Simplified,
        }
    }

    fn label(self) -> &'static str {
        match self {
            MusicLyricScript::Simplified => "简体",
            MusicLyricScript::Traditional => "繁体",
        }
    }

    fn switch_label(self) -> &'static str {
        match self {
            MusicLyricScript::Simplified => "切换繁体",
            MusicLyricScript::Traditional => "切换简体",
        }
    }

    fn toggled(self) -> Self {
        match self {
            MusicLyricScript::Simplified => MusicLyricScript::Traditional,
            MusicLyricScript::Traditional => MusicLyricScript::Simplified,
        }
    }
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
            MusicCallbackAction::Lyrics(platform, id, script) => {
                let mut payload =
                    format!("{CALLBACK_PREFIX} lyric {} {id}", platform.callback_code());
                if let Some(script) = script {
                    payload.push(' ');
                    payload.push_str(script.callback_code());
                }
                payload
            }
            MusicCallbackAction::InlineLyrics(platform, id, script) => {
                format!(
                    "{CALLBACK_PREFIX} ilyric {} {id} {}",
                    platform.callback_code(),
                    script.callback_code()
                )
            }
            MusicCallbackAction::InlineSend {
                platform,
                id,
                quality,
                requester_id,
            } => {
                if !is_callback_token(id) || !is_callback_token(quality) {
                    return None;
                }
                format!(
                    "{CALLBACK_PREFIX} i {} {id} {quality} {requester_id}",
                    platform.callback_code()
                )
            }
            MusicCallbackAction::Favorite(action) => action.encode()?,
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
                MusicSettingAction::LyricScript(script) => {
                    format!("{CALLBACK_PREFIX} setting lyric_script {script}")
                }
            },
            MusicCallbackAction::Close => format!("{CALLBACK_PREFIX} close"),
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
                "lyric_script" => Ok(MusicCallbackAction::Setting(
                    MusicSettingAction::LyricScript(value.to_string()),
                )),
                _ => Err(BotError::Custom("Unknown music setting action".to_string())),
            };
        }
        if action == "fav" {
            let value = payload
                .strip_prefix("fav ")
                .ok_or_else(|| BotError::Custom("Unknown favorite action".to_string()))?;
            return Ok(MusicCallbackAction::Favorite(MusicFavoriteAction::decode(
                value,
            )?));
        }
        if action == "close" {
            return Ok(MusicCallbackAction::Close);
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
            "lyric" => {
                let (id, script) = value.split_once(' ').map_or_else(
                    || (value.to_string(), None),
                    |(id, script)| (id.to_string(), MusicLyricScript::from_callback_code(script)),
                );
                Ok(MusicCallbackAction::Lyrics(platform, id, script))
            }
            "ilyric" => {
                let (id, script) = value
                    .split_once(' ')
                    .ok_or_else(|| BotError::Custom("Unknown inline lyric data".to_string()))?;
                let script = MusicLyricScript::from_callback_code(script)
                    .ok_or_else(|| BotError::Custom("Unknown inline lyric script".to_string()))?;
                Ok(MusicCallbackAction::InlineLyrics(
                    platform,
                    id.to_string(),
                    script,
                ))
            }
            "i" => {
                let mut parts = value.split_whitespace();
                let id = parts
                    .next()
                    .ok_or_else(|| BotError::Custom("Unknown inline music track".to_string()))?
                    .to_string();
                let quality = parts
                    .next()
                    .ok_or_else(|| BotError::Custom("Unknown inline music quality".to_string()))?
                    .to_string();
                let requester_id = parts
                    .next()
                    .ok_or_else(|| BotError::Custom("Unknown inline music requester".to_string()))?
                    .parse()
                    .map_err(|_| BotError::Custom("Unknown inline music requester".to_string()))?;
                if parts.next().is_some() {
                    return Err(BotError::Custom(
                        "Invalid inline music callback".to_string(),
                    ));
                }
                Ok(MusicCallbackAction::InlineSend {
                    platform,
                    id,
                    quality,
                    requester_id,
                })
            }
            _ => Err(BotError::Custom(
                "Unknown music callback action".to_string(),
            )),
        }
    }
}

impl MusicFavoriteAction {
    fn encode(&self) -> Option<String> {
        let payload = match self {
            MusicFavoriteAction::Toggle {
                scope,
                platform,
                id,
                chat_id,
            } => favorite_callback_payload("t", *scope, *platform, id, *chat_id)?,
            MusicFavoriteAction::AskRemove {
                scope,
                platform,
                id,
                chat_id,
            } => favorite_callback_payload("ask", *scope, *platform, id, *chat_id)?,
            MusicFavoriteAction::Remove {
                scope,
                platform,
                id,
                chat_id,
            } => favorite_callback_payload("rm", *scope, *platform, id, *chat_id)?,
            MusicFavoriteAction::List { scope, chat_id } => match scope {
                FavoriteScope::User => {
                    format!("{CALLBACK_PREFIX} fav list {}", scope.callback_code())
                }
                FavoriteScope::Group => format!(
                    "{CALLBACK_PREFIX} fav list {} {}",
                    scope.callback_code(),
                    (*chat_id)?
                ),
            },
            MusicFavoriteAction::Close => format!("{CALLBACK_PREFIX} fav close"),
        };
        (payload.len() <= CALLBACK_LIMIT).then_some(payload)
    }

    fn decode(value: &str) -> Result<Self, BotError> {
        let mut parts = value.split_whitespace();
        let action = parts
            .next()
            .ok_or_else(|| BotError::Custom("Unknown favorite action".to_string()))?;
        if action == "close" {
            return Ok(MusicFavoriteAction::Close);
        }
        if action == "list" {
            let scope = parts
                .next()
                .and_then(FavoriteScope::from_callback_code)
                .ok_or_else(|| BotError::Custom("Unknown favorite scope".to_string()))?;
            let chat_id = if scope == FavoriteScope::Group {
                Some(parse_callback_i64(parts.next(), "Unknown favorite chat")?)
            } else {
                None
            };
            return Ok(MusicFavoriteAction::List { scope, chat_id });
        }

        let scope = parts
            .next()
            .and_then(FavoriteScope::from_callback_code)
            .ok_or_else(|| BotError::Custom("Unknown favorite scope".to_string()))?;
        let chat_id = if scope == FavoriteScope::Group {
            Some(parse_callback_i64(parts.next(), "Unknown favorite chat")?)
        } else {
            None
        };
        let platform = parts
            .next()
            .and_then(MusicPlatform::from_callback_code)
            .ok_or_else(|| BotError::Custom("Unknown favorite platform".to_string()))?;
        let id = parts
            .next()
            .ok_or_else(|| BotError::Custom("Unknown favorite track".to_string()))?
            .to_string();
        if parts.next().is_some() {
            return Err(BotError::Custom("Invalid favorite callback".to_string()));
        }

        match action {
            "t" => Ok(MusicFavoriteAction::Toggle {
                scope,
                platform,
                id,
                chat_id,
            }),
            "ask" => Ok(MusicFavoriteAction::AskRemove {
                scope,
                platform,
                id,
                chat_id,
            }),
            "rm" => Ok(MusicFavoriteAction::Remove {
                scope,
                platform,
                id,
                chat_id,
            }),
            _ => Err(BotError::Custom("Unknown favorite action".to_string())),
        }
    }
}

fn favorite_callback_payload(
    action: &str,
    scope: FavoriteScope,
    platform: MusicPlatform,
    id: &str,
    chat_id: Option<i64>,
) -> Option<String> {
    if !is_callback_token(id) {
        return None;
    }
    match scope {
        FavoriteScope::User => Some(format!(
            "{CALLBACK_PREFIX} fav {action} {} {} {id}",
            scope.callback_code(),
            platform.callback_code(),
        )),
        FavoriteScope::Group => Some(format!(
            "{CALLBACK_PREFIX} fav {action} {} {} {} {id}",
            scope.callback_code(),
            chat_id?,
            platform.callback_code(),
        )),
    }
}

fn parse_callback_i64(value: Option<&str>, error: &str) -> Result<i64, BotError> {
    value
        .ok_or_else(|| BotError::Custom(error.to_string()))?
        .parse()
        .map_err(|_| BotError::Custom(error.to_string()))
}

fn is_callback_token(value: &str) -> bool {
    !value.trim().is_empty() && value.bytes().all(|byte| !byte.is_ascii_whitespace())
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

async fn search_collection(
    query: &MusicQuery,
    limit: usize,
) -> Result<Option<MusicCollection>, BotError> {
    provider_for(query.platform)
        .collection(&query.keyword, limit)
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
    if query.platform == MusicPlatform::Tencent {
        return tencent::resolve_with_quality(&query.keyword, selected_id, &settings.quality).await;
    }
    if query.platform == MusicPlatform::Netease {
        return netease::resolve_with_quality(&query.keyword, selected_id, &settings.quality).await;
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
        "音乐设置\n\n🎵 默认平台: {}\n🎧 默认音质: {}\n🖼️ 发送封面: {}\n📝 歌词文字: {}\n\n点击下方按钮修改设置",
        platform_label(&settings.default_platform),
        quality_label(&settings.quality),
        if settings.send_cover {
            "开启"
        } else {
            "关闭"
        },
        lyric_script_label(&settings.lyric_script),
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

    keyboard.push(
        MUSIC_LYRIC_SCRIPT_OPTIONS
            .iter()
            .filter_map(|(value, label)| {
                MusicCallbackAction::Setting(MusicSettingAction::LyricScript(value.to_string()))
                    .encode()
                    .map(|callback| {
                        InlineKeyboardButton::callback(
                            selected_label(label, settings.lyric_script == *value),
                            callback,
                        )
                    })
            })
            .collect(),
    );

    InlineKeyboardMarkup::new(keyboard)
}

fn inline_music_settings_menu(settings: &UserMusicSettings) -> InlineKeyboardMarkup {
    let mut keyboard = music_settings_menu(settings).inline_keyboard;
    if let Some(callback) = MusicCallbackAction::Close.encode() {
        keyboard.push(vec![InlineKeyboardButton::callback("关闭", callback)]);
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
        .unwrap_or("无损")
}

fn lyric_script_label(value: &str) -> &'static str {
    MUSIC_LYRIC_SCRIPT_OPTIONS
        .iter()
        .find(|(id, _)| *id == value)
        .map(|(_, label)| *label)
        .unwrap_or("简体")
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

async fn edit_inline_music_settings(
    bot: &Bot,
    inline_message_id: &str,
    settings: &UserMusicSettings,
) -> BotResult {
    bot.edit_message_text_inline(inline_message_id, music_settings_text(settings))
        .reply_markup(inline_music_settings_menu(settings))
        .await?;
    Ok(())
}

fn favorite_scope_id(scope: FavoriteScope, msg: &Message, user_id: i64) -> Result<i64, BotError> {
    match scope {
        FavoriteScope::User => Ok(user_id),
        FavoriteScope::Group => {
            if is_group_chat(msg) {
                Ok(msg.chat.id.0)
            } else {
                Err(BotError::Custom("群收藏只能在群聊里使用".to_string()))
            }
        }
    }
}

fn is_group_chat(msg: &Message) -> bool {
    msg.chat.is_group() || msg.chat.is_supergroup()
}

fn favorite_from_track(
    scope: FavoriteScope,
    scope_id: i64,
    user: &teloxide::types::User,
    track: &MusicTrack,
) -> MusicFavorite {
    MusicFavorite {
        scope_type: scope.storage_key().to_string(),
        scope_id,
        platform: track.platform.id().to_string(),
        track_id: track.id.clone(),
        added_by_user_id: user.id.0 as i64,
        added_by_name: user.full_name(),
        song: track.song.clone(),
        singer: track.singer.clone(),
        album: track.album.clone(),
        link: track.link.clone(),
        created_at: bson::DateTime::now(),
    }
}

async fn send_music_favorites(
    bot: &Bot,
    msg: &Message,
    user_id: i64,
    scope: FavoriteScope,
) -> BotResult {
    let scope_id = favorite_scope_id(scope, msg, user_id)?;
    let favorites = list_favorites(scope.storage_key(), scope_id, 20).await?;
    bot.send_message(msg.chat.id, favorite_list_text(scope, &favorites))
        .parse_mode(ParseMode::Html)
        .reply_parameters(ReplyParameters::new(msg.id))
        .reply_markup(favorite_list_menu(scope, scope_id, &favorites))
        .await?;
    Ok(())
}

async fn edit_music_favorites(
    bot: &Bot,
    msg: &Message,
    scope: FavoriteScope,
    scope_id: i64,
) -> BotResult {
    let favorites = list_favorites(scope.storage_key(), scope_id, 20).await?;
    bot.edit_message_text(msg.chat.id, msg.id, favorite_list_text(scope, &favorites))
        .parse_mode(ParseMode::Html)
        .reply_markup(favorite_list_menu(scope, scope_id, &favorites))
        .await?;
    Ok(())
}

fn favorite_list_text(scope: FavoriteScope, favorites: &[MusicFavorite]) -> String {
    if favorites.is_empty() {
        return format!("{}为空", scope.label());
    }
    let mut text = format!("<b>{}</b>\n", scope.label());
    for (index, favorite) in favorites.iter().enumerate() {
        let song = html_escape(&favorite.song);
        let singer = html_escape(&favorite.singer);
        let title = if favorite.link.trim().is_empty() {
            song
        } else {
            format!("<a href=\"{}\">{song}</a>", html_escape(&favorite.link))
        };
        let album = if favorite.album.trim().is_empty() {
            String::new()
        } else {
            format!(" · {}", html_escape(&favorite.album))
        };
        text.push_str(&format!("{}. {} - {}{}\n", index + 1, title, singer, album));
    }
    text
}

fn favorite_list_menu(
    scope: FavoriteScope,
    scope_id: i64,
    favorites: &[MusicFavorite],
) -> InlineKeyboardMarkup {
    let mut keyboard = Vec::new();
    for (index, favorite) in favorites.iter().enumerate() {
        let Some(platform) = MusicPlatform::from_alias(&favorite.platform) else {
            continue;
        };
        let send_callback = MusicCallbackAction::Select {
            platform,
            id: favorite.track_id.clone(),
            search_keyword: None,
        }
        .encode();
        let remove_action = MusicFavoriteAction::AskRemove {
            scope,
            platform,
            id: favorite.track_id.clone(),
            chat_id: (scope == FavoriteScope::Group).then_some(scope_id),
        };
        let remove_callback = MusicCallbackAction::Favorite(remove_action).encode();
        let mut row = Vec::new();
        if let Some(callback) = send_callback {
            row.push(InlineKeyboardButton::callback(
                format!("发送 {}", index + 1),
                callback,
            ));
        }
        if let Some(callback) = remove_callback {
            row.push(InlineKeyboardButton::callback(
                format!("取消 {}", index + 1),
                callback,
            ));
        }
        if !row.is_empty() {
            keyboard.push(row);
        }
    }
    keyboard.push(vec![InlineKeyboardButton::callback(
        "关闭",
        MusicCallbackAction::Favorite(MusicFavoriteAction::Close)
            .encode()
            .unwrap(),
    )]);
    InlineKeyboardMarkup::new(keyboard)
}

fn favorite_confirm_menu(
    scope: FavoriteScope,
    scope_id: i64,
    platform: MusicPlatform,
    id: &str,
) -> InlineKeyboardMarkup {
    let chat_id = (scope == FavoriteScope::Group).then_some(scope_id);
    let confirm = MusicCallbackAction::Favorite(MusicFavoriteAction::Remove {
        scope,
        platform,
        id: id.to_string(),
        chat_id,
    })
    .encode();
    let back = MusicCallbackAction::Favorite(MusicFavoriteAction::List { scope, chat_id }).encode();
    let mut row = Vec::new();
    if let Some(confirm) = confirm {
        row.push(InlineKeyboardButton::callback("确认取消", confirm));
    }
    if let Some(back) = back {
        row.push(InlineKeyboardButton::callback("返回列表", back));
    }
    InlineKeyboardMarkup::new([row])
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

    let (changed, response_text) = match apply_music_setting(&mut settings, action) {
        Ok(result) => result,
        Err(message) => {
            bot.answer_callback_query(callback_id.to_string())
                .text(message)
                .show_alert(true)
                .await?;
            return Ok(());
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

async fn handle_inline_setting_callback(
    bot: &Bot,
    callback_id: &str,
    inline_message_id: &str,
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

    let (changed, response_text) = match apply_music_setting(&mut settings, action) {
        Ok(result) => result,
        Err(message) => {
            bot.answer_callback_query(callback_id.to_string())
                .text(message)
                .show_alert(true)
                .await?;
            return Ok(());
        }
    };

    if changed {
        save_music_settings(bot, callback_id, &settings).await?;
        bot.answer_callback_query(callback_id.to_string())
            .text(response_text)
            .await?;
    } else {
        bot.answer_callback_query(callback_id.to_string()).await?;
    }
    edit_inline_music_settings(bot, inline_message_id, &settings).await
}

fn apply_music_setting(
    settings: &mut UserMusicSettings,
    action: MusicSettingAction,
) -> Result<(bool, String), &'static str> {
    let mut changed = false;
    let response_text = match action {
        MusicSettingAction::Platform(platform) => {
            if !MUSIC_PLATFORM_OPTIONS
                .iter()
                .any(|(value, _)| *value == platform)
            {
                return Err("不支持这个音乐平台");
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
                return Err("不支持这个音质");
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
        MusicSettingAction::LyricScript(script) => {
            if !MUSIC_LYRIC_SCRIPT_OPTIONS
                .iter()
                .any(|(value, _)| *value == script)
            {
                return Err("不支持这个歌词文字设置");
            }
            if settings.lyric_script != script {
                settings.lyric_script = script.clone();
                changed = true;
            }
            format!("歌词文字已设置为 {}", lyric_script_label(&script))
        }
    };
    Ok((changed, response_text))
}

async fn send_track(
    bot: &Bot,
    msg: &Message,
    status_msg: Option<&Message>,
    reply_to: Option<MessageId>,
    query: &MusicQuery,
    track: MusicTrack,
    media: MusicMedia,
) -> Result<Message, BotError> {
    let caption = build_music_caption(&track, &media);
    let MusicMedia {
        audio,
        cover,
        decrypt_elapsed: _,
    } = media;
    let audio_file = InputFile::memory(audio).file_name(track.file_name());
    send_track_input(
        bot, msg, status_msg, reply_to, query, &track, caption, audio_file, cover, None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn send_track_input(
    bot: &Bot,
    msg: &Message,
    status_msg: Option<&Message>,
    reply_to: Option<MessageId>,
    query: &MusicQuery,
    track: &MusicTrack,
    caption: String,
    audio_file: InputFile,
    cover: Vec<u8>,
    thumb_file_id: Option<&str>,
) -> Result<Message, BotError> {
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
        .send_audio(msg.chat.id, audio_file)
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
    if let Some(markup) = track_menu(track, Some(query), msg) {
        request = request.reply_markup(markup);
    }
    if let Some(file_id) = thumb_file_id
        .map(str::trim)
        .filter(|file_id| !file_id.is_empty())
    {
        request = request.thumbnail(InputFile::file_id(file_id.to_string()));
    } else if let Some(cover) = match thumbnail_task {
        Some(task) => task.await?,
        None => None,
    } {
        request = request.thumbnail(InputFile::memory(cover));
    }
    let sent = request.send().await?;

    Ok(sent)
}

fn build_music_caption(track: &MusicTrack, media: &MusicMedia) -> String {
    build_music_caption_with_size(track, media.audio.len() as u64, media.decrypt_elapsed)
}

fn build_music_caption_with_size(
    track: &MusicTrack,
    music_size: u64,
    decrypt_elapsed: Option<Duration>,
) -> String {
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
    if music_size > 0 {
        info_parts.push(format_file_size(music_size));
    }
    let bitrate = track
        .bitrate
        .or_else(|| estimate_bitrate(music_size, track.duration));
    if let Some(bitrate) = bitrate.filter(|bitrate| *bitrate > 0) {
        info_parts.push(format!("{:.2}kbps", bitrate as f64 / 1000.0));
    }
    let decrypt_line = decrypt_elapsed
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

fn build_inline_track_caption_with_lyrics(
    track: &MusicTrack,
    music_size: u64,
    lyrics: &provider::MusicLyrics,
    script: MusicLyricScript,
) -> String {
    let content = convert_lyric_script(
        &build_lrc_content(&lyrics.plain, &lyrics.translation),
        script,
    );
    let base = build_music_caption_with_size(track, music_size, None);
    let caption = append_inline_lyric_block(&base, &content, script);
    if caption.chars().count() <= TELEGRAM_CAPTION_LIMIT {
        return caption;
    }
    let compact_base = format!(
        "<b>「{}」- {}</b>",
        html_escape(&track.song),
        html_escape(&track.singer)
    );
    append_inline_lyric_block(&compact_base, &content, script)
}

const TELEGRAM_CAPTION_LIMIT: usize = 1024;

fn append_inline_lyric_block(base: &str, content: &str, script: MusicLyricScript) -> String {
    let header = format!("{base}\n当前歌词文字: {}\n", script.label());
    let wrapper_chars = "<blockquote expandable></blockquote>".chars().count();
    let mut content_limit = TELEGRAM_CAPTION_LIMIT
        .saturating_sub(header.chars().count())
        .saturating_sub(wrapper_chars)
        .saturating_sub(8)
        .max(64);
    loop {
        let content = truncate_chars(content.trim(), content_limit);
        let caption = format!(
            "{header}<blockquote expandable>{}</blockquote>",
            html_escape(&content)
        );
        if caption.chars().count() <= TELEGRAM_CAPTION_LIMIT || content_limit <= 64 {
            return caption;
        }
        content_limit = content_limit.saturating_mul(9) / 10;
    }
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

fn format_file_size(bytes: u64) -> String {
    format!("{:.2}MB", bytes as f64 / 1024.0 / 1024.0)
}

fn format_duration(duration: Duration) -> String {
    if duration.as_secs() > 0 {
        format!("{:.2}s", duration.as_secs_f64())
    } else {
        format!("{}ms", duration.as_millis())
    }
}

fn estimate_bitrate(bytes: u64, duration: Option<u32>) -> Option<u32> {
    let duration = duration.filter(|duration| *duration > 0)? as u64;
    Some((bytes.saturating_mul(8) / duration) as u32)
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

async fn try_send_cached_track(
    bot: &Bot,
    msg: &Message,
    status_msg: Option<&Message>,
    reply_to: Option<MessageId>,
    query: &MusicQuery,
    track: &MusicTrack,
    settings: &UserMusicSettings,
) -> Result<bool, BotError> {
    let quality = music_cache_quality(settings);
    let Some(cache) = find_music_cache(track.platform.id(), &track.id, &quality).await? else {
        return Ok(false);
    };
    if cache.file_id.trim().is_empty() {
        delete_music_cache(track.platform.id(), &track.id, &quality).await?;
        return Ok(false);
    }

    if let Some(status_msg) = status_msg {
        let _ = bot
            .edit_message_text(status_msg.chat.id, status_msg.id, "命中缓存，正在发送...")
            .await;
    }

    match send_cached_track(bot, msg, status_msg, reply_to, query, track, &cache).await {
        Ok(_) => Ok(true),
        Err(err) if is_invalid_cached_file_error(&err) => {
            delete_music_cache(track.platform.id(), &track.id, &quality).await?;
            Ok(false)
        }
        Err(err) => Err(err),
    }
}

async fn send_cached_track(
    bot: &Bot,
    msg: &Message,
    status_msg: Option<&Message>,
    reply_to: Option<MessageId>,
    query: &MusicQuery,
    fallback_track: &MusicTrack,
    cache: &MusicCache,
) -> Result<Message, BotError> {
    let track = cache_to_track(cache).unwrap_or_else(|| fallback_track.clone());
    let caption = build_music_caption_with_size(&track, cache.music_size, None);
    send_track_input(
        bot,
        msg,
        status_msg,
        reply_to,
        query,
        &track,
        caption,
        InputFile::file_id(cache.file_id.clone()),
        Vec::new(),
        Some(&cache.thumb_file_id),
    )
    .await
}

async fn save_track_cache(
    track: &MusicTrack,
    media: &MusicMedia,
    sent: &Message,
    msg: &Message,
    settings: &UserMusicSettings,
) -> BotResult {
    save_track_cache_for_context(
        track,
        media,
        sent,
        user_id_from_message(msg),
        msg.chat.id.0,
        &music_cache_quality(settings),
    )
    .await
}

async fn save_track_cache_for_context(
    track: &MusicTrack,
    media: &MusicMedia,
    sent: &Message,
    from_user_id: i64,
    from_chat_id: i64,
    quality: &str,
) -> BotResult {
    let Some((file_id, thumb_file_id)) = sent_audio_file_ids(sent) else {
        return Ok(());
    };
    if file_id.trim().is_empty() {
        return Ok(());
    }
    let cache = MusicCache {
        platform: track.platform.id().to_string(),
        track_id: track.id.clone(),
        quality: quality.to_string(),
        song: track.song.clone(),
        singer: track.singer.clone(),
        album: track.album.clone(),
        link: track.link.clone(),
        file_ext: track.file_extension(),
        music_size: media.audio.len() as u64,
        bitrate: track
            .bitrate
            .or_else(|| estimate_bitrate(media.audio.len() as u64, track.duration))
            .unwrap_or_default(),
        duration: track.duration,
        file_id,
        thumb_file_id: thumb_file_id.unwrap_or_default(),
        from_user_id,
        from_chat_id,
    };
    upsert_music_cache(&cache).await
}

fn sent_audio_file_ids(msg: &Message) -> Option<(String, Option<String>)> {
    let audio = match &msg.kind {
        MessageKind::Common(common) => match &common.media_kind {
            MediaKind::Audio(audio) => &audio.audio,
            _ => return None,
        },
        _ => return None,
    };
    Some((
        audio.file.id.clone(),
        audio
            .thumbnail
            .as_ref()
            .map(|thumbnail| thumbnail.file.id.clone()),
    ))
}

fn cache_to_track(cache: &MusicCache) -> Option<MusicTrack> {
    let platform = MusicPlatform::from_alias(&cache.platform)?;
    Some(MusicTrack {
        id: cache.track_id.clone(),
        platform,
        song: cache.song.clone(),
        singer: cache.singer.clone(),
        album: cache.album.clone(),
        cover: String::new(),
        link: cache.link.clone(),
        url: String::new(),
        headers: Default::default(),
        duration: cache.duration,
        bitrate: (cache.bitrate > 0).then_some(cache.bitrate),
        format: (!cache.file_ext.trim().is_empty()).then(|| cache.file_ext.clone()),
    })
}

fn music_cache_quality(settings: &UserMusicSettings) -> String {
    let quality = settings.quality.trim();
    if quality.is_empty() {
        "lossless".to_string()
    } else {
        quality.to_ascii_lowercase()
    }
}

fn is_invalid_cached_file_error(err: &BotError) -> bool {
    let text = err.to_string().to_ascii_lowercase();
    text.contains("wrong file identifier")
        || text.contains("file_id")
        || text.contains("file identifier")
        || text.contains("file not found")
}

async fn get_music(
    bot: &Bot,
    query: MusicQuery,
    msg: &Message,
    status_msg: &Message,
    settings: &UserMusicSettings,
) -> Result<(), BotError> {
    let track = resolve_track(&query, None, settings).await?;
    if try_send_cached_track(
        bot,
        msg,
        Some(status_msg),
        Some(msg.id),
        &query,
        &track,
        settings,
    )
    .await?
    {
        return Ok(());
    }
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
    let sent = send_track(
        bot,
        msg,
        Some(status_msg),
        Some(msg.id),
        &query,
        track.clone(),
        media.clone(),
    )
    .await?;
    if let Err(err) = save_track_cache(&track, &media, &sent, msg, settings).await {
        eprintln!("Failed to save music cache: {err}");
    }
    Ok(())
}

pub async fn music_inline_query_handler(bot: Bot, q: InlineQuery) -> BotResult {
    let user_id = q.from.id.0 as i64;
    let settings = get_user_settings_or_default(user_id).await;
    let text = q.query.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("help") {
        return answer_inline_music_help(&bot, &q, &settings).await;
    }

    if let Some(lyric_query) = parse_inline_lyric_query(text) {
        return answer_inline_lyric_query(&bot, &q, lyric_query, &settings).await;
    }

    let (keyword, platform, quality) = parse_inline_music_query(text, &settings);
    if keyword.trim().is_empty() {
        return answer_inline_music_help(&bot, &q, &settings).await;
    }

    let query = MusicQuery {
        platform,
        keyword: keyword.clone(),
    };
    let tracks = search_tracks(&query, 20).await.unwrap_or_default();
    let mut results = Vec::new();
    for track in tracks {
        let cover = (!track.cover.trim().is_empty()).then(|| track.cover.trim().to_string());
        if let Some(result) =
            inline_pending_audio_result(&track, &quality, cover.as_deref(), user_id)
        {
            results.push(result);
        }
    }
    if results.is_empty() {
        results.push(InlineQueryResult::Article(
            InlineQueryResultArticle::new(
                inline_result_id("empty", platform, &keyword, &quality),
                "没有找到可下载的音乐",
                InputMessageContent::Text(InputMessageContentText::new(format!(
                    "没有找到：{}",
                    keyword
                ))),
            )
            .description(format!(
                "{} · {}",
                platform.label(),
                quality_label(&quality)
            )),
        ));
    }
    results.push(inline_settings_result(&settings, user_id));

    bot.answer_inline_query(&q.id, results)
        .is_personal(true)
        .cache_time(1)
        .send()
        .await?;
    Ok(())
}

async fn answer_inline_lyric_query(
    bot: &Bot,
    q: &InlineQuery,
    text: &str,
    settings: &UserMusicSettings,
) -> BotResult {
    let (keyword, platform, quality) = parse_inline_music_query(text, settings);
    if keyword.trim().is_empty() {
        return answer_inline_music_help(bot, q, settings).await;
    }
    let query = MusicQuery {
        platform,
        keyword: keyword.clone(),
    };
    let script = MusicLyricScript::from_stored_value(&settings.lyric_script);
    let mut results = Vec::new();
    for track in search_tracks(&query, 8).await.unwrap_or_default() {
        let Some(lyrics) = provider_for(track.platform)
            .lyrics(&track.id)
            .await
            .ok()
            .flatten()
        else {
            continue;
        };
        let cover = (!track.cover.trim().is_empty()).then(|| track.cover.trim().to_string());
        if let Some(result) = inline_lyric_result(&track, &lyrics, script, cover.as_deref()) {
            results.push(result);
        }
    }
    if results.is_empty() {
        results.push(InlineQueryResult::Article(
            InlineQueryResultArticle::new(
                inline_result_id("lyric_empty", platform, &keyword, &quality),
                "没有找到可用歌词",
                InputMessageContent::Text(InputMessageContentText::new(format!(
                    "没有找到歌词：{}",
                    keyword
                ))),
            )
            .description(platform.label()),
        ));
    }
    bot.answer_inline_query(&q.id, results)
        .is_personal(true)
        .cache_time(1)
        .send()
        .await?;
    Ok(())
}

pub async fn music_chosen_inline_handler(bot: Bot, chosen: ChosenInlineResult) -> BotResult {
    let Some((platform, id, quality)) = parse_inline_pending_result_id(&chosen.result_id) else {
        return Ok(());
    };
    let Some(inline_message_id) = chosen.inline_message_id.as_deref() else {
        log::warn!(
            "chosen inline music result has no inline_message_id: result_id={}",
            chosen.result_id
        );
        return Ok(());
    };
    log::info!(
        "chosen inline music result: platform={} id={} quality={}",
        platform.id(),
        id,
        quality
    );
    run_inline_send_flow(
        &bot,
        inline_message_id,
        &chosen.from,
        platform,
        &id,
        &quality,
    )
    .await
}

async fn answer_inline_music_help(
    bot: &Bot,
    q: &InlineQuery,
    settings: &UserMusicSettings,
) -> BotResult {
    let platform = settings_platform(settings);
    let quality = music_cache_quality(settings);
    let text = format!(
        "输入歌名搜索音乐\n当前平台：{}\n当前音质：{}\n\n也可以在关键词后追加平台或音质，例如：稻香 qq high",
        platform.label(),
        quality_label(&quality)
    );
    let result = InlineQueryResultArticle::new(
        inline_result_id("help", platform, &q.from.id.0.to_string(), &quality),
        "搜索音乐",
        InputMessageContent::Text(InputMessageContentText::new(text)),
    )
    .description(format!(
        "{} · {}",
        platform.label(),
        quality_label(&quality)
    ));
    bot.answer_inline_query(
        &q.id,
        [
            InlineQueryResult::Article(result),
            inline_settings_result(settings, q.from.id.0 as i64),
        ],
    )
    .is_personal(true)
    .cache_time(5)
    .send()
    .await?;
    Ok(())
}

fn inline_settings_result(settings: &UserMusicSettings, user_id: i64) -> InlineQueryResult {
    let platform = settings_platform(settings);
    let quality = music_cache_quality(settings);
    InlineQueryResult::Article(
        InlineQueryResultArticle::new(
            inline_result_id("settings", platform, &user_id.to_string(), &quality),
            format!(
                "平台：{} | 音质：{}",
                platform.label(),
                quality_label(&quality)
            ),
            InputMessageContent::Text(InputMessageContentText::new(music_settings_text(settings))),
        )
        .description("点击修改设置，可在关键词后临时追加参数")
        .reply_markup(inline_music_settings_menu(settings)),
    )
}

fn parse_inline_music_query(
    text: &str,
    settings: &UserMusicSettings,
) -> (String, MusicPlatform, String) {
    let mut parts = text
        .split_whitespace()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let mut platform = settings_platform(settings);
    let mut quality = music_cache_quality(settings);

    for _ in 0..2 {
        let Some(last) = parts.last().map(|part| part.trim().to_string()) else {
            break;
        };
        if let Some(parsed_platform) = MusicPlatform::from_alias(&last) {
            platform = parsed_platform;
            parts.pop();
            continue;
        }
        if MUSIC_QUALITY_OPTIONS
            .iter()
            .any(|(value, _)| *value == last.as_str())
        {
            quality = last;
            parts.pop();
            continue;
        }
        break;
    }

    (parts.join(" "), platform, quality)
}

fn parse_inline_lyric_query(text: &str) -> Option<&str> {
    let text = text.trim();
    text.strip_prefix("歌词 ")
        .or_else(|| text.strip_prefix("lyric "))
        .or_else(|| text.strip_prefix("lyrics "))
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

fn inline_pending_audio_result(
    track: &MusicSearchItem,
    quality: &str,
    cover: Option<&str>,
    requester_id: i64,
) -> Option<InlineQueryResult> {
    let result_id = inline_pending_result_id(track.platform, &track.id, quality)?;
    let callback = MusicCallbackAction::InlineSend {
        platform: track.platform,
        id: track.id.clone(),
        quality: quality.to_string(),
        requester_id,
    }
    .encode()?;
    let title = format!("{} - {}", track.song, track.singer);
    let mut result = InlineQueryResultArticle::new(
        result_id,
        title.clone(),
        InputMessageContent::Text(InputMessageContentText::new(format!("正在准备：{title}"))),
    )
    .description(format!(
        "{} · {}",
        track.platform.label(),
        quality_label(quality)
    ))
    .reply_markup(InlineKeyboardMarkup::new([[
        InlineKeyboardButton::callback("没反应？点此刷新", callback),
    ]]));
    if let Some(url) = cover.and_then(|url| Url::parse(url).ok()) {
        result = result
            .thumbnail_url(url)
            .thumbnail_width(150)
            .thumbnail_height(150);
    }
    Some(InlineQueryResult::Article(result))
}

fn inline_lyric_result(
    track: &MusicSearchItem,
    lyrics: &provider::MusicLyrics,
    script: MusicLyricScript,
    cover: Option<&str>,
) -> Option<InlineQueryResult> {
    let message = inline_lyric_text(&track.song, &track.singer, lyrics, script);
    let title = format!("歌词：{} - {}", track.song, track.singer);
    let mut result = InlineQueryResultArticle::new(
        inline_result_id("lyric", track.platform, &track.id, script.callback_code()),
        title,
        InputMessageContent::Text(
            InputMessageContentText::new(message).parse_mode(ParseMode::Html),
        ),
    )
    .description(format!("{} · {}", track.platform.label(), script.label()))
    .reply_markup(inline_lyric_switch_menu(track.platform, &track.id, script));
    if let Some(url) = cover.and_then(|url| Url::parse(url).ok()) {
        result = result
            .thumbnail_url(url)
            .thumbnail_width(150)
            .thumbnail_height(150);
    }
    Some(InlineQueryResult::Article(result))
}

fn inline_lyric_text(
    song: &str,
    singer: &str,
    lyrics: &provider::MusicLyrics,
    script: MusicLyricScript,
) -> String {
    let content = convert_lyric_script(
        &build_lrc_content(&lyrics.plain, &lyrics.translation),
        script,
    );
    build_inline_lyric_html(song, singer, &content, script)
}

#[cfg(test)]
fn truncate_inline_text(text: &str) -> String {
    const MAX_INLINE_TEXT_CHARS: usize = 4096;
    if text.chars().count() <= MAX_INLINE_TEXT_CHARS {
        return text.to_string();
    }
    let mut truncated = text
        .chars()
        .take(MAX_INLINE_TEXT_CHARS.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut truncated = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

fn build_inline_lyric_html(
    song: &str,
    singer: &str,
    content: &str,
    script: MusicLyricScript,
) -> String {
    const MAX_INLINE_TEXT_CHARS: usize = 4096;
    let header = format!(
        "<b>{} - {}</b>\n当前歌词文字: {}\n",
        html_escape(song),
        html_escape(singer),
        script.label()
    );
    let wrapper_chars = "<blockquote expandable></blockquote>".chars().count();
    let mut content_limit = MAX_INLINE_TEXT_CHARS
        .saturating_sub(header.chars().count())
        .saturating_sub(wrapper_chars)
        .saturating_sub(8)
        .max(128);
    loop {
        let content = truncate_chars(content.trim(), content_limit);
        let html = format!(
            "{header}<blockquote expandable>{}</blockquote>",
            html_escape(&content)
        );
        if html.chars().count() <= MAX_INLINE_TEXT_CHARS || content_limit <= 128 {
            return html;
        }
        content_limit = content_limit.saturating_mul(9) / 10;
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_inline_send_callback(
    bot: &Bot,
    callback_id: &str,
    inline_message_id: &str,
    from: &teloxide::types::User,
    platform: MusicPlatform,
    id: &str,
    quality: &str,
    requester_id: i64,
) -> BotResult {
    if requester_id != 0 && requester_id != from.id.0 as i64 {
        bot.answer_callback_query(callback_id.to_string())
            .text("只能由发起者操作")
            .show_alert(true)
            .await?;
        return Ok(());
    }

    bot.answer_callback_query(callback_id.to_string())
        .text("正在处理...")
        .await?;

    run_inline_send_flow(bot, inline_message_id, from, platform, id, quality).await
}

async fn run_inline_send_flow(
    bot: &Bot,
    inline_message_id: &str,
    from: &teloxide::types::User,
    platform: MusicPlatform,
    id: &str,
    quality: &str,
) -> BotResult {
    let lock_flag = hashing(("music-inline", inline_message_id));
    if LIMITER.is_running(lock_flag) {
        return Ok(());
    }
    let _guard = Guard::new(&LIMITER, lock_flag);

    let mut settings = get_user_settings_or_default(from.id.0 as i64).await;
    settings.quality = quality.to_string();
    let query = MusicQuery {
        platform,
        keyword: id.to_string(),
    };

    edit_inline_status(
        bot,
        inline_message_id,
        format!("正在从{}获取音乐...", platform.label()),
    )
    .await;
    let track = resolve_track(&query, Some(id), &settings).await?;
    if try_edit_inline_cached_track(bot, inline_message_id, &track, quality).await? {
        return Ok(());
    }

    let (mut progress, progress_task) = inline_download_progress_updater(
        bot.clone(),
        inline_message_id.to_string(),
        track.song.clone(),
    );
    let media = provider::download_track_media_with_cover_progress(
        &track,
        settings.send_cover,
        &mut progress,
    )
    .await?;
    drop(progress);
    progress_task.abort();
    let _ = progress_task.await;

    edit_inline_status(bot, inline_message_id, "获取成功，正在上传...").await;
    let upload_chat_id = inline_upload_chat_id();
    let sent = send_inline_upload_audio(bot, upload_chat_id, &track, &media).await?;
    let Some((file_id, _)) = sent_audio_file_ids(&sent) else {
        return Err(BotError::Custom(
            "上传成功但没有拿到 Telegram file_id".to_string(),
        ));
    };
    let _ = bot.delete_message(sent.chat.id, sent.id).await;

    edit_inline_audio(
        bot,
        inline_message_id,
        &track,
        &file_id,
        media.audio.len() as u64,
        media.decrypt_elapsed,
    )
    .await?;
    if let Err(err) = save_track_cache_for_context(
        &track,
        &media,
        &sent,
        from.id.0 as i64,
        upload_chat_id.0,
        quality,
    )
    .await
    {
        eprintln!("Failed to save inline music cache: {err}");
    }
    Ok(())
}

async fn try_edit_inline_cached_track(
    bot: &Bot,
    inline_message_id: &str,
    track: &MusicTrack,
    quality: &str,
) -> Result<bool, BotError> {
    let Some(cache) = find_music_cache(track.platform.id(), &track.id, quality).await? else {
        return Ok(false);
    };
    if cache.file_id.trim().is_empty() {
        delete_music_cache(track.platform.id(), &track.id, quality).await?;
        return Ok(false);
    }
    let track = cache_to_track(&cache).unwrap_or_else(|| track.clone());
    match edit_inline_audio(
        bot,
        inline_message_id,
        &track,
        &cache.file_id,
        cache.music_size,
        None,
    )
    .await
    {
        Ok(()) => Ok(true),
        Err(err) if is_invalid_cached_file_error(&err) => {
            delete_music_cache(track.platform.id(), &track.id, quality).await?;
            Ok(false)
        }
        Err(err) => Err(err),
    }
}

async fn send_inline_upload_audio(
    bot: &Bot,
    chat_id: ChatId,
    track: &MusicTrack,
    media: &MusicMedia,
) -> Result<Message, BotError> {
    let audio_file = InputFile::memory(media.audio.clone()).file_name(track.file_name());
    let mut request = bot
        .send_audio(chat_id, audio_file)
        .caption(build_music_caption(track, media))
        .parse_mode(ParseMode::Html)
        .title(track.song.clone())
        .performer(track.singer.clone());
    if let Some(duration) = track.duration.filter(|duration| *duration > 0) {
        request = request.duration(duration);
    }
    if let Some(cover) = prepare_audio_thumbnail(&media.cover) {
        request = request.thumbnail(InputFile::memory(cover));
    }
    Ok(request.send().await?)
}

async fn edit_inline_audio(
    bot: &Bot,
    inline_message_id: &str,
    track: &MusicTrack,
    file_id: &str,
    music_size: u64,
    decrypt_elapsed: Option<Duration>,
) -> BotResult {
    let mut media = InputMediaAudio::new(InputFile::file_id(file_id.to_string()))
        .caption(build_music_caption_with_size(
            track,
            music_size,
            decrypt_elapsed,
        ))
        .parse_mode(ParseMode::Html)
        .title(track.song.clone())
        .performer(track.singer.clone());
    if let Some(duration) = track.duration.filter(|duration| *duration > 0) {
        media = media.duration(duration.min(u16::MAX as u32) as u16);
    }
    let mut request = bot.edit_message_media_inline(inline_message_id, InputMedia::Audio(media));
    if let Some(markup) = inline_track_menu(track) {
        request = request.reply_markup(markup);
    }
    request.await?;
    Ok(())
}

fn inline_upload_chat_id() -> ChatId {
    let configured = SETTINGS.music.inline_upload_chat_id;
    if configured != 0 {
        ChatId(configured)
    } else {
        ChatId(SETTINGS.bot.owner)
    }
}

async fn edit_inline_status(bot: &Bot, inline_message_id: &str, text: impl Into<String>) {
    let _ = bot
        .edit_message_text_inline(inline_message_id, text.into())
        .await;
}

fn inline_download_progress_updater(
    bot: Bot,
    inline_message_id: String,
    title: String,
) -> (
    impl FnMut(DownloadProgress) + Send + 'static,
    JoinHandle<()>,
) {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let task = tokio::spawn(async move {
        let mut last_edited = String::new();
        while let Some(text) = rx.recv().await {
            if text == last_edited {
                continue;
            }
            if bot
                .edit_message_text_inline(&inline_message_id, text.clone())
                .await
                .is_ok()
            {
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

fn inline_result_id(prefix: &str, platform: MusicPlatform, id: &str, quality: &str) -> String {
    format!(
        "{}_{:x}",
        prefix,
        hashing((platform.callback_code(), id, quality))
    )
}

fn inline_pending_result_id(platform: MusicPlatform, id: &str, quality: &str) -> Option<String> {
    if !is_inline_result_token(id) || !is_inline_result_token(quality) {
        return None;
    }
    let result_id = format!("p|{}|{id}|{quality}", platform.callback_code());
    (result_id.len() <= CALLBACK_LIMIT).then_some(result_id)
}

fn parse_inline_pending_result_id(value: &str) -> Option<(MusicPlatform, String, String)> {
    let mut parts = value.split('|');
    if parts.next()? != "p" {
        return None;
    }
    let platform = parts.next().and_then(MusicPlatform::from_callback_code)?;
    let id = parts.next()?.to_string();
    let quality = parts.next()?.to_string();
    if parts.next().is_some() || !is_inline_result_token(&id) || !is_inline_result_token(&quality) {
        return None;
    }
    Some((platform, id, quality))
}

fn is_inline_result_token(value: &str) -> bool {
    !value.trim().is_empty()
        && value
            .bytes()
            .all(|byte| !byte.is_ascii_whitespace() && byte != b'|')
}

async fn get_music_gui(bot: Bot, msg: Message, query: MusicQuery) -> Result<(), BotError> {
    if let Some(collection) = search_collection(&query, 10).await? {
        let text = if collection.items.is_empty() {
            format!("{} 没有可发送的曲目", collection.title)
        } else {
            format!(
                "选择你的音乐（{} · {}）",
                collection.platform.label(),
                collection.title
            )
        };
        let mut request = bot.send_message(msg.chat.id, text);
        if !collection.items.is_empty() {
            request = request.reply_markup(search_menu(collection.items, None));
        }
        request.await?;
        return Ok(());
    }
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
    if let Some(collection) = search_collection(&query, 10).await? {
        let text = if collection.items.is_empty() {
            format!("{} 没有可发送的曲目", collection.title)
        } else {
            format!(
                "选择你的音乐（{} · {}）",
                collection.platform.label(),
                collection.title
            )
        };
        let mut request = bot
            .send_message(msg.chat.id, text)
            .reply_parameters(ReplyParameters::new(msg.id));
        if !collection.items.is_empty() {
            request = request.reply_markup(search_menu(collection.items, None));
        }
        request.await?;
        return Ok(());
    }
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

async fn send_music_collection(
    bot: &Bot,
    msg: &Message,
    query: MusicQuery,
) -> Result<(), BotError> {
    let Some(collection) = search_collection(&query, 10).await? else {
        bot.send_message(msg.chat.id, "没有识别到支持的歌单或专辑")
            .reply_parameters(ReplyParameters::new(msg.id))
            .await?;
        return Ok(());
    };
    let text = if collection.items.is_empty() {
        format!("{} 没有可发送的曲目", collection.title)
    } else {
        format!(
            "选择你的音乐（{} · {}）",
            collection.platform.label(),
            collection.title
        )
    };
    let mut request = bot
        .send_message(msg.chat.id, text)
        .reply_parameters(ReplyParameters::new(msg.id));
    if !collection.items.is_empty() {
        request = request.reply_markup(search_menu(collection.items, None));
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

async fn handle_lyrics_callback(
    bot: &Bot,
    callback_id: &str,
    msg: &Message,
    platform: MusicPlatform,
    id: &str,
    script: MusicLyricScript,
) -> BotResult {
    let Some(lyrics) = provider_for(platform).lyrics(id).await? else {
        bot.answer_callback_query(callback_id.to_string())
            .text(format!("{} 没有可用歌词", platform.label()))
            .show_alert(true)
            .await?;
        return Ok(());
    };
    let document = render_lyric_document(
        msg,
        platform,
        id,
        &lyrics.plain,
        &lyrics.translation,
        script,
    );
    if message_has_document(msg) {
        let media = InputMediaDocument::new(
            InputFile::memory(document.content.into_bytes()).file_name(document.file_name),
        )
        .caption(document.caption)
        .parse_mode(ParseMode::Html);
        bot.edit_message_media(msg.chat.id, msg.id, InputMedia::Document(media))
            .reply_markup(lyric_switch_menu(platform, id, script))
            .await?;
        bot.answer_callback_query(callback_id.to_string())
            .text(format!("已切换{}", script.label()))
            .await?;
        return Ok(());
    }
    let mut request = bot
        .send_document(
            msg.chat.id,
            InputFile::memory(document.content.into_bytes()).file_name(document.file_name),
        )
        .reply_parameters(ReplyParameters::new(msg.id))
        .reply_markup(lyric_switch_menu(platform, id, script));
    if !document.caption.is_empty() {
        request = request
            .caption(document.caption)
            .parse_mode(ParseMode::Html);
    }
    request.await?;
    bot.answer_callback_query(callback_id.to_string())
        .text("已发送歌词")
        .await?;
    Ok(())
}

async fn handle_inline_lyrics_callback(
    bot: &Bot,
    callback_id: &str,
    inline_message_id: &str,
    platform: MusicPlatform,
    id: &str,
    script: MusicLyricScript,
    settings: &UserMusicSettings,
) -> BotResult {
    let Some(lyrics) = provider_for(platform).lyrics(id).await? else {
        bot.answer_callback_query(callback_id.to_string())
            .text(format!("{} 没有可用歌词", platform.label()))
            .show_alert(true)
            .await?;
        return Ok(());
    };
    let cached = find_music_cache(platform.id(), id, &music_cache_quality(settings))
        .await
        .ok()
        .flatten();
    let track = cached.as_ref().and_then(cache_to_track);
    let caption = if let Some(track) = track.as_ref() {
        build_inline_track_caption_with_lyrics(
            track,
            cached
                .as_ref()
                .map(|cache| cache.music_size)
                .unwrap_or_default(),
            &lyrics,
            script,
        )
    } else {
        inline_lyric_text("歌词", platform.label(), &lyrics, script)
    };
    let mut request = bot
        .edit_message_caption_inline(inline_message_id)
        .caption(caption)
        .parse_mode(ParseMode::Html);
    let markup = track
        .as_ref()
        .and_then(|track| inline_track_lyric_menu(track, script))
        .unwrap_or_else(|| inline_lyric_switch_menu(platform, id, script));
    request = request.reply_markup(markup);
    request.await?;
    bot.answer_callback_query(callback_id.to_string())
        .text(format!("已切换{}", script.label()))
        .await?;
    Ok(())
}

async fn handle_inline_lyric_text_callback(
    bot: &Bot,
    callback_id: &str,
    inline_message_id: &str,
    platform: MusicPlatform,
    id: &str,
    script: MusicLyricScript,
) -> BotResult {
    let Some(lyrics) = provider_for(platform).lyrics(id).await? else {
        bot.answer_callback_query(callback_id.to_string())
            .text(format!("{} 没有可用歌词", platform.label()))
            .show_alert(true)
            .await?;
        return Ok(());
    };
    bot.edit_message_text_inline(
        inline_message_id,
        inline_lyric_text("歌词", platform.label(), &lyrics, script),
    )
    .parse_mode(ParseMode::Html)
    .reply_markup(inline_lyric_switch_menu(platform, id, script))
    .await?;
    bot.answer_callback_query(callback_id.to_string())
        .text(format!("已切换{}", script.label()))
        .await?;
    Ok(())
}

struct RenderedLyricDocument {
    file_name: String,
    content: String,
    caption: String,
}

fn render_lyric_document(
    msg: &Message,
    platform: MusicPlatform,
    id: &str,
    plain: &str,
    translation: &str,
    script: MusicLyricScript,
) -> RenderedLyricDocument {
    let content = convert_lyric_script(&build_lrc_content(plain, translation), script);
    let file_name = build_lyric_file_name(msg, platform, id);
    let caption = build_lyric_caption(&content, "lrc", script);
    RenderedLyricDocument {
        file_name,
        content,
        caption,
    }
}

fn build_lrc_content(plain: &str, translation: &str) -> String {
    let mut content = plain.trim().to_string();
    if !translation.trim().is_empty() {
        if !content.is_empty() {
            content.push_str("\n\n");
        }
        content.push_str(translation.trim());
    }
    if content.is_empty() {
        "暂无歌词信息\n".to_string()
    } else {
        content
    }
}

fn convert_lyric_script(content: &str, script: MusicLyricScript) -> String {
    match script {
        MusicLyricScript::Simplified => LYRIC_TO_SIMPLIFIED.convert(content),
        MusicLyricScript::Traditional => LYRIC_TO_TRADITIONAL.convert(content),
    }
}

fn build_lyric_caption(content: &str, format: &str, script: MusicLyricScript) -> String {
    let header = format!(
        "当前歌词格式: {}\n当前歌词文字: {}",
        lyric_format_display_name(format),
        script.label()
    );
    let preview = lyric_preview_text(content);
    if preview.is_empty() {
        return header;
    }
    let escaped = html_escape(&preview);
    let candidate = format!("{header}\n<blockquote expandable>{escaped}</blockquote>");
    if candidate.chars().count() <= 1000 {
        return candidate;
    }
    let mut escaped = escaped.chars().take(400).collect::<String>();
    escaped.push('…');
    let candidate = format!("{header}\n<blockquote expandable>{escaped}</blockquote>");
    if candidate.chars().count() <= 1000 {
        candidate
    } else {
        header
    }
}

fn lyric_switch_menu(
    platform: MusicPlatform,
    id: &str,
    script: MusicLyricScript,
) -> InlineKeyboardMarkup {
    let target = script.toggled();
    let keyboard = MusicCallbackAction::Lyrics(platform, id.to_string(), Some(target))
        .encode()
        .map(|callback| {
            vec![vec![InlineKeyboardButton::callback(
                script.switch_label(),
                callback,
            )]]
        })
        .unwrap_or_default();
    InlineKeyboardMarkup::new(keyboard)
}

fn inline_lyric_switch_menu(
    platform: MusicPlatform,
    id: &str,
    script: MusicLyricScript,
) -> InlineKeyboardMarkup {
    let target = script.toggled();
    let keyboard = MusicCallbackAction::InlineLyrics(platform, id.to_string(), target)
        .encode()
        .map(|callback| {
            vec![vec![InlineKeyboardButton::callback(
                script.switch_label(),
                callback,
            )]]
        })
        .unwrap_or_default();
    InlineKeyboardMarkup::new(keyboard)
}

fn lyric_preview_text(content: &str) -> String {
    let lines = content
        .lines()
        .map(strip_lrc_timestamps)
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    lines.join("\n")
}

fn strip_lrc_timestamps(line: &str) -> &str {
    let mut rest = line.trim_start();
    while let Some(stripped) = rest.strip_prefix('[') {
        let Some((_, after)) = stripped.split_once(']') else {
            break;
        };
        rest = after.trim_start();
    }
    rest
}

fn lyric_format_display_name(format: &str) -> &'static str {
    match format {
        "lrc" => "LRC",
        "txt" => "纯文本",
        "srt" => "SRT 字幕",
        "ttml" => "TTML 逐词",
        _ => "LRC",
    }
}

fn build_lyric_file_name(msg: &Message, platform: MusicPlatform, id: &str) -> String {
    let base = audio_lyric_base_name(msg).unwrap_or_else(|| {
        let id = id.trim();
        if id.is_empty() {
            "歌词".to_string()
        } else {
            format!("{}-{id}", platform.label())
        }
    });
    sanitize_file_name(&format!("{base}.lrc"))
}

fn audio_lyric_base_name(msg: &Message) -> Option<String> {
    let audio = match &msg.kind {
        MessageKind::Common(common) => match &common.media_kind {
            MediaKind::Audio(audio) => &audio.audio,
            _ => return None,
        },
        _ => return None,
    };
    let title = audio.title.as_deref().map(str::trim).unwrap_or_default();
    let performer = audio
        .performer
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    match (performer.is_empty(), title.is_empty()) {
        (true, true) => audio
            .file_name
            .as_deref()
            .map(trim_file_extension)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToString::to_string),
        (true, false) => Some(title.to_string()),
        (false, true) => Some(performer.to_string()),
        (false, false) => Some(format!("{} - {}", performer.replace('/', ","), title)),
    }
}

fn trim_file_extension(value: &str) -> &str {
    value
        .rsplit_once('.')
        .map(|(base, _)| base)
        .unwrap_or(value)
}

fn sanitize_file_name(name: &str) -> String {
    let cleaned = name
        .chars()
        .map(|c| match c {
            '/' | '?' | '*' | ':' | '|' | '\\' | '<' | '>' | '"' => ' ',
            _ => c,
        })
        .collect::<String>();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return "file.lrc".to_string();
    }
    truncate_utf8_bytes(cleaned, 180)
}

fn truncate_utf8_bytes(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].trim().to_string()
}

async fn handle_favorite_callback(
    bot: &Bot,
    callback_id: &str,
    msg: &Message,
    from: &teloxide::types::User,
    action: MusicFavoriteAction,
    settings: &UserMusicSettings,
) -> BotResult {
    match action {
        MusicFavoriteAction::Toggle {
            scope,
            platform,
            id,
            chat_id,
        } => {
            let scope_id = match scope {
                FavoriteScope::User => from.id.0 as i64,
                FavoriteScope::Group => chat_id.unwrap_or(msg.chat.id.0),
            };
            if scope == FavoriteScope::Group && !is_group_chat(msg) {
                bot.answer_callback_query(callback_id.to_string())
                    .text("群收藏只能在群聊消息中使用")
                    .show_alert(true)
                    .await?;
                return Ok(());
            }
            if is_favorited(scope.storage_key(), scope_id, platform.id(), &id).await? {
                bot.answer_callback_query(callback_id.to_string())
                    .text("已收藏，可在收藏列表中取消")
                    .await?;
                return Ok(());
            }
            let query = MusicQuery {
                platform,
                keyword: id.clone(),
            };
            let track = resolve_track(&query, Some(&id), settings).await?;
            upsert_favorite(favorite_from_track(scope, scope_id, from, &track)).await?;
            bot.answer_callback_query(callback_id.to_string())
                .text(format!("已加入{}", scope.label()))
                .await?;
        }
        MusicFavoriteAction::AskRemove {
            scope,
            platform,
            id,
            chat_id,
        } => {
            let scope_id = match scope {
                FavoriteScope::User => from.id.0 as i64,
                FavoriteScope::Group => chat_id.unwrap_or(msg.chat.id.0),
            };
            bot.answer_callback_query(callback_id.to_string()).await?;
            bot.edit_message_reply_markup(msg.chat.id, msg.id)
                .reply_markup(favorite_confirm_menu(scope, scope_id, platform, &id))
                .await?;
        }
        MusicFavoriteAction::Remove {
            scope,
            platform,
            id,
            chat_id,
        } => {
            let scope_id = match scope {
                FavoriteScope::User => from.id.0 as i64,
                FavoriteScope::Group => chat_id.unwrap_or(msg.chat.id.0),
            };
            let removed =
                remove_favorite(scope.storage_key(), scope_id, platform.id(), &id).await?;
            bot.answer_callback_query(callback_id.to_string())
                .text(if removed {
                    "已取消收藏"
                } else {
                    "收藏不存在"
                })
                .await?;
            edit_music_favorites(bot, msg, scope, scope_id).await?;
        }
        MusicFavoriteAction::List { scope, chat_id } => {
            let scope_id = match scope {
                FavoriteScope::User => from.id.0 as i64,
                FavoriteScope::Group => chat_id.unwrap_or(msg.chat.id.0),
            };
            bot.answer_callback_query(callback_id.to_string()).await?;
            edit_music_favorites(bot, msg, scope, scope_id).await?;
        }
        MusicFavoriteAction::Close => {
            bot.answer_callback_query(callback_id.to_string()).await?;
            let _ = bot.delete_message(msg.chat.id, msg.id).await;
        }
    }
    Ok(())
}

async fn handle_inline_favorite_callback(
    bot: &Bot,
    callback_id: &str,
    from: &teloxide::types::User,
    action: MusicFavoriteAction,
    settings: &UserMusicSettings,
) -> BotResult {
    let MusicFavoriteAction::Toggle {
        scope: FavoriteScope::User,
        platform,
        id,
        ..
    } = action
    else {
        bot.answer_callback_query(callback_id.to_string())
            .text("inline 模式只支持个人收藏")
            .show_alert(true)
            .await?;
        return Ok(());
    };

    let scope_id = from.id.0 as i64;
    if is_favorited(FAVORITE_SCOPE_USER, scope_id, platform.id(), &id).await? {
        remove_favorite(FAVORITE_SCOPE_USER, scope_id, platform.id(), &id).await?;
        bot.answer_callback_query(callback_id.to_string())
            .text("已取消收藏")
            .await?;
        return Ok(());
    }

    let query = MusicQuery {
        platform,
        keyword: id.clone(),
    };
    let track = resolve_track(&query, Some(&id), settings).await?;
    upsert_favorite(favorite_from_track(
        FavoriteScope::User,
        scope_id,
        from,
        &track,
    ))
    .await?;
    bot.answer_callback_query(callback_id.to_string())
        .text("已加入个人收藏")
        .await?;
    Ok(())
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
    let search_query = search_keyword
        .map(str::trim)
        .filter(|keyword| !keyword.is_empty())
        .map(|keyword| MusicQuery {
            platform,
            keyword: keyword.to_string(),
        });
    let query_for_menu = search_query.as_ref().unwrap_or(&query);
    let status_msg = message_has_text(&msg).then_some(&msg);
    if try_send_cached_track(
        &bot,
        &msg,
        status_msg,
        callback_reply_target(&msg),
        query_for_menu,
        &track,
        settings,
    )
    .await?
    {
        delete_callback_status(
            &bot,
            &msg,
            format!("已从缓存发送：{} - {}", track.song, track.singer),
        )
        .await?;
        return Ok(());
    }
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
    let sent = send_track(
        &bot,
        &msg,
        status_msg,
        callback_reply_target(&msg),
        query_for_menu,
        track.clone(),
        media.clone(),
    )
    .await?;
    if let Err(err) = save_track_cache(&track, &media, &sent, &msg, settings).await {
        eprintln!("Failed to save music cache: {err}");
    }
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

fn message_has_document(msg: &Message) -> bool {
    matches!(
        &msg.kind,
        MessageKind::Common(common) if matches!(common.media_kind, MediaKind::Document(_))
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
        let from = q.from.clone();
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
        let action = match action {
            MusicCallbackAction::InlineSend {
                platform,
                id,
                quality,
                requester_id,
            } => {
                let Some(inline_message_id) = q.inline_message_id else {
                    return Ok(());
                };
                return handle_inline_send_callback(
                    &bot,
                    &callback_id,
                    &inline_message_id,
                    &from,
                    platform,
                    &id,
                    &quality,
                    requester_id,
                )
                .await;
            }
            MusicCallbackAction::InlineLyrics(platform, id, script)
                if q.inline_message_id.is_some() =>
            {
                let Some(inline_message_id) = q.inline_message_id else {
                    return Ok(());
                };
                return handle_inline_lyric_text_callback(
                    &bot,
                    &callback_id,
                    &inline_message_id,
                    platform,
                    &id,
                    script,
                )
                .await;
            }
            MusicCallbackAction::Lyrics(platform, id, script) if q.inline_message_id.is_some() => {
                let Some(inline_message_id) = q.inline_message_id else {
                    return Ok(());
                };
                let settings = get_user_settings_or_default(user_id).await;
                let script = script
                    .unwrap_or_else(|| MusicLyricScript::from_stored_value(&settings.lyric_script));
                return handle_inline_lyrics_callback(
                    &bot,
                    &callback_id,
                    &inline_message_id,
                    platform,
                    &id,
                    script,
                    &settings,
                )
                .await;
            }
            MusicCallbackAction::Setting(action) if q.inline_message_id.is_some() => {
                let Some(inline_message_id) = q.inline_message_id else {
                    return Ok(());
                };
                return handle_inline_setting_callback(
                    &bot,
                    &callback_id,
                    &inline_message_id,
                    user_id,
                    action,
                )
                .await;
            }
            MusicCallbackAction::Close if q.inline_message_id.is_some() => {
                let Some(inline_message_id) = q.inline_message_id else {
                    return Ok(());
                };
                bot.edit_message_text_inline(inline_message_id, "已关闭")
                    .reply_markup(empty_keyboard())
                    .await?;
                bot.answer_callback_query(callback_id.clone()).await?;
                return Ok(());
            }
            MusicCallbackAction::Favorite(action)
                if q.inline_message_id.is_some()
                    && matches!(
                        action,
                        MusicFavoriteAction::Toggle {
                            scope: FavoriteScope::User,
                            ..
                        }
                    ) =>
            {
                let settings = get_user_settings_or_default(user_id).await;
                return handle_inline_favorite_callback(
                    &bot,
                    &callback_id,
                    &from,
                    action,
                    &settings,
                )
                .await;
            }
            action => action,
        };
        let msg = match q.message {
            None => return Ok(()),
            Some(mbi_msg) => match mbi_msg {
                MaybeInaccessibleMessage::Inaccessible(_) => return Ok(()),
                MaybeInaccessibleMessage::Regular(msg) => msg,
            },
        };
        let lock_flag = hashing((msg.chat.id, msg.id));
        if LIMITER.is_running(lock_flag) {
            bot.answer_callback_query(callback_id)
                .text("正在处理上一次请求...")
                .await?;
            return Ok(());
        }
        let _guard = Guard::new(&LIMITER, lock_flag);
        if !matches!(
            action,
            MusicCallbackAction::Setting(_)
                | MusicCallbackAction::Favorite(_)
                | MusicCallbackAction::Lyrics(_, _, _)
                | MusicCallbackAction::InlineLyrics(_, _, _)
                | MusicCallbackAction::Close
        ) {
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
            MusicCallbackAction::Lyrics(platform, id, script) => {
                let settings = get_user_settings_or_default(user_id).await;
                let script = script
                    .unwrap_or_else(|| MusicLyricScript::from_stored_value(&settings.lyric_script));
                handle_lyrics_callback(&bot, &callback_id, &msg, platform, &id, script).await
            }
            MusicCallbackAction::InlineLyrics(platform, id, script) => {
                handle_lyrics_callback(&bot, &callback_id, &msg, platform, &id, script).await
            }
            MusicCallbackAction::Favorite(action) => {
                let settings = get_user_settings_or_default(user_id).await;
                handle_favorite_callback(&bot, &callback_id, &msg, &from, action, &settings).await
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
            MusicCallbackAction::Close => {
                let _ = bot.delete_message(msg.chat.id, msg.id).await;
                bot.answer_callback_query(callback_id.clone()).await?;
                Ok(())
            }
            MusicCallbackAction::InlineSend { .. } => Ok(()),
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
    msg: &Message,
) -> Option<InlineKeyboardMarkup> {
    track_menu_for_context(track, fallback_query, is_group_chat(msg), msg.chat.id.0)
}

fn inline_track_menu(track: &MusicTrack) -> Option<InlineKeyboardMarkup> {
    inline_track_menu_with_lyric_label(track, "歌词", None)
}

fn inline_track_lyric_menu(
    track: &MusicTrack,
    script: MusicLyricScript,
) -> Option<InlineKeyboardMarkup> {
    inline_track_menu_with_lyric_label(track, script.switch_label(), Some(script.toggled()))
}

fn inline_track_menu_with_lyric_label(
    track: &MusicTrack,
    lyric_label: &str,
    lyric_target: Option<MusicLyricScript>,
) -> Option<InlineKeyboardMarkup> {
    let mut utility_row = Vec::new();
    if let Ok(url) = Url::parse(track.cover.trim())
        && !track.cover.trim().is_empty()
    {
        utility_row.push(InlineKeyboardButton::url("获取封面", url));
    }
    if !track.song.trim().is_empty() {
        utility_row.push(InlineKeyboardButton::switch_inline_query_current_chat(
            "搜索更多",
            format!("{} {}", track.song, track.platform.id()),
        ));
    }

    let mut action_row = Vec::new();
    if !track.id.trim().is_empty()
        && let Some(callback) =
            MusicCallbackAction::Lyrics(track.platform, track.id.clone(), lyric_target).encode()
    {
        action_row.push(InlineKeyboardButton::callback(lyric_label, callback));
    }
    if !track.id.trim().is_empty()
        && let Some(callback) = MusicCallbackAction::Favorite(MusicFavoriteAction::Toggle {
            scope: FavoriteScope::User,
            platform: track.platform,
            id: track.id.clone(),
            chat_id: None,
        })
        .encode()
    {
        action_row.push(InlineKeyboardButton::callback("收藏", callback));
    }

    let rows = [utility_row, action_row]
        .into_iter()
        .filter(|row| !row.is_empty())
        .collect::<Vec<_>>();
    (!rows.is_empty()).then(|| InlineKeyboardMarkup::new(rows))
}

fn track_menu_for_context(
    track: &MusicTrack,
    fallback_query: Option<&MusicQuery>,
    is_group: bool,
    chat_id: i64,
) -> Option<InlineKeyboardMarkup> {
    let mut utility_row = Vec::new();
    if !track.cover.trim().is_empty()
        && !track.id.trim().is_empty()
        && let Some(callback) =
            MusicCallbackAction::Cover(track.platform, track.id.clone()).encode()
    {
        utility_row.push(InlineKeyboardButton::callback("获取封面", callback));
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
        utility_row.push(InlineKeyboardButton::callback("搜索更多", callback));
    }

    let mut action_row = Vec::new();
    if !track.id.trim().is_empty() {
        if let Some(callback) =
            MusicCallbackAction::Lyrics(track.platform, track.id.clone(), None).encode()
        {
            action_row.push(InlineKeyboardButton::callback("歌词", callback));
        }
        if let Some(callback) = MusicCallbackAction::Favorite(MusicFavoriteAction::Toggle {
            scope: FavoriteScope::User,
            platform: track.platform,
            id: track.id.clone(),
            chat_id: None,
        })
        .encode()
        {
            action_row.push(InlineKeyboardButton::callback("收藏", callback));
        }
        if is_group
            && let Some(callback) = MusicCallbackAction::Favorite(MusicFavoriteAction::Toggle {
                scope: FavoriteScope::Group,
                platform: track.platform,
                id: track.id.clone(),
                chat_id: Some(chat_id),
            })
            .encode()
        {
            action_row.push(InlineKeyboardButton::callback("群收藏", callback));
        }
    }

    let rows = [utility_row, action_row]
        .into_iter()
        .filter(|row| !row.is_empty())
        .collect::<Vec<_>>();
    (!rows.is_empty()).then(|| InlineKeyboardMarkup::new(rows))
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
        MusicAction::Favorite(scope) => {
            return send_music_favorites(bot, msg, user_id_from_message(msg), scope).await;
        }
        MusicAction::Collection(query) => return send_music_collection(bot, msg, query).await,
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
    Favorite(FavoriteScope),
    Collection(MusicQuery),
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
            Some(MusicSubcommand::Favorite(favorite)) => Ok(Self::Favorite(favorite.scope.into())),
            Some(MusicSubcommand::Collection(collection)) => {
                let platform =
                    parse_optional_platform(collection.platform.as_deref(), default_platform)?;
                Ok(Self::Collection(provider::build_query_with_platform(
                    platform,
                    &collection.query,
                )?))
            }
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
            quality: "lossless".to_string(),
            send_cover: true,
            lyric_script: "simplified".to_string(),
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
    fn parses_favorite_subcommand() {
        let cmd = MusicCmd::parse_i18n_from_bot(["/music", "favorite"], Some("zh-CN")).unwrap();
        let MusicAction::Favorite(scope) =
            MusicAction::from_cmd(cmd, &test_settings("tencent")).unwrap()
        else {
            panic!("expected favorite action");
        };
        assert_eq!(scope, FavoriteScope::User);

        let cmd = MusicCmd::parse_i18n_from_bot(["/music", "fav", "group"], Some("zh-CN")).unwrap();
        let MusicAction::Favorite(scope) =
            MusicAction::from_cmd(cmd, &test_settings("tencent")).unwrap()
        else {
            panic!("expected favorite action");
        };
        assert_eq!(scope, FavoriteScope::Group);
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
    fn favorite_callbacks_are_self_describing() {
        let payload = MusicCallbackAction::Favorite(MusicFavoriteAction::Toggle {
            scope: FavoriteScope::User,
            platform: MusicPlatform::AppleMusic,
            id: "1624001324".to_string(),
            chat_id: None,
        })
        .encode()
        .unwrap();
        assert_eq!(payload, "music fav t u a 1624001324");
        let MusicCallbackAction::Favorite(MusicFavoriteAction::Toggle {
            scope,
            platform,
            id,
            chat_id,
        }) = MusicCallbackAction::decode(&payload).unwrap()
        else {
            panic!("expected favorite toggle");
        };
        assert_eq!(scope, FavoriteScope::User);
        assert_eq!(platform, MusicPlatform::AppleMusic);
        assert_eq!(id, "1624001324");
        assert_eq!(chat_id, None);

        let payload = MusicCallbackAction::Favorite(MusicFavoriteAction::Toggle {
            scope: FavoriteScope::Group,
            platform: MusicPlatform::Soda,
            id: "739105056071".to_string(),
            chat_id: Some(-100123),
        })
        .encode()
        .unwrap();
        assert_eq!(payload, "music fav t g -100123 s 739105056071");
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
    fn lyric_callback_can_describe_target_script() {
        let payload = MusicCallbackAction::Lyrics(
            MusicPlatform::AppleMusic,
            "1624001324".to_string(),
            Some(MusicLyricScript::Traditional),
        )
        .encode()
        .unwrap();
        assert_eq!(payload, "music lyric a 1624001324 t");
        let MusicCallbackAction::Lyrics(platform, id, script) =
            MusicCallbackAction::decode(&payload).unwrap()
        else {
            panic!("expected lyric action");
        };
        assert_eq!(platform, MusicPlatform::AppleMusic);
        assert_eq!(id, "1624001324");
        assert_eq!(script, Some(MusicLyricScript::Traditional));
    }

    #[test]
    fn inline_lyric_result_has_script_switch_button() {
        let item = MusicSearchItem {
            platform: MusicPlatform::AppleMusic,
            id: "1624001324".to_string(),
            song: "稻香".to_string(),
            singer: "周杰伦".to_string(),
            cover: String::new(),
        };
        let lyrics = provider::MusicLyrics {
            plain: "开放中文转换".to_string(),
            translation: String::new(),
        };
        let result =
            inline_lyric_result(&item, &lyrics, MusicLyricScript::Simplified, None).unwrap();
        let InlineQueryResult::Article(article) = result else {
            panic!("expected article result");
        };
        let keyboard = article.reply_markup.unwrap().inline_keyboard;
        assert_eq!(keyboard[0][0].text, "切换繁体");
        let CallbackData(callback) = &keyboard[0][0].kind else {
            panic!("expected callback button");
        };
        assert_eq!(callback, "music ilyric a 1624001324 t");
        let InputMessageContent::Text(content) = article.input_message_content else {
            panic!("expected text content");
        };
        assert_eq!(content.parse_mode, Some(ParseMode::Html));
        assert!(content.message_text.contains("<blockquote expandable>"));
    }

    #[test]
    fn inline_lyric_text_converts_script() {
        let lyrics = provider::MusicLyrics {
            plain: "开放中文转换".to_string(),
            translation: String::new(),
        };
        assert_eq!(
            inline_lyric_text("歌", "人", &lyrics, MusicLyricScript::Traditional),
            "<b>歌 - 人</b>\n当前歌词文字: 繁体\n<blockquote expandable>開放中文轉換</blockquote>"
        );
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
        let markup = track_menu_for_context(&track, Some(&query), false, 0).unwrap();
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
    fn track_menu_renders_music_bottom_buttons() {
        let track = MusicTrack {
            id: "1624001324".to_string(),
            platform: MusicPlatform::AppleMusic,
            song: "稻香".to_string(),
            singer: "周杰伦".to_string(),
            album: "魔杰座".to_string(),
            cover: "https://example.com/cover.jpg".to_string(),
            link: "https://music.apple.com/song/1624001324".to_string(),
            url: "applemusic-wrapper://127.0.0.1/1624001324".to_string(),
            headers: Default::default(),
            duration: Some(223),
            bitrate: None,
            format: Some("m4a".to_string()),
        };
        let markup = track_menu_for_context(&track, None, true, -100123).unwrap();
        assert_eq!(markup.inline_keyboard.len(), 2);
        assert_eq!(markup.inline_keyboard[0][0].text, "获取封面");
        assert_eq!(markup.inline_keyboard[0][1].text, "搜索更多");
        let action_labels = markup.inline_keyboard[1]
            .iter()
            .map(|button| button.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(action_labels, vec!["歌词", "收藏", "群收藏"]);
    }

    #[test]
    fn lyric_document_caption_matches_upstream_shape() {
        let content = "[00:01.20]稻香\n[00:03.45]对这个世界如果你有太多的抱怨";
        assert_eq!(
            lyric_preview_text(content),
            "稻香\n对这个世界如果你有太多的抱怨"
        );
        assert_eq!(
            build_lyric_caption(content, "lrc", MusicLyricScript::Simplified),
            "当前歌词格式: LRC\n当前歌词文字: 简体\n<blockquote expandable>稻香\n对这个世界如果你有太多的抱怨</blockquote>"
        );
    }

    #[test]
    fn converts_lyrics_between_simplified_and_traditional() {
        assert_eq!(
            convert_lyric_script("开放中文转换", MusicLyricScript::Traditional),
            "開放中文轉換"
        );
        assert_eq!(
            convert_lyric_script("開放中文轉換", MusicLyricScript::Simplified),
            "开放中文转换"
        );
    }

    #[test]
    fn lyric_file_name_is_sanitized() {
        assert_eq!(
            sanitize_file_name("周杰伦/稻香:Live?.lrc"),
            "周杰伦 稻香 Live .lrc"
        );
        assert_eq!(sanitize_file_name("   "), "file.lrc");
    }

    #[test]
    fn inline_callback_describes_send_action() {
        let callback = MusicCallbackAction::InlineSend {
            platform: MusicPlatform::Soda,
            id: "449205".to_string(),
            quality: "high".to_string(),
            requester_id: 42,
        }
        .encode()
        .unwrap();

        assert_eq!(callback, "music i s 449205 high 42");
        let MusicCallbackAction::InlineSend {
            platform,
            id,
            quality,
            requester_id,
        } = MusicCallbackAction::decode("i s 449205 high 42").unwrap()
        else {
            panic!("expected inline send action");
        };
        assert_eq!(platform, MusicPlatform::Soda);
        assert_eq!(id, "449205");
        assert_eq!(quality, "high");
        assert_eq!(requester_id, 42);
    }

    #[test]
    fn inline_pending_result_id_describes_chosen_action() {
        let result_id = inline_pending_result_id(MusicPlatform::AppleMusic, "1624001324", "high")
            .expect("short inline result id");
        assert_eq!(result_id, "p|a|1624001324|high");
        assert_eq!(
            parse_inline_pending_result_id(&result_id),
            Some((
                MusicPlatform::AppleMusic,
                "1624001324".to_string(),
                "high".to_string()
            ))
        );
        assert!(inline_pending_result_id(MusicPlatform::Soda, "bad|id", "high").is_none());
    }

    #[test]
    fn inline_pending_result_has_cover_and_refresh_fallback() {
        let item = MusicSearchItem {
            platform: MusicPlatform::AppleMusic,
            id: "1624001324".to_string(),
            song: "稻香".to_string(),
            singer: "周杰伦".to_string(),
            cover: String::new(),
        };
        let result =
            inline_pending_audio_result(&item, "high", Some("https://example.com/cover.jpg"), 42)
                .unwrap();
        let InlineQueryResult::Article(article) = result else {
            panic!("expected article result");
        };
        assert_eq!(article.title, "稻香 - 周杰伦");
        assert_eq!(
            article.thumbnail_url.unwrap().as_str(),
            "https://example.com/cover.jpg"
        );
        let keyboard = article.reply_markup.unwrap().inline_keyboard;
        assert_eq!(keyboard[0][0].text, "没反应？点此刷新");
    }

    #[test]
    fn inline_query_suffix_can_override_platform_and_quality() {
        let settings = UserMusicSettings::defaults_for(42);
        let (keyword, platform, quality) = parse_inline_music_query("稻香 qq lossless", &settings);
        assert_eq!(keyword, "稻香");
        assert_eq!(platform, MusicPlatform::Tencent);
        assert_eq!(quality, "lossless");

        let (keyword, platform, quality) = parse_inline_music_query("晴天", &settings);
        assert_eq!(keyword, "晴天");
        assert_eq!(platform, MusicPlatform::Soda);
        assert_eq!(quality, "lossless");
    }

    #[test]
    fn inline_lyric_query_prefix_is_detected() {
        assert_eq!(parse_inline_lyric_query("歌词 稻香 qq"), Some("稻香 qq"));
        assert_eq!(parse_inline_lyric_query("lyric 稻香"), Some("稻香"));
        assert_eq!(parse_inline_lyric_query("稻香"), None);
    }

    #[test]
    fn inline_lyric_text_is_truncated_to_telegram_limit() {
        let long = "好".repeat(5000);
        let truncated = truncate_inline_text(&long);
        assert_eq!(truncated.chars().count(), 4096);
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn inline_track_menu_has_usable_music_buttons() {
        let track = MusicTrack {
            id: "1624001324".to_string(),
            platform: MusicPlatform::AppleMusic,
            song: "稻香".to_string(),
            singer: "周杰伦".to_string(),
            album: "魔杰座".to_string(),
            cover: "https://example.com/cover.jpg".to_string(),
            link: "https://music.apple.com/song/1624001324".to_string(),
            url: "applemusic-wrapper://127.0.0.1/1624001324".to_string(),
            headers: Default::default(),
            duration: Some(223),
            bitrate: None,
            format: Some("m4a".to_string()),
        };
        let markup = inline_track_menu(&track).unwrap();
        assert_eq!(markup.inline_keyboard.len(), 2);
        assert_eq!(markup.inline_keyboard[0][0].text, "获取封面");
        assert_eq!(markup.inline_keyboard[0][1].text, "搜索更多");
        assert_eq!(markup.inline_keyboard[1][0].text, "歌词");
        let CallbackData(callback) = &markup.inline_keyboard[1][0].kind else {
            panic!("expected lyric callback");
        };
        assert_eq!(callback, "music lyric a 1624001324");
        assert_eq!(markup.inline_keyboard[1][1].text, "收藏");
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
