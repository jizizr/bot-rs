use super::*;
use reqwest::{Client, Url};
use teloxide::{payloads::EditMessageReplyMarkupSetters, RequestError};
use tokio::task::JoinHandle;

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

async fn get_music_data(name: &str, num: &str) -> Result<MusicData, AppError> {
    let url = if num == "1" {
        format!("https://api.vkeys.cn/API/QQ_Music?word={}&n=1&q=7", name)
    } else {
        format!(
            "https://api.vkeys.cn/API/QQ_Music?word={}&id={}&q=7",
            name, num
        )
    };
    let music_data: Music = get(&url).await?;
    Ok(music_data.data)
}

async fn music2vec(url: &str) -> Result<Vec<u8>, AppError> {
    let mut resp = CLIENT.get(url).send().await?;
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

async fn get_music(
    bot: Arc<Bot>,
    music: MusicCmd,
    msg: &Message,
    jhandle: JoinHandle<Result<Message, RequestError>>,
) -> Result<Message, AppError> {
    let name = music.url.join(" ");
    let music = get_music_data(&name, "1").await?;
    let (audio, cover) = get_music_info(&music).await?;
    let bot_clone = bot.clone();
    let err = tokio::join!(
        bot.send_audio(
            msg.chat.id,
            InputFile::memory(audio).file_name(music.song.clone()),
        )
        .thumb(InputFile::memory(cover))
        .reply_to_message_id(msg.id)
        .reply_markup(link2gui_menu(music.cover, name))
        .caption(format!(
            "演唱者:「{}」\n歌曲链接：{}",
            music.singer, music.link
        ))
        .send(),
        handle_first_msg(bot_clone, jhandle)
    );
    err.0?;
    err.1
}

async fn get_music_gui(bot: Bot, msg: Message, search: &str) -> Result<(), AppError> {
    let music_datas: MusicList = get(&format!(
        "https://api.vkeys.cn/API/QQ_Music?word={}",
        search
    ))
    .await?;

    bot.edit_message_caption(msg.chat.id, msg.id)
        .caption("选择你的音乐")
        .reply_markup(gui_menu(music_datas.data, search))
        .await?;
    Ok(())
}

async fn get_music_cover(bot: Bot, msg: Message, search: &str) -> Result<(), AppError> {
    bot.send_photo(
        msg.chat.id,
        InputFile::url(
            Url::parse(&format!("https://y.qq.com/music/photo_new/{}", search)).unwrap(),
        ),
    )
    .reply_to_message_id(msg.id)
    .send()
    .await?;
    bot.edit_message_reply_markup(msg.chat.id, msg.id)
        .reply_markup(InlineKeyboardMarkup::new([[
            InlineKeyboardButton::callback(
                "搜索更多🔍",
                match &msg.reply_markup().unwrap().inline_keyboard[0][1].kind {
                    CallbackData(data) => data,
                    _ => return Err(AppError::CustomError("Unknown Error".to_string())),
                },
            ),
        ]]))
        .send()
        .await?;
    Ok(())
}

async fn get_callback_music(bot: Bot, msg: Message, id: &str, name: &str) -> Result<(), AppError> {
    let music_data: MusicData = get_music_data(name, id).await?;
    bot.edit_message_media(
        msg.chat.id,
        msg.id,
        teloxide::types::InputMedia::Audio(InputMediaAudio::caption(
            InputMediaAudio::new(
                InputFile::memory(music2vec(&music_data.url).await?).file_name(music_data.song),
            ),
            format!(
                "演唱者:「{}」\n歌曲链接：{}",
                music_data.singer, music_data.link,
            ),
        )),
    )
    .reply_markup(link2gui_menu(music_data.cover, name.to_string()))
    .send()
    .await?;
    Ok(())
}

pub async fn music_callback(bot: Bot, q: CallbackQuery) -> Result<(), AppError> {
    if let Some(music) = q.data {
        bot.answer_callback_query(q.id).await?;
        let mut music = music.splitn(2, ' ');
        let msg = match q.message {
            None => return Ok(()),
            Some(msg) => msg,
        };
        let _guard = lock!((msg.chat.id, msg.id));
        match music.next() {
            Some("gui") => get_music_gui(bot, msg, music.next().unwrap()).await?,
            Some("cover") => get_music_cover(bot, msg, music.next().unwrap()).await?,
            Some(music_name) => {
                get_callback_music(bot, msg, music_name, music.next().unwrap()).await?
            }
            None => {
                return Err(AppError::CustomError(
                    "Unknown Error in [Music music_callback]".to_string(),
                ))
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
    let mut keyboard: Vec<Vec<InlineKeyboardButton>> = vec![vec![]; 10];
    music_datas.chunks(2).for_each(|data| {
        let row = data
            .iter()
            .map(|music_data| {
                InlineKeyboardButton::callback(
                    format!("{}|{}", music_data.song, music_data.singer),
                    format!("music {} {}", music_data.id, search),
                )
            })
            .collect();
        keyboard.push(row)
    });
    InlineKeyboardMarkup::new(keyboard)
}

async fn get_music_info(music: &MusicData) -> Result<(Vec<u8>, Vec<u8>), AppError> {
    let (audio, cover) = tokio::join!(music2vec(&music.url), music2vec(&music.cover));
    Ok((audio?, cover?))
}

async fn handle_first_msg(
    bot: Arc<Bot>,
    jhandle: JoinHandle<Result<Message, RequestError>>,
) -> Result<Message, AppError> {
    if let Ok(result) = jhandle.await {
        let msg = result?;
        Ok(bot
            .edit_message_text(msg.chat.id, msg.id, "获取成功🎉！")
            .send()
            .await
            .unwrap_or(msg))
    } else {
        Err(AppError::CustomError("Join Task Error".to_string()))
    }
}

pub async fn music(bot: Bot, msg: Message) -> BotResult {
    let bot = Arc::new(bot);

    let bot_clone = bot.clone();
    tokio::spawn(async move {
        bot_clone
            .send_chat_action(msg.chat.id, ChatAction::Typing)
            .await
    });

    let music = match MusicCmd::try_parse_from(getor(&msg).unwrap().split_whitespace()) {
        Ok(music) => music,
        Err(e) => {
            bot.send_message(msg.chat.id, format!("{}", AppError::from(e)))
                .reply_to_message_id(msg.id)
                .send()
                .await?;
            return Ok(());
        }
    };

    let bot_clone = bot.clone();
    let jhandle = tokio::spawn(async move {
        bot_clone
            .send_message(msg.chat.id, "正在获取音乐...")
            .reply_to_message_id(msg.id)
            .send()
            .await
    });

    match get_music(bot.clone(), music, &msg, jhandle).await {
        Ok(msg) => {
            bot.clone()
                .delete_message(msg.chat.id, msg.id)
                .send()
                .await?;
        }
        Err(e) => {
            bot.edit_message_text(msg.chat.id, msg.id, format!("{e}"))
                .send()
                .await?;
        }
    }
    Ok(())
}
