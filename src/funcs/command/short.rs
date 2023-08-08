use super::*;
use regex::Regex;
use serde_json::json;
lazy_static! {
    static ref USAGE: String = ShortCmd::command().render_help().to_string();
    static ref MATCH: Regex = Regex::new(r#"(\s|^|https?://)([^:\./\s]+\.)+[^\d\./:\s\\"]{2,}(:(\d{1,4}|[1-5]\d{4}|6[0-4]\d{3}|65[0-4]\d{2}|655[0-2]\d|6553[0-5]))?(/\S*)*(\s|$)"#).unwrap();
    static ref CLIENT:reqwest::Client = reqwest::Client::new();
}

error_fmt!(USAGE);

#[derive(Parser)]
#[command(
    help_template = "使用方法：{usage}\n\n{all-args}\n\n{about}",
    about = "命令功能：缩短长链接",
    name = "/short",
    next_help_heading = "参数解释",
    disable_help_flag = true
)]
struct ShortCmd {
    ///短链后缀
    surl: Option<String>,
    ///长链接
    url: Option<String>,
}

#[derive(Deserialize)]
struct Short {
    code: String,
    shorturl: String,
}

fn fix_start(u: String) -> String {
    let url = u.trim().to_string();
    if url.starts_with("http://") || url.starts_with("https://") {
        return url;
    }
    format!("http://{}", url)
}

async fn get_short(msg: &Message) -> Result<String, AppError> {
    let short = ShortCmd::try_parse_from(getor(msg).unwrap().split_whitespace())?;
    let mut url = short
        .url
        .or_else(|| {
            msg.reply_to_message()
                .map(|m| getor(m).unwrap().to_string())
        })
        .ok_or(AppError::CustomError("用法详解：".to_string()))?;
    let request_body;
    url = MATCH
        .find(&url)
        .ok_or_else(|| AppError::CustomError("匹配不到符合规则的URL".to_string()))?
        .as_str()
        .to_string();
    match short.surl {
        None => request_body = json!({"url":fix_start(url)}),
        Some(s) => request_body = json!({"url":fix_start(url),"shorturl":s}),
    }
    let post_result: Short = CLIENT
        .post("https://774.gs/api.php")
        .form(&request_body)
        .send()
        .await?
        .json()
        .await?;
    if post_result.code == 200.to_string() {
        return Ok(format!("短链接：https://774.gs/{}", post_result.shorturl));
    } else if post_result.code == 2003.to_string() {
        return Err(AppError::CustomError("指定的短域已被占用".to_string()));
    } else {
        return Err(AppError::CustomError("未知错误".to_string()));
    }
}

pub async fn short(bot: Bot, msg: Message) -> Result<(), Box<dyn Error + Send + Sync>> {
    let text = get_short(&msg).await.unwrap_or_else(|e| format!("{e}"));
    bot.send_message(msg.chat.id, text)
        .reply_to_message_id(msg.id)
        .await?;
    Ok(())
}
