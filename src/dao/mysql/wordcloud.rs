use crate::settings;

use super::*;
use lazy_static::lazy_static;

lazy_static! {
    pub static ref WORD_POOL: Pool = init_mysql(&settings::SETTINGS.mysql.url.clone());
}

#[derive(Debug, PartialEq, Eq)]
pub struct Word {
    pub word: String,
    pub frequency: i32,
}

const CREATE_TABLE: &str = "
CREATE TABLE `WORD_{}` (
    word VARCHAR(255) PRIMARY KEY NOT NULL,
    frequency INT NOT NULL
);
";

const DELETE_TABLE: &str = "DROP TABLE `WORD_{}`;";

pub async fn create_table(conn: &mut ConnBufBuilder, table_name: &str) {
    exec!(
        conn.conn.clone(),
        &CREATE_TABLE.replacen("{}", table_name, 1)
    );
}

pub fn add_word(conn: &mut ConnBufBuilder, w: &str) {
    let values = format!("(\"{}\",1),", w);
    exec!(conn, values, []);
}

pub async fn get_words(pool: &Pool, table_name: &str) -> Result<Vec<Word>> {
    let mut conn = pool.get_conn().await?;
    let query = format!("SELECT word, frequency FROM `WORD_{}`", table_name);
    let words = conn
        .query_map(query, |(word, frequency)| Word { word, frequency })
        .await
        .unwrap_or(vec![]);
    drop(conn);
    Ok(words)
}

pub async fn active_group() -> std::result::Result<Vec<String>, mysql_async::Error> {
    Ok(WORD_POOL
        .get_conn()
        .await?
        .query_map("SHOW TABLES", |table_name: String| {
            table_name[5..].to_string()
        })
        .await
        .unwrap_or(vec![]))
}

pub async fn delete_table(table_name: &str) -> std::result::Result<(), mysql_async::Error> {
    Ok(WORD_POOL
        .get_conn()
        .await?
        .query_drop(&DELETE_TABLE.replacen("{}", table_name, 1))
        .await?)
}
