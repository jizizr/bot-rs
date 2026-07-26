use redis::{AsyncConnectionConfig, aio::MultiplexedConnection};

use super::*;
use std::{convert::Into, sync::Mutex, time::Duration};
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

// MultiplexedConnection 设计为创建一次、廉价 clone 复用；
// 命令失败时置空缓存，下次调用重新建连，保持原先每次建连的自愈能力
static CONN: Mutex<Option<MultiplexedConnection>> = Mutex::new(None);

/// 返回连接以及它是否来自缓存（缓存连接可能已被服务端断开）
async fn shared_conn() -> Result<(MultiplexedConnection, bool), BotError> {
    let cached = CONN.lock().unwrap().clone();
    if let Some(conn) = cached {
        return Ok((conn, true));
    }
    let conn = RDB
        .get_multiplexed_async_connection_with_config(
            &AsyncConnectionConfig::default()
                .set_connection_timeout(Duration::from_millis(500))
                .set_response_timeout(Duration::from_millis(500)),
        )
        .await?;
    *CONN.lock().unwrap() = Some(conn.clone());
    Ok((conn, false))
}

fn invalidate_conn() {
    *CONN.lock().unwrap() = None;
}

/// 执行命令；缓存连接失败时重建连接再试一次，让断线对调用方透明
async fn run_cmd<T: redis::FromRedisValue>(cmd: &redis::Cmd) -> Result<T, BotError> {
    let (mut conn, from_cache) = shared_conn().await?;
    match cmd.query_async(&mut conn).await {
        Ok(v) => Ok(v),
        Err(e) => {
            invalidate_conn();
            if !from_cache {
                return Err(e.into());
            }
            let (mut conn, _) = shared_conn().await?;
            match cmd.query_async(&mut conn).await {
                Ok(v) => Ok(v),
                Err(e) => {
                    invalidate_conn();
                    Err(e.into())
                }
            }
        }
    }
}

pub async fn change_flag<T: Into<usize>>(group_id: i64, offset: T, flag: bool) -> BotResult {
    let key = format!("bot:{group_id}");
    run_cmd(redis::cmd("SETBIT").arg(key).arg(offset.into()).arg(flag)).await
}

pub async fn get_flag<T: Into<usize>>(group_id: i64, offset: T) -> Result<bool, BotError> {
    let key = format!("bot:{group_id}");
    run_cmd(redis::cmd("GETBIT").arg(key).arg(offset.into())).await
}
