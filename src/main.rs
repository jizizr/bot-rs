use bot_rs::getor;
use filter::call_query::*;
use funcs::{command::*, text::*};
use std::error::Error;
use std::fs::File;
use std::io::read_to_string;
use teloxide::{prelude::*, types::Me, update_listeners::webhooks, utils::command::BotCommands};

mod dao;
mod filter;
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
    #[command(description = "测试")]
    Test,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mode = std::env::var("MODE").unwrap_or_default();
    pretty_env_logger::init();
    log::info!("Starting buttons bot...");

    let bot = Bot::new(
        read_to_string(File::open("TOKEN").expect("TOKEN文件打开失败")).expect("TOKEN文件读取失败"),
    );
    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint(message_handler))
        .branch(Update::filter_edited_message().endpoint(message_handler))
        .branch(Update::filter_callback_query().endpoint(call_query_handler))
        .branch(Update::filter_inline_query().endpoint(coin::inline_query_handler));

    let mut dispatcher = Dispatcher::builder(bot.clone(), handler)
        .enable_ctrlc_handler()
        .distribution_function(|_| None::<std::convert::Infallible>)
        .build();

    if mode == "r" {
        let addr = ([127, 0, 0, 1], 12345).into();
        let url =
            read_to_string(File::open("URL").expect("URL文件打开失败")).expect("URL文件读取失败");
        let url = url.parse().unwrap();
        let listener = webhooks::axum(bot, webhooks::Options::new(addr, url))
            .await
            .expect("Couldn't setup webhook");
        dispatcher
            .dispatch_with_listener(
                listener,
                LoggingErrorHandler::with_custom_text("An error from the update listener"),
            )
            .await
    } else {
        tokio::select! {
            _ = dispatcher.dispatch() => (),
            _ = tokio::signal::ctrl_c() => (),
        }
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
            Ok(Cmd::Coin) => coin::coin(bot, msg).await?,
            Ok(Cmd::Id) => id::id(bot, msg).await?,
            Ok(Cmd::Today) => today::today(bot, msg).await?,
            Ok(Cmd::Wiki) => wiki::wiki(bot, msg).await?,
            Ok(Cmd::Short) => short::short(bot, msg).await?,
            Ok(Cmd::Rate) => rate::rate(bot, msg).await?,
            Ok(Cmd::Wcloud) => wcloud::wcloud(bot, msg).await?,
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
