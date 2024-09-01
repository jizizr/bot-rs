use bot_rs::{
    filter::call_query::call_query_handler,
    funcs::{
        command::{coin, command_handler, Cmd},
        pkg::{self, cron},
        text::{init, text_handler},
        SendErrorHandler,
    },
    settings::{self, SETTINGS},
    BotError, BOT,
};
use std::error::Error;
use teloxide::{prelude::*, update_listeners::webhooks, utils::command::BotCommands};

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

    let err_handler = SendErrorHandler::new(BOT.clone(), ChatId(SETTINGS.bot.owner));

    let mut dispatcher = Dispatcher::builder(BOT.clone(), handler)
        .enable_ctrlc_handler()
        .distribution_function(|_| None::<std::convert::Infallible>)
        .error_handler(err_handler.clone())
        .build();

    cron::run::<BotError>(
        "0 0 10,14,18,22 * * ?",
        pkg::wcloud::cron::wcloud,
        err_handler.clone(),
    )
    .await;
    cron::run::<BotError>(
        "0 0 4 * * ?",
        pkg::wcloud::cron::wcloud_then_clear,
        err_handler.clone(),
    )
    .await;

    let mode = std::env::var("MODE").unwrap_or_default();
    if mode == "r" {
        BOT.set_my_commands(Cmd::bot_commands())
            .await
            .expect("Couldn't set commands");
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
