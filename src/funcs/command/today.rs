use super::*;
use std::fmt::Write;

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

async fn get_today(msg: &Message) -> Result<String, BotError> {
    let base_url = "http://hao.360.com/histoday/".to_string();
    let today =
        TodayCmd::try_parse_from(getor(msg).unwrap().split_whitespace()).map_err(ccerr!())?;
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
            Err(BotError::Custom("日期不完整\n".to_string()))?
        }
    } else {
        get_history(base_url, None).await?
    };
    Ok(his)
}

pub async fn today(bot: &Bot, msg: &Message) -> BotResult {
    bot.send_message(msg.chat.id, get_today(msg).await?)
        .parse_mode(ParseMode::MarkdownV2)
        .link_preview_options(LinkPreviewOptions {
            is_disabled: true,
            url: None,
            prefer_small_media: false,
            prefer_large_media: false,
            show_above_text: false,
        })
        .reply_parameters(ReplyParameters::new(msg.id))
        .await?;
    Ok(())
}

async fn get_history(url: String, time: Option<String>) -> Result<String, BotError> {
    let req = reqwest::get(url).await?;
    if req.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(BotError::Custom("日期范围错误".to_string()));
    }
    let rstring = req.text().await?;
    Ok(format!(
        "{}历史上发生了：\n{}",
        time.unwrap_or("今天".to_string()),
        HIS_MATCH
            .captures_iter(&rstring)
            .zip(LINK_MATCH.captures_iter(&rstring))
            .enumerate()
            .fold(String::new(), |mut acc, (i, (text, link))| {
                let _ = writeln!(
                    acc,
                    "{}\\. [{}]({})",
                    i + 1,
                    markdown::escape(&text[1]),
                    markdown::escape_link_url(&link[1])
                );
                acc
            })
    ))
}
