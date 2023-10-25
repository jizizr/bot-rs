use super::*;
use regex::Regex;
use reqwest::{Client, Response, Version};
use scraper::{Html, Selector};
use tokio::net::TcpStream;
use tokio_native_tls::{native_tls, TlsConnector};
use x509_parser::parse_x509_certificate;

lazy_static! {
    static ref USAGE: String = CurlCmd::command().render_help().to_string();
    static ref CLIENT: Client = Client::new();
    static ref MATCH: Regex = Regex::new(r#"(\s|^|https?://)(([^:\./\s]+\.)+[^\d\./:\s\\"]{2,}|((25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.){3}(25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?))(:\d{1,5})?(/\S*)*(\s|$)"#).unwrap();
    static ref CONNECTOR:TlsConnector = TlsConnector::from(native_tls::TlsConnector::new().unwrap());
}

#[derive(Parser)]
#[command(
    help_template = "使用方法：{usage}\n\n{all-args}\n\n{about}",
    about = "命令功能：访问网站",
    name = "/curl",
    next_help_heading = "参数解释",
    disable_help_flag = true
)]
struct CurlCmd {
    ///网址
    #[arg(value_parser = fixer)]
    url: String,
}

#[derive(Deserialize)]
struct Ip {
    continent: Option<String>,
    country: Option<String>,
    region: Option<String>,
    city: Option<String>,
    connection: Option<ASN>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct ASN {
    asn: i32,
    isp: String,
}

fn fixer(url: &str) -> Result<String, String> {
    let mut url = url.to_string();
    if !url.starts_with("http://") && !url.starts_with("https://") {
        url = format!("https://{}", url);
    }
    if MATCH.is_match(&url) {
        Ok(url)
    } else {
        Err(format!("不符合规则的URL\n\n{}", USAGE.to_string()))
    }
}

fn get_title(body: &String) -> Option<String> {
    // 使用 scraper 解析HTML响应
    let document = Html::parse_document(&body);
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

async fn get_ssl(url: &str) -> Result<String, BotError> {
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

async fn get_curl(msg: &Message) -> Result<String, BotError> {
    let curl = CurlCmd::try_parse_from(getor(&msg).unwrap().split_whitespace())?;
    let url = curl.url;
    let resp = CLIENT.get(&url).send().await?.error_for_status();
    // 如果请求失败，尝试使用 http 协议请求
    let resp = match resp {
        Ok(res) if res.status().is_success() => res,
        _ => CLIENT
            .get(format!("http{}", url.trim_start_matches("https")))
            .send()
            .await?
            .error_for_status()?,
    };
    let ip = markdown::escape(&resp.remote_addr().unwrap().ip().to_string());
    let mut header = String::new();
    resp.headers().iter().for_each(|(k, v)| {
        let hn = k.as_str();
        if hn.contains("cookie") {
            return;
        }
        header.push_str(&format!(
            "*{}:* {}\n",
            markdown::escape(k.as_str()),
            markdown::escape(v.to_str().unwrap())
        ));
    });
    let url = resp.url().clone();
    let (ssl, ip_info) = if url.scheme() == "https" {
        tokio::join!(get_ssl(url.host_str().unwrap()), get_ip_info(&ip))
    } else {
        (Ok("".to_string()), get_ip_info(&ip).await)
    };
    let version = get_http_version(&resp);
    let body = resp.text().await?;
    let mut result = format!(
        "*HTTP Request Summary*\n{}\n",
        markdown::escape(&url.to_string())
    );
    let title = match get_title(&body) {
        Some(title) => format!("*Page Title: *{}", title),
        None => String::new(),
    };
    result.push_str(&title);
    result.push_str(&format!(
        "\n\n▼ *Server Info:*\n{}\n*IP Address:*{ip}\n{ip_info}\n\n*▼ Headers:\n*{version}\n{header}",
        ssl.unwrap_or_else(|e| e.to_string()),
    ));
    Ok(result)
}

pub async fn curl(bot: Bot, msg: Message) -> Result<(), BotError> {
    match get_curl(&msg).await {
        Ok(text) => {
            bot.send_message(msg.chat.id, text)
                .reply_to_message_id(msg.id)
                .parse_mode(ParseMode::MarkdownV2)
                .send()
                .await?;
        }
        Err(e) => {
            bot.send_message(msg.chat.id, e.to_string())
                .reply_to_message_id(msg.id)
                .send()
                .await?;
        }
    }
    Ok(())
}
