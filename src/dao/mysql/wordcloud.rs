use crate::settings;

use super::*;
use lazy_static::lazy_static;

lazy_static! {
    static ref WORD_POOL: Pool = init_mysql(&settings::SETTINGS.mysql.url);
}

#[derive(Debug, PartialEq, Eq)]
pub struct Word {
    pub word: String,
    pub frequency: i32,
}

pub async fn add_words(group_id: i64, words: Vec<String>) -> BotResult {
    let repeat = format!("({},?,1)", group_id);
    let values_str = vec![repeat; words.len()].join(", ");
    let params = Params::Positional(words.into_iter().map(Into::into).collect());
    let sql = format!(
        "INSERT INTO `words` (group_id, word, count) VALUES {} ON DUPLICATE KEY UPDATE count = count + 1",
        values_str
    );

    // 使用参数执行批量操作
    WORD_POOL.get_conn().await?.exec_drop(sql, params).await?;
    Ok(())
}

pub async fn get_words(group_id: i64) -> Result<Vec<Word>> {
    Ok(WORD_POOL
        .get_conn()
        .await?
        .query_map(
            format!(
                "SELECT word, count FROM `words` WHERE group_id = {}",
                group_id
            ),
            |(word, frequency)| Word { word, frequency },
        )
        .await
        .unwrap_or(vec![]))
}

pub async fn active_group() -> std::result::Result<Vec<i64>, mysql_async::Error> {
    Ok(WORD_POOL
        .get_conn()
        .await?
        .query("SELECT DISTINCT group_id FROM words")
        .await?)
}

pub async fn clear_words() -> BotResult {
    WORD_POOL
        .get_conn()
        .await?
        .query_drop("TRUNCATE TABLE words")
        .await?;
    Ok(())
}
