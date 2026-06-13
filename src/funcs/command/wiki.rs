use super::*;

lazy_static! {
    static ref MATCH: Regex = Regex::new(r#"<span class="searchmatch">|</span>"#).unwrap();
}

cmd!(
    "/wiki",
    "在中文维基百科中搜索词条",
    WikiCmd,
    {
        ///词条名
        #[arg(required = true)]
        search: Vec<String>,
    }
);

#[derive(Deserialize)]
struct SearchResult {
    query: Query,
}
#[derive(Deserialize)]
struct Query {
    searchinfo: Searchinfo,
    search: Vec<Page>,
}
#[derive(Deserialize)]
struct Searchinfo {
    totalhits: usize,
}
#[derive(Deserialize)]
struct Page {
    title: String,
    wordcount: usize,
    snippet: String,
}

async fn get_wiki(msg: &Message) -> Result<String, BotError> {
    let language_tag = Some("zh-CN");
    let search =
        WikiCmd::parse_i18n_from_bot(getor(msg).unwrap().split_whitespace(), language_tag)?;
    let result: SearchResult = get(&format!(
        "https://zh.wikipedia.org/w/api.php?action=query&list=search&format=json&srlimit=1&srsearch={}",
        search.search.join(" ")
    ))
    .await?;
    if result.query.searchinfo.totalhits == 0 {
        return Ok(format!(
            "❌未查找到词条 `{}`",
            markdown::escape_code(&search.search.join(" "))
        ));
    }
    let search = &result.query.search[0];
    Ok(format!(
        "🔍查找到词条
*链接*: https://zh\\.wikipedia\\.org/wiki/{}
        
*概要*: {}

*总词数*: {}",
        markdown::escape(&search.title),
        markdown::escape(&MATCH.replace_all(&search.snippet, "")),
        search.wordcount
    ))
}

pub async fn wiki(bot: &Bot, msg: &Message) -> BotResult {
    bot.send_message(msg.chat.id, &get_wiki(msg).await?)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_parameters(ReplyParameters::new(msg.id))
        .await?;
    Ok(())
}
