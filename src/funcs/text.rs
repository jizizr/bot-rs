use super::*;
pub mod fix;
pub mod fuck_b23;
pub mod pretext;
pub mod repeat;
pub mod six;

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

pub async fn text_handler(bot: Bot, msg: Message) -> BotResult {
    if getor(&msg).is_some() {
        if !getor(&msg).unwrap().starts_with("/") {
            let e = tokio::join!(
                fix::fix(&bot, &msg),
                six::six(&bot, &msg),
                repeat::repeat(&bot, &msg),
                pretext::pretext(&bot, &msg),
                fuck_b23::fuck_b23(&bot, &msg)
            );
            if let Some(err) = e.fmt() {
                log::error!("{}", err);
            }
        }
    } else {
        println!("{:#?}", msg);
    }
    Ok(())
}
