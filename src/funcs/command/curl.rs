use std::collections::HashSet;

use super::*;
use reqwest::{
    Client, ClientBuilder, Response, Version,
    header::{HeaderMap, HeaderValue},
};
use scraper::{Html, Selector};
use tokio::net::TcpStream;
use tokio_native_tls::{TlsConnector, native_tls};
use x509_parser::parse_x509_certificate;

lazy_static! {
    static ref CLIENT: ClientWithMiddleware = retry_client(ClientBuilder::use_rustls_tls(ClientBuilder::new()).build().unwrap(),2);
    static ref PASTE :ClientWithMiddleware = retry_client(Client::builder().default_headers({
        let mut headers = HeaderMap::new();
        headers.insert(
            "Content-Type",
            HeaderValue::from_static("text/html"),
        );
        headers
    }).build().unwrap(),2);
    static ref MATCH: Regex = Regex::new(r#"(\s|^|https?://)(([^:\./\s]+\.)+[^\d\./:\s\\"]{2,}|((25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.){3}(25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?))(:\d{1,5})?(/\S*)*(\s|$)"#).unwrap();
    static ref CONNECTOR:TlsConnector = TlsConnector::from(native_tls::TlsConnector::new().unwrap());
}

cmd!(
    "/curl",
    "获取网页信息",
    CurlCmd,
    {
        ///网址
        #[arg(value_parser = fixer)]
        url: String,
    }
);

#[derive(Deserialize)]
struct Ip {
    continent: Option<String>,
    country: Option<String>,
    region: Option<String>,
    city: Option<String>,
    connection: Option<ASN>,
    message: Option<String>,
}

#[allow(clippy::upper_case_acronyms)]
#[derive(Deserialize)]
struct ASN {
    asn: i32,
    isp: String,
}

// paste.op.wiki 返回的数据结构
#[derive(Deserialize)]
struct Paste {
    key: String,
}

fn fixer(url: &str) -> Result<String, String> {
    let mut url = url.to_string();
    if !url.starts_with("http://") && !url.starts_with("https://") {
        url = format!("https://{}", url);
    }
    if MATCH.is_match(&url) {
        Ok(url)
    } else {
        Err(format!("不符合规则的URL\n\n{}", USAGE.as_str()))
    }
}

fn get_title(body: &str) -> Option<String> {
    // 使用 scraper 解析HTML响应
    let document = Html::parse_document(body);
    // 使用 CSS 选择器选择 <title> 标签
    let title_selector = Selector::parse("title").unwrap();
    if let Some(title_element) = document.select(&title_selector).next() {
        // 提取 <title> 标签的内容并打印
        let title_text = title_element.text().collect::<String>();
        Some(markdown::escape(title_text.trim()))
    } else {
        None
    }
}

fn get_http_version(resq: &Response) -> String {
    let version = resq.version();
    match version {
        Version::HTTP_09 => "HTTP/0\\.9",
        Version::HTTP_10 => "HTTP/1\\.0",
        Version::HTTP_11 => "HTTP/1\\.1",
        Version::HTTP_2 => "HTTP/2",
        Version::HTTP_3 => "HTTP/3",
        _ => "",
    }
    .to_string()
}

async fn get_ip_info(ip: &str) -> String {
    let ip_info: Result<Ip, reqwest::Error> = get(&format!("http://ipwho.is/{ip}")).await;
    match ip_info {
        Ok(ip) if ip.message.is_some() => {
            if ip.message.as_ref().unwrap() == "Invalid IP address"
                || ip.message.unwrap() == "Reserved range"
            {
                format!(
                    "*Location:* {}\n*Announced By:* {}",
                    "Reserved range", "Reserved range"
                )
            } else {
                "API到达限额".to_string()
            }
        }
        Ok(ip) => format!(
            "*Location:* {}\n*Announced By:* {}",
            markdown::escape(&format!(
                "{} {} {} {}",
                ip.continent.unwrap_or(String::new()),
                ip.country.unwrap_or(String::new()),
                ip.region.unwrap_or(String::new()),
                ip.city.unwrap_or(String::new())
            )),
            markdown::escape(&format!(
                "AS{} {}",
                ip.connection.as_ref().unwrap().asn,
                ip.connection.as_ref().unwrap().isp
            ))
        ),
        Err(e) => {
            log::error!("{e}");
            "".to_string()
        }
    }
}

async fn get_ssl(url: &str) -> Result<String, AppError> {
    // 连接到远程服务器
    let stream = CONNECTOR
        .connect(url, TcpStream::connect(format!("{url}:443")).await?)
        .await?;

    // 获取服务器证书
    let certificate = stream.get_ref().peer_certificate().unwrap();

    // 输出证书的 DER 编码
    let buffer = certificate.unwrap().to_der()?.to_vec();
    let (_, cert) = parse_x509_certificate(&buffer)?;

    // 获取证书信息
    let subject = cert.tbs_certificate.subject;
    let issuer = cert.tbs_certificate.issuer;
    let valid_from = cert.tbs_certificate.validity.not_before;
    let valid_until = cert.tbs_certificate.validity.not_after;

    Ok(format!(
        "*Subject:* {}\n*Issuer:* {}\n*Valid from:* {}\n*Valid until:* {}",
        markdown::escape(&subject.to_string()),
        markdown::escape(&issuer.to_string()),
        markdown::escape(&valid_from.to_string()),
        markdown::escape(&valid_until.to_string())
    ))
}

async fn get_resp(url: &str) -> Result<Response, AppError> {
    let resp = CLIENT.get(url).send().await?.error_for_status();
    // 如果请求失败，尝试使用 http 协议请求
    let resp = match resp {
        Ok(res) if res.status().is_success() => res,
        _ => CLIENT
            .get(format!("http{}", url.trim_start_matches("https")))
            .send()
            .await?
            .error_for_status()?,
    };
    Ok(resp)
}

async fn get_header(resp: &Response) -> Result<String, AppError> {
    let mut header = String::new();
    let mut hash_set = HashSet::new();
    resp.headers().iter().for_each(|(k, v)| {
        let hn = k.as_str();
        if hn.contains("cookie") {
            return;
        }
        if !hash_set.insert(hn) {
            return;
        }
        header.push_str(&format!(
            "*{}:* {}\n",
            markdown::escape(k.as_str()),
            markdown::escape(v.to_str().unwrap())
        ));
    });
    Ok(header)
}

async fn post_paste(text: String) -> Result<String, AppError> {
    let resp: Paste = PASTE
        .post("https://s.op.wiki/data/post")
        .body(text.trim().to_string())
        .send()
        .await?
        .json()
        .await?;
    Ok(format!("https://paste\\.op\\.wiki/{}", resp.key))
}

async fn get_curl(msg: &Message) -> Result<String, AppError> {
    let curl = CurlCmd::try_parse_from(getor(msg).unwrap().split_whitespace()).map_err(ccerr!())?;
    let url = curl.url;
    let resp = get_resp(&url).await?;
    let ip = &resp.remote_addr().unwrap().ip().to_string();
    let header = get_header(&resp).await?;
    let url = resp.url().clone();
    let version = get_http_version(&resp);
    let body = resp.text().await?;
    let title = match get_title(&body) {
        Some(s) => format!("*Page Title: *{}", s),
        None => String::new(),
    };
    let (ssl, ip_info, paste_url) = if url.scheme() == "https" {
        tokio::join!(
            get_ssl(url.host_str().unwrap()),
            get_ip_info(ip),
            post_paste(body)
        )
    } else {
        let temp = tokio::join!(get_ip_info(ip), post_paste(body));
        (Ok("".to_string()), temp.0, temp.1)
    };
    let result = format!(
        "
*HTTP Request Summary*
{}
{title}

▼ *Server Info:*
{}
*IP Address:*{}
{ip_info}

*▼ Headers:*
{version}
{header}

*▼ Body:*
{}",
        markdown::escape(url.as_ref()),
        ssl.unwrap_or_else(|e| e.to_string()),
        markdown::escape(ip),
        paste_url.unwrap_or_else(|e| e.to_string()),
    );
    Ok(result)
}

pub async fn curl(bot: &Bot, msg: &Message) -> BotResult {
    tokio::spawn(bot.send_chat_action(msg.chat.id, ChatAction::Typing).send());
    match get_curl(msg).await {
        Ok(text) => {
            bot.send_message(msg.chat.id, text)
                .reply_parameters(ReplyParameters::new(msg.id))
                .link_preview_options(LinkPreviewOptions {
                    is_disabled: true,
                    url: None,
                    prefer_small_media: false,
                    prefer_large_media: false,
                    show_above_text: false,
                })
                .parse_mode(ParseMode::MarkdownV2)
                .send()
                .await?;
        }
        Err(e) => {
            bot.send_message(msg.chat.id, e.to_string())
                .reply_parameters(ReplyParameters::new(msg.id))
                .send()
                .await?;
        }
    }
    Ok(())
}
