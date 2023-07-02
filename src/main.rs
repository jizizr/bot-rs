use ferrisgram::ext::handlers::{CommandHandler, MessageHandler};
use ferrisgram::ext::{Dispatcher, Updater};
use ferrisgram::Bot;
mod filter;
pub mod funcs;
use funcs::command::*;
use funcs::text::*;

#[allow(unused)]
#[tokio::main]
async fn main() {
    // This function creates a new bot instance and the error is handled accordingly
    let bot = match Bot::new("TOKEN", None).await {
        Ok(bot) => bot,
        Err(error) => panic!("failed to create bot: {}", &error),
    };

    let mut dispatcher = &mut Dispatcher::new(&bot);

    dispatcher.add_handler(CommandHandler::new("start", start::start));

    dispatcher.add_handler_to_group(
        MessageHandler::new(
            quote::quote,
            filter::simple::Contain::new("一言"),
        ),
        1,
    );
    dispatcher.add_handler(CommandHandler::new("my", quote::quote));
    let mut updater = Updater::new(&bot, dispatcher);

    // This method will start long polling through the getUpdates method
    updater.start_polling(true).await;
}

// This is our callable function for our message handler which will be used to
// repeat the text.
