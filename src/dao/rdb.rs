use super::*;
use redis::Client;
use std::sync::LazyLock;
use switch::SwitchType;

mod switch;
pub mod wordcloud;

static RDB: LazyLock<Client> =
    LazyLock::new(|| Client::open(&*SETTINGS.db.redis.url).expect("Redis connection failed"));
