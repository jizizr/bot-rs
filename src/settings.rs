use lazy_static::lazy_static;
use serde::Deserialize;
use std::{collections::HashMap, fs::File, io::Read};

lazy_static! {
    pub static ref SETTINGS: Settings = Settings::default();
}
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
    #[serde(default)]
    pub applemusic: AppleMusic,
    #[serde(default)]
    pub vkeys: VkeysMusic,
    #[serde(default)]
    pub tencent: TencentMusic,
    #[serde(default)]
    pub netease: NeteaseMusic,
    #[serde(default)]
    pub inline_upload_chat_id: i64,
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

impl Default for Music {
    fn default() -> Self {
        Self {
            applemusic: AppleMusic::default(),
            vkeys: VkeysMusic::default(),
            tencent: TencentMusic::default(),
            netease: NeteaseMusic::default(),
            inline_upload_chat_id: 0,
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
pub struct VkeysMusic {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_vkeys_base_url")]
    pub base_url: String,
    #[serde(default)]
    pub token: String,
}

impl Default for VkeysMusic {
    fn default() -> Self {
        Self {
            enabled: true,
            base_url: default_vkeys_base_url(),
            token: String::new(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct TencentMusic {
    #[serde(default)]
    pub cookie: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct NeteaseMusic {
    #[serde(default)]
    pub cookie: String,
}

#[derive(Debug, Deserialize)]
pub struct AppleMusic {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub media_user_token: String,
    #[serde(default = "default_apple_storefront")]
    pub storefront: String,
    #[serde(default = "default_apple_language")]
    pub language: String,
    #[serde(default = "default_apple_timeout")]
    pub timeout: u64,
    #[serde(default)]
    pub wrapper_host: String,
    #[serde(default)]
    pub wrapper_hosts: Vec<String>,
    #[serde(default = "default_apple_wrapper_track_concurrency")]
    pub wrapper_track_concurrency: usize,
    #[serde(default = "default_apple_wrapper_fragment_concurrency")]
    pub wrapper_fragment_concurrency: usize,
    #[serde(default = "default_apple_wrapper_connection_concurrency")]
    pub wrapper_connection_concurrency: usize,
    #[serde(default)]
    pub wv_client_id: String,
    #[serde(default)]
    pub wv_private_key: String,
}

impl Default for AppleMusic {
    fn default() -> Self {
        Self {
            enabled: true,
            media_user_token: String::new(),
            storefront: default_apple_storefront(),
            language: default_apple_language(),
            timeout: default_apple_timeout(),
            wrapper_host: String::new(),
            wrapper_hosts: Vec::new(),
            wrapper_track_concurrency: default_apple_wrapper_track_concurrency(),
            wrapper_fragment_concurrency: default_apple_wrapper_fragment_concurrency(),
            wrapper_connection_concurrency: default_apple_wrapper_connection_concurrency(),
            wv_client_id: String::new(),
            wv_private_key: String::new(),
        }
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

fn default_true() -> bool {
    true
}

fn default_vkeys_base_url() -> String {
    "https://api.vkeys.cn".to_string()
}

fn default_apple_storefront() -> String {
    "us".to_string()
}

fn default_apple_language() -> String {
    "en-US".to_string()
}

fn default_apple_timeout() -> u64 {
    30
}

fn default_apple_wrapper_track_concurrency() -> usize {
    1
}

fn default_apple_wrapper_fragment_concurrency() -> usize {
    2
}

fn default_apple_wrapper_connection_concurrency() -> usize {
    2
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

        assert!(settings.music.applemusic.enabled);
        assert_eq!(settings.music.applemusic.storefront, "us");
        assert_eq!(settings.music.applemusic.language, "en-US");
        assert_eq!(settings.music.applemusic.timeout, 30);
        assert_eq!(settings.music.applemusic.wrapper_track_concurrency, 1);
        assert!(settings.music.applemusic.wrapper_hosts.is_empty());
        assert_eq!(settings.music.applemusic.wrapper_fragment_concurrency, 2);
        assert_eq!(settings.music.applemusic.wrapper_connection_concurrency, 2);
        assert!(settings.music.vkeys.enabled);
        assert_eq!(settings.music.vkeys.base_url, "https://api.vkeys.cn");
        assert!(settings.music.tencent.cookie.is_empty());
        assert!(settings.music.netease.cookie.is_empty());
        assert!(settings.bot.api_url.is_none());
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
