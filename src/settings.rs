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
}

#[derive(Debug, Deserialize)]
pub struct Url {
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct Api {
    pub music: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct Music {
    #[serde(default)]
    pub applemusic: AppleMusic,
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

fn default_apple_storefront() -> String {
    "us".to_string()
}

fn default_apple_language() -> String {
    "en-US".to_string()
}

fn default_apple_timeout() -> u64 {
    30
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
    }
}
