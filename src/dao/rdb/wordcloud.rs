use crate::AppError;

use super::*;

pub async fn wc_switch(group_id: i64, flag: bool) -> BotResult {
    rdb::switch::change_flag(group_id, SwitchType::WordCloud, flag).await
}

pub async fn get_flag(group_id: i64) -> Result<bool, AppError> {
    rdb::switch::get_flag(group_id, SwitchType::WordCloud).await
}
