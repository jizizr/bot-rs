use bot_rs::{
    BOT, BotError,
    filter::call_query::call_query_handler,
    funcs::{
        // SendErrorHandler,
        command::{self, Cmd},
        pkg::{self, cron},
        text::init,
    },
    msg_handler,
    settings::{self},
};
use std::error::Error;
use teloxide::{
    payloads::SetWebhookSetters, prelude::*, types::AllowedUpdate, update_listeners::webhooks,
    utils::command::BotCommands,
};
use tokio::time::{Duration, sleep};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    pretty_env_logger::init();
    log::info!("Starting buttons bot...");
    init();
    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint(msg_handler))
        .branch(Update::filter_edited_message().endpoint(msg_handler))
        .branch(Update::filter_callback_query().endpoint(call_query_handler))
        .branch(Update::filter_inline_query().endpoint(command::inline_query_handler))
        .branch(Update::filter_chosen_inline_result().endpoint(command::chosen_inline_handler));

    // let err_handler = SendErrorHandler::new(BOT.clone(), ChatId(SETTINGS.bot.owner));

    let mut dispatcher = Dispatcher::builder(BOT.clone(), handler)
        .enable_ctrlc_handler()
        .distribution_function(|_| None::<std::convert::Infallible>)
        // .error_handler(err_handler.clone())
        .build();

    cron::run::<BotError>("0 0 10,14,18,22 * * ?", pkg::wcloud::cron::wcloud, None).await;
    cron::run::<BotError>("0 0 4 * * ?", pkg::wcloud::cron::wcloud_then_clear, None).await;

    let mode = std::env::var("MODE").unwrap_or_default();
    if mode == "r" {
        BOT.set_my_commands(Cmd::bot_commands())
            .await
            .expect("Couldn't set commands");
        let addr = ([127, 0, 0, 1], 12345).into();
        let url = &settings::SETTINGS.url.url;
        let url: url::Url = url.parse().unwrap();
        BOT.set_webhook(url.clone())
            .allowed_updates(vec![
                AllowedUpdate::Message,
                AllowedUpdate::EditedMessage,
                AllowedUpdate::CallbackQuery,
                AllowedUpdate::InlineQuery,
                AllowedUpdate::ChosenInlineResult,
            ])
            .await
            .expect("Couldn't set webhook allowed updates");
        sleep(Duration::from_secs(2)).await;
        let listener = loop {
            match webhooks::axum(BOT.clone(), webhooks::Options::new(addr, url.clone())).await {
                Ok(listener) => break listener,
                Err(err) => {
                    log::warn!("Couldn't setup webhook, retrying: {err}");
                    sleep(Duration::from_secs(2)).await;
                }
            }
        };
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
