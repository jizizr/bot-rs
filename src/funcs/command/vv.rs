use rand::RngExt;
use std::sync::LazyLock;
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
    }
);

static CLIENT: LazyLock<ClientWithMiddleware> =
    LazyLock::new(|| retry_client(reqwest::Client::new(), 2));
static API_URL: LazyLock<String> =
    LazyLock::new(|| SETTINGS.vv.api_url.trim_end_matches('/').to_string());
static PIC_URL: LazyLock<String> =
    LazyLock::new(|| SETTINGS.vv.pic_url.trim_end_matches('/').to_string());

async fn get_vv_list(desc: String) -> Result<Vec<String>, BotError> {
    let url = format!("{}/search?q={}&n=5", *API_URL, desc);
    Ok(CLIENT.post(url).send().await?.json().await?)
}

fn get_vv_pic_url(name: &str) -> Result<Url, BotError> {
    Url::parse(&format!("{}/{}", *PIC_URL, name)).map_err(BotError::from)
}

async fn vv_cmd(cmd: &VvCmd) -> Result<Url, BotError> {
    let desc = cmd.desc.join(" ");
    let vv_list = get_vv_list(desc).await?;
    if vv_list.is_empty() {
        return Err(BotError::Custom("vv被削了".to_string()));
    }
    let vv = {
        let mut rng = rand::rng();
        let vv_index = rng.random_range(0..vv_list.len());
        &vv_list[vv_index]
    };
    get_vv_pic_url(vv)
}

pub async fn vv(bot: &Bot, msg: &Message) -> BotResult {
    let language_tag = Some("zh-CN");
    let pic_url = vv_cmd(&VvCmd::parse_i18n_from_bot(
        getor(msg).unwrap().split_whitespace(),
        language_tag,
    )?)
    .await?;
    bot.send(
        SendPhoto::new(msg.chat().id(), InputFile::url(pic_url))
            .reply_parameters(reply_to(msg.message_id())),
    )
    .await?;
    Ok(())
}
