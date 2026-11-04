use super::*;

pub async fn start(bot: &Bot, msg: &Message) -> BotResult {
    // inline 结果上方「切换平台」按钮带着 deep-link 参数打开私聊
    if getor(msg).and_then(|text| text.split_whitespace().nth(1))
        == Some(music::INLINE_PLATFORM_START)
    {
        return music::inline_platform_picker(bot, msg).await;
    }
    bot.send(SendMessage::new(msg.chat().id(), "你好！我是Allen，使用 [telers](https://github.com/Desiders/telers) Telegram Bot API 框架和 \
    [Rust](https://www.rust\\-lang\\.org/) 编程语言书写。开源地址：https://github\\.com/jizizr/bot\\-rs 。\n发送 */help* 了解我的指令！")
    .parse_mode(ParseMode::MarkdownV2).reply_parameters(reply_to(msg.message_id()))).await?;
    Ok(())
}
