use super::*;

pub async fn six(bot: &Bot, msg: &Message) -> BotResult {
    if getor(msg) == Some("6") {
        bot.send(
            SendMessage::new(msg.chat().id(), "6").reply_parameters(reply_to(msg.message_id())),
        )
        .await?;
    };
    Ok(())
}
