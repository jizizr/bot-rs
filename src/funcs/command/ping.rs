use super::*;
use crate::{ResilientTcpStream, TcpStreamPool};
use async_once::AsyncOnce;
use clap::ValueEnum;
use dashmap::DashMap;
use futures::{future::join_all, stream::FuturesUnordered, StreamExt};
use ping_server_rs::model::*;
use std::{collections::HashMap, fmt::Write, sync::Arc};
use tokio::io::{AsyncBufReadExt, BufReader};

async fn init_hash_pool() -> HashMap<String, TcpStreamPool<ResilientTcpStream>> {
    let mut hm = HashMap::new();
    for (k, v) in SETTINGS.ping_server.iter() {
        hm.insert(k.clone(), TcpStreamPool::new(v.clone(), 3).await);
    }
    hm
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum Method {
    ICMP,
    TCP,
    HTTP,
}

impl Method {
    fn to_string(&self) -> String {
        match self {
            Method::ICMP => "ping",
            Method::TCP => "tcping",
            Method::HTTP => "http",
        }
        .to_string()
    }
}

lazy_static! {
    static ref HOST_MATCH: Regex =
        Regex::new(r#"(https?://|\s|^)(([^\x20-\x2C\x2E-\x2F\x7B-\x7E]+\.)+([^:\./\s]+))(\s|$)"#)
            .unwrap();
    static ref PING_SERVER: AsyncOnce<HashMap<String, TcpStreamPool<ResilientTcpStream>>> =
        AsyncOnce::new(init_hash_pool());
}

cmd!(
    "/ping",
    "多地测试延迟",
    PingCmd,
    {
        #[arg(value_enum)]
        /// 测试方式
        method: Method,
        /// 目标
        host: String,
        /// 端口
        #[arg(value_parser = clap::value_parser!(u16).range(1..))]
        port: Option<u16>,
        /// dns 为 A 记录
        #[arg(short='4', long)]
        v4: bool,
        /// dns 为 AAAA 记录
        #[arg(short='6', long)]
        v6: bool,
    },
    IOError(std::io::Error),
    FormatError(std::fmt::Error),
    RegexError(regex::Error),
    SerdeError(serde_json::Error),
);

fn parse_host(host: &str) -> Result<String, AppError> {
    let host = HOST_MATCH
        .captures(&host)
        .ok_or(AppError::Custom("Invalid target".to_string()))?
        .get(2)
        .unwrap()
        .as_str()
        .to_string();
    Ok(host)
}

fn into_target(ping_cmd: &PingCmd) -> Result<Target, AppError> {
    let mut req = Target::default();

    req.method = ping_cmd.method.to_string();

    // 确定 dns 记录类型
    if ping_cmd.v6 {
        req.record_type = "AAAA".to_string();
    } else {
        req.record_type = "A".to_string();
    }
    match ping_cmd.method {
        Method::ICMP => req.host = parse_host(&ping_cmd.host)?,
        Method::TCP => {
            req.port = ping_cmd.port;
            if ping_cmd.port.is_some() {
                req.host = parse_host(&ping_cmd.host)?;
                return Ok(req);
            }
            let host_port: Vec<_> = ping_cmd.host.splitn(2, ':').collect();
            if host_port.len() != 2 {
                req.host = parse_host(&ping_cmd.host)?;
                return Ok(req);
            }
            req.port = Some(
                host_port[1]
                    .parse()
                    .map_err(|_| AppError::Custom("Invalid port".to_string()))?,
            );
            req.host = parse_host(host_port[0])?;
        }
        Method::HTTP => {
            req.host = ping_cmd.host.clone();
            if !(ping_cmd.host.starts_with("http://") || ping_cmd.host.starts_with("https://")) {
                req.host = format!("http://{}", req.host)
            }
            if let Some(port) = ping_cmd.port {
                req.host.push_str(&format!(":{}", port));
            }
        }
    }
    Ok(req)
}

async fn send_json<T: ?Sized + serde::Serialize>(
    client: &mut ResilientTcpStream,
    target: &T,
) -> Result<(), AppError> {
    let mut buffer = String::with_capacity(256);
    write!(buffer, "{}\n", serde_json::to_string(target)?)?;
    client.write_all(buffer.as_bytes()).await?;
    Ok(())
}

async fn receive_json<'a, T: serde::Deserialize<'a>>(
    client: &mut ResilientTcpStream,
    buffer: &'a mut String,
) -> Result<T, AppError> {
    let mut reader = BufReader::new(&mut client.stream);

    reader
        .read_line(buffer)
        .await
        .map_err(|e| AppError::Custom(e.to_string()))?;

    serde_json::from_str(buffer).map_err(|e| {
        log::error!("{}", buffer);
        e.into()
    })
}

async fn send_receive_json<'a, T: ?Sized + serde::Serialize, R: serde::Deserialize<'a>>(
    client: &mut ResilientTcpStream,
    target: &T,
    buffer: &'a mut String,
) -> Result<R, AppError> {
    send_json(client, target).await?;
    receive_json(client, buffer).await
}

async fn get_ping(text: String) -> Result<DashMap<String, Answer>, AppError> {
    let ping_cmd = PingCmd::try_parse_from(text.to_lowercase().split_whitespace())?;
    let target = into_target(&ping_cmd)?;
    let mut futures = FuturesUnordered::new();
    let streams = Arc::new(DashMap::new());

    // 获取所有server的TcpStream
    for (k, v) in PING_SERVER.get().await.iter() {
        let streams = streams.clone();
        futures.push(tokio::spawn(async move {
            streams.insert(k.to_owned(), v.get().await);
        }));
    }

    // 等待所有TcpStream获取完成
    while let Some(result) = futures.next().await {
        if let Err(_) = result {
            return Err(AppError::Custom("内部错误".to_string()));
        }
    }
    let streams = match Arc::try_unwrap(streams) {
        Ok(v) => v,
        Err(_) => panic!(""),
    };
    let dm = DashMap::new();

    let futures: Vec<_> = {
        streams
            .iter_mut()
            .filter_map(|mut entry| {
                let k = entry.key().to_string();
                let v = entry.value_mut();

                match v {
                    Ok(client) => {
                        let target = target.clone();
                        // 将引用的生命周期扩展为 'static
                        // SAFETY: 我们确保 streams 在 join_all 完成前不会被释放
                        // 且所有 future 在 join_all 完成时都会结束
                        // 所以这里扩展引用生命周期是安全的
                        unsafe {
                            let client: &'static mut _ = std::mem::transmute(client);
                            Some(async move {
                                let mut buffer = String::with_capacity(128);
                                (
                                    k,
                                    send_receive_json::<Target, Answer>(
                                        client,
                                        &target,
                                        &mut buffer,
                                    )
                                    .await,
                                )
                            })
                        }
                    }
                    Err(_) => None,
                }
            })
            .collect()
    };

    // SAFETY: 这里完成后，所有扩展的引用生命周期都结束了
    let results = join_all(futures).await;

    // 处理结果
    for (k, result) in results {
        match result {
            Ok(answer) => {
                dm.insert(k, answer);
            }
            Err(e) => {
                if let AppError::IOError(_) = e {
                    streams.remove(&k);
                }
                let mut a = Answer::new();
                a.error = Some(e.to_string());
                dm.insert(k, a);
            }
        }
    }

    for (k, client) in streams.into_iter() {
        PING_SERVER
            .get()
            .await
            .get(&k)
            .unwrap()
            .put(match client {
                Ok(v) => v,
                Err(_) => continue,
            })
            .await;
    }
    Ok(dm)
}

pub async fn ping(bot: Bot, msg: Message) -> Result<(), AppError> {
    let text = match get_ping(getor(&msg).unwrap().to_string()).await {
        Ok(dm) => dm.into_iter().fold(String::new(), |mut acc, (k, v)| {
            acc.push_str(&match v.error {
                None => format!("{}: {:.2} ms, loss = {}% \n", k, v.avg_time, v.loss),
                Some(e) => format!("{}: {}\n", k, e),
            });
            acc
        }),
        Err(e) => e.to_string(),
    };
    bot.send_message(msg.chat.id, text).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::test;
    #[test]
    async fn test_ping_parse() {
        let icmp_v4 = Target {
            method: "ping".to_string(),
            host: "www.baidu.com".to_string(),
            port: None,
            record_type: "A".to_string(),
        };
        let icmp_v6 = Target {
            method: "ping".to_string(),
            host: "www.baidu.com".to_string(),
            port: None,
            record_type: "AAAA".to_string(),
        };
        let tcp_v4 = Target {
            method: "tcping".to_string(),
            host: "www.baidu.com".to_string(),
            port: None,
            record_type: "A".to_string(),
        };
        let tcp_v6 = Target {
            method: "tcping".to_string(),
            host: "www.baidu.com".to_string(),
            port: None,
            record_type: "AAAA".to_string(),
        };
        let tcp_v4_80 = Target {
            method: "tcping".to_string(),
            host: "www.baidu.com".to_string(),
            port: Some(80),
            record_type: "A".to_string(),
        };
        let tcp_v6_80 = Target {
            method: "tcping".to_string(),
            host: "www.baidu.com".to_string(),
            port: Some(80),
            record_type: "AAAA".to_string(),
        };
        let texts = [
            (icmp_v4.clone(), "/ping icmp www.baidu.com"),
            (icmp_v4.clone(), "/ping icmp www.baidu.com -4"),
            (icmp_v4.clone(), "/ping icmp www.baidu.com --v4"),
            (icmp_v6.clone(), "/ping icmp www.baidu.com -6"),
            (icmp_v6.clone(), "/ping icmp www.baidu.com --v6"),
            (tcp_v4.clone(), "/ping tcp www.baidu.com"),
            (tcp_v4.clone(), "/ping tcp www.baidu.com -4"),
            (tcp_v4.clone(), "/ping tcp www.baidu.com --v4"),
            (tcp_v6.clone(), "/ping tcp www.baidu.com -6"),
            (tcp_v6.clone(), "/ping tcp www.baidu.com --v6"),
            (tcp_v4_80.clone(), "/ping tcp www.baidu.com:80"),
            (tcp_v4_80.clone(), "/ping tcp www.baidu.com 80"),
            (tcp_v4_80.clone(), "/ping tcp www.baidu.com 80 -4"),
            (tcp_v4_80.clone(), "/ping tcp www.baidu.com 80 --v4"),
            (tcp_v4_80.clone(), "/ping tcp www.baidu.com:80 -4"),
            (tcp_v4_80.clone(), "/ping tcp www.baidu.com:80 --v4"),
            (tcp_v6_80.clone(), "/ping tcp www.baidu.com:80 -6"),
            (tcp_v6_80.clone(), "/ping tcp www.baidu.com:80 --v6"),
            (tcp_v6_80.clone(), "/ping tcp www.baidu.com 80 -6"),
            (tcp_v6_80.clone(), "/ping tcp www.baidu.com 80 --v6"),
        ];
        for (expect, text) in texts {
            let ping_cmd = PingCmd::try_parse_from(text.to_lowercase().split_whitespace())
                .unwrap_or_else(|e| panic!("{}", e.to_string()));
            let result = into_target(&ping_cmd).unwrap();
            println!("{}", text);
            assert_eq!(expect, result);
        }
    }
}
