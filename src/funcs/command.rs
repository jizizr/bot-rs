use crate::{
    analysis::model::{BotLogBuilder, MessageStatus},
    dao::mongo::analysis::insert_log,
};
use std::sync::LazyLock;

use super::*;
use clap::Parser;
use clap_i18n_richformatter::clap_i18n;
use dashmap::DashSet;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    ops::Deref,
};
use telers::types::{BotCommand, ChatMember};

pub mod chat;
pub mod coin;
pub mod config;
pub mod count;
pub mod curl;
pub mod dump;
pub mod hitokoto;
pub mod id;
pub mod music;
pub mod ping;
pub mod quote;
pub mod rate;
pub mod rom;
pub mod short;
pub mod start;
pub mod test;
pub mod timezone;
pub mod today;
pub mod translate;
pub mod uploader;
pub mod vv;
pub mod wcloud;
pub mod wiki;

pub async fn inline_query_handler(bot: Bot, q: InlineQuery) -> BotResult {
    // inline 只承载音乐搜索（虚拟货币 inline 查询已按设计移除）
    music::music_inline_query_handler(bot, q).await
}

pub async fn chosen_inline_handler(bot: Bot, chosen: ChosenInlineResult) -> BotResult {
    music::music_chosen_inline_handler(bot, chosen).await
}

static LIMITER: LazyLock<BottomLocker> = LazyLock::new(|| BottomLocker(DashSet::new()));

#[macro_export]
macro_rules! command_gen {
    ($name:expr, $about:expr, $struct_def:item) => {
        #[derive(Parser)]
        #[clap_i18n]
        #[command(
                                    help_template = "使用方法：{usage}\n\n{all-args}\n\n{about}",
                                    about = concat!("命令功能：",$about),
                                    name = $name,
                                    version = env!("CARGO_PKG_VERSION"),
                                    next_help_heading = "参数解释",
                                    disable_help_flag = true,
                                )]
        $struct_def
    };
}

#[macro_export]
macro_rules! cmd {
    ($name:expr, $about:expr, $struct_name:ident, { $($field:tt)* }) => {
        // lazy_static!{
        //     static ref USAGE: String = $struct_name::command_i18n().render_help().to_string();
        // }
        // error_fmt!(USAGE, $($variant($error_type),)*);
        command_gen!($name, $about, struct $struct_name { $($field)* });
            impl $struct_name {
                pub fn parse_i18n_from_bot<I, T>(
                    itr: I,
                    language_tag: Option<&str>,
                ) -> Result<Self, BotError>
                where
                    I: IntoIterator<Item = T>,
                    T: Into<std::ffi::OsString> + Clone,
                {
                    let usage = Self::get_usage_i18n_with_language_tag(language_tag);
                    Self::parse_i18n_from_with_language_tag(itr, language_tag).map_err(|e| BotError::Clap2(e, usage))
                }
            }
    };
}

async fn auth(bot: &Bot, msg: &Message, user_id: i64) -> Result<bool, BotError> {
    if matches!(msg.chat(), Chat::Private(_)) {
        return Ok(true);
    }
    let member = bot
        .send(GetChatMember::new(msg.chat().id(), user_id))
        .await?;
    Ok(matches!(
        member,
        ChatMember::Creator(_) | ChatMember::Administrator(_)
    ))
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
                $bot.send(SendMessage::new($msg.chat().id(), Cmd::descriptions()))
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

/// A bot command: its name, other names, description, and whether it is
/// left out of the command menu and /help.
macro_rules! commands {
    ($($variant:ident => $name:literal $([$($alias:literal),+])? $($hidden:ident)? : $description:literal,)+) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum Cmd {
            $($variant,)+
        }

        impl Cmd {
            const ALL: &'static [(Cmd, &'static str, &'static [&'static str], &'static str, bool)] = &[
                $((Cmd::$variant, $name, &[$($($alias),+)?], $description, commands!(@hidden $($hidden)?)),)+
            ];
        }
    };
    (@hidden hidden) => { true };
    (@hidden) => { false };
}

commands! {
    Help => "help": "获取帮助信息",
    Start => "start": "发送这个了解我",
    My => "my": "名人名言",
    Coin => "coin": "获取实时虚拟货币价格",
    Id => "id": "获取自己的id",
    Today => "today": "历史上的今天",
    Wiki => "wiki": "维基一下",
    Short => "short": "生成短链接",
    Rate => "rate": "查询实时汇率",
    Wcloud => "wcloud": "生成词云",
    UserFreq => "userfreq": "用户发言统计",
    Curl => "curl": "curl",
    Music => "music": "音乐",
    Config => "config": "功能开关",
    Chat => "chat": "Ai聊天",
    Translate => "translate" ["t"]: "翻译",
    Ocr => "ocr": "识别图片中的文字",
    Ping => "ping": "Ping",
    Rom => "rom": "小米刷机包链接",
    Dump => "dump": "从卡刷包链接提取分区镜像",
    Vv => "vv": "vv不削能玩？",
    Count => "count": "发言趋势",
    Timezone => "timezone" ["tz"]: "查看或设置本群时区",
    Quote => "q" ["quote"]: "回复消息生成语录贴纸",
    Test => "test" hidden: "",
    Uploader => "uploader" hidden: "",
}

impl Cmd {
    /// The command a message starts with, like Telegram clients address it:
    /// `/name` or `/name@bot`, the latter only for this bot.
    pub fn parse(text: &str, bot_username: &str) -> Option<Self> {
        let word = text.split_whitespace().next()?.strip_prefix('/')?;
        let name = match word.split_once('@') {
            Some((name, username)) if username.eq_ignore_ascii_case(bot_username) => name,
            Some(_) => return None,
            None => word,
        };
        Self::ALL
            .iter()
            .find(|(_, command, aliases, _, _)| *command == name || aliases.contains(&name))
            .map(|(cmd, ..)| *cmd)
    }

    fn visible() -> impl Iterator<
        Item = &'static (
            Cmd,
            &'static str,
            &'static [&'static str],
            &'static str,
            bool,
        ),
    > {
        Self::ALL.iter().filter(|(.., hidden)| !hidden)
    }

    pub fn descriptions() -> String {
        let lines = Self::visible().map(|(_, name, aliases, description, _)| {
            let names = std::iter::once(name)
                .chain(aliases.iter())
                .map(|name| format!("/{name}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{names} — {description}")
        });
        std::iter::once("These commands are supported:".to_string())
            .chain(std::iter::once(String::new()))
            .chain(lines)
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn bot_commands() -> Vec<BotCommand> {
        Self::visible()
            .map(|(_, name, _, description, _)| BotCommand::new(*name, *description))
            .collect()
    }
}

pub async fn command_handler(bot: &Bot, msg: &Message, me: &User) -> BotResult {
    // 安全地获取消息文本，没有文本则返回 Err，
    // 让 msg_handler 落入 text_handler（贴纸等非文本消息也需要被处理）
    let text = match getor(msg) {
        Some(text) => text,
        None => return Err(BotError::Custom("不是命令：消息没有文本内容".to_string())),
    };

    let cmd = Cmd::parse(text, me.username.as_deref().unwrap_or_default())
        .ok_or_else(|| BotError::Custom("不是命令".to_string()))?;
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
        Ocr => translate::ocr,
        Ping => ping::ping,
        Rom => rom::rom,
        Dump => dump::dump,
        Vv => vv::vv,
        Count => count::count,
        Timezone => timezone::timezone,
        Quote => quote::quote,
        Test => test::test,
        Uploader => uploader::uploader,
    );
    if let Err(e) = cmd_result {
        let err_msg = format!("{e}");
        let request = SendMessage::new(msg.chat().id(), err_msg.clone())
            .reply_parameters(reply_to(msg.message_id()));
        let bot = bot.clone();
        tokio::spawn(async move { bot.send(request).await });
        match e {
            BotError::Clap(_, _) | BotError::Clap2(_, _) => {
                log.set_status(MessageStatus::CmdError);
            }
            _ => {
                log.set_status(MessageStatus::RunError);
                log.set_error(err_msg);
            }
        }
    }
    // 带凭据的命令不进日志
    log.set_command(if cmd == Cmd::Uploader {
        "/uploader <hidden>".to_string()
    } else {
        text.to_string()
    });
    let user = model::User::from(msg);
    let group = model::Group::from(msg);
    let _ = insert_log((&log.into(), &user, &group)).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands_like_telegram_addresses_them() {
        assert_eq!(Cmd::parse("/music 晴天", "allen_bot"), Some(Cmd::Music));
        assert_eq!(
            Cmd::parse("/music@Allen_Bot 晴天", "allen_bot"),
            Some(Cmd::Music)
        );
        assert_eq!(Cmd::parse("/t hello", "allen_bot"), Some(Cmd::Translate));
        assert_eq!(Cmd::parse("/userfreq", "allen_bot"), Some(Cmd::UserFreq));
        assert_eq!(Cmd::parse("/music@other_bot 晴天", "allen_bot"), None);
        assert_eq!(Cmd::parse("/unknown", "allen_bot"), None);
        assert_eq!(Cmd::parse("music", "allen_bot"), None);
    }

    #[test]
    fn hidden_commands_stay_out_of_menus() {
        let commands = Cmd::bot_commands();
        assert!(commands.iter().all(|command| &*command.command != "test"));
        assert_eq!(
            commands.len(),
            Cmd::ALL.iter().filter(|(.., hidden)| !hidden).count()
        );
        assert!(
            commands
                .iter()
                .all(|command| &*command.command != "uploader")
        );
        let help = Cmd::descriptions();
        assert!(help.contains("/translate, /t — 翻译"));
        assert!(!help.contains("/test"));
    }
}
