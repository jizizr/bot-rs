use std::{collections::HashSet, sync::LazyLock};

use super::*;
use crate::funcs::pkg::rich::{RichPage, html_escape};
use reqwest::{
    Client, ClientBuilder, Response, Version,
    header::{HeaderMap, HeaderValue},
};
use scraper::{Html, Selector};
use tokio::net::TcpStream;
use tokio_native_tls::{TlsConnector, native_tls};
use x509_parser::parse_x509_certificate;

static CLIENT: LazyLock<ClientWithMiddleware> = LazyLock::new(|| {
    retry_client(
        ClientBuilder::use_rustls_tls(ClientBuilder::new())
            .build()
            .unwrap(),
        2,
    )
});
static PASTE: LazyLock<ClientWithMiddleware> = LazyLock::new(|| {
    retry_client(
        Client::builder()
            .default_headers({
                let mut headers = HeaderMap::new();
                headers.insert("Content-Type", HeaderValue::from_static("text/html"));
                headers
            })
            .build()
            .unwrap(),
        2,
    )
});
static MATCH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(\s|^|https?://)(([^:\./\s]+\.)+[^\d\./:\s\\"]{2,}|((25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.){3}(25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?))(:\d{1,5})?(/\S*)*(\s|$)"#).unwrap()
});
static CONNECTOR: LazyLock<TlsConnector> =
    LazyLock::new(|| TlsConnector::from(native_tls::TlsConnector::new().unwrap()));

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
        url = format!("https://{url}");
    }
    if MATCH.is_match(&url) {
        Ok(url)
    } else {
        Err(format!(
            "不符合规则的URL\n\n{}",
            CurlCmd::get_usage_i18n_with_language_tag(None)
        ))
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
        Some(title_text.trim().to_string())
    } else {
        None
    }
}

fn html_link(label: &str, url: &str) -> String {
    format!(
        "<a href=\"{}\">{}</a>",
        html_escape(url),
        html_escape(label)
    )
}

fn get_http_version(resq: &Response) -> String {
    let version = resq.version();
    match version {
        Version::HTTP_09 => "HTTP/0.9",
        Version::HTTP_10 => "HTTP/1.0",
        Version::HTTP_11 => "HTTP/1.1",
        Version::HTTP_2 => "HTTP/2",
        Version::HTTP_3 => "HTTP/3",
        _ => "",
    }
    .to_string()
}

/// Where an address is and which network announces it, as table rows.
async fn get_ip_info(ip: &str) -> Vec<(&'static str, String)> {
    let ip_info: Result<Ip, reqwest::Error> = get(&format!("http://ipwho.is/{ip}")).await;
    match ip_info {
        Ok(ip) if ip.message.is_some() => {
            let message = ip.message.unwrap_or_default();
            if message == "Invalid IP address" || message == "Reserved range" {
                vec![("位置", "保留地址".to_string())]
            } else {
                vec![("位置", "查询额度已用完".to_string())]
            }
        }
        Ok(ip) => {
            let place = [ip.continent, ip.country, ip.region, ip.city]
                .into_iter()
                .flatten()
                .filter(|part| !part.trim().is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let mut rows = vec![("位置", html_escape(&place))];
            if let Some(connection) = ip.connection {
                rows.push((
                    "网络",
                    html_escape(&format!("AS{} {}", connection.asn, connection.isp)),
                ));
            }
            rows
        }
        Err(e) => {
            log::error!("{e}");
            Vec::new()
        }
    }
}

/// The certificate the server presents, as table rows.
async fn get_ssl(url: &str) -> Result<Vec<(&'static str, String)>, BotError> {
    // 连接到远程服务器
    let stream = CONNECTOR
        .connect(url, TcpStream::connect(format!("{url}:443")).await?)
        .await?;

    // 获取服务器证书
    let certificate = stream
        .get_ref()
        .peer_certificate()?
        .ok_or_else(|| BotError::Custom("服务器未提供证书".to_string()))?;

    // 输出证书的 DER 编码
    let buffer = certificate.to_der()?.to_vec();
    let (_, cert) = parse_x509_certificate(&buffer)?;

    // 获取证书信息
    let subject = cert.tbs_certificate.subject;
    let issuer = cert.tbs_certificate.issuer;
    let valid_from = cert.tbs_certificate.validity.not_before;
    let valid_until = cert.tbs_certificate.validity.not_after;

    Ok(vec![
        ("主体", html_escape(&subject.to_string())),
        ("签发者", html_escape(&issuer.to_string())),
        ("生效", html_escape(&valid_from.to_string())),
        ("到期", html_escape(&valid_until.to_string())),
    ])
}

async fn get_resp(url: &str) -> Result<Response, BotError> {
    let resp = CLIENT.get(url).send().await?.error_for_status();
    // 如果请求失败，尝试使用 http 协议请求（仅当原始 URL 为 https，避免拼出 "httphttp://"）
    let resp = match resp {
        Ok(res) if res.status().is_success() => res,
        _ if url.starts_with("https://") => CLIENT
            .get(url.replacen("https://", "http://", 1))
            .send()
            .await?
            .error_for_status()?,
        Ok(res) => res,
        Err(e) => return Err(e.into()),
    };
    Ok(resp)
}

/// Response headers one per line, cookies left out, and how many there are.
async fn get_header(resp: &Response) -> Result<(usize, String), BotError> {
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
            "{}: {}\n",
            k.as_str(),
            String::from_utf8_lossy(v.as_bytes())
        ));
    });
    Ok((hash_set.len(), header))
}

async fn post_paste(text: String) -> Result<String, BotError> {
    let resp: Paste = PASTE
        .post("https://paste.op.wiki/data/post")
        .body(text.trim().to_string())
        .send()
        .await?
        .json()
        .await?;
    Ok(format!("https://paste.op.wiki/{}", resp.key))
}

/// A page summarising one request: what answered, from where, its
/// certificate and headers folded away, and the body on Paste.
async fn get_curl(msg: &Message) -> Result<RichPage, BotError> {
    let language_tag = Some("zh-CN");
    let curl = CurlCmd::parse_i18n_from_bot(getor(msg).unwrap().split_whitespace(), language_tag)?;
    let resp = get_resp(&curl.url).await?;
    let ip = resp
        .remote_addr()
        .map(|addr| addr.ip().to_string())
        .unwrap_or_default();
    let (header_count, header) = get_header(&resp).await?;
    let url = resp.url().clone();
    let status = resp.status();
    let version = get_http_version(&resp);
    let body = resp.text().await?;
    let title = get_title(&body).filter(|title| !title.is_empty());
    let (ssl, ip_info, paste_url) = if url.scheme() == "https" {
        let (ssl, ip_info, paste) = tokio::join!(
            get_ssl(url.host_str().unwrap()),
            get_ip_info(&ip),
            post_paste(body)
        );
        (Some(ssl), ip_info, paste)
    } else {
        let (ip_info, paste) = tokio::join!(get_ip_info(&ip), post_paste(body));
        (None, ip_info, paste)
    };

    let host = url.host_str().unwrap_or(url.as_str());
    let mut rows = vec![
        ("状态", html_escape(&format!("{version} {status}"))),
        ("IP", format!("<code>{}</code>", html_escape(&ip))),
    ];
    rows.extend(ip_info);
    let mut page = RichPage::default()
        .heading(&format!("🌐 {}", title.as_deref().unwrap_or(host)))
        .paragraph_html(&html_link(url.as_str(), url.as_str()))
        .fields_html(rows);
    match ssl {
        Some(Ok(certificate)) => {
            page = page.details("🔒 证书", RichPage::default().fields_html(certificate));
        }
        Some(Err(err)) => {
            page = page.details("🔒 证书", RichPage::default().paragraph(&err.to_string()));
        }
        None => {}
    }
    page = page.details(
        &format!("📋 响应头（{header_count} 条）"),
        RichPage::default().pre(&header),
    );
    Ok(match paste_url {
        Ok(paste_url) => {
            page.paragraph_html(&format!("📄 {}", html_link("查看完整响应内容", &paste_url)))
        }
        Err(err) => page.paragraph(&format!("📄 响应内容上传失败：{err}")),
    })
}

pub async fn curl(bot: &Bot, msg: &Message) -> BotResult {
    show_typing(bot, msg.chat().id());
    bot.send(
        SendRichMessage::new(msg.chat().id(), get_curl(msg).await?.build())
            .reply_parameters(reply_to(msg.message_id())),
    )
    .await?;
    Ok(())
}
