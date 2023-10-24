use bot_rs::{get, getor};
use command::*;
use lazy_static::lazy_static;
use serde::Deserialize;
use std::error::Error;
use teloxide::{
    prelude::*,
    types::{Me, ParseMode},
    utils::{command::BotCommands, markdown},
};
use text::*;

pub mod command;
pub mod pkg;
pub mod text;

type BotError = Box<dyn Error + Send + Sync>;

#[derive(BotCommands)]
#[command(
    rename_rule = "lowercase",
    description = "These commands are supported:"
)]
enum Cmd {
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

pub async fn message_handler(bot: Bot, msg: Message, me: Me) -> Result<(), BotError> {
    if let Some(text) = getor(&msg) {
        match BotCommands::parse(text, me.username()) {
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
            Err(_) => {
                if !text.starts_with("/") {
                    fix::fix(&bot, &msg).await?;
                    six::six(&bot, &msg).await?;
                    repeat::repeat(&bot, &msg).await?;
                    pretext::pretext(&bot, &msg).await?;
                }
            }
        }
    } else {
        println!("{:#?}", msg);
    }
    Ok(())
}
