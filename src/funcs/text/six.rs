use super::*;

pub async fn six(bot: Bot, msg: Message) -> Result<(), Box<dyn Error + Send + Sync>> {
    if getor(&msg) == Some("6") {
        bot.send_message(msg.chat.id, "6")
            .reply_to_message_id(msg.id)
            .await?;
    };
    Ok(())
}
