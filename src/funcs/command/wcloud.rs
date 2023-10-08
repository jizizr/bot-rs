use super::pkg::wcloud;
use super::*;
use crate::dao::mysql::wordcloud;
use crate::dao::mysql::wordcloud::WORD_POOL;
use std::collections::HashMap;
use teloxide::types::InputFile;

pub async fn wcloud(bot: Bot, msg: Message) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut png = Vec::new();

    //转换为wordcloud需要的HashMap
    let words: HashMap<String, i32> = wordcloud::get_words(&WORD_POOL, &msg.chat.id.to_string())
        .await?
        .into_iter()
        .map(|w| (w.word, w.frequency))
        .collect();

    //处理没有记录的情况
    if words.is_empty() {
        bot.send_message(msg.chat.id, "群里太冷清了，热闹一点吧！")
            .reply_to_message_id(msg.id)
            .await?;
        return Ok(());
    }

    //生成词云到内存
    wcloud::builder::build(&mut png, words)?;

    bot.send_photo(msg.chat.id, InputFile::memory(png))
        .reply_to_message_id(msg.id)
        .await?;
    Ok(())
}
