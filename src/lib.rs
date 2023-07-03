use ferrisgram::error::GroupIteration;
use ferrisgram::ext::handlers::CommandHandler;
use ferrisgram::ext::{Context, Dispatcher};
use ferrisgram::Bot;
use std::future::Future;
pub struct CommandInfo(Vec<String>);
impl CommandInfo {
    pub fn new() -> Self {
        CommandInfo(Vec::new())
    }

    pub fn add_handler<F>(
        &mut self,
        dispatcher: &mut Dispatcher<'_>,
        command: &'static str,
        callback: fn(Bot, Context) -> F,
    ) where
        F: Future<Output = ferrisgram::error::Result<GroupIteration>> + Send + 'static,
    {
        self.0.push(command.to_string());
        dispatcher.add_handler(CommandHandler::new(command, callback));
    }

    pub fn get_command(&self) -> &Vec<String> {
        &self.0
    }
}
