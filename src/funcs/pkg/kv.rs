use std::sync::Arc;

use dashmap::DashMap;
use pstore::Sled;
mod core;
mod pstore;
type FuncSwitch = DashMap<String, bool>;

pub struct GroupFuncSwitch {
    map: DashMap<i64, FuncSwitch>,
    template: DashMap<String, String>,
    pub pstorer: Arc<Sled>,
}
