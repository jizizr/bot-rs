use super::*;
use clap::{CommandFactory, Parser};
use regex::Regex;
use reqwest;

lazy_static::lazy_static! {
    static ref HIS_MATCH: Regex = Regex::new(r#"</em>\.(.*?)</dt>"#).unwrap();
    static ref LINK_MATCH:Regex = Regex::new(r#"<a href="(.*?)" target="_blank" class="read-btn">阅读全文</a>"#).unwrap();
    static ref USAGE:String = TodayCmd::command().render_help().to_string() ;
}

#[derive(Parser)]
#[command(
    help_template = "使用方法：{usage}\n\n{all-args}\n\n{about}",
    about = "命令功能：获取历史上的今天",
    name = "/today",
    next_help_heading = "参数解释",
    disable_help_flag = true
)]
struct TodayCmd {
    /// 月
    month: Option<u8>,
    /// 日
    day: Option<u8>,
}

//自定义错误类型
#[derive(Debug)]
pub enum AppError {
    RequestError(reqwest::Error),
    ClapError(clap::error::Error),
    CustomError(String),
}

impl From<reqwest::Error> for AppError {
    fn from(error: reqwest::Error) -> Self {
        AppError::RequestError(error)
    }
}

impl From<clap::error::Error> for AppError {
    fn from(error: clap::error::Error) -> Self {
        AppError::ClapError(error)
    }
}

impl From<String> for AppError {
    fn from(error: String) -> Self {
        AppError::CustomError(error)
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::RequestError(err) => write!(f, "API请求失败: {}", err),
            AppError::ClapError(err) => {
                write!(
                    f,
                    "{}",
                    format!(
                        "{}\n{}\n",
                        err.render().to_string().splitn(2, "Usage").nth(0).unwrap(),
                        *USAGE
                    )
                )
            }
            AppError::CustomError(err) => write!(f, "{}\n\n{}", err, *USAGE),
        }
    }
}

pub async fn get_today(ctx: &Context) -> Result<String, AppError> {
    let base_url = "http://hao.360.com/histoday/".to_string();
    let msg = ctx.effective_message.as_ref().unwrap();
    let today = TodayCmd::try_parse_from(msg.text.as_ref().unwrap().split_whitespace())
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
            Err(AppError::CustomError(format!("日期不完整\n\n")))?
        }
    } else {
        get_history(base_url, None).await?
    };
    Ok(his)
}

pub async fn today(bot: Bot, ctx: Context) -> FResult<GroupIteration> {
    let text = match get_today(&ctx).await {
        Ok(msg) => msg,
        Err(e) => format!("{e}"),
    };
    ctx.effective_message
        .unwrap()
        .reply(&bot, &text)
        .parse_mode("markdown".to_string())
        .disable_web_page_preview(true)
        .send()
        .await?;
    Ok(GroupIteration::EndGroups)
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
                "{}. [{}]({})\n",
                i + 1,
                text[1].to_string(),
                link[1].to_string()
            ))
            .collect::<String>()
    ))
}
