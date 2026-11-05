use crate::{dao::mysql::wordcloud, funcs::pkg::escape::markdown, *};
use std::collections::HashMap;

pub async fn wcloud(bot: &Bot, group: i64) -> Result<(), BotError> {
    //转换为wordcloud需要的HashMap
    let word_vec = wordcloud::get_words(group).await?;
    let words: HashMap<&str, usize> = word_vec
        .iter()
        .map(|w| (w.word.as_str(), w.frequency))
        .collect();
    //处理没有记录的情况
    if words.is_empty() {
        bot.send(SendMessage::new(group, "群里太冷清了，热闹一点吧！"))
            .await?;
        return Ok(());
    }

    let mut png = Vec::new();
    //生成词云到内存
    super::builder::build(&mut png, words)?;

    bot.send(SendPhoto::new(group, InputFile::buffered(png)).disable_notification(true))
        .await?;
    Ok(())
}

pub async fn user_freq(bot: &Bot, group: i64) -> Result<(), BotError> {
    let users = wordcloud::get_users(group).await?;
    if users.is_empty() {
        return Ok(());
    }
    let time = chrono::Local::now().format("%m-%d %H:%M").to_string();
    let users_str = users
        .iter()
        .map(|u| format!("*{}* 发言: `{}` 句", markdown::escape(&u.name), u.frequency))
        .collect::<Vec<String>>()
        .join("\n");
    bot.send(
        SendMessage::new(
            group,
            format!(
                "*📝发言统计*\n🕙`{}`\n\n*群里的活跃用户：*\n{}",
                markdown::escape(&time),
                users_str
            ),
        )
        .parse_mode(ParseMode::MarkdownV2),
    )
    .await?;
    Ok(())
}
