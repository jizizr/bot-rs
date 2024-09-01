use super::*;
use fast_qr::{
    convert::{image::ImageBuilder, Builder, Shape},
    qr::QRBuilder,
};
use serde_json::json;
use teloxide::types::InputFile;

lazy_static! {
    static ref MATCH: Regex = Regex::new(r#"(\s|^|https?://)([^:\./\s]+\.)+[^\d\./:\s\\"]{2,}(:(\d{1,4}|[1-5]\d{4}|6[0-4]\d{3}|65[0-4]\d{2}|655[0-2]\d|6553[0-5]))?(/\S*)*(\s|$)"#).unwrap();
    static ref CLIENT:ClientWithMiddleware = retry_client(reqwest::Client::new(),2);
}

cmd!(
    "/short",
    "缩短长链接",
    ShortCmd,
    {
        ///长链接
        url: Option<String>,
        ///短链后缀
        surl: Option<String>,
    },
);

#[derive(Deserialize)]
struct Short {
    code: String,
    shorturl: String,
}

pub fn fix_start(u: String) -> String {
    let url = u.trim().to_string();
    if url.starts_with("http://") || url.starts_with("https://") {
        return url;
    }
    format!("http://{}", url)
}

async fn get_short(msg: &Message) -> Result<String, AppError> {
    let short = ShortCmd::try_parse_from(getor(msg).unwrap().split_whitespace())?;
    let surl;
    let mut url: String = match (short.surl, msg.reply_to_message(), &short.url) {
        (Some(s), _, _) => {
            surl = Some(s);
            short.url.unwrap()
        }
        (_, Some(reply), _) => {
            surl = short.url;
            getor(reply).unwrap().to_string()
        }
        (_, _, Some(url_value)) => {
            surl = None;
            url_value.to_string()
        }
        _ => return Err(AppError::Custom("用法详解：".to_string())),
    };
    url = MATCH
        .find(&url)
        .ok_or_else(|| AppError::Custom("匹配不到符合规则的URL".to_string()))?
        .as_str()
        .to_string();
    let request_body = match surl {
        None => json!({"url":fix_start(url)}),
        Some(s) => json!({"url":fix_start(url),"shorturl":s}),
    };
    let post_result: Short = CLIENT
        .post("https://774.gs/api.php")
        .form(&request_body)
        .send()
        .await?
        .json()
        .await?;
    if post_result.code == 200.to_string() {
        Ok(format!("https://774.gs/{}", post_result.shorturl))
    } else if post_result.code == 2003.to_string() {
        return Err(AppError::Custom("指定的短域已被占用".to_string()));
    } else {
        return Err(AppError::Custom("未知错误".to_string()));
    }
}

fn url2qr(url: &str) -> Vec<u8> {
    let qrcode = QRBuilder::new(url).build().unwrap();

    ImageBuilder::default()
        .shape(Shape::RoundedSquare)
        .background_color([255, 255, 255, 0]) // Handles transparency
        .fit_width(150)
        .to_pixmap(&qrcode)
        .encode_png()
        .unwrap()
}

pub async fn short(bot: Bot, msg: Message) -> BotResult {
    match get_short(&msg).await {
        Ok(url) => {
            bot.send_photo(msg.chat.id, InputFile::memory(url2qr(&url)))
                .caption(format!("短链接：{}", url))
                .send()
                .await?
        }
        Err(e) => {
            bot.send_message(msg.chat.id, format!("{e}"))
                .reply_parameters(ReplyParameters::new(msg.id))
                .send()
                .await?
        }
    };
    Ok(())
}
