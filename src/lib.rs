use crate::settings::SETTINGS;
use async_trait::async_trait;
use funcs::{command::command_handler, text::text_handler};
use lazy_static::lazy_static;
use myclap::clap::MyErrorFormatter;
use serde::de::DeserializeOwned;
use std::{collections::VecDeque, fs::File, io::Read, time::Duration};
use teloxide::{Bot, prelude::*, types::Me};
use tokio::{
    io::{self, AsyncWriteExt},
    net::TcpStream,
    sync::Mutex,
    time::timeout,
};
pub mod analysis;
pub mod dao;
pub mod filter;
pub mod funcs;
pub mod myclap;
pub mod settings;

pub type BotResult = Result<(), BotError>;
#[derive(Debug, thiserror::Error)]
pub enum BotError {
    #[error("API请求失败: {0}")]
    Request(#[from] reqwest::Error),
    #[error("API请求失败: {0}")]
    Retry(#[from] reqwest_middleware::Error),
    #[error("{}\n\n{}", 
    .0,
    .1)]
    Clap(clap::error::Error<MyErrorFormatter>, &'static String),
    #[error("{}", .0)]
    Custom(String),
    #[error("{}", .0)]
    Send(#[from] teloxide::RequestError),
    #[error("{}", .0)]
    IOError(#[from] std::io::Error),
    #[error("{}", .0)]
    FormatError(#[from] std::fmt::Error),
    #[error("{}", .0)]
    RegexError(regex::Error),
    #[error("{}", .0)]
    SerdeError(#[from] serde_json::Error),
    #[error("{}", .0)]
    UrlParseError(#[from] url::ParseError),
    #[error("{}", .0)]
    MySQLError(#[from] mysql_async::Error),
    #[error("{}", .0)]
    JoinError(#[from] tokio::task::JoinError),
    #[error("{}", .0)]
    ImageError(#[from] image::ImageError),
    #[error("{}", .0)]
    RedisError(#[from] redis::RedisError),
    #[error("{}", .0)]
    TlsError(#[from] tokio_native_tls::native_tls::Error),
    #[error("{}", .0)]
    SslError(#[from] x509_parser::nom::Err<x509_parser::error::X509Error>),
    #[error("{}", .0)]
    BotCommandParseError(#[from] teloxide::utils::command::ParseError),
    #[error("{}", .0)]
    MongoError(#[from] mongodb::error::Error),
    #[error("{}", .0)]
    PaintError(#[from] charts_rs::CanvasError),
}

#[macro_export]
macro_rules! ccerr {
    () => {
        |e| clap_err!(e)
    };
}

#[macro_export]
macro_rules! clap_err {
    ($e:expr) => {
        BotError::Clap($e.apply::<MyErrorFormatter>(), &USAGE)
    };
}

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

pub async fn msg_handler(bot: Bot, msg: Message, me: Me) -> BotResult {
    if command_handler(&bot, &msg, &me).await.is_ok() {
        return Ok(());
    }
    text_handler(&bot, &msg).await?;
    Ok(())
}
