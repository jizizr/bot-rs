use super::*;
use crate::dao::mysql::wordcloud::*;
use crate::dao::mysql::GetConnBuf;

pub async fn test(_bot: Bot, msg: Message) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut conn = WORD_POOL.get_conn_buf().await?;
    add_word(&mut conn, getor(&msg).unwrap()).await;
    if let Err(mysql_async::Error::Server(mysql_err)) = conn.run().await {
        if mysql_err.code == 1146 {
            let mut conn = WORD_POOL.get_conn_buf().await?;
            create_table(&mut conn).await;
            add_word(&mut conn, getor(&msg).unwrap()).await;
            conn.run().await?
        }
    }
    Ok(())
}
