use super::*;
use crate::{cmd, command_gen, error_fmt};
use clap::{CommandFactory, Parser};
use dashmap::DashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use teloxide::types::{
    ChatKind, InlineKeyboardButton, InlineKeyboardButtonKind::CallbackData, InlineKeyboardMarkup,
    InlineQueryResult, InlineQueryResultArticle, InputFile, InputMediaAudio, InputMessageContent,
    InputMessageContentText, Message, ParseMode,
};
use thiserror::Error;

pub mod chat;
pub mod coin;
pub mod config;
pub mod curl;
pub mod id;
pub mod music;
pub mod quote;
pub mod rate;
pub mod short;
pub mod start;
pub mod test;
pub mod today;
pub mod translate;
pub mod wcloud;
pub mod wiki;

lazy_static! {
    static ref LIMITER_Q: BottomLocker<(ChatId, MessageId)> = BottomLocker(DashSet::new());
    static ref LIMITER_I: BottomLocker<u64> = BottomLocker(DashSet::new());
}

#[macro_export]
macro_rules! error_fmt {
    ($usage:ident) => {
        fn clap_fmt(err: &clap::error::Error) -> String {
            format!(
                "{}\n{}",
                err.render().to_string().splitn(2, "Usage").nth(0).unwrap(),
                *$usage
            )
        }
        fn custom_fmt(err: &String) -> String {
            format!("{}\n\n{}", err, *USAGE)
        }
        #[allow(dead_code)]
        #[derive(Error, Debug)]
        pub enum AppError {
            #[error("API请求失败: {0}")]
            RequestError(#[from] reqwest::Error),
            #[error("API请求失败: {0}")]
            RetryError(#[from] reqwest_middleware::Error),
            #[error("{}",clap_fmt(.0))]
            ClapError(#[from] clap::error::Error),
            #[error("{}",custom_fmt(.0))]
            CustomError(String),
            #[error("{}",.0)]
            SendError(#[from] teloxide::RequestError),
            #[error("{}", .0)]
            DynamicError(BotError),
        }
    };
}

#[macro_export]
macro_rules! command_gen {
    ($name:expr, $about:expr, $struct_def:item) => {
        #[derive(Parser)]
        #[command(
                            help_template = "使用方法：{usage}\n\n{all-args}\n\n{about}",
                            about = concat!("命令功能：",$about),
                            name = $name,
                            next_help_heading = "参数解释",
                            disable_help_flag = true
                        )]
        $struct_def
    };
}

#[macro_export]
macro_rules! cmd {
    ($name:expr, $about:expr, $struct_name:ident, { $($field:tt)* }) => {
        lazy_static!{
            static ref USAGE: String = $struct_name::command().render_help().to_string();
        }
        error_fmt!(USAGE);
        command_gen!($name, $about, struct $struct_name { $($field)* });
    };
}

#[derive(BotCommands)]
#[command(
    rename_rule = "lowercase",
    description = "These commands are supported:"
)]

pub enum Cmd {
    #[command(description = "获取帮助信息")]
    Help,
    #[command(description = "发送这个了解我")]
    Start,
    #[command(description = "名人名言")]
    My,
    #[command(description = "获取实时虚拟货币价格")]
    Coin,
    #[command(description = "获取自己的id")]
    Id,
    #[command(description = "历史上的今天")]
    Today,
    #[command(description = "维基一下")]
    Wiki,
    #[command(description = "生成短链接")]
    Short,
    #[command(description = "查询实时汇率")]
    Rate,
    #[command(description = "生成词云")]
    Wcloud,
    #[command(description = "curl")]
    Curl,
    #[command(description = "音乐")]
    Music,
    #[command(description = "功能开关")]
    Config,
    #[command(description = "Ai聊天")]
    Chat,
    #[command(description = "翻译")]
    Translate,
    #[command(description = "测试")]
    Test,
}

async fn auth(bot: &Bot, msg: &Message, user_id: UserId) -> Result<bool, BotError> {
    match msg.chat.kind {
        ChatKind::Private { .. } => Ok(true),
        _ => {
            let mconfig = bot.get_chat_member(msg.chat.id, user_id).await?;
            Ok(mconfig.is_administrator() || mconfig.is_privileged())
        }
    }
}

fn hashing(s: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

struct BottomLocker<T>(DashSet<T>);

impl<T> BottomLocker<T>
where
    T: Eq + Hash,
{
    fn is_running(&self, flag: T) -> bool {
        !self.0.insert(flag)
    }
    fn over(&self, flag: T) {
        self.0.remove(&flag);
    }
}

struct Guard<'a, T>
where
    T: Hash + Eq + Copy,
{
    locker: &'a BottomLocker<T>,
    flag: T,
    is_running: bool,
}

impl<'a, T> Guard<'a, T>
where
    T: Eq + Hash + Copy,
{
    fn new(locker: &'a BottomLocker<T>, flag: T) -> Self {
        Guard {
            locker,
            flag,
            is_running: locker.is_running(flag),
        }
    }
}

impl<'a, T> Drop for Guard<'a, T>
where
    T: Hash + Eq + Copy,
{
    fn drop(&mut self) {
        self.locker.over(self.flag);
    }
}

pub async fn command_handler(bot: Bot, msg: Message, me: Me) -> BotResult {
    match BotCommands::parse(getor(&msg).unwrap(), me.username()) {
        Ok(Cmd::Help) => {
            bot.send_message(msg.chat.id, Cmd::descriptions().to_string())
                .await?;
        }
        Ok(Cmd::Start) => start::start(bot, msg).await?,
        Ok(Cmd::My) => quote::quote(bot, msg).await?,
        Ok(Cmd::Coin) => coin::coin(bot, msg).await?,
        Ok(Cmd::Id) => id::id(bot, msg).await?,
        Ok(Cmd::Today) => today::today(bot, msg).await?,
        Ok(Cmd::Wiki) => wiki::wiki(bot, msg).await?,
        Ok(Cmd::Short) => short::short(bot, msg).await?,
        Ok(Cmd::Rate) => rate::rate(bot, msg).await?,
        Ok(Cmd::Wcloud) => wcloud::wcloud(bot, msg).await?,
        Ok(Cmd::Curl) => curl::curl(bot, msg).await?,
        Ok(Cmd::Music) => music::music(bot, msg).await?,
        Ok(Cmd::Config) => config::config(bot, msg).await?,
        Ok(Cmd::Chat) => chat::chat(bot, msg).await?,
        Ok(Cmd::Translate) => translate::translate(bot, msg).await?,
        Ok(Cmd::Test) => test::test(bot, msg).await?,
        Err(e) => {
            log::error!("Error in handler: {}", e);
        }
    }
    Ok(())
}
