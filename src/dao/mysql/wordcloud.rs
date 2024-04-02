use crate::settings;

use super::*;

lazy_static! {
    static ref WORD_POOL: Pool = init_mysql(&settings::SETTINGS.db.mysql.url);
}

#[derive(Debug, PartialEq, Eq)]
pub struct Word {
    pub word: String,
    pub frequency: i32,
}

pub struct UserFrequency {
    pub user_id: u64,
    pub name: String,
    pub frequency: i32,
}

pub async fn add_words(group_id: i64, words: Vec<String>) -> BotResult {
    if words.is_empty() {
        return Ok(());
    }
    let repeat = format!("({},?,1)", group_id);
    let values_str = vec![repeat; words.len()].join(", ");
    let params = Params::Positional(words.into_iter().map(Into::into).collect());
    let sql = format!(
        "INSERT INTO `words` (group_id, word, count) VALUES {} ON DUPLICATE KEY UPDATE count = count + 1",
        values_str
    );
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

pub async fn active_group() -> Result<Vec<i64>> {
    WORD_POOL
        .get_conn()
        .await?
        .query("SELECT DISTINCT group_id FROM words")
        .await
}

async fn clear_words() -> BotResult {
    WORD_POOL
        .get_conn()
        .await?
        .query_drop("TRUNCATE TABLE words")
        .await?;
    Ok(())
}

pub async fn add_user(group_id: i64, name: String, user_id: u64) -> BotResult {
    let sql = "INSERT INTO `users` (group_id, user_id, name, count) VALUES (?, ?, ?, ?) ON DUPLICATE KEY UPDATE count = count + 1,name = VALUES(name)";
    let params = Params::Positional(vec![group_id.into(), user_id.into(), name.into(), 1.into()]);
    WORD_POOL.get_conn().await?.exec_drop(sql, params).await?;
    Ok(())
}

pub async fn get_users(group_id: i64) -> Result<Vec<UserFrequency>> {
    Ok(WORD_POOL
        .get_conn()
        .await?
        .query_map(
            format!(
                "SELECT user_id, name, count FROM `users` WHERE group_id = {} ORDER BY count DESC LIMIT 5",
                group_id
            ),
            |(user_id, name, frequency)| UserFrequency {
                user_id,
                name,
                frequency,
            },
        )
        .await
        .unwrap_or(vec![]))
}

async fn clear_users() -> BotResult {
    WORD_POOL
        .get_conn()
        .await?
        .query_drop("TRUNCATE TABLE users")
        .await?;
    Ok(())
}

pub async fn clear() -> BotResult {
    let (e1, e2) = tokio::join!(clear_words(), clear_users());
    e1.and(e2)
}
