use super::*;

pub async fn start(bot: Bot, ctx: Context) -> ferrisgram::error::Result<GroupIteration> {
    let msg = ctx.effective_message.unwrap();
    msg.reply(
        &bot,
        "你好！我是Allen，使用 [Ferrisgram](https://github.com/ferrisgram/ferrisgram) Telegram Bot API 包装器和 \
         [Rust](https://www.rust-lang.org/) 编程语言书写。开源地址：https://github.com/jizizr/bot-rs 。\n发送 */help* 了解我的指令！",
    )
    .parse_mode("markdown".to_string())
    .disable_web_page_preview(true)
    .send()
    .await?;
    Ok(GroupIteration::EndGroups)
}
