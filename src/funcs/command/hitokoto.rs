use super::*;

#[derive(Deserialize)]
struct Quote {
    hitokoto: String,
    from_who: Option<String>,
    from: String,
}

pub async fn hitokoto(bot: Bot, msg: Message) -> BotResult {
    let resp: Result<Quote, reqwest::Error> = get("https://v1.hitokoto.cn/").await;
    match resp {
        Err(e) => {
            let error_message = format!("{:?}", e);
            bot.send_message(msg.chat.id, error_message)
                .reply_parameters(ReplyParameters::new(msg.id))
                .await?;
        }
        Ok(quote) => {
            // let quote = resp.unwrap();
            let who = match quote.from_who {
                Some(text) => text,
                None => "".to_string(),
            };
            let message = format!("{}\n—— {}《{}》", quote.hitokoto, who, quote.from);
            bot.send_message(msg.chat.id, message)
                .reply_parameters(ReplyParameters::new(msg.id))
                .await?;
        }
    }
    Ok(())
}
