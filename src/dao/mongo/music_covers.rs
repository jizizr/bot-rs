//! Cover banners already uploaded to Telegram, one document per banner:
//! `{ _id: <banner key>, file_id }`. Kept across restarts so a cover is
//! uploaded once, not once per process.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct CoverPhoto {
    #[serde(rename = "_id")]
    key: String,
    file_id: String,
}

async fn collection() -> Collection<CoverPhoto> {
    db().await.collection::<CoverPhoto>("music_covers")
}

/// The file id stored for `key`, if any. Unknown while MongoDB is down.
pub async fn get(key: &str) -> Option<String> {
    if is_mongo_disabled() {
        return None;
    }
    match collection().await.find_one(doc! { "_id": key }).await {
        Ok(found) => {
            mark_mongo_ok();
            found.map(|cover| cover.file_id)
        }
        Err(err) => {
            disable_mongo();
            log::error!("music cover lookup failed: {err}");
            None
        }
    }
}

/// Remembers the file id uploaded for `key`.
pub async fn put(key: &str, file_id: &str) -> Result<(), BotError> {
    if is_mongo_disabled() {
        return Err(BotError::Custom("MongoDB 暂不可用".to_string()));
    }
    collection()
        .await
        .replace_one(
            doc! { "_id": key },
            CoverPhoto {
                key: key.to_string(),
                file_id: file_id.to_string(),
            },
        )
        .upsert(true)
        .await
        .inspect_err(|_| disable_mongo())?;
    mark_mongo_ok();
    Ok(())
}
