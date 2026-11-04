use bot_rs::{
    BOT, BotError, BotResult, ME,
    filter::call_query::call_query_handler,
    funcs::{
        command::{self, Cmd},
        pkg::{self, cron},
        text::init,
    },
    msg_handler,
    settings::{self},
};
use std::error::Error;
use telers::{
    Bot, Dispatcher, Router,
    enums::UpdateType,
    event::{
        EventReturn,
        telegram::{Handler, HandlerResult},
    },
    methods::{DeleteWebhook, GetMe, SetMyCommands, SetWebhook},
    types::{CallbackQuery, ChosenInlineResult, InlineQuery, Message},
    webhooks::axum::{UpdatesHandler, get_updates_router},
};
use tokio::time::{Duration, sleep};

const ALLOWED_UPDATES: [UpdateType; 5] = [
    UpdateType::Message,
    UpdateType::EditedMessage,
    UpdateType::CallbackQuery,
    UpdateType::InlineQuery,
    UpdateType::ChosenInlineResult,
];

/// Every handler reports its own failures to the chat; here they are only
/// logged, and the update always counts as handled.
fn handled(result: BotResult, kind: &str) -> HandlerResult {
    if let Err(err) = result {
        log::error!("{kind} handler failed: {err}");
    }
    Ok(EventReturn::Finish)
}

async fn on_message(bot: Bot, message: Message) -> HandlerResult {
    handled(msg_handler(bot, message).await, "message")
}

async fn on_callback_query(bot: Bot, query: CallbackQuery) -> HandlerResult {
    handled(call_query_handler(bot, query).await, "callback query")
}

async fn on_inline_query(bot: Bot, query: InlineQuery) -> HandlerResult {
    handled(
        command::inline_query_handler(bot, query).await,
        "inline query",
    )
}

async fn on_chosen_inline_result(bot: Bot, chosen: ChosenInlineResult) -> HandlerResult {
    handled(
        command::chosen_inline_handler(bot, chosen).await,
        "chosen inline result",
    )
}

fn router() -> Router {
    Router::new("main")
        .on_message(|observer| observer.register(Handler::new(on_message)))
        .on_edited_message(|observer| observer.register(Handler::new(on_message)))
        .on_callback_query(|observer| observer.register(Handler::new(on_callback_query)))
        .on_inline_query(|observer| observer.register(Handler::new(on_inline_query)))
        .on_chosen_inline_result(|observer| {
            observer.register(Handler::new(on_chosen_inline_result))
        })
}

/// A fresh secret per start: Telegram sends it with every webhook call, so
/// nothing else can feed updates to the local listener.
fn webhook_secret() -> String {
    use rand::Rng;
    rand::rng()
        .sample_iter(rand::distr::Alphanumeric)
        .take(48)
        .map(char::from)
        .collect()
}

async fn serve_webhook(dispatcher: Dispatcher) -> Result<(), BotError> {
    BOT.send(SetMyCommands::new(Cmd::bot_commands())).await?;
    let url: url::Url = settings::SETTINGS
        .url
        .url
        .parse()
        .expect("invalid webhook url");
    let secret = webhook_secret();
    let app = axum::Router::new().route(
        url.path(),
        get_updates_router(
            UpdatesHandler::new(BOT.clone(), dispatcher).secret_token(secret.as_str()),
        ),
    );
    let listener = loop {
        match tokio::net::TcpListener::bind(("127.0.0.1", 12345)).await {
            Ok(listener) => break listener,
            Err(err) => {
                log::warn!("Couldn't bind the webhook listener, retrying: {err}");
                sleep(Duration::from_secs(2)).await;
            }
        }
    };
    loop {
        let request = SetWebhook::new(url.as_str())
            .allowed_updates(ALLOWED_UPDATES)
            .secret_token(secret.as_str());
        match BOT.send(request).await {
            Ok(_) => break,
            Err(err) => {
                log::warn!("Couldn't set webhook, retrying: {err}");
                sleep(Duration::from_secs(2)).await;
            }
        }
    }
    tokio::select! {
        result = axum::serve(listener, app) => result?,
        _ = tokio::signal::ctrl_c() => {}
    }
    Ok(())
}

/// 和 `pretty_env_logger::init` 一样按 RUST_LOG 过滤，另外丢掉扫描器探测 webhook
/// 时 telers 打的 "Invalid secret token"：请求本身已经被 401 拒掉，日志只是噪音
fn init_logger() {
    let mut builder = pretty_env_logger::formatted_builder();
    if let Ok(filters) = std::env::var("RUST_LOG") {
        builder.parse_filters(&filters);
    }
    let logger = builder.build();
    log::set_max_level(logger.filter());
    log::set_boxed_logger(Box::new(QuietScanners(logger))).expect("logger is set once");
}

struct QuietScanners(pretty_env_logger::env_logger::Logger);

impl QuietScanners {
    fn is_scanner_probe(record: &log::Record) -> bool {
        record.target().starts_with("telers::webhooks")
            && record
                .args()
                .to_string()
                .starts_with("Invalid secret token")
    }
}

impl log::Log for QuietScanners {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.0.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        if self.0.matches(record) && !Self::is_scanner_probe(record) {
            self.0.log(record);
        }
    }

    fn flush(&self) {
        self.0.flush();
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    init_logger();
    log::info!("Starting buttons bot...");
    init();
    let me = loop {
        match BOT.send(GetMe::new()).await {
            Ok(me) => break me,
            Err(err) => {
                log::warn!("Couldn't fetch the bot identity, retrying: {err}");
                sleep(Duration::from_secs(2)).await;
            }
        }
    };
    ME.set(me).expect("bot identity is fetched once");

    let dispatcher = Dispatcher::builder()
        .main_router(router().configure_default())
        .bot(BOT.clone())
        .allowed_updates(ALLOWED_UPDATES)
        .build();

    cron::run("0 0 10,14,18,22 * * ?", pkg::wcloud::cron::wcloud).await;
    cron::run("0 0 4 * * ?", pkg::wcloud::cron::wcloud_then_clear).await;

    if std::env::var("MODE").unwrap_or_default() == "r" {
        serve_webhook(dispatcher).await?;
    } else {
        BOT.send(DeleteWebhook::new()).await?;
        tokio::select! {
            result = dispatcher.run_polling() => {
                if let Err(err) = result {
                    log::error!("polling stopped: {err}");
                }
            }
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::QuietScanners;

    fn is_probe(target: &str, message: std::fmt::Arguments) -> bool {
        QuietScanners::is_scanner_probe(
            &log::Record::builder()
                .target(target)
                .level(log::Level::Error)
                .args(message)
                .build(),
        )
    }

    #[test]
    fn only_secret_token_rejections_are_dropped() {
        let handler = "telers::webhooks::axum::handler";
        assert!(is_probe(
            handler,
            format_args!("Invalid secret token secret_token=XTelegramBotApiSecretToken(None)")
        ));
        assert!(!is_probe(handler, format_args!("Failed to parse update")));
        assert!(!is_probe("bot_rs", format_args!("Invalid secret token")));
    }
}
