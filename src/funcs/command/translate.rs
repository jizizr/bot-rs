use super::*;
use serde_json::Value;

lazy_static! {
    static ref CLIENT: ClientWithMiddleware =
        retry_client(reqwest::Client::builder().build().unwrap(), 2);
}

cmd!(
    "/translate",
    "翻译",
    TranslateCmd,
    {
        ///翻译内容
        #[arg(required = true)]
        content: Vec<String>,
    }
);

fn extract_data(json_data: Value) -> Result<Vec<(String, String)>, AppError> {
    let extracted = json_data
        .as_array()
        .and_then(|array| array.get(0)?.as_array())
        .map(|first_element| {
            first_element
                .iter()
                .filter_map(|item| {
                    if let Value::Array(inner_array) = item {
                        let first = inner_array.get(0)?.as_str()?;
                        let second = inner_array.get(1)?.as_str()?;
                        Some((first.to_string(), second.to_string()))
                    } else {
                        None
                    }
                })
                .collect()
        })
        .ok_or(AppError::CustomError("Invalid structure".to_string()))?;

    Ok(extracted)
}

async fn translate_req(tl: &str, text: &str, is_compare: bool) -> Result<String, AppError> {
    let url_str = format!(
        "https://translate.googleapis.com/translate_a/single?client=gtx&sl=auto&tl={}&dt=t&q={}",
        tl,
        urlencoding::encode(text),
    );
    Ok({
        let data = extract_data(
            reqwest::Client::new()
                .get(url_str)
                .send()
                .await?
                .json::<Value>()
                .await
                .unwrap(),
        )?;
        let data_iter = data.iter();
        if is_compare {
            data_iter
                .map(|(a, b)| format!("{}\n{}", b, a))
                .collect::<Vec<String>>()
        } else {
            data_iter
                .map(|(_, b)| format!("{}", b))
                .collect::<Vec<String>>()
        }
        .join("\n")
    })
}

async fn get_translate(msg: &Message) -> Result<String, AppError> {
    let translate = TranslateCmd::try_parse_from(getor(&msg).unwrap().split_whitespace())?;
    let text = translate.content.join(" ");
    let tl = "zh-CN";
    let translated = translate_req(tl, &text, true).await?;
    Ok(translated)
}

pub async fn translate(bot: Bot, msg: Message) -> BotResult {
    match get_translate(&msg).await {
        Ok(text) => bot.send_message(msg.chat.id, text),
        Err(e) => bot.send_message(msg.chat.id, format!("{e}")),
    }
    .reply_to_message_id(msg.id)
    .await?;
    Ok(())
}
