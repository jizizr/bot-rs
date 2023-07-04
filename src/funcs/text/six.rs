use super::*;

pub async fn six(bot: Bot, ctx: Context) -> ferrisgram::error::Result<GroupIteration> {
    let msg = ctx.effective_message.unwrap();
    msg.reply(&bot, "6").send().await?;
    Ok(GroupIteration::EndGroups)
}
