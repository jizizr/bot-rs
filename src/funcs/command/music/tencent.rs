use super::provider::{MusicPlatform, MusicProvider, MusicSearchItem, MusicTrack};
use crate::BotError;
use async_trait::async_trait;
use base64::Engine;
use lazy_static::lazy_static;
use regex::Regex;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use sha1::{Digest, Sha1};
use std::collections::{HashMap, HashSet};
use url::Url;

const MUSICU_ENDPOINT: &str = "https://u.y.qq.com/cgi-bin/musicu.fcg";
const MUSICS_ENDPOINT: &str = "https://u.y.qq.com/cgi-bin/musics.fcg";
const SONG_DETAIL_ENDPOINT: &str = "https://c.y.qq.com/v8/fcg-bin/fcg_play_single_song.fcg";
const SEARCH_ENDPOINT: &str = "https://c.y.qq.com/soso/fcgi-bin/client_search_cp";

lazy_static! {
    static ref CLIENT: Client = Client::new();
    static ref QQ_SONG_PATH_PATTERNS: Vec<Regex> = vec![
        Regex::new(r"^n/ryqq/songDetail/([^/?#]+)$").unwrap(),
        Regex::new(r"^n/ryqq_v2/songDetail/([^/?#]+)$").unwrap(),
        Regex::new(r"^n/ryqq/song/([^/?#]+)$").unwrap(),
        Regex::new(r"^song/([^/?#]+)$").unwrap(),
    ];
}

pub static TENCENT_PROVIDER: TencentProvider = TencentProvider;

pub struct TencentProvider;

#[async_trait]
impl MusicProvider for TencentProvider {
    async fn search(&self, keyword: &str, limit: usize) -> Result<Vec<MusicSearchItem>, BotError> {
        let limit = limit.max(1);
        let songs = match search_by_meting_desktop(keyword, limit).await {
            Ok(songs) if !songs.is_empty() => songs,
            _ => match search_by_musicu(keyword, limit).await {
                Ok(songs) if !songs.is_empty() => songs,
                _ => search_by_legacy(keyword, limit).await?,
            },
        };
        Ok(songs
            .into_iter()
            .take(limit)
            .map(search_song_to_item)
            .filter(|item| !item.id.is_empty())
            .collect())
    }

    async fn resolve(
        &self,
        keyword: &str,
        selected_id: Option<&str>,
    ) -> Result<MusicTrack, BotError> {
        let id = match selected_id {
            Some(id) => id.to_string(),
            None => parse_qq_track_id(keyword).unwrap_or_default(),
        };
        let id = if id.is_empty() {
            self.search(keyword, 1)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| BotError::Custom("没有找到可下载的音乐".to_string()))?
                .id
        } else {
            id
        };

        let detail = get_song_detail(&id).await?;
        let song_mid = if !detail.mid.trim().is_empty() {
            detail.mid.clone()
        } else if detail.id > 0 {
            detail.id.to_string()
        } else {
            id.clone()
        };
        let file_info = get_song_file_info(&song_mid).await?;
        let media_mid = if file_info.media_mid.trim().is_empty() {
            song_mid.clone()
        } else {
            file_info.media_mid.clone()
        };
        let (uin, authst) = parse_qq_auth(&qq_cookie());
        let mut last_error = None;
        for quality in fallback_quality_profiles(&file_info) {
            match get_vkey(
                &song_mid,
                &media_mid,
                quality.code,
                quality.ext,
                &uin,
                &authst,
            )
            .await
            {
                Ok(purl) => {
                    let url = build_stream_url(&purl);
                    if !url.trim().is_empty() {
                        return Ok(MusicTrack {
                            id: song_mid.clone(),
                            platform: MusicPlatform::Tencent,
                            song: detail.title(),
                            singer: singers_to_string(&detail.singer),
                            album: detail.album.name.clone(),
                            cover: build_track_cover_url(
                                first_non_empty([
                                    file_info.cover_mid.as_deref(),
                                    Some(detail.album.mid.as_str()),
                                ])
                                .unwrap_or_default(),
                            ),
                            link: build_track_url(&song_mid),
                            url,
                            headers: HashMap::new(),
                            duration: None,
                            bitrate: None,
                            format: Some(quality.ext.to_string()),
                        });
                    }
                }
                Err(err) => last_error = Some(err),
            }
        }
        Err(last_error.unwrap_or_else(|| BotError::Custom("QQ音乐没有返回可下载直链".to_string())))
    }
}

#[derive(Clone, Deserialize)]
struct QQSinger {
    #[serde(default)]
    name: String,
}

#[derive(Clone, Default, Deserialize)]
struct QQAlbum {
    #[serde(default)]
    mid: String,
    #[serde(default)]
    name: String,
}

#[derive(Clone, Default, Deserialize)]
struct QQSearchSong {
    #[serde(default)]
    songid: i64,
    #[serde(default)]
    id: i64,
    #[serde(default)]
    songmid: String,
    #[serde(default)]
    mid: String,
    #[serde(default)]
    songname: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    singer: Vec<QQSinger>,
}

#[derive(Clone, Default, Deserialize)]
struct QQSearchSongMobile {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    mid: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    singer: Vec<QQSinger>,
}

#[derive(Clone, Default, Deserialize)]
struct QQSongDetail {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    mid: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    album: QQAlbum,
    #[serde(default)]
    singer: Vec<QQSinger>,
}

impl QQSongDetail {
    fn title(&self) -> String {
        first_non_empty([Some(self.title.as_str()), Some(self.name.as_str())])
            .unwrap_or("未知歌曲")
            .to_string()
    }
}

#[derive(Default, Deserialize)]
struct QQFileInfo {
    #[serde(default, rename = "media_mid")]
    media_mid: String,
    #[serde(default, rename = "size_128mp3")]
    size_128: i64,
    #[serde(default, rename = "size_320mp3")]
    size_320: i64,
    #[serde(default, rename = "size_flac")]
    size_flac: i64,
    #[serde(default, rename = "size_hires")]
    size_hires: i64,
    #[serde(skip)]
    cover_mid: Option<String>,
}

struct QualityProfile {
    code: &'static str,
    ext: &'static str,
    size_key: &'static str,
}

async fn search_by_meting_desktop(
    keyword: &str,
    limit: usize,
) -> Result<Vec<QQSearchSong>, BotError> {
    let payload = json!({
        "comm": { "ct": "19", "cv": "1859", "uin": "0" },
        "req": {
            "method": "DoSearchForQQMusicDesktop",
            "module": "music.search.SearchCgiService",
            "param": {
                "grp": 1,
                "num_per_page": limit,
                "page_num": 1,
                "query": keyword,
                "search_type": 0
            }
        }
    });
    let body = post_json(MUSICU_ENDPOINT, &payload, false).await?;
    #[derive(Deserialize)]
    struct Resp {
        code: i64,
        req: Req,
    }
    #[derive(Deserialize)]
    struct Req {
        code: i64,
        data: ReqData,
    }
    #[derive(Deserialize)]
    struct ReqData {
        body: ReqBody,
    }
    #[derive(Deserialize)]
    struct ReqBody {
        song: SongList,
    }
    #[derive(Deserialize)]
    struct SongList {
        #[serde(default)]
        list: Vec<QQSearchSong>,
    }
    let resp: Resp = serde_json::from_slice(&body)?;
    if resp.code != 0 || resp.req.code != 0 {
        return Err(BotError::Custom("QQ音乐搜索接口不可用".to_string()));
    }
    Ok(resp.req.data.body.song.list)
}

async fn search_by_musicu(keyword: &str, limit: usize) -> Result<Vec<QQSearchSong>, BotError> {
    let payload = json!({
        "comm": {
            "ct": "11",
            "cv": "14090508",
            "v": "14090508",
            "tmeAppID": "qqmusic",
            "phonetype": "EBG-AN10",
            "deviceScore": "553.47",
            "devicelevel": "50",
            "newdevicelevel": "20",
            "rom": "HuaWei/EMOTION/EmotionUI_14.2.0",
            "os_ver": "12",
            "OpenUDID": "0",
            "uid": "0",
            "modeSwitch": "6",
            "teenMode": "0",
            "ui_mode": "2",
            "nettype": "1020",
            "v4ip": ""
        },
        "req": {
            "method": "DoSearchForQQMusicMobile",
            "module": "music.search.SearchCgiService",
            "param": {
                "search_type": 0,
                "query": keyword,
                "page_num": 1,
                "num_per_page": limit,
                "highlight": 0,
                "nqc_flag": 0,
                "multi_zhida": 0,
                "cat": 2,
                "grp": 1,
                "sin": 0,
                "sem": 0
            }
        }
    });
    let body = post_json(
        &format!("{MUSICU_ENDPOINT}?format=json&inCharset=utf8&outCharset=utf8"),
        &payload,
        false,
    )
    .await?;
    #[derive(Deserialize)]
    struct Resp {
        code: i64,
        req: Req,
    }
    #[derive(Deserialize)]
    struct Req {
        code: i64,
        data: ReqData,
    }
    #[derive(Deserialize)]
    struct ReqData {
        body: ReqBody,
    }
    #[derive(Deserialize)]
    struct ReqBody {
        #[serde(default)]
        item_song: Vec<QQSearchSongMobile>,
        song: Option<SongList>,
    }
    #[derive(Deserialize)]
    struct SongList {
        #[serde(default)]
        list: Vec<QQSearchSong>,
    }
    let resp: Resp = serde_json::from_slice(&body)?;
    if resp.code != 0 || resp.req.code != 0 {
        return Err(BotError::Custom("QQ音乐移动搜索接口不可用".to_string()));
    }
    let body = resp.req.data.body;
    if let Some(song) = body.song
        && !song.list.is_empty()
    {
        return Ok(song.list);
    }
    Ok(body
        .item_song
        .into_iter()
        .map(|song| QQSearchSong {
            songid: song.id,
            songmid: song.mid,
            songname: song.name,
            singer: song.singer,
            ..Default::default()
        })
        .collect())
}

async fn search_by_legacy(keyword: &str, limit: usize) -> Result<Vec<QQSearchSong>, BotError> {
    let mut url = Url::parse(SEARCH_ENDPOINT).unwrap();
    url.query_pairs_mut()
        .append_pair("ct", "24")
        .append_pair("qqmusic_ver", "1298")
        .append_pair("new_json", "1")
        .append_pair("remoteplace", "txt.yqq.center")
        .append_pair("t", "0")
        .append_pair("aggr", "1")
        .append_pair("cr", "1")
        .append_pair("catZhida", "1")
        .append_pair("lossless", "0")
        .append_pair("flag_qc", "0")
        .append_pair("needNewCode", "0")
        .append_pair("g_tk", "5381")
        .append_pair("loginUin", "0")
        .append_pair("hostUin", "0")
        .append_pair("uin", "0")
        .append_pair("inCharset", "utf8")
        .append_pair("outCharset", "utf-8")
        .append_pair("notice", "0")
        .append_pair("format", "json")
        .append_pair("w", keyword)
        .append_pair("p", "1")
        .append_pair("n", &limit.to_string())
        .append_pair("platform", "yqq");
    let body = get_bytes(url.as_str(), false).await?;
    #[derive(Deserialize)]
    struct Resp {
        code: i64,
        data: Option<Data>,
        song: Option<SongList>,
    }
    #[derive(Deserialize)]
    struct Data {
        song: SongList,
    }
    #[derive(Deserialize)]
    struct SongList {
        #[serde(default)]
        list: Vec<QQSearchSong>,
    }
    let resp: Resp = serde_json::from_slice(&body)?;
    if resp.code != 0 {
        return Err(BotError::Custom("QQ音乐 legacy 搜索接口不可用".to_string()));
    }
    Ok(resp
        .data
        .map(|data| data.song.list)
        .or_else(|| resp.song.map(|song| song.list))
        .unwrap_or_default())
}

async fn get_song_detail(id: &str) -> Result<QQSongDetail, BotError> {
    let mut url = Url::parse(SONG_DETAIL_ENDPOINT).unwrap();
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("platform", "yqq")
            .append_pair("format", "json");
        if id.chars().all(|c| c.is_ascii_digit()) {
            query.append_pair("songid", id);
        } else {
            query.append_pair("songmid", id);
        }
    }
    let body = get_bytes(url.as_str(), true).await?;
    #[derive(Deserialize)]
    struct Resp {
        code: i64,
        #[serde(default)]
        data: Vec<QQSongDetail>,
    }
    let resp: Resp = serde_json::from_slice(&body)?;
    if resp.code != 0 {
        return Err(BotError::Custom("QQ音乐歌曲详情接口不可用".to_string()));
    }
    resp.data
        .into_iter()
        .next()
        .ok_or_else(|| BotError::Custom("没有找到 QQ 音乐歌曲详情".to_string()))
}

async fn get_song_file_info(song_mid: &str) -> Result<QQFileInfo, BotError> {
    let payload = json!({
        "comm": { "ct": "19", "cv": "1859", "uin": "0" },
        "req": {
            "module": "music.pf_song_detail_svr",
            "method": "get_song_detail_yqq",
            "param": { "song_type": 0, "song_mid": song_mid }
        }
    });
    let json_body = serde_json::to_vec(&payload)?;
    let sign = tencent_sign(&String::from_utf8_lossy(&json_body), false);
    let endpoint = format!(
        "{MUSICS_ENDPOINT}?format=json&sign={}",
        urlencoding::encode(&sign)
    );
    let body = post_json_raw(&endpoint, json_body, true).await?;
    #[derive(Deserialize)]
    struct Resp {
        req: Req,
    }
    #[derive(Deserialize)]
    struct Req {
        data: ReqData,
    }
    #[derive(Deserialize)]
    struct ReqData {
        track_info: TrackInfo,
    }
    #[derive(Deserialize)]
    struct TrackInfo {
        file: QQFileInfo,
        #[serde(default)]
        vs: Vec<String>,
    }
    let resp: Resp = serde_json::from_slice(&body)?;
    let mut file = resp.req.data.track_info.file;
    if let Some(cover_mid) = resp.req.data.track_info.vs.get(1) {
        file.cover_mid = Some(cover_mid.trim().to_string());
    }
    if file.media_mid.trim().is_empty() {
        return Err(BotError::Custom("QQ音乐没有返回文件信息".to_string()));
    }
    Ok(file)
}

async fn get_vkey(
    song_mid: &str,
    media_mid: &str,
    quality_code: &str,
    ext: &str,
    uin: &str,
    authst: &str,
) -> Result<String, BotError> {
    let guid = random_hex32();
    for filename in build_vkey_filenames(song_mid, media_mid, quality_code, ext) {
        let payload = json!({
            "req": {
                "module": "music.vkey.GetVkey",
                "method": "UrlGetVkey",
                "param": {
                    "filename": [filename],
                    "guid": guid,
                    "songmid": [song_mid],
                    "songtype": [0],
                    "uin": uin,
                    "loginflag": 1,
                    "platform": "20"
                }
            },
            "comm": {
                "qq": uin,
                "uin": uin,
                "authst": authst,
                "tmeLoginType": 2,
                "ct": 19,
                "cv": 13020508,
                "v": 13020508,
                "format": "json"
            }
        });
        let json_body = serde_json::to_vec(&payload)?;
        let sign = tencent_sign(&String::from_utf8_lossy(&json_body), true);
        let endpoint = format!(
            "{MUSICS_ENDPOINT}?format=json&sign={}",
            urlencoding::encode(&sign)
        );
        let body = post_json_raw(&endpoint, json_body, true).await?;
        #[derive(Deserialize)]
        struct Resp {
            req: Req,
        }
        #[derive(Deserialize)]
        struct Req {
            data: ReqData,
        }
        #[derive(Deserialize)]
        struct ReqData {
            #[serde(default)]
            midurlinfo: Vec<MidUrlInfo>,
        }
        #[derive(Deserialize)]
        struct MidUrlInfo {
            #[serde(default)]
            purl: String,
            #[serde(default)]
            vkey: String,
            #[serde(default)]
            wifiurl: String,
        }
        let resp: Resp = serde_json::from_slice(&body)?;
        if let Some(info) = resp.req.data.midurlinfo.into_iter().next()
            && let Some(url) = resolve_vkey_url(&info.purl, &info.wifiurl, &info.vkey)
        {
            return Ok(url);
        }
    }
    Err(BotError::Custom("QQ音乐没有返回 vkey".to_string()))
}

async fn get_bytes(url: &str, include_cookie: bool) -> Result<Vec<u8>, BotError> {
    let mut request = CLIENT.get(url);
    request = apply_qq_headers(request, include_cookie);
    let response = request.send().await?;
    if !response.status().is_success() {
        return Err(BotError::Custom(format!(
            "QQ音乐接口请求失败：HTTP {}",
            response.status()
        )));
    }
    Ok(response.bytes().await?.to_vec())
}

async fn post_json(
    url: &str,
    payload: &serde_json::Value,
    include_cookie: bool,
) -> Result<Vec<u8>, BotError> {
    post_json_raw(url, serde_json::to_vec(payload)?, include_cookie).await
}

async fn post_json_raw(
    url: &str,
    body: Vec<u8>,
    include_cookie: bool,
) -> Result<Vec<u8>, BotError> {
    let mut request = CLIENT.post(url).body(body);
    request = apply_qq_headers(request, include_cookie);
    let response = request.send().await?;
    if !response.status().is_success() {
        return Err(BotError::Custom(format!(
            "QQ音乐接口请求失败：HTTP {}",
            response.status()
        )));
    }
    Ok(response.bytes().await?.to_vec())
}

fn apply_qq_headers(
    mut request: reqwest::RequestBuilder,
    include_cookie: bool,
) -> reqwest::RequestBuilder {
    request = request
        .header("User-Agent", "QQMusic/14090508 (android 12)")
        .header("Referer", "https://y.qq.com/")
        .header("Origin", "https://y.qq.com")
        .header("Accept", "*/*")
        .header("Content-Type", "application/json");
    if include_cookie {
        let cookie = qq_cookie();
        if !cookie.trim().is_empty() {
            request = request.header("Cookie", cookie);
        }
    }
    request
}

fn search_song_to_item(song: QQSearchSong) -> MusicSearchItem {
    MusicSearchItem {
        platform: MusicPlatform::Tencent,
        id: first_non_empty([Some(song.songmid.as_str()), Some(song.mid.as_str())])
            .map(ToOwned::to_owned)
            .or_else(|| (song.songid > 0).then(|| song.songid.to_string()))
            .or_else(|| (song.id > 0).then(|| song.id.to_string()))
            .unwrap_or_default(),
        song: first_non_empty([
            Some(song.songname.as_str()),
            Some(song.title.as_str()),
            Some(song.name.as_str()),
        ])
        .unwrap_or("未知歌曲")
        .to_string(),
        singer: singers_to_string(&song.singer),
        cover: String::new(),
    }
}

fn parse_qq_track_id(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if text.chars().all(|c| c.is_ascii_digit()) || is_tencent_song_mid(text) {
        return Some(text.to_string());
    }
    let url = Url::parse(text).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    if !host.contains("qq.com") {
        return None;
    }
    let path = url.path().trim_matches('/');
    for re in QQ_SONG_PATH_PATTERNS.iter() {
        if let Some(id) = re
            .captures(path)
            .and_then(|captures| captures.get(1))
            .map(|value| value.as_str().to_string())
        {
            return Some(id);
        }
    }
    for key in ["songmid", "songid", "id"] {
        if let Some(id) = url
            .query_pairs()
            .find(|(query_key, _)| query_key == key)
            .map(|(_, value)| value.to_string())
            .filter(|value| !value.trim().is_empty())
        {
            return Some(id);
        }
    }
    None
}

fn fallback_quality_profiles(info: &QQFileInfo) -> Vec<QualityProfile> {
    [
        QualityProfile {
            code: "RS01",
            ext: "flac",
            size_key: "size_hires",
        },
        QualityProfile {
            code: "F000",
            ext: "flac",
            size_key: "size_flac",
        },
        QualityProfile {
            code: "M800",
            ext: "mp3",
            size_key: "size_320mp3",
        },
        QualityProfile {
            code: "M500",
            ext: "mp3",
            size_key: "size_128mp3",
        },
    ]
    .into_iter()
    .filter(|quality| quality.size(info) > 0)
    .collect()
}

impl QualityProfile {
    fn size(&self, info: &QQFileInfo) -> i64 {
        match self.size_key {
            "size_hires" => info.size_hires,
            "size_flac" => info.size_flac,
            "size_320mp3" => info.size_320,
            "size_128mp3" => info.size_128,
            _ => 0,
        }
    }
}

fn build_vkey_filenames(
    song_mid: &str,
    media_mid: &str,
    quality_code: &str,
    ext: &str,
) -> Vec<String> {
    let mut seen = HashSet::new();
    [
        format!("{quality_code}{media_mid}.{ext}"),
        format!("{quality_code}{song_mid}{song_mid}.{ext}"),
    ]
    .into_iter()
    .filter(|item| !item.trim().is_empty())
    .filter(|item| seen.insert(item.clone()))
    .collect()
}

fn resolve_vkey_url(purl: &str, wifi_url: &str, vkey: &str) -> Option<String> {
    if !purl.trim().is_empty() && !vkey.trim().is_empty() {
        Some(purl.trim().to_string())
    } else if !wifi_url.trim().is_empty() {
        Some(wifi_url.trim().to_string())
    } else {
        None
    }
}

fn build_stream_url(purl: &str) -> String {
    let purl = purl.trim();
    if purl.starts_with("http://") || purl.starts_with("https://") {
        purl.to_string()
    } else if purl.is_empty() {
        String::new()
    } else {
        format!("https://ws.stream.qqmusic.qq.com/{purl}")
    }
}

fn build_track_url(track_id: &str) -> String {
    format!("https://y.qq.com/n/ryqq_v2/songDetail/{track_id}")
}

fn build_track_cover_url(album_mid: &str) -> String {
    let album_mid = album_mid.trim();
    if album_mid.is_empty() {
        String::new()
    } else {
        format!("https://y.gtimg.cn/music/photo_new/T002M000{album_mid}.jpg")
    }
}

fn singers_to_string(singers: &[QQSinger]) -> String {
    let names = singers
        .iter()
        .map(|singer| singer.name.trim())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    if names.is_empty() {
        "未知歌手".to_string()
    } else {
        names.join(" / ")
    }
}

fn first_non_empty<'a>(items: impl IntoIterator<Item = Option<&'a str>>) -> Option<&'a str> {
    items
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|item| !item.is_empty())
}

fn is_tencent_song_mid(text: &str) -> bool {
    let text = text.trim();
    text.len() >= 12
        && text.len() <= 16
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn parse_qq_auth(cookie: &str) -> (String, String) {
    let uin = normalize_qq_uin(&parse_cookie_value(cookie, "uin"));
    let authst = first_non_empty([
        Some(parse_cookie_value(cookie, "qqmusic_key").as_str()),
        Some(parse_cookie_value(cookie, "qm_keyst").as_str()),
    ])
    .unwrap_or_default()
    .to_string();
    (uin, authst)
}

fn normalize_qq_uin(raw: &str) -> String {
    let trimmed = raw
        .trim()
        .trim_start_matches(['o', 'O'])
        .trim_start_matches('0');
    if trimmed.is_empty() || !trimmed.chars().all(|c| c.is_ascii_digit()) {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

fn parse_cookie_value(cookie: &str, key: &str) -> String {
    cookie
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(cookie_key, _)| cookie_key.trim() == key)
        .map(|(_, value)| value.trim().to_string())
        .unwrap_or_default()
}

fn qq_cookie() -> String {
    std::env::var("QQ_MUSIC_COOKIE")
        .ok()
        .map(|cookie| cookie.trim().to_string())
        .unwrap_or_default()
}

fn random_hex32() -> String {
    let bytes = rand::random::<[u8; 16]>();
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn tencent_sign(payload: &str, clear_part1: bool) -> String {
    if payload.is_empty() {
        return String::new();
    }
    let mut hasher = Sha1::new();
    hasher.update(payload.as_bytes());
    let hash = format!("{:X}", hasher.finalize());
    let mut part1 = [23usize, 14, 6, 36, 16, 40, 7, 19]
        .into_iter()
        .filter_map(|idx| hash.get(idx..idx + 1))
        .collect::<String>();
    if clear_part1 {
        part1.clear();
    }
    let part2 = [16usize, 1, 32, 12, 19, 27, 8, 5]
        .into_iter()
        .filter_map(|idx| hash.get(idx..idx + 1))
        .collect::<String>();
    let scramble_values = [
        89u8, 39, 179, 150, 218, 82, 58, 252, 177, 52, 186, 123, 120, 64, 242, 133, 143, 161, 121,
        179,
    ];
    let mut part3 = Vec::with_capacity(scramble_values.len());
    for (i, scramble) in scramble_values.into_iter().enumerate() {
        let pos = i * 2;
        let Some(hex_byte) = hash.get(pos..pos + 2) else {
            break;
        };
        let Ok(value) = u8::from_str_radix(hex_byte, 16) else {
            return String::new();
        };
        part3.push(scramble ^ value);
    }
    let b64 = base64::engine::general_purpose::STANDARD
        .encode(part3)
        .replace(['/', '\\', '+', '='], "");
    format!("zzc{}", format!("{part1}{b64}{part2}").to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_qq_song_url() {
        assert_eq!(
            parse_qq_track_id("https://y.qq.com/n/ryqq/songDetail/0039MnYb0qxYhV"),
            Some("0039MnYb0qxYhV".to_string())
        );
    }

    #[test]
    fn builds_vkey_filenames_without_duplicates() {
        let names = build_vkey_filenames("songmid", "songmid", "M800", "mp3");
        assert_eq!(names, vec!["M800songmid.mp3", "M800songmidsongmid.mp3"]);
    }

    #[test]
    fn native_search_endpoint_does_not_use_legacy_bot_api() {
        let url = Url::parse(MUSICU_ENDPOINT).unwrap();
        assert_eq!(url.host_str(), Some("u.y.qq.com"));
    }

    #[test]
    fn sign_matches_known_shape() {
        let sign = tencent_sign(r#"{"test":1}"#, true);
        assert!(sign.starts_with("zzc"));
        assert!(sign.len() > 20);
    }
}
