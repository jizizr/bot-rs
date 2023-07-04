use super::*;
use serde::Deserialize;
use std::error::Error;

// use ferrisgram::error::Result;
#[derive(Deserialize)]
struct Quote {
    hitokoto: String,
    from_who: Option<String>,
    from: String,
}

pub async fn quote(bot: Bot, ctx: Context) -> ferrisgram::error::Result<GroupIteration> {
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

// async fn get_quote() -> std::result::Result<Quote, Box<dyn Send + Sync + std::error::Error>> {
//     let response = reqwest::get("https://international.v1.hitokoto.cn/").await?;
//     let quote: Quote = response.json().await?;
//     Ok(quote)
// }
