use std::ops::Range;

use chrono::{DateTime, FixedOffset};
use telers::types::{ChatMember, MessageEntity, PhotoSize};

use super::*;
use crate::funcs::pkg::quote::{self as render, Badge, MediaKind, Quote, Span, SpanStyle, Theme};

/// 回复一条消息发 `/q`，把那条消息画成语录贴纸；`/q light` 用浅色气泡。
/// 支持文字、图片（带不带说明都行）和贴纸，动态/视频贴纸用它的静态缩略图
pub async fn quote(bot: &Bot, msg: &Message) -> BotResult {
    let target = msg
        .reply_to_message()
        // 话题群里不回复时，reply_to_message 是话题的创建消息
        .filter(|target| !matches!(target, Message::ForumTopicCreated(_)))
        .ok_or_else(|| BotError::Custom("请回复一条消息再使用 /q".to_string()))?;
    let text = getor(target).unwrap_or_default();
    let media = media_of(target);
    if text.trim().is_empty() && media.is_none() {
        return Err(BotError::Custom(
            "暂时只支持文字、图片和贴纸消息".to_string(),
        ));
    }
    let theme = theme(getor(msg).unwrap_or_default());
    show_typing(bot, msg.chat().id());

    let entities = target
        .entities()
        .or(target.caption_entities())
        .unwrap_or_default();
    let (sender, bytes) = tokio::join!(sender(bot, msg.chat(), target), async {
        match &media {
            Some(PendingMedia {
                file_id: Some(file_id),
                ..
            }) => download(bot, file_id).await,
            _ => None,
        }
    });
    let (text, spans, media) = match (media, bytes) {
        (Some(media), Some(bytes)) => (
            text.to_string(),
            spans(text, entities),
            Some(render::Media {
                kind: media.kind,
                bytes,
            }),
        ),
        // 图没拿到就退回成文字，说明文字优先
        (Some(media), None) if text.trim().is_empty() => (media.fallback, Vec::new(), None),
        _ => (text.to_string(), spans(text, entities), None),
    };
    let quote = Quote {
        name: sender.name,
        color_index: sender.color_index,
        badge: sender.badge,
        avatar: sender.avatar,
        text,
        spans,
        time: time_label(target),
        media,
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

struct PendingMedia {
    kind: MediaKind,
    /// 动态贴纸没有缩略图时是 None，直接画成文字
    file_id: Option<String>,
    /// 下载失败、又没有说明文字时画成这段字
    fallback: String,
}

fn media_of(target: &Message) -> Option<PendingMedia> {
    if let Some(sizes) = target.photo() {
        return Some(PendingMedia {
            kind: MediaKind::Photo,
            file_id: photo_size(sizes),
            fallback: "[图片]".to_string(),
        });
    }
    let sticker = target.sticker()?;
    let fallback = match sticker.emoji() {
        Some(emoji) => format!("{emoji} [贴纸]"),
        None => "[贴纸]".to_string(),
    };
    // tgs 和 webm 画不了，用缩略图；静态贴纸本身就是 webp
    let file_id = match sticker.is_animated() || sticker.is_video() {
        true => sticker.thumbnail().map(|thumb| thumb.file_id.to_string()),
        false => Some(sticker.file_id().to_string()),
    };
    Some(PendingMedia {
        kind: MediaKind::Sticker,
        file_id,
        fallback,
    })
}

/// 贴纸里图片最宽三百来逻辑像素、放大后不到 600px，取最长边至少 640 的最小一档
fn photo_size(sizes: &[PhotoSize]) -> Option<String> {
    sizes
        .iter()
        .find(|size| size.width.max(size.height) >= 640)
        .or(sizes.last())
        .map(|size| size.file_id.to_string())
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
            // 160px 的小图就够贴纸用，大图是 640px
            Some(photo) => download(bot, &photo.small_file_id).await,
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
    let (info, member) = tokio::join!(bot.send(GetChat::new(user.id)), async {
        match in_group {
            true => bot.send(GetChatMember::new(chat.id(), user.id)).await.ok(),
            false => None,
        }
    });
    let info = info.ok();
    // getChat 已经带了 160px 的头像，拿不到时再查头像列表
    let avatar_id = match info.as_ref().and_then(|info| info.photo()) {
        Some(photo) => Some(photo.small_file_id.to_string()),
        None => bot
            .send(GetUserProfilePhotos::new(user.id).limit(1u8))
            .await
            .ok()
            .and_then(|photos| photos.photos.first().and_then(|sizes| avatar_size(sizes))),
    };
    let avatar = match avatar_id {
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
        color_index: color_index(info.map(|info| info.accent_color_id()), user.id),
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
    crate::funcs::pkg::tgfile::download(bot, file_id, 10.0)
        .await
        .inspect_err(|e| log::error!("语录图片下载失败：{e}"))
        .ok()
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
