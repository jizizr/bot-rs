use super::*;

pub async fn start(bot: Bot, ctx: Context) -> ferrisgram::error::Result<GroupIteration> {
    // Same logic as chat applies on unwrapping effective message here.
    let msg = ctx.effective_message.unwrap();
    // Ferrisgram offers some custom helpers which make your work easy
    // Here we have used one of those helpers known as msg.reply
    msg.reply(
        &bot,
        "Hey! I am an echo bot built using [Ferrisgram](https://github.com/ferrisgram/ferrisgram).
I will repeat your messages.",
    )
    .parse_mode("markdown".to_string())
    .disable_web_page_preview(true)
    .send()
    .await?;
    // GroupIteration::EndGroups will end iteration of groups for an update.
    // This means that rest of the pending groups and their handlers won't be checked
    // for this particular update.
    Ok(GroupIteration::EndGroups)
}
