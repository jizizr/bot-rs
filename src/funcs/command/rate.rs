use super::*;
use scraper::{Html, Selector};

lazy_static! {
    static ref CLIENT: ClientWithMiddleware = retry_client(reqwest::Client::new(), 2);
}

cmd!(
    "/rate",
    "获取实时汇率",
    RateCmd,
    {
        ///原货币 [数量{defult:1}]+(货币单位)
        from: String,
        ///目标货币
        #[arg(default_value_t = String::from("CNY"))]
        to: String,
    }
);

//This function code was contributed by @Misaka_master
async fn coin_exchange(from: &str, to: &str) -> Result<String, AppError> {
    let (num, from) = parse(from)?;
    let exchange_rate: f64;
    if from == to {
        exchange_rate = 1.0;
    } else {
        let r = CLIENT.get(format!("https://www.google.com/finance/quote/{from}-{to}?hl=zh")).header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/115.0.0.0 Safari/537.36")
    .send()
    .await?;
        let html = r.text().await.unwrap();
        let html = Html::parse_document(&html);
        let selector = Selector::parse("#yDmH0d > c-wiz.zQTmif.SSPGKf.u5wqUe > div > div.e1AOyf > div > main > div.Gfxi4 > div.yWOrNb > div.VfPpkd-WsjYwc.VfPpkd-WsjYwc-OWXEXe-INsAgc.KC1dQ.Usd1Ac.AaN0Dd.QZMA8b > c-wiz > div > div:nth-child(1) > div > div.rPF6Lc > div > div:nth-child(1) > div > span > div").unwrap();
        let got = html.select(&selector).collect::<Vec<_>>();
        if got.is_empty() {
            return Err(AppError::CustomError("不支持的货币单位".to_string()));
        }
        let rate = format!("{:?}", got[0].text().next().unwrap());
        exchange_rate = rate[1..rate.len() - 1].parse().unwrap_or(0.0);
    }
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
    Err(AppError::CustomError("解析错误".to_string()))
}

pub async fn rate(bot: Bot, msg: Message) -> BotResult {
    let text = get_rate(&msg)
        .await
        .unwrap_or_else(|e| markdown::escape(&format!("{e}")));
    bot.send_message(msg.chat.id, text)
        .reply_to_message_id(msg.id)
        .parse_mode(ParseMode::MarkdownV2)
        .send()
        .await?;
    Ok(())
}
