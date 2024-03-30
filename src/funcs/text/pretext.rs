use super::*;
use crate::dao::{mysql::wordcloud::*, rdb::wordcloud::*};
use pkg::jieba::cut::text_cut;

pub async fn pretext(_bot: &Bot, msg: &Message) -> BotResult {
    if msg.edit_date().is_some() {
        return Ok(());
    }
    let text = getor(msg).unwrap();
    let words = text_cut(text);
    let (e1, e2) = tokio::join!(
        add_words(msg.chat.id.0, words),
        wc_switch(msg.chat.id.0, true)
    );
    e1.and(e2)
}
