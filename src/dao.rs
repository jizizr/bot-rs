pub mod mongo;
pub mod mysql;
pub mod rdb;

use crate::{AppError, BotResult, settings::SETTINGS};
use lazy_static::lazy_static;
