#[macro_export]
macro_rules! simple_command_test {
    ($cmd:ident,$func:expr) => {
        #[tokio::test]
        async fn $cmd() {
            use bot_rs::{funcs::SendErrorHandler, settings::SETTINGS, *};
            use teloxide::prelude::*;
            let handler = dptree::entry().branch(Update::filter_message().endpoint(
                |bot: Bot, msg: Message| async move {
                    let key = "/".to_string() + stringify!($cmd);
                    if getor(&msg).unwrap().starts_with(key.as_str()) {
                        if let Err(e) = $func(&bot, &msg).await {
                            println!("{}", e);
                            return Err(e);
                        } else {
                            return Ok(());
                        }
                    } else {
                        Ok(())
                    }
                },
            ));
            let err_handler = SendErrorHandler::new(BOT.clone(), ChatId(SETTINGS.bot.owner));

            let mut dispatcher = Dispatcher::builder(BOT.clone(), handler)
                .enable_ctrlc_handler()
                .distribution_function(|_| None::<std::convert::Infallible>)
                .error_handler(err_handler.clone())
                .build();

            tokio::select! {
                _ = dispatcher.dispatch() => (),
                _ = tokio::signal::ctrl_c() => (),
            }
        }
    };
}
