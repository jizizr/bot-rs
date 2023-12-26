use super::*;
use regex::Regex;

lazy_static! {
    static ref HIS_MATCH: Regex = Regex::new(r#"</em>\.(.*?)</dt>"#).unwrap();
    static ref LINK_MATCH: Regex =
        Regex::new(r#"<a href="(.*?)" target="_blank" class="read-btn">阅读全文</a>"#).unwrap();
}

cmd!(
    "/today",
    "获取历史上的今天",
    TodayCmd,
    {
        /// 月
        month: Option<u8>,
        /// 日
        day: Option<u8>,
    }
);

async fn get_today(msg: &Message) -> Result<String, AppError> {
    let base_url = "http://hao.360.com/histoday/".to_string();
    let today = TodayCmd::try_parse_from(getor(&msg).unwrap().split_whitespace())
        .map_err(AppError::from)?;
    let his = if today.month.is_some() {
        if today.day.is_some() {
            get_history(
                format!(
                    "{}{:02}{:02}.html",
                    base_url,
                    today.month.unwrap(),
                    today.day.unwrap()
                ),
                Some(format!(
                    "{}月{}日",
                    today.month.unwrap(),
                    today.day.unwrap()
                )),
            )
            .await?
        } else {
            Err(AppError::CustomError(format!("日期不完整\n")))?
        }
    } else {
        get_history(base_url, None).await?
    };
    Ok(his)
}

pub async fn today(bot: Bot, msg: Message) -> BotResult {
    let text = get_today(&msg).await.unwrap_or_else(|e| format!("{e}"));
    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::MarkdownV2)
        .disable_web_page_preview(true)
        .reply_to_message_id(msg.id)
        .await?;
    Ok(())
}

async fn get_history(url: String, time: Option<String>) -> Result<String, AppError> {
    let req = reqwest::get(url).await?;
    if req.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(AppError::CustomError("日期范围错误".to_string()));
    }
    let rstring = req.text().await?;
    Ok(format!(
        "{}历史上发生了：\n{}",
        time.unwrap_or("今天".to_string()),
        HIS_MATCH
            .captures_iter(&rstring)
            .zip(LINK_MATCH.captures_iter(&rstring))
            .enumerate()
            .map(|(i, (text, link))| format!(
                "{}\\. [{}]({})\n",
                i + 1,
                markdown::escape(&text[1]),
                markdown::escape_link_url(&link[1])
            ))
            .collect::<String>()
    ))
}
