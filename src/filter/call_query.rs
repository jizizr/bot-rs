use super::*;
use crate::{
    BotResult,
    analysis::model::{BotLogBuilder, Group, MessageStatus, User},
    dao::mongo::analysis::insert_log,
    funcs::command::{coin, config, music, translate},
};
macro_rules! dispatch_callbacks {
    (
        $text:expr,
        $bot:expr,
        $q:expr,
        $($command:literal => $module_path:path),* $(,)?
    ) => {
            match $text {
                $(
                    $command => $module_path($bot, $q).await,
                )*
                _ => Ok(())
            }

    };
}
pub async fn call_query_handler(bot: Bot, mut q: CallbackQuery) -> BotResult {
    let Some(binding) = q.data.clone() else {
        return Ok(());
    };
    let (command, payload) = binding
        .split_once(' ')
        .map(|(command, payload)| (command, payload))
        .unwrap_or((binding.as_str(), ""));
    q.data = Some(payload.to_string());

    let mut blog = BotLogBuilder::from(&q);
    let user = User::from(&q);
    let group = Group::from(&q);
    let _ = dispatch_callbacks!(
        command,
        bot,
        q,
        "coin" => coin::coin_callback,
        "music" => music::music_callback,
        "config" => config::config_callback,
        "trans" => translate::translate_callback,
    )
    .inspect_err(|e| {
        blog.set_error(e.to_string());
        blog.set_status(MessageStatus::RunError);
    });
    blog.set_command(binding);
    let _ = insert_log((&blog.into(), &user, &group)).await;
    Ok(())
}
