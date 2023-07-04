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
    static ref COMMAND_INFO: Mutex<CommandInfo> = Mutex::new(CommandInfo::new());
}

#[allow(unused)]
#[tokio::main]
async fn main() {
    // This function creates a new bot instance and the error is handled accordingly
    let bot = match Bot::new("Token", None).await {
        Ok(bot) => bot,
        Err(error) => panic!("failed to create bot: {}", &error),
    };
    let mut dispatcher = &mut Dispatcher::new(&bot);

    COMMAND_INFO
        .lock()
        .unwrap()
        .add_handler(dispatcher, "start", start::start, "发送这个了解我");

    COMMAND_INFO
        .lock()
        .unwrap()
        .add_handler(dispatcher, "my", quote::quote, "名人名言");

    COMMAND_INFO
        .lock()
        .unwrap()
        .add_handler(dispatcher, "help", help, "获取帮助信息");

    dispatcher.add_handler_to_group(
        MessageHandler::new(quote::quote, filter::simple::Contain::new("一言")),
        1,
    );

    dispatcher.add_handler_to_group(
        MessageHandler::new(six::six, filter::simple::Equal::new("6")),
        1,
    );

    let mut updater = Updater::new(&bot, dispatcher);
    updater.start_polling(true).await;
}

pub async fn help(bot: Bot, ctx: Context) -> ferrisgram::error::Result<GroupIteration> {
    let text = COMMAND_INFO
        .lock()
        .unwrap()
        .get_command()
        .iter()
        .map(|(key, value)| format!("/{:8}    {}", key, value))
        .collect::<Vec<String>>()
        .join("\n");

    let msg = ctx.effective_message.unwrap();
    msg.reply(&bot, text.as_str())
        .parse_mode("markdown".to_string())
        .send()
        .await?;

    Ok(GroupIteration::EndGroups)
}
