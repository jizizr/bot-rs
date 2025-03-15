use crate::{
    analysis::model::{BotLogBuilder, MessageStatus},
    dao::mongo::analysis::insert_log,
};

use super::*;
use clap::{CommandFactory, Parser};
use dashmap::DashSet;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    ops::Deref,
};
use teloxide::types::{
    ChatKind, InlineKeyboardButton, InlineKeyboardButtonKind::CallbackData, InlineKeyboardMarkup,
    InlineQueryResult, InlineQueryResultArticle, InputFile, InputMediaAudio, InputMessageContent,
    InputMessageContentText, LinkPreviewOptions, MediaKind, Message, MessageId, MessageKind,
    ParseMode,
};

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
macro_rules! command_gen {
    ($name:expr, $about:expr, $struct_def:item) => {
        #[derive(Parser)]
        #[command(
                                    help_template = "使用方法：{usage}\n\n{all-args}\n\n{about}",
                                    about = concat!("命令功能：",$about),
                                    name = $name,
                                    next_help_heading = "参数解释",
                                    disable_help_flag = true,
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
        // error_fmt!(USAGE, $($variant($error_type),)*);
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
            Cmd::Help => {
                $bot.send_message($msg.chat.id, Cmd::descriptions().to_string())
                    .await
                    .map(|_| ())
                    .map_err(|e| e.into())
            }
            $(
                Cmd::$stat => $func($bot, $msg).await,
            )+
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

pub async fn command_handler(bot: &Bot, msg: &Message, me: &Me) -> BotResult {
    // 安全地获取消息文本，如果没有文本则提早返回
    let text = match getor(msg) {
        Some(text) => text,
        None => return Ok(()), // 没有文本内容，直接返回
    };

    let cmd = if let Ok(cmd) = BotCommands::parse(text, me.username()) {
        cmd
    } else {
        return Ok(());
    };
    let mut log = BotLogBuilder::from(msg);
    let cmd_result: Result<(), BotError> = cmd_match!(
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
    if let Err(e) = cmd_result {
        let err_msg = format!("{e}");
        tokio::spawn(
            bot.send_message(msg.chat.id, err_msg.clone())
                .reply_parameters(ReplyParameters::new(msg.id))
                .send(),
        );
        match e {
            BotError::Clap(_, _) => {
                log.set_status(MessageStatus::CmdError);
            }
            _ => {
                log.set_status(MessageStatus::RunError);
                log.set_command(text.to_string());
                log.set_error(err_msg);
            }
        }
    }
    let _ = insert_log(&log.into()).await;
    Ok(())
}
