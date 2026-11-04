use crate::funcs::text::SWITCH;

use super::*;

fn config_menu(group_id: i64) -> InlineKeyboardMarkup {
    let mut keyboard: Vec<Vec<InlineKeyboardButton>> = vec![];
    let mut i = 0;
    let mut row = vec![];
    for entry in SWITCH.get_template().iter() {
        let flag = SWITCH.get_status(group_id, entry.key().to_string());
        row.push(callback_button(
            format!("{} {}", entry.value(), if flag { "✅" } else { "❌" }),
            format!("config {} {}", entry.key(), if flag { "t" } else { "f" }),
        ));
        i += 1;
        if i == 2 {
            keyboard.push(row);
            row = vec![];
            i = 0;
        }
    }
    if i != 0 {
        keyboard.push(row);
    }
    InlineKeyboardMarkup::new(keyboard)
}

pub async fn config_callback(bot: Bot, q: CallbackQuery) -> BotResult {
    if let Some(config) = q.data
        && let Some(MaybeInaccessibleMessage::Message(msg)) = q.message.map(|msg| *msg)
    {
        if !auth(&bot, &msg, q.from.id).await? {
            bot.send(
                AnswerCallbackQuery::new(q.id)
                    .text("你不是管理员")
                    .show_alert(true),
            )
            .await?;
            return Ok(());
        }
        let ack = AnswerCallbackQuery::new(q.id.clone()).text("正在处理，请稍后");
        let ack_bot = bot.clone();
        tokio::spawn(async move { ack_bot.send(ack).await });
        let _guard = lock!((msg.chat().id(), msg.message_id()));
        let mut func_cfg = config.splitn(2, ' ');
        let (Some(func_name), Some(status)) = (func_cfg.next(), func_cfg.next()) else {
            return Ok(());
        };
        SWITCH
            .update_status(msg.chat().id(), func_name.to_string(), status == "f")
            .await;
        bot.send(
            EditMessageReplyMarkup::new()
                .chat_id(msg.chat().id())
                .message_id(msg.message_id())
                .reply_markup(config_menu(msg.chat().id())),
        )
        .await?;
    }
    Ok(())
}

pub async fn config(bot: &Bot, msg: &Message) -> BotResult {
    bot.send(
        SendMessage::new(msg.chat().id(), "功能开关").reply_markup(config_menu(msg.chat().id())),
    )
    .await?;
    Ok(())
}
