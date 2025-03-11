use super::*;

pub async fn id(bot: &Bot, msg: &Message) -> BotResult {
    bot.send_message(
        msg.chat.id,
        format!("您的id是 `{}`", msg.from.as_ref().unwrap().id),
    )
    .parse_mode(ParseMode::MarkdownV2)
    .reply_parameters(ReplyParameters::new(msg.id))
    .await?;
    Ok(())
}
