use bot_rs::getor;
use funcs::command::*;
use std::error::Error;
use std::fs::File;
use std::io::read_to_string;
use teloxide::{prelude::*, types::Me, utils::command::BotCommands};
mod funcs;

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
    #[command(description = "生成短链接")]
    Short,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    pretty_env_logger::init();
    log::info!("Starting buttons bot...");

    let bot = Bot::new(
        read_to_string(File::open("TOKEN").expect("TOKEN文件打开失败")).expect("TOKEN文件读取失败"),
    );

    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint(message_handler))
        .branch(Update::filter_edited_message().endpoint(message_handler));

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
    if let Some(text) = getor(&msg) {
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
            Ok(Cmd::Short) => short::short(bot, msg).await?,
            Err(_) => {}
        }
    } else {
        //Debug
        bot.send_message(msg.chat.id, format!("{:#?}", msg.caption()))
            .await?;
    }

    Ok(())
}
