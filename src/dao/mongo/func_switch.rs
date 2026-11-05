//! Per-group feature switches, one document per group:
//! `{ _id: <group id>, switches: { <feature>: <on> } }`.
//! A feature missing from a group's document is on.
use super::*;
use futures::TryStreamExt;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize)]
struct GroupSwitches {
    #[serde(rename = "_id")]
    group_id: i64,
    #[serde(default)]
    switches: HashMap<String, bool>,
}

async fn collection() -> Collection<GroupSwitches> {
    db().await.collection::<GroupSwitches>("func_switch")
}

/// Every group's stored switches.
pub async fn load_all() -> Result<Vec<(i64, HashMap<String, bool>)>, BotError> {
    if is_mongo_disabled() {
        return Err(BotError::Custom("MongoDB 暂不可用".to_string()));
    }
    let groups: Vec<GroupSwitches> = collection()
        .await
        .find(doc! {})
        .await
        .inspect_err(|_| disable_mongo())?
        .try_collect()
        .await?;
    mark_mongo_ok();
    Ok(groups
        .into_iter()
        .map(|group| (group.group_id, group.switches))
        .collect())
}

/// Stores one switch of one group, leaving its other switches as they are.
pub async fn set(group_id: i64, feature: &str, on: bool) -> Result<(), BotError> {
    if is_mongo_disabled() {
        return Err(BotError::Custom("MongoDB 暂不可用".to_string()));
    }
    collection()
        .await
        .update_one(
            doc! { "_id": group_id },
            doc! { "$set": { format!("switches.{feature}"): on } },
        )
        .upsert(true)
        .await
        .inspect_err(|_| disable_mongo())?;
    mark_mongo_ok();
    Ok(())
}
