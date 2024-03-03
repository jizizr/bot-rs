use super::*;

impl GroupFuncSwitch {
    pub fn new() -> Self {
        let mut gs = Self {
            map: DashMap::new(),
            template: DashMap::new(),
            pstorer: Arc::new(Sled::new("data/func_switch")),
        };
        gs.map = gs.pstorer.load();
        gs
    }

    pub fn update_template(&self, func_name: &str, func_desc: &str) {
        self.template
            .insert(func_name.to_string(), func_desc.to_string());
    }

    pub async fn update_status(&self, group_id: i64, func_name: String, status: bool) {
        self.map.get(&group_id).unwrap().insert(func_name, status);
        self.pstorer
            .insert(group_id, self.map.get(&group_id).unwrap().clone())
            .await;
    }

    fn init(&self, group_id: i64) {
        self.map.insert(group_id, {
            let init_map: DashMap<String, bool> = DashMap::new();

            // 遍历 A 的键，并将它们添加到 B 中，值设置为 true
            self.template.iter().for_each(|x| {
                init_map.insert(x.key().to_string(), true);
            });
            init_map
        });
    }

    pub fn get_status(&self, group_id: i64, func_name: String) -> bool {
        if let Some(func_switch) = self.map.get(&group_id) {
            match func_switch.get(&func_name) {
                Some(status) => return *status,
                None => {
                    func_switch.insert(func_name, true);
                    return true;
                }
            }
        }
        self.init(group_id);
        let pstorer = self.pstorer.clone();
        let map = self.map.get(&group_id).unwrap().clone();
        tokio::spawn(async move {
            pstorer.insert(group_id, map).await;
        });
        true
    }

    pub fn get_template(&self) -> &DashMap<String, String> {
        &self.template
    }
}
