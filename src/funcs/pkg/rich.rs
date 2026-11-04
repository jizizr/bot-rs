//! Rich messages (Bot API 10.1+). Pages are written in the small HTML subset
//! Telegram accepts for them; all text handed to a page builder is escaped
//! here, and only `*_html` methods take markup.

use telers::types::{
    InputFile, InputMediaAudio, InputMediaPhoto, InputRichMessage, InputRichMessageMedia, Message,
    RichBlock, RichText,
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

    /// Links, hashtags and the like in song names stay plain text.
    pub fn build(self) -> InputRichMessage {
        let message = InputRichMessage::new()
            .html(self.html)
            .skip_entity_detection(true);
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
}
