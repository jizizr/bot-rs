use super::*;
use crate::{
    analysis::freq::{paint, summary},
    dao::mongo::freq::{Duration as FreqDuration, query_activity},
    funcs::pkg::timezone::{Appearance, chat_timezone},
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
    // 按群设置的时区统计时段，并按当地是白天还是晚上出浅色或深色的图
    let tz = chat_timezone(msg.chat().id()).await;
    let appearance = Appearance::at(&chrono::Utc::now().with_timezone(&tz));
    let activity = query_activity(
        msg.chat().id(),
        user.id,
        command.duration,
        tz,
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
        SendPhoto::new(
            msg.chat().id(),
            InputFile::buffered(paint(&activity, appearance)?),
        )
        .caption(summary(&activity))
        .reply_parameters(reply_to(msg.message_id())),
    )
    .await?;
    Ok(())
}
