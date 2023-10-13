use std::sync::Arc;

use crate::exec;
use async_trait::async_trait;
use mysql_async::{prelude::Queryable, *};
use tokio::sync::Mutex;

pub mod wordcloud;

pub struct ConnBufBuilder {
    begin: String,
    end: String,
    conn: Arc<Mutex<Conn>>,
    buffer: String,
}

impl ConnBufBuilder {
    pub fn exec(&mut self, sql: String) {
        self.buffer.push_str(&sql)
    }

    pub async fn run(&mut self){
        let _ = self.conn.clone().lock().await.query_drop(&self.buffer).await;
    }
    
    pub fn build(&mut self) -> ConnBuf {
        self.buffer.pop();
        self.buffer = format!("{}{}{}", self.begin, self.buffer, self.end);
        println!("{}", self.buffer);
        ConnBuf {
            conn: self.conn.clone(),
            buffer: &self.buffer,
        }
    }
}

pub struct ConnBuf<'a> {
    conn: Arc<Mutex<Conn>>,
    buffer: &'a str,
}

impl ConnBuf<'_> {
    pub async fn run(&mut self) -> Result<()> {
        self.conn
            .clone()
            .lock()
            .await
            .query_drop(self.buffer)
            .await?;
        Ok(())
    }
}

#[macro_export]
macro_rules! exec {
    // Handle the case where only the query string is provided
    ($conn:expr,$query:expr) => {
        println!("{}", $query);
        let _ = ConnBuf{
            conn: $conn,
            buffer: $query,
        }.run().await;
    };
    // Handle the case where both query string and replacements are provided
    ($conn:expr, $query:expr, [$($replacement:expr),*]) => {
        {
            let mut result = String::new();
            let mut query_iter = $query.chars();
            'out: for re in vec![$($replacement),*].into_iter() {
                loop {
                    match query_iter.next() {
                        Some('?') => {
                            result.push_str(re);
                            break;
                        }
                        Some(c) => result.push(c),
                        None => break 'out,
                    }
                }
            }
            result.push_str(query_iter.as_str());
            $conn.exec(result);
        }
    };
}

#[async_trait]
pub trait GetConnBuf<'a> {
    async fn get_conn_buf(&self, begin: &'a str, end: &'a str) -> Result<ConnBufBuilder>;
}

#[async_trait]
impl<'a> GetConnBuf<'a> for Pool {
    async fn get_conn_buf(&self, begin: &'a str, end: &'a str) -> Result<ConnBufBuilder> {
        let conn = self.get_conn().await?;
        Ok(ConnBufBuilder {
            begin: begin.to_string(),
            end: end.to_string(),
            conn: Arc::new(Mutex::new(conn)),
            buffer: String::new(),
        })
    }
}

fn init_mysql(url: &str) -> Pool {
    Pool::new(url)
}
