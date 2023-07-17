use super::*;

pub async fn six(bot: Bot, msg: Message) -> Result<(), Box<dyn Error + Send + Sync>> {
    bot.send_message(msg.chat.id, "")
        .reply_to_message_id(msg.id)
        .await?;
    Ok(())
}
