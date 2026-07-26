//! Download governor for the music flows (port of upstream e8576f9 + 824e42b).
//!
//! Two cooperating layers:
//! 1. Admission control (`admit`): caps LIVE download work (queued +
//!    downloading + uploading) globally and per-user / per-chat, so one user
//!    or chat cannot monopolize the pipeline. Acquire it at the TOP of a
//!    download flow, before cache checks — cache hits release the guard
//!    within milliseconds, so they are effectively free.
//! 2. Download slots (`acquire_slot`): a semaphore bounding concurrent
//!    downloads. Tasks over the slot count wait in a bounded queue (the
//!    `on_queued` callback fires exactly once so the caller can show a
//!    "已加入队列" notice); tasks over the queue cap are rejected outright.
//!
//! Both layers hand out RAII guards; dropping a guard releases its share.
//! `snapshot()` feeds the queue-status alert button.

use crate::{BotError, settings::SETTINGS};
use lazy_static::lazy_static;
use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::sync::{Semaphore, SemaphorePermit};

lazy_static! {
    /// Global download governor, configured from the unified `[music]` config section.
    pub static ref GOVERNOR: DownloadGovernor = DownloadGovernor::new(
        SETTINGS.music.download_concurrency,
        SETTINGS.music.download_queue_wait_limit,
        SETTINGS.music.download_queue_global_limit,
        SETTINGS.music.download_queue_per_user_limit,
        SETTINGS.music.download_queue_per_chat_limit,
    );
}

pub struct DownloadGovernor {
    /// Download slots. Config `music.download_concurrency`, default 4 (min 1).
    slots: Semaphore,
    concurrency: usize,
    /// Max tasks allowed to wait for a slot; 0 = unbounded queue.
    /// Config `music.download_queue_wait_limit`, default 20.
    wait_limit: usize,
    /// Advisory counters for the queue-status snapshot.
    waiting: AtomicUsize,
    running: AtomicUsize,
    /// Live-work cap across everyone; 0 = unlimited.
    /// Config `music.download_queue_global_limit`, default 24.
    global_limit: usize,
    /// Live-work cap per user; 0 = unlimited.
    /// Config `music.download_queue_per_user_limit`, default 2.
    per_user_limit: usize,
    /// Live-work cap per chat; 0 = unlimited.
    /// Config `music.download_queue_per_chat_limit`, default 6.
    per_chat_limit: usize,
    active: Mutex<ActiveWork>,
}

#[derive(Default)]
struct ActiveWork {
    total: usize,
    by_user: HashMap<i64, usize>,
    by_chat: HashMap<i64, usize>,
}

/// Live-work admission share. Hold it across the whole download + upload;
/// dropping it releases the global / per-user / per-chat counters.
pub struct AdmissionGuard<'a> {
    governor: &'a DownloadGovernor,
    user_id: i64,
    chat_id: Option<i64>,
}

/// One download slot. Dropping it frees the slot for the next queued task.
pub struct SlotGuard<'a> {
    governor: &'a DownloadGovernor,
    _permit: SemaphorePermit<'a>,
}

/// Point-in-time queue state for the view-queue alert.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueueSnapshot {
    /// Downloads currently holding a slot.
    pub running: usize,
    /// Tasks waiting for a slot.
    pub waiting: usize,
    /// Admitted live work (queued + downloading + uploading).
    pub active: usize,
    pub concurrency: usize,
    pub wait_limit: usize,
    pub global_limit: usize,
}

fn cap_reached(limit: usize, used: usize) -> bool {
    limit > 0 && used >= limit
}

impl DownloadGovernor {
    pub fn new(
        concurrency: usize,
        wait_limit: usize,
        global_limit: usize,
        per_user_limit: usize,
        per_chat_limit: usize,
    ) -> Self {
        let concurrency = concurrency.max(1);
        Self {
            slots: Semaphore::new(concurrency),
            concurrency,
            wait_limit,
            waiting: AtomicUsize::new(0),
            running: AtomicUsize::new(0),
            global_limit,
            per_user_limit,
            per_chat_limit,
            active: Mutex::new(ActiveWork::default()),
        }
    }

    /// Fair admission: rejects when the global, per-user, or per-chat
    /// live-work cap is already full. `user_id == 0` / `chat_id` of
    /// `None`/`Some(0)` skip their dimension (inline queries carry no chat).
    pub fn admit(
        &self,
        user_id: i64,
        chat_id: Option<i64>,
    ) -> Result<AdmissionGuard<'_>, BotError> {
        let chat_id = chat_id.filter(|chat_id| *chat_id != 0);
        let mut active = self.active.lock().expect("download governor active lock");
        if cap_reached(self.global_limit, active.total) {
            return Err(BotError::Busy("当前下载任务较多，请稍后再试".to_string()));
        }
        if user_id != 0
            && cap_reached(
                self.per_user_limit,
                active.by_user.get(&user_id).copied().unwrap_or(0),
            )
        {
            return Err(BotError::Busy("当前下载任务较多，请稍后再试".to_string()));
        }
        if let Some(chat_id) = chat_id
            && cap_reached(
                self.per_chat_limit,
                active.by_chat.get(&chat_id).copied().unwrap_or(0),
            )
        {
            return Err(BotError::Busy("当前下载任务较多，请稍后再试".to_string()));
        }

        active.total += 1;
        if user_id != 0 {
            *active.by_user.entry(user_id).or_insert(0) += 1;
        }
        if let Some(chat_id) = chat_id {
            *active.by_chat.entry(chat_id).or_insert(0) += 1;
        }
        Ok(AdmissionGuard {
            governor: self,
            user_id,
            chat_id,
        })
    }

    /// Acquires a download slot. When all slots are busy the task joins the
    /// waiting queue (awaiting `on_queued` exactly once so the caller can
    /// show a queued notice) unless the queue is already at
    /// `wait_limit`, in which case the request is rejected immediately.
    ///
    /// `on_queued` 在等待信号量之前被 await 完成，因此排队通知的编辑一定
    /// 先于同一任务后续的任何消息编辑落地，不存在覆盖竞态。
    pub async fn acquire_slot<F, Fut>(&self, on_queued: F) -> Result<SlotGuard<'_>, BotError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let permit = match self.slots.try_acquire() {
            Ok(permit) => permit,
            Err(_) => {
                if self
                    .waiting
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |waiting| {
                        (self.wait_limit == 0 || waiting < self.wait_limit).then_some(waiting + 1)
                    })
                    .is_err()
                {
                    return Err(BotError::Busy("服务器繁忙，请稍后再试".to_string()));
                }
                // RAII so the waiting counter also unwinds if the caller is cancelled.
                let _waiting = WaitingGuard(&self.waiting);
                on_queued().await;
                self.slots
                    .acquire()
                    .await
                    .map_err(|_| BotError::Busy("服务器繁忙，请稍后再试".to_string()))?
            }
        };
        self.running.fetch_add(1, Ordering::Relaxed);
        Ok(SlotGuard {
            governor: self,
            _permit: permit,
        })
    }

    pub fn snapshot(&self) -> QueueSnapshot {
        let active = self
            .active
            .lock()
            .expect("download governor active lock")
            .total;
        QueueSnapshot {
            running: self.running.load(Ordering::Relaxed),
            waiting: self.waiting.load(Ordering::Relaxed),
            active,
            concurrency: self.concurrency,
            wait_limit: self.wait_limit,
            global_limit: self.global_limit,
        }
    }
}

struct WaitingGuard<'a>(&'a AtomicUsize);

impl Drop for WaitingGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Drop for AdmissionGuard<'_> {
    fn drop(&mut self) {
        let mut active = self
            .governor
            .active
            .lock()
            .expect("download governor active lock");
        active.total = active.total.saturating_sub(1);
        // Remove drained entries so the maps do not grow without bound.
        if self.user_id != 0
            && let Some(count) = active.by_user.get_mut(&self.user_id)
        {
            *count = count.saturating_sub(1);
            if *count == 0 {
                active.by_user.remove(&self.user_id);
            }
        }
        if let Some(chat_id) = self.chat_id
            && let Some(count) = active.by_chat.get_mut(&chat_id)
        {
            *count = count.saturating_sub(1);
            if *count == 0 {
                active.by_chat.remove(&chat_id);
            }
        }
    }
}

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        self.governor.running.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[test]
    fn admission_enforces_per_user_and_global_caps() {
        let governor = DownloadGovernor::new(4, 20, 2, 1, 6);
        let first = governor.admit(1, Some(10)).unwrap();
        // Same user again: per-user cap (1).
        assert!(governor.admit(1, Some(10)).is_err());
        let second = governor.admit(2, Some(10)).unwrap();
        // Third distinct user: global cap (2).
        assert!(governor.admit(3, Some(11)).is_err());
        assert_eq!(governor.snapshot().active, 2);

        drop(first);
        drop(second);
        assert_eq!(governor.snapshot().active, 0);
        let active = governor.active.lock().unwrap();
        assert!(active.by_user.is_empty());
        assert!(active.by_chat.is_empty());
    }

    #[test]
    fn admission_enforces_per_chat_cap() {
        let governor = DownloadGovernor::new(4, 20, 10, 5, 2);
        let _a = governor.admit(1, Some(10)).unwrap();
        let _b = governor.admit(2, Some(10)).unwrap();
        assert!(governor.admit(3, Some(10)).is_err());
        // A different chat is unaffected by the first chat's cap.
        let _c = governor.admit(3, Some(11)).unwrap();
    }

    #[test]
    fn admission_skips_anonymous_dimensions() {
        // user_id 0 / chat None (inline flow) must not share one bucket.
        let governor = DownloadGovernor::new(4, 20, 10, 1, 1);
        let _a = governor.admit(0, None).unwrap();
        let _b = governor.admit(0, None).unwrap();
        let _c = governor.admit(0, Some(0)).unwrap();
        assert_eq!(governor.snapshot().active, 3);
    }

    #[test]
    fn admission_zero_caps_are_unlimited() {
        let governor = DownloadGovernor::new(4, 20, 0, 0, 0);
        let guards: Vec<_> = (0..50)
            .map(|i| governor.admit(i % 3, Some(7)).unwrap())
            .collect();
        assert_eq!(governor.snapshot().active, 50);
        drop(guards);
        assert_eq!(governor.snapshot().active, 0);
    }

    #[tokio::test]
    async fn slot_fast_path_skips_queue_notice() {
        let governor = DownloadGovernor::new(2, 5, 0, 0, 0);
        let queued = AtomicBool::new(false);
        let slot = governor
            .acquire_slot(|| async {
                queued.store(true, Ordering::SeqCst);
            })
            .await
            .unwrap();
        assert!(!queued.load(Ordering::SeqCst));
        assert_eq!(governor.snapshot().running, 1);
        drop(slot);
        assert_eq!(governor.snapshot().running, 0);
    }

    #[tokio::test]
    async fn slot_queueing_notifies_once_and_wait_limit_rejects() {
        let governor: &'static DownloadGovernor =
            Box::leak(Box::new(DownloadGovernor::new(1, 1, 0, 0, 0)));
        let first = governor.acquire_slot(|| async {}).await.unwrap();

        let queued = Arc::new(AtomicBool::new(false));
        let queued_flag = queued.clone();
        let waiter = tokio::spawn(async move {
            let _slot = governor
                .acquire_slot(move || async move { queued_flag.store(true, Ordering::SeqCst) })
                .await
                .unwrap();
        });
        while governor.snapshot().waiting == 0 {
            tokio::task::yield_now().await;
        }
        assert!(queued.load(Ordering::SeqCst));

        // Queue is at wait_limit (1): next contender is rejected without waiting.
        let rejected = governor.acquire_slot(|| async {}).await;
        assert!(rejected.is_err());
        assert_eq!(governor.snapshot().waiting, 1);

        drop(first);
        waiter.await.unwrap();
        let snapshot = governor.snapshot();
        assert_eq!(snapshot.waiting, 0);
        assert_eq!(snapshot.running, 0);
    }
}
