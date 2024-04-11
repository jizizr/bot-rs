use crate::dao::mysql::wordcloud;
use std::collections::HashMap;
use teloxide::prelude::*;
use teloxide::types::{ChatId, InputFile, ParseMode};
use teloxide::utils::markdown;

pub async fn wcloud(bot: &Bot, group: i64) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut png = Vec::new();

    //转换为wordcloud需要的HashMap
    let words: HashMap<String, i32> = wordcloud::get_words(group)
        .await?
        .into_iter()
        .map(|w| (w.word, w.frequency))
        .collect();
    //处理没有记录的情况
    let group = ChatId(group);
    if words.is_empty() {
        bot.send_message(group, "群里太冷清了，热闹一点吧！")
            .await?;
        return Ok(());
    }

    //生成词云到内存
    super::builder::build(&mut png, words)?;

    bot.send_photo(group, InputFile::memory(png)).await?;
    Ok(())
}

pub async fn user_freq(
    bot: &Bot,
    group: i64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
    bot.send_message(
        ChatId(group),
        format!(
            "*📝发言统计*\n🕙`{}`\n\n*群里的活跃用户：*\n{}",
            markdown::escape(&time),
            users_str
        ),
    )
    .parse_mode(ParseMode::MarkdownV2)
    .await?;
    Ok(())
}
