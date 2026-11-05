use super::*;
use crate::{
    analysis::freq::{paint, summary},
    dao::mongo::freq::{Duration as FreqDuration, query_activity},
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
    let command =
        CountCmd::parse_i18n_from_bot(getor(msg).unwrap().split_whitespace(), language_tag)?;
    let user = msg
        .from()
        .ok_or(BotError::Custom("failed to get uid".to_string()))?;
    show_typing(bot, msg.chat().id());
    let activity = query_activity(
        msg.chat().id(),
        user.id,
        command.duration,
        crate::funcs::get_name(user),
        msg.chat().title().unwrap_or("本群").to_string(),
    )
    .await?;
    if activity.user_total() == 0 {
        bot.send(
            SendMessage::new(
                msg.chat().id(),
                format!("{}没有你在本群的发言记录", activity.period),
            )
            .reply_parameters(reply_to(msg.message_id())),
        )
        .await?;
        return Ok(());
    }

    bot.send(
        SendPhoto::new(msg.chat().id(), InputFile::buffered(paint(&activity)?))
            .caption(summary(&activity))
            .reply_parameters(reply_to(msg.message_id())),
    )
    .await?;
    Ok(())
}
