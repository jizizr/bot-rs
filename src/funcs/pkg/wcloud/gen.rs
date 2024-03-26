use crate::dao::mysql::wordcloud;
use std::collections::HashMap;
use teloxide::prelude::*;
use teloxide::types::{ChatId, InputFile};

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
