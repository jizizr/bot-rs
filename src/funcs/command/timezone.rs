//! `/timezone`: shows the chat's time zone and lets an admin change it, by
//! typing one or with the buttons of common zones.
use super::*;
use crate::funcs::pkg::timezone::{
    Appearance, COMMON, chat_timezone, parse_timezone, set_chat_timezone, utc_offset, zone_name,
};
use chrono::Utc;
use chrono_tz::Tz;

cmd!(
    "/timezone",
    "查看或设置本群时区",
    TimezoneCmd,
    {
        /// 时区：城市名（如 东京）、时区名（如 东九区、Asia/Tokyo）或整点偏移（如 +9、UTC-5）
        #[arg(allow_hyphen_values = true)]
        zone: Option<String>,
    }
);

fn status_text(tz: Tz) -> String {
    let now = Utc::now();
    let local = now.with_timezone(&tz);
    format!(
        "本群时区：{}（{}）\n当地时间：{}\n图表现在用{}（当地 19:00–07:00 用深色）\n\n管理员可以点下面的按钮，或发送 /timezone 加时区来修改，例如 /timezone 东京、/timezone +9",
        zone_name(tz),
        utc_offset(tz, now),
        local.format("%m-%d %H:%M"),
        Appearance::at(&local).name(),
    )
}

/// The common zones, three a row, the chat's own one ticked.
fn zone_menu(current: Tz) -> InlineKeyboardMarkup {
    let buttons: Vec<InlineKeyboardButton> = COMMON
        .iter()
        .map(|(name, tz)| {
            let label = if *tz == current {
                format!("✅ {name}")
            } else {
                name.to_string()
            };
            callback_button(label, format!("tz {}", tz.name()))
        })
        .collect();
    InlineKeyboardMarkup::new(buttons.chunks(3).map(<[_]>::to_vec).collect::<Vec<_>>())
}

pub async fn timezone(bot: &Bot, msg: &Message) -> BotResult {
    let command =
        TimezoneCmd::parse_i18n_from_bot(getor(msg).unwrap().split_whitespace(), Some("zh-CN"))?;
    let chat_id = msg.chat().id();
    let reply =
        |text: String| SendMessage::new(chat_id, text).reply_parameters(reply_to(msg.message_id()));
    let Some(input) = command.zone else {
        let tz = chat_timezone(chat_id).await;
        bot.send(reply(status_text(tz)).reply_markup(zone_menu(tz)))
            .await?;
        return Ok(());
    };
    let Some(tz) = parse_timezone(&input) else {
        return Err(BotError::Custom(format!(
            "认不出时区「{input}」\n可以用城市名（如 东京）、时区名（如 东九区、Asia/Tokyo）或整点偏移（如 +9、UTC-5）"
        )));
    };
    let user_id = msg
        .from()
        .ok_or(BotError::Custom("failed to get uid".to_string()))?
        .id;
    if !auth(bot, msg, user_id).await? {
        bot.send(reply("只有管理员可以修改本群时区".to_string()))
            .await?;
        return Ok(());
    }
    set_chat_timezone(chat_id, tz).await?;
    bot.send(reply(status_text(tz)).reply_markup(zone_menu(tz)))
        .await?;
    Ok(())
}

/// A tap on one of the zone buttons; the button carries the zone's name.
pub async fn timezone_callback(bot: Bot, q: CallbackQuery) -> BotResult {
    let (Some(name), Some(MaybeInaccessibleMessage::Message(msg))) =
        (q.data.clone(), q.message.map(|msg| *msg))
    else {
        return Ok(());
    };
    if !auth(&bot, &msg, q.from.id).await? {
        bot.send(
            AnswerCallbackQuery::new(q.id)
                .text("只有管理员可以修改本群时区")
                .show_alert(true),
        )
        .await?;
        return Ok(());
    }
    let Ok(tz) = name.parse::<Tz>() else {
        return Ok(());
    };
    let chat_id = msg.chat().id();
    if chat_timezone(chat_id).await != tz {
        set_chat_timezone(chat_id, tz).await?;
        bot.send(
            EditMessageText::new()
                .chat_id(chat_id)
                .message_id(msg.message_id())
                .text(status_text(tz))
                .reply_markup(zone_menu(tz)),
        )
        .await?;
    }
    bot.send(AnswerCallbackQuery::new(q.id).text(format!("本群时区：{}", zone_name(tz))))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_offsets_are_a_zone_not_an_option() {
        let parse = |text: &str| {
            TimezoneCmd::parse_i18n_from_bot(text.split_whitespace(), Some("zh-CN"))
                .unwrap()
                .zone
        };
        assert_eq!(parse("/timezone -5").as_deref(), Some("-5"));
        assert_eq!(parse("/tz 东京").as_deref(), Some("东京"));
        assert_eq!(parse("/timezone"), None);
    }

    #[test]
    fn menu_ticks_the_current_zone() {
        let menu = zone_menu(Tz::Asia__Tokyo);
        let labels: Vec<_> = menu
            .inline_keyboard
            .iter()
            .flatten()
            .map(|button| button.text.to_string())
            .collect();
        assert_eq!(labels.len(), COMMON.len());
        assert_eq!(labels.iter().filter(|l| l.starts_with('✅')).count(), 1);
        assert!(labels.contains(&"✅ 东京".to_string()));
    }
}
