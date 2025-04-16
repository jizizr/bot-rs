use super::*;
use crate::analysis::model::MessageCount;
use futures::stream::{StreamExt, TryStreamExt};
use std::collections::HashMap;

pub async fn query_data(gid: i64, uid: u64) -> Result<HashMap<String, HashMap<u8, f32>>, BotError> {
    let pipeline = vec![
        doc! {
            "$match": {
                "group_id": bson::Bson::Int64(gid)
            }
        },
        doc! {
            "$facet": {
                "userStats": [
                    {
                        "$match": {
                            "user_id": bson::Bson::Int64(uid as i64)
                        }
                    },
                    {
                        "$group": {
                            "_id": {
                                "user_id": "$user_id",
                                "group_id": "$group_id",
                                "hour_num": {
                                    "$hour": {
                                        "date": "$timestamp",
                                        "timezone": "Asia/Shanghai"
                                    }
                                }
                            },
                            "count": { "$sum": 1 }
                        }
                    },
                    {
                        "$project": {
                            "_id": 0,
                            "user_id": "$_id.user_id",
                            "group_id": "$_id.group_id",
                            "hour_num": "$_id.hour_num",
                            "count": "$count"
                        }
                    }
                ],
                "groupStats": [
                    {
                        "$group": {
                            "_id": {
                                "group_id": "$group_id",
                                "hour_num": {
                                    "$hour": {
                                        "date": "$timestamp",
                                        "timezone": "Asia/Shanghai"
                                    }
                                }
                            },
                            "count": { "$sum": 1 }
                        }
                    },
                    {
                        "$project": {
                            "_id": 0,
                            "user_id": null,
                            "group_id": "$_id.group_id",
                            "hour_num": "$_id.hour_num",
                            "count": "$count"
                        }
                    }
                ]
            }
        },
        doc! {
            "$project": {
                "combined": {
                    "$concatArrays": ["$userStats", "$groupStats"]
                }
            }
        },
        doc! {
            "$unwind": "$combined"
        },
        doc! {
            "$replaceRoot": {
                "newRoot": "$combined"
            }
        },
        doc! {
            "$lookup": {
                "from": "groups",
                "let": { "let_group_id___1": "$group_id" },
                "pipeline": [
                    { "$match": { "$expr": { "$eq": ["$$let_group_id___1", "$group_id"] } } }
                ],
                "as": "join_alias_Groups"
            }
        },
        doc! {
            "$unwind": {
                "path": "$join_alias_Groups",
                "preserveNullAndEmptyArrays": true
            }
        },
        doc! {
            "$lookup": {
                "from": "users",
                "let": { "let_user_id___2": "$user_id" },
                "pipeline": [
                    { "$match": { "$expr": { "$eq": ["$$let_user_id___2", "$user_id"] } } }
                ],
                "as": "join_alias_Users"
            }
        },
        doc! {
            "$unwind": {
                "path": "$join_alias_Users",
                "preserveNullAndEmptyArrays": true
            }
        },
        doc! {
            "$project": {
                "_id": false,
                "user_id": "$user_id",
                "group_id": "$group_id",
                "hour_num": "$hour_num",
                "count": "$count",
                "group_username": "$join_alias_Groups.group_username",
                "group_name": "$join_alias_Groups.group_name",
                "username": "$join_alias_Users.username"
            }
        },
    ];

    let cursor = BOTLOG.get().await.aggregate(pipeline).await?;
    let results: Vec<MessageCount> = cursor
        .map(|result| {
            result.and_then(|doc| {
                mongodb::bson::from_document(doc).map_err(mongodb::error::Error::from)
            })
        })
        .try_collect()
        .await?;

    // 4. 处理结果
    let mut datas: HashMap<String, HashMap<u8, f32>> = HashMap::new();
    for result in results.into_iter() {
        let username = result.username.unwrap_or_else(|| {
            result.user_id.map(|id| id.to_string()).unwrap_or_else(|| {
                result
                    .group_name
                    .unwrap_or_else(|| result.group_username.unwrap_or(result.group_id.to_string()))
            })
        });
        let hour_num = result.hour_num as u8;
        let count = result.count as f32;
        datas.entry(username).or_default().insert(hour_num, count);
    }
    Ok(datas)
}
