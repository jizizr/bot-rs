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
    // this method will ensure that our text will be sent with markdown formatting.
    .parse_mode("markdown".to_string())
    // this method will disable the web page preview for out message
    .disable_web_page_preview(true)
    // You must use this send() method in order to send the request to the API
    .send()
    .await?;

    // GroupIteration::EndGroups will end iteration of groups for an update.
    // This means that rest of the pending groups and their handlers won't be checked
    // for this particular update.
    Ok(GroupIteration::EndGroups)
}
