use super::*;
use crate::{
    analysis::freq::paint,
    dao::mongo::freq::{Duration as FreqDuration, query_data},
};

cmd!(
    "/count",
    "统计消息发送频率",
    CountCmd,
    {
        /// 日期范围
        duration: Option<FreqDuration>,
    }
);

pub async fn count(bot: &Bot, msg: &Message) -> BotResult {
    let language_tag = Some("zh-CN");
    let command = CountCmd::parse_i18n_from_bot(
        getor(msg).unwrap().split_whitespace(),
        language_tag,
    )?;
    let datas = query_data(
        msg.chat.id.0,
        msg.from
            .as_ref()
            .ok_or(BotError::Custom("failed to get uid".to_string()))?
            .id
            .0,
        command.duration,
    )
    .await?;

    bot.send_photo(msg.chat.id, InputFile::memory(paint(datas)?))
        .reply_parameters(ReplyParameters::new(msg.id))
        .await?;
    Ok(())
}
