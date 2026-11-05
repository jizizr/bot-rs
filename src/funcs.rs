use crate::{
    analysis::model::{self, BotLogBuilder, MessageStatus},
    dao::mongo::analysis::insert_log,
    *,
};
use regex::Regex;
use serde::Deserialize;

pub mod command;
pub mod pkg;
pub mod text;

use pkg::escape::markdown;

/// 百分号编码 URL 组件：只保留 `A-Z a-z 0-9 - _ . ~`，与原 urlencoding 一致，
/// 已发出的回调按钮仍能按同样的规则解码。
const URL_COMPONENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

pub fn url_encode(text: &str) -> String {
    percent_encoding::utf8_percent_encode(text, URL_COMPONENT).to_string()
}

pub fn url_decode(text: &str) -> Result<std::borrow::Cow<'_, str>, std::str::Utf8Error> {
    percent_encoding::percent_decode_str(text).decode_utf8()
}

/// A client that retries its requests to `host` up to twice when they fail
/// transiently: a connection error, a timeout, or 408/429/5xx.
///
/// Uses reqwest's own retry policy, which is scoped to one host (its retry
/// budget is kept per policy) and resends without waiting in between. The
/// classifier checks the host too: reqwest consults the scope only from the
/// second retry on.
fn retry_client(builder: reqwest::ClientBuilder, host: &str) -> reqwest::Client {
    let scope = host.to_string();
    let policy = reqwest::retry::for_host(host.to_string()).classify_fn(move |attempt| {
        if attempt.uri().host() != Some(scope.as_str()) {
            return attempt.success();
        }
        let transient = match attempt.status() {
            Some(status) => {
                status.is_server_error()
                    || status == reqwest::StatusCode::REQUEST_TIMEOUT
                    || status == reqwest::StatusCode::TOO_MANY_REQUESTS
            }
            None => attempt
                .error()
                .and_then(|err| err.downcast_ref::<reqwest::Error>())
                .is_some_and(|err| err.is_connect() || err.is_timeout()),
        };
        if transient {
            attempt.retryable()
        } else {
            attempt.success()
        }
    });
    builder.retry(policy).build().unwrap()
}

fn get_name(u: &User) -> String {
    let mut name = String::new();
    if !u.first_name.is_empty() {
        name.push_str(&u.first_name);
    }
    if let Some(last_name) = &u.last_name {
        name.push(' ');
        name.push_str(last_name);
    }
    name
}

fn fmt_at(name: &str, user_id: i64) -> String {
    format!("[{}](tg://user?id={})", markdown::escape(name), user_id)
}

#[cfg(test)]
mod tests {
    use super::retry_client;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    /// A local server answering every request with `status`, counting them.
    async fn serve(status: &'static str) -> (u16, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                counter.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0; 1024];
                let _ = socket.read(&mut buf).await;
                let response =
                    format!("HTTP/1.1 {status}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        (port, hits)
    }

    #[tokio::test]
    async fn retries_transient_statuses_twice() {
        let client = retry_client(reqwest::Client::builder(), "127.0.0.1");
        let (port, hits) = serve("503 Service Unavailable").await;
        let resp = client
            .get(format!("http://127.0.0.1:{port}/"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 503);
        assert_eq!(hits.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn leaves_other_statuses_and_hosts_alone() {
        let client = retry_client(reqwest::Client::builder(), "127.0.0.1");
        let (port, hits) = serve("404 Not Found").await;
        let resp = client
            .get(format!("http://127.0.0.1:{port}/"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        let elsewhere = retry_client(reqwest::Client::builder(), "example.invalid");
        let (port, hits) = serve("503 Service Unavailable").await;
        let resp = elsewhere
            .get(format!("http://127.0.0.1:{port}/"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 503);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }
}
