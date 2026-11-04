/// Runs the bot with long polling and only `$func` behind `/$cmd`, for trying
/// one command against the configured test bot by hand.
#[macro_export]
macro_rules! simple_command_test {
    ($cmd:ident,$func:expr) => {
        #[tokio::test]
        async fn $cmd() {
            use bot_rs::*;
            use telers::{
                Dispatcher, Router,
                event::{
                    EventReturn,
                    telegram::{Handler, HandlerResult},
                },
            };
            async fn handler(bot: Bot, msg: Message) -> HandlerResult {
                let key = "/".to_string() + stringify!($cmd);
                if getor(&msg).is_some_and(|text| text.starts_with(key.as_str()))
                    && let Err(e) = $func(&bot, &msg).await
                {
                    println!("{}", e);
                }
                Ok(EventReturn::Finish)
            }
            let router =
                Router::new("test").on_message(|observer| observer.register(Handler::new(handler)));
            let dispatcher = Dispatcher::builder()
                .main_router(router.configure_default())
                .bot(BOT.clone())
                .build();
            tokio::select! {
                _ = dispatcher.run_polling() => (),
                _ = tokio::signal::ctrl_c() => (),
            }
        }
    };
}
