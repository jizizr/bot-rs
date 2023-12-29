use crate::settings;

use super::*;

lazy_static! {
    static ref CLIENT: ClientWithMiddleware = retry_client(
        reqwest::Client::builder()
            .default_headers({
                let mut headers = reqwest::header::HeaderMap::new();
                headers.insert("Content-Type", "application/json".parse().unwrap());
                headers
            })
            .build()
            .unwrap(),
        2
    );
    static ref API_URL: String = format!(
        "https://generativelanguage.googleapis.774.gs/proxy?key={}",
        settings::SETTINGS.gemini.key
    );
}

cmd!(
    "/chat",
    "Ai聊天",
    ChatCmd ,
    {
        ///聊天内容
        #[arg(required = true)]
        content: Vec<String>,
    }
);

#[derive(Deserialize)]
struct Root {
    candidates: Vec<Candidate>,
}

#[derive(Deserialize)]
struct Candidate {
    content: Content,
    #[serde(rename = "finishReason")]
    finish_reason: String,
}

#[derive(Deserialize)]
struct Content {
    parts: Vec<Part>,
}

#[derive(Deserialize)]
struct Part {
    text: String,
}

pub async fn chat(bot: Bot, msg: Message) -> BotResult {
    match get_chat(&msg).await {
        Ok(text) => bot
            .send_message(msg.chat.id, pkg::escape::markdown::escape_markdown(&text))
            .parse_mode(ParseMode::MarkdownV2),
        Err(e) => bot.send_message(msg.chat.id, format!("{e}").replace(&*API_URL, "")),
    }
    .disable_web_page_preview(true)
    .reply_to_message_id(msg.id)
    .await?;
    Ok(())
}

async fn get_chat(msg: &Message) -> Result<String, AppError> {
    let chat = ChatCmd::try_parse_from(getor(&msg).unwrap().split_whitespace())?;
    let request_body = format!(
        r#"{{"contents":[{{"parts":[{{"text":"{}"}}]}}]}}"#,
        chat.content.join(" ")
    );
    let res = CLIENT
        .post(&*API_URL)
        .body(request_body)
        .send()
        .await?
        .json::<Root>()
        .await?;
    let content = res
        .candidates
        .first()
        .ok_or(AppError::CustomError("未知错误".to_string()))?;
    if content.finish_reason != "STOP" {
        return Err(AppError::CustomError(content.finish_reason.to_string()));
    }
    Ok(content
        .content
        .parts
        .iter()
        .map(|p| p.text.clone())
        .collect::<Vec<String>>()
        .join("\n\n"))
}
