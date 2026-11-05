use regex::Regex;
use std::sync::LazyLock;

static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\*(.*?)\*\*").unwrap());

pub fn escape_markdown(text: &str) -> String {
    let mut last_end = 0;
    let mut escaped_text = String::new();

    for cap in RE.captures_iter(text) {
        let inner_text = cap.get(1).unwrap().as_str();
        escaped_text.push_str(&escape_special_chars(
            &text[last_end..cap.get(0).unwrap().start()],
        ));
        if !inner_text.trim().is_empty() {
            // 将 "** **" 转换为 "* *"
            escaped_text.push_str(&format!("*{}*", escape_special_chars(inner_text)));
        }
        last_end = cap.get(0).unwrap().end();
    }
    escaped_text.push_str(&escape_special_chars(&text[last_end..]));

    escaped_text
}

fn escape_special_chars(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '*' | '_' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#' | '+' | '-' | '=' | '|'
            | '{' | '}' | '.' | '!' => format!("\\{c}"),
            _ => c.to_string(),
        })
        .collect()
}

/// Escapes text for MarkdownV2 outside code and links.
pub fn escape(text: &str) -> String {
    escape_with(text, |c| {
        matches!(
            c,
            '\\' | '_'
                | '*'
                | '['
                | ']'
                | '('
                | ')'
                | '~'
                | '`'
                | '>'
                | '#'
                | '+'
                | '-'
                | '='
                | '|'
                | '{'
                | '}'
                | '.'
                | '!'
        )
    })
}

/// Escapes the URL part of an inline MarkdownV2 link.
pub fn escape_link_url(text: &str) -> String {
    escape_with(text, |c| matches!(c, '\\' | ')'))
}

/// Escapes text inside MarkdownV2 inline code or code blocks.
pub fn escape_code(text: &str) -> String {
    escape_with(text, |c| matches!(c, '\\' | '`'))
}

fn escape_with(text: &str, special: impl Fn(char) -> bool) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if special(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}
