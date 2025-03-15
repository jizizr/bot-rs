pub mod analysis;
use super::*;
use async_once::AsyncOnce;
use mongodb::{Client, Database, bson::doc};

lazy_static! {
    static ref DB: AsyncOnce<Database> = AsyncOnce::new(init_mongo());
}

#[macro_export]
macro_rules! index_builder {
    // 基本情况：空列表
    ($collection:expr, ) => {};

    // 处理联合索引
    ($collection:expr, ($($field:expr),+)) => {
        {
            $collection
                .create_index(
                    mongodb::IndexModel::builder()
                    .keys(mongodb::bson::doc! { $($field: 1),+ })
                    .build()
                )
                .await
                .expect("Failed to create composite index");
        }
    };

    // 处理单索引
    ($collection:expr, $field:literal) => {
        {
            $collection
                .create_index(mongodb::IndexModel::builder().keys(mongodb::bson::doc! { $field: 1 }).build())
                .await
                .expect(&format!("Failed to create index for {}", $field));
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
