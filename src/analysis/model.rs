use mongodb::bson::DateTime;
use serde::Serialize;
use serde_repr::Serialize_repr;
use teloxide::types::{CallbackQuery, Message};

use crate::getor;

#[derive(Debug, Serialize_repr)]
#[repr(u8)]
pub enum MessageType {
    Command = 0,
    Text = 1,
    Callback = 2,
}

#[derive(Debug, Serialize_repr)]
#[repr(u8)]
pub enum MessageStatus {
    Success = 0,
    CmdError = 1,
    RunError = 2,
}

#[derive(Debug, Serialize)]
pub struct BotLog {
    group_id: i64,
    user_id: u64,
    username: Option<String>,
    group_username: Option<String>,
    group_name: Option<String>,
    timestamp: DateTime,
    msg_type: MessageType,
    msg_ctx: MessageContext,
    error: Option<String>,
}

pub struct BotLogBuilder(BotLog);

#[derive(Debug, Serialize)]
pub struct MessageContext {
    message_id: i32,
    command: Option<String>,
    status: MessageStatus,
    time_cost: i64,
}

impl MessageContext {
    pub fn new(message_id: i32) -> Self {
        Self {
            message_id,
            command: None,
            status: MessageStatus::Success,
            time_cost: 0,
        }
    }
}

impl BotLogBuilder {
    pub fn set_status(&mut self, status: MessageStatus) {
        self.0.msg_ctx.status = status;
    }
    pub fn set_command(&mut self, command: String) {
        self.0.msg_ctx.command = Some(command);
    }
    pub fn set_error(&mut self, error: String) {
        self.0.error = Some(error);
    }
}

impl From<&Message> for BotLogBuilder {
    fn from(msg: &Message) -> Self {
        let mut bl = BotLog {
            group_id: msg.chat.id.0,
            group_username: msg.chat.username().map(|s| s.to_string()),
            group_name: msg.chat.title().map(|s| s.to_string()),
            user_id: msg.from.as_ref().unwrap().id.0,
            username: msg.from.as_ref().unwrap().username.clone(),
            timestamp: DateTime::now(),
            msg_type: MessageType::Text,
            msg_ctx: MessageContext::new(msg.id.0),
            error: None,
        };
        if getor(msg).unwrap().starts_with("/") {
            bl.msg_type = MessageType::Command;
        }
        Self(bl)
    }
}

impl From<&CallbackQuery> for BotLogBuilder {
    fn from(callback_query: &CallbackQuery) -> Self {
        let bl = BotLog {
            group_id: match &callback_query.message {
                Some(msg) => msg.chat().id.0,
                None => 0,
            },
            group_name: match &callback_query.message {
                Some(msg) => msg.chat().title().map(|s| s.to_string()),
                None => None,
            },
            group_username: if let Some(msg) = &callback_query.message {
                msg.chat().username().map(|s| s.to_string())
            } else {
                None
            },
            user_id: callback_query.from.id.0,
            username: callback_query.from.username.clone(),
            timestamp: DateTime::now(),
            msg_type: MessageType::Callback,
            msg_ctx: MessageContext::new(0),
            error: None,
        };
        Self(bl)
    }
}

impl From<BotLogBuilder> for BotLog {
    fn from(mut val: BotLogBuilder) -> Self {
        val.0.msg_ctx.time_cost =
            DateTime::now().timestamp_millis() - val.0.timestamp.timestamp_millis();
        val.0
    }
}
