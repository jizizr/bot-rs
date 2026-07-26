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
    // 先收集两个正则的所有匹配并按位置升序排序，
    // 再统一应用替换，避免第二个正则的偏移被第一个正则的替换破坏
    let mut matches: Vec<(usize, usize, &BiliUrl)> = MATCHS
        .iter()
        .flat_map(|ms| {
            ms.re
                .find_iter(text)
                .map(move |m| (m.start(), m.end(), &ms.sign))
        })
        .collect();
    matches.sort_by_key(|(start, _, _)| *start);
    let mut replaced = text.to_string();
    let mut offset: isize = 0;
    let mut flag = false;
    for (start, end, sign) in matches {
        let url = &text[start..end];
        let u = sign.get_safe_url(url).await?;
        let new_url = format!("https://{}{}", u.host().unwrap(), u.path());
        // 去除原有文本中链接跟踪参数
        replaced.replace_range(
            ((start as isize + offset) as usize)..((end as isize + offset) as usize),
            &new_url,
        );
        offset += new_url.len() as isize - url.len() as isize;
        flag = true;
    }
    if flag {
        bot.send_message(msg.chat.id, replaced).send().await?;
    }
    Ok(())
}
