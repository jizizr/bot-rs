use crate::settings::SETTINGS;
use bot_rs::{get, getor, BotError, BotResult};
use lazy_static::lazy_static;
use regex::Regex;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use serde::Deserialize;
use teloxide::{
    prelude::*,
    types::{ChatAction, Me, User},
    utils::{command::BotCommands, markdown},
};

pub mod command;
pub mod pkg;
pub mod text;

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

fn fmt_at(name: &str, user_id: u64) -> String {
    format!("[{}](tg://user?id={})", markdown::escape(name), user_id)
}
