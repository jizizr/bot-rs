use super::*;
use crate::funcs::command::{coin, music};
use std::error::Error;

pub async fn call_query_handler(
    bot: Bot,
    mut q: CallbackQuery,
) -> std::result::Result<(), Box<dyn Error + std::marker::Send + Sync>> {
    let binding = q.data.unwrap();
    let data: Vec<&str> = binding.splitn(2, " ").collect();
    q.data = Some(data[1].to_string());
    if "coin" == data[0] {
        return coin::coin_callback(bot, q).await;
    } else if "music" == data[0] {
        return music::music_callback(bot, q).await;
    }
    Ok(())
}
