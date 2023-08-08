use super::*;

pub async fn id(bot: Bot, msg: Message) -> Result<(), Box<dyn Error + Send + Sync>> {
    bot.send_message(
        msg.chat.id,
        format!("您的id是 `{}`", msg.from().unwrap().id),
    )
    .parse_mode(ParseMode::MarkdownV2)
    .reply_to_message_id(msg.id)
    .await?;
    Ok(())
}
