use super::*;
use crate::dao::mysql::{wordcloud::*, GetConnBuf};
use pkg::jieba::cut::text_cut;

const WCLOUD_END: &str = " ON DUPLICATE KEY UPDATE frequency = frequency + 1;";

pub async fn pretext(_bot: &Bot, msg: &Message) -> BotResult {
    if msg.edit_date().is_some() {
        return Ok(());
    }
    let mut conn = WORD_POOL
        .get_conn_buf(
            &format!(
                "INSERT INTO `WORD_{}` (word, frequency) VALUES ",
                msg.chat.id.0
            ),
            WCLOUD_END,
        )
        .await?;
    let text = getor(msg).unwrap();
    let words = text_cut(text);
    if words.is_empty() {
        return Ok(());
    }
    for w in words.iter() {
        add_word(&mut conn, w);
    }
    match conn.build().run().await {
        Err(mysql_async::Error::Server(mysql_err)) if mysql_err.code == 1146 => {
            create_table(&mut conn, &msg.chat.id.to_string()).await;
            conn.run().await;
        }
        Err(mysql_async::Error::Server(mysql_err)) => {
            return Err(Box::new(mysql_err));
        }
        _ => {}
    }
    
    Ok(())
}
