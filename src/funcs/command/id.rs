use super::*;

pub async fn id(bot: &Bot, msg: &Message) -> BotResult {
    bot.send(
        SendMessage::new(
            msg.chat().id(),
            format!("您的id是 `{}`", msg.from().unwrap().id),
        )
        .parse_mode(ParseMode::MarkdownV2)
        .reply_parameters(reply_to(msg.message_id())),
    )
    .await?;
    Ok(())
}
