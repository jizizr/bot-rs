use super::*;

#[derive(Deserialize)]
struct Quote {
    hitokoto: String,
    from_who: Option<String>,
    from: String,
}

pub async fn quote(bot: Bot, ctx: Context) -> FResult<GroupIteration> {
    let msg = ctx.effective_message.unwrap();
    let resp: Result<Quote, Box<dyn Error + Send + Sync>> = get("https://v1.hitokoto.cn/").await;
    match resp {
        Err(e) => {
            let error_message = format!("{:?}", e);
            msg.reply(&bot, &error_message).send().await?;
        }
        Ok(quote) => {
            // let quote = resp.unwrap();
            let who = match quote.from_who {
                Some(text) => text,
                None => "".to_string(),
            };
            let message = format!("{}\n—— {}《{}》", quote.hitokoto, who, quote.from);
            msg.reply(&bot, &message).send().await?;
        }
    }
    Ok(GroupIteration::EndGroups)
}
