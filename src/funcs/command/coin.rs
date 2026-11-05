use super::*;
use cached::proc_macro::cached;
use chrono::prelude::*;
use std::sync::LazyLock;

static COIN_TYPES: LazyLock<[&'static str; 3]> = LazyLock::new(|| ["BTC", "XMR", "ETH"]);

#[derive(Deserialize)]
struct Coin {
    data: Vec<CoinData>,
}

#[derive(Deserialize)]
struct CoinData {
    #[serde(rename = "close")]
    price: f64,
}

#[cached(ttl_secs = 10)]
async fn coin_price(coin_type: String) -> Result<f64, BotError> {
    let price: Coin = get(&format!(
        "https://api.huobi.pro/market/history/kline?&period=1min&size=1&symbol={coin_type}usdt"
    ))
    .await?;
    price
        .data
        .first()
        .map(|d| d.price)
        .ok_or_else(|| BotError::Custom("未获取到价格数据".to_string()))
}

async fn coin_handle(coin_type: &str) -> String {
    match coin_price(coin_type.to_string()).await {
        Ok(price) => {
            format!(
                "1.0 {} = {price} USDT\n最后更新于：{}",
                coin_type.to_uppercase(),
                Utc::now()
                    .with_timezone(&FixedOffset::east_opt(8 * 3600).unwrap())
                    .format("%Y-%m-%d %H:%M:%S%.3f")
            )
        }
        Err(e) => {
            format!("Api 请求异常:{e}")
        }
    }
}

fn popular_coins_menu() -> InlineKeyboardMarkup {
    let keyboard: Vec<Vec<InlineKeyboardButton>> = COIN_TYPES
        .chunks(3)
        .map(|coins| {
            coins
                .iter()
                .map(|&coin_type| callback_button(coin_type, format!("coin {coin_type}")))
                .collect()
        })
        .collect();
    InlineKeyboardMarkup::new(keyboard)
}

fn function_menu(coin_type: &str) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new([[
        callback_button("返回🔙", "coin back".to_string()),
        callback_button("刷新🔁", format!("coin {coin_type}")),
    ]])
}

pub async fn coin(bot: &Bot, msg: &Message) -> BotResult {
    bot.send(
        SendMessage::new(msg.chat().id(), "选择您要查询的虚拟货币")
            .reply_markup(popular_coins_menu()),
    )
    .await?;
    Ok(())
}

pub async fn coin_callback(bot: Bot, q: CallbackQuery) -> BotResult {
    if let Some(coin_type) = q.data {
        bot.send(AnswerCallbackQuery::new(q.id)).await?;
        if let Some(msg) = q.message {
            let _guard = lock!((msg.chat().id(), msg.message_id()));
            if &*coin_type == "back" {
                bot.send(
                    EditMessageText::new()
                        .chat_id(msg.chat().id())
                        .message_id(msg.message_id())
                        .text("选择您要查询的虚拟货币")
                        .reply_markup(popular_coins_menu()),
                )
                .await?;
                return Ok(());
            }
            let text = coin_handle(&coin_type.to_lowercase()).await;
            bot.send(
                EditMessageText::new()
                    .chat_id(msg.chat().id())
                    .message_id(msg.message_id())
                    .text(text)
                    .reply_markup(function_menu(&coin_type)),
            )
            .await?;
        } else if let Some(id) = q.inline_message_id {
            let _guard = lock!(&id);
            if &*coin_type == "back" {
                bot.send(
                    EditMessageText::new()
                        .inline_message_id(id.clone())
                        .text("选择您要查询的虚拟货币")
                        .reply_markup(popular_coins_menu()),
                )
                .await?;
                return Ok(());
            }
            let text = coin_handle(&coin_type.to_lowercase()).await;
            bot.send(
                EditMessageText::new()
                    .inline_message_id(id.clone())
                    .text(text)
                    .reply_markup(function_menu(&coin_type)),
            )
            .await?;
        }
    }

    Ok(())
}
