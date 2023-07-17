use super::*;
use regex::Regex;

error_fmt!(USAGE);

lazy_static! {
    static ref USAGE: String = WikiCmd::command().render_help().to_string();
    static ref MATCH: Regex = Regex::new(r#"<span class="searchmatch">|</span>"#).unwrap();
}
#[derive(Parser)]
#[command(
    help_template = "使用方法：{usage}\n\n{all-args}\n\n{about}",
    about = "命令功能：在中文维基百科中搜索词条",
    name = "/wiki",
    next_help_heading = "参数解释",
    disable_help_flag = true
)]
struct WikiCmd {
    ///词条名
    #[arg(required = true)]
    search: Vec<String>,
}

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

async fn get_wiki(ctx: &Context) -> Result<String, AppError> {
    let search = WikiCmd::try_parse_from(
        ctx.effective_message
            .as_ref()
            .unwrap()
            .text
            .as_ref()
            .unwrap()
            .split_whitespace(),
    )?;
    let result: SearchResult = get(&format!("https://zh.wikipedia.org/w/api.php?action=query&list=search&format=json&srlimit=1&srsearch={}",search.search.join(" "))).await?;
    if result.query.searchinfo.totalhits == 0 {
        return Err(AppError::CustomError("❌未查找到该词条❌".to_string()));
    }
    let search = &result.query.search[0];
    Ok(format!(
        "*链接*: https://zh.wikipedia.org/wiki/{}
        
*概要*: {}

*总词数*: {}",
        search.title,
        MATCH.replace_all(&search.snippet, ""),
        search.wordcount
    ))
}

pub async fn wiki(bot: Bot, ctx: Context) -> FResult<GroupIteration> {
    let text = match get_wiki(&ctx).await {
        Ok(msg) => msg,
        Err(e) => format!("{e}"),
    };
    ctx.effective_message
        .unwrap()
        .reply(&bot, &text)
        .parse_mode("markdown".to_string())
        .send()
        .await?;
    Ok(GroupIteration::EndGroups)
}
