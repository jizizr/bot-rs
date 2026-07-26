use super::*;

// 频道歌曲被自动转发进评论区（讨论群）时 Telegram 会剥离内联按钮，
// 这里委托 music 模块识别自家歌曲并在评论区补发一份操作按钮
pub async fn comment_buttons(bot: &Bot, msg: &Message) -> BotResult {
    crate::funcs::command::music::comment_buttons(bot, msg).await
}
