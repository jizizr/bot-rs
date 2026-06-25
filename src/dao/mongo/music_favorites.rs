use super::*;
use crate::{BotError, index_builder};
use futures::TryStreamExt;
use mongodb::{
    IndexModel,
    options::{FindOptions, IndexOptions},
};
use serde::{Deserialize, Serialize};

pub const FAVORITE_SCOPE_USER: &str = "user";
pub const FAVORITE_SCOPE_GROUP: &str = "group";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MusicFavorite {
    pub scope_type: String,
    pub scope_id: i64,
    pub platform: String,
    pub track_id: String,
    pub added_by_user_id: i64,
    pub added_by_name: String,
    pub song: String,
    pub singer: String,
    pub album: String,
    pub link: String,
    pub created_at: bson::DateTime,
}

impl MusicFavorite {
    pub fn key(scope_type: &str, scope_id: i64, platform: &str, track_id: &str) -> bson::Document {
        doc! {
            "scope_type": scope_type,
            "scope_id": scope_id,
            "platform": platform,
            "track_id": track_id,
        }
    }
}

async fn async_collection() -> Collection<MusicFavorite> {
    db().await.collection::<MusicFavorite>("music_favorites")
}

pub async fn create_index(db: &mongodb::Database) {
    let collection = db.collection::<bson::Document>("music_favorites");
    index_builder!(collection, "scope_id", "added_by_user_id", "created_at");
    let _ = collection
        .create_index(
            IndexModel::builder()
                .keys(doc! {
                    "scope_type": 1,
                    "scope_id": 1,
                    "platform": 1,
                    "track_id": 1,
                })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await
        .inspect_err(|_| {
            eprintln!("Failed to create unique music_favorites index");
        });
}

pub async fn is_favorited(
    scope_type: &str,
    scope_id: i64,
    platform: &str,
    track_id: &str,
) -> Result<bool, BotError> {
    if is_mongo_disabled() {
        return Ok(false);
    }
    let filter = MusicFavorite::key(scope_type, scope_id, platform, track_id);
    async_collection()
        .await
        .find_one(filter)
        .await
        .map(|favorite| favorite.is_some())
        .inspect_err(|_| {
            disable_mongo();
        })
        .map_err(Into::into)
}

pub async fn upsert_favorite(favorite: MusicFavorite) -> Result<(), BotError> {
    if is_mongo_disabled() {
        return Err(BotError::Custom("MongoDB 未启用，无法保存收藏".to_string()));
    }
    let filter = MusicFavorite::key(
        &favorite.scope_type,
        favorite.scope_id,
        &favorite.platform,
        &favorite.track_id,
    );
    async_collection()
        .await
        .replace_one(filter, favorite)
        .upsert(true)
        .await
        .inspect_err(|_| {
            disable_mongo();
        })?;
    Ok(())
}

pub async fn remove_favorite(
    scope_type: &str,
    scope_id: i64,
    platform: &str,
    track_id: &str,
) -> Result<bool, BotError> {
    if is_mongo_disabled() {
        return Err(BotError::Custom("MongoDB 未启用，无法删除收藏".to_string()));
    }
    let filter = MusicFavorite::key(scope_type, scope_id, platform, track_id);
    let result = async_collection()
        .await
        .delete_one(filter)
        .await
        .inspect_err(|_| {
            disable_mongo();
        })?;
    Ok(result.deleted_count > 0)
}

pub async fn list_favorites(
    scope_type: &str,
    scope_id: i64,
    limit: i64,
) -> Result<Vec<MusicFavorite>, BotError> {
    if is_mongo_disabled() {
        return Ok(Vec::new());
    }
    let options = FindOptions::builder()
        .sort(doc! {"created_at": -1})
        .limit(limit.max(1))
        .build();
    async_collection()
        .await
        .find(doc! {"scope_type": scope_type, "scope_id": scope_id})
        .with_options(options)
        .await?
        .try_collect()
        .await
        .inspect_err(|_| {
            disable_mongo();
        })
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn favorite_key_uses_scope_and_track() {
        let key = MusicFavorite::key(FAVORITE_SCOPE_USER, 1, "soda", "739105056071");
        assert_eq!(key.get_str("scope_type").unwrap(), FAVORITE_SCOPE_USER);
        assert_eq!(key.get_i64("scope_id").unwrap(), 1);
        assert_eq!(key.get_str("platform").unwrap(), "soda");
        assert_eq!(key.get_str("track_id").unwrap(), "739105056071");
    }
}
