use super::*;
use crate::dao::{mysql::wordcloud::*, rdb::wordcloud::*};
use pkg::jieba::cut::text_cut;

pub async fn pretext(_bot: &Bot, msg: &Message) -> BotResult {
    if msg.edit_date().is_some() {
        return Ok(());
    }
    let text = getor(msg).unwrap();
    let words = text_cut(text);
    let group_id = msg.chat.id.0;
    let user = msg.from.as_ref().unwrap();
    let mut name = get_name(user);
    if name.chars().take(6).count() == 6 {
        name = name.split('|').next().unwrap().to_string();
    }
    if name.chars().take(6).count() == 6 {
        name = name.split(' ').next().unwrap().to_string();
    }
    if name.chars().take(6).count() == 6 {
        name = name.chars().take(6).collect();
    }
    let (e1, e2, e3) = tokio::join!(
        add_words(group_id, words),
        wc_switch(group_id, true),
        add_user(group_id, name, user.id.0)
    );
    e1.and(e2).and(e3)
}
