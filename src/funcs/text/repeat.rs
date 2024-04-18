use super::*;
use dashmap::DashMap;
use lazy_static::lazy_static;
use std::{
    collections::VecDeque,
    ops::{Index, IndexMut},
};

lazy_static! {
    static ref MESSAGE_MAP: DashMap<i64, FixedQueue<MessageConfig>> = DashMap::new();
}

struct FixedQueue<T> {
    queue: VecDeque<T>,
    capacity: usize,
}

impl<T> FixedQueue<T> {
    fn new(capacity: usize) -> Self {
        Self {
            queue: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    fn push(&mut self, item: T) {
        if self.queue.len() == self.capacity {
            self.queue.pop_front();
        }
        self.queue.push_back(item);
    }

    fn remove(&mut self, index: usize) {
        self.queue.remove(index);
    }

    fn len(&self) -> usize {
        self.queue.len()
    }
}

impl<T> Index<usize> for FixedQueue<T> {
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        &self.queue[index]
    }
}

impl<T> IndexMut<usize> for FixedQueue<T> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.queue[index]
    }
}

#[derive(Debug)]
struct MessageConfig {
    message: String,
    user_id: u64,
}

impl MessageConfig {
    fn new(message: String, user_id: u64) -> Self {
        Self { message, user_id }
    }
}

fn search(mc: &mut FixedQueue<MessageConfig>, msg: &MessageConfig) -> bool {
    let mut i = 0;
    while i < mc.len() {
        let m = &mc[i];
        if m.message == msg.message {
            if m.user_id != msg.user_id {
                return true;
            }
            mc.remove(i);
        }
        i += 1;
    }
    false
}

pub async fn repeat(bot: &Bot, msg: &Message) -> BotResult {
    let m: String = match getor(msg) {
        Some(m) => m.to_string(),
        None => match msg.sticker() {
            Some(sticker) => &sticker.file.unique_id,
            None => return Ok(()),
        }
        .to_string(),
    };
    let m = MessageConfig::new(m, msg.from().unwrap().id.0);
    let should_forward = match MESSAGE_MAP.get_mut(&msg.chat.id.0) {
        Some(mut mc) => {
            let result = search(mc.value_mut(), &m);
            if !result {
                mc.value_mut().push(m);
            }
            result
        }
        None => {
            let mut vd: FixedQueue<MessageConfig> = FixedQueue::new(5);
            vd.push(m);
            MESSAGE_MAP.insert(msg.chat.id.0, vd);
            false
        }
    };
    if should_forward {
        bot.forward_message(msg.chat.id, msg.chat.id, msg.id)
            .send()
            .await?;
    }
    Ok(())
}
