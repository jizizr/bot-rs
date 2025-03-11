use super::*;
use crate::load_json;
use cached::proc_macro::cached;
use chrono::prelude::*;
use std::collections::HashSet;

lazy_static! {
    static ref COIN_TYPES: [&'static str; 3] = ["BTC", "XMR", "ETH"];
    static ref COINS_SET: HashSet<String> = HashSet::from_iter(
        load_json::<Vec<String>>("./data/supported_coin_types.json").into_iter()
    );
}

#[derive(Deserialize)]
struct Coin {
    data: Vec<CoinData>,
}

#[derive(Deserialize)]
struct CoinData {
    #[serde(rename = "close")]
    price: f64,
}

#[cached(time = 10, result = true)]
async fn coin_price(coin_type: String) -> Result<f64, reqwest::Error> {
    let price: Coin = get(&format!(
        "https://api.huobi.pro/market/history/kline?&period=1min&size=1&symbol={}usdt",
        coin_type
    ))
    .await?;
    Ok(price.data[0].price)
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
            format!("Api 请求异常:{}", e)
        }
    }
}

fn popular_coins_menu() -> InlineKeyboardMarkup {
    let mut keyboard: Vec<Vec<InlineKeyboardButton>> = vec![vec![]; (COIN_TYPES.len() - 1) / 3 + 2];

    for (i, coins) in COIN_TYPES.chunks(3).enumerate() {
        let row = coins
            .iter()
            .map(|&coin_type| {
                InlineKeyboardButton::callback(coin_type, format!("coin {}", coin_type))
            })
            .collect();
        keyboard[i] = row
    }
    keyboard.push(vec![
        InlineKeyboardButton::switch_inline_query_current_chat("其他货币", ""),
    ]);
    InlineKeyboardMarkup::new(keyboard)
}

fn function_menu(coin_type: &str) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new([[
        InlineKeyboardButton::callback("返回🔙", "coin back".to_string()),
        InlineKeyboardButton::callback("刷新🔁", format!("coin {}", coin_type)),
        InlineKeyboardButton::switch_inline_query_current_chat("其他货币", ""),
    ]])
}

pub async fn coin(bot: &Bot, msg: &Message) -> BotResult {
    bot.send_message(msg.chat.id, "选择您要查询的虚拟货币")
        .reply_markup(popular_coins_menu())
        .await?;
    Ok(())
}

pub async fn coin_callback(bot: Bot, q: CallbackQuery) -> BotResult {
    if let Some(coin_type) = q.data {
        let text = coin_handle(&coin_type.to_lowercase()).await;
        bot.answer_callback_query(q.id).await?;
        if let Some(msg) = q.message {
            let _guard = lock!((msg.chat().id, msg.id()));
            if coin_type == "back" {
                bot.edit_message_text(msg.chat().id, msg.id(), "选择您要查询的虚拟货币")
                    .reply_markup(popular_coins_menu())
                    .await?;
                return Ok(());
            }
            bot.edit_message_text(msg.chat().id, msg.id(), text)
                .reply_markup(function_menu(&coin_type))
                .await?;
        } else if let Some(id) = q.inline_message_id {
            let _guard = lock!(&id);
            if coin_type == "back" {
                let _ = bot
                    .edit_message_text_inline(&id, "选择您要查询的虚拟货币")
                    .reply_markup(popular_coins_menu())
                    .await;
                return Ok(());
            }
            bot.edit_message_text_inline(&id, text)
                .reply_markup(function_menu(&coin_type))
                .await?;
        }
    }

    Ok(())
}

async fn inline_coin_handle(coin_type: &str) -> String {
    if coin_type.is_empty() {
        return "以下是热门虚拟货币查询\n如果不在下面的列表中，请点击\"其他\"并输入想要查找货币查询".to_string();
    } else if !COINS_SET.contains(coin_type) {
        return "不支持的虚拟货币".to_string();
    }
    coin_handle(coin_type).await
}

fn inline_keyboard(coin_type: &str) -> InlineKeyboardMarkup {
    if !COINS_SET.contains(coin_type) {
        popular_coins_menu()
    } else {
        function_menu(coin_type)
    }
}

pub async fn inline_query_handler(bot: Bot, q: InlineQuery) -> BotResult {
    let coins_query = InlineQueryResultArticle::new(
        "01".to_string(),
        "查询虚拟货币实时价格".to_string(),
        InputMessageContent::Text(InputMessageContentText::new(
            inline_coin_handle(&q.query.to_lowercase()).await,
        )),
    )
    .reply_markup(inline_keyboard(&q.query.to_lowercase()));
    let results = vec![InlineQueryResult::Article(coins_query)];
    let response = bot.answer_inline_query(&q.id, results).send().await;
    if let Err(err) = response {
        log::error!("Error in handler: {:?}", err);
    }
    Ok(respond(())?)
}
