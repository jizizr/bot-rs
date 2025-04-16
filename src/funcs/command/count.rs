use super::*;
use crate::{analysis::freq::paint, dao::mongo::freq::query_data};

pub async fn count(bot: &Bot, msg: &Message) -> BotResult {
    let datas = query_data(
        msg.chat.id.0,
        msg.from
            .as_ref()
            .ok_or(BotError::Custom("failed to get uid".to_string()))?
            .id
            .0,
    )
    .await?;

    bot.send_photo(msg.chat.id, InputFile::memory(paint(datas)?))
        .reply_parameters(ReplyParameters::new(msg.id))
        .await?;
    Ok(())
}
