use crate::settings;
use std::sync::LazyLock;

use super::*;

static WORD_POOL: LazyLock<Pool> = LazyLock::new(|| init_mysql(&settings::SETTINGS.db.mysql.url));

#[derive(Debug, PartialEq, Eq)]
pub struct Word {
    pub word: String,
    pub frequency: usize,
}

pub struct UserFrequency {
    pub user_id: u64,
    pub name: String,
    pub frequency: i32,
}

/// 词云最多用到的词数，`get_words` 只取这么多。
pub const MAX_WORDS: usize = 200;

/// 合并同一条消息里的重复词，并按词排序。
///
/// 每个事务都按 `(group_id, word)` 同一顺序加锁，同群并发消息不会互相死锁；
/// 重复词也只写一行。
fn tally(words: Vec<String>) -> Vec<(String, u32)> {
    let mut counts = std::collections::BTreeMap::new();
    for word in words {
        *counts.entry(word).or_insert(0u32) += 1;
    }
    counts.into_iter().collect()
}

pub async fn add_words(group_id: i64, words: Vec<String>) -> BotResult {
    let words = tally(words);
    if words.is_empty() {
        return Ok(());
    }
    let values_str = vec!["(?,?,?)"; words.len()].join(", ");
    let mut params = Vec::with_capacity(words.len() * 3);
    for (word, count) in words {
        params.extend([group_id.into(), word.into(), count.into()]);
    }
    let sql = format!(
        "INSERT INTO `words` (group_id, word, count) VALUES {values_str} AS new \
         ON DUPLICATE KEY UPDATE count = `words`.count + new.count"
    );
    WORD_POOL
        .get_conn()
        .await?
        .exec_drop(sql, Params::Positional(params))
        .await?;
    Ok(())
}

pub async fn get_words(group_id: i64) -> Result<Vec<Word>> {
    WORD_POOL
        .get_conn()
        .await?
        .query_map(
            format!(
                "SELECT word, count FROM `words` WHERE group_id = {group_id} \
                 ORDER BY count DESC, word LIMIT {MAX_WORDS}"
            ),
            |(word, frequency)| Word { word, frequency },
        )
        .await
}

pub async fn active_group() -> Result<Vec<i64>> {
    WORD_POOL
        .get_conn()
        .await?
        // 有发言但一个词都没分出来的群只在 `users` 里，也要发发言统计
        .query("SELECT group_id FROM words UNION SELECT group_id FROM users")
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
    WORD_POOL
        .get_conn()
        .await?
        .query_map(
            format!(
                "SELECT user_id, name, count FROM `users` WHERE group_id = {group_id} ORDER BY count DESC LIMIT 5"
            ),
            |(user_id, name, frequency)| UserFrequency { user_id, name, frequency },
        )
        .await
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tally_merges_duplicates_in_word_order() {
        let words = ["天气", "吃饭", "天气", "摸鱼", "天气", "吃饭"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            tally(words),
            vec![
                ("吃饭".to_string(), 2),
                ("天气".to_string(), 3),
                ("摸鱼".to_string(), 1),
            ]
        );
        assert!(tally(Vec::new()).is_empty());
    }
}
