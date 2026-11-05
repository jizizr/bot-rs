//! Every lyric page, whatever message it lands in. `LyricView` gathers what a
//! page shows once — the song, its timed lines with translations, the
//! credits, platform and script — and renders it either as a rich message
//! (chat pages, inline results, inline edits) or as a caption (an LRC file,
//! an inline audio message), so the two look alike and change together.
//! Covers go through `lyric_cover`, which turns them into a short banner and
//! uploads it once, so every page — inline ones too, which may not link
//! outside images — shows the same cover at the same size.

use super::{
    MusicLyricScript, MusicPlatform, MusicQuery, MusicTrack, convert_lyric_script,
    get_user_settings_or_default, html_escape, lock_unpoisoned, provider, resolve_track,
};
use crate::{Bot, BotError, InputFile, SendPhoto, funcs::pkg::rich::RichPage, settings::SETTINGS};
use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::Duration,
};
use telers::types::InputRichMessage;

/// Telegram's limit for a caption.
const CAPTION_LIMIT: usize = 1024;
const COVER_UPLOAD_TIMEOUT: Duration = Duration::from_secs(8);
const COVER_MAX_BYTES: u64 = 8 * 1024 * 1024;
const BANNER_WIDTH: u32 = 1200;
const BANNER_HEIGHT: u32 = 400;
const BANNER_COVER_SIZE: u32 = 320;
const COVER_CACHE_CAPACITY: usize = 1024;

/// Cover URL → file id of the photo uploaded for it.
static COVER_PHOTOS: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);

/// Telegram's limit for a rich message, with room for the closing tags.
const RICH_MESSAGE_LIMIT: usize = 32_000;

/// One sung line, when it is sung, and the translation shown under it.
#[derive(Debug, Eq, PartialEq)]
pub(super) struct LyricLine {
    pub(super) time: Option<u64>,
    pub(super) text: String,
    pub(super) translation: Option<String>,
}

/// The leading `[mm:ss.xx]` tags of an LRC line as milliseconds, and its text.
/// Metadata tags such as `[ar:...]` yield no times.
pub(super) fn lrc_times(line: &str) -> (Vec<u64>, &str) {
    let mut times = Vec::new();
    let mut rest = line.trim_start();
    while let Some(tag) = rest.strip_prefix('[') {
        let Some((tag, after)) = tag.split_once(']') else {
            break;
        };
        let parsed = tag.split_once(':').and_then(|(minutes, seconds)| {
            let minutes = minutes.trim().parse::<u64>().ok()?;
            let seconds = seconds.trim().replace(':', ".").parse::<f64>().ok()?;
            Some(minutes * 60_000 + (seconds * 1000.0).round() as u64)
        });
        match parsed {
            Some(time) => times.push(time),
            None => return (Vec::new(), ""),
        }
        rest = after.trim_start();
    }
    (times, rest.trim())
}

/// Sung lines in time order, each with the translation sharing its time.
/// Untimed lyrics are kept in their own order without translations.
pub(super) fn timed_lyric_lines(plain: &str, translation: &str) -> Vec<LyricLine> {
    let translations = translation
        .lines()
        .flat_map(|line| {
            let (times, text) = lrc_times(line);
            times.into_iter().map(move |time| (time, text))
        })
        .filter(|(_, text)| !text.is_empty())
        .collect::<std::collections::HashMap<_, _>>();
    let mut timed = Vec::new();
    let mut untimed = Vec::new();
    for line in plain.lines() {
        let (times, text) = lrc_times(line);
        if text.is_empty() {
            continue;
        }
        if times.is_empty() {
            if !line.trim_start().starts_with('[') {
                untimed.push(LyricLine {
                    time: None,
                    text: text.to_string(),
                    translation: None,
                });
            }
            continue;
        }
        for time in times {
            timed.push((time, text));
        }
    }
    if timed.is_empty() {
        return untimed;
    }
    timed.sort_by_key(|(time, _)| *time);
    timed
        .into_iter()
        .map(|(time, text)| LyricLine {
            time: Some(time),
            text: text.to_string(),
            translation: translations
                .get(&time)
                .filter(|translation| **translation != text)
                .map(|translation| translation.to_string()),
        })
        .collect()
}

/// What a lyric page shows about its song; any of it may be unknown.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct LyricSong {
    pub(super) title: Option<String>,
    pub(super) singer: Option<String>,
    pub(super) album: Option<String>,
    pub(super) cover: Option<String>,
}

impl LyricSong {
    /// `singer · platform · album`, leaving out what is unknown.
    pub(super) fn byline(&self, platform: MusicPlatform) -> String {
        [
            self.singer.as_deref(),
            Some(platform.label()),
            self.album.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
    }

    pub(super) fn from_track(track: &MusicTrack) -> Self {
        let known = |value: &str| Some(value.trim().to_string()).filter(|value| !value.is_empty());
        Self {
            title: known(&track.song),
            singer: known(&track.singer),
            album: known(&track.album),
            cover: known(&track.cover).map(|cover| sized_cover(track.platform, &cover)),
        }
    }

    pub(super) fn from_search_item(item: &super::MusicSearchItem) -> Self {
        let known = |value: &str| Some(value.trim().to_string()).filter(|value| !value.is_empty());
        Self {
            title: known(&item.song),
            singer: known(&item.singer),
            album: None,
            cover: known(&item.cover).map(|cover| sized_cover(item.platform, &cover)),
        }
    }

    pub(super) fn from_audio(audio: Option<&telers::types::Audio>) -> Self {
        let known = |value: Option<&str>| {
            value
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        Self {
            title: known(audio.and_then(|audio| audio.title.as_deref())),
            singer: known(audio.and_then(|audio| audio.performer.as_deref())),
            ..Self::default()
        }
    }

    /// The song as MHub knows it, filling in whatever `fallback` says.
    pub(super) async fn resolve(
        platform: MusicPlatform,
        id: &str,
        user_id: i64,
        fallback: LyricSong,
    ) -> LyricSong {
        let settings = get_user_settings_or_default(user_id).await;
        let query = MusicQuery {
            platform,
            keyword: id.to_string(),
        };
        match resolve_track(&query, Some(id), &settings).await {
            Ok(track) => {
                let song = Self::from_track(&track);
                Self {
                    title: song.title.or(fallback.title),
                    singer: song.singer.or(fallback.singer),
                    album: song.album.or(fallback.album),
                    cover: song.cover.or(fallback.cover),
                }
            }
            Err(err) => {
                log::warn!("lyric page could not resolve {}:{id}: {err}", platform.id());
                fallback
            }
        }
    }
}

/// Covers at a size that reads well beside lyrics without a heavy download.
/// URLs whose size cannot be chosen are kept as they are.
pub(super) fn sized_cover(platform: MusicPlatform, url: &str) -> String {
    static APPLE_SIZE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"/\d+x\d+(bb|cc)?\.(jpg|jpeg|png|webp)$").unwrap());
    static QQ_SIZE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"R\d+x\d+").unwrap());
    match platform {
        MusicPlatform::AppleMusic => APPLE_SIZE.replace(url, "/600x600bb.jpg").into_owned(),
        MusicPlatform::Tencent => QQ_SIZE.replace(url, "R500x500").into_owned(),
        MusicPlatform::Netease if !url.contains('?') => format!("{url}?param=600y600"),
        _ => url.to_string(),
    }
}

/// The cover a lyric page shows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Cover {
    /// A banner photo already on Telegram, usable in every kind of message.
    Uploaded(String),
    None,
}

/// The banner of `song`'s cover, made and uploaded once to the shared
/// storage chat, then reused by file id. Pages go without a cover when it
/// cannot be made, rather than fall back to a different look.
pub(super) async fn lyric_cover(bot: &Bot, song: &LyricSong) -> Cover {
    let Some(url) = song.cover.as_deref() else {
        return Cover::None;
    };
    match tokio::time::timeout(COVER_UPLOAD_TIMEOUT, uploaded_banner(bot, url)).await {
        Ok(Ok(file_id)) => Cover::Uploaded(file_id),
        Ok(Err(err)) => {
            log::error!("lyric cover banner failed: {err}");
            Cover::None
        }
        Err(_) => {
            log::error!("lyric cover banner timed out");
            Cover::None
        }
    }
}

async fn uploaded_banner(bot: &Bot, url: &str) -> Result<String, BotError> {
    if let Some(file_id) = lock_unpoisoned(&COVER_PHOTOS).get(url) {
        return Ok(file_id.clone());
    }
    // 与 MHub 的音频缓存同一个落点：没配存储群时传到 bot 主人的私聊
    let chat_id = match SETTINGS.music.inline_upload_chat_id {
        0 => SETTINGS.bot.owner,
        chat_id => chat_id,
    };
    if chat_id == 0 {
        return Err(BotError::Custom("no chat to keep covers in".to_string()));
    }
    let response = crate::HTTP_CLIENT
        .get(url)
        .send()
        .await?
        .error_for_status()?;
    if response.content_length().unwrap_or(0) > COVER_MAX_BYTES {
        return Err(BotError::Custom("cover too large".to_string()));
    }
    let bytes = response.bytes().await?;
    if bytes.len() as u64 > COVER_MAX_BYTES {
        return Err(BotError::Custom("cover too large".to_string()));
    }
    let banner = tokio::task::spawn_blocking(move || cover_banner(&bytes)).await??;
    let message = bot
        .send(
            SendPhoto::new(chat_id, InputFile::buffered_with_name(banner, "cover.jpg"))
                .disable_notification(true)
                .caption(url),
        )
        .await?;
    let file_id = message
        .photo()
        .and_then(|sizes| sizes.last())
        .map(|size| size.file_id.to_string())
        .ok_or_else(|| BotError::Custom("uploaded cover has no photo".to_string()))?;
    let mut photos = lock_unpoisoned(&COVER_PHOTOS);
    if photos.len() >= COVER_CACHE_CAPACITY {
        photos.clear();
    }
    photos.insert(url.to_string(), file_id.clone());
    Ok(file_id)
}

/// A wide, short banner from a square cover: the cover in the middle over a
/// blurred, darkened copy of itself. Telegram shows rich-message pictures at
/// full width with no size control, so a 3:1 picture keeps the cover small
/// instead of filling the screen.
pub(super) fn cover_banner(cover: &[u8]) -> Result<Vec<u8>, image::ImageError> {
    use image::{DynamicImage, imageops, imageops::FilterType};
    let cover = image::load_from_memory(cover)?;
    let background = cover
        .resize_to_fill(BANNER_WIDTH / 8, BANNER_HEIGHT / 8, FilterType::Triangle)
        .blur(4.0)
        .resize_exact(BANNER_WIDTH, BANNER_HEIGHT, FilterType::Triangle);
    let mut banner = background.to_rgb8();
    for pixel in banner.pixels_mut() {
        for channel in &mut pixel.0 {
            *channel = (f32::from(*channel) * 0.55) as u8;
        }
    }
    let front = cover
        .resize_to_fill(BANNER_COVER_SIZE, BANNER_COVER_SIZE, FilterType::Lanczos3)
        .to_rgb8();
    let offset = i64::from((BANNER_HEIGHT - BANNER_COVER_SIZE) / 2);
    imageops::overlay(
        &mut banner,
        &front,
        i64::from((BANNER_WIDTH - BANNER_COVER_SIZE) / 2),
        offset,
    );
    let mut jpeg = Vec::new();
    DynamicImage::ImageRgb8(banner).write_with_encoder(
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85),
    )?;
    Ok(jpeg)
}

/// A song's player for a lyric page: the cached Telegram file and its tags.
pub(super) struct SongAudio {
    file_id: String,
    title: String,
    performer: String,
    duration: Option<u32>,
    /// What the audio message's caption used to say about the file: size and
    /// bitrate, decryption time, platform and format tags.
    info: Vec<String>,
}

impl SongAudio {
    pub(super) fn from_media(media: &super::MusicMedia, info: Vec<String>) -> Self {
        Self {
            file_id: media.file_id.clone(),
            title: media.track.song.clone(),
            performer: media.track.singer.clone(),
            duration: media.track.duration,
            info,
        }
    }

    /// The song as an inline message shows it: name, byline, the player and
    /// the file's details, hashtags clickable.
    fn card(&self, page: RichPage, title: &str, byline: &str) -> RichPage {
        page.heading(title)
            .paragraph(byline)
            .uploaded_audio(&self.file_id, &self.title, &self.performer, self.duration)
            .quote(&self.info.join("\n"))
            .detect_entities()
    }
}

/// `page` with the cover banner appended, when there is one.
fn with_cover(page: RichPage, cover: &Cover) -> RichPage {
    match cover {
        Cover::Uploaded(file_id) => page.uploaded_photo(file_id),
        Cover::None => page,
    }
}

/// A song as a rich card: name, byline, player, details, then the cover.
/// Its lyrics open by editing it into the same card with the lyric page
/// under the cover, which an audio message cannot be.
pub(super) fn song_card(
    song: &LyricSong,
    platform: MusicPlatform,
    audio: &SongAudio,
    cover: &Cover,
) -> InputRichMessage {
    with_cover(
        audio.card(
            RichPage::default(),
            song.title.as_deref().unwrap_or("歌曲"),
            &song.byline(platform),
        ),
        cover,
    )
    .build()
}

/// What every lyric page shows, already in the requested script.
pub(super) struct LyricView {
    title: String,
    byline: String,
    lines: Vec<LyricLine>,
    credits: Vec<String>,
    footer: String,
}

impl LyricView {
    pub(super) fn new(
        song: &LyricSong,
        platform: MusicPlatform,
        lyrics: &provider::MusicLyrics,
        script: MusicLyricScript,
    ) -> Self {
        let convert = |text: &str| convert_lyric_script(text, script);
        let byline = song.byline(platform);
        let lines = timed_lyric_lines(&lyrics.plain, &lyrics.translation)
            .into_iter()
            .map(|line| LyricLine {
                time: line.time,
                text: convert(&line.text),
                translation: line.translation.as_deref().map(convert),
            })
            .collect();
        Self {
            title: song.title.clone().unwrap_or_else(|| "歌词".to_string()),
            byline,
            lines,
            credits: lyrics
                .credits
                .iter()
                .map(|credit| credit.trim())
                .filter(|credit| !credit.is_empty())
                .map(convert)
                .collect(),
            footer: format!("{} · {}", platform.label(), script.label()),
        }
    }

    /// The lines as markup, `break_tag` between them, stopping with "…"
    /// before going over `budget` bytes.
    fn lines_html(&self, break_tag: &str, budget: usize) -> String {
        let mut body = String::new();
        for line in &self.lines {
            // 给人读的页面不带时间轴，时间只留在 LRC 文件里
            let mut entry = html_escape(&line.text);
            if let Some(translation) = &line.translation {
                entry.push_str(&format!("{break_tag}<i>{}</i>", html_escape(translation)));
            }
            if body.len() + entry.len() + break_tag.len() * 2 + 3 > budget {
                body.push_str(break_tag);
                body.push('…');
                break;
            }
            if !body.is_empty() {
                body.push_str(break_tag);
            }
            body.push_str(&entry);
        }
        if body.is_empty() {
            body.push_str("暂无歌词信息");
        }
        body
    }

    /// The page as a rich message: cover, heading, byline, the lines in a
    /// quote that opens on demand, credits folded away, and the footer.
    pub(super) fn rich(&self, cover: &Cover) -> InputRichMessage {
        self.rich_page(cover, None)
    }

    /// An inline song's card with its lyrics opened: the card as it was, then
    /// the cover above the lyric page.
    pub(super) fn rich_with_audio(&self, cover: &Cover, audio: &SongAudio) -> InputRichMessage {
        self.rich_page(cover, Some(audio))
    }

    fn rich_page(&self, cover: &Cover, audio: Option<&SongAudio>) -> InputRichMessage {
        let page = match audio {
            Some(audio) => with_cover(
                audio.card(RichPage::default(), &self.title, &self.byline),
                cover,
            ),
            None => with_cover(RichPage::default(), cover)
                .heading(&self.title)
                .paragraph(&self.byline),
        };
        let credits = self
            .credits
            .iter()
            .fold(RichPage::default(), |page, credit| page.paragraph(credit));
        let tail = RichPage::default()
            .details("制作人员", credits)
            .footer(&self.footer);
        let budget = RICH_MESSAGE_LIMIT.saturating_sub(page.html().len() + tail.html().len() + 64);
        page.block_html(&format!(
            "<blockquote expandable>{}</blockquote>",
            self.lines_html("<br>", budget)
        ))
        .block_html(tail.html())
        .build()
    }

    /// The same page as an HTML caption, within Telegram's caption limit:
    /// heading and byline, the lines in an expandable quote, credits on one
    /// line when they fit, and the footer.
    pub(super) fn caption(&self) -> String {
        let head = format!(
            "<b>{}</b>\n{}\n",
            html_escape(&self.title),
            html_escape(&self.byline)
        );
        let credits = if self.credits.is_empty() {
            String::new()
        } else {
            format!("\n{}", html_escape(&self.credits.join(" · ")))
        };
        let footer = format!("\n{}", html_escape(&self.footer));
        let quote = |budget| {
            format!(
                "<blockquote expandable>{}</blockquote>",
                self.lines_html("\n", budget)
            )
        };
        // Telegram counts characters after parsing; bytes overestimate that,
        // so measure the visible text and shrink until it fits.
        let visible = |html: &str| {
            let mut count = 0;
            let mut in_tag = false;
            for c in html.chars() {
                match c {
                    '<' => in_tag = true,
                    '>' => in_tag = false,
                    _ if !in_tag => count += 1,
                    _ => {}
                }
            }
            count
        };
        for credits in [credits.as_str(), ""] {
            let mut budget = CAPTION_LIMIT * 4;
            while budget > 64 {
                let caption = format!("{head}{}{credits}{footer}", quote(budget));
                if visible(&caption) <= CAPTION_LIMIT {
                    return caption;
                }
                budget = budget * 9 / 10;
            }
        }
        format!("{head}{footer}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lrc_lines_pair_translations_by_time() {
        let plain =
            "[ti:晴天]\n[00:01.50]故事的小黄花\n[00:05.00][00:30.00]从出生那年就飘着\n[00:08.1]\n";
        let translation = "[00:01.500]The little yellow flower\n[00:30.00]Drifting since birth\n";
        assert_eq!(lrc_times("[01:02.34]词"), (vec![62_340], "词"));
        assert_eq!(lrc_times("[ar:周杰伦]"), (Vec::new(), ""));
        let lines = timed_lyric_lines(plain, translation);
        assert_eq!(
            lines,
            [
                LyricLine {
                    time: Some(1_500),
                    text: "故事的小黄花".to_string(),
                    translation: Some("The little yellow flower".to_string()),
                },
                LyricLine {
                    time: Some(5_000),
                    text: "从出生那年就飘着".to_string(),
                    translation: None,
                },
                LyricLine {
                    time: Some(30_000),
                    text: "从出生那年就飘着".to_string(),
                    translation: Some("Drifting since birth".to_string()),
                },
            ]
        );
        let untimed = timed_lyric_lines("第一行\n\n第二行", "");
        assert_eq!(untimed.len(), 2);
    }

    #[test]
    fn covers_are_sized_per_platform() {
        assert_eq!(
            sized_cover(
                MusicPlatform::AppleMusic,
                "https://is1.mzstatic.com/image/thumb/a/b.jpg/1000x1000bb.jpg"
            ),
            "https://is1.mzstatic.com/image/thumb/a/b.jpg/600x600bb.jpg"
        );
        assert_eq!(
            sized_cover(
                MusicPlatform::Tencent,
                "https://y.gtimg.cn/music/photo_new/T002R800x800M000x.jpg"
            ),
            "https://y.gtimg.cn/music/photo_new/T002R500x500M000x.jpg"
        );
        assert_eq!(
            sized_cover(MusicPlatform::Netease, "https://p1.music.126.net/x.jpg"),
            "https://p1.music.126.net/x.jpg?param=600y600"
        );
        assert_eq!(
            sized_cover(MusicPlatform::Netease, "https://p1.music.126.net/x.jpg?a=1"),
            "https://p1.music.126.net/x.jpg?a=1"
        );
    }

    fn sample_audio() -> SongAudio {
        SongAudio {
            file_id: "CQAC".to_string(),
            title: "晴天".to_string(),
            performer: "周杰伦".to_string(),
            duration: Some(269),
            info: vec![
                "22.36MB 750.33kbps".to_string(),
                "#AppleMusic #m4a".to_string(),
            ],
        }
    }

    fn sample() -> (LyricSong, provider::MusicLyrics) {
        let song = LyricSong {
            title: Some("晴天".to_string()),
            singer: Some("周杰伦 & <Band>".to_string()),
            album: Some("叶惠美".to_string()),
            cover: Some("https://example.com/c.jpg".to_string()),
        };
        let lyrics = provider::MusicLyrics {
            plain: "[00:01.00]故事的小黄花".to_string(),
            translation: "[00:01.00]Little flower".to_string(),
            roman: String::new(),
            credits: vec!["作词：周杰伦".to_string()],
        };
        (song, lyrics)
    }

    #[test]
    fn rich_page_and_caption_show_the_same_page() {
        let (song, lyrics) = sample();
        let view = LyricView::new(
            &song,
            MusicPlatform::Tencent,
            &lyrics,
            MusicLyricScript::Traditional,
        );
        let rich = view.rich(&Cover::Uploaded("AgAD".to_string()));
        assert_eq!(
            rich.html.as_deref(),
            Some(
                "<img src=\"tg://photo?id=media0\"/><h3>晴天</h3>\
                 <p>周杰伦 &amp; &lt;Band&gt; · QQ音乐 · 叶惠美</p>\
                 <blockquote expandable>故事的小黃花<br><i>Little flower</i></blockquote>\
                 <details><summary>制作人员</summary><p>作詞：周杰倫</p></details>\
                 <footer>QQ音乐 · 繁体</footer>"
            )
        );
        assert_eq!(
            view.caption(),
            "<b>晴天</b>\n周杰伦 &amp; &lt;Band&gt; · QQ音乐 · 叶惠美\n\
             <blockquote expandable>故事的小黃花\n<i>Little flower</i></blockquote>\n\
             作詞：周杰倫\nQQ音乐 · 繁体"
        );
    }

    #[test]
    fn uploaded_covers_are_referenced_by_file_id() {
        let (song, lyrics) = sample();
        let view = LyricView::new(
            &song,
            MusicPlatform::Tencent,
            &lyrics,
            MusicLyricScript::Simplified,
        );
        let rich = view.rich(&Cover::Uploaded("AgAD".to_string()));
        assert!(
            rich.html
                .as_deref()
                .unwrap()
                .starts_with("<img src=\"tg://photo?id=media0\"/>")
        );
        assert_eq!(rich.media.as_ref().map(|media| media.len()), Some(1));
        let plain = view.rich(&Cover::None);
        assert!(plain.html.as_deref().unwrap().starts_with("<h3>晴天</h3>"));
        assert!(plain.media.is_none());
    }

    #[test]
    fn inline_pages_carry_the_song_player() {
        let (song, lyrics) = sample();
        let view = LyricView::new(
            &song,
            MusicPlatform::Tencent,
            &lyrics,
            MusicLyricScript::Simplified,
        );
        let rich = view.rich_with_audio(&Cover::Uploaded("AgAD".to_string()), &sample_audio());
        let html = rich.html.as_deref().unwrap();
        assert!(html.starts_with(
            "<h3>晴天</h3><p>周杰伦 &amp; &lt;Band&gt; · QQ音乐 · 叶惠美</p>\
             <audio src=\"tg://audio?id=media0\"></audio>\
             <blockquote>22.36MB 750.33kbps<br>#AppleMusic #m4a</blockquote>\
             <img src=\"tg://photo?id=media1\"/><blockquote expandable>"
        ));
        assert_eq!(rich.media.as_ref().map(|media| media.len()), Some(2));
    }

    #[test]
    fn song_cards_show_the_song_its_player_and_its_cover() {
        let (song, _) = sample();
        let head = "<h3>晴天</h3><p>周杰伦 &amp; &lt;Band&gt; · QQ音乐 · 叶惠美</p>\
                    <audio src=\"tg://audio?id=media0\"></audio>\
                    <blockquote>22.36MB 750.33kbps<br>#AppleMusic #m4a</blockquote>";
        let card = song_card(
            &song,
            MusicPlatform::Tencent,
            &sample_audio(),
            &Cover::Uploaded("AgAD".to_string()),
        );
        assert_eq!(
            card.html.as_deref(),
            Some(format!("{head}<img src=\"tg://photo?id=media1\"/>").as_str())
        );
        assert_eq!(card.media.as_ref().map(|media| media.len()), Some(2));
        assert_eq!(card.skip_entity_detection, Some(false));
        let bare = song_card(&song, MusicPlatform::Tencent, &sample_audio(), &Cover::None);
        assert_eq!(bare.html.as_deref(), Some(head));
    }

    #[test]
    fn banners_are_wide_and_short() {
        let mut cover = image::RgbImage::new(600, 600);
        for (x, _, pixel) in cover.enumerate_pixels_mut() {
            *pixel = image::Rgb([(x % 256) as u8, 80, 160]);
        }
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(cover)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let banner = image::load_from_memory(&cover_banner(&png).unwrap()).unwrap();
        assert_eq!(
            (banner.width(), banner.height()),
            (BANNER_WIDTH, BANNER_HEIGHT)
        );
        assert!(cover_banner(b"not an image").is_err());
    }

    #[test]
    fn long_lyrics_fit_a_caption() {
        let (song, mut lyrics) = sample();
        lyrics.plain = (0..400)
            .map(|i| format!("[{:02}:{:02}.00]很长很长的一句歌词{i}", i / 60, i % 60))
            .collect::<Vec<_>>()
            .join("\n");
        let caption = LyricView::new(
            &song,
            MusicPlatform::Tencent,
            &lyrics,
            MusicLyricScript::Simplified,
        )
        .caption();
        let visible = caption
            .split(['<', '>'])
            .step_by(2)
            .map(|part| part.chars().count())
            .sum::<usize>();
        assert!(visible <= CAPTION_LIMIT, "{visible}");
        assert!(caption.contains("…</blockquote>"));
        assert!(caption.ends_with("QQ音乐 · 简体"));
    }
}
