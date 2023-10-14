use bot_rs::BOT;
use filter::call_query::*;
use funcs::message_handler;
use funcs::{command::*, pkg, pkg::cron::cron};
use std::error::Error;
use std::fs::File;
use std::io::read_to_string;
use teloxide::{prelude::*, update_listeners::webhooks, utils::command::BotCommands};

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
    pretty_env_logger::init();
    log::info!("Starting buttons bot...");

    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint(message_handler))
        .branch(Update::filter_edited_message().endpoint(message_handler))
        .branch(Update::filter_callback_query().endpoint(call_query_handler))
        .branch(Update::filter_inline_query().endpoint(coin::inline_query_handler));

    let mut dispatcher = Dispatcher::builder(BOT.clone(), handler)
        .enable_ctrlc_handler()
        .distribution_function(|_| None::<std::convert::Infallible>)
        .build();

    cron::run("0 0 10,14,18,22 * * ?", pkg::wcloud::cron::wcloud).await;

    let mode = std::env::var("MODE").unwrap_or_default();

    if mode == "r" {
        let addr = ([127, 0, 0, 1], 12345).into();
        let url =
            read_to_string(File::open("URL").expect("URL文件打开失败")).expect("URL文件读取失败");
        let url = url.parse().unwrap();
        let listener = webhooks::axum(BOT.clone(), webhooks::Options::new(addr, url))
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
