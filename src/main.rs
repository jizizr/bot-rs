use bot_rs::add_handler;
use bot_rs::CommandInfo;
use ferrisgram::error::GroupIteration;
use ferrisgram::ext::handlers::MessageHandler;
use ferrisgram::ext::{Context, Dispatcher, Updater};
use ferrisgram::Bot;
use funcs::command::*;
use funcs::text::*;
use lazy_static::lazy_static;
use std::fs::File;
use std::io::read_to_string;
use std::sync::Mutex;

mod filter;
mod funcs;
lazy_static! {
    static ref COMMAND_INFO: Mutex<CommandInfo> = Mutex::new(CommandInfo::new());
}

#[allow(unused)]
#[tokio::main]
async fn main() {
    // This function creates a new bot instance and the error is handled accordingly
    let bot = match Bot::new(&read_to_string(File::open("TOKEN").unwrap()).unwrap(), None).await {
        Ok(bot) => bot,
        Err(error) => panic!("failed to create bot: {}", &error),
    };
    let mut dispatcher = &mut Dispatcher::new(&bot);

    add_handler!(dispatcher, "start", start::start, "发送这个了解我");
    add_handler!(dispatcher, "my", quote::quote, "名人名言");
    add_handler!(dispatcher, "help", help, "获取帮助信息");
    add_handler!(dispatcher, "btc", coin::btc, "实时BTC兑换USDT价格");
    add_handler!(dispatcher, "eth", coin::eth, "实时ETH兑换USDT价格");
    add_handler!(dispatcher, "xmr", coin::xmr, "实时XMR兑换USDT价格");

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
