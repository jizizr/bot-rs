use super::*;
use std::collections::HashMap;

macro_rules! hashmaps {
    ($($key:expr => $value:expr),*) => {
        {
            let mut original_map = HashMap::new();
            $(
                original_map.insert($key, $value);
            )*

            let mut swapped_map = HashMap::new();
            $(
                swapped_map.insert($value, $key);
            )*

            original_map.extend(swapped_map);
            original_map
        }
    };
}

lazy_static! {
    static ref FIX_MAP: HashMap<char, char> = hashmaps![
        ')' => '(',
        ']' => '[',
        '}' => '{',
        '】' => '【',
        '｝' => '｛',
        '>' => '<',
        '』' => '『',
        '」' => '「',
        '»' => '«',
        '）' => '（',
        '》' => '《',
        '＞' => '＜',
        '］' => '［'
    ];
}

fn clear(input: &mut String, target_char: char) -> Option<()> {
    if let Some((index, char_len)) = input
        .char_indices()
        .find(|(_, c)| *c == target_char)
        .map(|(i, _)| (i, target_char.len_utf8()))
    {
        let bytes = unsafe { input.as_mut_vec() };
        for i in index..(index + char_len) {
            bytes[i] = 0;
        }
        Some(())
    } else {
        None
    }
}

fn extend(buffer: &mut String, new: char, old: char) {
    if clear(buffer, old).is_none() {
        buffer.push(new);
    }
}

pub async fn fix(bot: &Bot, msg: &Message) -> BotResult {
    let mut buffer = String::with_capacity(4);
    for c in getor(msg).unwrap().chars() {
        if let Some(ch) = FIX_MAP.get(&c) {
            extend(&mut buffer, *ch, c)
        }
    }
    let t: String = buffer.chars().filter(|&c| c != '\0').collect();
    if !t.trim_end().is_empty() {
        bot.send_message(msg.chat.id, t)
            .reply_to_message_id(msg.id)
            .send()
            .await?;
    }
    Ok(())
}
