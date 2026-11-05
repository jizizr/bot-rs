use crate::{
    analysis::model::{self, BotLogBuilder, MessageStatus},
    dao::mongo::analysis::insert_log,
    *,
};
use regex::Regex;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};
use serde::Deserialize;

pub mod command;
pub mod pkg;
pub mod text;

use pkg::escape::markdown;

/// 百分号编码 URL 组件：只保留 `A-Z a-z 0-9 - _ . ~`，与原 urlencoding 一致，
/// 已发出的回调按钮仍能按同样的规则解码。
const URL_COMPONENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

pub fn url_encode(text: &str) -> String {
    percent_encoding::utf8_percent_encode(text, URL_COMPONENT).to_string()
}

pub fn url_decode(text: &str) -> Result<std::borrow::Cow<'_, str>, std::str::Utf8Error> {
    percent_encoding::percent_decode_str(text).decode_utf8()
}

fn retry_client(client: reqwest::Client, times: u32) -> ClientWithMiddleware {
    let retry_policy = ExponentialBackoff::builder().build_with_max_retries(times);
    ClientBuilder::new(client)
        .with(RetryTransientMiddleware::new_with_policy(retry_policy))
        .build()
}

fn get_name(u: &User) -> String {
    let mut name = String::new();
    if !u.first_name.is_empty() {
        name.push_str(&u.first_name);
    }
    if let Some(last_name) = &u.last_name {
        name.push(' ');
        name.push_str(last_name);
    }
    name
}

fn fmt_at(name: &str, user_id: i64) -> String {
    format!("[{}](tg://user?id={})", markdown::escape(name), user_id)
}
