use super::*;
use serde_json::Value;

lazy_static! {
    static ref CLIENT: ClientWithMiddleware =
        retry_client(reqwest::Client::builder().build().unwrap(), 2);
}

cmd!(
    "/translate",
    "翻译",
    TranslateCmd,
    {
        ///翻译内容
        #[arg(required = true)]
        content: Vec<String>,
    },
);

fn extract_data(json_data: Value) -> Result<Vec<(String, String)>, AppError> {
    let extracted = json_data
        .as_array()
        .and_then(|array| array.first()?.as_array())
        .map(|first_element| {
            first_element
                .iter()
                .filter_map(|item| {
                    if let Value::Array(inner_array) = item {
                        let first = inner_array.first()?.as_str()?;
                        let second = inner_array.get(1)?.as_str()?;
                        Some((first.to_string(), second.to_string()))
                    } else {
                        None
                    }
                })
                .collect()
        })
        .ok_or(AppError::Custom("Invalid structure".to_string()))?;

    Ok(extracted)
}

async fn translate_req(tl: &str, text: &str, is_compare: bool) -> Result<String, AppError> {
    let url_str = format!(
        "https://translate.googleapis.com/translate_a/single?client=gtx&sl=auto&tl={}&dt=t&q={}",
        tl,
        urlencoding::encode(text),
    );
    Ok({
        let data = extract_data(
            CLIENT
                .get(url_str)
                .send()
                .await?
                .json::<Value>()
                .await
                .unwrap(),
        )?;
        let data_iter = data.iter();
        if is_compare {
            data_iter
                .map(|(a, b)| format!("{}\n{}", b, a))
                .collect::<Vec<String>>()
        } else {
            data_iter
                .map(|(a, _)| a.to_string())
                .collect::<Vec<String>>()
        }
        .join("\n")
    })
}

pub async fn translate_callback(bot: Bot, q: CallbackQuery) -> Result<(), AppError> {
    if let Some(translate) = q.data {
        bot.answer_callback_query(q.id).await?;
        let mut translate = translate.splitn(2, ' ');
        let msg = match q.message {
            None => return Ok(()),
            Some(msg) => msg,
        };
        let _guard = lock!((msg.chat.id, msg.id));
        let is_compare;
        match translate.next() {
            Some("one") => {
                is_compare = false;
            }
            Some("two") => {
                is_compare = true;
            }
            _ => {
                return Err(AppError::Custom(
                    "Unknown Error in [Translate translate_callback]".to_string(),
                ));
            }
        }
        match &msg.reply_to_message() {
            Some(m) => {
                bot.edit_message_text(
                    msg.chat.id,
                    msg.id,
                    get_translate(m, is_compare, true).await?.0,
                )
                .reply_markup(translate_menu(is_compare))
                .await?;
            }
            None => {
                bot.edit_message_text(msg.chat.id, msg.id, "待翻译文本已被删除")
                    .await?;
            }
        }
    }
    Ok(())
}

fn extract_text(message: &Message) -> Option<&str> {
    if let MessageKind::Common(common) = &message.kind {
        if let MediaKind::Text(media_text) = &common.media_kind {
            return Some(&media_text.text);
        }
    }
    None
}

async fn get_translate(
    msg: &Message,
    is_compare: bool,
    is_callback: bool,
) -> Result<(String, MessageId), AppError> {
    let (translate, mid) =
        match TranslateCmd::try_parse_from(getor(msg).unwrap().split_whitespace()) {
            Ok(translate) => (translate, msg.id),
            Err(e) => (
                TranslateCmd::try_parse_from(
                    [
                        "/translate",
                        if is_callback {
                            extract_text(msg)
                        } else {
                            msg.reply_to_message().and_then(|msg| msg.text())
                        }
                        .ok_or(e)?,
                    ]
                    .into_iter(),
                )?,
                match msg.reply_to_message() {
                    Some(m) => m.id,
                    None => msg.id,
                },
            ),
        };

    let text = translate.content.join(" ");
    let tl = "zh-CN";
    let translated = translate_req(tl, &text, is_compare).await?;
    Ok((translated, mid))
}

fn translate_menu(is_compare: bool) -> InlineKeyboardMarkup {
    if is_compare {
        InlineKeyboardMarkup::new([[InlineKeyboardButton::callback(
            "普通翻译模式",
            "trans one".to_string(),
        )]])
    } else {
        InlineKeyboardMarkup::new([[InlineKeyboardButton::callback(
            "对照翻译模式",
            "trans two".to_string(),
        )]])
    }
}

pub async fn translate(bot: Bot, msg: Message) -> BotResult {
    let is_compare = false;
    match get_translate(&msg, is_compare, false).await {
        Ok((text, mid)) => bot
            .send_message(msg.chat.id, text)
            .reply_markup(translate_menu(is_compare))
            .reply_to_message_id(mid),
        Err(e) => bot
            .send_message(msg.chat.id, format!("{e}"))
            .reply_to_message_id(msg.id),
    }
    .await?;
    Ok(())
}
