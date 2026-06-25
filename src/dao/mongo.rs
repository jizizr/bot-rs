pub mod analysis;
pub mod freq;
pub mod music_favorites;
pub mod music_settings;

use super::*;
use bson::doc;
use mongodb::{Client, Collection, Database};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::OnceCell;

static MONGO_DISABLED: AtomicBool = AtomicBool::new(false);

pub(super) fn is_mongo_disabled() -> bool {
    if MONGO_DISABLED.load(Ordering::Relaxed) {
        return true;
    }
    matches!(
        std::env::var("MONGO_DISABLE"),
        Ok(v) if v == "1" || v.eq_ignore_ascii_case("true")
    )
}

pub(super) fn disable_mongo() {
    MONGO_DISABLED.store(true, Ordering::Relaxed);
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
