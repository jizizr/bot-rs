use super::*;
use fast_qr::{
    convert::{Builder, Shape, svg::SvgBuilder},
    qr::QRBuilder,
};
use serde_json::json;
use std::sync::LazyLock;

static MATCH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(\s|^|https?://)([^:\./\s]+\.)+[^\d\./:\s\\"]{2,}(:(\d{1,4}|[1-5]\d{4}|6[0-4]\d{3}|65[0-4]\d{2}|655[0-2]\d|6553[0-5]))?(/\S*)*(\s|$)"#).unwrap()
});
static CLIENT: LazyLock<ClientWithMiddleware> =
    LazyLock::new(|| retry_client(reqwest::Client::new(), 2));

cmd!(
    "/short",
    "缩短长链接",
    ShortCmd,
    {
        ///长链接
        url: Option<String>,
        ///短链后缀
        surl: Option<String>,
    }
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
    format!("http://{url}")
}

async fn get_short(msg: &Message) -> Result<String, BotError> {
    let language_tag = Some("zh-CN");
    let short =
        ShortCmd::parse_i18n_from_bot(getor(msg).unwrap().split_whitespace(), language_tag)?;
    let surl;
    let mut url: String = match (short.surl, msg.reply_to_message(), &short.url) {
        (Some(s), _, _) => {
            surl = Some(s);
            short.url.unwrap()
        }
        (_, Some(reply), _) => {
            surl = short.url;
            getor(reply)
                .ok_or_else(|| BotError::Custom("被回复的消息没有文本".to_string()))?
                .to_string()
        }
        (_, _, Some(url_value)) => {
            surl = None;
            url_value.to_string()
        }
        _ => return Err(BotError::Custom("用法详解：".to_string())),
    };
    url = MATCH
        .find(&url)
        .ok_or_else(|| BotError::Custom("匹配不到符合规则的URL".to_string()))?
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
        Err(BotError::Custom("指定的短域已被占用".to_string()))
    } else {
        Err(BotError::Custom("未知错误".to_string()))
    }
}

fn url2qr(url: &str) -> Result<Vec<u8>, BotError> {
    let qrcode = QRBuilder::new(url)
        .build()
        .map_err(|e| BotError::Custom(format!("二维码生成失败: {e:?}")))?;

    let svg = SvgBuilder::default()
        .shape(Shape::RoundedSquare)
        .background_color([255, 255, 255, 0]) // Handles transparency
        .to_str(&qrcode);
    // 用 charts-rs 自带的 resvg 栅格化，省掉 fast_qr 的另一套旧版 resvg
    Ok(charts_rs::svg_to_png_with_size(&svg, Some(150), None)?)
}

pub async fn short(bot: &Bot, msg: &Message) -> BotResult {
    show_typing(bot, msg.chat().id());
    let url = get_short(msg).await?;
    bot.send(
        SendPhoto::new(msg.chat().id(), InputFile::buffered(url2qr(&url)?))
            .caption(format!("短链接：{url}")),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_is_a_150px_png() {
        let png = url2qr("https://example.com/s/abc").unwrap();
        let qr = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap();
        assert_eq!(qr.width(), 150);
        assert_eq!(qr.width(), qr.height());
    }
}
