use super::*;

#[derive(Deserialize)]
struct Quote {
    hitokoto: String,
    from_who: Option<String>,
    from: String,
}

pub async fn hitokoto(bot: &Bot, msg: &Message) -> BotResult {
    let resp: Quote = get("https://v1.hitokoto.cn/").await?;
    bot.send(
        SendMessage::new(
            msg.chat().id(),
            format!(
                "{}\n—— {}《{}》",
                resp.hitokoto,
                match resp.from_who {
                    Some(text) => text,
                    None => "".to_string(),
                },
                resp.from
            ),
        )
        .reply_parameters(reply_to(msg.message_id())),
    )
    .await?;
    Ok(())
}
