pub mod analysis;
pub mod freq;
pub mod music_favorites;
pub mod music_settings;

use super::*;
use bson::doc;
use mongodb::{Client, Collection, Database};
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::OnceCell;

/// 失败后的冷却秒数：期间所有 mongo 调用快速降级（避免每次都等驱动超时），
/// 冷却结束后自动重试，而不是一次失败就永久停用
const MONGO_RETRY_COOLDOWN_SECS: u64 = 300;

static MONGO_FAILED_AT: AtomicU64 = AtomicU64::new(0);

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub(super) fn is_mongo_disabled() -> bool {
    let failed_at = MONGO_FAILED_AT.load(Ordering::Relaxed);
    if failed_at != 0 && now_secs().saturating_sub(failed_at) < MONGO_RETRY_COOLDOWN_SECS {
        return true;
    }
    matches!(
        std::env::var("MONGO_DISABLE"),
        Ok(v) if v == "1" || v.eq_ignore_ascii_case("true")
    )
}

pub(super) fn disable_mongo() {
    MONGO_FAILED_AT.store(now_secs(), Ordering::Relaxed);
}

pub(super) fn mark_mongo_ok() {
    MONGO_FAILED_AT.store(0, Ordering::Relaxed);
}
static DB: OnceCell<Database> = OnceCell::const_new();
static BOTLOG: OnceCell<Collection<bson::Document>> = OnceCell::const_new();
static GROUP: OnceCell<Collection<bson::Document>> = OnceCell::const_new();
static USER: OnceCell<Collection<bson::Document>> = OnceCell::const_new();

#[macro_export]
macro_rules! index_builder {
    // 基本情况：空列表
    ($collection:expr, ) => {};

    // 处理联合索引
    ($collection:expr, ($($field:expr),+)) => {
        {
            let _ = $collection
                .create_index(
                    mongodb::IndexModel::builder()
                    .keys(mongodb::bson::doc! { $($field: 1),+ })
                    .build()
                )
                .await.inspect_err(|_| {
                    eprintln!("Failed to create composite index");
                });
        }
    };

    // 处理单索引
    ($collection:expr, $field:literal) => {
        {
            let _ = $collection
                .create_index(mongodb::IndexModel::builder().keys(mongodb::bson::doc! { $field: 1 }).build())
                .await
                .inspect_err(|_|{
                    eprintln!("Failed to create index for {}", $field);
                });
        }
    };

    // 递归处理列表
    ($collection:expr, $first:tt, $($rest:tt),*) => {
        index_builder!($collection, $first);
        index_builder!($collection, $($rest),*);
    };
}

async fn init_mongo() -> Database {
    let db = Client::with_uri_str(&SETTINGS.db.mongo.url)
        .await
        .unwrap()
        .database("logs");
    analysis::create_index(&db).await;
    music_settings::create_index(&db).await;
    music_favorites::create_index(&db).await;
    db
}

async fn init_botlog() -> Collection<bson::Document> {
    db().await.collection::<bson::Document>("logs")
}

async fn init_group() -> Collection<bson::Document> {
    db().await.collection::<bson::Document>("groups")
}
async fn init_user() -> Collection<bson::Document> {
    db().await.collection::<bson::Document>("users")
}

pub(super) async fn db() -> &'static Database {
    DB.get_or_init(init_mongo).await
}

pub(super) async fn botlog() -> &'static Collection<bson::Document> {
    BOTLOG.get_or_init(init_botlog).await
}

pub(super) async fn group() -> &'static Collection<bson::Document> {
    GROUP.get_or_init(init_group).await
}

pub(super) async fn user() -> &'static Collection<bson::Document> {
    USER.get_or_init(init_user).await
}
