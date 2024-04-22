use super::*;

pub async fn test(bot: Bot, _msg: Message) -> BotResult {
    let photo = bot.get_user_profile_photos(UserId(1071410342)).await?;
    let file_id = &photo.photos[0][0].file.id;
    let f = bot.get_file(file_id).await?;
    
    println!("{}", f.path);
    Ok(())
}
