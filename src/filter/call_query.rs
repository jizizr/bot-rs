use super::*;
use crate::{
    BotResult,
    funcs::command::{coin, config, music, translate},
};

pub async fn call_query_handler(bot: Bot, mut q: CallbackQuery) -> BotResult {
    let binding = q.data.unwrap();
    let data: Vec<&str> = binding.splitn(2, ' ').collect();
    q.data = Some(data[1].to_string());
    if "coin" == data[0] {
        return coin::coin_callback(bot, q).await;
    } else if "music" == data[0] {
        return music::music_callback(bot, q).await.map_err(|e| e.into());
    } else if "config" == data[0] {
        return config::config_callback(bot, q).await;
    } else if "trans" == data[0] {
        return translate::translate_callback(bot, q)
            .await
            .map_err(|e| e.into());
    }
    Ok(())
}
