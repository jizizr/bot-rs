use crate::settings::SETTINGS;
use lazy_static::lazy_static;
use serde::de::DeserializeOwned;
use std::{error::Error, fs::File, io::Read};
use teloxide::prelude::*;
use teloxide::Bot;
pub mod settings;

pub type BotError = Box<dyn Error + Send + Sync>;
pub type BotResult = Result<(), BotError>;

lazy_static! {
    pub static ref BOT: Bot = Bot::new(&SETTINGS.bot.token);
}

pub fn getor(msg: &Message) -> Option<&str> {
    msg.text().or(msg.caption())
}

pub fn load_json<T: DeserializeOwned>(path: &str) -> T {
    let mut file = File::open(path).unwrap_or_else(|_| panic!("找不到 {path}"));
    let mut json_data = String::new();
    file.read_to_string(&mut json_data)
        .unwrap_or_else(|_| panic!("读取 {path} 失败"));
    // 解析 JSON 文件
    serde_json::from_str(&json_data).expect("JSON 数据解析失败")
}

pub async fn get<T: DeserializeOwned>(url: &str) -> Result<T, reqwest::Error> {
    let resp = reqwest::get(url).await?;
    let model: T = resp.json().await?;
    Ok(model)
}
