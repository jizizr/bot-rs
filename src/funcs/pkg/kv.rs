use crate::dao::mongo::func_switch;
use dashmap::DashMap;

type FuncSwitch = DashMap<String, bool>;

/// Which features each group has turned off, kept in memory and stored in
/// MongoDB. A feature a group never set is on.
#[derive(Default)]
pub struct GroupFuncSwitch {
    map: DashMap<i64, FuncSwitch>,
    template: DashMap<String, String>,
}

impl GroupFuncSwitch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads every group's switches from MongoDB. A switch changed since
    /// start-up is kept over the stored one.
    pub async fn load(&self) -> Result<usize, crate::BotError> {
        let groups = func_switch::load_all().await?;
        let count = groups.len();
        for (group_id, switches) in groups {
            let funcs = self.map.entry(group_id).or_default();
            for (func_name, status) in switches {
                funcs.entry(func_name).or_insert(status);
            }
        }
        Ok(count)
    }

    pub fn update_template(&self, func_name: &str, func_desc: &str) {
        self.template
            .insert(func_name.to_string(), func_desc.to_string());
    }

    pub async fn update_status(&self, group_id: i64, func_name: String, status: bool) {
        self.map
            .entry(group_id)
            .or_default()
            .insert(func_name.clone(), status);
        if let Err(err) = func_switch::set(group_id, &func_name, status).await {
            log::error!("Couldn't store switch {func_name}={status} of group {group_id}: {err}");
        }
    }

    pub fn get_status(&self, group_id: i64, func_name: String) -> bool {
        self.map
            .get(&group_id)
            .and_then(|funcs| funcs.get(&func_name).map(|status| *status))
            .unwrap_or(true)
    }

    pub fn get_template(&self) -> &DashMap<String, String> {
        &self.template
    }
}
