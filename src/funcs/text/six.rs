use super::*;

pub async fn six(bot: &Bot, msg: &Message) -> BotResult {
    if getor(msg) == Some("6") {
        bot.send_message(msg.chat.id, "6")
            .reply_parameters(ReplyParameters::new(msg.id))
            .await?;
    };
    Ok(())
}
