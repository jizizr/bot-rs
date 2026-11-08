//! `/dump <卡刷包链接> [分区...]`：用 HTTP Range 只拉 zip 里目标分区那几 MB，
//! 不下载整个包。A/B 包从 payload.bin 里解，老式卡刷包直接取 zip 里的 `*.img`。
//! 不给分区名时列出包里所有分区。提取过的镜像按 file_id 缓存在 MongoDB。

use super::*;
use crate::{
    dao::mongo::rom_dumps::{self, RomDump},
    funcs::pkg::escape::html::escape,
    settings::SETTINGS,
};
use bytes::Bytes;
use payload::{PartitionUpdate, Payload, sha256_file};
use remote::{Remote, Source};
use std::{
    fmt::Write as _,
    io::Write as _,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use zip::Entry;

mod payload;
mod remote;
mod zip;

/// 本地 Bot API 的上传上限
const MAX_IMAGE: u64 = 2000 << 20;
/// 进度消息的刷新间隔
const PROGRESS_EVERY: Duration = Duration::from_secs(3);
/// 大文件上传可能要好几分钟
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// 老式卡刷包里直接存放的镜像，按块读取
const CHUNK: u64 = 8 << 20;

cmd!(
    "/dump",
    "从卡刷包（zip）链接里只提取指定分区镜像，不下载整个包",
    DumpCmd,
    {
        ///卡刷包 zip 的直链
        url: String,
        ///分区名，例如 init_boot boot vendor_boot；不填则列出所有分区
        partitions: Vec<String>,
    }
);

/// 包里能提取的东西：A/B 包的 payload，或者 zip 里直接放的 img
enum Package {
    Payload(Payload),
    Plain(Vec<Entry>),
}

/// 一个待提取的分区
enum Target<'a> {
    Payload(&'a PartitionUpdate),
    Plain(&'a Entry),
}

impl Target<'_> {
    fn size(&self) -> u64 {
        match self {
            Target::Payload(part) => part.size(),
            Target::Plain(entry) => entry.size,
        }
    }

    fn download_size(&self) -> u64 {
        match self {
            Target::Payload(part) => part.download_size(),
            Target::Plain(entry) => entry.compressed_size,
        }
    }
}

impl Package {
    async fn open(src: &Remote) -> Result<Self, BotError> {
        let entries = zip::entries(src, &src.tail).await?;
        if let Some(entry) = entries.iter().find(|e| e.name == "payload.bin") {
            if entry.method != zip::STORED {
                return Err(BotError::Custom(
                    "payload.bin 被压缩存放，无法按偏移读取".to_string(),
                ));
            }
            let offset = entry.data_offset(src).await?;
            return Ok(Package::Payload(Payload::open(src, offset).await?));
        }
        let images: Vec<_> = entries
            .into_iter()
            .filter(|e| {
                e.name.to_ascii_lowercase().ends_with(".img")
                    && matches!(e.method, zip::STORED | zip::DEFLATED)
            })
            .collect();
        if images.is_empty() {
            return Err(BotError::Custom(
                "包里没有 payload.bin，也没有 .img 镜像（线刷包 tgz 暂不支持）".to_string(),
            ));
        }
        Ok(Package::Plain(images))
    }

    fn find(&self, name: &str) -> Option<Target<'_>> {
        match self {
            Package::Payload(payload) => payload.partition(name).map(Target::Payload),
            Package::Plain(entries) => entries
                .iter()
                .find(|e| plain_name(&e.name).eq_ignore_ascii_case(name))
                .map(Target::Plain),
        }
    }

    /// 分区清单（名字，大小）
    fn list(&self) -> Vec<(String, u64)> {
        match self {
            Package::Payload(payload) => payload
                .partitions
                .iter()
                .map(|p| (p.name().to_string(), p.size()))
                .collect(),
            Package::Plain(entries) => entries
                .iter()
                .map(|e| (plain_name(&e.name).to_string(), e.size))
                .collect(),
        }
    }
}

/// `firmware-update/vbmeta.img` -> `vbmeta`
fn plain_name(path: &str) -> &str {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.get(..file.len() - 4).unwrap_or(file)
}

fn human_size(size: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{size} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// 缓存键：去掉签名参数（`?t=..&s=..` 会过期）的包地址 + 分区名
fn cache_key(url: &url::Url, partition: &str) -> String {
    format!(
        "{}{}#{}",
        url.host_str().unwrap_or_default(),
        url.path(),
        partition.to_ascii_lowercase()
    )
}

/// 包的文件名，放在说明里
fn package_name(url: &url::Url) -> String {
    url.path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|name| !name.is_empty())
        .map(|name| {
            url_decode(name)
                .map(|n| n.into_owned())
                .unwrap_or(name.to_string())
        })
        .unwrap_or_else(|| url.host_str().unwrap_or_default().to_string())
}

fn caption(partition: &str, package: &str, size: u64, sha256: &str) -> String {
    format!(
        "<b>{}.img</b>\n包：<code>{}</code>\n大小：{}\nSHA256：<code>{}</code>",
        escape(partition),
        escape(package),
        human_size(size),
        sha256
    )
}

/// 进度消息：发一条，之后反复改它，最后删掉
struct Status {
    bot: Bot,
    chat_id: i64,
    message_id: i64,
}

impl Status {
    async fn send(bot: &Bot, msg: &Message, text: &str) -> Result<Self, BotError> {
        let sent = bot
            .send(
                SendMessage::new(msg.chat().id(), text)
                    .reply_parameters(reply_to(msg.message_id())),
            )
            .await?;
        Ok(Self {
            bot: bot.clone(),
            chat_id: msg.chat().id(),
            message_id: sent.message_id(),
        })
    }

    async fn set(&self, text: impl Into<String>) {
        let _ = self
            .bot
            .send(
                EditMessageText::new()
                    .text(text.into())
                    .chat_id(self.chat_id)
                    .message_id(self.message_id),
            )
            .await;
    }

    async fn set_html(&self, text: String) -> BotResult {
        self.bot
            .send(
                EditMessageText::new()
                    .text(text)
                    .chat_id(self.chat_id)
                    .message_id(self.message_id)
                    .parse_mode(ParseMode::HTML)
                    .link_preview_options(no_link_preview()),
            )
            .await?;
        Ok(())
    }

    async fn delete(&self) {
        let _ = self
            .bot
            .send(DeleteMessage::new(self.chat_id, self.message_id))
            .await;
    }
}

/// 临时目录，用完即删
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> std::io::Result<Self> {
        let path = std::env::temp_dir()
            .join("bot-rs-dump")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub async fn dump(bot: &Bot, msg: &Message) -> BotResult {
    let cmd = DumpCmd::parse_i18n_from_bot(getor(msg).unwrap().split_whitespace(), Some("zh-CN"))?;
    let url = url::Url::parse(&cmd.url)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
        .ok_or_else(|| BotError::Custom("请给出卡刷包的 http(s) 直链".to_string()))?;
    let package = package_name(&url);

    // 没给分区名就列出所有分区
    let listing = cmd.partitions.is_empty();
    // 缓存里有的直接发
    let mut pending = Vec::new();
    for partition in dedup(cmd.partitions) {
        match rom_dumps::get(&cache_key(&url, &partition)).await {
            Some(cached) => send_cached(bot, msg, &cached, &partition, &package).await?,
            None => pending.push(partition),
        }
    }
    if pending.is_empty() && !listing {
        return Ok(());
    }

    let status = Status::send(bot, msg, "正在读取包目录…").await?;
    let result = run(bot, msg, &status, &url, &package, pending).await;
    match result {
        Ok(true) => Ok(()),
        Ok(false) => {
            status.delete().await;
            Ok(())
        }
        Err(err) => {
            status.delete().await;
            Err(err)
        }
    }
}

fn dedup(partitions: Vec<String>) -> Vec<String> {
    let mut seen = Vec::<String>::new();
    for partition in partitions {
        let partition = partition
            .trim()
            .trim_end_matches(".img")
            .to_ascii_lowercase();
        if !partition.is_empty() && !seen.contains(&partition) {
            seen.push(partition);
        }
    }
    seen
}

/// 返回 true 表示进度消息已改成最终内容，要保留
async fn run(
    bot: &Bot,
    msg: &Message,
    status: &Status,
    url: &url::Url,
    package: &str,
    partitions: Vec<String>,
) -> Result<bool, BotError> {
    let src = Arc::new(Remote::open(url.as_str(), zip::TAIL_LEN).await?);
    let pkg = Package::open(&src).await?;

    if partitions.is_empty() {
        status.set_html(listing(&pkg, package)).await?;
        return Ok(true);
    }

    let mut missing = Vec::new();
    for partition in &partitions {
        let Some(target) = pkg.find(partition) else {
            missing.push(partition.as_str());
            continue;
        };
        if target.size() > MAX_IMAGE {
            return Err(BotError::Custom(format!(
                "{partition}.img 有 {}，超过 Telegram 2GB 上限",
                human_size(target.size())
            )));
        }
        let dir = TempDir::new()?;
        let out = dir.0.join(format!("{partition}.img"));
        let digest = extract(&src, &pkg, &target, &out, status, partition).await?;
        let sha256 = hex(&digest);
        status
            .set(format!(
                "正在上传 {partition}.img（{}）…",
                human_size(target.size())
            ))
            .await;
        let caption = caption(partition, package, target.size(), &sha256);
        let file_id = upload(bot, msg, &out, &format!("{partition}.img"), &caption).await?;
        let dump = RomDump {
            key: cache_key(url, partition),
            file_id,
            sha256,
            size: target.size(),
        };
        if let Err(err) = rom_dumps::put(&dump).await {
            log::warn!("rom dump cache write failed: {err}");
        }
    }
    if !missing.is_empty() {
        let names = pkg
            .list()
            .into_iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>()
            .join(" ");
        return Err(BotError::Custom(format!(
            "包里没有分区：{}\n可用分区：{names}",
            missing.join(" ")
        )));
    }
    Ok(false)
}

fn listing(pkg: &Package, package: &str) -> String {
    let parts = pkg.list();
    let width = parts.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
    let mut table = String::new();
    for (name, size) in &parts {
        let _ = writeln!(table, "{name:<width$}  {:>10}", human_size(*size));
    }
    let kind = match pkg {
        Package::Payload(_) => "A/B 卡刷包（payload.bin）",
        Package::Plain(_) => "卡刷包（zip 内镜像）",
    };
    format!(
        "<b>{}</b>\n{kind}，共 {} 个分区\n<blockquote expandable><pre>{}</pre></blockquote>\n用法：<code>/dump 链接 分区名 [分区名...]</code>",
        escape(package),
        parts.len(),
        escape(table.trim_end()),
    )
}

async fn extract(
    src: &Arc<Remote>,
    pkg: &Package,
    target: &Target<'_>,
    out: &Path,
    status: &Status,
    partition: &str,
) -> Result<[u8; 32], BotError> {
    let total = target.download_size();
    let downloaded = Arc::new(AtomicU64::new(0));
    status
        .set(format!(
            "正在提取 {partition}.img：需下载 {}…",
            human_size(total)
        ))
        .await;
    let work = async {
        match (pkg, target) {
            (Package::Payload(payload), Target::Payload(part)) => {
                payload
                    .extract(src.clone(), part, out, downloaded.clone())
                    .await
            }
            (_, Target::Plain(entry)) => extract_plain(src.as_ref(), entry, out, &downloaded).await,
            _ => unreachable!("target comes from the same package"),
        }
    };
    tokio::pin!(work);
    let mut ticker = tokio::time::interval(PROGRESS_EVERY);
    ticker.tick().await;
    loop {
        tokio::select! {
            result = &mut work => return result,
            _ = ticker.tick() => {
                let done = downloaded.load(Ordering::Relaxed);
                status.set(format!(
                    "正在提取 {partition}.img：{} / {}（{:.0}%）",
                    human_size(done),
                    human_size(total),
                    done as f64 * 100.0 / total.max(1) as f64
                )).await;
            }
        }
    }
}

/// zip 里直接放的镜像：STORED 原样写，DEFLATED 边下边解
async fn extract_plain(
    src: &dyn Source,
    entry: &Entry,
    out: &Path,
    downloaded: &AtomicU64,
) -> Result<[u8; 32], BotError> {
    let start = entry.data_offset(src).await?;
    let file = std::fs::File::create(out)?;
    let mut writer: Box<dyn std::io::Write + Send> = match entry.method {
        zip::STORED => Box::new(file),
        _ => Box::new(flate2::write::DeflateDecoder::new(file)),
    };
    let mut offset = 0;
    while offset < entry.compressed_size {
        let len = CHUNK.min(entry.compressed_size - offset);
        let chunk: Bytes = src.read(start + offset, len).await?;
        downloaded.fetch_add(len, Ordering::Relaxed);
        writer =
            tokio::task::spawn_blocking(move || writer.write_all(&chunk).map(|_| writer)).await??;
        offset += len;
    }
    tokio::task::spawn_blocking(move || writer.flush()).await??;
    let out = out.to_path_buf();
    tokio::task::spawn_blocking(move || sha256_file(&out)).await?
}

async fn send_cached(
    bot: &Bot,
    msg: &Message,
    cached: &RomDump,
    partition: &str,
    package: &str,
) -> BotResult {
    bot.send(
        SendDocument::new(msg.chat().id(), InputFile::id(cached.file_id.clone()))
            .caption(caption(partition, package, cached.size, &cached.sha256))
            .parse_mode(ParseMode::HTML)
            .reply_parameters(reply_to(msg.message_id())),
    )
    .await?;
    Ok(())
}

/// 发给用户，返回主 bot 的 file_id。配置了上传频道时先由上传 bot
/// （DC 离服务器近）传到频道，主 bot 转发拿到自己的 file_id 再发；
/// 这条路任何一步失败都退回主 bot 直接上传
async fn upload(
    bot: &Bot,
    msg: &Message,
    path: &Path,
    file_name: &str,
    caption: &str,
) -> Result<String, BotError> {
    let input = match relay(path, file_name).await {
        Ok(file_id) => InputFile::id(file_id),
        Err(err) => {
            if SETTINGS.music.upload_channel_id != 0 {
                log::warn!("dump upload relay failed, uploading directly: {err}");
            }
            InputFile::fs_with_name(path, file_name)
        }
    };
    // 直接上传可能要好几分钟，用超时更长的客户端
    let sender = match &input {
        InputFile::FS(_) => bot_with_token(&SETTINGS.bot.token, UPLOAD_TIMEOUT),
        _ => bot.clone(),
    };
    let sent = sender
        .send(
            SendDocument::new(msg.chat().id(), input)
                .caption(caption)
                .parse_mode(ParseMode::HTML)
                .reply_parameters(reply_to(msg.message_id())),
        )
        .await?;
    sent.document()
        .map(|doc| doc.file_id.to_string())
        .ok_or_else(|| BotError::Custom("Telegram 没有返回文件".to_string()))
}

/// 上传 bot 中转：发到频道 → 主 bot 转发拿 file_id → 删掉转发
async fn relay(path: &Path, file_name: &str) -> Result<String, BotError> {
    let channel_id = SETTINGS.music.upload_channel_id;
    if channel_id == 0 {
        return Err(BotError::Custom("未配置上传频道".to_string()));
    }
    let token = std::fs::read_to_string(&SETTINGS.music.upload_bot_token_file)?;
    let token = token.trim();
    if token.is_empty() {
        return Err(BotError::Custom("上传 bot 的 token 为空".to_string()));
    }
    let uploader = bot_with_token(token, UPLOAD_TIMEOUT);
    let post = uploader
        .send(
            SendDocument::new(channel_id, InputFile::fs_with_name(path, file_name))
                .disable_notification(true),
        )
        .await?;
    let forwarded = BOT
        .send(
            ForwardMessage::new(channel_id, channel_id, post.message_id())
                .disable_notification(true),
        )
        .await?;
    // 上传 bot 的那条留着存文件；转发只是为了拿主 bot 自己的 file_id
    let _ = BOT
        .send(DeleteMessage::new(channel_id, forwarded.message_id()))
        .await;
    forwarded
        .document()
        .map(|doc| doc.file_id.to_string())
        .ok_or_else(|| BotError::Custom("转发的消息里没有文件".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_drops_signature() {
        let a = url::Url::parse("https://cdn.example.com/OS/x.zip?t=1&s=abc").unwrap();
        let b = url::Url::parse("https://cdn.example.com/OS/x.zip?t=2&s=def").unwrap();
        assert_eq!(cache_key(&a, "Init_Boot"), cache_key(&b, "init_boot"));
        assert_eq!(package_name(&a), "x.zip");
    }

    #[test]
    fn names_plain_images_by_file_stem() {
        assert_eq!(plain_name("firmware-update/vbmeta.img"), "vbmeta");
        assert_eq!(plain_name("boot.img"), "boot");
    }

    #[test]
    fn dedups_partition_names() {
        assert_eq!(
            dedup(vec!["boot.img".into(), "BOOT".into(), "init_boot".into()]),
            vec!["boot", "init_boot"]
        );
    }
}

/// 用真实链接跑一遍提取（不上传）：
/// `DUMP_URL=... DUMP_PART=init_boot cargo test --release live_dump -- --ignored --nocapture`
#[cfg(test)]
mod live {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn live_dump() {
        let url = std::env::var("DUMP_URL").expect("DUMP_URL");
        let part = std::env::var("DUMP_PART").unwrap_or_else(|_| "init_boot".into());
        let started = std::time::Instant::now();
        let src = Arc::new(Remote::open(&url, zip::TAIL_LEN).await.unwrap());
        let pkg = Package::open(&src).await.unwrap();
        println!(
            "size {} | opened in {:?}",
            human_size(src.size()),
            started.elapsed()
        );
        println!("{}", listing(&pkg, "pkg"));
        let target = pkg.find(&part).expect("partition");
        let dir = TempDir::new().unwrap();
        let out = dir.0.join(format!("{part}.img"));
        let downloaded = Arc::new(AtomicU64::new(0));
        let digest = match (&pkg, &target) {
            (Package::Payload(p), Target::Payload(t)) => p
                .extract(src.clone(), t, &out, downloaded.clone())
                .await
                .unwrap(),
            (_, Target::Plain(e)) => extract_plain(src.as_ref(), e, &out, &downloaded)
                .await
                .unwrap(),
            _ => unreachable!(),
        };
        println!(
            "{part}: {} (downloaded {}) sha256 {} in {:?}",
            human_size(std::fs::metadata(&out).unwrap().len()),
            human_size(downloaded.load(Ordering::Relaxed)),
            hex(&digest),
            started.elapsed()
        );
    }
}
