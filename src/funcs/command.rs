use super::*;
use crate::{cmd, command_gen, error_fmt, lock};
use clap::{CommandFactory, Parser};
use dashmap::DashSet;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    ops::Deref,
};
use teloxide::{
    types::{
        ChatKind, InlineKeyboardButton, InlineKeyboardButtonKind::CallbackData,
        InlineKeyboardMarkup, InlineQueryResult, InlineQueryResultArticle, InputFile,
        InputMediaAudio, InputMessageContent, InputMessageContentText, LinkPreviewOptions,
        MediaKind, Message, MessageId, MessageKind, ParseMode,
    },
    utils::command::ParseError,
};
use thiserror::Error;

pub mod chat;
pub mod coin;
pub mod config;
pub mod curl;
pub mod hitokoto;
pub mod id;
pub mod music;
pub mod ping;
pub mod rate;
pub mod short;
pub mod start;
pub mod test;
pub mod today;
pub mod translate;
pub mod vv;
pub mod wcloud;
pub mod wiki;

lazy_static! {
    static ref LIMITER: BottomLocker = BottomLocker(DashSet::new());
}

#[macro_export]
macro_rules! error_fmt {
    ($usage:ident, $($variant:ident($error_type:ty),)*) => {
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
            Request(#[from] reqwest::Error),
            #[error("API请求失败: {0}")]
            Retry(#[from] reqwest_middleware::Error),
            #[error("{}",clap_fmt(.0))]
            Clap(#[from] clap::error::Error),
            #[error("{}",custom_fmt(.0))]
            Custom(String),
            #[error("{}",.0)]
            Send(#[from] teloxide::RequestError),
            $(
            #[error("{}",.0)]
            $variant(#[from] $error_type),
            )*
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
    ($name:expr, $about:expr, $struct_name:ident, { $($field:tt)* }, $($variant:ident($error_type:ty),)*) => {
        lazy_static!{
            static ref USAGE: String = $struct_name::command().render_help().to_string();
        }
        error_fmt!(USAGE, $($variant($error_type),)*);
        command_gen!($name, $about, struct $struct_name { $($field)* });
    };
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

fn hashing<T: Hash>(s: T) -> u64 {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

struct BottomLocker(DashSet<u64>);

impl Deref for BottomLocker {
    type Target = DashSet<u64>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl BottomLocker {
    fn is_running(&self, flag: u64) -> bool {
        !self.insert(flag)
    }
    fn over(&self, flag: u64) {
        self.remove(&flag);
    }
}

struct Guard<'a> {
    locker: &'a BottomLocker,
    flag: u64,
}

impl<'a> Guard<'a> {
    fn new(locker: &'a BottomLocker, flag: u64) -> Self {
        Guard { locker, flag }
    }
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.locker.over(self.flag);
    }
}

#[macro_export]
macro_rules! lock {
    ($conf:expr) => {{
        let h = hashing($conf);
        if LIMITER.is_running(h) {
            return Ok(());
        }
        Guard::new(&LIMITER, h)
    }};
}

macro_rules! cmd_match {
    ($cmd:expr, $bot:expr, $msg:expr,$($stat:ident => $func:expr),+ $(,)?) => {
        match $cmd {
            Ok(Cmd::Help) => {
                $bot.send_message($msg.chat.id, Cmd::descriptions().to_string())
                    .await?;
            }
            $(
                Ok(Cmd::$stat) => $func($bot, $msg).await?,
            )+
            Err(e) => {
                log::error!("Error in handler: {}", e);
            }
        }
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
    #[command(description = "用户发言统计")]
    UserFreq,
    #[command(description = "curl")]
    Curl,
    #[command(description = "音乐")]
    Music,
    #[command(description = "功能开关")]
    Config,
    #[command(description = "Ai聊天")]
    Chat,
    #[command(description = "翻译", aliases = ["t"])]
    Translate,
    #[command(description = "Ping")]
    Ping,
    #[command(description = "vv不削能玩？")]
    Vv,
    #[command(hide)]
    Test,
}

pub async fn command_handler(bot: Bot, msg: Message, me: Me) -> BotResult {
    let cmd: Result<Cmd, ParseError> = BotCommands::parse(getor(&msg).unwrap(), me.username());
    cmd_match!(
        cmd,
        bot,
        msg,
        Start => start::start,
        My => hitokoto::hitokoto,
        Coin => coin::coin,
        Id => id::id,
        Today => today::today,
        Wiki => wiki::wiki,
        Short => short::short,
        Rate => rate::rate,
        Wcloud => wcloud::wcloud,
        UserFreq => wcloud::user_freq,
        Curl => curl::curl,
        Music => music::music,
        Config => config::config,
        Chat => chat::chat,
        Translate => translate::translate,
        Ping => ping::ping,
        Vv => vv::vv,
        Test => test::test,
    );
    Ok(())
}
