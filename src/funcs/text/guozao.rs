use teloxide::types::ParseMode;

use super::*;

fn contains_chinese(text: &str) -> bool {
    text.chars()
        .skip(1)
        .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}

pub async fn guozao(bot: &Bot, msg: &Message) -> BotResult {
    let args: Vec<&str> = msg.text().unwrap_or_default().split(' ').collect();
    if args.is_empty() || !contains_chinese(args[0]) {
        return Ok(());
    }
    let me = {
        let u = msg.from.as_ref().unwrap();
        fmt_at(&get_name(u), u.id.0)
    };
    let play_with = match msg.reply_to_message() {
        Some(m) => {
            let u = m.from.as_ref().unwrap();
            fmt_at(&get_name(u), u.id.0)
        }
        None => format!("[自己](tg://user?id={})", msg.from.as_ref().unwrap().id),
    };
    let text = if args.len() == 1 {
        format!("{} {}了 {}", me, &args[0][1..], play_with)
    } else {
        format!(
            "{} {}了 {} {}",
            me,
            &args[0][1..],
            play_with,
            args[1..].join(" ")
        )
    };
    let text = text.replace("$from", &me).replace("$to", &play_with);
    bot.send_message(msg.chat.id, text)
        .reply_parameters(ReplyParameters::new(msg.id))
        .parse_mode(ParseMode::MarkdownV2)
        .await?;
    Ok(())
}
