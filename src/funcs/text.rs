use crate::{
    analysis::model::{BotLogBuilder, MessageStatus},
    dao::mongo::analysis::insert_log,
};

use super::{pkg::kv::GroupFuncSwitch, *};

mod fix;
mod fuck_b23;
mod guozao;
mod pretext;
mod repeat;
mod six;

lazy_static! {
    pub static ref SWITCH: GroupFuncSwitch = GroupFuncSwitch::new();
    static ref _T: Option<()> = init();
}

trait Display {
    fn fmt(&self) -> Option<String>;
}

impl Display for BotResult {
    fn fmt(&self) -> Option<String> {
        if let Err(e) = self {
            Some(format!("{}", e))
        } else {
            None
        }
    }
}

macro_rules! impl_tuple {
    ($($idx:tt $t:tt),+) => {
        impl<$($t,)+> Display for ($($t,)+)
        where
            $($t: Display,)+
        {
            fn fmt(&self) -> Option<String> {
                let mut estring = String::new();
                ($(
                    match self.$idx.fmt() {
                        Some(s) => estring.push_str(&s),
                        None => (),
                    },
                )+);
                if estring.is_empty() {
                    None
                } else {
                    Some(estring)
                }
            }
        }
    };
}

impl_tuple!(0 A, 1 B, 2 C, 3 D,4 E);

impl_tuple!(0 A);

macro_rules! with_switch {
    ($func:expr,$bot:expr, $msg:expr) => {
        async {
            if SWITCH.get_status($msg.chat.id.0, stringify!($func).to_string()) {
                $func($bot, $msg).await
            } else {
                Ok(())
            }
        }
    };
}

macro_rules! join_with_switch {
    ($bot:expr, $msg:expr, $($func:expr),+ $(,)?) => {
        tokio::join!(
            $(with_switch!($func,$bot, $msg)),+
        )
    };
}
macro_rules! add_template {
    ($($func_name:expr=> $func_desc:expr),+ $(,)?) => {
        $(
            SWITCH.update_template(stringify!($func_name), $func_desc);
        )+
    };
}

pub fn init() -> Option<()> {
    add_template!(
        fix::fix => "补括号",
        six::six => "6",
        repeat::repeat => "复读机",
        fuck_b23::fuck_b23 => "去除b站短链跟踪参数",
        guozao::guozao => "play的一环"
    );
    tokio::spawn(SWITCH.pstorer.pool());
    Some(())
}

pub async fn text_handler(bot: &Bot, msg: &Message) -> BotResult {
    if let Some(m) = getor(msg) {
        let mut blog = BotLogBuilder::from(msg);
        if !m.starts_with('/') {
            let e = join_with_switch!(
                &bot,
                &msg,
                fix::fix,
                six::six,
                repeat::repeat,
                pretext::pretext,
                fuck_b23::fuck_b23
            );
            if let Some(err) = e.fmt() {
                log::error!("{}", err);
                blog.set_status(MessageStatus::RunError);
                blog.set_error(err);
            }
        } else {
            let e = join_with_switch!(&bot, &msg, guozao::guozao);
            if let Some(err) = e.fmt() {
                log::error!("{}", err);
                blog.set_status(MessageStatus::RunError);
                blog.set_error(err);
            }
        }
        blog.set_command(m.to_string());
        let _ = insert_log(&blog.into()).await;
    }
    Ok(())
}
