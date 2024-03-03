use crate::funcs::text::SWITCH;

use super::*;

fn config_menu(group_id: i64) -> InlineKeyboardMarkup {
    let mut keyboard: Vec<Vec<InlineKeyboardButton>> = vec![];
    let mut i = 0;
    let mut row = vec![];
    for entry in SWITCH.get_template().iter() {
        let flag = SWITCH.get_status(group_id, entry.key().to_string());
        row.push(InlineKeyboardButton::callback(
            format!(
                "{} {}",
                entry.value(),
                if flag { "✅" } else { "❌" }
            ),
            format!(
                "config {} {}",
                entry.key(),
                if flag { "t" } else { "f" }
            ),
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
    if let Some(config) = q.data {
        if let Some(msg) = q.message {
            if !auth(&bot, &msg, q.from.id).await? {
                bot.answer_callback_query(q.id)
                    .text("你不是管理员")
                    .show_alert(true)
                    .send()
                    .await?;
                return Ok(());
            }
            let bot = Arc::new(bot);
            let bot_clone = bot.clone();
            tokio::spawn(async move {
                bot_clone
                    .answer_callback_query(&q.id)
                    .text("正在处理，请稍后")
                    .send()
                    .await
            });
            let _guard = lock!((msg.chat.id, msg.id));
            let mut func_cfg = config.splitn(2, ' ');
            SWITCH
                .update_status(
                    msg.chat.id.0,
                    func_cfg.next().unwrap().to_string(),
                    func_cfg.next().unwrap() == "f",
                )
                .await;
            bot.edit_message_reply_markup(msg.chat.id, msg.id)
                .reply_markup(config_menu(msg.chat.id.0))
                .send()
                .await?;
        }
    }
    Ok(())
}

pub async fn config(bot: Bot, msg: Message) -> BotResult {
    bot.send_message(msg.chat.id, "功能开关")
        .reply_markup(config_menu(msg.chat.id.0))
        .await?;
    Ok(())
}
