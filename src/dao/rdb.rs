use super::*;
use redis::{AsyncCommands, Client};
use switch::SwitchType;

mod switch;
pub mod wordcloud;

lazy_static! {
    static ref RDB: Client =
        Client::open(&*SETTINGS.db.redis.url).expect("Redis connection failed");
}
