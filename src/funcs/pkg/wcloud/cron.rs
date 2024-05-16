use super::gen;
use crate::dao::{
    mysql::wordcloud::{active_group, clear},
    rdb::wordcloud::*,
};
use bot_rs::{BotError, BOT};
use futures::{stream::FuturesUnordered, StreamExt};
pub async fn wcloud() -> Result<(), Vec<BotError>> {
    let mut err_vec = Vec::new();
    let mut futures = FuturesUnordered::new();
    for group in active_group().await.map_err(|e| vec![e.into()])? {
        futures.push(tokio::spawn(wcloud_single(group)));
    }

    while let Some(result) = futures.next().await {
        match result {
            Ok(e) => {
                err_vec.extend(e);
            }
            Err(e) => {
                err_vec.push(e.into());
            }
        }
    }

    if err_vec.is_empty() {
        Ok(())
    } else {
        Err(err_vec)
    }
}

async fn wcloud_single(group: i64) -> Vec<BotError> {
    let mut err_vec = Vec::new();
    let flag = get_flag(group).await.unwrap_or_else(|e| {
        err_vec.push(e);
        true
    });
    if !flag {
        return err_vec;
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
    err_vec
}

pub async fn wcloud_then_clear() -> Result<(), Vec<BotError>> {
    let mut err_vec = match wcloud().await {
        Ok(_) => {
            vec![]
        }
        Err(e) => e,
    };
    clear().await.unwrap_or_else(|e| err_vec.push(e));
    if err_vec.is_empty() {
        Ok(())
    } else {
        Err(err_vec)
    }
}
