use super::*;
use lazy_static::lazy_static;

lazy_static! {
    pub static ref WORD_POOL: Pool = init_mysql("mysql://root@127.0.0.1:3306/wordcloud");
}

#[derive(Debug, PartialEq, Eq)]
pub struct Word {
    pub word: String,
    pub frequency: i32,
}

const CREATE_TABLE: &str = "
CREATE TABLE `{}` (
    word VARCHAR(255) PRIMARY KEY NOT NULL,
    frequency INT NOT NULL
);
";

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
    let query = format!("SELECT word, frequency FROM `{}`", table_name);
    let words = conn.query_map(query, |(word, frequency)| Word { word, frequency }).await?;
    drop(conn);
    Ok(words)
}