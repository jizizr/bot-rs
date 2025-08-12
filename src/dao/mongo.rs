pub mod analysis;
pub mod freq;

use super::*;
use async_once::AsyncOnce;
use bson::doc;
use mongodb::{Client, Collection, Database};
lazy_static! {
    static ref DB: AsyncOnce<Database> = AsyncOnce::new(init_mongo());
    static ref BOTLOG: AsyncOnce<Collection<bson::Document>> = AsyncOnce::new(init_botlog());
    static ref GROUP: AsyncOnce<Collection<bson::Document>> = AsyncOnce::new(init_group());
    static ref USER: AsyncOnce<Collection<bson::Document>> = AsyncOnce::new(init_user());
}

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
    db
}

async fn init_botlog() -> Collection<bson::Document> {
    DB.get().await.collection::<bson::Document>("logs")
}

async fn init_group() -> Collection<bson::Document> {
    DB.get().await.collection::<bson::Document>("groups")
}
async fn init_user() -> Collection<bson::Document> {
    DB.get().await.collection::<bson::Document>("users")
}
