use super::*;
use crate::{
    BOT, BotResult,
    dao::{
        mysql::wordcloud::{active_group, clear},
        rdb::wordcloud::*,
    },
    funcs::text::SWITCH,
};
use futures::{StreamExt, stream::FuturesUnordered};

pub const WORDCLOUD_SWITCH: &str = "scheduled_wordcloud";
pub const USER_FREQ_SWITCH: &str = "scheduled_user_freq";

pub async fn wcloud() -> Result<(), Vec<BotError>> {
    let mut err_vec = Vec::new();
    let mut futures = FuturesUnordered::new();
    for group in active_group().await.map_err(|e| vec![e.into()])? {
        futures.push(tokio::spawn(wcloud_single(group)));
    }

    while let Some(result) = futures.next().await {
        match result {
            Ok(e) => {
                err_vec.extend(e);
            }
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

async fn wcloud_single(group: i64) -> Vec<BotError> {
    let mut err_vec: Vec<BotError> = Vec::new();
    let wordcloud_enabled = SWITCH.get_status(group, WORDCLOUD_SWITCH.to_string());
    let user_freq_enabled = SWITCH.get_status(group, USER_FREQ_SWITCH.to_string());
    if !wordcloud_enabled && !user_freq_enabled {
        return err_vec;
    }
    let flag = get_flag(group).await.unwrap_or_else(|e| {
        err_vec.push(e);
        true
    });
    if !flag {
        return err_vec;
    }
    err_vec.extend(
        send_reports(
            wordcloud_enabled,
            user_freq_enabled,
            generate::wcloud(&BOT, group),
            generate::user_freq(&BOT, group),
        )
        .await,
    );
    // 只在启用的推送均成功后消费活跃标记，支持仅发送发言统计。
    if err_vec.is_empty() {
        wc_switch(group, false)
            .await
            .unwrap_or_else(|e| err_vec.push(e));
    }
    err_vec
}

async fn send_reports(
    wordcloud_enabled: bool,
    user_freq_enabled: bool,
    wordcloud: impl std::future::Future<Output = BotResult>,
    user_freq: impl std::future::Future<Output = BotResult>,
) -> Vec<BotError> {
    let mut errors = Vec::new();
    if wordcloud_enabled && let Err(e) = wordcloud.await {
        errors.push(e);
    }
    if user_freq_enabled && let Err(e) = user_freq.await {
        errors.push(e);
    }
    errors
}

pub async fn wcloud_then_clear() -> Result<(), Vec<BotError>> {
    let mut err_vec = match wcloud().await {
        Ok(_) => {
            vec![]
        }
        Err(e) => e,
    };
    clear().await.unwrap_or_else(|e| err_vec.push(e));
    if err_vec.is_empty() {
        Ok(())
    } else {
        Err(err_vec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[tokio::test]
    async fn scheduled_switches_gate_each_report_independently() {
        for (wordcloud_enabled, user_freq_enabled) in
            [(false, false), (false, true), (true, false), (true, true)]
        {
            let wordcloud_sent = Cell::new(false);
            let user_freq_sent = Cell::new(false);
            let errors = send_reports(
                wordcloud_enabled,
                user_freq_enabled,
                async {
                    wordcloud_sent.set(true);
                    Ok(())
                },
                async {
                    user_freq_sent.set(true);
                    Ok(())
                },
            )
            .await;
            assert!(errors.is_empty());
            assert_eq!(wordcloud_sent.get(), wordcloud_enabled);
            assert_eq!(user_freq_sent.get(), user_freq_enabled);
        }
    }

    #[tokio::test]
    async fn failed_wordcloud_does_not_skip_enabled_user_stats() {
        let user_freq_sent = Cell::new(false);
        let errors = send_reports(
            true,
            true,
            async { Err(std::io::Error::other("send failed").into()) },
            async {
                user_freq_sent.set(true);
                Ok(())
            },
        )
        .await;
        assert_eq!(errors.len(), 1);
        assert!(user_freq_sent.get());
    }
}
