use crate::*;
use futures::future::BoxFuture;
use lazy_static::lazy_static;
use regex::Regex;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};
use serde::Deserialize;
use std::{fmt, sync::Arc};
use teloxide::{
    error_handlers::ErrorHandler,
    types::{ChatAction, Me, ReplyParameters, User},
    utils::{command::BotCommands, markdown},
};

pub mod command;
pub mod pkg;
pub mod text;

pub struct SendErrorHandler {
    bot: Bot,
    owner: ChatId,
}

impl SendErrorHandler {
    pub fn new(bot: Bot, owner: ChatId) -> Arc<Self> {
        Arc::new(Self { bot, owner })
    }
}

impl<E> ErrorHandler<E> for SendErrorHandler
where
    E: fmt::Debug,
{
    fn handle_error(self: Arc<Self>, error: E) -> BoxFuture<'static, ()> {
        let error_msg = format!("Error: {:?}", error);
        log::error!("{}", error_msg);
        Box::pin(async move {
            let _ = self.bot.send_message(self.owner, error_msg).await;
        })
    }
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

fn fmt_at(name: &str, user_id: u64) -> String {
    format!("[{}](tg://user?id={})", markdown::escape(name), user_id)
}
