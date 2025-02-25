use redis::AsyncConnectionConfig;

use super::*;
use std::{convert::Into, time::Duration};
pub enum SwitchType {
    WordCloud,
}

impl From<SwitchType> for usize {
    fn from(val: SwitchType) -> Self {
        match val {
            SwitchType::WordCloud => 0,
        }
    }
}

pub async fn change_flag<T: Into<usize>>(group_id: i64, offset: T, flag: bool) -> BotResult {
    let mut conn = RDB
        .get_multiplexed_async_connection_with_config(
            &AsyncConnectionConfig::default()
                .set_connection_timeout(Duration::from_millis(500))
                .set_response_timeout(Duration::from_millis(500)),
        )
        .await?;
    let key = format!("bot:{}", group_id);
    Ok(conn.setbit(key, offset.into(), flag).await?)
}

pub async fn get_flag<T: Into<usize>>(group_id: i64, offset: T) -> Result<bool, BotError> {
    let mut conn = RDB
        .get_multiplexed_async_connection_with_config(
            &AsyncConnectionConfig::default()
                .set_connection_timeout(Duration::from_millis(500))
                .set_response_timeout(Duration::from_millis(500)),
        )
        .await?;
    let key = format!("bot:{}", group_id);
    Ok(conn.getbit(key, offset.into()).await?)
}
