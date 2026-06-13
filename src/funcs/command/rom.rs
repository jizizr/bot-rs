use super::*;
use std::sync::LazyLock;
use updater_core::{Device, QueryResult, RemoteDevices, RomInfoData, RomQuery, fetch_rom_info};

mod updater_core;
mod updater_links;

const TELEGRAM_MESSAGE_LIMIT: usize = 3900;
const DEVICE_LIST_URL: &str =
    "https://raw.githubusercontent.com/YuKongA/Updater-KMP/device-list/device.json";

cmd!(
    "/rom",
    "抓取小米刷机包链接",
    RomCmd,
    {
        ///机型名或代号 + 系统版本 + 可选 Android/区域/运营商，例如 Xiaomi 15 3.0.302.0
        #[arg(required = true, num_args = 2.., trailing_var_arg = true)]
        args: Vec<String>,
    }
);

struct ParsedRomCmd {
    device_input: String,
    code_name: String,
    system_version: String,
    android_version: String,
    region: String,
    carrier: String,
    devices: Vec<Device>,
}

fn parse_region(value: &str) -> Result<String, String> {
    let normalized = value.trim().to_uppercase();
    let region = match normalized.as_str() {
        "CN" | "DEFAULT" | "DEFAULT(CN)" | "DEFAULT (CN)" => "Default (CN)",
        "GL" | "MI" | "GLOBAL" => "GL (MI)",
        "EU" | "EEA" => "EEA (EU)",
        "CL" => "CL",
        "GT" => "GT",
        "ID" => "ID",
        "IN" => "IN",
        "JP" => "JP",
        "KR" => "KR",
        "LM" => "LM",
        "MX" => "MX",
        "RU" => "RU",
        "TR" => "TR",
        "TW" => "TW",
        "ZA" => "ZA",
        _ => return Err("不支持的区域代码".to_string()),
    };
    Ok(region.to_string())
}

fn parse_carrier(value: &str) -> Result<String, String> {
    let normalized = value.trim().to_uppercase();
    let carrier = match normalized.as_str() {
        "XM" | "DEFAULT" | "XIAOMI" | "DEFAULT(XIAOMI)" | "DEFAULT (XIAOMI)" => "Default (Xiaomi)",
        "DM" | "DEMO" => "MiStore (Demo)",
        "DC" => "DeviceLockController",
        "AT" => "AT&T",
        "BY" => "Bouygues",
        "CR" => "Claro",
        "EN" => "Entel",
        "HG" => "3HK",
        "KD" => "KDDI",
        "MS" => "Movistar",
        "MT" => "MTN",
        "OR" => "Orange",
        "SB" => "SoftBank",
        "SF" => "Altice France",
        "TF" => "Telefónica",
        "TG" => "Tigo",
        "TI" => "TIM",
        "VC" => "Vodacom",
        "VF" => "Vodafone",
        _ => return Err("不支持的运营商代码".to_string()),
    };
    Ok(carrier.to_string())
}

fn is_system_version(value: &str) -> bool {
    static VERSION_MATCH: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^(OS|V)?\d+(?:\.\d+){2,}(?:\.[A-Z0-9]+)?(?:\.AUTO|\.DEV)?$").unwrap()
    });
    VERSION_MATCH.is_match(value)
}

fn normalize_system_version(value: &str) -> String {
    let upper = value.trim().to_uppercase();
    let with_prefix = if upper.starts_with("OS") || upper.starts_with('V') {
        upper
    } else {
        format!("OS{upper}")
    };
    if with_prefix.ends_with(".AUTO")
        || with_prefix.ends_with(".DEV")
        || Regex::new(r"\.[A-Z]\w{6,}$")
            .unwrap()
            .is_match(&with_prefix)
    {
        with_prefix
    } else {
        format!("{with_prefix}.AUTO")
    }
}

fn normalize_device_text(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn clean_json_trailing_commas(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b',' {
            let mut lookahead = index + 1;
            while lookahead < bytes.len() && bytes[lookahead].is_ascii_whitespace() {
                lookahead += 1;
            }
            if lookahead < bytes.len() && (bytes[lookahead] == b'}' || bytes[lookahead] == b']') {
                index += 1;
                continue;
            }
        }
        out.push(bytes[index] as char);
        index += 1;
    }
    out
}

async fn fetch_devices() -> Result<Vec<Device>, BotError> {
    let body = reqwest::get(DEVICE_LIST_URL).await?.text().await?;
    let cleaned = clean_json_trailing_commas(&body);
    let remote: RemoteDevices = serde_json::from_str(&cleaned)?;
    Ok(remote.devices)
}

fn looks_like_code_name(input: &str) -> bool {
    !input.contains(char::is_whitespace)
        && input
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

async fn resolve_code_name(input: &str) -> Result<(String, Vec<Device>), BotError> {
    let input = input.trim();
    let normalized_input = normalize_device_text(input);
    let devices = match fetch_devices().await {
        Ok(devices) => devices,
        Err(_) if looks_like_code_name(input) => return Ok((input.to_string(), Vec::new())),
        Err(error) => return Err(error),
    };

    if devices
        .iter()
        .any(|device| device.device_code_name.eq_ignore_ascii_case(input))
    {
        return Ok((input.to_lowercase(), devices));
    }

    let exact = devices.iter().find(|device| {
        device
            .device_name
            .split('/')
            .any(|name| normalize_device_text(name) == normalized_input)
    });
    if let Some(device) = exact {
        return Ok((device.device_code_name.clone(), devices));
    }

    let contains = devices.iter().find(|device| {
        normalize_device_text(&device.device_name).contains(&normalized_input)
            || normalized_input.contains(&normalize_device_text(&device.device_name))
    });
    if let Some(device) = contains {
        return Ok((device.device_code_name.clone(), devices));
    }

    Err(BotError::Custom(format!("未找到机型：{input}")))
}

async fn parse_rom_args(args: Vec<String>) -> Result<ParsedRomCmd, BotError> {
    let version_index = args
        .iter()
        .rposition(|arg| is_system_version(arg))
        .ok_or_else(|| BotError::Custom("未找到系统版本，例如 3.0.302.0".to_string()))?;
    if version_index == 0 {
        return Err(BotError::Custom("缺少机型名或代号".to_string()));
    }

    let device_input = args[..version_index].join(" ");
    let system_version = normalize_system_version(&args[version_index]);
    let mut tail = args[version_index + 1..].iter();
    let first_tail = tail.next();
    let first_is_android = first_tail.is_some_and(|value| value.contains('.'));
    let android_version = if first_is_android {
        first_tail.cloned().unwrap_or_else(|| "16.0".to_string())
    } else {
        "16.0".to_string()
    };
    let region_value = if first_is_android {
        tail.next()
    } else {
        first_tail
    };
    let region = region_value
        .map(|value| parse_region(value))
        .transpose()
        .map_err(BotError::Custom)?
        .unwrap_or_else(|| "Default (CN)".to_string());
    let carrier = tail
        .next()
        .map(|value| parse_carrier(value))
        .transpose()
        .map_err(BotError::Custom)?
        .unwrap_or_else(|| "Default (Xiaomi)".to_string());
    let (code_name, devices) = resolve_code_name(&device_input).await?;

    Ok(ParsedRomCmd {
        device_input,
        code_name,
        system_version,
        android_version,
        region,
        carrier,
        devices,
    })
}

fn human_size(raw: &str) -> String {
    let Ok(size) = raw.parse::<f64>() else {
        return raw.to_string();
    };
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = size;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{size:.0} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

fn link_or_unavailable(link: &str) -> &str {
    if link.is_empty() { "不可用" } else { link }
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn html_link(label: &str, url: &str) -> Option<String> {
    if url.is_empty() {
        return None;
    }
    Some(format!(
        "<a href=\"{}\">{}</a>",
        html_escape(url),
        html_escape(label)
    ))
}

fn download_links(rom: &RomInfoData) -> String {
    let links = [
        html_link("官方1", &rom.official1_download),
        html_link("官方2", &rom.official2_download),
        html_link("CDN1", &rom.cdn1_download),
        html_link("CDN2", &rom.cdn2_download),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();

    if links.is_empty() {
        link_or_unavailable("").to_string()
    } else {
        links.join(" / ")
    }
}

fn push_optional_line(text: &mut String, label: &str, value: &str) {
    if !value.is_empty() && value != "null" {
        text.push_str(label);
        text.push_str(&html_escape(value));
        text.push('\n');
    }
}

fn push_rom_info(text: &mut String, rom: &RomInfoData) {
    push_optional_line(text, "大版本：", &rom.big_version);
    push_optional_line(text, "类型：", &rom.rom_type);
    push_optional_line(text, "代号：", &rom.device);
    push_optional_line(text, "分支：", &rom.branch);
    push_optional_line(text, "Android：", &rom.codebase);
    if rom.is_beta {
        text.push_str("Beta：是\n");
    }
    if rom.is_gov {
        text.push_str("政企版：是\n");
    }
    push_optional_line(text, "安全补丁：", &rom.security_patch_level);
    push_optional_line(text, "SDK：", &rom.sdk_level);
    push_optional_line(text, "构建时间：", &rom.timestamp);
    push_optional_line(text, "指纹：", &rom.fingerprint);
}

fn split_text_chunks(text: &str, max_len: usize) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + max_len).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if end < text.len()
            && let Some(relative_newline) = text[start..end].rfind('\n')
            && relative_newline > 0
        {
            end = start + relative_newline;
        }
        chunks.push(text[start..end].trim());
        start = end;
        while start < text.len() && text.as_bytes()[start].is_ascii_whitespace() {
            start += 1;
        }
    }
    chunks
}

fn expandable_blockquote_sections(title: &str, changelog: &str) -> Vec<String> {
    let changelog = changelog.trim();
    if changelog.is_empty() {
        return Vec::new();
    }

    split_text_chunks(changelog, 2800)
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            let title = if index == 0 {
                title.to_string()
            } else {
                format!("{title}（续 {}）", index + 1)
            };
            format!(
                "<b>{}</b>\n<blockquote expandable>{}</blockquote>",
                html_escape(&title),
                html_escape(chunk)
            )
        })
        .collect()
}

fn pack_messages(sections: Vec<String>) -> Vec<String> {
    let mut messages = Vec::new();
    let mut current = String::new();
    for section in sections {
        let separator_len = if current.is_empty() { 0 } else { 2 };
        if current.len() + separator_len + section.len() <= TELEGRAM_MESSAGE_LIMIT {
            if !current.is_empty() {
                current.push_str("\n\n");
            }
            current.push_str(&section);
        } else {
            if !current.is_empty() {
                messages.push(current);
            }
            current = section;
        }
    }
    if !current.is_empty() {
        messages.push(current);
    }
    messages
}

fn build_reply(query: &ParsedRomCmd, result: QueryResult) -> Vec<String> {
    if !result.ok {
        return vec![format!("查询失败：{}", html_escape(&result.message))];
    }

    let rom = &result.cur_rom_info;
    let mut text = format!(
        "<b>小米刷机包更新信息</b>\n\
         机型代号：{}\n\
         版本：{}\n\
         区域/运营商：{} / {}\n\
         文件：{}\n\
         大小：{}\n\
         MD5：{}\n",
        html_escape(&query.device_input),
        html_escape(&rom.version),
        html_escape(&query.region),
        html_escape(&query.carrier),
        html_escape(&rom.file_name),
        human_size(&rom.file_size),
        html_escape(&rom.md5),
    );

    push_rom_info(&mut text, rom);

    text.push_str("\n<b>下载链接</b>\n");
    text.push_str(&download_links(rom));

    if result.no_ultimate_link {
        text.push_str("\n\n提示：官方 ultimate 链接不可用，已返回可构造的 CDN 链接。");
    }
    if result.is_fallback {
        text.push_str("\n\n提示：请求版本不存在，返回了接口给出的回退包信息。");
    }
    if !rom.gentle_notice.is_empty() {
        text.push_str("\n\n<b>提示</b>\n");
        text.push_str(&html_escape(rom.gentle_notice.trim()));
    }

    let mut sections = expandable_blockquote_sections("刷机包更新日志", &rom.changelog);

    if result.xms_info.has_update && !result.xms_info.changelog_text.trim().is_empty() {
        sections.extend(expandable_blockquote_sections(
            "系统应用更新日志",
            &result.xms_info.changelog_text,
        ));
    }

    let mut messages = vec![text];
    messages.extend(pack_messages(sections));
    messages
}

async fn get_rom(msg: &Message) -> Result<Vec<String>, BotError> {
    let language_tag = Some("zh-CN");
    let rom = RomCmd::parse_i18n_from_bot(getor(msg).unwrap().split_whitespace(), language_tag)?;
    let rom = parse_rom_args(rom.args).await?;
    let query = RomQuery {
        code_name: rom.code_name.clone(),
        android_version: rom.android_version.clone(),
        system_version: rom.system_version.clone(),
        device_region: rom.region.clone(),
        device_carrier: rom.carrier.clone(),
        devices: rom.devices.clone(),
        ..Default::default()
    };
    let result = tokio::task::spawn_blocking(move || fetch_rom_info(query))
        .await?
        .map_err(BotError::Custom)?;
    Ok(build_reply(&rom, result))
}

pub async fn rom(bot: &Bot, msg: &Message) -> BotResult {
    tokio::spawn(bot.send_chat_action(msg.chat.id, ChatAction::Typing).send());
    let mut reply_to = msg.id;
    for (index, message) in get_rom(msg).await?.into_iter().enumerate() {
        let sent = bot
            .send_message(msg.chat.id, message)
            .parse_mode(ParseMode::Html)
            .reply_parameters(ReplyParameters::new(reply_to))
            .link_preview_options(LinkPreviewOptions {
                is_disabled: true,
                url: None,
                prefer_small_media: false,
                prefer_large_media: false,
                show_above_text: false,
            })
            .send()
            .await?;
        if index == 0 || sent.id != reply_to {
            reply_to = sent.id;
        }
    }
    Ok(())
}
