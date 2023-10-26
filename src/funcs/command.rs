use super::*;
use crate::error_fmt;
use clap::{CommandFactory, Parser};
use thiserror::Error;

pub mod coin;
pub mod curl;
pub mod id;
pub mod quote;
pub mod rate;
pub mod short;
pub mod start;
pub mod test;
pub mod today;
pub mod wcloud;
pub mod wiki;

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
        enum AppError {
            #[error("API请求失败: {0}")]
            RequestError(#[from] reqwest::Error),
            #[error("{}",clap_fmt(.0))]
            ClapError(#[from] clap::error::Error),
            #[error("{}",custom_fmt(.0))]
            CustomError(String),
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
    #[command(description = "curl")]
    Curl,
    #[command(description = "测试")]
    Test,
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
        Ok(Cmd::Test) => test::test(bot, msg).await?,
        Err(e) => {
            log::error!("Error in handler: {}", e);
        }
    }
    Ok(())
}
