use super::*;

#[derive(Deserialize)]
struct Coin {
    price: String,
}

async fn coin(coin_type: &str) -> Result<f32, reqwest::Error> {
    // tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let price: Result<Coin, reqwest::Error> = get(&format!(
        "https://api.binance.com/api/v3/ticker/price?symbol={}USDT",
        coin_type
    ))
    .await;
    price.map(|x| x.price.parse().unwrap())
}

async fn coin_handle(coin_type: &str) -> String {
    match coin(coin_type).await {
        Ok(price) => format!("1.0 {coin_type} = {price} USDT"),
        Err(_) => "Api 请求异常".to_string(),
        // Err(e) => format!("{e}"),
    }
}

macro_rules! generate_crypto_fn {
    ($coin_type:ident) => {
        pub async fn $coin_type(bot: Bot, ctx: Context) -> FResult<GroupIteration> {
            let msg = ctx.effective_message.unwrap();
            msg.reply(
                &bot,
                coin_handle(&stringify!($coin_type).to_uppercase())
                    .await
                    .as_str(),
            )
            .send()
            .await?;
            Ok(GroupIteration::EndGroups)
        }
    };
}

generate_crypto_fn!(btc);
generate_crypto_fn!(eth);
generate_crypto_fn!(xmr);
