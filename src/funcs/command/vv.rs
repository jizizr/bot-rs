use rand::Rng;
use url::Url;

use super::*;

cmd!(
    "/vv",
    "vv不削能玩?",
    VvCmd,
    {
        ///描述vv
        #[arg(required = true)]
        desc: Vec<String>,
    },
    UrlParseError(url::ParseError),
);

lazy_static! {
    static ref CLIENT: ClientWithMiddleware = retry_client(reqwest::Client::new(), 2);
    static ref API_URL: String = SETTINGS.vv.api_url.trim_end_matches('/').to_string();
    static ref PIC_URL: String = SETTINGS.vv.pic_url.trim_end_matches('/').to_string();
}

async fn get_vv_list(desc: String) -> Result<Vec<String>, AppError> {
    let url = format!("{}/search?q={}&n=5", *API_URL, desc);
    Ok(CLIENT.post(url).send().await?.json().await?)
}

fn get_vv_pic_url(name: &str) -> Result<Url, AppError> {
    Ok(Url::parse(&format!("{}/{}", *PIC_URL, name))?)
}

async fn vv_cmd(cmd: &VvCmd) -> Result<Url, AppError> {
    let desc = cmd.desc.join(" ");
    let vv_list = get_vv_list(desc).await?;
    if vv_list.is_empty() {
        return Err(AppError::Custom("vv被削了".to_string()));
    }
    let vv = {
        let mut rng = rand::thread_rng();
        let vv_index = rng.gen_range(0..vv_list.len());
        &vv_list[vv_index]
    };
    Ok(get_vv_pic_url(vv)?)
}

pub async fn vv(bot: Bot, msg: Message) -> BotResult {
    let cmd = match VvCmd::try_parse_from(getor(&msg).unwrap().split_whitespace()) {
        Ok(cmd) => cmd,
        Err(e) => {
            bot.send_message(msg.chat.id, format!("{}", AppError::from(e)))
                .reply_parameters(ReplyParameters::new(msg.id))
                .await?;
            return Ok(());
        }
    };

    let pic_url = vv_cmd(&cmd).await?;
    bot.send_photo(msg.chat.id, InputFile::url(pic_url))
        .reply_parameters(ReplyParameters::new(msg.id))
        .await?;
    Ok(())
}
