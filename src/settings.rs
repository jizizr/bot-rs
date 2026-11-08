use serde::Deserialize;
use std::{collections::HashMap, fs::File, io::Read, sync::LazyLock};

pub static SETTINGS: LazyLock<Settings> = LazyLock::new(Settings::default);
#[derive(Debug, Deserialize)]
pub struct Settings {
    pub bot: Bot,
    pub url: Url,
    pub db: DB,
    pub gemini: Gemini,
    pub api: Api,
    #[serde(default)]
    pub music: Music,
    pub ping_server: HashMap<String, String>,
    pub vv: Vv,
}

#[derive(Debug, Deserialize)]
pub struct Bot {
    pub owner: i64,
    pub token: String,
    #[serde(default)]
    pub api_url: Option<String>,
    /// 本地模式（`--local`）的 Bot API 在本机的数据目录。它的 getFile 返回
    /// 服务端的绝对路径，不提供下载接口；放在容器里时路径和本机不同，
    /// 按这个目录换算到本机再读
    #[serde(default)]
    pub files_dir: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Url {
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct Api {
    pub music: String,
}

#[derive(Debug, Deserialize)]
pub struct Music {
    /// MHub's music gateway, which owns every platform account, download and
    /// the shared Telegram cache. The bot keeps no music credentials.
    #[serde(default)]
    pub gateway: MusicGateway,
    /// Telegram chat MHub stores its uploads in (shared `[music]` key);
    /// lyric covers are uploaded there once so every page can reuse them.
    /// 0 keeps them in the owner's private chat, as MHub does.
    #[serde(default)]
    pub inline_upload_chat_id: i64,
    /// MHub 同名配置：上传 bot 往这个私有频道发文件，主 bot 转发那条消息
    /// 拿到自己的 file_id。上传 bot 的 DC 离服务器更近，大文件快得多。
    /// 0 表示不中转，主 bot 直接上传
    #[serde(default)]
    pub upload_channel_id: i64,
    /// 上传 bot 的 token 文件，`/uploader` 写的就是它
    #[serde(default = "default_upload_bot_token_file")]
    pub upload_bot_token_file: String,
    // Download governor (funcs/command/music/queue.rs): concurrent download
    // slots, bounded waiting queue, and live-work admission caps.
    #[serde(default = "default_music_download_concurrency")]
    pub download_concurrency: usize,
    #[serde(default = "default_music_download_queue_wait_limit")]
    pub download_queue_wait_limit: usize,
    #[serde(default = "default_music_download_queue_global_limit")]
    pub download_queue_global_limit: usize,
    #[serde(default = "default_music_download_queue_per_user_limit")]
    pub download_queue_per_user_limit: usize,
    #[serde(default = "default_music_download_queue_per_chat_limit")]
    pub download_queue_per_chat_limit: usize,
    // Resource rate limits (funcs/command/music/ratelimit.rs): admitted
    // requests per rate_limit_window seconds, per dimension; 0 disables a
    // dimension.
    #[serde(default = "default_music_rate_limit_window")]
    pub rate_limit_window: u64,
    #[serde(default = "default_music_search_rate_per_user")]
    pub search_rate_per_user: usize,
    #[serde(default = "default_music_search_rate_per_chat")]
    pub search_rate_per_chat: usize,
    #[serde(default = "default_music_search_rate_per_platform")]
    pub search_rate_per_platform: usize,
    #[serde(default = "default_music_search_rate_global")]
    pub search_rate_global: usize,
    // inline 查询随输入逐字触发，配额独立于命令搜索且默认更宽松（无 chat 维度）
    #[serde(default = "default_music_inline_search_rate_per_user")]
    pub inline_search_rate_per_user: usize,
    #[serde(default = "default_music_inline_search_rate_per_platform")]
    pub inline_search_rate_per_platform: usize,
    #[serde(default = "default_music_inline_search_rate_global")]
    pub inline_search_rate_global: usize,
    #[serde(default = "default_music_lyric_rate_per_user")]
    pub lyric_rate_per_user: usize,
    #[serde(default = "default_music_lyric_rate_per_chat")]
    pub lyric_rate_per_chat: usize,
    #[serde(default = "default_music_lyric_rate_per_platform")]
    pub lyric_rate_per_platform: usize,
    #[serde(default = "default_music_lyric_rate_global")]
    pub lyric_rate_global: usize,
    #[serde(default = "default_music_download_rate_per_user")]
    pub download_rate_per_user: usize,
    #[serde(default = "default_music_download_rate_per_chat")]
    pub download_rate_per_chat: usize,
    #[serde(default = "default_music_download_rate_per_platform")]
    pub download_rate_per_platform: usize,
    #[serde(default = "default_music_download_rate_global")]
    pub download_rate_global: usize,
    #[serde(default = "default_music_collection_rate_per_user")]
    pub collection_rate_per_user: usize,
    #[serde(default = "default_music_collection_rate_per_chat")]
    pub collection_rate_per_chat: usize,
    #[serde(default = "default_music_collection_rate_per_platform")]
    pub collection_rate_per_platform: usize,
    #[serde(default = "default_music_collection_rate_global")]
    pub collection_rate_global: usize,
}

fn default_upload_bot_token_file() -> String {
    "/etc/music-gateway/uploader-token".to_string()
}

impl Default for Music {
    fn default() -> Self {
        Self {
            gateway: MusicGateway::default(),
            inline_upload_chat_id: 0,
            upload_channel_id: 0,
            upload_bot_token_file: default_upload_bot_token_file(),
            download_concurrency: default_music_download_concurrency(),
            download_queue_wait_limit: default_music_download_queue_wait_limit(),
            download_queue_global_limit: default_music_download_queue_global_limit(),
            download_queue_per_user_limit: default_music_download_queue_per_user_limit(),
            download_queue_per_chat_limit: default_music_download_queue_per_chat_limit(),
            rate_limit_window: default_music_rate_limit_window(),
            search_rate_per_user: default_music_search_rate_per_user(),
            search_rate_per_chat: default_music_search_rate_per_chat(),
            search_rate_per_platform: default_music_search_rate_per_platform(),
            search_rate_global: default_music_search_rate_global(),
            inline_search_rate_per_user: default_music_inline_search_rate_per_user(),
            inline_search_rate_per_platform: default_music_inline_search_rate_per_platform(),
            inline_search_rate_global: default_music_inline_search_rate_global(),
            lyric_rate_per_user: default_music_lyric_rate_per_user(),
            lyric_rate_per_chat: default_music_lyric_rate_per_chat(),
            lyric_rate_per_platform: default_music_lyric_rate_per_platform(),
            lyric_rate_global: default_music_lyric_rate_global(),
            download_rate_per_user: default_music_download_rate_per_user(),
            download_rate_per_chat: default_music_download_rate_per_chat(),
            download_rate_per_platform: default_music_download_rate_per_platform(),
            download_rate_global: default_music_download_rate_global(),
            collection_rate_per_user: default_music_collection_rate_per_user(),
            collection_rate_per_chat: default_music_collection_rate_per_chat(),
            collection_rate_per_platform: default_music_collection_rate_per_platform(),
            collection_rate_global: default_music_collection_rate_global(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct MusicGateway {
    /// Origin of MHub's `[gateway] listen`, e.g. `http://127.0.0.1:8091`.
    pub url: String,
    /// The same secret as MHub's gateway; `token_file` takes precedence.
    pub token: String,
    pub token_file: String,
    /// Upper bound for one song, download and Telegram upload included.
    pub media_timeout_seconds: u64,
}

impl Default for MusicGateway {
    fn default() -> Self {
        Self {
            url: "http://127.0.0.1:8091".into(),
            token: String::new(),
            token_file: String::new(),
            media_timeout_seconds: 15 * 60,
        }
    }
}

impl MusicGateway {
    pub fn resolve_token(&self) -> Result<String, String> {
        if self.token_file.trim().is_empty() {
            return Ok(self.token.trim().to_string());
        }
        std::fs::read_to_string(self.token_file.trim())
            .map(|token| token.trim().to_string())
            .map_err(|err| format!("无法读取 music.gateway.token_file：{err}"))
    }
}

#[derive(Debug, Deserialize)]
pub struct DB {
    pub mysql: Mysql,
    pub redis: Redis,
    pub mongo: Mongo,
}

#[derive(Debug, Deserialize)]
pub struct Mysql {
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct Redis {
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct Mongo {
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct Gemini {
    pub key: String,
}

#[derive(Debug, Deserialize)]
pub struct Vv {
    pub api_url: String,
    pub pic_url: String,
}

fn default_music_download_concurrency() -> usize {
    4
}

fn default_music_download_queue_wait_limit() -> usize {
    20
}

fn default_music_download_queue_global_limit() -> usize {
    24
}

fn default_music_download_queue_per_user_limit() -> usize {
    2
}

fn default_music_download_queue_per_chat_limit() -> usize {
    6
}

fn default_music_rate_limit_window() -> u64 {
    60
}

fn default_music_search_rate_per_user() -> usize {
    5
}

fn default_music_search_rate_per_chat() -> usize {
    12
}

fn default_music_search_rate_per_platform() -> usize {
    10
}

fn default_music_search_rate_global() -> usize {
    20
}

fn default_music_inline_search_rate_per_user() -> usize {
    20
}

fn default_music_inline_search_rate_per_platform() -> usize {
    40
}

fn default_music_inline_search_rate_global() -> usize {
    80
}

fn default_music_lyric_rate_per_user() -> usize {
    8
}

fn default_music_lyric_rate_per_chat() -> usize {
    20
}

fn default_music_lyric_rate_per_platform() -> usize {
    20
}

fn default_music_lyric_rate_global() -> usize {
    40
}

fn default_music_download_rate_per_user() -> usize {
    4
}

fn default_music_download_rate_per_chat() -> usize {
    8
}

fn default_music_download_rate_per_platform() -> usize {
    15
}

fn default_music_download_rate_global() -> usize {
    30
}

fn default_music_collection_rate_per_user() -> usize {
    5
}

fn default_music_collection_rate_per_chat() -> usize {
    10
}

fn default_music_collection_rate_per_platform() -> usize {
    12
}

fn default_music_collection_rate_global() -> usize {
    25
}

impl Default for Settings {
    fn default() -> Self {
        let file_path = "./data/conf.toml";
        let mut file = match File::open(file_path) {
            Ok(f) => f,
            Err(e) => panic!("no such file {file_path} exception:{e}"),
        };
        let mut str_val = String::new();
        match file.read_to_string(&mut str_val) {
            Ok(s) => s,
            Err(e) => panic!("Error Reading file: {e}"),
        };
        toml::from_str(&str_val).expect("Parsing the configuration file failed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apple_music_settings_have_safe_defaults() {
        let settings: Settings = toml::from_str(
            r#"
            [bot]
            owner = 1
            token = "token"

            [url]
            url = "https://example.com"

            [api]
            music = "https://example.com/music"

            [db.mysql]
            url = "mysql://localhost"

            [db.redis]
            url = "redis://localhost"

            [db.mongo]
            url = "mongodb://localhost"

            [gemini]
            key = ""

            [ping_server]

            [vv]
            api_url = ""
            pic_url = ""
            "#,
        )
        .unwrap();

        assert_eq!(settings.music.gateway.url, "http://127.0.0.1:8091");
        assert!(settings.music.gateway.token.is_empty());
        assert_eq!(settings.music.gateway.media_timeout_seconds, 900);
        assert!(settings.bot.api_url.is_none());
    }

    #[test]
    fn legacy_music_credentials_are_ignored_and_token_file_wins() {
        // Configs written before the gateway still parse; their platform
        // credentials now live only in MHub.
        let music: Music = toml::from_str(
            r#"
            inline_upload_chat_id = 1
            [netease]
            cookie = "old"
            [applemusic]
            wrapper_host = "127.0.0.1:10020"
            [gateway]
            url = "http://127.0.0.1:9000"
            token = "inline"
            "#,
        )
        .unwrap();
        assert_eq!(music.gateway.url, "http://127.0.0.1:9000");
        assert_eq!(music.gateway.resolve_token().unwrap(), "inline");

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("gateway.token");
        std::fs::write(&path, "from-file\n").unwrap();
        let gateway = MusicGateway {
            token_file: path.display().to_string(),
            ..music.gateway
        };
        assert_eq!(gateway.resolve_token().unwrap(), "from-file");
    }

    #[test]
    fn music_governor_and_rate_limit_defaults() {
        // A partial [music] section keeps every unspecified knob at its default.
        let settings: Settings = toml::from_str(
            r#"
            [bot]
            owner = 1
            token = "token"

            [url]
            url = "https://example.com"

            [api]
            music = "https://example.com/music"

            [db.mysql]
            url = "mysql://localhost"

            [db.redis]
            url = "redis://localhost"

            [db.mongo]
            url = "mongodb://localhost"

            [gemini]
            key = ""

            [ping_server]

            [vv]
            api_url = ""
            pic_url = ""

            [music]
            download_concurrency = 8
            "#,
        )
        .unwrap();

        assert_eq!(settings.music.download_concurrency, 8);
        assert_eq!(settings.music.download_queue_wait_limit, 20);
        assert_eq!(settings.music.download_queue_global_limit, 24);
        assert_eq!(settings.music.download_queue_per_user_limit, 2);
        assert_eq!(settings.music.download_queue_per_chat_limit, 6);
        assert_eq!(settings.music.rate_limit_window, 60);

        // Music::default() (used when [music] is entirely absent) must match
        // the serde defaults.
        let music = Music::default();
        assert_eq!(music.download_concurrency, 4);
        assert_eq!(music.download_queue_wait_limit, 20);
        assert_eq!(music.download_queue_global_limit, 24);
        assert_eq!(music.download_queue_per_user_limit, 2);
        assert_eq!(music.download_queue_per_chat_limit, 6);
        assert_eq!(music.rate_limit_window, 60);
        assert_eq!(
            (
                music.search_rate_per_user,
                music.search_rate_per_chat,
                music.search_rate_per_platform,
                music.search_rate_global
            ),
            (5, 12, 10, 20)
        );
        assert_eq!(
            (
                music.lyric_rate_per_user,
                music.lyric_rate_per_chat,
                music.lyric_rate_per_platform,
                music.lyric_rate_global
            ),
            (8, 20, 20, 40)
        );
        assert_eq!(
            (
                music.download_rate_per_user,
                music.download_rate_per_chat,
                music.download_rate_per_platform,
                music.download_rate_global
            ),
            (4, 8, 15, 30)
        );
        assert_eq!(
            (
                music.collection_rate_per_user,
                music.collection_rate_per_chat,
                music.collection_rate_per_platform,
                music.collection_rate_global
            ),
            (5, 10, 12, 25)
        );
    }
}
