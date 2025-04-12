use crate::{
    analysis::model::{BotLog, Group, User},
    index_builder,
};

use super::*;

pub(super) async fn create_index(db: &mongodb::Database) {
    let logs_collection = db.collection::<bson::Document>("logs");
    let group_collection = db.collection::<bson::Document>("groups");
    let user_collection = db.collection::<bson::Document>("users");

    index_builder!(
        logs_collection,
        "timestamp",
        "user_id",
        ("group_id", "user_id", "timestamp"),
        "msg_ctx.status"
    );

    index_builder!(group_collection, "group_id");
    index_builder!(user_collection, "user_id");
}

pub async fn insert_log(log: (&BotLog, &User, &Group)) -> Result<(), mongodb::error::Error> {
    let (botlog, user, group) = log;

    BOTLOG
        .get()
        .await
        .insert_one(bson::to_document(botlog)?)
        .await?;
    USER.get()
        .await
        .replace_one(
            doc! {"user_id": user.get_id() as i64},
            bson::to_document(user)?,
        )
        .upsert(true)
        .await?;
    GROUP
        .get()
        .await
        .replace_one(doc! {"group_id": group.get_id()}, bson::to_document(group)?)
        .upsert(true)
        .await?;
    Ok(())
}
