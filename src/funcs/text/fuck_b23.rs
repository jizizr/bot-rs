use super::*;
use regex::Regex;
lazy_static! {
    static ref MATCH: Regex = Regex::new(r#"(https?://)?b23.tv/\w+"#).unwrap();
    static ref CLIENT: reqwest::Client = reqwest::Client::new();
}

pub async fn fuck_b23(bot: &Bot, msg: &Message) -> BotResult {
    if let Some(s) = MATCH.find(getor(msg).unwrap()) {
        let r = CLIENT.get(s.as_str()).send().await?.error_for_status()?;
        bot.send_message(
            msg.chat.id,
            format!(
                "已经帮你去除b23的跟踪链接：\nhttps://www.bilibili.com{}",
                r.url().path()
            ),
        )
        .send()
        .await?;
    }

    Ok(())
}
