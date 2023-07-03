use ferrisgram::error::GroupIteration;
use ferrisgram::ext::handlers::MessageHandler;
use ferrisgram::ext::{Context, Dispatcher, Updater};
use ferrisgram::Bot;
mod filter;
pub mod funcs;
use bot_rs::CommandInfo;
use funcs::command::*;
use funcs::text::*;
use lazy_static::lazy_static;
use std::sync::Mutex;

lazy_static! {
    static ref command_info: Mutex<CommandInfo> = Mutex::new(CommandInfo::new());
}
#[allow(unused)]
#[tokio::main]
async fn main() {
    // This function creates a new bot instance and the error is handled accordingly
    let bot = match Bot::new("TOKEN", None).await {
        Ok(bot) => bot,
        Err(error) => panic!("failed to create bot: {}", &error),
    };
    let mut dispatcher = &mut Dispatcher::new(&bot);

    // dispatcher.add_handler(CommandHandler::new("start", start::start));
    command_info
        .lock()
        .unwrap()
        .add_handler(dispatcher, "start", start::start);

    dispatcher.add_handler_to_group(
        MessageHandler::new(quote::quote, filter::simple::Contain::new("一言")),
        1,
    );
    // dispatcher.add_handler(CommandHandler::new("my", quote::quote));
    command_info
        .lock()
        .unwrap()
        .add_handler(dispatcher, "my", quote::quote);
    command_info
        .lock()
        .unwrap()
        .add_handler(dispatcher, "help", help);
    let mut updater = Updater::new(&bot, dispatcher);
    // This method will start long polling through the getUpdates method
    updater.start_polling(true).await;
}

pub async fn help(bot: Bot, ctx: Context) -> ferrisgram::error::Result<GroupIteration> {
    let text = command_info.lock().unwrap().get_command().join("\n");
    let msg = ctx.effective_message.unwrap();
    msg.reply(&bot, text.as_str()).send().await?;
    Ok(GroupIteration::EndGroups)
}
