use super::gen;
use crate::dao::mysql::wordcloud::active_group;
use bot_rs::BOT;
pub async fn wcloud() -> Result<(), Vec<Box<dyn std::error::Error + Send + Sync>>> {
    let mut err_vec: Vec<Box<dyn std::error::Error + Send + Sync>> = vec![];
    for group in active_group().await.map_err(|e| vec![e.into()])? {
        match gen::wcloud(&BOT, group).await {
            Ok(_) => {}
            Err(e) => {
                err_vec.push(e.into());
            }
        }
    }
    if err_vec.is_empty() {
        Ok(())
    } else {
        Err(err_vec)
    }
}
