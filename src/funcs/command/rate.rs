use super::*;
use reqwest::Client;
use scraper::{Html, Selector};

lazy_static! {
    static ref USAGE: String = RateCmd::command().render_help().to_string();
    static ref CLIENT: Client = reqwest::Client::new();
}

error_fmt!(USAGE);

#[derive(Parser)]
#[command(
    help_template = "使用方法：{usage}\n\n{all-args}\n\n{about}",
    about = "命令功能：获取实时汇率",
    name = "/rate",
    next_help_heading = "参数解释",
    disable_help_flag = true
)]

struct RateCmd {
    /// 原货币
    from: String,

    /// 目标货币
    #[arg(default_value_t = String::from("USD"))]
    to: String,
}

//This function code was contributed by @Misaka_master
async fn coin_exchange(from: &str, to: &str) -> Result<String, AppError> {
    if from == to {
        return Ok("1.0000".to_string());
    }
    let r = CLIENT.get(format!("https://www.google.com/finance/quote/{from}-{to}?hl=zh")).header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/115.0.0.0 Safari/537.36")
    .send()
    .await?;
    let html = r.text().await.unwrap();
    let html = Html::parse_document(&html);
    let selector = Selector::parse("#yDmH0d > c-wiz.zQTmif.SSPGKf.u5wqUe > div > div.e1AOyf > div > main > div.Gfxi4 > div.yWOrNb > div.VfPpkd-WsjYwc.VfPpkd-WsjYwc-OWXEXe-INsAgc.KC1dQ.Usd1Ac.AaN0Dd.QZMA8b > c-wiz > div > div:nth-child(1) > div > div.rPF6Lc > div > div:nth-child(1) > div > span > div").unwrap();
    let got = html.select(&selector).collect::<Vec<_>>();
    if got.len() == 0 {
        return Err(AppError::CustomError("不支持的货币单位".to_string()));
    }
    let rate = format!("{:?}", got[0].text().next().unwrap());
    Ok(rate[1..rate.len() - 1].to_string())
}

async fn get_rate(msg: &Message) -> Result<String, AppError> {
    let rate = RateCmd::try_parse_from(getor(msg).unwrap().to_uppercase().split_whitespace())
        .map_err(AppError::from)?;
    let exchange_rate = coin_exchange(&rate.from, &rate.to).await?;
    Ok(format!(
        "1{} \\= *`{}`* {}",
        rate.from, exchange_rate, rate.to
    ))
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
