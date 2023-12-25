use super::*;
use regex::Regex;

lazy_static! {
    static ref RE: Regex = Regex::new(r"\*\*(.*?)\*\*").unwrap();
}

pub fn escape_markdown(text: &str) -> String {
    let mut last_end = 0;
    let mut escaped_text = String::new();

    for cap in RE.captures_iter(text) {
        let whole_match = cap.get(0).unwrap().as_str();
        let inner_text = cap.get(1).unwrap().as_str();
        escaped_text.push_str(&escape_special_chars(
            &text[last_end..cap.get(0).unwrap().start()],
        ));
        if !inner_text.trim().is_empty() {
            escaped_text.push_str(&format!("*{inner_text}*")); // 将 "** **" 转换为 "* *"
        } else {
            escaped_text.push_str(whole_match); // 其他粗体文本保持不变
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
            | '{' | '}' | '.' | '!' => format!("\\{}", c),
            _ => c.to_string(),
        })
        .collect()
}
