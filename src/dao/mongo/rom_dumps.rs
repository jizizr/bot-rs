//! /dump 提取过的分区镜像：`{ _id: <包路径#分区>, file_id, sha256, size }`。
//! 同一个包的同一个分区再请求时直接用 file_id 发，不再下载。
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize)]
pub struct RomDump {
    #[serde(rename = "_id")]
    pub key: String,
    pub file_id: String,
    pub sha256: String,
    pub size: u64,
}

async fn collection() -> Collection<RomDump> {
    db().await.collection::<RomDump>("rom_dumps")
}

/// 缓存的镜像；MongoDB 不可用时当作没有
pub async fn get(key: &str) -> Option<RomDump> {
    if is_mongo_disabled() {
        return None;
    }
    match collection().await.find_one(doc! { "_id": key }).await {
        Ok(found) => {
            mark_mongo_ok();
            found
        }
        Err(err) => {
            disable_mongo();
            log::error!("rom dump lookup failed: {err}");
            None
        }
    }
}

pub async fn put(dump: &RomDump) -> Result<(), BotError> {
    if is_mongo_disabled() {
        return Err(BotError::Custom("MongoDB 暂不可用".to_string()));
    }
    collection()
        .await
        .replace_one(doc! { "_id": &dump.key }, dump)
        .upsert(true)
        .await
        .inspect_err(|_| disable_mongo())?;
    mark_mongo_ok();
    Ok(())
}
