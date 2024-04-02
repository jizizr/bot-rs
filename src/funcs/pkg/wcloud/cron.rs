use super::gen;
use crate::dao::{
    mysql::wordcloud::{active_group, clear_words},
    rdb::wordcloud::*,
};
use bot_rs::BOT;

pub async fn wcloud() -> Result<(), Vec<Box<dyn std::error::Error + Send + Sync>>> {
    let mut err_vec: Vec<Box<dyn std::error::Error + Send + Sync>> = vec![];
    for group in active_group().await.map_err(|e| vec![e.into()])? {
        if !get_flag(group).await.unwrap_or_else(|e| {
            err_vec.push(e);
            true
        }) {
            continue;
        }
        match gen::wcloud(&BOT, group).await {
            Ok(_) => {
                wc_switch(group, false)
                    .await
                    .unwrap_or_else(|e| err_vec.push(e));
            }
            Err(e) => {
                err_vec.push(e);
            }
        }
        if let Err(e) = gen::user_freq(&BOT, group).await {
            err_vec.push(e);
        }
    }
    if err_vec.is_empty() {
        Ok(())
    } else {
        Err(err_vec)
    }
}

pub async fn wcloud_then_clear() -> Result<(), Vec<Box<dyn std::error::Error + Send + Sync>>> {
    let mut err_vec = match wcloud().await {
        Ok(_) => {
            vec![]
        }
        Err(e) => e,
    };
    clear_words().await.unwrap_or_else(|e| err_vec.push(e));
    if err_vec.is_empty() {
        Ok(())
    } else {
        Err(err_vec)
    }
}
