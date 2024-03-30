use crate::settings::SETTINGS;
use bot_rs::{get, getor, BotError, BotResult};
use lazy_static::lazy_static;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use serde::Deserialize;
use teloxide::{
    prelude::*,
    types::{ChatAction, Me},
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
