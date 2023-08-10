use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::error::Error;
use teloxide::{prelude::*, types::ParseMode, utils::markdown};
use bot_rs::getor;
use lazy_static::lazy_static;
pub mod command;
pub mod text;

async fn get<T: DeserializeOwned>(url: &str) -> Result<T, reqwest::Error> {
    let resp = reqwest::get(url).await?;
    let model: T = resp.json().await?;
    Ok(model)
}
