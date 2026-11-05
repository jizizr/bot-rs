use crate::settings::SETTINGS;
use async_trait::async_trait;
use funcs::{command::command_handler, text::text_handler};
use myclap::clap::MyErrorFormatter;
use serde::de::DeserializeOwned;
use std::{
    collections::VecDeque,
    fs::File,
    io::Read,
    sync::{LazyLock, OnceLock},
    time::Duration,
};
pub use telers::{
    Bot,
    enums::ParseMode,
    methods::*,
    types::{
        CallbackQuery, Chat, ChosenInlineResult, InlineKeyboardButton, InlineKeyboardMarkup,
        InlineQuery, InputFile, MaybeInaccessibleMessage, Message, ReplyParameters, User,
    },
};
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
    /// MHub's music gateway; the text is already written for users.
    #[error("{0}")]
    MusicGateway(#[from] music_gateway::GatewayError),
    #[error("API请求失败: {0}")]
    Request(#[from] reqwest::Error),
    #[error("API请求失败: {0}")]
    Retry(#[from] reqwest_middleware::Error),
    #[error("{}\n\n{}", 
        .0,
        .1)]
    Clap(clap::error::Error<MyErrorFormatter>, &'static String),
    #[error("{}\n\n{}", 
        .0,
        .1)]
    Clap2(
        clap::error::Error<clap_i18n_richformatter::ClapI18nRichFormatter>,
        String,
    ),
    #[error("{}", .0)]
    Custom(String),
    /// 限流/排队等瞬时拒绝：只弹提示，不应破坏已有消息或菜单
    #[error("{}", .0)]
    Busy(String),
    #[error("{}", .0)]
    Send(#[from] telers::errors::SessionErrorKind),
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
    SslError(#[from] x509_parser::nom::Err<x509_parser::error::X509Error>),
    #[error("{}", .0)]
    MongoError(#[from] mongodb::error::Error),
    #[error("{}", .0)]
    PaintError(#[from] charts_rs::Error),
}

pub static BOT: LazyLock<Bot> = LazyLock::new(|| {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(300))
        .tcp_nodelay(true)
        .build()
        .expect("telegram client creation failed");
    let session = telers::client::Reqwest::new(client);
    let session = match SETTINGS
        .bot
        .api_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        Some(api_url) => {
            let api_url = api_url.trim_end_matches('/');
            reqwest::Url::parse(api_url).expect("invalid telegram bot api_url");
            session.with_api_server(std::borrow::Cow::Owned(
                telers::client::telegram::APIServer::new(
                    &format!("{api_url}/bot{{token}}/{{method_name}}"),
                    &format!("{api_url}/file/bot{{token}}/{{path}}"),
                    false,
                    telers::client::telegram::BareFilesPathWrapper,
                ),
            ))
        }
        None => session,
    };
    Bot::with_client(&SETTINGS.bot.token, session)
});
pub static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);

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
            // 超时视为连接失败，跳过该连接而不是 panic
            if let Ok(Ok(s)) = timeout(Duration::from_secs(3), S::connect(&addr)).await {
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

/// Shows "typing…" in the chat while a slow command works; best effort.
pub fn show_typing(bot: &Bot, chat_id: i64) {
    let bot = bot.clone();
    tokio::spawn(async move { bot.send(SendChatAction::new(chat_id, "typing")).await });
}

pub fn callback_button(
    text: impl Into<Box<str>>,
    data: impl Into<Box<str>>,
) -> InlineKeyboardButton {
    InlineKeyboardButton::new(text).callback_data(data)
}

pub fn url_button(text: impl Into<Box<str>>, url: url::Url) -> InlineKeyboardButton {
    InlineKeyboardButton::new(text).url(String::from(url))
}

/// Starts an inline query in the current chat.
pub fn switch_inline_current_chat_button(
    text: impl Into<Box<str>>,
    query: impl Into<Box<str>>,
) -> InlineKeyboardButton {
    InlineKeyboardButton::new(text).switch_inline_query_current_chat(query)
}

/// The rows of a keyboard, to add more.
pub fn keyboard_rows(markup: InlineKeyboardMarkup) -> Vec<Vec<InlineKeyboardButton>> {
    markup
        .inline_keyboard
        .into_vec()
        .into_iter()
        .map(Vec::from)
        .collect()
}

/// Telegram refuses an edit that would change nothing.
pub fn is_not_modified(err: &BotError) -> bool {
    matches!(
        err,
        BotError::Send(telers::errors::SessionErrorKind::Telegram(
            telers::errors::TelegramErrorKind::BadRequest { message },
        )) if message.contains("message is not modified")
    )
}

/// Sends a message without a link preview.
pub fn no_link_preview() -> telers::types::LinkPreviewOptions {
    telers::types::LinkPreviewOptions::new().is_disabled(true)
}

/// Replies to `message_id` in the same chat.
pub fn reply_to(message_id: i64) -> ReplyParameters {
    ReplyParameters::new().message_id(message_id)
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
    let resp = HTTP_CLIENT.get(url).send().await?;
    let model: T = resp.json().await?;
    Ok(model)
}

/// The bot's own account, fetched once at startup.
pub static ME: OnceLock<User> = OnceLock::new();

pub fn me() -> &'static User {
    ME.get()
        .expect("bot identity is fetched before handling updates")
}

fn chat_info(chat: &Chat) -> String {
    format!(
        "群组ID：{}\n群组名称：@{}\n群组标题：{}",
        chat.id(),
        chat.username().unwrap_or_default(),
        chat.title().unwrap_or_default()
    )
}

pub async fn msg_handler(bot: Bot, msg: Message) -> BotResult {
    let me = me();
    match &msg {
        Message::LeftChatMember(left) => {
            if me.id != left.left_chat_member.id {
                return Ok(());
            }
            bot.send(SendMessage::new(
                SETTINGS.bot.owner,
                format!("被群组移出：\n{}", chat_info(msg.chat())),
            ))
            .await?;
        }
        Message::NewChatMembers(joined) => {
            if !joined.new_chat_members.iter().any(|u| u.id == me.id) {
                return Ok(());
            }
            bot.send(SendMessage::new(
                SETTINGS.bot.owner,
                format!("被群组邀请：\n{}", chat_info(msg.chat())),
            ))
            .await?;
        }
        _ => {}
    };
    if command_handler(&bot, &msg, me).await.is_ok() {
        return Ok(());
    }
    text_handler(&bot, &msg).await
}
