use super::{pkg::wcloud, *};

pub async fn wcloud(bot: &Bot, msg: &Message) -> BotResult {
    wcloud::generate::wcloud(bot, msg.chat.id.0).await
}

pub async fn user_freq(bot: &Bot, msg: &Message) -> BotResult {
    wcloud::generate::user_freq(bot, msg.chat.id.0).await
}
