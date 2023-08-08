use teloxide::prelude::*;

pub fn getor(msg: &Message) -> Option<&str> {
    msg.text().or(msg.caption())
}
