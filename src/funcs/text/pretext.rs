use super::*;
use crate::dao::mysql::{wordcloud::*, GetConnBuf};
use pkg::jieba::cut::text_cut;

const WCLOUD_END: &str = " ON DUPLICATE KEY UPDATE frequency = frequency + 1;";

pub async fn pretext(_bot: &Bot, msg: &Message) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut conn = WORD_POOL
        .get_conn_buf(
            &format!("INSERT INTO `{}` (word, frequency) VALUES ", msg.chat.id.0),
            WCLOUD_END,
        )
        .await?;
    let text = getor(&msg).unwrap();

    for w in text_cut(text).iter() {
        add_word(&mut conn, w);
    }
    if let Err(e) = conn.build().run().await {
        if let mysql_async::Error::Server(mysql_err) = e {
            if mysql_err.code == 1146 {
                create_table(&mut conn, &msg.chat.id.to_string()).await;
                conn.run().await
            } else {
                return Err(Box::new(mysql_err));
            }
        }
    }
    Ok(())
}
