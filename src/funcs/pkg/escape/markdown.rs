use super::*;
use regex::Regex;

lazy_static! {
    static ref RE: Regex = Regex::new(r"\*\*(.*?)\*\*").unwrap();
}

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
