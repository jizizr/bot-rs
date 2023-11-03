use super::*;
use reqwest::{Client, Url};
use teloxide::{
    payloads::EditMessageReplyMarkupSetters,
    types::{
        InlineKeyboardButton, InlineKeyboardButtonKind::CallbackData, InlineKeyboardMarkup,
        InputFile, InputMediaAudio,
    },
};
lazy_static! {
    static ref USAGE: String = MusicCmd::command().render_help().to_string();
    static ref CLIENT: Client = Client::new();
}

#[derive(Parser)]
#[command(
    help_template = "使用方法：{usage}\n\n{all-args}\n\n{about}",
    about = "命令功能：获取音乐",
    name = "/music",
    next_help_heading = "参数解释",
    disable_help_flag = true
)]
struct MusicCmd {
    ///音乐名
    url: Vec<String>,
}

error_fmt!(USAGE);

#[derive(Deserialize)]
struct Music {
    data: MusicData,
}

#[derive(Deserialize)]
struct MusicData {
    songname: String,
    name: String,
    cover: String,
    songurl: String,
    src: String,
}

#[derive(Deserialize)]
struct MusicListData {
    id: i32,
    songname: String,
    name: String,
}

#[derive(Deserialize)]
struct MusicList {
    data: Vec<MusicListData>,
}

async fn get_music_data(name: &str, num: &str) -> Result<MusicData, AppError> {
    let url = format!(
        "http://ovoa.cc/api/QQmusic.php?msg={}&n={}&type=",
        name, num
    );
    let music_data: Music = get(&url).await?;
    Ok(music_data.data)
}

async fn music2vec(url: String) -> Result<Vec<u8>, AppError> {
    let mut resp = CLIENT.get(url).send().await?;
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

async fn get_music(msg: &Message) -> Result<(MusicData, String), AppError> {
    let music = MusicCmd::try_parse_from(getor(&msg).unwrap().split_whitespace())?;
    let name = music.url.join(" ");
    Ok((get_music_data(&name, "1").await?, name))
}

async fn get_music_gui(bot: Bot, msg: Message, search: &str) -> Result<(), AppError> {
    let music_datas: MusicList = get(&format!(
        "http://ovoa.cc/api/QQmusic.php?msg={}&n=&type=",
        search
    ))
    .await?;

    bot.edit_message_caption(msg.chat.id, msg.id)
        .caption("选择你的音乐")
        .reply_markup(gui_menu(music_datas.data, search))
        .await?;
    Ok(())
}

async fn get_music_cover(bot: Bot, msg: Message, search: &str) {
    let _ = tokio::try_join!(
        bot.send_photo(
            msg.chat.id,
            InputFile::url(
                Url::parse(&format!("https://y.qq.com/music/photo_new/{}", search)).unwrap(),
            ),
        )
        .reply_to_message_id(msg.id)
        .send(),
        bot.edit_message_reply_markup(msg.chat.id, msg.id)
            .reply_markup(InlineKeyboardMarkup::new([[
                InlineKeyboardButton::callback(
                    "搜索更多🔍",
                    if let CallbackData(data) =
                        &msg.reply_markup().unwrap().inline_keyboard[0][1].kind
                    {
                        data
                    } else {
                        return;
                    },
                ),
            ]]))
            .send()
    );
}

async fn get_callback_music(bot: Bot, msg: Message, id: &str, name: &str) -> Result<(), AppError> {
    let music_data: MusicData = get_music_data(name, id).await?;
    bot.edit_message_media(
        msg.chat.id,
        msg.id,
        teloxide::types::InputMedia::Audio(InputMediaAudio::caption(
            InputMediaAudio::new(
                InputFile::memory(music2vec(music_data.src.to_string()).await?)
                    .file_name(music_data.songname),
            ),
            format!(
                "演唱者:「{}」\n歌曲链接：{}",
                music_data.name, music_data.songurl,
            ),
        )),
    )
    .reply_markup(link2gui_menu(music_data.cover, name.to_string()))
    .send()
    .await?;
    Ok(())
}

pub async fn music_callback(bot: Bot, q: CallbackQuery) -> BotResult {
    if let Some(music) = q.data {
        bot.answer_callback_query(q.id).await?;
        let mut music = music.splitn(2, " ");
        if let Some(msg) = q.message {
            let guard = Guard::new(&LIMITER_Q, (msg.chat.id, msg.id));
            if guard.is_running {
                return Ok(());
            }
            if let Some(music_name) = music.next() {
                if music_name == "gui" {
                    get_music_gui(bot, msg, music.next().unwrap()).await?;
                } else if music_name == "cover" {
                    get_music_cover(bot, msg, music.next().unwrap()).await;
                } else {
                    get_callback_music(bot, msg, music_name, music.next().unwrap()).await?;
                }
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
                    format!("{}|{}", music_data.songname, music_data.name),
                    format!("music {} {}", music_data.id, search),
                )
            })
            .collect();
        keyboard.push(row)
    });
    InlineKeyboardMarkup::new(keyboard)
}

pub async fn music(bot: Bot, msg: Message) -> BotResult {
    let (music, name) = get_music(&msg).await?;
    bot.send_audio(
        msg.chat.id,
        InputFile::memory(music2vec(music.src).await?).file_name(music.songname.clone()),
    )
    .reply_to_message_id(msg.id)
    .reply_markup(link2gui_menu(music.cover, name))
    .caption(format!(
        "演唱者:「{}」\n歌曲链接：{}",
        music.name, music.songurl
    ))
    .send()
    .await?;
    Ok(())
}
