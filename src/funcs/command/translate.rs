use super::*;
use lingua::{Language, LanguageDetector, LanguageDetectorBuilder};
use serde_json::Value;
use std::sync::LazyLock;
static CLIENT: LazyLock<ClientWithMiddleware> =
    LazyLock::new(|| retry_client(reqwest::Client::builder().build().unwrap(), 2));
static LANG: LazyLock<LanguageDetector> = LazyLock::new(|| {
    LanguageDetectorBuilder::from_languages(&[Language::English, Language::Chinese])
        .with_preloaded_language_models()
        .build()
});

const CN: &str = "ZH";
const EN: &str = "EN";

cmd!(
    "/translate",
    "翻译",
    TranslateCmd,
    {
        ///翻译内容
        #[arg(required = true)]
        content: Vec<String>,
    }
);

fn extract_data(json_data: Value) -> Result<Vec<(String, String)>, BotError> {
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
        .ok_or(BotError::Custom("Invalid structure".to_string()))?;

    Ok(extracted)
}

async fn translate_req(tl: &str, text: &str, is_compare: bool) -> Result<String, BotError> {
    let url_str = format!(
        "https://translate.googleapis.com/translate_a/single?client=gtx&sl=auto&tl={}&dt=t&q={}",
        tl,
        urlencoding::encode(text),
    );
    Ok({
        let data = extract_data(CLIENT.get(url_str).send().await?.json::<Value>().await?)?;
        let data_iter = data.iter();
        if is_compare {
            data_iter
                .map(|(a, b)| format!("{b}\n{a}"))
                .collect::<Vec<String>>()
        } else {
            data_iter
                .map(|(a, _)| a.to_string())
                .collect::<Vec<String>>()
        }
        .join("\n")
    })
}

fn compare_info(msg: &Message) -> Result<bool, BotError> {
    if let Some(data) = msg
        .reply_markup()
        .ok_or(BotError::Custom("No reply_markup".to_string()))?
        .inline_keyboard[0][0]
        .callback_data
        .as_deref()
    {
        if data == "trans one" {
            Ok(true)
        } else {
            Ok(false)
        }
    } else {
        Err(BotError::Custom(
            "Unknown Error in [Translate compare_info]".to_string(),
        ))
    }
}

fn tl_info(msg: &Message) -> Result<&str, BotError> {
    if let Some(data) = msg
        .reply_markup()
        .ok_or(BotError::Custom("No reply_markup".to_string()))?
        .inline_keyboard[0][1]
        .callback_data
        .as_deref()
    {
        Ok(data.splitn(2, ' ').last().ok_or(BotError::Custom(
            "Unknown Error in [Translate tl_info]".to_string(),
        ))?)
    } else {
        Err(BotError::Custom(
            "Unknown Error in [Translate tl_info]".to_string(),
        ))
    }
}

pub async fn translate_callback(bot: Bot, q: CallbackQuery) -> Result<(), BotError> {
    if let Some(translate) = q.data {
        bot.send(AnswerCallbackQuery::new(q.id)).await?;
        let mut translate = translate.splitn(2, ' ');
        let mbi_msg = match q.message {
            None => return Ok(()),
            Some(msg) => msg,
        };
        let msg = match *mbi_msg {
            MaybeInaccessibleMessage::InaccessibleMessage(_) => return Ok(()),
            MaybeInaccessibleMessage::Message(msg) => msg,
        };
        let _guard = lock!((msg.chat().id(), msg.message_id()));
        let is_compare;
        let tl;
        match translate.next() {
            Some("one") => {
                is_compare = false;
                tl = tl_info(&msg)?;
            }
            Some("two") => {
                is_compare = true;
                tl = tl_info(&msg)?;
            }
            Some(CN) => {
                is_compare = compare_info(&msg)?;
                tl = EN;
            }
            Some(EN) => {
                is_compare = compare_info(&msg)?;
                tl = CN;
            }
            _ => {
                return Err(BotError::Custom(
                    "Unknown Error in [Translate translate_callback]".to_string(),
                ));
            }
        }
        match &msg.reply_to_message() {
            Some(m) => {
                bot.send(
                    EditMessageText::new()
                        .chat_id(msg.chat().id())
                        .message_id(msg.message_id())
                        .text(get_translate(m, Some(tl), is_compare, true).await?.0)
                        .reply_markup(translate_menu(is_compare, tl)),
                )
                .await?;
            }
            None => {
                bot.send(
                    EditMessageText::new()
                        .chat_id(msg.chat().id())
                        .message_id(msg.message_id())
                        .text("待翻译文本已被删除"),
                )
                .await?;
            }
        }
    }
    Ok(())
}

fn extract_text(message: &Message) -> Option<&str> {
    match message {
        Message::Text(text) => Some(&text.text),
        _ => None,
    }
}

async fn get_translate<'a>(
    msg: &'a Message,
    tl: Option<&'a str>,
    is_compare: bool,
    is_callback: bool,
) -> Result<(String, i64, &'a str), BotError> {
    let language_tag = Some("zh-CN");
    let (translate, mid) = match TranslateCmd::parse_i18n_from_bot(
        getor(msg).unwrap().split_whitespace(),
        language_tag,
    ) {
        Ok(translate) => (translate, msg.message_id()),
        Err(e) => (
            TranslateCmd::parse_i18n_from_bot(
                [
                    "/translate",
                    if is_callback {
                        extract_text(msg)
                    } else {
                        msg.reply_to_message().and_then(|msg| msg.text())
                    }
                    .ok_or(e)?,
                ],
                language_tag,
            )?,
            match msg.reply_to_message() {
                Some(m) => m.message_id(),
                None => msg.message_id(),
            },
        ),
    };
    let text = translate.content.join(" ");
    let tl = tl.unwrap_or_else(|| match LANG.detect_language_of(&text) {
        Some(Language::Chinese) => EN,
        _ => CN,
    });
    let translated = translate_req(tl, &text, is_compare).await?;
    Ok((translated, mid, tl))
}

fn translate_menu(is_compare: bool, tl: &str) -> InlineKeyboardMarkup {
    let mut buttom = Vec::new();
    buttom.push(if is_compare {
        callback_button("对照翻译模式", "trans one")
    } else {
        callback_button("普通翻译模式", "trans two")
    });
    buttom.push(callback_button(tl, format!("trans {tl}")));
    InlineKeyboardMarkup::new(vec![buttom])
}

pub async fn translate(bot: &Bot, msg: &Message) -> BotResult {
    let is_compare = false;
    let (text, mid, tl) = get_translate(msg, None, is_compare, false).await?;
    bot.send(
        SendMessage::new(msg.chat().id(), text)
            .reply_markup(translate_menu(is_compare, tl))
            .reply_parameters(reply_to(mid)),
    )
    .await?;
    Ok(())
}
