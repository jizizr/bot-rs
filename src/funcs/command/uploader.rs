use super::*;

/// Where the uploader bot's token is kept for MHub's uploads.
const UPLOADER_TOKEN_PATH: &str = "/etc/music-gateway/uploader-token";

/// `/uploader <token>`, owner only, private chat only: stores the uploader
/// bot's token on the server so the owner can set it from a phone. The
/// message carrying the token is deleted right away and the token is never
/// echoed back.
pub async fn uploader(bot: &Bot, msg: &Message) -> BotResult {
    let from_owner = msg
        .from()
        .is_some_and(|user| user.id == crate::settings::SETTINGS.bot.owner);
    if !from_owner || !matches!(msg.chat(), Chat::Private(_)) {
        return Ok(());
    }
    let _ = bot
        .send(DeleteMessage::new(msg.chat().id(), msg.message_id()))
        .await;
    let token = getor(msg)
        .and_then(|text| text.split_whitespace().nth(1))
        .filter(|token| {
            token.split_once(':').is_some_and(|(id, secret)| {
                !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) && secret.len() >= 30
            })
        });
    let reply = match token {
        Some(token) => match save_token(token) {
            Ok(()) => "上传 bot 的 token 已保存（你的消息已删除）".to_string(),
            Err(err) => format!("保存失败：{err}"),
        },
        None => "格式：/uploader <bot token>".to_string(),
    };
    bot.send(SendMessage::new(msg.chat().id(), reply)).await?;
    Ok(())
}

fn save_token(token: &str) -> std::io::Result<()> {
    use std::{fs, io::Write, os::unix::fs::OpenOptionsExt};
    let temp = format!("{UPLOADER_TOKEN_PATH}.new");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temp)?;
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    fs::rename(temp, UPLOADER_TOKEN_PATH)
}
