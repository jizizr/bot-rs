use bot_rs::{get, getor};
use lazy_static::lazy_static;
use serde::Deserialize;
use std::error::Error;
use teloxide::{prelude::*, types::ParseMode, utils::markdown};
pub mod command;
pub mod pkg;
pub mod text;
