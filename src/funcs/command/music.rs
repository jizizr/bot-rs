use super::*;
use crate::settings::SETTINGS;
use reqwest::Client;
use teloxide::{payloads::EditMessageReplyMarkupSetters, types::MaybeInaccessibleMessage};
use url::Url;
lazy_static! {
    static ref CLIENT: ClientWithMiddleware = retry_client(Client::new(), 2);
}

cmd!(
    "/music",
    "获取音乐",
    MusicCmd ,
    {
        ///音乐名
        #[arg(required = true)]
        url: Vec<String>,
    }
);

#[derive(Deserialize)]
struct Music {
    data: MusicData,
}

#[derive(Deserialize)]
struct MusicData {
    song: String,
    singer: String,
    cover: String,
    link: String,
    url: String,
}

#[derive(Deserialize)]
struct MusicListData {
    id: i32,
    song: String,
    singer: String,
}

#[derive(Deserialize)]
struct MusicList {
    data: Vec<MusicListData>,
}

async fn get_music_data(name: &str, num: &str) -> Result<MusicData, BotError> {
    let url = if num == "1" {
        format!("{}={}&choose=1", SETTINGS.api.music, name)
    } else {
        format!("{}={}&id={}", SETTINGS.api.music, name, num)
    };
    let music_data: Music = get(&url).await?;
    Ok(music_data.data)
}

#[allow(dead_code)]
async fn music2vec(url: &str) -> Result<Vec<u8>, BotError> {
    let mut resp = CLIENT.get(url).send().await?;
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

async fn get_music(
    bot: &Bot,
    music: MusicCmd,
    msg: &Message,
    msg_bot: &Message,
) -> Result<(), BotError> {
    let name = music.url.join(" ");
    let music = get_music_data(&name, "1").await?;
    let (audio, cover) = get_music_info(&music).await?;
    let err = tokio::join!(
        bot.send_audio(
            msg.chat.id,
            InputFile::memory(audio).file_name(music.song.clone()),
        )
        .thumbnail(InputFile::memory(cover))
        .reply_parameters(ReplyParameters::new(msg.id))
        .reply_markup(link2gui_menu(music.cover, name))
        .caption(format!(
            "演唱者:「{}」\n歌曲链接：{}",
            music.singer, music.link
        ))
        .send(),
        bot.edit_message_text(msg_bot.chat.id, msg_bot.id, "获取成功🎉，正在上传，稍等...")
            .send()
    );
    err.0?;
    err.1?;
    Ok(())
}

async fn get_music_gui(bot: Bot, msg: Message, search: &str) -> Result<(), BotError> {
    let music_datas: MusicList = get(&format!("{}={}", SETTINGS.api.music, search)).await?;
    bot.edit_message_caption(msg.chat.id, msg.id)
        .caption("选择你的音乐")
        .reply_markup(gui_menu(music_datas.data, search))
        .await?;
    Ok(())
}

async fn get_music_cover(bot: Bot, msg: Message, search: &str) -> Result<(), BotError> {
    bot.send_photo(
        msg.chat.id,
        InputFile::url(
            Url::parse(&format!("https://y.qq.com/music/photo_new/{}", search)).unwrap(),
        ),
    )
    .reply_parameters(ReplyParameters::new(msg.id))
    .send()
    .await?;
    bot.edit_message_reply_markup(msg.chat.id, msg.id)
        .reply_markup(InlineKeyboardMarkup::new([[
            InlineKeyboardButton::callback(
                "搜索更多🔍",
                match &msg.reply_markup().unwrap().inline_keyboard[0][1].kind {
                    CallbackData(data) => data,
                    _ => {
                        return Err(BotError::Custom("Unknown Error".to_string()));
                    }
                },
            ),
        ]]))
        .send()
        .await?;
    Ok(())
}

async fn get_callback_music(bot: Bot, msg: Message, id: &str, name: &str) -> Result<(), BotError> {
    let music_data: MusicData = get_music_data(name, id).await?;
    let (audio, cover) = get_music_info(&music_data).await?;
    bot.edit_message_media(
        msg.chat.id,
        msg.id,
        teloxide::types::InputMedia::Audio(
            InputMediaAudio::caption(
                InputMediaAudio::new(InputFile::memory(audio).file_name(music_data.song)),
                format!(
                    "演唱者:「{}」\n歌曲链接：{}",
                    music_data.singer, music_data.link,
                ),
            )
            .thumbnail(InputFile::memory(cover)),
        ),
    )
    .reply_markup(link2gui_menu(music_data.cover, name.to_string()))
    .send()
    .await?;
    Ok(())
}

pub async fn music_callback(bot: Bot, q: CallbackQuery) -> Result<(), BotError> {
    if let Some(music) = q.data {
        bot.answer_callback_query(q.id).await?;
        let mut music = music.splitn(2, ' ');
        let msg = match q.message {
            None => return Ok(()),
            Some(mbi_msg) => match mbi_msg {
                MaybeInaccessibleMessage::Inaccessible(_) => return Ok(()),
                MaybeInaccessibleMessage::Regular(msg) => msg,
            },
        };
        let _guard = lock!((msg.chat.id, msg.id));
        match music.next() {
            Some("gui") => get_music_gui(bot, msg, music.next().unwrap()).await?,
            Some("cover") => get_music_cover(bot, msg, music.next().unwrap()).await?,
            Some(music_name) => {
                get_callback_music(bot, msg, music_name, music.next().unwrap()).await?
            }
            None => {
                return Err(BotError::Custom(
                    "Unknown Error in [Music music_callback]".to_string(),
                ));
            }
        }
    }
    Ok(())
}

fn link2gui_menu(url: String, songname: String) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new([[
        InlineKeyboardButton::callback(
            "获取封面⛰️",
            format!(
                "music cover {}",
                url.trim_start_matches("https://y.qq.com/music/photo_new/")
            ),
        ),
        InlineKeyboardButton::callback("搜索更多🔍", format!("music gui {songname}")),
    ]])
}

fn gui_menu(music_datas: Vec<MusicListData>, search: &str) -> InlineKeyboardMarkup {
    let mut keyboard: Vec<Vec<InlineKeyboardButton>> = vec![vec![]; 5];
    music_datas.iter().take(5).for_each(|music_data| {
        keyboard.push(vec![InlineKeyboardButton::callback(
            format!("{}|{}", music_data.song, music_data.singer),
            format!("music {} {}", music_data.id, search),
        )])
    });
    InlineKeyboardMarkup::new(keyboard)
}

async fn get_music_info(music: &MusicData) -> Result<(Vec<u8>, Vec<u8>), BotError> {
    let (audio, cover) = tokio::join!(music2vec(&music.url), music2vec(&music.cover));
    Ok((audio?, cover?))
}

pub async fn music(bot: &Bot, msg: &Message) -> BotResult {
    tokio::spawn(bot.send_chat_action(msg.chat.id, ChatAction::Typing).send());

    let music =
        MusicCmd::try_parse_from(getor(msg).unwrap().split_whitespace()).map_err(ccerr!())?;
    let msg_bot = bot
        .send_message(msg.chat.id, "正在获取音乐...")
        .reply_parameters(ReplyParameters::new(msg.id))
        .await?;

    match get_music(bot, music, msg, &msg_bot).await {
        Ok(()) => {
            bot.delete_message(msg_bot.chat.id, msg_bot.id).await?;
        }
        Err(e) => {
            bot.edit_message_text(msg_bot.chat.id, msg_bot.id, format!("{e}"))
                .await?;
            return Err(e);
        }
    }
    Ok(())
}
