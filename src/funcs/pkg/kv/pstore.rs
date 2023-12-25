use super::*;
use sled::Db;
use std::collections::HashMap;
use tokio::{sync::RwLock, time};

struct DashLock {
    pub map: DashMap<i64, DashMap<String, bool>>,
    pub lock: RwLock<()>,
}
pub struct Sled {
    db: Db,
    queue: DashLock,
}

impl Sled {
    pub fn new(path: &str) -> Self {
        Self {
            db: sled::open(path).unwrap(),
            queue: DashLock {
                map: DashMap::new(),
                lock: RwLock::new(()),
            },
        }
    }

    pub async fn insert(&self, group_id: i64, config: DashMap<String, bool>) {
        let _ = self.queue.lock.read().await;
        self.queue.map.insert(group_id, config);
    }
    pub fn load(&self) -> DashMap<i64, FuncSwitch> {
        let group_func_switch = DashMap::new();
        for entry in self.db.iter() {
            let entry = entry.unwrap();
            let group_id = i64::from_be_bytes(entry.0.as_ref().try_into().unwrap());
            let config: HashMap<String, bool> = serde_json::from_slice(entry.1.as_ref()).unwrap();
            let config: DashMap<String, bool> = config.into_iter().collect();
            group_func_switch.insert(group_id, config);
        }
        group_func_switch
    }
    pub async fn pool(&self) -> ! {
        loop {
            {
                {
                    let _lock = self.queue.lock.write().await;
                    let key2remove = self
                        .queue
                        .map
                        .iter()
                        .map(|entry| *entry.key())
                        .collect::<Vec<i64>>();
                    {
                        for group_id in key2remove.iter() {
                            let config = self.queue.map.get(&group_id).unwrap();
                            let config_hashmap: HashMap<String, bool> = config
                                .iter()
                                .map(|x| (x.key().to_string(), *x.value()))
                                .collect();
                            let config_serialized = serde_json::to_vec(&config_hashmap).unwrap();
                            self.db
                                .insert((*group_id).to_be_bytes(), config_serialized)
                                .unwrap();
                        }

                        for group_id in key2remove {
                            self.queue.map.remove(&group_id).unwrap();
                        }
                    }
                }
                time::sleep(time::Duration::from_secs(5)).await;
            }
        }
    }
}
