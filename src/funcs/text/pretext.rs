use super::*;
use crate::dao::mysql::wordcloud::*;
use pkg::jieba::cut::text_cut;

pub async fn pretext(_bot: &Bot, msg: &Message) -> BotResult {
    if msg.edit_date().is_some() {
        return Ok(());
    }
    let text = getor(msg).unwrap();
    let words = text_cut(text);
    add_words(msg.chat.id.0, words).await?;
    Ok(())
}
