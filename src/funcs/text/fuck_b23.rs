use super::*;
use reqwest::Url;
lazy_static! {
    static ref MATCHS: Vec<RegexSign> = vec![
        RegexSign::new(BiliUrl::B23, r#"(https?://)b23\.tv/\w+"#),
        RegexSign::new(
            BiliUrl::Bili,
            r#"(https?://)(.+\.)?bilibili.com[/\w+]+\?[\w+=&]*"#
        )
    ];
    static ref CLIENT: ClientWithMiddleware = retry_client(reqwest::Client::new(), 2);
}

enum BiliUrl {
    B23,
    Bili,
}

struct RegexSign {
    pub sign: BiliUrl,
    pub re: Regex,
}

impl RegexSign {
    fn new(sign: BiliUrl, regex_str: &str) -> Self {
        Self {
            sign,
            re: Regex::new(regex_str).unwrap(),
        }
    }
}

impl BiliUrl {
    async fn get_safe_url(&self, url: &str) -> Result<Url, BotError> {
        match self {
            BiliUrl::B23 => Ok(CLIENT
                .get(url)
                .send()
                .await?
                .error_for_status()?
                .url()
                .to_owned()),
            BiliUrl::Bili => Ok(Url::parse(url)?),
        }
    }
}

pub async fn fuck_b23(bot: &Bot, msg: &Message) -> BotResult {
    let text = getor(msg).unwrap();
    let mut replaced = text.to_string();
    let mut offset: isize = 0;
    let mut flag = false;
    for ms in MATCHS.iter() {
        for m in ms.re.find_iter(text) {
            let url = m.as_str();
            let u = ms.sign.get_safe_url(url).await?;
            println!("{:#?}", u);

            let new_url = format!("https://{}{}", u.host().unwrap(), u.path());
            // 去除原有文本中链接跟踪参数
            replaced.replace_range(
                ((m.start() as isize + offset) as usize)..((m.end() as isize + offset) as usize),
                &new_url,
            );
            offset += new_url.len() as isize - url.len() as isize;
            flag = true;
        }
    }
    if flag {
        bot.send_message(msg.chat.id, replaced).send().await?;
    }
    Ok(())
}
