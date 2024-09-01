use super::*;

lazy_static! {
    static ref CLIENT: ClientWithMiddleware = retry_client(reqwest::Client::new(), 2);
}

const RATE_API: &str = "https://wise.com/rates/live";

cmd!(
    "/rate",
    "获取实时汇率",
    RateCmd,
    {
        ///原货币 [数量{defult:1}]+(货币单位)
        from: String,
        ///目标货币
        #[arg(default_value_t = String::from("CNY"),value_parser  = is_alphabetic)]
        to: String,
    },
);

fn is_alphabetic(value: &str) -> Result<String, String> {
    if value.chars().all(|c| c.is_alphabetic()) {
        Ok(value.to_string())
    } else {
        Err(String::from("货币单位格式错误"))
    }
}

#[derive(Deserialize)]
struct RateResponse {
    value: f64,
}

async fn get_exchange_rate(from: &str, to: &str) -> Result<f64, AppError> {
    if from == to {
        return Ok(1.0);
    }
    let resp = CLIENT
        .get(RATE_API)
        .query(&[("source", from), ("target", to)])
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(AppError::Custom("不支持的货币".to_string()));
    }
    let rate: RateResponse = resp.json().await?;
    Ok(rate.value)
}

async fn coin_exchange(from: &str, to: &str) -> Result<String, AppError> {
    let (num, from) = parse(from)?;
    let exchange_rate = get_exchange_rate(from, to).await?;
    let mut answer = String::new();
    answer.push_str(&format!(
        "*`1`* {} \\= *`{:.4}`* {}\n",
        from, exchange_rate, to
    ));
    if num != 1.0 {
        answer.push_str(&format!(
            "*`{}`* {} \\= *`{:.4}`* {}",
            num,
            from,
            exchange_rate * num,
            to
        ));
    }
    Ok(answer)
}

async fn get_rate(msg: &Message) -> Result<String, AppError> {
    let rate = RateCmd::try_parse_from(getor(msg).unwrap().to_uppercase().split_whitespace())
        .map_err(AppError::from)?;
    coin_exchange(&rate.from, &rate.to).await
}

fn parse(raw: &str) -> Result<(f64, &str), AppError> {
    let iter = raw.chars().enumerate().peekable();
    for (i, c) in iter {
        if !c.is_ascii_digit() && c != '.' {
            if i == 0 {
                return Ok((1.0, raw));
            } else {
                return Ok((raw[..i].parse().unwrap_or(0.0), &raw[i..]));
            }
        }
    }
    Err(AppError::Custom("解析错误".to_string()))
}

pub async fn rate(bot: Bot, msg: Message) -> BotResult {
    let text = get_rate(&msg)
        .await
        .unwrap_or_else(|e| markdown::escape(&format!("{e}")));
    bot.send_message(msg.chat.id, text)
        .reply_parameters(ReplyParameters::new(msg.id))
        .parse_mode(ParseMode::MarkdownV2)
        .send()
        .await?;
    Ok(())
}
