use bot_rs::BOT;
use filter::call_query::*;
use funcs::{command::*, pkg, pkg::cron::cron, text::*};
use std::error::Error;
use teloxide::{prelude::*, update_listeners::webhooks};

mod dao;
mod filter;
mod funcs;
mod settings;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    pretty_env_logger::init();
    log::info!("Starting buttons bot...");
    init();
    let handler = dptree::entry()
        .branch(
            Update::filter_message()
                .filter_command::<Cmd>()
                .endpoint(command_handler),
        )
        .branch(
            Update::filter_edited_message()
                .filter_command::<Cmd>()
                .endpoint(command_handler),
        )
        .branch(Update::filter_message().endpoint(text_handler))
        .branch(Update::filter_edited_message().endpoint(text_handler))
        .branch(Update::filter_callback_query().endpoint(call_query_handler))
        .branch(Update::filter_inline_query().endpoint(coin::inline_query_handler));

    let mut dispatcher = Dispatcher::builder(BOT.clone(), handler)
        .enable_ctrlc_handler()
        .distribution_function(|_| None::<std::convert::Infallible>)
        .build();

    cron::run("0 0 10,14,18,22 * * ?", pkg::wcloud::cron::wcloud).await;
    cron::run("0 0 4 * * ?", pkg::wcloud::cron::wcloud_then_clear).await;
    let mode = std::env::var("MODE").unwrap_or_default();

    if mode == "r" {
        let addr = ([127, 0, 0, 1], 12345).into();
        let url = &settings::SETTINGS.url.url;
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
