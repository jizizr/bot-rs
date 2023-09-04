use super::*;
use lazy_static::lazy_static;

lazy_static! {
    pub static ref WORD_POOL: Pool = init_mysql("mysql://zr:zr@localhost:3306/wordcloud");
}

const CREATE_TABLE: &str = "
CREATE TABLE word_frequency (
    word VARCHAR(255) PRIMARY KEY NOT NULL,
    frequency INT NOT NULL
);
";

const INSERT_WORD: &str = r#"
INSERT INTO word_frequency (word, frequency) VALUES ("?", 1) ON DUPLICATE KEY UPDATE frequency = frequency + 1;
"#;

pub async fn create_table(conn: &mut ConnBuf) {
    exec!(conn, CREATE_TABLE);
}

pub async fn add_word(conn: &mut ConnBuf, w: &str) {
    exec!(conn, INSERT_WORD, [w]);
}
