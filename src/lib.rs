use crate::settings::SETTINGS;
use async_trait::async_trait;
use lazy_static::lazy_static;
use serde::de::DeserializeOwned;
use std::{collections::VecDeque, error::Error, fs::File, io::Read, time::Duration};
use teloxide::{Bot, prelude::*};
use tokio::{
    io::{self, AsyncWriteExt},
    net::TcpStream,
    sync::Mutex,
    time::timeout,
};
pub mod dao;
pub mod filter;
pub mod funcs;
pub mod settings;

pub type BotError = Box<dyn Error + Send + Sync>;
pub type BotResult = Result<(), BotError>;

lazy_static! {
    pub static ref BOT: Bot = Bot::new(&SETTINGS.bot.token);
}

#[async_trait]
pub trait Stream {
    type Output;
    type Error;

    async fn connect(addr: &str) -> Result<Self::Output, Self::Error>;
}

pub struct ResilientTcpStream {
    address: String,
    pub stream: TcpStream,
    max_retries: usize,
}

#[async_trait]
impl Stream for ResilientTcpStream {
    type Output = Self; // 使用Self代替ResilientTcpStream以简化
    type Error = io::Error;

    async fn connect(addr: &str) -> Result<Self, io::Error> {
        let stream = TcpStream::connect(addr).await?;
        Ok(Self {
            address: addr.to_string(),
            stream,
            max_retries: 3,
        })
    }
}

impl ResilientTcpStream {
    pub fn retries(&mut self, max_retries: usize) {
        self.max_retries = max_retries;
    }

    async fn reconnect(&mut self) -> io::Result<()> {
        let mut retries = 0;
        loop {
            match TcpStream::connect(&self.address).await {
                Ok(new_stream) => {
                    self.stream = new_stream;
                    return Ok(());
                }
                Err(e) => {
                    if retries > self.max_retries {
                        return Err(e);
                    }
                    retries += 1;
                    tokio::time::sleep(Duration::from_millis(100)).await; // 等待一秒再重连
                }
            }
        }
    }

    pub async fn write_all(&mut self, data: &[u8]) -> Result<(), io::Error> {
        match self.stream.write_all(data).await {
            Ok(_) => Ok(()),
            Err(e)
                if e.kind() == io::ErrorKind::BrokenPipe
                    || e.kind() == io::ErrorKind::ConnectionReset =>
            {
                self.reconnect().await?;
                self.stream.write_all(data).await
            }
            Err(e) => Err(e),
        }
    }
}

pub struct TcpStreamPool<S> {
    pool: Mutex<VecDeque<S>>,
    addr: String,
}

impl<S> TcpStreamPool<S>
where
    S: Stream<Output = S, Error = io::Error> + Send + Sync + 'static,
{
    pub async fn new(addr: String, num: u16) -> Self {
        let mut pool = VecDeque::new();
        let address = addr.clone();
        for _ in 0..num {
            if let Ok(s) = timeout(Duration::from_secs(3), S::connect(&addr))
                .await
                .unwrap()
            {
                pool.push_back(s);
            }
        }
        Self {
            pool: Mutex::new(pool),
            addr: address,
        }
    }

    // 从池中获取一个TcpStream，如果池为空，则创建一个新的
    pub async fn get(&self) -> tokio::io::Result<S>
    where
        S: Stream<Output = S, Error = io::Error>,
    {
        let mut pool = self.pool.lock().await;
        if let Some(stream) = pool.pop_front() {
            Ok(stream)
        } else {
            drop(pool);
            S::connect(&self.addr).await
        }
    }

    // 将一个TcpStream放回池中
    pub async fn put(&self, stream: S) {
        let mut pool = self.pool.lock().await;
        pool.push_back(stream);
    }
}

pub fn getor(msg: &Message) -> Option<&str> {
    msg.text().or(msg.caption())
}

pub fn load_json<T: DeserializeOwned>(path: &str) -> T {
    let mut file = File::open(path).unwrap_or_else(|_| panic!("找不到 {path}"));
    let mut json_data = String::new();
    file.read_to_string(&mut json_data)
        .unwrap_or_else(|_| panic!("读取 {path} 失败"));
    // 解析 JSON 文件
    serde_json::from_str(&json_data).expect("JSON 数据解析失败")
}

pub async fn get<T: DeserializeOwned>(url: &str) -> Result<T, reqwest::Error> {
    let resp = reqwest::get(url).await?;
    let model: T = resp.json().await?;
    Ok(model)
}
