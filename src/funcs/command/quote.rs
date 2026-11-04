use std::{
    ops::Range,
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::{DateTime, FixedOffset};
use telers::types::{ChatMember, MessageEntity, PhotoSize};

use super::*;
use crate::funcs::pkg::quote::{self as render, Badge, Quote, Span, SpanStyle, Theme};

/// 回复一条消息发 `/q`，把那条消息画成语录贴纸；`/q light` 用浅色气泡
pub async fn quote(bot: &Bot, msg: &Message) -> BotResult {
    let target = msg
        .reply_to_message()
        // 话题群里不回复时，reply_to_message 是话题的创建消息
        .filter(|target| !matches!(target, Message::ForumTopicCreated(_)))
        .ok_or_else(|| BotError::Custom("请回复一条消息再使用 /q".to_string()))?;
    let text = getor(target)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| BotError::Custom("暂时只支持文字消息".to_string()))?;
    let theme = theme(getor(msg).unwrap_or_default());

    let entities = target
        .entities()
        .or(target.caption_entities())
        .unwrap_or_default();
    let sender = sender(bot, msg.chat(), target).await;
    let quote = Quote {
        name: sender.name,
        color_index: sender.color_index,
        badge: sender.badge,
        avatar: sender.avatar,
        text: text.to_string(),
        spans: spans(text, entities),
        time: time_label(target),
    };
    let sticker = tokio::task::spawn_blocking(move || render::render(&quote, theme)).await??;
    bot.send(
        SendSticker::new(
            msg.chat().id(),
            InputFile::buffered_with_name(sticker, "quote.webp"),
        )
        .reply_parameters(reply_to(msg.message_id())),
    )
    .await?;
    Ok(())
}

fn theme(command: &str) -> Theme {
    match command.split_whitespace().nth(1) {
        Some(arg) if ["light", "l", "亮", "白"].contains(&arg.to_lowercase().as_str()) => {
            Theme::Light
        }
        _ => Theme::Dark,
    }
}

struct Sender {
    name: String,
    color_index: usize,
    badge: Option<Badge>,
    avatar: Option<Vec<u8>>,
}

async fn sender(bot: &Bot, chat: &Chat, target: &Message) -> Sender {
    // 匿名管理员、频道身份发言时显示的是那个聊天
    if let Some(sender_chat) = target.sender_chat() {
        let info = bot.send(GetChat::new(sender_chat.id())).await.ok();
        let avatar = match info.as_ref().and_then(|info| info.photo()) {
            Some(photo) => download(bot, &photo.big_file_id).await,
            None => None,
        };
        return Sender {
            name: sender_chat.title().unwrap_or_default().to_string(),
            color_index: color_index(
                info.as_ref().map(|info| info.accent_color_id()),
                sender_chat.id(),
            ),
            badge: target
                .author_signature()
                .map(|title| Badge::Title(title.to_string())),
            avatar,
        };
    }
    let Some(user) = target.from() else {
        return Sender {
            name: "未知用户".to_string(),
            color_index: 0,
            badge: None,
            avatar: None,
        };
    };

    let in_group = !matches!(chat, Chat::Private(_));
    let (info, photos, member) = tokio::join!(
        bot.send(GetChat::new(user.id)),
        bot.send(GetUserProfilePhotos::new(user.id).limit(1u8)),
        async {
            match in_group {
                true => bot.send(GetChatMember::new(chat.id(), user.id)).await.ok(),
                false => None,
            }
        },
    );
    let avatar = match photos
        .ok()
        .and_then(|photos| photos.photos.first().and_then(|sizes| avatar_size(sizes)))
    {
        Some(file_id) => download(bot, &file_id).await,
        None => None,
    };
    let badge = match (target.sender_tag(), member) {
        (Some(tag), _) => Some(Badge::Tag(tag.to_string())),
        (None, Some(member @ ChatMember::Creator(_))) => Some(Badge::Title(
            member.custom_title().unwrap_or("群主").to_string(),
        )),
        (None, Some(member @ ChatMember::Administrator(_))) => Some(Badge::Title(
            member.custom_title().unwrap_or("管理员").to_string(),
        )),
        _ => None,
    };
    Sender {
        name: get_name(user),
        color_index: color_index(info.ok().map(|info| info.accent_color_id()), user.id),
        badge,
        avatar,
    }
}

/// 取至少 160px 的最小一档，头像在贴纸里只有一百来像素
fn avatar_size(sizes: &[PhotoSize]) -> Option<String> {
    sizes
        .iter()
        .find(|size| size.width >= 160)
        .or(sizes.last())
        .map(|size| size.file_id.to_string())
}

/// 0..7 是七种基础色，7..14 是同色系的双色款，其余按 id 取色
fn color_index(accent_color_id: Option<i64>, id: i64) -> usize {
    match accent_color_id {
        Some(accent @ 0..=6) => accent as usize,
        Some(accent @ 7..=13) => (accent - 7) as usize,
        _ => (id.unsigned_abs() % 7) as usize,
    }
}

async fn download(bot: &Bot, file_id: &str) -> Option<Vec<u8>> {
    let file = match bot.send(GetFile::new(file_id)).await {
        Ok(file) => file,
        Err(e) => {
            log::error!("语录头像 getFile 失败：{e}");
            return None;
        }
    };
    let path = file.file_path?;
    // 本地模式的 Bot API 直接给出文件路径，没有下载接口
    if path.starts_with('/') {
        let local = local_file_path(
            &path,
            &SETTINGS.bot.token,
            SETTINGS.bot.files_dir.as_deref(),
        );
        return match tokio::fs::read(&local).await {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                // 路径里带着 token，不能打出来
                log::error!("语录头像读取本地文件失败（检查 bot.files_dir）：{e}");
                None
            }
        };
    }
    let api = SETTINGS
        .bot
        .api_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .unwrap_or("https://api.telegram.org")
        .trim_end_matches('/');
    let url = format!("{api}/file/bot{}/{path}", SETTINGS.bot.token);
    let response = HTTP_CLIENT
        .get(url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status);
    match response {
        Ok(response) => response.bytes().await.ok().map(|bytes| bytes.to_vec()),
        Err(e) => {
            // reqwest 的错误里带着含 token 的 URL，不能原样打出来
            log::error!("语录头像下载失败：{}", e.without_url());
            None
        }
    }
}

/// 把本地模式 Bot API 给的服务端路径 `<服务端数据目录>/<token>/photos/x.jpg`
/// 换到 `<files_dir>/<token>/photos/x.jpg`；没配 `files_dir` 时视为同机同路径
fn local_file_path(path: &str, token: &str, files_dir: Option<&str>) -> PathBuf {
    let files_dir = files_dir.map(str::trim).filter(|dir| !dir.is_empty());
    match (files_dir, path.find(&format!("/{token}/"))) {
        (Some(dir), Some(at)) => Path::new(dir).join(&path[at + 1..]),
        _ => PathBuf::from(path),
    }
}

fn time_label(target: &Message) -> String {
    let tz = FixedOffset::east_opt(8 * 3600).expect("UTC+8 是合法时区");
    let time = DateTime::from_timestamp(target.date(), 0)
        .map(|time| time.with_timezone(&tz).format("%H:%M").to_string())
        .unwrap_or_default();
    match target.edit_date() {
        Some(_) => format!("已编辑 {time}"),
        None => time,
    }
}

fn spans(text: &str, entities: &[MessageEntity]) -> Vec<Span> {
    entities
        .iter()
        .filter_map(|entity| {
            let style = match entity {
                MessageEntity::Bold(_) => SpanStyle {
                    bold: true,
                    ..Default::default()
                },
                MessageEntity::Italic(_) => SpanStyle {
                    italic: true,
                    ..Default::default()
                },
                MessageEntity::Underline(_) => SpanStyle {
                    underline: true,
                    ..Default::default()
                },
                MessageEntity::Strikethrough(_) => SpanStyle {
                    strikethrough: true,
                    ..Default::default()
                },
                MessageEntity::Spoiler(_) => SpanStyle {
                    spoiler: true,
                    ..Default::default()
                },
                MessageEntity::Code(_) | MessageEntity::Pre(_) => SpanStyle {
                    monospace: true,
                    ..Default::default()
                },
                MessageEntity::Mention(_)
                | MessageEntity::Hashtag(_)
                | MessageEntity::Cashtag(_)
                | MessageEntity::BotCommand(_)
                | MessageEntity::Url(_)
                | MessageEntity::Email(_)
                | MessageEntity::PhoneNumber(_)
                | MessageEntity::TextLink(_)
                | MessageEntity::TextMention(_) => SpanStyle {
                    link: true,
                    ..Default::default()
                },
                _ => return None,
            };
            Some(Span {
                range: utf16_range(text, entity.offset(), entity.length())?,
                style,
            })
        })
        .collect()
}

/// Telegram 实体按 UTF-16 计偏移，换成字节区间；越界或切在字符中间就丢掉
fn utf16_range(text: &str, offset: i64, length: i64) -> Option<Range<usize>> {
    let start = usize::try_from(offset).ok()?;
    let end = start.checked_add(usize::try_from(length).ok()?)?;
    let (mut from, mut to) = (None, None);
    let mut units = 0;
    for (index, ch) in text
        .char_indices()
        .chain(std::iter::once((text.len(), '\0')))
    {
        if units == start {
            from = Some(index);
        }
        if units == end {
            to = Some(index);
            break;
        }
        units += ch.len_utf16();
    }
    Some(from?..to?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_offsets_become_byte_ranges() {
        let text = "a😀你b";
        // 😀 占两个 UTF-16 单元
        assert_eq!(utf16_range(text, 1, 2), Some(1..5));
        assert_eq!(utf16_range(text, 3, 1), Some(5..8));
        assert_eq!(utf16_range(text, 0, 5), Some(0..text.len()));
        // 切在代理对中间、越界
        assert_eq!(utf16_range(text, 2, 1), None);
        assert_eq!(utf16_range(text, 4, 9), None);
    }

    #[test]
    fn local_bot_api_paths_map_into_files_dir() {
        let path = "/var/lib/telegram-bot-api/123:abc/photos/file_1.jpg";
        assert_eq!(
            local_file_path(path, "123:abc", Some("/root/telegram-bot-api/data")),
            PathBuf::from("/root/telegram-bot-api/data/123:abc/photos/file_1.jpg")
        );
        // 没配目录、配了空串、路径里没有 token 时原样使用
        assert_eq!(local_file_path(path, "123:abc", None), PathBuf::from(path));
        assert_eq!(
            local_file_path(path, "123:abc", Some(" ")),
            PathBuf::from(path)
        );
        assert_eq!(
            local_file_path(path, "456:def", Some("/data")),
            PathBuf::from(path)
        );
    }

    #[test]
    fn accent_colors_map_to_the_base_palette() {
        assert_eq!(color_index(Some(3), 100), 3);
        assert_eq!(color_index(Some(13), 100), 6);
        assert_eq!(color_index(Some(16), 100), 2);
        assert_eq!(color_index(None, -100), 2);
    }

    #[test]
    fn theme_defaults_to_dark() {
        assert_eq!(theme("/q"), Theme::Dark);
        assert_eq!(theme("/q@bot light"), Theme::Light);
        assert_eq!(theme("/q 亮"), Theme::Light);
        assert_eq!(theme("/q whatever"), Theme::Dark);
    }
}
