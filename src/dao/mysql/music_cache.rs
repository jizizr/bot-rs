use crate::{BotResult, settings};
use sea_orm::{
    ColumnTrait, ConnectionTrait, Database, DatabaseConnection, EntityTrait, QueryFilter, Schema,
    Set, Statement,
    sea_query::{Index, OnConflict},
};
use tokio::sync::OnceCell;

static MUSIC_CACHE_DB: OnceCell<DatabaseConnection> = OnceCell::const_new();
static MUSIC_CACHE_TABLE: OnceCell<()> = OnceCell::const_new();

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicCache {
    pub platform: String,
    pub track_id: String,
    pub quality: String,
    pub song: String,
    pub singer: String,
    pub album: String,
    pub link: String,
    pub file_ext: String,
    pub music_size: u64,
    pub bitrate: u32,
    pub duration: Option<u32>,
    pub file_id: String,
    pub thumb_file_id: String,
    pub from_user_id: i64,
    pub from_chat_id: i64,
}

mod entity {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, DeriveEntityModel, Eq, PartialEq)]
    #[sea_orm(table_name = "music_cache")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i64,
        #[sea_orm(column_type = "String(StringLen::N(32))")]
        pub platform: String,
        #[sea_orm(column_type = "String(StringLen::N(191))")]
        pub track_id: String,
        #[sea_orm(column_type = "String(StringLen::N(32))")]
        pub quality: String,
        #[sea_orm(column_type = "String(StringLen::N(512))")]
        pub song: String,
        #[sea_orm(column_type = "String(StringLen::N(512))")]
        pub singer: String,
        #[sea_orm(column_type = "String(StringLen::N(512))")]
        pub album: String,
        #[sea_orm(column_type = "Text")]
        pub link: String,
        #[sea_orm(column_type = "String(StringLen::N(32))")]
        pub file_ext: String,
        pub music_size: i64,
        pub bitrate: i32,
        pub duration: Option<i32>,
        #[sea_orm(column_type = "String(StringLen::N(512))")]
        pub file_id: String,
        #[sea_orm(column_type = "String(StringLen::N(512))")]
        pub thumb_file_id: String,
        pub from_user_id: i64,
        pub from_chat_id: i64,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

async fn db() -> Result<&'static DatabaseConnection, crate::BotError> {
    let db = MUSIC_CACHE_DB
        .get_or_try_init(|| async {
            Database::connect(settings::SETTINGS.db.mysql.url.as_str()).await
        })
        .await?;
    ensure_table(db).await?;
    Ok(db)
}

async fn ensure_table(db: &DatabaseConnection) -> BotResult {
    MUSIC_CACHE_TABLE
        .get_or_try_init(|| async {
            let backend = db.get_database_backend();
            let schema = Schema::new(backend);
            let create_table = schema
                .create_table_from_entity(entity::Entity)
                .if_not_exists()
                .to_owned();
            db.execute(backend.build(&create_table)).await?;

            ensure_index(
                db,
                "uniq_music_cache_track",
                Index::create()
                    .name("uniq_music_cache_track")
                    .table(entity::Entity)
                    .col(entity::Column::Platform)
                    .col(entity::Column::TrackId)
                    .col(entity::Column::Quality)
                    .unique()
                    .to_owned(),
            )
            .await?;
            ensure_index(
                db,
                "idx_music_cache_file_id",
                Index::create()
                    .name("idx_music_cache_file_id")
                    .table(entity::Entity)
                    .col(entity::Column::FileId)
                    .to_owned(),
            )
            .await?;
            ensure_index(
                db,
                "idx_music_cache_platform_quality",
                Index::create()
                    .name("idx_music_cache_platform_quality")
                    .table(entity::Entity)
                    .col(entity::Column::Platform)
                    .col(entity::Column::Quality)
                    .to_owned(),
            )
            .await?;

            Ok::<(), crate::BotError>(())
        })
        .await?;
    Ok(())
}

async fn ensure_index(
    db: &DatabaseConnection,
    index_name: &str,
    index: sea_orm::sea_query::IndexCreateStatement,
) -> BotResult {
    let backend = db.get_database_backend();
    let exists = db
        .query_one(Statement::from_sql_and_values(
            backend,
            "SELECT 1 FROM information_schema.statistics \
             WHERE table_schema = DATABASE() \
             AND table_name = ? \
             AND index_name = ? \
             LIMIT 1",
            ["music_cache".into(), index_name.into()],
        ))
        .await?
        .is_some();
    if !exists {
        db.execute(backend.build(&index)).await?;
    }
    Ok(())
}

pub async fn find_cache(
    platform: &str,
    track_id: &str,
    quality: &str,
) -> Result<Option<MusicCache>, crate::BotError> {
    Ok(entity::Entity::find()
        .filter(entity::Column::Platform.eq(platform))
        .filter(entity::Column::TrackId.eq(track_id))
        .filter(entity::Column::Quality.eq(quality))
        .one(db().await?)
        .await?
        .map(Into::into))
}

pub async fn upsert_cache(cache: &MusicCache) -> BotResult {
    entity::Entity::insert(to_active_model(cache))
        .on_conflict(
            OnConflict::columns([
                entity::Column::Platform,
                entity::Column::TrackId,
                entity::Column::Quality,
            ])
            .update_columns([
                entity::Column::Song,
                entity::Column::Singer,
                entity::Column::Album,
                entity::Column::Link,
                entity::Column::FileExt,
                entity::Column::MusicSize,
                entity::Column::Bitrate,
                entity::Column::Duration,
                entity::Column::FileId,
                entity::Column::ThumbFileId,
                entity::Column::FromUserId,
                entity::Column::FromChatId,
            ])
            .to_owned(),
        )
        .exec_without_returning(db().await?)
        .await?;
    Ok(())
}

pub async fn delete_cache(platform: &str, track_id: &str, quality: &str) -> BotResult {
    entity::Entity::delete_many()
        .filter(entity::Column::Platform.eq(platform))
        .filter(entity::Column::TrackId.eq(track_id))
        .filter(entity::Column::Quality.eq(quality))
        .exec(db().await?)
        .await?;
    Ok(())
}

fn to_active_model(cache: &MusicCache) -> entity::ActiveModel {
    entity::ActiveModel {
        platform: Set(cache.platform.clone()),
        track_id: Set(cache.track_id.clone()),
        quality: Set(cache.quality.clone()),
        song: Set(cache.song.clone()),
        singer: Set(cache.singer.clone()),
        album: Set(cache.album.clone()),
        link: Set(cache.link.clone()),
        file_ext: Set(cache.file_ext.clone()),
        music_size: Set(cache.music_size.min(i64::MAX as u64) as i64),
        bitrate: Set(cache.bitrate.min(i32::MAX as u32) as i32),
        duration: Set(cache
            .duration
            .map(|duration| duration.min(i32::MAX as u32) as i32)),
        file_id: Set(cache.file_id.clone()),
        thumb_file_id: Set(cache.thumb_file_id.clone()),
        from_user_id: Set(cache.from_user_id),
        from_chat_id: Set(cache.from_chat_id),
        ..Default::default()
    }
}

impl From<entity::Model> for MusicCache {
    fn from(model: entity::Model) -> Self {
        Self {
            platform: model.platform,
            track_id: model.track_id,
            quality: model.quality,
            song: model.song,
            singer: model.singer,
            album: model.album,
            link: model.link,
            file_ext: model.file_ext,
            music_size: model.music_size.max(0) as u64,
            bitrate: model.bitrate.max(0) as u32,
            duration: model.duration.map(|duration| duration.max(0) as u32),
            file_id: model.file_id,
            thumb_file_id: model.thumb_file_id,
            from_user_id: model.from_user_id,
            from_chat_id: model.from_chat_id,
        }
    }
}
