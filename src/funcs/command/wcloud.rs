use super::pkg::wcloud;
use super::*;

pub async fn wcloud(bot: Bot, msg: Message) -> Result<(), BotError> {
    wcloud::gen::wcloud(&bot, msg.chat.id.to_string()).await
}
