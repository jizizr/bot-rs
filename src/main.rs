use std::error::Error;
use teloxide::{prelude::*, types::Me, utils::command::BotCommands};
mod funcs;
use funcs::command::*;
use std::fs::File;
use std::io::read_to_string;
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
    #[command(description = "实时BTC兑换USDT价格")]
    Btc,
    #[command(description = "实时XMR兑换USDT价格")]
    Xmr,
    #[command(description = "实时ETH兑换USDT价格")]
    Eth,
    #[command(description = "获取自己的id")]
    Id,
    #[command(description = "历史上的今天")]
    Today,
    #[command(description = "维基一下")]
    Wiki,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    pretty_env_logger::init();
    log::info!("Starting buttons bot...");

    let bot = Bot::new(read_to_string(File::open("TOKEN").unwrap()).unwrap());

    let handler = dptree::entry().branch(Update::filter_message().endpoint(message_handler));

    let mut dispatcher = Dispatcher::builder(bot, handler)
        .enable_ctrlc_handler()
        .distribution_function(|_| None::<std::convert::Infallible>)
        .build();
    tokio::select! {
        _ = dispatcher.dispatch() => (),
        _ = tokio::signal::ctrl_c() => (),
    }
    Ok(())
}

async fn message_handler(
    bot: Bot,
    msg: Message,
    me: Me,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if let Some(text) = msg.text() {
        match BotCommands::parse(text, me.username()) {
            Ok(Cmd::Help) => {
                bot.send_message(msg.chat.id, Cmd::descriptions().to_string())
                    .await?;
            }
            Ok(Cmd::Start) => start::start(bot, msg).await?,
            Ok(Cmd::My) => quote::quote(bot, msg).await?,
            Ok(Cmd::Btc) => coin::btc(bot, msg).await?,
            Ok(Cmd::Xmr) => coin::xmr(bot, msg).await?,
            Ok(Cmd::Eth) => coin::eth(bot, msg).await?,
            Ok(Cmd::Id) => id::id(bot, msg).await?,
            Ok(Cmd::Today) => today::today(bot, msg).await?,
            Ok(Cmd::Wiki) => wiki::wiki(bot, msg).await?,
            Err(_) => {}
        }
    }

    Ok(())
}
