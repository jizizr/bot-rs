//! Rich messages (Bot API 10.1+). Pages are written in the small HTML subset
//! Telegram accepts for them; all text handed to a page builder is escaped
//! here, and only `*_html` methods take markup.

use telers::types::{
    InputFile, InputMediaAudio, InputMediaPhoto, InputRichMessage, InputRichMessageMedia, Message,
    PhotoSize, RichBlock, RichBlockCaption, RichText,
};

pub fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[derive(Debug, Default)]
pub struct RichPage {
    html: String,
    /// Telegram files the page shows, referenced from `html` by id.
    media: Vec<InputRichMessageMedia>,
    /// Let Telegram turn hashtags, links and the like into entities.
    detect_entities: bool,
}

impl RichPage {
    pub fn heading(mut self, text: &str) -> Self {
        self.html
            .push_str(&format!("<h3>{}</h3>", html_escape(text)));
        self
    }

    pub fn paragraph(self, text: &str) -> Self {
        self.paragraph_html(&html_escape(text))
    }

    pub fn paragraph_html(mut self, html: &str) -> Self {
        self.html.push_str(&format!("<p>{html}</p>"));
        self
    }

    /// A numbered list starting from 1; empty lists are left out.
    pub fn numbered_html(mut self, items: impl IntoIterator<Item = String>) -> Self {
        let items = items
            .into_iter()
            .map(|item| format!("<li>{item}</li>"))
            .collect::<String>();
        if !items.is_empty() {
            self.html.push_str(&format!("<ol>{items}</ol>"));
        }
        self
    }

    /// A bulleted list; empty lists are left out.
    pub fn bullets_html(mut self, items: impl IntoIterator<Item = String>) -> Self {
        let items = items
            .into_iter()
            .map(|item| format!("<li>{item}</li>"))
            .collect::<String>();
        if !items.is_empty() {
            self.html.push_str(&format!("<ul>{items}</ul>"));
        }
        self
    }

    pub fn quote(mut self, text: &str) -> Self {
        let lines = text.lines().map(html_escape).collect::<Vec<_>>();
        self.html
            .push_str(&format!("<blockquote>{}</blockquote>", lines.join("<br>")));
        self
    }

    /// A picture from an HTTP(S) URL, with an optional caption under it.
    pub fn image(mut self, url: &str, caption: Option<&str>) -> Self {
        let img = format!("<img src=\"{}\"/>", html_escape(url));
        self.html.push_str(
            &match caption.filter(|caption| !caption.trim().is_empty()) {
                Some(caption) => format!(
                    "<figure>{img}<figcaption>{}</figcaption></figure>",
                    html_escape(caption)
                ),
                None => img,
            },
        );
        self
    }

    /// A photo already on Telegram, by file id. Unlike `image`, this works in
    /// inline results and inline edits, which may not link outside files.
    pub fn uploaded_photo(mut self, file_id: &str) -> Self {
        let id = format!("media{}", self.media.len());
        self.html
            .push_str(&format!("<img src=\"tg://photo?id={id}\"/>"));
        self.media.push(InputRichMessageMedia::new(
            id,
            InputMediaPhoto::new(InputFile::id(file_id)),
        ));
        self
    }

    /// A song already on Telegram, by file id, shown as a player.
    pub fn uploaded_audio(
        mut self,
        file_id: &str,
        title: &str,
        performer: &str,
        duration: Option<u32>,
    ) -> Self {
        let id = format!("media{}", self.media.len());
        self.html
            .push_str(&format!("<audio src=\"tg://audio?id={id}\"></audio>"));
        let mut audio = InputMediaAudio::new(InputFile::id(file_id))
            .title(title)
            .performer(performer);
        if let Some(duration) = duration.filter(|duration| *duration > 0) {
            audio = audio.duration(i64::from(duration));
        }
        self.media.push(InputRichMessageMedia::new(id, audio));
        self
    }

    /// A two-column table of `name | value` rows; values are markup.
    pub fn fields_html<'a>(mut self, rows: impl IntoIterator<Item = (&'a str, String)>) -> Self {
        let rows = rows
            .into_iter()
            .map(|(name, value)| format!("<tr><th>{}</th><td>{value}</td></tr>", html_escape(name)))
            .collect::<String>();
        if !rows.is_empty() {
            self.html
                .push_str(&format!("<table bordered striped compact>{rows}</table>"));
        }
        self
    }

    /// A block folded under `summary` until opened.
    pub fn details(mut self, summary: &str, inner: RichPage) -> Self {
        self.media.extend(inner.media);
        if !inner.html.is_empty() {
            self.html.push_str(&format!(
                "<details><summary>{}</summary>{}</details>",
                html_escape(summary),
                inner.html
            ));
        }
        self
    }

    /// Preformatted text, kept as written.
    pub fn pre(mut self, text: &str) -> Self {
        self.html
            .push_str(&format!("<pre>{}</pre>", html_escape(text.trim_end())));
        self
    }

    /// Markup for a block Telegram supports but this builder does not name.
    pub fn block_html(mut self, html: &str) -> Self {
        self.html.push_str(html);
        self
    }

    pub fn footer(mut self, text: &str) -> Self {
        self.html
            .push_str(&format!("<footer>{}</footer>", html_escape(text)));
        self
    }

    pub fn html(&self) -> &str {
        &self.html
    }

    /// Make hashtags and links in the text clickable; off by default so song
    /// names stay plain text.
    pub fn detect_entities(mut self) -> Self {
        self.detect_entities = true;
        self
    }

    pub fn build(self) -> InputRichMessage {
        let message = InputRichMessage::new()
            .html(self.html)
            .skip_entity_detection(!self.detect_entities);
        if self.media.is_empty() {
            message
        } else {
            message.media(self.media)
        }
    }
}

/// A status line such as download progress, one paragraph per line.
pub fn status(text: &str) -> InputRichMessage {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .fold(RichPage::default(), RichPage::paragraph)
        .build()
}

/// The text of the first heading of a rich message.
pub fn first_heading(msg: &Message) -> Option<String> {
    let Message::RichMessage(message) = msg else {
        return None;
    };
    message
        .rich_message
        .blocks
        .iter()
        .find_map(|block| match block {
            RichBlock::Heading(heading) => Some(plain_text(&heading.text)),
            _ => None,
        })
}

/// 富文本消息的文字，按版式摊平成纯文本，供翻译：块之间空一行，列表项带序号或圆点，
/// 引用逐行加 `> `，表格一行一行用 ` | ` 分隔，媒体只取说明文字。没有文字时返回 None
pub fn plain_blocks(msg: &Message) -> Option<String> {
    let Message::RichMessage(message) = msg else {
        return None;
    };
    let text = flatten(&message.rich_message.blocks).join("\n\n");
    (!text.trim().is_empty()).then_some(text)
}

/// 富文本消息里的第一张图（含拼图、轮播、引用、折叠块、列表里的）
pub fn first_photo(msg: &Message) -> Option<&[PhotoSize]> {
    let Message::RichMessage(message) = msg else {
        return None;
    };
    fn find(blocks: &[RichBlock]) -> Option<&[PhotoSize]> {
        blocks.iter().find_map(|block| match block {
            RichBlock::Photo(photo) => Some(photo.photo.as_ref()),
            RichBlock::List(list) => list.items.iter().find_map(|item| find(&item.blocks)),
            other => other.blocks().and_then(find),
        })
    }
    find(&message.rich_message.blocks)
}

fn flatten(blocks: &[RichBlock]) -> Vec<String> {
    let mut out = Vec::new();
    for block in blocks {
        let text = match block {
            RichBlock::Paragraph(b) => plain_text(&b.text),
            RichBlock::Heading(b) => plain_text(&b.text),
            RichBlock::Pre(b) => plain_text(&b.text),
            RichBlock::Footer(b) => plain_text(&b.text),
            RichBlock::Pullquote(b) => quoted(&plain_text(&b.text), b.credit.as_deref()),
            RichBlock::ExpandableBlockquote(b) => quoted(&plain_text(&b.text), b.credit.as_deref()),
            RichBlock::Blockquote(b) => {
                quoted(&flatten(&b.blocks).join("\n\n"), b.credit.as_deref())
            }
            RichBlock::List(list) => list
                .items
                .iter()
                .map(|item| {
                    let label = item.label.trim();
                    let label = if label.is_empty() { "•" } else { label };
                    format!("{label} {}", flatten(&item.blocks).join("\n"))
                })
                .collect::<Vec<_>>()
                .join("\n"),
            RichBlock::Table(table) => {
                let mut rows: Vec<String> = table
                    .cells
                    .iter()
                    .map(|row| {
                        row.iter()
                            .map(|cell| cell.text.as_deref().map(plain_text).unwrap_or_default())
                            .collect::<Vec<_>>()
                            .join(" | ")
                    })
                    .collect();
                rows.extend(table.caption.as_deref().map(plain_text));
                rows.join("\n")
            }
            RichBlock::Details(details) => std::iter::once(plain_text(&details.summary))
                .chain(flatten(&details.blocks))
                .collect::<Vec<_>>()
                .join("\n\n"),
            RichBlock::Collage(b) => with_caption(flatten(&b.blocks), b.caption.as_ref()),
            RichBlock::Slideshow(b) => with_caption(flatten(&b.blocks), b.caption.as_ref()),
            RichBlock::Photo(b) => caption_text(b.caption.as_ref()),
            RichBlock::Video(b) => caption_text(b.caption.as_ref()),
            RichBlock::Animation(b) => caption_text(b.caption.as_ref()),
            RichBlock::Audio(b) => caption_text(b.caption.as_ref()),
            RichBlock::Document(b) => caption_text(b.caption.as_ref()),
            RichBlock::Map(b) => caption_text(b.caption.as_ref()),
            RichBlock::VoiceNote(b) => caption_text(b.caption.as_ref()),
            // 分隔线、锚点、按钮、公式、思考过程等没有要翻的正文
            _ => String::new(),
        };
        if !text.trim().is_empty() {
            out.push(text);
        }
    }
    out
}

fn quoted(text: &str, credit: Option<&RichText>) -> String {
    let mut lines: Vec<String> = text.lines().map(|line| format!("> {line}")).collect();
    if let Some(credit) = credit.map(plain_text).filter(|c| !c.trim().is_empty()) {
        lines.push(format!("— {credit}"));
    }
    lines.join("\n")
}

fn caption_text(caption: Option<&RichBlockCaption>) -> String {
    let Some(caption) = caption else {
        return String::new();
    };
    let text = plain_text(&caption.text);
    match caption.credit.as_deref().map(plain_text) {
        Some(credit) if !credit.trim().is_empty() => format!("{text}\n— {credit}"),
        _ => text,
    }
}

fn with_caption(mut parts: Vec<String>, caption: Option<&RichBlockCaption>) -> String {
    parts.push(caption_text(caption));
    parts.retain(|part| !part.trim().is_empty());
    parts.join("\n\n")
}

fn plain_text(text: &RichText) -> String {
    match text {
        RichText::Plain(text) => text.to_string(),
        RichText::Multiple(parts) => parts.iter().map(plain_text).collect(),
        RichText::CustomEmoji(emoji) => emoji.alternative_text.to_string(),
        other => other.text().map(plain_text).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_escape_text_and_skip_empty_lists() {
        let page = RichPage::default()
            .heading("🔍 a<b>")
            .paragraph("x & y")
            .numbered_html(Vec::new())
            .numbered_html(["<b>1</b>".to_string()])
            .quote("一\n二")
            .footer("页脚");
        assert_eq!(
            page.html(),
            "<h3>🔍 a&lt;b&gt;</h3><p>x &amp; y</p><ol><li><b>1</b></li></ol>\
             <blockquote>一<br>二</blockquote><footer>页脚</footer>"
        );
    }

    #[test]
    fn reads_back_the_first_heading() {
        let msg: Message = serde_json::from_value(serde_json::json!({
            "message_id": 1,
            "date": 1700000000,
            "chat": { "id": 1, "type": "private", "first_name": "u" },
            "rich_message": { "blocks": [
                { "type": "paragraph", "text": "intro" },
                { "type": "heading", "size": 3, "text": ["🔍 ", { "type": "bold", "text": "晴天" }] },
            ] }
        }))
        .unwrap();
        assert_eq!(first_heading(&msg).as_deref(), Some("🔍 晴天"));
    }

    #[test]
    fn flattens_blocks_for_translation() {
        let msg: Message = serde_json::from_value(serde_json::json!({
            "message_id": 1,
            "date": 1700000000,
            "chat": { "id": 1, "type": "private", "first_name": "u" },
            "rich_message": { "blocks": [
                { "type": "heading", "size": 3, "text": ["Title ", { "type": "bold", "text": "Bold" }] },
                { "type": "photo", "photo": [
                    { "file_id": "small", "file_unique_id": "s", "width": 90, "height": 90 },
                    { "file_id": "big", "file_unique_id": "b", "width": 800, "height": 800 },
                ], "caption": { "text": "A cover" } },
                { "type": "divider" },
                { "type": "list", "items": [
                    { "label": "1.", "blocks": [{ "type": "paragraph", "text": "one" }] },
                    { "label": "", "blocks": [{ "type": "paragraph", "text": "two" }] },
                ] },
                { "type": "blockquote", "blocks": [{ "type": "paragraph", "text": "line" }], "credit": "me" },
                { "type": "table", "cells": [[
                    { "text": "a", "align": "left", "valign": "top" },
                    { "text": "b", "align": "left", "valign": "top" },
                ]] },
            ] }
        }))
        .unwrap();
        assert_eq!(
            plain_blocks(&msg).as_deref(),
            Some("Title Bold\n\nA cover\n\n1. one\n• two\n\n> line\n— me\n\na | b")
        );
        assert_eq!(first_photo(&msg).map(|p| p.len()), Some(2));
    }
}
