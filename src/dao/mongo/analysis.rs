use mongodb::bson;

use crate::{analysis::model::BotLog, index_builder};

use super::*;

pub(super) async fn create_index(db: &mongodb::Database) {
    let logs_collection = db.collection::<bson::Document>("logs");

    index_builder!(
        logs_collection,
        "timestamp",
        "user_id",
        ("group_id", "user_id", "timestamp"),
        "msg_ctx.status"
    );
}

pub async fn insert_log(log: &BotLog) -> Result<(), mongodb::error::Error> {
    let logs_collection = DB.get().await.collection::<bson::Document>("logs");
    logs_collection.insert_one(bson::to_document(log)?).await?;
    Ok(())
}
