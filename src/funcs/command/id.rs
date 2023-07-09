use super::*;

pub async fn id(bot: Bot, ctx: Context) -> FResult<GroupIteration> {
    let msg = ctx.effective_message.unwrap();
    msg.reply(
        &bot,
        &format!("您的id是 `{}`", ctx.effective_user.unwrap().id),
    )
    .parse_mode("markdown".to_string())
    .send()
    .await?;
    Ok(GroupIteration::EndGroups)
}
