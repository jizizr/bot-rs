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
