use super::*;
use std::sync::LazyLock;

static CLIENT: LazyLock<ClientWithMiddleware> = LazyLock::new(|| {
    retry_client(
        reqwest::Client::builder()
            .default_headers({
                let mut headers = reqwest::header::HeaderMap::new();
                headers.insert("Content-Type", "application/json".parse().unwrap());
                headers
            })
            .build()
            .unwrap(),
        2,
    )
});
static API_URL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "https://generativelanguage.googleapis.774.gs/proxy?key={}",
        SETTINGS.gemini.key
    )
});

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

pub async fn chat(bot: &Bot, msg: &Message) -> BotResult {
    bot.send(
        SendMessage::new(
            msg.chat().id(),
            pkg::escape::markdown::escape_markdown(&get_chat(msg).await?),
        )
        .parse_mode(ParseMode::MarkdownV2)
        .link_preview_options(no_link_preview())
        .reply_parameters(reply_to(msg.message_id())),
    )
    .await?;
    Ok(())
}

async fn get_chat(msg: &Message) -> Result<String, BotError> {
    let language_tag = Some("zh-CN");
    let chat = ChatCmd::parse_i18n_from_bot(getor(msg).unwrap().split_whitespace(), language_tag)?;
    let request_body = serde_json::json!({
        "contents": [{"parts": [{"text": chat.content.join(" ")}]}]
    });
    let res = CLIENT
        .post(&*API_URL)
        .body(request_body.to_string())
        .send()
        .await?
        .json::<Root>()
        .await?;
    let content = res
        .candidates
        .first()
        .ok_or(BotError::Custom("未知错误".to_string()))?;
    if content.finish_reason != "STOP" {
        return Err(BotError::Custom(content.finish_reason.to_string()));
    }
    Ok(content
        .content
        .parts
        .iter()
        .map(|p| p.text.clone())
        .collect::<Vec<String>>()
        .join("\n\n"))
}
