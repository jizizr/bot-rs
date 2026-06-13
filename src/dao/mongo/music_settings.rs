use super::*;
use crate::index_builder;
use serde::{Deserialize, Serialize};

const DEFAULT_PLATFORM: &str = "tencent";
const DEFAULT_APPLE_QUALITY: &str = "high";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UserMusicSettings {
    pub user_id: i64,
    #[serde(default = "default_platform")]
    pub default_platform: String,
    #[serde(default = "default_apple_quality")]
    pub apple_quality: String,
    #[serde(default = "default_true")]
    pub send_cover: bool,
}

impl UserMusicSettings {
    pub fn defaults_for(user_id: i64) -> Self {
        Self {
            user_id,
            default_platform: DEFAULT_PLATFORM.to_string(),
            apple_quality: default_apple_quality(),
            send_cover: true,
        }
    }
}

pub fn default_platform() -> String {
    DEFAULT_PLATFORM.to_string()
}

pub fn default_apple_quality() -> String {
    SETTINGS
        .music
        .applemusic
        .quality
        .trim()
        .to_string()
        .if_empty(DEFAULT_APPLE_QUALITY)
}

fn default_true() -> bool {
    true
}

async fn async_collection() -> Collection<UserMusicSettings> {
    DB.get()
        .await
        .collection::<UserMusicSettings>("music_settings")
}

pub async fn create_index(db: &mongodb::Database) {
    let collection = db.collection::<bson::Document>("music_settings");
    index_builder!(collection, "user_id");
}

pub async fn get_user_settings(user_id: i64) -> Result<UserMusicSettings, BotError> {
    if is_mongo_disabled() {
        return Ok(UserMusicSettings::defaults_for(user_id));
    }
    let collection = async_collection().await;
    let filter = doc! {"user_id": user_id};
    match collection.find_one(filter.clone()).await {
        Ok(Some(settings)) => Ok(normalize_settings(settings)),
        Ok(None) => {
            let settings = UserMusicSettings::defaults_for(user_id);
            collection
                .replace_one(filter, settings.clone())
                .upsert(true)
                .await
                .map_err(|e| {
                    disable_mongo();
                    e
                })?;
            Ok(settings)
        }
        Err(e) => {
            disable_mongo();
            Err(e.into())
        }
    }
}

pub async fn get_user_settings_or_default(user_id: i64) -> UserMusicSettings {
    get_user_settings(user_id)
        .await
        .unwrap_or_else(|_| UserMusicSettings::defaults_for(user_id))
}

pub async fn save_user_settings(settings: &UserMusicSettings) -> Result<(), BotError> {
    if is_mongo_disabled() {
        return Err(BotError::Custom(
            "MongoDB 未启用，无法保存音乐设置".to_string(),
        ));
    }
    let settings = normalize_settings(settings.clone());
    async_collection()
        .await
        .replace_one(doc! {"user_id": settings.user_id}, settings)
        .upsert(true)
        .await
        .map_err(|e| {
            disable_mongo();
            e
        })?;
    Ok(())
}

fn normalize_settings(mut settings: UserMusicSettings) -> UserMusicSettings {
    if settings.default_platform.trim().is_empty() {
        settings.default_platform = default_platform();
    }
    if settings.apple_quality.trim().is_empty() {
        settings.apple_quality = default_apple_quality();
    }
    settings
}

trait IfEmpty {
    fn if_empty(self, fallback: &str) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.trim().is_empty() {
            fallback.to_string()
        } else {
            self
        }
    }
}
