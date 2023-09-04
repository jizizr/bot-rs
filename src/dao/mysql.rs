use async_trait::async_trait;
use mysql_async::{prelude::Queryable, *};
use crate::exec;

pub mod wordcloud;

pub struct ConnBuf {
    conn: Conn,
    buffer: String,
}

impl ConnBuf {
    pub fn exec(&mut self, sql: String) {
        self.buffer.push_str(&sql)
    }
    pub async fn run(mut self) -> Result<()> {
        self.conn.query_drop(self.buffer).await?;
        Ok(())
    }
}

#[macro_export]
macro_rules! exec {
    // Handle the case where only the query string is provided
    ($conn:expr,$query:expr) => {
        $conn.exec($query.to_string());
    };
    // Handle the case where both query string and replacements are provided
    ($conn:expr,$query:expr, [$($replacement:expr),*]) => {
        {
            let mut result = String::new();
            let mut replacement_iter = vec![$($replacement),*].into_iter();

            for char in $query.chars() {
                if char == '?' {
                    if let Some(replacement) = replacement_iter.next() {
                        result.push_str(replacement);
                    } else {
                        result.push('?');
                    }
                } else {
                    result.push(char);
                }
            }

            $conn.exec(result);
        }
    };
}

#[async_trait]
pub trait GetConnBuf {
    async fn get_conn_buf(&self) -> Result<ConnBuf>;
}

#[async_trait]
impl GetConnBuf for Pool {
    async fn get_conn_buf(&self) -> Result<ConnBuf> {
        let conn = self.get_conn().await?;
        Ok(ConnBuf {
            conn,
            buffer: String::new(),
        })
    }
}
fn init_mysql(url: &str) -> Pool {
    Pool::new(url)
}
