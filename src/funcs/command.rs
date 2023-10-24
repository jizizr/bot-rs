use super::*;
use crate::error_fmt;
use clap::{CommandFactory, Parser};
use thiserror::Error;

pub mod coin;
pub mod curl;
pub mod id;
pub mod quote;
pub mod rate;
pub mod short;
pub mod start;
pub mod test;
pub mod today;
pub mod wcloud;
pub mod wiki;

#[macro_export]
macro_rules! error_fmt {
    ($usage:ident) => {
        fn clap_fmt(err: &clap::error::Error) -> String {
            format!(
                "{}\n{}",
                err.render().to_string().splitn(2, "Usage").nth(0).unwrap(),
                *$usage
            )
        }
        fn custom_fmt(err: &String) -> String {
            format!("{}\n\n{}", err, *USAGE)
        }
        #[allow(dead_code)]
        #[derive(Error, Debug)]
        enum AppError {
            #[error("API请求失败: {0}")]
            RequestError(#[from] reqwest::Error),
            #[error("{}",clap_fmt(.0))]
            ClapError(#[from] clap::error::Error),
            #[error("{}",custom_fmt(.0))]
            CustomError(String),
        }
    };
}
