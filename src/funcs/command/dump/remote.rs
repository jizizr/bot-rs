//! 远程文件的随机读取：每次读都是一个 HTTP Range 请求。

use super::*;
use async_trait::async_trait;
use bytes::Bytes;
use reqwest::{StatusCode, header};
use std::time::Duration;

/// 能按偏移读取的字节源。线上是 HTTP Range，测试里是内存。
#[async_trait]
pub trait Source: Send + Sync {
    /// 总长度
    fn size(&self) -> u64;
    /// 读 `[offset, offset + len)`，必须正好返回 `len` 字节
    async fn read(&self, offset: u64, len: u64) -> Result<Bytes, BotError>;
}

/// 下载用的客户端：不设总超时（大块可能要读一阵），只限连接和读空闲
static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(60))
        .tcp_nodelay(true)
        .build()
        .expect("dump client creation failed")
});

/// 每个 Range 请求的尝试次数
const ATTEMPTS: usize = 3;

pub struct Remote {
    url: String,
    size: u64,
    /// 打开时顺带读到的文件末尾，zip 目录多半就在里面
    pub tail: Bytes,
}

impl Remote {
    /// 打开远程文件：读最后 `tail_len` 字节，同时从 Content-Range 拿到总长度。
    /// 服务器不支持 Range（不回 206）就没法只取一部分，直接报错
    pub async fn open(url: &str, tail_len: u64) -> Result<Self, BotError> {
        let resp = with_retry(|| {
            CLIENT
                .get(url)
                .header(header::RANGE, format!("bytes=-{tail_len}"))
                .send()
        })
        .await?;
        match resp.status() {
            StatusCode::PARTIAL_CONTENT => {}
            StatusCode::OK => {
                return Err(BotError::Custom(
                    "服务器不支持分段下载（Range），无法只提取单个镜像".to_string(),
                ));
            }
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND | StatusCode::GONE => {
                return Err(BotError::Custom(format!(
                    "链接打不开（{}），签名链接可能已过期",
                    resp.status()
                )));
            }
            status => return Err(BotError::Custom(format!("链接打不开（{status}）"))),
        }
        let size = resp
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(total_from_content_range)
            .ok_or_else(|| BotError::Custom("服务器没有返回文件总长度".to_string()))?;
        let tail = resp.bytes().await?;
        Ok(Self {
            url: url.to_string(),
            size,
            tail,
        })
    }
}

/// `bytes 0-99/1234` -> 1234
fn total_from_content_range(value: &str) -> Option<u64> {
    value.rsplit_once('/')?.1.trim().parse().ok()
}

async fn with_retry<F, Fut>(mut send: F) -> Result<reqwest::Response, BotError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<reqwest::Response, reqwest::Error>>,
{
    let mut last = None;
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(500 * attempt as u64)).await;
        }
        match send().await {
            Ok(resp) if resp.status().is_server_error() => {
                last = Some(BotError::Custom(format!("服务器出错（{}）", resp.status())));
            }
            Ok(resp) => return Ok(resp),
            Err(err) => last = Some(err.into()),
        }
    }
    Err(last.expect("at least one attempt"))
}

#[async_trait]
impl Source for Remote {
    fn size(&self) -> u64 {
        self.size
    }

    async fn read(&self, offset: u64, len: u64) -> Result<Bytes, BotError> {
        if len == 0 {
            return Ok(Bytes::new());
        }
        if offset + len > self.size {
            return Err(BotError::Custom(
                "读取超出文件末尾，文件可能已损坏".to_string(),
            ));
        }
        // 末尾那段打开时已经读过了
        let tail_start = self.size - self.tail.len() as u64;
        if offset >= tail_start {
            let start = (offset - tail_start) as usize;
            return Ok(self.tail.slice(start..start + len as usize));
        }
        let range = format!("bytes={}-{}", offset, offset + len - 1);
        let mut last = None;
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(500 * attempt as u64)).await;
            }
            let resp =
                with_retry(|| CLIENT.get(&self.url).header(header::RANGE, &range).send()).await?;
            if resp.status() != StatusCode::PARTIAL_CONTENT {
                return Err(BotError::Custom(format!(
                    "分段下载失败（{}），签名链接可能已过期",
                    resp.status()
                )));
            }
            // 读到一半断开就整段重来
            match resp.bytes().await {
                Ok(body) if body.len() as u64 == len => return Ok(body),
                Ok(body) => {
                    last = Some(BotError::Custom(format!(
                        "分段下载长度不对：要 {len} 字节，收到 {}",
                        body.len()
                    )));
                }
                Err(err) => last = Some(err.into()),
            }
        }
        Err(last.expect("at least one attempt"))
    }
}

/// 内存里的字节源，测试用
#[cfg(test)]
pub struct Memory(pub Bytes);

#[cfg(test)]
#[async_trait]
impl Source for Memory {
    fn size(&self) -> u64 {
        self.0.len() as u64
    }

    async fn read(&self, offset: u64, len: u64) -> Result<Bytes, BotError> {
        let end = offset
            .checked_add(len)
            .filter(|end| *end <= self.size())
            .ok_or_else(|| BotError::Custom("读取超出文件末尾".to_string()))?;
        Ok(self.0.slice(offset as usize..end as usize))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_total_from_content_range() {
        assert_eq!(total_from_content_range("bytes 7000-7999/8000"), Some(8000));
        assert_eq!(total_from_content_range("bytes */8000"), Some(8000));
        assert_eq!(total_from_content_range("bytes 0-1/*"), None);
    }
}
