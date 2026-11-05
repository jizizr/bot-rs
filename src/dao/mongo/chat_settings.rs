//! Settings that belong to a chat rather than to a user or a feature, one
//! document per chat: `{ _id: <chat id>, timezone: "<IANA name>" }`.
use super::*;
use serde::Deserialize;

#[derive(Deserialize)]
struct ChatSettings {
    #[serde(default)]
    timezone: Option<String>,
}

async fn collection() -> Collection<ChatSettings> {
    db().await.collection::<ChatSettings>("chat_settings")
}

/// The chat's time zone name, if one was set.
pub async fn get_timezone(chat_id: i64) -> Result<Option<String>, BotError> {
    if is_mongo_disabled() {
        return Err(BotError::Custom("MongoDB 暂不可用".to_string()));
    }
    let settings = collection()
        .await
        .find_one(doc! { "_id": chat_id })
        .await
        .inspect_err(|_| disable_mongo())?;
    mark_mongo_ok();
    Ok(settings.and_then(|settings| settings.timezone))
}

pub async fn set_timezone(chat_id: i64, timezone: &str) -> Result<(), BotError> {
    if is_mongo_disabled() {
        return Err(BotError::Custom("MongoDB 暂不可用".to_string()));
    }
    collection()
        .await
        .update_one(
            doc! { "_id": chat_id },
            doc! { "$set": { "timezone": timezone } },
        )
        .upsert(true)
        .await
        .inspect_err(|_| disable_mongo())?;
    mark_mongo_ok();
    Ok(())
}
