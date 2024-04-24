use super::*;

pub async fn test(_bot: Bot, _msg: Message) -> BotResult {
    #[cfg(debug_assertions)]
    {}
    Ok(())
}
