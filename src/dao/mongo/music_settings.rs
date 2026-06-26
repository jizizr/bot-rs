use super::*;
use crate::index_builder;
use serde::{Deserialize, Serialize};

const DEFAULT_PLATFORM: &str = "soda";
const DEFAULT_QUALITY: &str = "lossless";
const DEFAULT_LYRIC_SCRIPT: &str = "simplified";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UserMusicSettings {
    pub user_id: i64,
    #[serde(default = "default_platform")]
    pub default_platform: String,
    #[serde(default = "default_quality")]
    pub quality: String,
    #[serde(default = "default_true")]
    pub send_cover: bool,
    #[serde(default = "default_lyric_script")]
    pub lyric_script: String,
}

impl UserMusicSettings {
    pub fn defaults_for(user_id: i64) -> Self {
        Self {
            user_id,
            default_platform: DEFAULT_PLATFORM.to_string(),
            quality: default_quality(),
            send_cover: true,
            lyric_script: default_lyric_script(),
        }
    }
}

pub fn default_platform() -> String {
    DEFAULT_PLATFORM.to_string()
}

pub fn default_quality() -> String {
    DEFAULT_QUALITY.to_string()
}

pub fn default_lyric_script() -> String {
    DEFAULT_LYRIC_SCRIPT.to_string()
}

fn default_true() -> bool {
    true
}

async fn async_collection() -> Collection<UserMusicSettings> {
    db().await.collection::<UserMusicSettings>("music_settings")
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
                .inspect_err(|_| {
                    disable_mongo();
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
        .inspect_err(|_| {
            disable_mongo();
        })?;
    Ok(())
}

fn normalize_settings(mut settings: UserMusicSettings) -> UserMusicSettings {
    if settings.default_platform.trim().is_empty() {
        settings.default_platform = default_platform();
    }
    if settings.quality.trim().is_empty() {
        settings.quality = default_quality();
    }
    if !matches!(settings.lyric_script.trim(), "simplified" | "traditional") {
        settings.lyric_script = default_lyric_script();
    }
    settings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_use_soda_and_lossless_quality() {
        let settings = UserMusicSettings::defaults_for(1);

        assert_eq!(settings.default_platform, "soda");
        assert_eq!(settings.quality, "lossless");
        assert_eq!(settings.lyric_script, "simplified");
    }
}
