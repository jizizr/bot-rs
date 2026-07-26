//! Sliding-window rate limiting for abusable platform-API entry points
//! (port of upstream e5d06be), plus a raw-lyrics TTL cache so lyric format
//! toggles stay free local conversions.
//!
//! Semantics:
//! - Admit-then-record: a rejected request consumes no quota in any dimension.
//! - Dimensions: per-user / per-chat / per-platform / global. A quota of 0
//!   disables that dimension; `user_id == 0` and `chat_id` of `None`/`Some(0)`
//!   skip their dimension (inline queries carry no chat).
//! - Unregistered actions fail open.
//! - Bucket cleanup is opportunistic: every `PRUNE_EVERY` admissions the whole
//!   table is swept and drained buckets are dropped.
//!
//! Quotas come from the unified `[music]` config section (see src/settings.rs),
//! all "N requests per `rate_limit_window` seconds" (window default 60):
//! search 5/12/10/20, lyric 8/20/20/40, download 4/8/15/30, collection
//! 5/10/12/25 (per-user / per-chat / per-platform / global).

use super::provider::{MusicLyrics, MusicPlatform};
use crate::{BotError, settings::SETTINGS};
use dashmap::DashMap;
use lazy_static::lazy_static;
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

/// Sweep cadence for the shared bucket table.
const PRUNE_EVERY: usize = 256;
/// Raw-lyrics cache TTL (30 min): within it, 简/繁 toggles re-use the raw
/// provider payload instead of re-hitting the platform API.
const LYRIC_CACHE_TTL: Duration = Duration::from_secs(30 * 60);
/// Sweep cadence for the lyric cache.
const LYRIC_PRUNE_EVERY: usize = 64;

lazy_static! {
    /// Global limiter, configured from the unified `[music]` config section.
    pub static ref LIMITER: ResourceRateLimiter = ResourceRateLimiter::from_settings();
    static ref LYRIC_CACHE: DashMap<(MusicPlatform, String), CachedLyrics> = DashMap::new();
}

static LYRIC_STORES: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ResourceAction {
    Search,
    /// inline 查询随输入逐字触发，配额独立于命令搜索且默认更宽松
    InlineSearch,
    Download,
    Lyric,
    Collection,
}

/// Per-action quota: at most N admitted requests per `window`, per dimension.
#[derive(Clone, Copy, Debug)]
pub struct ResourceQuota {
    pub window: Duration,
    pub per_user: usize,
    pub per_chat: usize,
    pub per_platform: usize,
    pub global: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
enum Dim {
    Global,
    User(i64),
    Chat(i64),
    Platform(&'static str),
}

/// 桶内一条准入记录：时间戳用于滑窗剔除，递增 id 用于精确退款
/// （`Instant` 在同一平台上可能出现相同值，不能作为唯一标识）。
#[derive(Clone, Copy)]
struct Admission {
    at: Instant,
    id: u64,
}

pub struct ResourceRateLimiter {
    rules: HashMap<ResourceAction, ResourceQuota>,
    buckets: Mutex<HashMap<(ResourceAction, Dim), VecDeque<Admission>>>,
    admissions: AtomicUsize,
    next_admission_id: AtomicU64,
}

fn prune(deque: &mut VecDeque<Admission>, window: Duration, now: Instant) {
    while let Some(front) = deque.front() {
        // duration_since saturates to zero for not-yet-reached instants.
        if now.duration_since(front.at) >= window {
            deque.pop_front();
        } else {
            break;
        }
    }
}

impl ResourceRateLimiter {
    pub fn new(rules: HashMap<ResourceAction, ResourceQuota>) -> Self {
        Self {
            rules,
            buckets: Mutex::new(HashMap::new()),
            admissions: AtomicUsize::new(0),
            next_admission_id: AtomicU64::new(0),
        }
    }

    fn from_settings() -> Self {
        let music = &SETTINGS.music;
        let window = Duration::from_secs(music.rate_limit_window.max(1));
        let quota = |per_user, per_chat, per_platform, global| ResourceQuota {
            window,
            per_user,
            per_chat,
            per_platform,
            global,
        };
        let mut rules = HashMap::new();
        rules.insert(
            ResourceAction::Search,
            quota(
                music.search_rate_per_user,
                music.search_rate_per_chat,
                music.search_rate_per_platform,
                music.search_rate_global,
            ),
        );
        rules.insert(
            ResourceAction::Lyric,
            quota(
                music.lyric_rate_per_user,
                music.lyric_rate_per_chat,
                music.lyric_rate_per_platform,
                music.lyric_rate_global,
            ),
        );
        rules.insert(
            ResourceAction::Download,
            quota(
                music.download_rate_per_user,
                music.download_rate_per_chat,
                music.download_rate_per_platform,
                music.download_rate_global,
            ),
        );
        rules.insert(
            ResourceAction::Collection,
            quota(
                music.collection_rate_per_user,
                music.collection_rate_per_chat,
                music.collection_rate_per_platform,
                music.collection_rate_global,
            ),
        );
        rules.insert(
            ResourceAction::InlineSearch,
            quota(
                music.inline_search_rate_per_user,
                0,
                music.inline_search_rate_per_platform,
                music.inline_search_rate_global,
            ),
        );
        Self::new(rules)
    }

    pub fn allow(
        &self,
        action: ResourceAction,
        user_id: i64,
        chat_id: Option<i64>,
        platform: MusicPlatform,
    ) -> bool {
        self.try_admit_at(action, user_id, chat_id, platform, Instant::now())
            .is_some()
    }

    /// 与 `allow` 相同的准入检查，但返回可用于精确退款的凭证。
    pub fn try_admit(
        &self,
        action: ResourceAction,
        user_id: i64,
        chat_id: Option<i64>,
        platform: MusicPlatform,
    ) -> Option<AdmissionToken> {
        self.try_admit_at(action, user_id, chat_id, platform, Instant::now())
            .map(|(dims, id)| AdmissionToken { action, dims, id })
    }

    /// 退还一次已记录的额度：额度已记录但后续步骤（如下载槽排队）被拒、
    /// 实际未发生资源消耗时调用。按凭证里的唯一 admission id 从对应桶中
    /// 精确删除本次请求的记录，并发下也不会误删其他请求的记录。
    pub fn refund(&self, token: AdmissionToken) {
        let mut buckets = self.buckets.lock().expect("resource rate limiter lock");
        for dim in token.dims {
            if let Some(deque) = buckets.get_mut(&(token.action, dim))
                && let Some(pos) = deque.iter().rposition(|entry| entry.id == token.id)
            {
                deque.remove(pos);
            }
        }
    }

    #[cfg(test)]
    fn allow_at(
        &self,
        action: ResourceAction,
        user_id: i64,
        chat_id: Option<i64>,
        platform: MusicPlatform,
        now: Instant,
    ) -> bool {
        self.try_admit_at(action, user_id, chat_id, platform, now)
            .is_some()
    }

    /// 准入成功时返回被记录的维度列表（供退款用），失败返回 `None`。
    fn try_admit_at(
        &self,
        action: ResourceAction,
        user_id: i64,
        chat_id: Option<i64>,
        platform: MusicPlatform,
        now: Instant,
    ) -> Option<(Vec<Dim>, u64)> {
        // Unregistered actions fail open (nothing recorded, so refund is a no-op).
        let Some(quota) = self.rules.get(&action) else {
            return Some((Vec::new(), 0));
        };
        let mut dims: Vec<(Dim, usize)> = Vec::with_capacity(4);
        if quota.global > 0 {
            dims.push((Dim::Global, quota.global));
        }
        if quota.per_platform > 0 {
            dims.push((Dim::Platform(platform.id()), quota.per_platform));
        }
        if quota.per_user > 0 && user_id != 0 {
            dims.push((Dim::User(user_id), quota.per_user));
        }
        if quota.per_chat > 0
            && let Some(chat_id) = chat_id.filter(|chat_id| *chat_id != 0)
        {
            dims.push((Dim::Chat(chat_id), quota.per_chat));
        }

        let mut buckets = self.buckets.lock().expect("resource rate limiter lock");
        // Admit-then-record: check every dimension before touching any quota.
        for (dim, cap) in &dims {
            if let Some(deque) = buckets.get_mut(&(action, *dim)) {
                prune(deque, quota.window, now);
                if deque.len() >= *cap {
                    return None;
                }
            }
        }
        let id = self.next_admission_id.fetch_add(1, Ordering::Relaxed);
        for (dim, _) in &dims {
            buckets
                .entry((action, *dim))
                .or_default()
                .push_back(Admission { at: now, id });
        }

        // Opportunistic sweep so idle user/chat buckets do not accumulate.
        if self.admissions.fetch_add(1, Ordering::Relaxed) % PRUNE_EVERY == PRUNE_EVERY - 1 {
            buckets.retain(|(bucket_action, _), deque| {
                if let Some(rule) = self.rules.get(bucket_action) {
                    prune(deque, rule.window, now);
                }
                !deque.is_empty()
            });
        }
        Some((dims.into_iter().map(|(dim, _)| dim).collect(), id))
    }
}

/// 一次已记录的限流准入凭证：内含维度与唯一 admission id，供后续精确退款。
pub struct AdmissionToken {
    action: ResourceAction,
    dims: Vec<Dim>,
    id: u64,
}

/// Admission helper for call sites: `Ok(())` or the Chinese throttle error to
/// surface via the flow's existing error path.
pub fn ensure_allowed(
    action: ResourceAction,
    user_id: i64,
    chat_id: Option<i64>,
    platform: MusicPlatform,
) -> Result<(), BotError> {
    if LIMITER.allow(action, user_id, chat_id, platform) {
        Ok(())
    } else {
        Err(BotError::Busy("请求过于频繁，请稍后再试".to_string()))
    }
}

/// 带退款凭证的准入检查：下载流程在额度记录后若排队被拒，可用凭证精确退还。
pub fn ensure_admitted(
    action: ResourceAction,
    user_id: i64,
    chat_id: Option<i64>,
    platform: MusicPlatform,
) -> Result<AdmissionToken, BotError> {
    LIMITER
        .try_admit(action, user_id, chat_id, platform)
        .ok_or_else(|| BotError::Busy("请求过于频繁，请稍后再试".to_string()))
}

struct CachedLyrics {
    expires_at: Instant,
    lyrics: Option<MusicLyrics>,
}

/// Fresh cache hit returns the raw provider result — including `Some(None)`
/// for tracks already known to have no lyrics. `None` means miss/expired, so
/// the caller must consume lyric quota and hit the provider.
pub fn cached_lyrics(platform: MusicPlatform, track_id: &str) -> Option<Option<MusicLyrics>> {
    let key = (platform, track_id.to_string());
    let now = Instant::now();
    {
        let entry = LYRIC_CACHE.get(&key)?;
        if now < entry.expires_at {
            return Some(entry.lyrics.clone());
        }
    }
    LYRIC_CACHE.remove_if(&key, |_, cached| now >= cached.expires_at);
    None
}

/// Stores a raw provider lyric result for `LYRIC_CACHE_TTL` (30 min).
pub fn store_lyrics(platform: MusicPlatform, track_id: &str, lyrics: Option<MusicLyrics>) {
    store_lyrics_with_ttl(platform, track_id, lyrics, LYRIC_CACHE_TTL)
}

fn store_lyrics_with_ttl(
    platform: MusicPlatform,
    track_id: &str,
    lyrics: Option<MusicLyrics>,
    ttl: Duration,
) {
    let now = Instant::now();
    LYRIC_CACHE.insert(
        (platform, track_id.to_string()),
        CachedLyrics {
            expires_at: now + ttl,
            lyrics,
        },
    );
    if LYRIC_STORES.fetch_add(1, Ordering::Relaxed) % LYRIC_PRUNE_EVERY == LYRIC_PRUNE_EVERY - 1 {
        LYRIC_CACHE.retain(|_, cached| now < cached.expires_at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter_with(action: ResourceAction, quota: ResourceQuota) -> ResourceRateLimiter {
        let mut rules = HashMap::new();
        rules.insert(action, quota);
        ResourceRateLimiter::new(rules)
    }

    fn user_quota(per_user: usize) -> ResourceQuota {
        ResourceQuota {
            window: Duration::from_secs(60),
            per_user,
            per_chat: 0,
            per_platform: 0,
            global: 0,
        }
    }

    #[test]
    fn sliding_window_expires_entries() {
        let limiter = limiter_with(ResourceAction::Search, user_quota(2));
        let base = Instant::now();
        let allow = |offset_secs: u64| {
            limiter.allow_at(
                ResourceAction::Search,
                1,
                None,
                MusicPlatform::Tencent,
                base + Duration::from_secs(offset_secs),
            )
        };
        assert!(allow(0));
        assert!(allow(1));
        // Quota (2) exhausted inside the window.
        assert!(!allow(2));
        assert!(!allow(59));
        // At base+60 the base entry has aged out of the 60s window.
        assert!(allow(60));
        assert!(!allow(60));
        // At base+61 the base+1 entry expires as well.
        assert!(allow(61));
    }

    #[test]
    fn rejected_requests_consume_no_quota() {
        let limiter = limiter_with(
            ResourceAction::Download,
            ResourceQuota {
                window: Duration::from_secs(60),
                per_user: 1,
                per_chat: 0,
                per_platform: 0,
                global: 2,
            },
        );
        let base = Instant::now();
        let allow = |user_id: i64| {
            limiter.allow_at(
                ResourceAction::Download,
                user_id,
                None,
                MusicPlatform::Netease,
                base,
            )
        };
        assert!(allow(1));
        // Per-user rejection must not record into the global bucket…
        assert!(!allow(1));
        assert!(!allow(1));
        // …so a second user still fits under the global quota of 2.
        assert!(allow(2));
        // Now the global quota really is exhausted.
        assert!(!allow(3));
    }

    #[test]
    fn dimensions_are_independent() {
        let limiter = limiter_with(
            ResourceAction::Search,
            ResourceQuota {
                window: Duration::from_secs(60),
                per_user: 5,
                per_chat: 1,
                per_platform: 0,
                global: 0,
            },
        );
        let base = Instant::now();
        let allow = |user_id: i64, chat_id: Option<i64>| {
            limiter.allow_at(
                ResourceAction::Search,
                user_id,
                chat_id,
                MusicPlatform::Tencent,
                base,
            )
        };
        assert!(allow(1, Some(5)));
        // Same chat, different user: per-chat quota of 1 is spent.
        assert!(!allow(2, Some(5)));
        // Another chat is unaffected.
        assert!(allow(2, Some(6)));
    }

    #[test]
    fn anonymous_dimensions_are_skipped() {
        let limiter = limiter_with(
            ResourceAction::Search,
            ResourceQuota {
                window: Duration::from_secs(60),
                per_user: 1,
                per_chat: 1,
                per_platform: 0,
                global: 100,
            },
        );
        let base = Instant::now();
        for _ in 0..10 {
            // user 0 + no chat (inline): only the global quota applies.
            assert!(limiter.allow_at(ResourceAction::Search, 0, None, MusicPlatform::Soda, base));
            assert!(limiter.allow_at(
                ResourceAction::Search,
                0,
                Some(0),
                MusicPlatform::Soda,
                base
            ));
        }
    }

    #[test]
    fn unregistered_actions_fail_open() {
        let limiter = ResourceRateLimiter::new(HashMap::new());
        for _ in 0..100 {
            assert!(limiter.allow(ResourceAction::Lyric, 1, Some(1), MusicPlatform::Tencent));
        }
    }

    #[test]
    fn platform_dimension_is_per_platform() {
        let limiter = limiter_with(
            ResourceAction::Search,
            ResourceQuota {
                window: Duration::from_secs(60),
                per_user: 0,
                per_chat: 0,
                per_platform: 1,
                global: 0,
            },
        );
        let base = Instant::now();
        assert!(limiter.allow_at(
            ResourceAction::Search,
            1,
            None,
            MusicPlatform::Tencent,
            base
        ));
        assert!(!limiter.allow_at(
            ResourceAction::Search,
            2,
            None,
            MusicPlatform::Tencent,
            base
        ));
        assert!(limiter.allow_at(
            ResourceAction::Search,
            2,
            None,
            MusicPlatform::Netease,
            base
        ));
    }

    #[test]
    fn opportunistic_sweep_drops_stale_buckets() {
        let limiter = limiter_with(
            ResourceAction::Search,
            ResourceQuota {
                window: Duration::from_secs(1),
                per_user: 1,
                per_chat: 0,
                per_platform: 0,
                global: 0,
            },
        );
        let base = Instant::now();
        // PRUNE_EVERY admissions from distinct users, spaced beyond the
        // window, so the sweep on the last admission drops every stale bucket.
        for i in 0..PRUNE_EVERY {
            assert!(limiter.allow_at(
                ResourceAction::Search,
                i as i64 + 1,
                None,
                MusicPlatform::Kugou,
                base + Duration::from_secs(2 * i as u64),
            ));
        }
        // Only the last admission's bucket survives.
        assert_eq!(limiter.buckets.lock().unwrap().len(), 1);
    }

    #[test]
    fn lyric_cache_hits_negatives_and_ttl() {
        store_lyrics(
            MusicPlatform::Tencent,
            "ratelimit-test-hit",
            Some(MusicLyrics {
                plain: "歌词".to_string(),
                translation: String::new(),
            }),
        );
        let hit = cached_lyrics(MusicPlatform::Tencent, "ratelimit-test-hit");
        assert_eq!(hit.unwrap().unwrap().plain, "歌词");
        // The same id on another platform is a distinct key.
        assert!(cached_lyrics(MusicPlatform::Netease, "ratelimit-test-hit").is_none());

        // "No lyrics" results are cached too.
        store_lyrics(MusicPlatform::Bilibili, "ratelimit-test-none", None);
        assert!(matches!(
            cached_lyrics(MusicPlatform::Bilibili, "ratelimit-test-none"),
            Some(None)
        ));

        // An expired entry misses and is evicted.
        store_lyrics_with_ttl(
            MusicPlatform::Tencent,
            "ratelimit-test-expired",
            None,
            Duration::ZERO,
        );
        assert!(cached_lyrics(MusicPlatform::Tencent, "ratelimit-test-expired").is_none());
        assert!(
            !LYRIC_CACHE
                .contains_key(&(MusicPlatform::Tencent, "ratelimit-test-expired".to_string()))
        );
    }

    #[test]
    fn refund_removes_only_the_refunded_admission() {
        // 全局配额 2：A、B 先后准入占满；退还 A 后应恰好释放一个名额，
        // 且 B 的记录仍然存在（再准入一个成功、下一个仍被拒）。
        let limiter = limiter_with(
            ResourceAction::Download,
            ResourceQuota {
                window: Duration::from_secs(60),
                per_user: 0,
                per_chat: 0,
                per_platform: 0,
                global: 2,
            },
        );
        let token_a = limiter
            .try_admit(ResourceAction::Download, 1, None, MusicPlatform::Soda)
            .unwrap();
        let _token_b = limiter
            .try_admit(ResourceAction::Download, 2, None, MusicPlatform::Soda)
            .unwrap();
        assert!(
            limiter
                .try_admit(ResourceAction::Download, 3, None, MusicPlatform::Soda)
                .is_none()
        );

        limiter.refund(token_a);
        assert!(
            limiter
                .try_admit(ResourceAction::Download, 3, None, MusicPlatform::Soda)
                .is_some()
        );
        assert!(
            limiter
                .try_admit(ResourceAction::Download, 4, None, MusicPlatform::Soda)
                .is_none()
        );
    }
}
