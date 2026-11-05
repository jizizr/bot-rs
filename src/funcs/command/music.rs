use super::*;
use crate::{
    dao::mongo::{
        music_favorites::{
            FAVORITE_SCOPE_GROUP, FAVORITE_SCOPE_USER, MusicFavorite, is_favorited, list_favorites,
            remove_favorite, upsert_favorite,
        },
        music_settings::{
            UserMusicSettings, get_user_settings, get_user_settings_or_default, save_user_settings,
        },
    },
    funcs::pkg::rich::{self, RichPage},
};
use ferrous_opencc::{OpenCC, config::BuiltinConfig};
use futures::future::join_all;
use lyrics_view::{Cover, LyricSong, LyricView, SongAudio, lyric_cover, song_card};
use provider::{
    DownloadProgress, JobState, MediaRequest, MusicCollection, MusicMedia, MusicPlatform,
    MusicQuery, MusicSearchItem, MusicTrack,
};
use ratelimit::ResourceAction;
use std::{
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};
use telers::types::{
    InlineQueryResult, InlineQueryResultArticle, InputMedia, InputMediaAudio, InputMediaDocument,
    InputRichMessageContent, InputTextMessageContent,
};
use tokio::{sync::mpsc, task::JoinHandle};
use url::Url;

mod lyrics_view;
mod provider;
mod queue;
mod ratelimit;

const CALLBACK_PREFIX: &str = "music";
const CALLBACK_LIMIT: usize = 64;
const MUSIC_PANEL_OWNER_NOTICE: &str = "这是其他用户的音乐面板，请发送 /music 打开自己的面板";
const MUSIC_PLATFORM_OPTIONS: &[(&str, &str)] = &[
    ("soda", "汽水"),
    ("tencent", "QQ音乐"),
    ("netease", "网易云"),
    ("kugou", "酷狗"),
    ("bilibili", "Bilibili"),
    ("applemusic", "Apple Music"),
    ("youtubemusic", "YouTube Music"),
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
/// Platforms offered for switching a search, most used first, with the short
/// names their buttons carry.
const SEARCH_PLATFORMS: &[(MusicPlatform, &str)] = &[
    (MusicPlatform::Tencent, "QQ音乐"),
    (MusicPlatform::Netease, "网易云"),
    (MusicPlatform::AppleMusic, "Apple"),
    (MusicPlatform::Soda, "汽水"),
    (MusicPlatform::Kugou, "酷狗"),
    (MusicPlatform::Bilibili, "B站"),
    (MusicPlatform::YouTubeMusic, "YouTube"),
];
const SEARCH_PAGE_SIZE: usize = 8;
/// The first page is fetched alone; asking for the next one fetches up to this.
const SEARCH_RESULT_LIMIT: usize = 32;
const SEARCH_CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const SEARCH_CACHE_CAPACITY: usize = 256;
/// A search page's heading is this and the keyword, which is how paging and
/// platform buttons recover the keyword without carrying it in callback data.
const SEARCH_HEADER_PREFIX: &str = "🔍 ";
const SEARCH_EXPIRED_NOTICE: &str = "搜索结果已过期，请重新搜索";
const SEARCH_OWNER_NOTICE: &str = "这是其他用户的搜索结果，请自己发送 /music 歌名";
const SEARCH_TEXT_LIMIT: usize = 40;
/// Deep-link prefix that opens a song's full lyric page in the private chat.
pub const LYRIC_START_PREFIX: &str = "lyric-";
/// 空 inline 查询的「搜索音乐」条目发出的提示里，按钮一点就搜这首
const INLINE_EXAMPLE_KEYWORD: &str = "稻香";

/// Search results by platform and keyword, with when they were fetched.
type SearchCache =
    std::collections::HashMap<(MusicPlatform, String), (Instant, Arc<SearchResults>)>;

static SEARCH_CACHE: LazyLock<std::sync::Mutex<SearchCache>> = LazyLock::new(Default::default);

static LYRIC_TO_SIMPLIFIED: LazyLock<OpenCC> =
    LazyLock::new(|| OpenCC::from_config(BuiltinConfig::T2s).expect("builtin OpenCC t2s config"));
static LYRIC_TO_TRADITIONAL: LazyLock<OpenCC> =
    LazyLock::new(|| OpenCC::from_config(BuiltinConfig::S2t).expect("builtin OpenCC s2t config"));

cmd!(
    "/music",
    "获取音乐",
    MusicCmd ,
    {
        ///子命令：search 搜索候选，get 直接下载
        #[command(subcommand)]
        command: Option<MusicSubcommand>,
        ///歌曲名搜索候选，链接直接获取；不填写时打开音乐平台面板
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
    ///设置默认平台、音质、封面和歌词文字
    #[command(alias = "settings")]
    Setting,
    ///查看收藏列表
    #[command(alias = "fav", alias = "favorites")]
    Favorite(MusicFavoriteCmd),
    ///展开歌单或专辑
    #[command(alias = "playlist", alias = "album")]
    Collection(MusicCollectionCmd),
    ///查看下载队列状态
    #[command(alias = "q")]
    Queue,
}

#[derive(Clone, Debug, clap::Args)]
struct MusicSearchCmd {
    ///音乐名、链接或 ID
    #[arg(required = true, num_args = 1.., trailing_var_arg = true)]
    query: Vec<String>,
}

#[derive(Clone, Debug, clap::Args)]
struct MusicGetCmd {
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
    /// 搜索结果翻页或换平台；关键词取自消息首行，不占回调长度
    SearchPage {
        platform: MusicPlatform,
        page: usize,
        requester_id: i64,
    },
    SearchClose {
        requester_id: i64,
    },
    Select {
        platform: MusicPlatform,
        id: String,
        search_keyword: Option<String>,
    },
    Cover(MusicPlatform, String),
    Lyrics(MusicPlatform, String, Option<MusicLyricScript>),
    /// 把富文本歌词另存为 LRC 文件发出
    LyricsFile(MusicPlatform, String, MusicLyricScript),
    InlineLyrics(MusicPlatform, String, MusicLyricScript),
    /// 把展开了歌词的 inline 歌曲收回成音频消息
    InlineCollapse(MusicPlatform, String),
    InlineSend {
        platform: MusicPlatform,
        id: String,
        quality: String,
        requester_id: i64,
    },
    Favorite(MusicFavoriteAction),
    Setting(MusicSettingAction),
    Panel {
        requester_id: i64,
        action: MusicPanelAction,
    },
    /// 下载队列状态弹窗（常量负载，无状态自描述回调）
    QueueStatus,
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
    LyricScript(String),
}

impl MusicSettingAction {
    fn encode(&self) -> String {
        match self {
            Self::Platform(value) => format!("platform {value}"),
            Self::Quality(value) => format!("quality {value}"),
            Self::LyricScript(value) => format!("lyric_script {value}"),
        }
    }

    fn decode(payload: &str) -> Result<Self, BotError> {
        let invalid = || BotError::Custom("Unknown music setting data".to_string());
        let (action, value) = payload.split_once(' ').ok_or_else(invalid)?;
        if !is_callback_token(value) {
            return Err(invalid());
        }
        match action {
            "platform" => Ok(Self::Platform(value.to_string())),
            "quality" => Ok(Self::Quality(value.to_string())),
            "lyric_script" => Ok(Self::LyricScript(value.to_string())),
            _ => Err(invalid()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MusicPanelView {
    Platforms,
    Settings,
}

#[derive(Clone, Debug)]
enum MusicPanelAction {
    Show(MusicPanelView),
    Platform(MusicPlatform),
    Setting(MusicSettingAction),
    Close,
}

impl MusicPanelAction {
    fn encode(&self) -> String {
        match self {
            Self::Show(MusicPanelView::Platforms) => "home".to_string(),
            Self::Show(MusicPanelView::Settings) => "settings".to_string(),
            Self::Platform(platform) => format!("platform {}", platform.callback_code()),
            Self::Setting(action) => format!("setting {}", action.encode()),
            Self::Close => "close".to_string(),
        }
    }

    fn decode(payload: &str) -> Result<Self, BotError> {
        match payload {
            "home" => Ok(Self::Show(MusicPanelView::Platforms)),
            "settings" => Ok(Self::Show(MusicPanelView::Settings)),
            "close" => Ok(Self::Close),
            _ => {
                if let Some(value) = payload.strip_prefix("platform ")
                    && let Some(platform) = MusicPlatform::from_callback_code(value)
                {
                    return Ok(Self::Platform(platform));
                }
                if let Some(value) = payload.strip_prefix("setting ") {
                    return MusicSettingAction::decode(value).map(Self::Setting);
                }
                Err(BotError::Custom("Unknown music panel action".to_string()))
            }
        }
    }
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
                crate::funcs::url_encode(&query.keyword)
            ),
            MusicCallbackAction::SearchPage {
                platform,
                page,
                requester_id,
            } => format!(
                "{CALLBACK_PREFIX} sp {} {page} {requester_id}",
                platform.callback_code()
            ),
            MusicCallbackAction::SearchClose { requester_id } => {
                format!("{CALLBACK_PREFIX} sc {requester_id}")
            }
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
                    let with_keyword = format!("{payload} {}", crate::funcs::url_encode(keyword));
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
            MusicCallbackAction::LyricsFile(platform, id, script) => format!(
                "{CALLBACK_PREFIX} lyricf {} {id} {}",
                platform.callback_code(),
                script.callback_code()
            ),
            MusicCallbackAction::InlineCollapse(platform, id) => {
                format!("{CALLBACK_PREFIX} icol {} {id}", platform.callback_code())
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
            MusicCallbackAction::Setting(action) => {
                format!("{CALLBACK_PREFIX} setting {}", action.encode())
            }
            MusicCallbackAction::Panel {
                requester_id,
                action,
            } => {
                if *requester_id <= 0 {
                    return None;
                }
                format!("{CALLBACK_PREFIX} panel {requester_id} {}", action.encode())
            }
            MusicCallbackAction::QueueStatus => format!("{CALLBACK_PREFIX} queue"),
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
            return MusicSettingAction::decode(payload.strip_prefix("setting ").unwrap_or(""))
                .map(MusicCallbackAction::Setting);
        }
        if action == "panel" {
            let requester_id = parse_callback_i64(parts.next(), "Invalid music panel owner")?;
            if requester_id <= 0 {
                return Err(BotError::Custom("Invalid music panel owner".to_string()));
            }
            let action = MusicPanelAction::decode(parts.next().unwrap_or(""))?;
            return Ok(MusicCallbackAction::Panel {
                requester_id,
                action,
            });
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
        if action == "sc" {
            let requester_id = parse_callback_i64(parts.next(), "Invalid music search owner")?;
            return Ok(MusicCallbackAction::SearchClose { requester_id });
        }
        if action == "sp" {
            let invalid = || BotError::Custom("Invalid music search page".to_string());
            let platform = parts
                .next()
                .and_then(MusicPlatform::from_callback_code)
                .ok_or_else(invalid)?;
            let (page, requester_id) = parts
                .next()
                .and_then(|rest| rest.split_once(' '))
                .ok_or_else(invalid)?;
            let page = page.parse::<usize>().ok().filter(|page| *page > 0);
            return Ok(MusicCallbackAction::SearchPage {
                platform,
                page: page.ok_or_else(invalid)?,
                requester_id: requester_id.parse().map_err(|_| invalid())?,
            });
        }
        if action == "queue" {
            return Ok(MusicCallbackAction::QueueStatus);
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
                keyword: crate::funcs::url_decode(value)
                    .map_err(|e| BotError::Custom(e.to_string()))?
                    .into_owned(),
            })),
            "d" => {
                let (id, search_keyword) = value.split_once(' ').map_or_else(
                    || (value.to_string(), None),
                    |(id, keyword)| {
                        (
                            id.to_string(),
                            crate::funcs::url_decode(keyword)
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
            "lyricf" => {
                let (id, script) = value
                    .split_once(' ')
                    .and_then(|(id, script)| {
                        Some((id, MusicLyricScript::from_callback_code(script)?))
                    })
                    .ok_or_else(|| BotError::Custom("Unknown lyric file data".to_string()))?;
                Ok(MusicCallbackAction::LyricsFile(
                    platform,
                    id.to_string(),
                    script,
                ))
            }
            "icol" => Ok(MusicCallbackAction::InlineCollapse(
                platform,
                value.to_string(),
            )),
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

async fn search_tracks(query: &MusicQuery, limit: usize) -> Result<Vec<MusicSearchItem>, BotError> {
    provider::search(query, limit).await
}

async fn search_collection(
    query: &MusicQuery,
    limit: usize,
) -> Result<Option<MusicCollection>, BotError> {
    provider::collection(query, limit).await
}

async fn resolve_track(
    query: &MusicQuery,
    selected_id: Option<&str>,
    settings: &UserMusicSettings,
) -> Result<MusicTrack, BotError> {
    provider::resolve(
        query,
        selected_id,
        provider::parse_quality(&settings.quality),
    )
    .await
}

fn media_request(query: &MusicQuery, selected_id: Option<&str>, quality: &str) -> MediaRequest {
    MediaRequest {
        platform: query.platform,
        keyword: query.keyword.clone(),
        selected_id: selected_id.map(str::to_string),
        quality: provider::parse_quality(quality),
        // MHub always gives the shared upload its cover; kept for older MHub.
        include_cover: true,
        wait_for_lyrics: false,
    }
}

fn settings_platform(settings: &UserMusicSettings) -> MusicPlatform {
    MusicPlatform::from_stored_default(&settings.default_platform)
}

fn user_id_from_message(msg: &Message) -> i64 {
    msg.from().map(|user| user.id).unwrap_or(0)
}

fn setting_item(icon: &str, name: &str, value: &str) -> String {
    format!("{icon} {name}：<b>{}</b>", html_escape(value))
}

fn music_settings_page(settings: &UserMusicSettings) -> RichPage {
    RichPage::default()
        .heading("⚙️ 音乐设置")
        .bullets_html([
            setting_item("🎵", "默认平台", platform_label(&settings.default_platform)),
            setting_item("🎧", "默认音质", quality_label(&settings.quality)),
            setting_item("📝", "歌词文字", lyric_script_label(&settings.lyric_script)),
        ])
        .footer("点击下方按钮修改设置")
}

fn music_settings_menu(settings: &UserMusicSettings) -> InlineKeyboardMarkup {
    music_settings_menu_with_owner(settings, None)
}

fn music_settings_menu_with_owner(
    settings: &UserMusicSettings,
    requester_id: Option<i64>,
) -> InlineKeyboardMarkup {
    let mut keyboard: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    let encode_setting = |action| {
        match requester_id {
            Some(requester_id) => MusicCallbackAction::Panel {
                requester_id,
                action: MusicPanelAction::Setting(action),
            },
            None => MusicCallbackAction::Setting(action),
        }
        .encode()
    };

    for chunk in MUSIC_PLATFORM_OPTIONS.chunks(3) {
        keyboard.push(
            chunk
                .iter()
                .filter_map(|(value, label)| {
                    encode_setting(MusicSettingAction::Platform(value.to_string())).map(
                        |callback| {
                            callback_button(
                                selected_label(label, settings.default_platform == *value),
                                callback,
                            )
                        },
                    )
                })
                .collect(),
        );
    }

    keyboard.push(
        MUSIC_QUALITY_OPTIONS
            .iter()
            .filter_map(|(value, label)| {
                encode_setting(MusicSettingAction::Quality(value.to_string())).map(|callback| {
                    callback_button(selected_label(label, settings.quality == *value), callback)
                })
            })
            .collect(),
    );

    keyboard.push(
        MUSIC_LYRIC_SCRIPT_OPTIONS
            .iter()
            .filter_map(|(value, label)| {
                encode_setting(MusicSettingAction::LyricScript(value.to_string())).map(|callback| {
                    callback_button(
                        selected_label(label, settings.lyric_script == *value),
                        callback,
                    )
                })
            })
            .collect(),
    );

    InlineKeyboardMarkup::new(keyboard)
}

fn music_panel_page(settings: &UserMusicSettings, view: MusicPanelView) -> RichPage {
    match view {
        MusicPanelView::Platforms => RichPage::default()
            .heading("🎵 音乐")
            .bullets_html([
                setting_item("🎵", "默认平台", platform_label(&settings.default_platform)),
                setting_item("🎧", "默认音质", quality_label(&settings.quality)),
            ])
            .paragraph("发送「/music 歌名」搜索，点结果即可获取")
            .footer("下方按钮切换默认平台"),
        MusicPanelView::Settings => music_settings_page(settings),
    }
}

fn music_panel_menu(settings: &UserMusicSettings, view: MusicPanelView) -> InlineKeyboardMarkup {
    let button = |label: String, action| {
        MusicCallbackAction::Panel {
            requester_id: settings.user_id,
            action,
        }
        .encode()
        .map(|callback| callback_button(label, callback))
    };
    let mut keyboard = match view {
        MusicPanelView::Platforms => {
            let current = settings_platform(settings);
            let mut rows = Vec::new();
            if let Some(current_button) = button(
                selected_label(current.label(), true),
                MusicPanelAction::Platform(current),
            ) {
                rows.push(vec![current_button]);
            }
            let others = MUSIC_PLATFORM_OPTIONS
                .iter()
                .filter_map(|(id, label)| {
                    let platform = MusicPlatform::from_alias(id)?;
                    (platform != current)
                        .then(|| button(label.to_string(), MusicPanelAction::Platform(platform)))?
                })
                .collect::<Vec<_>>();
            rows.extend(others.chunks(2).map(<[_]>::to_vec));
            rows
        }
        MusicPanelView::Settings => keyboard_rows(music_settings_menu_with_owner(
            settings,
            Some(settings.user_id),
        )),
    };
    let (label, destination) = match view {
        MusicPanelView::Platforms => ("更多设置", MusicPanelView::Settings),
        MusicPanelView::Settings => ("返回", MusicPanelView::Platforms),
    };
    keyboard.push(
        [
            button(label.to_string(), MusicPanelAction::Show(destination)),
            button("关闭".to_string(), MusicPanelAction::Close),
        ]
        .into_iter()
        .flatten()
        .collect(),
    );
    InlineKeyboardMarkup::new(keyboard)
}

fn music_panel_owner(msg: &Message) -> Option<i64> {
    msg.reply_markup()?
        .inline_keyboard
        .iter()
        .flatten()
        .find_map(|button| {
            match MusicCallbackAction::decode(button.callback_data.as_deref()?).ok()? {
                MusicCallbackAction::Panel { requester_id, .. } => Some(requester_id),
                _ => None,
            }
        })
}

fn inline_music_settings_menu(settings: &UserMusicSettings) -> InlineKeyboardMarkup {
    let mut keyboard = keyboard_rows(music_settings_menu(settings));
    if let Some(callback) = MusicCallbackAction::Close.encode() {
        keyboard.push(vec![callback_button("关闭", callback)]);
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

async fn send_music_panel(
    bot: &Bot,
    msg: &Message,
    settings: &UserMusicSettings,
    view: MusicPanelView,
) -> BotResult {
    if settings.user_id <= 0 {
        return Err(BotError::Custom(
            "无法识别用户，请使用个人账号打开音乐设置".to_string(),
        ));
    }
    bot.send(
        SendRichMessage::new(msg.chat().id(), music_panel_page(settings, view).build())
            .reply_parameters(reply_to(msg.message_id()))
            .reply_markup(music_panel_menu(settings, view)),
    )
    .await?;
    Ok(())
}

async fn edit_music_panel(
    bot: &Bot,
    msg: &Message,
    settings: &UserMusicSettings,
    view: MusicPanelView,
) -> BotResult {
    let page = music_panel_page(settings, view).build();
    let markup = music_panel_menu(settings, view);
    let edit = EditMessageText::new()
        .chat_id(msg.chat().id())
        .message_id(msg.message_id())
        .rich_message(page)
        .reply_markup(markup);
    match bot.send(edit).await.map_err(BotError::from) {
        Err(err) if !is_not_modified(&err) => return Err(err),
        _ => {}
    }
    Ok(())
}

async fn edit_inline_music_settings(
    bot: &Bot,
    inline_message_id: &str,
    settings: &UserMusicSettings,
) -> BotResult {
    bot.send(
        EditMessageText::new()
            .inline_message_id(inline_message_id)
            .rich_message(music_settings_page(settings).build())
            .reply_markup(inline_music_settings_menu(settings)),
    )
    .await?;
    Ok(())
}

fn favorite_scope_id(scope: FavoriteScope, msg: &Message, user_id: i64) -> Result<i64, BotError> {
    match scope {
        FavoriteScope::User => Ok(user_id),
        FavoriteScope::Group => {
            if is_group_chat(msg) {
                Ok(msg.chat().id())
            } else {
                Err(BotError::Custom("群收藏只能在群聊里使用".to_string()))
            }
        }
    }
}

fn is_group_chat(msg: &Message) -> bool {
    matches!(msg.chat(), Chat::Group(_) | Chat::Supergroup(_))
}

fn favorite_from_track(
    scope: FavoriteScope,
    scope_id: i64,
    user: &User,
    track: &MusicTrack,
) -> MusicFavorite {
    MusicFavorite {
        scope_type: scope.storage_key().to_string(),
        scope_id,
        platform: track.platform.id().to_string(),
        track_id: track.id.clone(),
        added_by_user_id: user.id,
        added_by_name: match &user.last_name {
            Some(last_name) => format!("{} {last_name}", user.first_name),
            None => user.first_name.to_string(),
        },
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
    bot.send(
        SendRichMessage::new(
            msg.chat().id(),
            favorite_list_page(scope, &favorites).build(),
        )
        .reply_parameters(reply_to(msg.message_id()))
        .reply_markup(favorite_list_menu(scope, scope_id, &favorites)),
    )
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
    bot.send(
        EditMessageText::new()
            .chat_id(msg.chat().id())
            .message_id(msg.message_id())
            .rich_message(favorite_list_page(scope, &favorites).build())
            .reply_markup(favorite_list_menu(scope, scope_id, &favorites)),
    )
    .await?;
    Ok(())
}

fn favorite_list_page(scope: FavoriteScope, favorites: &[MusicFavorite]) -> RichPage {
    let head = RichPage::default().heading(&format!("⭐ {}", scope.label()));
    if favorites.is_empty() {
        return head.paragraph("还没有收藏，在歌曲下方点「收藏」即可加入");
    }
    let items = favorites.iter().map(|favorite| {
        let song = html_escape(&favorite.song);
        let title = if favorite.link.trim().is_empty() {
            format!("<b>{song}</b>")
        } else {
            format!(
                "<a href=\"{}\"><b>{song}</b></a>",
                html_escape(&favorite.link)
            )
        };
        let album = if favorite.album.trim().is_empty() {
            String::new()
        } else {
            format!(" · <i>{}</i>", html_escape(&favorite.album))
        };
        format!("{title} — {}{album}", html_escape(&favorite.singer))
    });
    head.paragraph(&format!("共 {} 首", favorites.len()))
        .numbered_html(items)
        .footer("「发送」获取歌曲，「取消」移出收藏")
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
            row.push(callback_button(format!("发送 {}", index + 1), callback));
        }
        if let Some(callback) = remove_callback {
            row.push(callback_button(format!("取消 {}", index + 1), callback));
        }
        if !row.is_empty() {
            keyboard.push(row);
        }
    }
    keyboard.push(vec![callback_button(
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
        row.push(callback_button("确认取消", confirm));
    }
    if let Some(back) = back {
        row.push(callback_button("返回列表", back));
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
            .send(
                AnswerCallbackQuery::new(callback_id.to_string())
                    .text("保存音乐设置失败")
                    .show_alert(true),
            )
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
    view: MusicPanelView,
) -> BotResult {
    let mut settings = match get_user_settings(user_id).await {
        Ok(settings) => settings,
        Err(e) => {
            let _ = bot
                .send(
                    AnswerCallbackQuery::new(callback_id.to_string())
                        .text("获取音乐设置失败")
                        .show_alert(true),
                )
                .await;
            return Err(e);
        }
    };

    let (changed, response_text) = match apply_music_setting(&mut settings, action) {
        Ok(result) => result,
        Err(message) => {
            bot.send(
                AnswerCallbackQuery::new(callback_id.to_string())
                    .text(message)
                    .show_alert(true),
            )
            .await?;
            return Ok(());
        }
    };

    if changed {
        save_music_settings(bot, callback_id, &settings).await?;
        bot.send(AnswerCallbackQuery::new(callback_id.to_string()).text(response_text))
            .await?;
    } else {
        bot.send(AnswerCallbackQuery::new(callback_id.to_string()))
            .await?;
    }
    edit_music_panel(bot, msg, &settings, view).await
}

async fn handle_music_panel_callback(
    bot: &Bot,
    callback_id: &str,
    msg: &Message,
    user_id: i64,
    action: MusicPanelAction,
) -> BotResult {
    match action {
        MusicPanelAction::Show(view) => {
            bot.send(AnswerCallbackQuery::new(callback_id.to_string()))
                .await?;
            let settings = get_user_settings(user_id).await?;
            edit_music_panel(bot, msg, &settings, view).await
        }
        MusicPanelAction::Platform(platform) => {
            handle_setting_callback(
                bot,
                callback_id,
                msg,
                user_id,
                MusicSettingAction::Platform(platform.id().to_string()),
                MusicPanelView::Platforms,
            )
            .await
        }
        MusicPanelAction::Setting(action) => {
            handle_setting_callback(
                bot,
                callback_id,
                msg,
                user_id,
                action,
                MusicPanelView::Settings,
            )
            .await
        }
        MusicPanelAction::Close => {
            bot.send(AnswerCallbackQuery::new(callback_id.to_string()))
                .await?;
            bot.send(DeleteMessage::new(msg.chat().id(), msg.message_id()))
                .await?;
            Ok(())
        }
    }
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
                .send(
                    AnswerCallbackQuery::new(callback_id.to_string())
                        .text("获取音乐设置失败")
                        .show_alert(true),
                )
                .await;
            return Err(e);
        }
    };

    let (changed, response_text) = match apply_music_setting(&mut settings, action) {
        Ok(result) => result,
        Err(message) => {
            bot.send(
                AnswerCallbackQuery::new(callback_id.to_string())
                    .text(message)
                    .show_alert(true),
            )
            .await?;
            return Ok(());
        }
    };

    if changed {
        save_music_settings(bot, callback_id, &settings).await?;
        bot.send(AnswerCallbackQuery::new(callback_id.to_string()).text(response_text))
            .await?;
        edit_inline_music_settings(bot, inline_message_id, &settings).await?;
    } else {
        bot.send(AnswerCallbackQuery::new(callback_id.to_string()))
            .await?;
    }
    Ok(())
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

/// Sends a song MHub has in the shared Telegram cache, by `file_id`.
/// Delivers a song in a chat. The status message that showed the progress
/// becomes the song card itself; only without one, or when Telegram refuses
/// the edit, is a new message sent (and the status removed).
async fn send_media(
    bot: &Bot,
    msg: &Message,
    status_msg: Option<&Message>,
    reply_to: Option<i64>,
    query: &MusicQuery,
    media: &MusicMedia,
) -> BotResult {
    let track = &media.track;
    let markup = track_menu(track, Some(query), msg);
    if let Some(status) = status_msg {
        let mut edit = SongMessage::Chat(status)
            .edit()
            .rich_message(chat_song_card(media));
        if let Some(markup) = markup.clone() {
            edit = edit.reply_markup(markup);
        }
        match bot.send(edit).await.map_err(BotError::from) {
            Ok(_) => return Ok(()),
            // 文件失效交给上层作废缓存重取
            Err(err) if is_invalid_cached_file_error(&err) => return Err(err),
            Err(err) => log::error!("status could not become the song card: {err}"),
        }
    }

    let mut card = SendRichMessage::new(msg.chat().id(), chat_song_card(media));
    if let Some(reply_to) = reply_to {
        card = card.reply_parameters(crate::reply_to(reply_to));
    }
    if let Some(markup) = markup.clone() {
        card = card.reply_markup(markup);
    }
    let sent = match bot.send(card).await.map_err(BotError::from) {
        Ok(_) => Ok(()),
        Err(err) if is_invalid_cached_file_error(&err) => return Err(err),
        Err(err) => {
            // 其余原因退回普通音频消息
            log::error!("song card refused, sending audio: {err}");
            send_audio_message(bot, msg, reply_to, markup, media).await
        }
    };
    if sent.is_ok()
        && let Some(status) = status_msg
    {
        let _ = bot
            .send(DeleteMessage::new(status.chat().id(), status.message_id()))
            .await;
    }
    sent
}

async fn send_audio_message(
    bot: &Bot,
    msg: &Message,
    reply_to: Option<i64>,
    markup: Option<InlineKeyboardMarkup>,
    media: &MusicMedia,
) -> BotResult {
    let track = &media.track;
    let mut request = SendAudio::new(msg.chat().id(), InputFile::id(media.file_id.clone()))
        .caption(build_music_caption(media))
        .parse_mode(ParseMode::HTML)
        .title(track.song.clone())
        .performer(track.singer.clone());
    if let Some(reply_to) = reply_to {
        request = request.reply_parameters(crate::reply_to(reply_to));
    }
    if let Some(duration) = track.duration.filter(|duration| *duration > 0) {
        request = request.duration(duration);
    }
    if let Some(markup) = markup {
        request = request.reply_markup(markup);
    }
    if let Some(file_id) = media
        .thumb_file_id
        .as_deref()
        .map(str::trim)
        .filter(|file_id| !file_id.is_empty())
    {
        request = request.thumbnail(InputFile::id(file_id.to_string()));
    }
    bot.send(request).await?;
    Ok(())
}

/// A song as chat and inline messages show it: a rich card whose lyrics can
/// open in place.
fn chat_song_card(media: &MusicMedia) -> telers::types::InputRichMessage {
    song_card(
        &LyricSong::from_track(&media.track),
        media.track.platform,
        &song_audio(media),
    )
}

/// The song file a card plays, if `msg` is one.
fn card_audio_file_id(msg: &Message) -> Option<&str> {
    let Message::RichMessage(message) = msg else {
        return None;
    };
    message
        .rich_message
        .blocks
        .iter()
        .find_map(|block| match block {
            telers::types::RichBlock::Audio(audio) => Some(audio.audio.file_id.as_ref()),
            _ => None,
        })
}

fn build_music_caption(media: &MusicMedia) -> String {
    build_music_caption_with_size(
        &media.track,
        media.byte_size,
        media.decrypt_elapsed_ms.map(Duration::from_millis),
    )
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
    let info = media_info_lines(track, music_size, decrypt_elapsed)
        .iter()
        .map(|line| format!("{}\n", html_escape(line)))
        .collect::<String>();
    format!("<b>「{song_html}」- {singer}</b>\n{album_line}<blockquote>{info}</blockquote>")
}

/// What a song's message says about its file, one line each: size and
/// bitrate, decryption time, and the platform and format tags.
fn media_info_lines(
    track: &MusicTrack,
    music_size: u64,
    decrypt_elapsed: Option<Duration>,
) -> Vec<String> {
    let mut lines = Vec::new();
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
    if !info_parts.is_empty() {
        lines.push(info_parts.join(" "));
    }
    if let Some(elapsed) = decrypt_elapsed {
        lines.push(format!("解密: {}", format_duration(elapsed)));
    }
    lines.push(format!(
        "#{} #{}",
        platform_tag(track.platform),
        track.file_extension()
    ));
    lines
}

fn platform_tag(platform: MusicPlatform) -> &'static str {
    match platform {
        MusicPlatform::AppleMusic => "AppleMusic",
        MusicPlatform::Bilibili => "Bilibili",
        MusicPlatform::Kugou => "Kugou",
        MusicPlatform::Netease => "Netease",
        MusicPlatform::Soda => "Soda",
        MusicPlatform::Tencent => "QQMusic",
        MusicPlatform::YouTubeMusic => "YouTubeMusic",
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

/// Where a delivery reports its progress.
#[derive(Clone)]
enum StatusTarget {
    /// A status message (text or a caption) in the requesting chat.
    Message(Box<Message>),
    Inline(String),
}

impl StatusTarget {
    async fn show(&self, bot: &Bot, text: String, markup: Option<InlineKeyboardMarkup>) -> bool {
        match self {
            StatusTarget::Message(msg) => edit_callback_status_with_markup(bot, msg, text, markup)
                .await
                .is_ok(),
            StatusTarget::Inline(id) => {
                let mut request = EditMessageText::new()
                    .inline_message_id(id.as_str())
                    .text(text);
                if let Some(markup) = markup {
                    request = request.reply_markup(markup);
                }
                bot.send(request).await.is_ok()
            }
        }
    }
}

/// Edits the status in order, skipping repeats; finishes once every sender
/// is dropped.
fn status_reporter(
    bot: Bot,
    target: StatusTarget,
) -> (mpsc::UnboundedSender<String>, JoinHandle<()>) {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let task = tokio::spawn(async move {
        let mut last_edited = String::new();
        while let Some(text) = rx.recv().await {
            if text != last_edited && target.show(&bot, text.clone(), None).await {
                last_edited = text;
            }
        }
    });
    (tx, task)
}

/// Where a delivered song goes.
enum Destination<'a> {
    Chat {
        msg: &'a Message,
        status_msg: Option<&'a Message>,
        reply_to: Option<i64>,
        query: &'a MusicQuery,
    },
    Inline(&'a str),
}

impl Destination<'_> {
    async fn send(&self, bot: &Bot, media: &MusicMedia) -> BotResult {
        match self {
            Destination::Chat {
                msg,
                status_msg,
                reply_to,
                query,
            } => send_media(bot, msg, *status_msg, *reply_to, query, media).await,
            Destination::Inline(id) => edit_inline_song_card(bot, id, media).await,
        }
    }
}

/// Sends a song from the shared Telegram cache, asking MHub once for a fresh
/// file when Telegram rejects the cached one or it looks truncated.
async fn deliver(
    bot: &Bot,
    request: MediaRequest,
    user_id: i64,
    chat_id: Option<i64>,
    status: &StatusTarget,
    destination: Destination<'_>,
) -> Result<MusicMedia, BotError> {
    let mut retried = false;
    loop {
        let media = obtain_media(bot, request.clone(), user_id, chat_id, status).await?;
        if !retried && is_suspicious_media(&media.track, media.byte_size) {
            provider::invalidate(&media).await;
            retried = true;
            continue;
        }
        match destination.send(bot, &media).await {
            Ok(()) => return Ok(media),
            Err(err) if !retried && is_invalid_cached_file_error(&err) => {
                provider::invalidate(&media).await;
                retried = true;
            }
            Err(err) => return Err(err),
        }
    }
}

/// Gets the song into the shared Telegram cache through MHub. Cache hits stay
/// free; a miss takes a fair-admission place, is metered against the download
/// quota and holds a download slot until MHub has the audio (its upload to
/// Telegram no longer counts against concurrency).
async fn obtain_media(
    bot: &Bot,
    request: MediaRequest,
    user_id: i64,
    chat_id: Option<i64>,
    status: &StatusTarget,
) -> Result<MusicMedia, BotError> {
    let gateway = provider::gateway()?;
    // 即将发歌：老缓存若还没内嵌歌词，让 MHub 先补上再给 file_id（超时则仍给旧文件）
    let lookup = gateway
        .lookup_media(&MediaRequest {
            wait_for_lyrics: true,
            ..request.clone()
        })
        .await?;
    if let Some(media) = lookup.media {
        return Ok(media);
    }
    let _admission = queue::GOVERNOR.admit(user_id, chat_id)?;
    let download_token =
        ratelimit::ensure_admitted(ResourceAction::Download, user_id, chat_id, request.platform)?;
    let slot = {
        let queued_bot = bot.clone();
        let queued_status = status.clone();
        // 排队通知在 acquire_slot 内先 await 完成，不与后续编辑竞态
        let acquired = queue::GOVERNOR
            .acquire_slot(move || async move {
                queued_status
                    .show(
                        &queued_bot,
                        "⏳ 当前下载繁忙，已加入队列，请稍候…".to_string(),
                        Some(queue_status_menu()),
                    )
                    .await;
            })
            .await;
        match acquired {
            Ok(slot) => slot,
            Err(e) => {
                // 未实际下载，按凭证精确退还刚记录的下载额度
                ratelimit::LIMITER.refund(download_token);
                return Err(e);
            }
        }
    };

    let known_title = lookup.track.as_ref().map(|track| track.song.clone());
    let request = MediaRequest {
        selected_id: lookup.track.map(|track| track.id).or(request.selected_id),
        ..request
    };
    let (tx, reporter) = status_reporter(bot.clone(), status.clone());
    let mut progress = None;
    let mut slot = Some(slot);
    let result = gateway
        .media(&request, |job| match &job.state {
            JobState::Downloading { written, total } => {
                let title = job
                    .track
                    .as_ref()
                    .map(|track| track.song.clone())
                    .or_else(|| known_title.clone());
                if let Some(title) = title {
                    let report =
                        progress.get_or_insert_with(|| make_progress_throttle(title, tx.clone()));
                    report(DownloadProgress {
                        written: written.unwrap_or_default(),
                        total: *total,
                    });
                }
            }
            JobState::Uploading => {
                slot.take();
                let _ = tx.send("☁️ 下载完成，正在上传…".to_string());
            }
            _ => {}
        })
        .await;
    drop(progress);
    drop(tx);
    reporter.abort();
    let _ = reporter.await;
    Ok(result?)
}

/// Old Apple Music entries sometimes hold a preview with the full song's
/// duration; such a file is far below any real bitrate.
fn is_suspicious_media(track: &MusicTrack, byte_size: u64) -> bool {
    if track.platform != MusicPlatform::AppleMusic {
        return false;
    }
    let Some(duration) = track.duration.filter(|duration| *duration > 60) else {
        return false;
    };
    let estimated_bitrate = estimate_bitrate(byte_size, Some(duration)).unwrap_or_default();
    estimated_bitrate > 0 && estimated_bitrate < 64_000
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
        || text.contains("audio_invalid")
        || text.contains("media_invalid")
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
    let request = media_request(&query, None, &settings.quality);
    deliver(
        bot,
        request,
        user_id_from_message(msg),
        Some(msg.chat().id()),
        &StatusTarget::Message(Box::new(status_msg.clone())),
        Destination::Chat {
            msg,
            status_msg: Some(status_msg),
            reply_to: Some(msg.message_id()),
            query: &query,
        },
    )
    .await?;
    Ok(())
}

pub async fn music_inline_query_handler(bot: Bot, q: InlineQuery) -> BotResult {
    let user_id = q.from.id;
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
    // inline 查询随输入逐字触发，使用独立于命令搜索的更宽松配额；
    // 没有会话 chat，按用户/平台/全局维度限流
    if ratelimit::ensure_allowed(ResourceAction::InlineSearch, user_id, None, platform).is_err() {
        return answer_inline_throttled(&bot, &q, platform, &keyword, &quality).await;
    }
    let (tracks, search_failed) = match search_tracks(&query, 20).await {
        Ok(tracks) => (tracks, false),
        Err(err) => {
            log::warn!(
                "inline music search failed: platform={} err={err}",
                platform.id()
            );
            (Vec::new(), true)
        }
    };
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
        let (result_key, title, text) = if search_failed {
            (
                "error",
                "搜索失败，请稍后再试",
                format!("搜索失败：{}", keyword),
            )
        } else {
            (
                "empty",
                "没有找到可下载的音乐",
                format!("没有找到：{}", keyword),
            )
        };
        results.push(InlineQueryResult::Article(
            InlineQueryResultArticle::new(
                inline_result_id(result_key, platform, &keyword, &quality),
                title,
                InputTextMessageContent::new(text),
            )
            .description(format!(
                "{} · {}",
                platform.label(),
                quality_label(&quality)
            )),
        ));
    }
    results.push(inline_settings_result(&settings, user_id));

    bot.send(
        AnswerInlineQuery::new(q.id.clone(), results)
            .is_personal(true)
            .cache_time(1),
    )
    .await?;
    Ok(())
}

async fn answer_inline_throttled(
    bot: &Bot,
    q: &InlineQuery,
    platform: MusicPlatform,
    keyword: &str,
    quality: &str,
) -> BotResult {
    const THROTTLED_TEXT: &str = "请求过于频繁，请稍后再试";
    let result = InlineQueryResultArticle::new(
        inline_result_id("throttled", platform, keyword, quality),
        THROTTLED_TEXT,
        InputTextMessageContent::new(THROTTLED_TEXT),
    )
    .description(platform.label());
    bot.send(
        AnswerInlineQuery::new(q.id.clone(), [InlineQueryResult::Article(result)])
            .is_personal(true)
            .cache_time(1),
    )
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
    // 歌词批量搜索整体按一次歌词额度计费；单曲抓取走原始歌词缓存
    if ratelimit::ensure_allowed(ResourceAction::Lyric, q.from.id, None, platform).is_err() {
        return answer_inline_throttled(bot, q, platform, &keyword, &quality).await;
    }
    let query = MusicQuery {
        platform,
        keyword: keyword.clone(),
    };
    let script = MusicLyricScript::from_stored_value(&settings.lyric_script);
    let mut results = Vec::new();
    let (tracks, search_failed) = match search_tracks(&query, 8).await {
        Ok(tracks) => (tracks, false),
        Err(err) => {
            log::warn!(
                "inline lyric search failed: platform={} err={err}",
                platform.id()
            );
            (Vec::new(), true)
        }
    };
    let lyric_results = join_all(
        tracks
            .iter()
            .map(|track| fetch_lyrics_cached(track.platform, &track.id)),
    )
    .await;
    let found = tracks
        .iter()
        .zip(lyric_results)
        .filter_map(|(track, lyrics)| Some((track, lyrics.ok().flatten()?)))
        .collect::<Vec<_>>();
    let songs = found
        .iter()
        .map(|(track, _)| LyricSong::from_search_item(track))
        .collect::<Vec<_>>();
    let covers = join_all(songs.iter().map(|song| lyric_cover(bot, song))).await;
    for (((track, lyrics), song), cover) in found.iter().zip(&songs).zip(&covers) {
        let view = LyricView::new(song, track.platform, lyrics, script);
        results.push(inline_lyric_result(track, &view, cover, script));
    }
    if results.is_empty() {
        let (result_key, title, text) = if search_failed {
            (
                "lyric_error",
                "搜索失败，请稍后再试",
                format!("搜索失败：{}", keyword),
            )
        } else {
            (
                "lyric_empty",
                "没有找到可用歌词",
                format!("没有找到歌词：{}", keyword),
            )
        };
        results.push(InlineQueryResult::Article(
            InlineQueryResultArticle::new(
                inline_result_id(result_key, platform, &keyword, &quality),
                title,
                InputTextMessageContent::new(text),
            )
            .description(platform.label()),
        ));
    }
    bot.send(
        AnswerInlineQuery::new(q.id.clone(), results)
            .is_personal(true)
            .cache_time(1),
    )
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

/// 空查询：「搜索音乐」只发一句提示，按钮一点就在当前输入框填好示例歌名
async fn answer_inline_music_help(
    bot: &Bot,
    q: &InlineQuery,
    settings: &UserMusicSettings,
) -> BotResult {
    let platform = settings_platform(settings);
    let quality = music_cache_quality(settings);
    let username = crate::me().username.as_deref().unwrap_or_default();
    let result = InlineQueryResultArticle::new(
        inline_result_id("help", platform, &q.from.id.to_string(), &quality),
        "搜索音乐",
        InputTextMessageContent::new(inline_help_text(username)),
    )
    .description(format!(
        "在 @{username} 后面输入歌名，例如：{INLINE_EXAMPLE_KEYWORD}"
    ))
    .reply_markup(InlineKeyboardMarkup::new([[
        switch_inline_current_chat_button(
            format!("🔍 试试搜索「{INLINE_EXAMPLE_KEYWORD}」"),
            INLINE_EXAMPLE_KEYWORD,
        ),
    ]]));
    bot.send(
        AnswerInlineQuery::new(
            q.id.clone(),
            [
                InlineQueryResult::Article(result),
                inline_settings_result(settings, q.from.id),
            ],
        )
        .is_personal(true)
        .cache_time(5),
    )
    .await?;
    Ok(())
}

fn inline_help_text(username: &str) -> String {
    format!("🎵 在输入框里 @{username} 后面输入歌名就能搜索音乐")
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
            InputRichMessageContent::new(music_settings_page(settings).build()),
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
        InputTextMessageContent::new(format!("正在准备：{title}")),
    )
    .description(format!(
        "{} · {}",
        track.platform.label(),
        quality_label(quality)
    ))
    .reply_markup(InlineKeyboardMarkup::new([[callback_button(
        "没反应？点此刷新",
        callback,
    )]]));
    if let Some(url) = cover.and_then(|url| Url::parse(url).ok()) {
        result = result
            .thumbnail_url(String::from(url))
            .thumbnail_width(150)
            .thumbnail_height(150);
    }
    Some(InlineQueryResult::Article(result))
}

fn inline_lyric_result(
    track: &MusicSearchItem,
    view: &LyricView,
    cover: &Cover,
    script: MusicLyricScript,
) -> InlineQueryResult {
    let title = format!("歌词：{} - {}", track.song, track.singer);
    let mut result = InlineQueryResultArticle::new(
        inline_result_id("lyric", track.platform, &track.id, script.callback_code()),
        title,
        InputRichMessageContent::new(view.rich(cover)),
    )
    .description(format!("{} · {}", track.platform.label(), script.label()))
    .reply_markup(inline_lyric_switch_menu(track.platform, &track.id, script));
    if let Ok(url) = Url::parse(track.cover.trim()) {
        result = result
            .thumbnail_url(String::from(url))
            .thumbnail_width(150)
            .thumbnail_height(150);
    }
    InlineQueryResult::Article(result)
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

/// Under a rich lyric page: switch script, or get the lyrics as an LRC file.
fn rich_lyric_menu(
    platform: MusicPlatform,
    id: &str,
    script: MusicLyricScript,
) -> InlineKeyboardMarkup {
    let row = [
        MusicCallbackAction::Lyrics(platform, id.to_string(), Some(script.toggled()))
            .encode()
            .map(|callback| callback_button(script.switch_label(), callback)),
        MusicCallbackAction::LyricsFile(platform, id.to_string(), script)
            .encode()
            .map(|callback| callback_button("下载 LRC", callback)),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    InlineKeyboardMarkup::new([row])
}

#[allow(clippy::too_many_arguments)]
async fn handle_inline_send_callback(
    bot: &Bot,
    callback_id: &str,
    inline_message_id: &str,
    from: &User,
    platform: MusicPlatform,
    id: &str,
    quality: &str,
    requester_id: i64,
) -> BotResult {
    if requester_id != 0 && requester_id != from.id {
        bot.send(
            AnswerCallbackQuery::new(callback_id.to_string())
                .text("只能由发起者操作")
                .show_alert(true),
        )
        .await?;
        return Ok(());
    }

    bot.send(AnswerCallbackQuery::new(callback_id.to_string()).text("正在处理..."))
        .await?;

    run_inline_send_flow(bot, inline_message_id, from, platform, id, quality).await
}

async fn run_inline_send_flow(
    bot: &Bot,
    inline_message_id: &str,
    from: &User,
    platform: MusicPlatform,
    id: &str,
    quality: &str,
) -> BotResult {
    let lock_flag = hashing(("music-inline", inline_message_id));
    if LIMITER.is_running(lock_flag) {
        return Ok(());
    }
    let _guard = Guard::new(&LIMITER, lock_flag);

    let result =
        run_inline_send_flow_inner(bot, inline_message_id, from, platform, id, quality).await;
    if let Err(err) = &result {
        // 失败时把错误写回 inline 消息并恢复重试按钮，否则消息会永远停在状态文本上
        let mut request = EditMessageText::new()
            .inline_message_id(inline_message_id)
            .text(format!("{err}"));
        if let Some(markup) = inline_send_retry_menu(platform, id, quality, from.id) {
            request = request.reply_markup(markup);
        }
        let _ = bot.send(request).await;
    }
    result
}

fn inline_send_retry_menu(
    platform: MusicPlatform,
    id: &str,
    quality: &str,
    requester_id: i64,
) -> Option<InlineKeyboardMarkup> {
    let callback = MusicCallbackAction::InlineSend {
        platform,
        id: id.to_string(),
        quality: quality.to_string(),
        requester_id,
    }
    .encode()?;
    Some(InlineKeyboardMarkup::new([[callback_button(
        "重试", callback,
    )]]))
}

async fn run_inline_send_flow_inner(
    bot: &Bot,
    inline_message_id: &str,
    from: &User,
    platform: MusicPlatform,
    id: &str,
    quality: &str,
) -> BotResult {
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
    // inline 消息没有会话 chat，只按用户与全局维度限制
    deliver(
        bot,
        media_request(&query, Some(id), quality),
        from.id,
        None,
        &StatusTarget::Inline(inline_message_id.to_string()),
        Destination::Inline(inline_message_id),
    )
    .await?;
    Ok(())
}

fn song_audio(media: &MusicMedia) -> SongAudio {
    SongAudio::from_media(
        media,
        media_info_lines(
            &media.track,
            media.byte_size,
            media.decrypt_elapsed_ms.map(Duration::from_millis),
        ),
    )
}

/// Turns an inline message into the song's rich card. Rich messages can be
/// edited into lyric pages and back; an audio message cannot, so inline songs
/// are sent as cards and only fall back to audio if the card is refused.
async fn edit_inline_song_card(
    bot: &Bot,
    inline_message_id: &str,
    media: &MusicMedia,
) -> BotResult {
    let card = chat_song_card(media);
    let mut request = EditMessageText::new()
        .inline_message_id(inline_message_id)
        .rich_message(card);
    if let Some(markup) = inline_track_menu(&media.track) {
        request = request.reply_markup(markup);
    }
    match bot.send(request).await.map_err(BotError::from) {
        Ok(_) => Ok(()),
        Err(err) if is_not_modified(&err) => Ok(()),
        Err(err) => {
            log::error!("inline song card refused, sending audio: {err}");
            edit_inline_audio(bot, inline_message_id, media).await
        }
    }
}

async fn edit_inline_audio(bot: &Bot, inline_message_id: &str, media: &MusicMedia) -> BotResult {
    let track = &media.track;
    let mut audio = InputMediaAudio::new(InputFile::id(media.file_id.clone()))
        .caption(build_music_caption(media))
        .parse_mode(ParseMode::HTML)
        .title(track.song.clone())
        .performer(track.singer.clone());
    if let Some(duration) = track.duration.filter(|duration| *duration > 0) {
        audio = audio.duration(duration.min(u16::MAX as u32) as u16);
    }
    let mut request =
        EditMessageMedia::new(InputMedia::Audio(audio)).inline_message_id(inline_message_id);
    if let Some(markup) = inline_track_menu(track) {
        request = request.reply_markup(markup);
    }
    bot.send(request).await?;
    Ok(())
}

async fn edit_inline_status(bot: &Bot, inline_message_id: &str, text: impl Into<String>) {
    let _ = bot
        .send(
            EditMessageText::new()
                .inline_message_id(inline_message_id)
                .text(text.into()),
        )
        .await;
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

/// One platform's results for a keyword, shared by every page of it.
struct SearchResults {
    tracks: Vec<MusicSearchItem>,
    /// Nothing more to fetch: the platform returned fewer than asked, or the
    /// limit was reached.
    complete: bool,
}

impl SearchResults {
    fn page_count(&self) -> usize {
        self.tracks.len().div_ceil(SEARCH_PAGE_SIZE).max(1)
    }

    fn page(&self, page: usize) -> &[MusicSearchItem] {
        let start = (page - 1) * SEARCH_PAGE_SIZE;
        let end = (start + SEARCH_PAGE_SIZE).min(self.tracks.len());
        self.tracks.get(start..end).unwrap_or_default()
    }
}

/// Searches through a short-lived cache, so paging and going back to a
/// platform already shown do not ask MHub again. Only real searches are
/// metered.
async fn cached_search(
    query: &MusicQuery,
    needed: usize,
    user_id: i64,
    chat_id: Option<i64>,
) -> Result<Arc<SearchResults>, BotError> {
    let key = (query.platform, query.keyword.clone());
    let cached = lock_unpoisoned(&SEARCH_CACHE)
        .get(&key)
        .filter(|(at, _)| at.elapsed() < SEARCH_CACHE_TTL)
        .map(|(_, results)| results.clone());
    if let Some(results) = cached
        && (results.complete || results.tracks.len() >= needed)
    {
        return Ok(results);
    }

    ratelimit::ensure_allowed(ResourceAction::Search, user_id, chat_id, query.platform)?;
    let limit = if needed <= SEARCH_PAGE_SIZE {
        SEARCH_PAGE_SIZE
    } else {
        SEARCH_RESULT_LIMIT
    };
    let mut tracks = search_tracks(query, limit).await?;
    let complete = tracks.len() < limit || limit >= SEARCH_RESULT_LIMIT;
    tracks.truncate(SEARCH_RESULT_LIMIT);
    let results = Arc::new(SearchResults { tracks, complete });

    let mut cache = lock_unpoisoned(&SEARCH_CACHE);
    cache.retain(|_, (at, _)| at.elapsed() < SEARCH_CACHE_TTL);
    if cache.len() >= SEARCH_CACHE_CAPACITY
        && let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, (at, _))| *at)
            .map(|(key, _)| key.clone())
    {
        cache.remove(&oldest);
    }
    cache.insert(key, (Instant::now(), results.clone()));
    Ok(results)
}

fn lock_unpoisoned<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A link, `platform:id` or a long bare number names one song (or a
/// playlist) rather than describing it.
fn is_direct_query(keyword: &str) -> bool {
    let keyword = keyword.trim();
    let is_long_number =
        |value: &str| value.len() >= 7 && value.bytes().all(|b| b.is_ascii_digit());
    keyword.contains("http://")
        || keyword.contains("https://")
        || is_long_number(keyword)
        || keyword.split_once(':').is_some_and(|(platform, id)| {
            MusicPlatform::from_alias(platform).is_some() && is_callback_token(id)
        })
}

/// `/music 歌名` lists candidates; a link or ID is fetched straight away.
async fn send_music_search(
    bot: &Bot,
    msg: &Message,
    query: MusicQuery,
    settings: &UserMusicSettings,
) -> BotResult {
    let user_id = user_id_from_message(msg);
    if !is_direct_query(&query.keyword) {
        return open_search_page(bot, msg.chat().id(), Some(msg.message_id()), query, user_id)
            .await;
    }
    ratelimit::ensure_allowed(
        ResourceAction::Search,
        user_id,
        Some(msg.chat().id()),
        query.platform,
    )?;
    if let Some(collection) = search_collection(&query, 10).await? {
        return send_collection_results(
            bot,
            msg.chat().id(),
            Some(msg.message_id()),
            collection,
            user_id,
        )
        .await;
    }
    send_direct_music(bot, msg, query, settings).await
}

async fn send_direct_music(
    bot: &Bot,
    msg: &Message,
    query: MusicQuery,
    settings: &UserMusicSettings,
) -> BotResult {
    let status_msg = bot
        .send(
            SendMessage::new(
                msg.chat().id(),
                format!("⏳ 正在从{}获取音乐…", query.platform.label()),
            )
            .reply_parameters(reply_to(msg.message_id())),
        )
        .await?;

    // 成功时状态消息已经原地变成歌曲卡片
    match get_music(bot, query, msg, &status_msg, settings).await {
        Ok(()) => Ok(()),
        Err(e) => {
            bot.send(
                EditMessageText::new()
                    .chat_id(status_msg.chat().id())
                    .message_id(status_msg.message_id())
                    .text(format!("{e}")),
            )
            .await?;
            Err(e)
        }
    }
}

/// Posts a placeholder answering `reply_to`, then turns it into the first
/// page of results.
async fn open_search_page(
    bot: &Bot,
    chat_id: i64,
    reply_to: Option<i64>,
    query: MusicQuery,
    user_id: i64,
) -> BotResult {
    let mut request = SendRichMessage::new(
        chat_id,
        rich::status(&format!(
            "⏳ 正在{}搜索「{}」…",
            query.platform.label(),
            query.keyword
        )),
    );
    if let Some(reply_to) = reply_to {
        request = request.reply_parameters(crate::reply_to(reply_to));
    }
    let page_msg = bot.send(request).await?;
    let result = show_search_page(bot, &page_msg, &query, 1, user_id, user_id).await;
    if result.is_err() {
        // 命令出错时由命令处理器统一回复错误，这里只收起占位消息
        let _ = bot
            .send(DeleteMessage::new(
                page_msg.chat().id(),
                page_msg.message_id(),
            ))
            .await;
    }
    result
}

/// Renders one page of a search into `page_msg`. A failed search still
/// offers the other platforms; being rate limited leaves the page as it was.
async fn show_search_page(
    bot: &Bot,
    page_msg: &Message,
    query: &MusicQuery,
    page: usize,
    requester_id: i64,
    user_id: i64,
) -> BotResult {
    let needed = page.saturating_mul(SEARCH_PAGE_SIZE);
    let (content, markup) =
        match cached_search(query, needed, user_id, Some(page_msg.chat().id())).await {
            Ok(results) => {
                let page = page.min(results.page_count());
                (
                    search_page(query, &results, page),
                    search_page_menu(query, Some(&results), page, requester_id),
                )
            }
            Err(err @ BotError::Busy(_)) => return Err(err),
            Err(err) => {
                log::warn!(
                    "music search failed: platform={} err={err}",
                    query.platform.id()
                );
                (
                    search_failure_page(query, &err),
                    search_page_menu(query, None, 1, requester_id),
                )
            }
        };
    bot.send(
        EditMessageText::new()
            .chat_id(page_msg.chat().id())
            .message_id(page_msg.message_id())
            .rich_message(content.build())
            .reply_markup(markup),
    )
    .await?;
    Ok(())
}

/// The keyword a search page was made for, from its heading (or the first
/// line of a page sent before pages became rich messages).
fn search_page_keyword(msg: &Message) -> Option<String> {
    let title = rich::first_heading(msg).or_else(|| {
        msg.text()
            .and_then(|text| text.lines().next())
            .map(str::to_string)
    })?;
    let keyword = title.strip_prefix(SEARCH_HEADER_PREFIX)?.trim();
    (!keyword.is_empty()).then(|| keyword.to_string())
}

fn search_page_head(query: &MusicQuery) -> RichPage {
    RichPage::default().heading(&format!("{SEARCH_HEADER_PREFIX}{}", query.keyword))
}

fn search_page(query: &MusicQuery, results: &SearchResults, page: usize) -> RichPage {
    let head = search_page_head(query);
    let platform = query.platform.label();
    if results.tracks.is_empty() {
        return head
            .paragraph(&format!("{platform} · 没有找到结果"))
            .footer("换个关键词，或者点下面换个平台试试");
    }
    let pages = results.page_count();
    let position = if !results.complete {
        format!("第 {page} 页")
    } else if pages > 1 {
        format!("第 {page}/{pages} 页")
    } else {
        format!("共 {} 首", results.tracks.len())
    };
    head.paragraph(&format!("{platform} · {position}"))
        .numbered_html(track_list_items(results.page(page)))
        .footer("点击下方序号获取歌曲")
}

fn search_failure_page(query: &MusicQuery, err: &BotError) -> RichPage {
    search_page_head(query)
        .paragraph(&format!("{} · 搜索失败", query.platform.label()))
        .quote(&err.to_string())
        .footer("可以稍后重试，或者点下面换个平台")
}

/// `<b>song</b> — singer` items, numbered like the buttons below them.
fn track_list_items(tracks: &[MusicSearchItem]) -> Vec<String> {
    tracks
        .iter()
        .map(|track| {
            let song = html_escape(&truncate_chars(track.song.trim(), SEARCH_TEXT_LIMIT));
            let singer = track.singer.trim();
            if singer.is_empty() {
                format!("<b>{song}</b>")
            } else {
                format!(
                    "<b>{song}</b> — {}",
                    html_escape(&truncate_chars(singer, SEARCH_TEXT_LIMIT))
                )
            }
        })
        .collect()
}

/// Number buttons for `track_list_items`: one row up to five, else two
/// balanced rows.
fn track_pick_rows(
    tracks: &[MusicSearchItem],
    search_keyword: Option<&str>,
) -> Vec<Vec<InlineKeyboardButton>> {
    let buttons = tracks
        .iter()
        .enumerate()
        .filter_map(|(index, track)| {
            let callback = MusicCallbackAction::Select {
                platform: track.platform,
                id: track.id.clone(),
                search_keyword: search_keyword.map(str::to_string),
            }
            .encode()?;
            Some(callback_button((index + 1).to_string(), callback))
        })
        .collect::<Vec<_>>();
    let per_row = if buttons.len() <= 5 {
        5
    } else {
        buttons.len().div_ceil(2)
    };
    buttons.chunks(per_row).map(<[_]>::to_vec).collect()
}

fn search_page_menu(
    query: &MusicQuery,
    results: Option<&SearchResults>,
    page: usize,
    requester_id: i64,
) -> InlineKeyboardMarkup {
    let page_button = |label: &str, platform: MusicPlatform, page: usize| {
        MusicCallbackAction::SearchPage {
            platform,
            page,
            requester_id,
        }
        .encode()
        .map(|callback| callback_button(label, callback))
    };
    let mut rows = Vec::new();
    if let Some(results) = results.filter(|results| !results.tracks.is_empty()) {
        rows.extend(track_pick_rows(results.page(page), Some(&query.keyword)));
        let mut nav = Vec::new();
        if page > 1 {
            nav.extend(page_button("⬅️ 上一页", query.platform, page - 1));
        }
        if page < results.page_count() || !results.complete {
            nav.extend(page_button("下一页 ➡️", query.platform, page + 1));
        }
        if !nav.is_empty() {
            rows.push(nav);
        }
    }
    let platforms = SEARCH_PLATFORMS
        .iter()
        .filter_map(|(platform, label)| {
            page_button(
                &selected_label(label, *platform == query.platform),
                *platform,
                1,
            )
        })
        .collect::<Vec<_>>();
    rows.extend(platforms.chunks(4).map(<[_]>::to_vec));
    if let Some(callback) = (MusicCallbackAction::SearchClose { requester_id }).encode() {
        rows.push(vec![callback_button("❌ 关闭", callback)]);
    }
    InlineKeyboardMarkup::new(rows)
}

async fn handle_search_page_callback(
    bot: &Bot,
    callback_id: &str,
    msg: &Message,
    user_id: i64,
    platform: MusicPlatform,
    page: usize,
    requester_id: i64,
) -> BotResult {
    let Some(keyword) = search_page_keyword(msg) else {
        bot.send(
            AnswerCallbackQuery::new(callback_id.to_string())
                .text(SEARCH_EXPIRED_NOTICE)
                .show_alert(true),
        )
        .await?;
        return Ok(());
    };
    let query = MusicQuery { platform, keyword };
    match show_search_page(bot, msg, &query, page, requester_id, user_id).await {
        Ok(()) => {
            bot.send(AnswerCallbackQuery::new(callback_id.to_string()))
                .await?;
            Ok(())
        }
        // 点了当前所在的平台或页码，页面没有变化
        Err(err) if is_not_modified(&err) => {
            bot.send(
                AnswerCallbackQuery::new(callback_id.to_string())
                    .text(format!("已经是{}的第 {page} 页", platform.label())),
            )
            .await?;
            Ok(())
        }
        Err(err) => Err(err),
    }
}

/// Only whoever searched may page or close it; anyone may pick a song.
fn is_search_owner(requester_id: i64, user_id: i64) -> bool {
    requester_id <= 0 || requester_id == user_id
}

async fn send_collection_results(
    bot: &Bot,
    chat_id: i64,
    reply_to: Option<i64>,
    collection: MusicCollection,
    requester_id: i64,
) -> Result<(), BotError> {
    let head = RichPage::default().heading(&format!("💿 {}", collection.title));
    let platform = collection.platform.label();
    let page = if collection.items.is_empty() {
        head.paragraph(&format!("{platform} · 没有可发送的曲目"))
    } else {
        let count = if collection.items.len() < 10 {
            format!("共 {} 首", collection.items.len())
        } else {
            format!("前 {} 首", collection.items.len())
        };
        head.paragraph(&format!("{platform} · {count}"))
            .numbered_html(track_list_items(&collection.items))
            .footer("点击下方序号获取歌曲")
    };
    let mut rows = track_pick_rows(&collection.items, None);
    if let Some(callback) = (MusicCallbackAction::SearchClose { requester_id }).encode() {
        rows.push(vec![callback_button("❌ 关闭", callback)]);
    }
    let mut request =
        SendRichMessage::new(chat_id, page.build()).reply_markup(InlineKeyboardMarkup::new(rows));
    if let Some(reply_to) = reply_to {
        request = request.reply_parameters(crate::reply_to(reply_to));
    }
    bot.send(request).await?;
    Ok(())
}

async fn send_music_collection(
    bot: &Bot,
    msg: &Message,
    query: MusicQuery,
) -> Result<(), BotError> {
    ratelimit::ensure_allowed(
        ResourceAction::Collection,
        user_id_from_message(msg),
        Some(msg.chat().id()),
        query.platform,
    )?;
    let Some(collection) = search_collection(&query, 10).await? else {
        bot.send(
            SendMessage::new(msg.chat().id(), "没有识别到支持的歌单或专辑")
                .reply_parameters(reply_to(msg.message_id())),
        )
        .await?;
        return Ok(());
    };
    send_collection_results(
        bot,
        msg.chat().id(),
        Some(msg.message_id()),
        collection,
        user_id_from_message(msg),
    )
    .await
}

async fn get_music_cover(bot: &Bot, msg: &Message, url: &str) -> Result<(), BotError> {
    bot.send(
        SendPhoto::new(msg.chat().id(), InputFile::url(Url::parse(url)?))
            .reply_parameters(reply_to(msg.message_id())),
    )
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

// 歌词原始结果缓存 + 频率限制：30 分钟内命中缓存（含“无歌词”负缓存）时
// 不打 provider 也不消耗歌词额度，简繁切换因此是纯本地转换
async fn fetch_lyrics_limited(
    platform: MusicPlatform,
    id: &str,
    user_id: i64,
    chat_id: Option<i64>,
) -> Result<Option<provider::MusicLyrics>, BotError> {
    if let Some(cached) = ratelimit::cached_lyrics(platform, id) {
        return Ok(cached);
    }
    ratelimit::ensure_allowed(ResourceAction::Lyric, user_id, chat_id, platform)?;
    fetch_and_cache_lyrics(platform, id).await
}

// 只走缓存不扣额度的版本，给已在入口整体计费的批量场景（inline 歌词搜索）用
async fn fetch_lyrics_cached(
    platform: MusicPlatform,
    id: &str,
) -> Result<Option<provider::MusicLyrics>, BotError> {
    if let Some(cached) = ratelimit::cached_lyrics(platform, id) {
        return Ok(cached);
    }
    fetch_and_cache_lyrics(platform, id).await
}

async fn fetch_and_cache_lyrics(
    platform: MusicPlatform,
    id: &str,
) -> Result<Option<provider::MusicLyrics>, BotError> {
    let lyrics = provider::lyrics(platform, id).await?;
    ratelimit::store_lyrics(platform, id, lyrics.clone());
    Ok(lyrics)
}

fn is_lyric_button(button: &InlineKeyboardButton) -> bool {
    button.callback_data.as_deref().is_some_and(|data| {
        matches!(
            MusicCallbackAction::decode(data),
            Ok(MusicCallbackAction::Lyrics(..)) | Ok(MusicCallbackAction::InlineLyrics(..))
        )
    })
}

/// 过滤掉键盘里的歌词按钮（按回调数据解码判定，非按钮文字）；
/// 没有歌词按钮可去时返回 None，表示无需编辑
fn remove_lyric_buttons(markup: &InlineKeyboardMarkup) -> Option<InlineKeyboardMarkup> {
    let mut removed = false;
    let rows = markup
        .inline_keyboard
        .iter()
        .filter_map(|row| {
            let buttons = row
                .iter()
                .filter(|button| {
                    if is_lyric_button(button) {
                        removed = true;
                        false
                    } else {
                        true
                    }
                })
                .cloned()
                .collect::<Vec<_>>();
            (!buttons.is_empty()).then_some(buttons)
        })
        .collect::<Vec<_>>();
    removed.then(|| InlineKeyboardMarkup::new(rows))
}

async fn handle_lyrics_callback(
    bot: &Bot,
    callback_id: &str,
    msg: &Message,
    user_id: i64,
    platform: MusicPlatform,
    id: &str,
    script: MusicLyricScript,
) -> BotResult {
    // 歌曲卡片在原地展开歌词，和 inline 一样
    if card_audio_file_id(msg).is_some() {
        let settings = get_user_settings_or_default(user_id).await;
        return handle_song_lyrics_callback(
            bot,
            callback_id,
            SongMessage::Chat(msg),
            user_id,
            platform,
            id,
            script,
            &settings,
        )
        .await;
    }
    let lyrics = match fetch_lyrics_limited(platform, id, user_id, Some(msg.chat().id())).await {
        Ok(lyrics) => lyrics,
        Err(e) => {
            // 歌词获取失败（含限流）只弹提示：歌词按钮可能挂在搜索菜单或
            // 评论区补发消息上，走通用错误路径编辑正文会毁掉整组按钮
            log::warn!("歌词获取失败 {}:{id}: {e}", platform.id());
            let _ = bot
                .send(
                    AnswerCallbackQuery::new(callback_id.to_string())
                        .text(format!("{e}"))
                        .show_alert(true),
                )
                .await;
            return Ok(());
        }
    };
    let Some(lyrics) = lyrics else {
        // 歌词确认不可用：顺手撤掉被点消息上的歌词按钮，避免留下死按钮（尽力而为）
        if let Some(filtered) = msg.reply_markup().and_then(remove_lyric_buttons) {
            let _ = bot
                .send(
                    EditMessageReplyMarkup::new()
                        .chat_id(msg.chat().id())
                        .message_id(msg.message_id())
                        .reply_markup(filtered),
                )
                .await;
        }
        bot.send(
            AnswerCallbackQuery::new(callback_id.to_string())
                .text(format!("{} 没有可用歌词", platform.label()))
                .show_alert(true),
        )
        .await?;
        return Ok(());
    };
    // 旧版歌词以 LRC 文件发出，在原文件上切换简繁
    let fallback = LyricSong::from_audio(lyric_song_audio(msg));
    let song = LyricSong::resolve(platform, id, user_id, fallback).await;
    if message_has_document(msg) {
        let document = render_lyric_document(msg, &song, platform, id, &lyrics, script);
        let media = InputMediaDocument::new(InputFile::buffered_with_name(
            document.content.into_bytes(),
            document.file_name,
        ))
        .caption(document.caption)
        .parse_mode(ParseMode::HTML);
        bot.send(
            EditMessageMedia::new(InputMedia::Document(media))
                .chat_id(msg.chat().id())
                .message_id(msg.message_id())
                .reply_markup(lyric_switch_menu(platform, id, script)),
        )
        .await?;
        bot.send(
            AnswerCallbackQuery::new(callback_id.to_string())
                .text(format!("已切换{}", script.label())),
        )
        .await?;
        return Ok(());
    }
    if matches!(msg, Message::RichMessage(_)) {
        edit_lyric_page(bot, msg, &song, platform, id, &lyrics, script).await?;
        bot.send(
            AnswerCallbackQuery::new(callback_id.to_string())
                .text(format!("已切换{}", script.label())),
        )
        .await?;
        return Ok(());
    }
    send_lyric_page(
        bot,
        msg.chat().id(),
        Some(msg.message_id()),
        &song,
        platform,
        id,
        &lyrics,
        script,
    )
    .await?;
    bot.send(AnswerCallbackQuery::new(callback_id.to_string()).text("已发送歌词"))
        .await?;
    Ok(())
}

/// Sends a lyric page, without the cover if Telegram cannot fetch it.
#[allow(clippy::too_many_arguments)]
async fn send_lyric_page(
    bot: &Bot,
    chat_id: i64,
    reply: Option<i64>,
    song: &LyricSong,
    platform: MusicPlatform,
    id: &str,
    lyrics: &provider::MusicLyrics,
    script: MusicLyricScript,
) -> BotResult {
    let view = LyricView::new(song, platform, lyrics, script);
    let cover = lyric_cover(bot, song).await;
    let send = |cover: &Cover| {
        SendRichMessage::new(chat_id, view.rich(cover))
            .reply_parameters_option(reply.map(reply_to))
            .reply_markup(rich_lyric_menu(platform, id, script))
    };
    match bot.send(send(&cover)).await {
        Ok(_) => Ok(()),
        Err(err) if cover != Cover::None => {
            log::error!("lyric page with cover failed, sending without: {err}");
            bot.send(send(&Cover::None)).await?;
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}

/// Rewrites a lyric page in place, without the cover if Telegram cannot
/// fetch it.
async fn edit_lyric_page(
    bot: &Bot,
    msg: &Message,
    song: &LyricSong,
    platform: MusicPlatform,
    id: &str,
    lyrics: &provider::MusicLyrics,
    script: MusicLyricScript,
) -> BotResult {
    let view = LyricView::new(song, platform, lyrics, script);
    let cover = lyric_cover(bot, song).await;
    let edit = |cover: &Cover| {
        EditMessageText::new()
            .chat_id(msg.chat().id())
            .message_id(msg.message_id())
            .rich_message(view.rich(cover))
            .reply_markup(rich_lyric_menu(platform, id, script))
    };
    let result = match bot.send(edit(&cover)).await.map_err(BotError::from) {
        Err(err) if cover != Cover::None && !is_not_modified(&err) => {
            log::error!("lyric page with cover failed, editing without: {err}");
            bot.send(edit(&Cover::None)).await.map_err(BotError::from)
        }
        result => result,
    };
    match result {
        Err(err) if !is_not_modified(&err) => Err(err),
        _ => Ok(()),
    }
}

/// Sends the lyrics of a rich lyric page as an LRC file, answering the page.
async fn handle_lyric_file_callback(
    bot: &Bot,
    callback_id: &str,
    msg: &Message,
    user_id: i64,
    platform: MusicPlatform,
    id: &str,
    script: MusicLyricScript,
) -> BotResult {
    let lyrics = match fetch_lyrics_limited(platform, id, user_id, Some(msg.chat().id())).await {
        Ok(Some(lyrics)) => lyrics,
        Ok(None) => {
            bot.send(
                AnswerCallbackQuery::new(callback_id.to_string())
                    .text(format!("{} 没有可用歌词", platform.label()))
                    .show_alert(true),
            )
            .await?;
            return Ok(());
        }
        Err(e) => {
            let _ = bot
                .send(
                    AnswerCallbackQuery::new(callback_id.to_string())
                        .text(format!("{e}"))
                        .show_alert(true),
                )
                .await;
            return Ok(());
        }
    };
    let fallback = LyricSong::from_audio(lyric_song_audio(msg));
    let song = LyricSong::resolve(platform, id, user_id, fallback).await;
    let document = render_lyric_document(msg, &song, platform, id, &lyrics, script);
    let mut request = SendDocument::new(
        msg.chat().id(),
        InputFile::buffered_with_name(document.content.into_bytes(), document.file_name),
    )
    .reply_parameters(reply_to(msg.message_id()))
    .reply_markup(lyric_switch_menu(platform, id, script));
    if !document.caption.is_empty() {
        request = request
            .caption(document.caption)
            .parse_mode(ParseMode::HTML);
    }
    bot.send(request).await?;
    bot.send(AnswerCallbackQuery::new(callback_id.to_string()).text("已发送 LRC 文件"))
        .await?;
    Ok(())
}

/// The song an inline message carries, from the shared cache: the user's
/// quality first, then any other it was cached at.
async fn inline_song_media(
    platform: MusicPlatform,
    id: &str,
    settings: &UserMusicSettings,
) -> Option<MusicMedia> {
    let gateway = provider::gateway().ok()?;
    let query = MusicQuery {
        platform,
        keyword: id.to_string(),
    };
    let preferred = music_cache_quality(settings);
    let qualities = std::iter::once(preferred.as_str()).chain(
        MUSIC_QUALITY_OPTIONS
            .iter()
            .map(|(quality, _)| *quality)
            .filter(|quality| *quality != preferred),
    );
    for quality in qualities {
        let request = media_request(&query, Some(id), quality);
        if let Ok(lookup) = gateway.lookup_media(&request).await
            && let Some(media) = lookup.media
        {
            return Some(media);
        }
    }
    None
}

/// A song card a lyrics button sits on: an inline message, or a message in a
/// chat. Both are rich messages, so both open their lyrics in place.
enum SongMessage<'a> {
    Inline(&'a str),
    Chat(&'a Message),
}

impl SongMessage<'_> {
    fn edit(&self) -> EditMessageText {
        match self {
            SongMessage::Inline(id) => EditMessageText::new().inline_message_id(*id),
            SongMessage::Chat(msg) => EditMessageText::new()
                .chat_id(msg.chat().id())
                .message_id(msg.message_id()),
        }
    }
}

/// Opens a song card's lyrics in place: the card as it was, then the cover
/// and the lyric page, with buttons to switch script or fold it back.
#[allow(clippy::too_many_arguments)]
async fn handle_song_lyrics_callback(
    bot: &Bot,
    callback_id: &str,
    target: SongMessage<'_>,
    user_id: i64,
    platform: MusicPlatform,
    id: &str,
    script: MusicLyricScript,
    settings: &UserMusicSettings,
) -> BotResult {
    let chat_id = match &target {
        SongMessage::Chat(msg) => Some(msg.chat().id()),
        SongMessage::Inline(_) => None,
    };
    let Some(lyrics) = fetch_lyrics_limited(platform, id, user_id, chat_id).await? else {
        bot.send(
            AnswerCallbackQuery::new(callback_id.to_string())
                .text(format!("{} 没有可用歌词", platform.label()))
                .show_alert(true),
        )
        .await?;
        return Ok(());
    };
    let Some(media) = inline_song_media(platform, id, settings).await else {
        bot.send(
            AnswerCallbackQuery::new(callback_id.to_string())
                .text("这首歌的缓存已失效，请重新发送")
                .show_alert(true),
        )
        .await?;
        return Ok(());
    };
    // 缓存里的歌曲信息不带封面，缺了就向网关补一次
    let song = match LyricSong::from_track(&media.track) {
        song if song.cover.is_some() => song,
        song => LyricSong::resolve(platform, id, user_id, song).await,
    };
    let cover = lyric_cover(bot, &song).await;
    let audio = song_audio(&media);
    let page = LyricView::new(&song, platform, &lyrics, script).rich_with_audio(&cover, &audio);
    let edit = target
        .edit()
        .rich_message(page)
        .reply_markup(open_lyrics_menu(platform, id, script));
    match bot.send(edit).await.map_err(BotError::from) {
        Err(err) if !is_not_modified(&err) => return Err(err),
        _ => {}
    }
    bot.send(AnswerCallbackQuery::new(callback_id.to_string()))
        .await?;
    Ok(())
}

/// Under a song card with its lyrics open: switch script, or fold back.
fn open_lyrics_menu(
    platform: MusicPlatform,
    id: &str,
    script: MusicLyricScript,
) -> InlineKeyboardMarkup {
    let row = [
        MusicCallbackAction::Lyrics(platform, id.to_string(), Some(script.toggled()))
            .encode()
            .map(|callback| callback_button(script.switch_label(), callback)),
        MusicCallbackAction::InlineCollapse(platform, id.to_string())
            .encode()
            .map(|callback| callback_button("收起歌词", callback)),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    InlineKeyboardMarkup::new([row])
}

/// Folds a song card's lyrics back into the card.
async fn handle_song_collapse_callback(
    bot: &Bot,
    callback_id: &str,
    target: SongMessage<'_>,
    platform: MusicPlatform,
    id: &str,
    settings: &UserMusicSettings,
) -> BotResult {
    let Some(media) = inline_song_media(platform, id, settings).await else {
        bot.send(
            AnswerCallbackQuery::new(callback_id.to_string())
                .text("这首歌的缓存已失效，请重新发送")
                .show_alert(true),
        )
        .await?;
        return Ok(());
    };
    match target {
        SongMessage::Inline(id) => edit_inline_song_card(bot, id, &media).await?,
        SongMessage::Chat(msg) => {
            let mut edit = SongMessage::Chat(msg)
                .edit()
                .rich_message(chat_song_card(&media));
            if let Some(markup) = track_menu(&media.track, None, msg) {
                edit = edit.reply_markup(markup);
            }
            match bot.send(edit).await.map_err(BotError::from) {
                Err(err) if !is_not_modified(&err) => return Err(err),
                _ => {}
            }
        }
    }
    bot.send(AnswerCallbackQuery::new(callback_id.to_string()))
        .await?;
    Ok(())
}

async fn handle_inline_lyric_text_callback(
    bot: &Bot,
    callback_id: &str,
    inline_message_id: &str,
    user_id: i64,
    platform: MusicPlatform,
    id: &str,
    script: MusicLyricScript,
) -> BotResult {
    // inline 消息的回调里拿不到当前键盘，无法可靠地摘除歌词按钮，保持仅弹窗提示
    let Some(lyrics) = fetch_lyrics_limited(platform, id, user_id, None).await? else {
        bot.send(
            AnswerCallbackQuery::new(callback_id.to_string())
                .text(format!("{} 没有可用歌词", platform.label()))
                .show_alert(true),
        )
        .await?;
        return Ok(());
    };
    // inline 消息读不回原内容，歌名靠网关解析一次
    let song = LyricSong::resolve(platform, id, user_id, LyricSong::default()).await;
    let cover = lyric_cover(bot, &song).await;
    bot.send(
        EditMessageText::new()
            .inline_message_id(inline_message_id)
            .rich_message(LyricView::new(&song, platform, &lyrics, script).rich(&cover))
            .reply_markup(inline_lyric_switch_menu(platform, id, script)),
    )
    .await?;
    bot.send(
        AnswerCallbackQuery::new(callback_id.to_string()).text(format!("已切换{}", script.label())),
    )
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
    song: &LyricSong,
    platform: MusicPlatform,
    id: &str,
    lyrics: &provider::MusicLyrics,
    script: MusicLyricScript,
) -> RenderedLyricDocument {
    let content = convert_lyric_script(
        &build_lrc_content(&lyrics.plain, &lyrics.translation),
        script,
    );
    let file_name = build_lyric_file_name(msg, platform, id);
    let caption = LyricView::new(song, platform, lyrics, script).caption();
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

fn lyric_switch_menu(
    platform: MusicPlatform,
    id: &str,
    script: MusicLyricScript,
) -> InlineKeyboardMarkup {
    let target = script.toggled();
    let keyboard = MusicCallbackAction::Lyrics(platform, id.to_string(), Some(target))
        .encode()
        .map(|callback| vec![vec![callback_button(script.switch_label(), callback)]])
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
        .map(|callback| vec![vec![callback_button(script.switch_label(), callback)]])
        .unwrap_or_default();
    InlineKeyboardMarkup::new(keyboard)
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

/// The song a lyric button belongs to: the audio it sits on, or the audio
/// the message (a rich lyric page, a comment-thread button row) replies to.
fn lyric_song_audio(msg: &Message) -> Option<&telers::types::Audio> {
    msg.audio()
        .or_else(|| msg.reply_to_message().and_then(Message::audio))
}

fn audio_lyric_base_name(msg: &Message) -> Option<String> {
    let audio = lyric_song_audio(msg)?;
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
    from: &User,
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
                FavoriteScope::User => from.id,
                FavoriteScope::Group => chat_id.unwrap_or(msg.chat().id()),
            };
            if scope == FavoriteScope::Group && !is_group_chat(msg) {
                bot.send(
                    AnswerCallbackQuery::new(callback_id.to_string())
                        .text("群收藏只能在群聊消息中使用")
                        .show_alert(true),
                )
                .await?;
                return Ok(());
            }
            if is_favorited(scope.storage_key(), scope_id, platform.id(), &id).await? {
                bot.send(
                    AnswerCallbackQuery::new(callback_id.to_string())
                        .text("已收藏，可在收藏列表中取消"),
                )
                .await?;
                return Ok(());
            }
            let query = MusicQuery {
                platform,
                keyword: id.clone(),
            };
            let track = resolve_track(&query, Some(&id), settings).await?;
            upsert_favorite(favorite_from_track(scope, scope_id, from, &track)).await?;
            bot.send(
                AnswerCallbackQuery::new(callback_id.to_string())
                    .text(format!("已加入{}", scope.label())),
            )
            .await?;
        }
        MusicFavoriteAction::AskRemove {
            scope,
            platform,
            id,
            chat_id,
        } => {
            let scope_id = match scope {
                FavoriteScope::User => from.id,
                FavoriteScope::Group => chat_id.unwrap_or(msg.chat().id()),
            };
            bot.send(AnswerCallbackQuery::new(callback_id.to_string()))
                .await?;
            bot.send(
                EditMessageReplyMarkup::new()
                    .chat_id(msg.chat().id())
                    .message_id(msg.message_id())
                    .reply_markup(favorite_confirm_menu(scope, scope_id, platform, &id)),
            )
            .await?;
        }
        MusicFavoriteAction::Remove {
            scope,
            platform,
            id,
            chat_id,
        } => {
            let scope_id = match scope {
                FavoriteScope::User => from.id,
                FavoriteScope::Group => chat_id.unwrap_or(msg.chat().id()),
            };
            let removed =
                remove_favorite(scope.storage_key(), scope_id, platform.id(), &id).await?;
            bot.send(
                AnswerCallbackQuery::new(callback_id.to_string()).text(if removed {
                    "已取消收藏"
                } else {
                    "收藏不存在"
                }),
            )
            .await?;
            edit_music_favorites(bot, msg, scope, scope_id).await?;
        }
        MusicFavoriteAction::List { scope, chat_id } => {
            let scope_id = match scope {
                FavoriteScope::User => from.id,
                FavoriteScope::Group => chat_id.unwrap_or(msg.chat().id()),
            };
            bot.send(AnswerCallbackQuery::new(callback_id.to_string()))
                .await?;
            edit_music_favorites(bot, msg, scope, scope_id).await?;
        }
        MusicFavoriteAction::Close => {
            bot.send(AnswerCallbackQuery::new(callback_id.to_string()))
                .await?;
            let _ = bot
                .send(DeleteMessage::new(msg.chat().id(), msg.message_id()))
                .await;
        }
    }
    Ok(())
}

async fn handle_inline_favorite_callback(
    bot: &Bot,
    callback_id: &str,
    from: &User,
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
        bot.send(
            AnswerCallbackQuery::new(callback_id.to_string())
                .text("inline 模式只支持个人收藏")
                .show_alert(true),
        )
        .await?;
        return Ok(());
    };

    let scope_id = from.id;
    if is_favorited(FAVORITE_SCOPE_USER, scope_id, platform.id(), &id).await? {
        remove_favorite(FAVORITE_SCOPE_USER, scope_id, platform.id(), &id).await?;
        bot.send(AnswerCallbackQuery::new(callback_id.to_string()).text("已取消收藏"))
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
    bot.send(AnswerCallbackQuery::new(callback_id.to_string()).text("已加入个人收藏"))
        .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn get_callback_music(
    bot: Bot,
    msg: Message,
    user_id: i64,
    platform: MusicPlatform,
    id: &str,
    search_keyword: Option<&str>,
    settings: &UserMusicSettings,
) -> Result<(), BotError> {
    edit_callback_status(
        &bot,
        &msg,
        format!("⏳ 正在从{}获取音乐…", platform.label()),
    )
    .await?;
    let query = MusicQuery {
        platform,
        keyword: id.to_string(),
    };
    let search_query = search_keyword
        .map(str::trim)
        .filter(|keyword| !keyword.is_empty())
        .map(|keyword| MusicQuery {
            platform,
            keyword: keyword.to_string(),
        });
    let query_for_menu = search_query.as_ref().unwrap_or(&query);
    let status_msg = is_page_message(&msg).then_some(&msg);
    let media = deliver(
        &bot,
        media_request(&query, Some(id), &settings.quality),
        user_id,
        Some(msg.chat().id()),
        &StatusTarget::Message(Box::new(msg.clone())),
        Destination::Chat {
            msg: &msg,
            status_msg,
            reply_to: callback_reply_target(&msg),
            query: query_for_menu,
        },
    )
    .await?;
    if status_msg.is_some() {
        return Ok(());
    }
    let verb = if media.cached {
        "已从缓存发送"
    } else {
        "已发送"
    };
    delete_callback_status(
        &bot,
        &msg,
        format!("{verb}：{} - {}", media.track.song, media.track.singer),
    )
    .await?;
    Ok(())
}

fn make_progress_throttle(
    title: String,
    tx: mpsc::UnboundedSender<String>,
) -> impl FnMut(DownloadProgress) + Send + 'static {
    let mut last_progress_text = String::new();
    let mut last_progress_at: Option<Instant> = None;
    let mut reported_complete = false;
    move |progress: DownloadProgress| {
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
    }
}

fn format_download_progress(title: &str, progress: DownloadProgress) -> String {
    const BAR_CELLS: usize = 10;
    let written_mb = bytes_to_mb(progress.written);
    match progress.total {
        Some(total) if total > 0 => {
            let total_mb = bytes_to_mb(total);
            let percent = (progress.written as f64 * 100.0 / total as f64).clamp(0.0, 100.0);
            let filled = ((percent / 100.0 * BAR_CELLS as f64).round() as usize).min(BAR_CELLS);
            format!(
                "⬇️ 正在下载「{title}」\n{}{} {percent:.1}%\n{written_mb:.2} MB / {total_mb:.2} MB",
                "▰".repeat(filled),
                "▱".repeat(BAR_CELLS - filled)
            )
        }
        _ => format!("⬇️ 正在下载「{title}」\n已下载 {written_mb:.2} MB"),
    }
}

fn bytes_to_mb(bytes: u64) -> f64 {
    bytes as f64 / 1024.0 / 1024.0
}

fn message_has_text(msg: &Message) -> bool {
    matches!(msg, Message::Text(_))
}

/// A message the bot wrote as a page (plain or rich text) rather than a
/// file with a caption.
fn is_page_message(msg: &Message) -> bool {
    matches!(msg, Message::Text(_) | Message::RichMessage(_))
}

fn message_has_document(msg: &Message) -> bool {
    matches!(msg, Message::Document(_))
}

fn callback_reply_target(msg: &Message) -> Option<i64> {
    if is_page_message(msg) {
        msg.reply_to_message().map(|msg| msg.message_id())
    } else {
        Some(msg.message_id())
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
    if matches!(msg, Message::RichMessage(_)) {
        let request = EditMessageText::new()
            .chat_id(msg.chat().id())
            .message_id(msg.message_id())
            .rich_message(rich::status(&text))
            .reply_markup(markup.unwrap_or_else(empty_keyboard));
        bot.send(request).await?;
    } else if message_has_text(msg) {
        let mut request = EditMessageText::new()
            .chat_id(msg.chat().id())
            .message_id(msg.message_id())
            .text(text);
        request = match markup {
            Some(markup) => request.reply_markup(markup),
            None => request.reply_markup(empty_keyboard()),
        };
        bot.send(request).await?;
    } else {
        let mut request = EditMessageCaption::new()
            .chat_id(msg.chat().id())
            .message_id(msg.message_id())
            .caption(text);
        request = match markup {
            Some(markup) => request.reply_markup(markup),
            None => request.reply_markup(empty_keyboard()),
        };
        bot.send(request).await?;
    }
    Ok(())
}

async fn delete_callback_status(
    bot: &Bot,
    msg: &Message,
    fallback_text: String,
) -> Result<(), BotError> {
    if is_page_message(msg) {
        let _ = bot
            .send(DeleteMessage::new(msg.chat().id(), msg.message_id()))
            .await;
    } else {
        let _ = edit_callback_status(bot, msg, fallback_text).await;
    }
    Ok(())
}

fn queue_status_text(snapshot: &queue::QueueSnapshot) -> String {
    let waiting_cap = if snapshot.wait_limit > 0 {
        format!("/{}", snapshot.wait_limit)
    } else {
        String::new()
    };
    let active_cap = if snapshot.global_limit > 0 {
        format!("/{}", snapshot.global_limit)
    } else {
        String::new()
    };
    format!(
        "📥 下载队列\n下载中：{}/{}\n排队中：{}{}\n进行中任务：{}{}",
        snapshot.running,
        snapshot.concurrency,
        snapshot.waiting,
        waiting_cap,
        snapshot.active,
        active_cap,
    )
}

fn queue_status_menu() -> InlineKeyboardMarkup {
    let keyboard = MusicCallbackAction::QueueStatus
        .encode()
        .map(|callback| vec![vec![callback_button("查看队列", callback)]])
        .unwrap_or_default();
    InlineKeyboardMarkup::new(keyboard)
}

async fn send_queue_status(bot: &Bot, msg: &Message) -> BotResult {
    bot.send(
        SendMessage::new(
            msg.chat().id(),
            queue_status_text(&queue::GOVERNOR.snapshot()),
        )
        .reply_parameters(reply_to(msg.message_id())),
    )
    .await?;
    Ok(())
}

pub async fn music_callback(bot: Bot, q: CallbackQuery) -> Result<(), BotError> {
    if let Some(data) = q.data {
        let callback_id = q.id.clone();
        let user_id = q.from.id;
        let from = q.from.clone();
        let action = match MusicCallbackAction::decode(&data) {
            Ok(action) => action,
            Err(e) => {
                let _ = bot
                    .send(
                        AnswerCallbackQuery::new(callback_id)
                            .text(format!("{e}"))
                            .show_alert(true),
                    )
                    .await;
                return Err(e);
            }
        };
        if let MusicCallbackAction::Panel { requester_id, .. } = &action
            && *requester_id != user_id
        {
            bot.send(
                AnswerCallbackQuery::new(callback_id)
                    .text(MUSIC_PANEL_OWNER_NOTICE)
                    .show_alert(true),
            )
            .await?;
            return Ok(());
        }
        let action = match action {
            // 队列状态与消息上下文无关（inline 与普通消息都可点），直接弹窗返回
            MusicCallbackAction::QueueStatus => {
                bot.send(
                    AnswerCallbackQuery::new(callback_id)
                        .text(queue_status_text(&queue::GOVERNOR.snapshot()))
                        .show_alert(true),
                )
                .await?;
                return Ok(());
            }
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
                    user_id,
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
                return handle_song_lyrics_callback(
                    &bot,
                    &callback_id,
                    SongMessage::Inline(&inline_message_id),
                    user_id,
                    platform,
                    &id,
                    script,
                    &settings,
                )
                .await;
            }
            MusicCallbackAction::InlineCollapse(platform, id) if q.inline_message_id.is_some() => {
                let Some(inline_message_id) = q.inline_message_id else {
                    return Ok(());
                };
                let settings = get_user_settings_or_default(user_id).await;
                return handle_song_collapse_callback(
                    &bot,
                    &callback_id,
                    SongMessage::Inline(&inline_message_id),
                    platform,
                    &id,
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
                bot.send(
                    EditMessageText::new()
                        .inline_message_id(inline_message_id)
                        .text("已关闭")
                        .reply_markup(empty_keyboard()),
                )
                .await?;
                bot.send(AnswerCallbackQuery::new(callback_id.clone()))
                    .await?;
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
            Some(mbi_msg) => match *mbi_msg {
                MaybeInaccessibleMessage::InaccessibleMessage(_) => return Ok(()),
                MaybeInaccessibleMessage::Message(msg) => msg,
            },
        };
        // A client can forge callback data, so also check the bot-generated
        // keyboard rather than trusting the owner encoded in the click alone.
        if music_panel_owner(&msg).is_some_and(|owner| owner != user_id) {
            bot.send(
                AnswerCallbackQuery::new(callback_id)
                    .text(MUSIC_PANEL_OWNER_NOTICE)
                    .show_alert(true),
            )
            .await?;
            return Ok(());
        }
        let lock_flag = hashing((msg.chat().id(), msg.message_id()));
        if LIMITER.is_running(lock_flag) {
            bot.send(AnswerCallbackQuery::new(callback_id).text("正在处理上一次请求..."))
                .await?;
            return Ok(());
        }
        let _guard = Guard::new(&LIMITER, lock_flag);
        if let MusicCallbackAction::SearchPage { requester_id, .. }
        | MusicCallbackAction::SearchClose { requester_id } = &action
            && !is_search_owner(*requester_id, user_id)
        {
            bot.send(
                AnswerCallbackQuery::new(callback_id)
                    .text(SEARCH_OWNER_NOTICE)
                    .show_alert(true),
            )
            .await?;
            return Ok(());
        }
        let preserve_panel = matches!(
            &action,
            MusicCallbackAction::Panel { .. }
                | MusicCallbackAction::Setting(_)
                | MusicCallbackAction::SearchPage { .. }
        );
        if !matches!(
            action,
            MusicCallbackAction::Setting(_)
                | MusicCallbackAction::Panel { .. }
                | MusicCallbackAction::Favorite(_)
                | MusicCallbackAction::Lyrics(_, _, _)
                | MusicCallbackAction::LyricsFile(_, _, _)
                | MusicCallbackAction::InlineLyrics(_, _, _)
                | MusicCallbackAction::SearchPage { .. }
                | MusicCallbackAction::SearchClose { .. }
                | MusicCallbackAction::InlineCollapse(..)
                | MusicCallbackAction::Close
        ) {
            bot.send(AnswerCallbackQuery::new(callback_id.clone()).text("正在处理..."))
                .await?;
        }

        let result = match action {
            MusicCallbackAction::SearchMore(query) => {
                open_search_page(
                    &bot,
                    msg.chat().id(),
                    Some(msg.message_id()),
                    query,
                    user_id,
                )
                .await
            }
            MusicCallbackAction::SearchPage {
                platform,
                page,
                requester_id,
            } => {
                handle_search_page_callback(
                    &bot,
                    &callback_id,
                    &msg,
                    user_id,
                    platform,
                    page,
                    requester_id,
                )
                .await
            }
            MusicCallbackAction::SearchClose { .. } => {
                let _ = bot
                    .send(DeleteMessage::new(msg.chat().id(), msg.message_id()))
                    .await;
                bot.send(AnswerCallbackQuery::new(callback_id.clone()))
                    .await?;
                Ok(())
            }
            MusicCallbackAction::Cover(platform, id) => {
                let settings = get_user_settings_or_default(user_id).await;
                get_callback_cover(bot.clone(), msg.clone(), platform, &id, &settings).await
            }
            MusicCallbackAction::Lyrics(platform, id, script) => {
                let settings = get_user_settings_or_default(user_id).await;
                let script = script
                    .unwrap_or_else(|| MusicLyricScript::from_stored_value(&settings.lyric_script));
                handle_lyrics_callback(&bot, &callback_id, &msg, user_id, platform, &id, script)
                    .await
            }
            MusicCallbackAction::LyricsFile(platform, id, script) => {
                handle_lyric_file_callback(&bot, &callback_id, &msg, user_id, platform, &id, script)
                    .await
            }
            MusicCallbackAction::InlineLyrics(platform, id, script) => {
                handle_lyrics_callback(&bot, &callback_id, &msg, user_id, platform, &id, script)
                    .await
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
                    user_id,
                    platform,
                    &id,
                    search_keyword.as_deref(),
                    &settings,
                )
                .await
            }
            MusicCallbackAction::Setting(action) => {
                handle_setting_callback(
                    &bot,
                    &callback_id,
                    &msg,
                    user_id,
                    action,
                    MusicPanelView::Settings,
                )
                .await
            }
            MusicCallbackAction::Panel { action, .. } => {
                handle_music_panel_callback(&bot, &callback_id, &msg, user_id, action).await
            }
            MusicCallbackAction::Close => {
                let _ = bot
                    .send(DeleteMessage::new(msg.chat().id(), msg.message_id()))
                    .await;
                bot.send(AnswerCallbackQuery::new(callback_id.clone()))
                    .await?;
                Ok(())
            }
            // 已在前面提前返回，此处不可达
            MusicCallbackAction::QueueStatus => Ok(()),
            MusicCallbackAction::InlineSend { .. } => Ok(()),
            MusicCallbackAction::InlineCollapse(platform, id) => {
                let settings = get_user_settings_or_default(user_id).await;
                handle_song_collapse_callback(
                    &bot,
                    &callback_id,
                    SongMessage::Chat(&msg),
                    platform,
                    &id,
                    &settings,
                )
                .await
            }
        };
        if let Err(e) = result {
            // 限流/排队等瞬时拒绝（Busy）只弹提示，不能抹掉消息里已有的菜单
            if !preserve_panel && !matches!(e, BotError::Busy(_)) && is_page_message(&msg) {
                let _ = edit_callback_status(&bot, &msg, format!("{e}")).await;
            }
            let _ = bot
                .send(
                    AnswerCallbackQuery::new(callback_id)
                        .text(format!("{e}"))
                        .show_alert(true),
                )
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
    track_menu_for_context(track, fallback_query, is_group_chat(msg), msg.chat().id())
}

fn inline_track_menu(track: &MusicTrack) -> Option<InlineKeyboardMarkup> {
    inline_track_menu_with_lyric_label(track, "歌词", None)
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
        utility_row.push(url_button("获取封面", url));
    }
    if !track.song.trim().is_empty() {
        utility_row.push(switch_inline_current_chat_button(
            "搜索更多",
            format!("{} {}", track.song, track.platform.id()),
        ));
    }

    let mut action_row = Vec::new();
    // Bilibili/酷狗没有歌词接口，不渲染必然弹“没有可用歌词”的死按钮。
    // 点开后这条 inline 消息原地变成带播放器的富文本歌词页
    if track.platform.supports_lyrics()
        && !track.id.trim().is_empty()
        && let Some(callback) =
            MusicCallbackAction::Lyrics(track.platform, track.id.clone(), lyric_target).encode()
    {
        action_row.push(callback_button(lyric_label, callback));
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
        action_row.push(callback_button("收藏", callback));
    }
    let rows = [utility_row, action_row]
        .into_iter()
        .filter(|row| !row.is_empty())
        .collect::<Vec<_>>();
    (!rows.is_empty()).then(|| InlineKeyboardMarkup::new(rows))
}

fn parse_lyric_start_parameter(parameter: &str) -> Option<(MusicPlatform, &str)> {
    let (code, id) = parameter
        .strip_prefix(LYRIC_START_PREFIX)?
        .split_once('-')?;
    Some((MusicPlatform::from_callback_code(code)?, id)).filter(|(_, id)| !id.is_empty())
}

/// Answers the deep link of a full lyric page with the page.
pub async fn send_lyric_start(bot: &Bot, msg: &Message, parameter: &str) -> BotResult {
    let Some((platform, id)) = parse_lyric_start_parameter(parameter) else {
        return Ok(());
    };
    let user_id = user_id_from_message(msg);
    let Some(lyrics) = fetch_lyrics_limited(platform, id, user_id, Some(msg.chat().id())).await?
    else {
        bot.send(
            SendMessage::new(
                msg.chat().id(),
                format!("{} 没有可用歌词", platform.label()),
            )
            .reply_parameters(reply_to(msg.message_id())),
        )
        .await?;
        return Ok(());
    };
    let settings = get_user_settings_or_default(user_id).await;
    let script = MusicLyricScript::from_stored_value(&settings.lyric_script);
    let song = LyricSong::resolve(platform, id, user_id, LyricSong::default()).await;
    send_lyric_page(
        bot,
        msg.chat().id(),
        Some(msg.message_id()),
        &song,
        platform,
        id,
        &lyrics,
        script,
    )
    .await
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
        utility_row.push(callback_button("获取封面", callback));
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
        utility_row.push(callback_button("搜索更多", callback));
    }

    let mut action_row = Vec::new();
    if !track.id.trim().is_empty() {
        // Bilibili/酷狗没有歌词接口，不渲染必然弹“没有可用歌词”的死按钮
        if track.platform.supports_lyrics()
            && let Some(callback) =
                MusicCallbackAction::Lyrics(track.platform, track.id.clone(), None).encode()
        {
            action_row.push(callback_button("歌词", callback));
        }
        if let Some(callback) = MusicCallbackAction::Favorite(MusicFavoriteAction::Toggle {
            scope: FavoriteScope::User,
            platform: track.platform,
            id: track.id.clone(),
            chat_id: None,
        })
        .encode()
        {
            action_row.push(callback_button("收藏", callback));
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
            action_row.push(callback_button("群收藏", callback));
        }
    }

    let rows = [utility_row, action_row]
        .into_iter()
        .filter(|row| !row.is_empty())
        .collect::<Vec<_>>();
    (!rows.is_empty()).then(|| InlineKeyboardMarkup::new(rows))
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
    show_typing(bot, msg.chat().id());

    let settings = get_user_settings_or_default(user_id_from_message(msg)).await;
    let language_tag = Some("zh-CN");
    let music =
        MusicCmd::parse_i18n_from_bot(getor(msg).unwrap().split_whitespace(), language_tag)?;
    let action = MusicAction::from_cmd(music, &settings)?;

    match action {
        MusicAction::Platforms => {
            send_music_panel(bot, msg, &settings, MusicPanelView::Platforms).await
        }
        MusicAction::Setting => {
            send_music_panel(bot, msg, &settings, MusicPanelView::Settings).await
        }
        MusicAction::Queue => send_queue_status(bot, msg).await,
        MusicAction::Favorite(scope) => {
            send_music_favorites(bot, msg, user_id_from_message(msg), scope).await
        }
        MusicAction::Collection(query) => send_music_collection(bot, msg, query).await,
        MusicAction::Search(query) => send_music_search(bot, msg, query, &settings).await,
        MusicAction::Get(query) => send_direct_music(bot, msg, query, &settings).await,
    }
}

/// 频道歌曲被自动转发进讨论群（评论区）时，Telegram 会剥离原消息的内联按钮；
/// 这里用音频的 file_id 反查 music_cache 认领自家歌曲，并在评论线程补发一份操作按钮。
/// 任何未命中或缓存层故障都静默返回 Ok——这只是补发按钮的增强功能。
/// 注意：Telegram 只对 file_unique_id 承诺稳定，file_id 理论上可能变化；上游同样按
/// file_id 匹配且实践可靠，若出现漏配可再给 music_cache 增加 file_unique_id 列。
pub async fn comment_buttons(bot: &Bot, msg: &Message) -> BotResult {
    if msg.is_automatic_forward() != Some(true) || !is_group_chat(msg) {
        return Ok(());
    }
    // 频道帖被编辑时讨论组副本会以 edited_message 再次进入本管线，
    // 不加此守卫每次编辑都会重复补发一条按钮消息
    if msg.edit_date().is_some() {
        return Ok(());
    }
    // 歌曲以卡片或旧式音频消息发出，两种都能认出
    let Some(file_id) =
        card_audio_file_id(msg).or_else(|| msg.audio().map(|audio| &*audio.file_id))
    else {
        return Ok(());
    };
    let lookup = match provider::gateway() {
        Ok(gateway) => gateway.lookup_cache(file_id).await.map_err(BotError::from),
        Err(err) => Err(err),
    };
    let cached = match lookup {
        Ok(Some(cached)) => cached,
        Ok(None) => return Ok(()),
        Err(err) => {
            log::warn!("music cache lookup by file_id failed: {err}");
            return Ok(());
        }
    };
    if is_suspicious_media(&cached.track, cached.byte_size) {
        return Ok(());
    }
    let track = cached.track;
    // 缓存行不带封面，获取封面按钮会被 track_menu_for_context 自然省略
    let Some(keyboard) = track_menu_for_context(&track, None, true, msg.chat().id()) else {
        return Ok(());
    };
    if let Err(err) = bot
        .send(
            SendMessage::new(msg.chat().id(), "🎵 这首歌的操作")
                .reply_parameters(reply_to(msg.message_id()))
                .reply_markup(keyboard),
        )
        .await
    {
        log::warn!("failed to repost song buttons in comment thread: {err}");
    }
    Ok(())
}

enum MusicAction {
    Get(MusicQuery),
    Search(MusicQuery),
    Platforms,
    Setting,
    Favorite(FavoriteScope),
    Collection(MusicQuery),
    Queue,
}

impl MusicAction {
    fn from_cmd(cmd: MusicCmd, settings: &UserMusicSettings) -> Result<Self, BotError> {
        let default_platform = settings_platform(settings);
        match cmd.command {
            Some(MusicSubcommand::Search(search)) => Ok(Self::Search(
                provider::build_query_with_platform(default_platform, &search.query)?,
            )),
            Some(MusicSubcommand::Get(get)) => Ok(Self::Get(provider::build_query_with_platform(
                default_platform,
                &get.query,
            )?)),
            Some(MusicSubcommand::Setting) => Ok(Self::Setting),
            Some(MusicSubcommand::Queue) => Ok(Self::Queue),
            Some(MusicSubcommand::Favorite(favorite)) => Ok(Self::Favorite(favorite.scope.into())),
            Some(MusicSubcommand::Collection(collection)) => Ok(Self::Collection(
                provider::build_query_with_platform(default_platform, &collection.query)?,
            )),
            None if cmd.query.is_empty() => Ok(Self::Platforms),
            None => Ok(Self::Search(provider::build_query_with_platform(
                default_platform,
                &cmd.query,
            )?)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    fn test_settings(default_platform: &str) -> UserMusicSettings {
        UserMusicSettings {
            user_id: 1,
            default_platform: default_platform.to_string(),
            quality: "lossless".to_string(),
            lyric_script: "simplified".to_string(),
        }
    }

    #[test]
    fn inline_help_is_one_line_naming_the_bot() {
        let text = inline_help_text("allen_bot");
        assert_eq!(text, "🎵 在输入框里 @allen_bot 后面输入歌名就能搜索音乐");
        assert!(!text.contains('\n'));
    }

    #[test]
    fn bare_music_opens_platform_panel() {
        let cmd = MusicCmd::parse_i18n_from_bot(["/music"], Some("zh-CN")).unwrap();
        assert!(matches!(
            MusicAction::from_cmd(cmd, &test_settings("applemusic")).unwrap(),
            MusicAction::Platforms
        ));
    }

    #[test]
    fn platform_names_in_song_queries_do_not_override_default() {
        for keyword in ["qq", "网易云", "ytm", "#qq"] {
            let cmd =
                MusicCmd::parse_i18n_from_bot(["/music", keyword, "晴天"], Some("zh-CN")).unwrap();
            let MusicAction::Search(query) =
                MusicAction::from_cmd(cmd, &test_settings("applemusic")).unwrap()
            else {
                panic!("expected search action");
            };
            assert_eq!(query.platform, MusicPlatform::AppleMusic);
            assert_eq!(query.keyword, format!("{keyword} 晴天"));
        }
    }

    #[test]
    fn music_commands_reject_platform_flags() {
        for command in [None, Some("get"), Some("search"), Some("collection")] {
            for flag in ["-p", "--platform"] {
                let mut args = vec!["/music"];
                args.extend(command);
                args.extend([flag, "netease", "晴天"]);
                assert!(MusicCmd::parse_i18n_from_bot(args, Some("zh-CN")).is_err());
            }
        }
    }

    #[test]
    fn music_commands_use_saved_default_platform() {
        for command in [None, Some("get"), Some("search"), Some("collection")] {
            let mut args = vec!["/music"];
            args.extend(command);
            args.push("稻香");
            let cmd = MusicCmd::parse_i18n_from_bot(args, Some("zh-CN")).unwrap();
            let action = MusicAction::from_cmd(cmd, &test_settings("netease")).unwrap();
            let query = match (command, action) {
                (Some("get"), MusicAction::Get(query))
                | (None | Some("search"), MusicAction::Search(query))
                | (Some("collection"), MusicAction::Collection(query)) => query,
                _ => panic!("unexpected music action"),
            };
            assert_eq!(query.platform, MusicPlatform::Netease);
            assert_eq!(query.keyword, "稻香");
        }
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
    fn parses_added_provider_aliases() {
        for (alias, platform) in [
            ("kugou", MusicPlatform::Kugou),
            ("bilibili", MusicPlatform::Bilibili),
            ("汽水", MusicPlatform::Soda),
            ("applemusic", MusicPlatform::AppleMusic),
            ("网易云", MusicPlatform::Netease),
            ("ytm", MusicPlatform::YouTubeMusic),
        ] {
            assert_eq!(MusicPlatform::from_alias(alias), Some(platform));
            assert_eq!(
                MusicPlatform::from_callback_code(platform.callback_code()),
                Some(platform)
            );
        }
    }

    #[test]
    fn panel_buttons_are_owned_self_describing_and_within_telegram_limit() {
        let mut settings = test_settings("applemusic");
        settings.user_id = i64::MAX;
        for view in [MusicPanelView::Platforms, MusicPanelView::Settings] {
            let menu = music_panel_menu(&settings, view);
            for button in menu.inline_keyboard.iter().flatten() {
                let Some(payload) = button.callback_data.as_deref() else {
                    panic!("expected callback button");
                };
                assert!(payload.len() <= CALLBACK_LIMIT);
                let decoded = MusicCallbackAction::decode(payload).unwrap();
                let MusicCallbackAction::Panel { requester_id, .. } = &decoded else {
                    panic!("all panel buttons must carry their owner");
                };
                assert_eq!(*requester_id, settings.user_id);
                assert_eq!(decoded.encode().as_deref(), Some(payload));
            }
            let footer = menu.inline_keyboard.last().unwrap();
            assert_eq!(footer.len(), 2);
            assert_eq!(&*footer[1].text, "关闭");
        }
        let home = music_panel_menu(&settings, MusicPanelView::Platforms);
        assert_eq!(
            &*home.inline_keyboard[0][0].text,
            selected_label("Apple Music", true)
        );
        assert_eq!(
            home.inline_keyboard.iter().flatten().count(),
            MUSIC_PLATFORM_OPTIONS.len() + 2
        );
        assert!(home.inline_keyboard.iter().all(|row| row.len() <= 2));
    }

    #[test]
    fn panel_platform_selection_changes_only_saved_default() {
        let mut settings = test_settings("applemusic");
        let mut expected = settings.clone();
        expected.default_platform = "netease".to_string();
        let MusicCallbackAction::Panel {
            requester_id,
            action: MusicPanelAction::Platform(platform),
        } = MusicCallbackAction::decode("panel 1 platform n").unwrap()
        else {
            panic!("expected platform selection");
        };
        assert_eq!(requester_id, settings.user_id);
        let (changed, _) = apply_music_setting(
            &mut settings,
            MusicSettingAction::Platform(platform.id().to_string()),
        )
        .unwrap();
        assert!(changed);
        assert_eq!(settings, expected);
        assert_eq!(
            &*music_panel_menu(&settings, MusicPanelView::Platforms).inline_keyboard[0][0].text,
            selected_label("网易云音乐", true),
        );
    }

    #[test]
    fn panel_callbacks_reject_invalid_owners_and_actions() {
        for payload in [
            "panel 0 home",
            "panel -1 close",
            "panel abc settings",
            "panel 1",
            "panel 1 platform z",
            "panel 1 platform a extra",
            "panel 1 setting cover maybe",
            "panel 1 close extra",
        ] {
            assert!(MusicCallbackAction::decode(payload).is_err(), "{payload}");
        }
        assert!(
            MusicCallbackAction::Panel {
                requester_id: 0,
                action: MusicPanelAction::Show(MusicPanelView::Platforms),
            }
            .encode()
            .is_none()
        );
    }

    #[tokio::test]
    async fn other_users_cannot_modify_or_close_music_panel() {
        let mut payloads = [
            MusicPanelAction::Show(MusicPanelView::Platforms),
            MusicPanelAction::Show(MusicPanelView::Settings),
            MusicPanelAction::Platform(MusicPlatform::Netease),
            MusicPanelAction::Setting(MusicSettingAction::Quality("high".to_string())),
            MusicPanelAction::Close,
        ]
        .into_iter()
        .map(|action| {
            MusicCallbackAction::Panel {
                requester_id: 1,
                action,
            }
            .encode()
            .unwrap()
        })
        .collect::<Vec<_>>();
        payloads.extend(
            [
                "music panel 2 close",
                "music panel 2 platform n",
                "music setting platform netease",
                "music close",
            ]
            .map(str::to_string),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let count = payloads.len();
        let server = tokio::spawn(async move {
            for _ in 0..count {
                let (stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                assert!(line.to_ascii_lowercase().contains("/answercallbackquery "));
                let mut size = None;
                loop {
                    line.clear();
                    assert!(reader.read_line(&mut line).await.unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        size = Some(value.trim().parse::<usize>().unwrap());
                    }
                }
                let mut body = vec![0; size.unwrap()];
                reader.read_exact(&mut body).await.unwrap();
                // telers posts every request as a multipart form
                let body = String::from_utf8(body).unwrap();
                let field = |name: &str| {
                    let start = body.find(&format!("name=\"{name}\"")).unwrap();
                    let value = &body[start..];
                    let value = &value[value.find("\r\n\r\n").unwrap() + 4..];
                    value[..value.find("\r\n").unwrap()].to_string()
                };
                assert_eq!(field("show_alert"), "true");
                assert!(field("text").contains("其他用户"));
                let response = r#"{"ok":true,"result":true}"#;
                reader.get_mut().write_all(format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                    response.len(),
                ).as_bytes()).await.unwrap();
            }
        });
        let api = telers::client::telegram::APIServer::new(
            &format!("http://{addr}/bot{{token}}/{{method_name}}"),
            &format!("http://{addr}/file/bot{{token}}/{{path}}"),
            false,
            telers::client::telegram::BareFilesPathWrapper,
        );
        let bot = Bot::with_client(
            "1:test",
            telers::client::Reqwest::new(reqwest::Client::builder().no_proxy().build().unwrap())
                .with_api_server(std::borrow::Cow::Owned(api)),
        );
        for payload in payloads {
            let callback: CallbackQuery = serde_json::from_value(serde_json::json!({
                "id": "panel-click",
                "from": { "id": 2, "is_bot": false, "first_name": "Other user" },
                "chat_instance": "test-chat",
                "data": payload,
                "message": {
                    "message_id": 123,
                    "date": 1700000000,
                    "chat": { "id": -100123, "type": "supergroup", "title": "Test group" },
                    "from": { "id": 99, "is_bot": true, "first_name": "Test bot" },
                    "text": "音乐",
                    "reply_markup": music_panel_menu(&test_settings("applemusic"), MusicPanelView::Platforms),
                },
            }))
            .unwrap();
            tokio::time::timeout(
                Duration::from_secs(5),
                music_callback(bot.clone(), callback),
            )
            .await
            .unwrap()
            .unwrap();
        }
        server.await.unwrap();
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
            ..Default::default()
        };
        let result = inline_lyric_result(
            &item,
            &LyricView::new(
                &LyricSong::from_search_item(&item),
                item.platform,
                &lyrics,
                MusicLyricScript::Simplified,
            ),
            &Cover::None,
            MusicLyricScript::Simplified,
        );
        let InlineQueryResult::Article(article) = result else {
            panic!("expected article result");
        };
        let keyboard = article.reply_markup.unwrap().inline_keyboard;
        assert_eq!(&*keyboard[0][0].text, "切换繁体");
        let Some(callback) = keyboard[0][0].callback_data.as_deref() else {
            panic!("expected callback button");
        };
        assert_eq!(callback, "music ilyric a 1624001324 t");
        let telers::types::InputMessageContent::InputRichMessageContent(content) =
            article.input_message_content
        else {
            panic!("expected rich content");
        };
        let html = content.rich_message.html.unwrap();
        assert!(html.starts_with(&format!("<h3>{}</h3>", item.song)));
        assert!(html.contains("<blockquote expandable>"));
        assert_eq!(content.rich_message.skip_entity_detection, Some(true));
    }

    #[test]
    fn apple_music_preview_sized_cache_is_rejected() {
        let track = MusicTrack {
            platform: MusicPlatform::AppleMusic,
            id: "966805806".to_string(),
            song: "匆匆那年".to_string(),
            singer: "王菲".to_string(),
            album: "匆匆那年 - Single".to_string(),
            cover: String::new(),
            link: "https://music.apple.com/cn/album/966805714?i=966805806".to_string(),
            duration: Some(241),
            bitrate: None,
            format: Some("m4a".to_string()),
        };
        assert!(is_suspicious_media(&track, 780 * 1024));
        assert!(!is_suspicious_media(&track, 7_861_599));
        let other = MusicTrack {
            platform: MusicPlatform::Netease,
            ..track
        };
        assert!(!is_suspicious_media(&other, 780 * 1024));
    }
    fn test_media(track: MusicTrack, decrypt_elapsed_ms: Option<u64>) -> MusicMedia {
        MusicMedia {
            track,
            quality: provider::Quality::Lossless,
            file_id: "file-id".to_string(),
            thumb_file_id: None,
            byte_size: 1024 * 1024,
            decrypt_elapsed_ms,
            cached: false,
        }
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
            duration: Some(2),
            bitrate: None,
            format: Some("m4a".to_string()),
        };
        let media = test_media(track, None);
        assert_eq!(
            build_music_caption(&media),
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
            duration: Some(2),
            bitrate: None,
            format: Some("m4a".to_string()),
        };
        let media = test_media(track, Some(7850));
        assert_eq!(
            build_music_caption(&media),
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
            .find_map(|button| {
                (&*button.text == "搜索更多")
                    .then_some(button.callback_data.as_deref())
                    .flatten()
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
            duration: Some(223),
            bitrate: None,
            format: Some("m4a".to_string()),
        };
        let markup = track_menu_for_context(&track, None, true, -100123).unwrap();
        assert_eq!(markup.inline_keyboard.len(), 2);
        assert_eq!(&*markup.inline_keyboard[0][0].text, "获取封面");
        assert_eq!(&*markup.inline_keyboard[0][1].text, "搜索更多");
        let action_labels = markup.inline_keyboard[1]
            .iter()
            .map(|button| &*button.text)
            .collect::<Vec<_>>();
        assert_eq!(action_labels, vec!["歌词", "收藏", "群收藏"]);
    }

    fn button_labels(markup: &InlineKeyboardMarkup) -> Vec<&str> {
        markup
            .inline_keyboard
            .iter()
            .flatten()
            .map(|button| &*button.text)
            .collect()
    }

    #[test]
    fn lyricless_platform_menus_omit_lyric_button() {
        let track = MusicTrack {
            id: "BV1BZbSzZEGT".to_string(),
            platform: MusicPlatform::Bilibili,
            song: "晴天".to_string(),
            singer: "周杰伦".to_string(),
            album: String::new(),
            cover: "https://example.com/cover.jpg".to_string(),
            link: "https://www.bilibili.com/video/BV1BZbSzZEGT".to_string(),
            duration: Some(223),
            bitrate: None,
            format: Some("m4a".to_string()),
        };
        let markup = track_menu_for_context(&track, None, true, -100123).unwrap();
        let labels = button_labels(&markup);
        assert!(!labels.contains(&"歌词"));
        assert!(labels.contains(&"收藏"));
        assert!(labels.contains(&"群收藏"));

        let markup = inline_track_menu(&track).unwrap();
        let labels = button_labels(&markup);
        assert!(!labels.contains(&"歌词"));
        assert!(labels.contains(&"收藏"));

        let mut kugou_track = track.clone();
        kugou_track.platform = MusicPlatform::Kugou;
        kugou_track.id = "ABCDEF0123456789ABCDEF0123456789".to_string();
        let markup = track_menu_for_context(&kugou_track, None, false, 0).unwrap();
        assert!(!button_labels(&markup).contains(&"歌词"));
    }

    #[test]
    fn remove_lyric_buttons_strips_only_lyric_buttons() {
        let lyric = callback_button(
            "歌词",
            MusicCallbackAction::Lyrics(MusicPlatform::Tencent, "12345".to_string(), None)
                .encode()
                .unwrap(),
        );
        let inline_lyric = callback_button(
            "切换繁体",
            MusicCallbackAction::InlineLyrics(
                MusicPlatform::Tencent,
                "12345".to_string(),
                MusicLyricScript::Traditional,
            )
            .encode()
            .unwrap(),
        );
        let favorite = callback_button(
            "收藏",
            MusicCallbackAction::Favorite(MusicFavoriteAction::Toggle {
                scope: FavoriteScope::User,
                platform: MusicPlatform::Tencent,
                id: "12345".to_string(),
                chat_id: None,
            })
            .encode()
            .unwrap(),
        );
        let markup = InlineKeyboardMarkup::new([vec![lyric, favorite.clone()], vec![inline_lyric]]);
        let filtered = remove_lyric_buttons(&markup).unwrap();
        // 歌词按钮所在行剩收藏，纯歌词行整行消失
        assert_eq!(filtered.inline_keyboard.len(), 1);
        assert_eq!(filtered.inline_keyboard[0].len(), 1);
        assert_eq!(&*filtered.inline_keyboard[0][0].text, "收藏");

        // 没有歌词按钮时返回 None，表示无需编辑
        let markup = InlineKeyboardMarkup::new([vec![favorite]]);
        assert!(remove_lyric_buttons(&markup).is_none());
    }

    #[test]
    fn queue_status_callback_is_stateless() {
        let payload = MusicCallbackAction::QueueStatus.encode().unwrap();
        assert_eq!(payload, "music queue");
        assert!(matches!(
            MusicCallbackAction::decode(&payload),
            Ok(MusicCallbackAction::QueueStatus)
        ));
        assert!(matches!(
            MusicCallbackAction::decode("queue"),
            Ok(MusicCallbackAction::QueueStatus)
        ));
    }

    #[test]
    fn parses_queue_subcommand() {
        let cmd = MusicCmd::parse_i18n_from_bot(["/music", "queue"], Some("zh-CN")).unwrap();
        let MusicAction::Queue = MusicAction::from_cmd(cmd, &test_settings("tencent")).unwrap()
        else {
            panic!("expected queue action");
        };
    }

    #[test]
    fn queue_status_text_reports_limits() {
        let snapshot = queue::QueueSnapshot {
            running: 2,
            waiting: 3,
            active: 6,
            concurrency: 4,
            wait_limit: 20,
            global_limit: 24,
        };
        assert_eq!(
            queue_status_text(&snapshot),
            "📥 下载队列\n下载中：2/4\n排队中：3/20\n进行中任务：6/24"
        );
        let unlimited = queue::QueueSnapshot {
            wait_limit: 0,
            global_limit: 0,
            ..snapshot
        };
        assert_eq!(
            queue_status_text(&unlimited),
            "📥 下载队列\n下载中：2/4\n排队中：3\n进行中任务：6"
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
        assert_eq!(&*article.title, "稻香 - 周杰伦");
        assert_eq!(
            &*article.thumbnail_url.unwrap(),
            "https://example.com/cover.jpg"
        );
        let keyboard = article.reply_markup.unwrap().inline_keyboard;
        assert_eq!(&*keyboard[0][0].text, "没反应？点此刷新");
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
        assert_eq!(platform, MusicPlatform::AppleMusic);
        assert_eq!(quality, "lossless");
    }

    #[test]
    fn inline_lyric_query_prefix_is_detected() {
        assert_eq!(parse_inline_lyric_query("歌词 稻香 qq"), Some("稻香 qq"));
        assert_eq!(parse_inline_lyric_query("lyric 稻香"), Some("稻香"));
        assert_eq!(parse_inline_lyric_query("稻香"), None);
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
            duration: Some(223),
            bitrate: None,
            format: Some("m4a".to_string()),
        };
        let markup = inline_track_menu(&track).unwrap();
        assert_eq!(markup.inline_keyboard.len(), 2);
        assert_eq!(&*markup.inline_keyboard[0][0].text, "获取封面");
        assert_eq!(&*markup.inline_keyboard[0][1].text, "搜索更多");
        assert_eq!(&*markup.inline_keyboard[1][0].text, "歌词");
        let Some(callback) = markup.inline_keyboard[1][0].callback_data.as_deref() else {
            panic!("expected lyric callback");
        };
        assert_eq!(callback, "music lyric a 1624001324");
        assert_eq!(&*markup.inline_keyboard[1][1].text, "收藏");
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
            "⬇️ 正在下载「晴天」\n已下载 0.50 MB"
        );
        assert_eq!(
            format_download_progress(
                "晴天",
                DownloadProgress {
                    written: 1024 * 1024,
                    total: Some(4 * 1024 * 1024),
                }
            ),
            "⬇️ 正在下载「晴天」\n▰▰▰▱▱▱▱▱▱▱ 25.0%\n1.00 MB / 4.00 MB"
        );
    }

    #[test]
    fn lyric_deep_links_carry_platform_and_id() {
        assert_eq!(
            parse_lyric_start_parameter("lyric-n-1893321422"),
            Some((MusicPlatform::Netease, "1893321422"))
        );
        assert_eq!(
            parse_lyric_start_parameter("lyric-y-dQw4w9-WgXcQ"),
            Some((MusicPlatform::YouTubeMusic, "dQw4w9-WgXcQ"))
        );
        assert!(parse_lyric_start_parameter("lyric-n-").is_none());
    }

    #[test]
    fn song_cards_are_recognised_by_their_player() {
        let card: Message = serde_json::from_value(serde_json::json!({
            "message_id": 9,
            "date": 1700000000,
            "chat": { "id": 1, "type": "private", "first_name": "u" },
            "rich_message": { "blocks": [
                { "type": "heading", "size": 3, "text": "晴天" },
                { "type": "audio", "audio": { "file_id": "CQAC", "file_unique_id": "u", "duration": 269 } },
            ] }
        }))
        .unwrap();
        assert_eq!(card_audio_file_id(&card), Some("CQAC"));
        let page: Message = serde_json::from_value(serde_json::json!({
            "message_id": 10,
            "date": 1700000000,
            "chat": { "id": 1, "type": "private", "first_name": "u" },
            "rich_message": { "blocks": [{ "type": "heading", "size": 3, "text": "歌词" }] }
        }))
        .unwrap();
        assert_eq!(card_audio_file_id(&page), None);
    }

    fn search_item(index: usize) -> MusicSearchItem {
        MusicSearchItem {
            platform: MusicPlatform::Tencent,
            id: format!("00{index}"),
            song: format!("歌<{index}>"),
            singer: "周杰伦".to_string(),
            cover: String::new(),
        }
    }

    fn callbacks(markup: &InlineKeyboardMarkup) -> Vec<Vec<(String, String)>> {
        markup
            .inline_keyboard
            .iter()
            .map(|row| {
                row.iter()
                    .map(|button| {
                        let Some(data) = button.callback_data.as_deref() else {
                            panic!("expected callback button");
                        };
                        (button.text.to_string(), data.to_string())
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn search_page_callbacks_round_trip() {
        let payload = MusicCallbackAction::SearchPage {
            platform: MusicPlatform::Netease,
            page: 3,
            requester_id: 42,
        }
        .encode()
        .unwrap();
        let MusicCallbackAction::SearchPage {
            platform,
            page,
            requester_id,
        } = MusicCallbackAction::decode(&payload).unwrap()
        else {
            panic!("expected search page");
        };
        assert_eq!(
            (platform, page, requester_id),
            (MusicPlatform::Netease, 3, 42)
        );

        let payload = MusicCallbackAction::SearchClose { requester_id: 42 }
            .encode()
            .unwrap();
        assert!(matches!(
            MusicCallbackAction::decode(&payload).unwrap(),
            MusicCallbackAction::SearchClose { requester_id: 42 }
        ));
        for invalid in [
            "music sp n 0 42",
            "music sp n x 42",
            "music sp n 1",
            "music sp zz 1 42",
        ] {
            assert!(MusicCallbackAction::decode(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn search_page_lists_numbered_results_with_platform_switch() {
        let query = MusicQuery {
            platform: MusicPlatform::Tencent,
            keyword: "晴天 & 稻香".to_string(),
        };
        let results = SearchResults {
            tracks: (1..=10).map(search_item).collect(),
            complete: true,
        };
        let page = search_page(&query, &results, 2);
        assert_eq!(
            page.html(),
            "<h3>🔍 晴天 &amp; 稻香</h3><p>QQ音乐 · 第 2/2 页</p>\
             <ol><li><b>歌&lt;9&gt;</b> — 周杰伦</li><li><b>歌&lt;10&gt;</b> — 周杰伦</li></ol>\
             <footer>点击下方序号获取歌曲</footer>"
        );

        let rows = callbacks(&search_page_menu(&query, Some(&results), 2, 42));
        assert_eq!(
            rows[0],
            [
                (
                    "1".to_string(),
                    "music d t 009 %E6%99%B4%E5%A4%A9%20%26%20%E7%A8%BB%E9%A6%99".to_string()
                ),
                (
                    "2".to_string(),
                    "music d t 0010 %E6%99%B4%E5%A4%A9%20%26%20%E7%A8%BB%E9%A6%99".to_string()
                )
            ]
        );
        assert_eq!(
            rows[1],
            [("⬅️ 上一页".to_string(), "music sp t 1 42".to_string())]
        );
        assert_eq!(
            rows[2][0],
            ("✅ QQ音乐".to_string(), "music sp t 1 42".to_string())
        );
        assert_eq!(
            rows[2][1],
            ("网易云".to_string(), "music sp n 1 42".to_string())
        );
        assert_eq!(rows[2].len() + rows[3].len(), SEARCH_PLATFORMS.len());
        assert_eq!(
            rows[4],
            [("❌ 关闭".to_string(), "music sc 42".to_string())]
        );
        for row in &rows {
            for (_, data) in row {
                assert!(data.len() <= CALLBACK_LIMIT);
            }
        }
    }

    #[test]
    fn unfinished_search_offers_next_page_without_total() {
        let query = MusicQuery {
            platform: MusicPlatform::Tencent,
            keyword: "晴天".to_string(),
        };
        let results = SearchResults {
            tracks: (1..=SEARCH_PAGE_SIZE).map(search_item).collect(),
            complete: false,
        };
        assert!(
            search_page(&query, &results, 1)
                .html()
                .contains("<p>QQ音乐 · 第 1 页</p>")
        );
        let rows = callbacks(&search_page_menu(&query, Some(&results), 1, 42));
        assert_eq!(rows[0].len(), 4);
        assert_eq!(rows[1].len(), 4);
        assert_eq!(
            rows[2],
            [("下一页 ➡️".to_string(), "music sp t 2 42".to_string())]
        );
    }

    #[test]
    fn empty_and_failed_searches_still_offer_platforms() {
        let query = MusicQuery {
            platform: MusicPlatform::Kugou,
            keyword: "晴天".to_string(),
        };
        let results = SearchResults {
            tracks: Vec::new(),
            complete: true,
        };
        assert!(
            search_page(&query, &results, 1)
                .html()
                .contains("<p>酷狗音乐 · 没有找到结果</p>")
        );
        let failure = search_failure_page(&query, &BotError::Custom("超时 <x>".to_string()));
        assert!(
            failure
                .html()
                .contains("<p>酷狗音乐 · 搜索失败</p><blockquote>超时 &lt;x&gt;</blockquote>")
        );
        for results in [Some(&results), None] {
            let rows = callbacks(&search_page_menu(&query, results, 1, 42));
            assert_eq!(rows.len(), 3);
            assert!(rows[1].iter().any(|(text, _)| text == "✅ 酷狗"));
        }
    }

    #[test]
    fn number_buttons_split_into_balanced_rows() {
        let lengths = |count: usize| {
            track_pick_rows(&(1..=count).map(search_item).collect::<Vec<_>>(), None)
                .iter()
                .map(Vec::len)
                .collect::<Vec<_>>()
        };
        assert_eq!(lengths(3), [3]);
        assert_eq!(lengths(5), [5]);
        assert_eq!(lengths(7), [4, 3]);
        assert_eq!(lengths(10), [5, 5]);
    }

    #[test]
    fn plain_keywords_search_and_links_fetch_directly() {
        for keyword in ["晴天 周杰伦", "1989", "七里香", "Re:Zero", "qq: 晴天"] {
            assert!(!is_direct_query(keyword), "{keyword}");
        }
        for keyword in [
            "https://y.qq.com/n/ryqq/songDetail/0039MnYb0qxYhV",
            "分享周杰伦的单曲《晴天》: https://y.qq.com/x",
            "1893321422",
            "netease:1893321422",
            "applemusic:1624001324",
        ] {
            assert!(is_direct_query(keyword), "{keyword}");
        }
    }
}
