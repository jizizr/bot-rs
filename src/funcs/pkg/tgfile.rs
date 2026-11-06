//! 下载用户发来的 Telegram 文件（图片、贴纸等），语录和翻译共用

use std::path::{Path, PathBuf};

use telers::{Bot, methods::GetFile};

use crate::{BotError, settings::SETTINGS};

/// 按 file_id 取文件内容；`timeout` 是下载本身的秒数。
/// 错误信息里不会带 token（本地文件路径、下载 URL 都不打出来）
pub async fn download(bot: &Bot, file_id: &str, timeout: f32) -> Result<Vec<u8>, BotError> {
    let file = bot.send(GetFile::new(file_id)).await?;
    let path = file
        .file_path
        .ok_or_else(|| BotError::Custom("Telegram 没有给出文件路径".to_string()))?;
    // 本地模式的 Bot API 直接给出文件路径，没有下载接口
    if path.starts_with('/') {
        let local = local_file_path(
            &path,
            &SETTINGS.bot.token,
            SETTINGS.bot.files_dir.as_deref(),
        );
        // io::Error 的文字不含路径
        return tokio::fs::read(&local)
            .await
            .map_err(|e| BotError::Custom(format!("读取本地文件失败（检查 bot.files_dir）：{e}")));
    }
    // 走 Bot API 会话自己的连接池（和其他 API 调用同一个 host，连接复用），
    // telers 的错误里已经去掉了带 token 的 URL
    let bytes = bot
        .download_file_with_timeout(&path, timeout)
        .await
        .map_err(|e| BotError::Custom(format!("下载文件失败：{e}")))?
        .bytes()
        .await
        .map_err(|e| BotError::Custom(format!("下载文件失败：{e}")))?;
    // 独占的 Bytes 转 Vec 不会再拷一遍
    Ok(bytes.into())
}

/// 把本地模式 Bot API 给的服务端路径 `<服务端数据目录>/<token>/photos/x.jpg`
/// 换到 `<files_dir>/<token>/photos/x.jpg`；没配 `files_dir` 时视为同机同路径
fn local_file_path(path: &str, token: &str, files_dir: Option<&str>) -> PathBuf {
    let files_dir = files_dir.map(str::trim).filter(|dir| !dir.is_empty());
    match (files_dir, path.find(&format!("/{token}/"))) {
        (Some(dir), Some(at)) => Path::new(dir).join(&path[at + 1..]),
        _ => PathBuf::from(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_bot_api_paths_map_into_files_dir() {
        let path = "/var/lib/telegram-bot-api/123:abc/photos/file_1.jpg";
        assert_eq!(
            local_file_path(path, "123:abc", Some("/root/telegram-bot-api/data")),
            PathBuf::from("/root/telegram-bot-api/data/123:abc/photos/file_1.jpg")
        );
        // 没配目录、配了空串、路径里没有 token 时原样使用
        assert_eq!(local_file_path(path, "123:abc", None), PathBuf::from(path));
        assert_eq!(
            local_file_path(path, "123:abc", Some(" ")),
            PathBuf::from(path)
        );
        assert_eq!(
            local_file_path(path, "456:def", Some("/data")),
            PathBuf::from(path)
        );
    }
}
