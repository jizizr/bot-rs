/// 转义 Telegram HTML 模式下的正文与属性值
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn escapes_markup() {
        assert_eq!(
            super::escape(r#"<b>"a" & b</b>"#),
            "&lt;b&gt;&quot;a&quot; &amp; b&lt;/b&gt;"
        );
    }
}
