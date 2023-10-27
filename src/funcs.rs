use bot_rs::{get, getor};
use lazy_static::lazy_static;
use serde::Deserialize;
use std::error::Error;
use teloxide::{
    prelude::*,
    types::{Me, ParseMode,ChatAction},
    utils::{command::BotCommands, markdown},
};

pub mod command;
pub mod pkg;
pub mod text;

type BotError = Box<dyn Error + Send + Sync>;
type BotResult = Result<(), BotError>;
