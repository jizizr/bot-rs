use ferrisgram::error::GroupIteration;
use ferrisgram::ext::handlers::CommandHandler;
use ferrisgram::ext::{Context, Dispatcher};
use ferrisgram::Bot;
use std::collections::HashMap;
use std::future::Future;

pub struct CommandInfo(HashMap<String, String>);

impl CommandInfo {
    pub fn new() -> Self {
        CommandInfo(HashMap::new())
    }

    pub fn add_handler<F>(
        &mut self,
        dispatcher: &mut Dispatcher<'_>,
        command: &'static str,
        callback: fn(Bot, Context) -> F,
        comment: &str,
    ) where
        F: Future<Output = ferrisgram::error::Result<GroupIteration>> + Send + 'static,
    {
        self.0.insert(command.to_string(), comment.to_string());
        dispatcher.add_handler(CommandHandler::new(command, callback));
    }

    pub fn get_command(&self) -> &HashMap<String, String> {
        &self.0
    }
}
