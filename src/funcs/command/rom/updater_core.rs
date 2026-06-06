use super::updater_links::{
    CurrentDownloadProbe, resolve_current_download, resolve_xms_download_url,
};
use aes::Aes128;
use base64::{Engine as _, engine::general_purpose};
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
use chrono::{Local, TimeZone};
use indexmap::IndexMap;
use prost::Message;
use reqwest::blocking::Client;
use reqwest::header::{CONTENT_LENGTH, CONTENT_RANGE, COOKIE, RANGE};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::error::Error;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::OnceLock;
use std::time::Duration;
use uuid::Uuid;

pub use super::updater_links::{DownloadLinks, OfficialPath, ResolvedDownloadPath};

type Aes128CbcEnc = cbc::Encryptor<Aes128>;
type Aes128CbcDec = cbc::Decryptor<Aes128>;

const IV: &[u8; 16] = b"0102030405060708";
const DEFAULT_SECURITY_KEY: &[u8; 16] = b"miuiotavalided11";
const CN_RECOVERY_URL: &str = "https://update.miui.com/updates/miotaV3.php";
const INTL_RECOVERY_URL: &str = "https://update.intl.miui.com/updates/miotaV3.php";
const CN_GETXMSVER_URL: &str = "https://update.miui.com/api/v3/xms/getXmsVer";
const INTL_GETXMSVER_URL: &str = "https://update.intl.miui.com/api/v3/xms/getXmsVer";
const DEVICE_LIST_URL: &str =
    "https://raw.githubusercontent.com/YuKongA/Updater-KMP/device-list/device.json";
const ACCOUNT_URL: &str = "https://account.xiaomi.com";
const USER_AGENT_VALUE: &str =
    "Dalvik/2.1.0 (Linux; U; Android 16; 2509FPN0BC Build/BP2A.250605.031.A3)";
const METADATA_PATH: &str = "META-INF/com/android/metadata";
const METADATA_PB_PATH: &str = "META-INF/com/android/metadata.pb";
const END_BYTES_SIZE: i64 = 4096;
const LOCAL_HEADER_SIZE: i64 = 256;

static REMOTE_DEVICE_CACHE: OnceLock<Option<Vec<Device>>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Device {
    #[serde(rename = "deviceName")]
    pub device_name: String,
    #[serde(rename = "deviceCodeName")]
    pub device_code_name: String,
    #[serde(rename = "deviceCode")]
    pub device_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteDevices {
    pub devices: Vec<Device>,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LoginData {
    #[serde(rename = "accountType", default)]
    pub account_type: Option<String>,
    #[serde(rename = "authResult", default)]
    pub auth_result: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub ssecurity: Option<String>,
    #[serde(rename = "serviceToken", default)]
    pub service_token: Option<String>,
    #[serde(rename = "userId", default)]
    pub user_id: Option<String>,
    #[serde(rename = "cUserId", default)]
    pub c_user_id: Option<String>,
    #[serde(rename = "passToken", default)]
    pub pass_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RomQuery {
    #[serde(rename = "deviceName", default)]
    pub device_name: String,
    #[serde(rename = "codeName", default)]
    pub code_name: String,
    #[serde(rename = "deviceRegion", default = "default_region")]
    pub device_region: String,
    #[serde(rename = "deviceCarrier", default = "default_carrier")]
    pub device_carrier: String,
    #[serde(rename = "androidVersion", default = "default_android")]
    pub android_version: String,
    #[serde(rename = "systemVersion", default)]
    pub system_version: String,
    #[serde(default)]
    pub devices: Vec<Device>,
    #[serde(rename = "loginData", default)]
    pub login_data: Option<LoginData>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RomInfoData {
    #[serde(rename = "type")]
    pub rom_type: String,
    pub device: String,
    pub version: String,
    pub codebase: String,
    pub branch: String,
    #[serde(rename = "bigVersion")]
    pub big_version: String,
    #[serde(rename = "fileName")]
    pub file_name: String,
    #[serde(rename = "fileSize")]
    pub file_size: String,
    pub md5: String,
    #[serde(rename = "isBeta")]
    pub is_beta: bool,
    #[serde(rename = "isGov")]
    pub is_gov: bool,
    #[serde(rename = "official1Download")]
    pub official1_download: String,
    #[serde(rename = "official2Download")]
    pub official2_download: String,
    #[serde(rename = "cdn1Download")]
    pub cdn1_download: String,
    #[serde(rename = "cdn2Download")]
    pub cdn2_download: String,
    pub changelog: String,
    #[serde(rename = "gentleNotice")]
    pub gentle_notice: String,
    pub fingerprint: String,
    #[serde(rename = "securityPatchLevel")]
    pub security_patch_level: String,
    pub timestamp: String,
    #[serde(rename = "sdkLevel")]
    pub sdk_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IconInfoData {
    pub changelog: String,
    #[serde(rename = "iconName")]
    pub icon_name: String,
    #[serde(rename = "iconLink")]
    pub icon_link: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImageInfoData {
    pub title: String,
    pub changelog: String,
    #[serde(rename = "imageUrl")]
    pub image_url: String,
    #[serde(rename = "imageWidth")]
    pub image_width: Option<i64>,
    #[serde(rename = "imageHeight")]
    pub image_height: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct XmsAppInfo {
    pub name: String,
    #[serde(rename = "packName")]
    pub pack_name: String,
    #[serde(rename = "versionCode")]
    pub version_code: String,
    #[serde(rename = "fileName")]
    pub file_name: String,
    #[serde(rename = "fileSize")]
    pub file_size: String,
    pub md5: String,
    #[serde(rename = "downloadUrls")]
    pub download_urls: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct XmsInfoData {
    #[serde(rename = "hasUpdate")]
    pub has_update: bool,
    #[serde(rename = "curVer")]
    pub cur_ver: String,
    #[serde(rename = "lstVer")]
    pub lst_ver: String,
    #[serde(rename = "pkgCnt")]
    pub pkg_cnt: i64,
    pub prio: i64,
    pub apps: Vec<XmsAppInfo>,
    #[serde(rename = "gentleNotice")]
    pub gentle_notice: String,
    #[serde(rename = "changelogItems")]
    pub changelog_items: Vec<ImageInfoData>,
    #[serde(rename = "changelogText")]
    pub changelog_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QueryResult {
    pub ok: bool,
    #[serde(default)]
    pub message: String,
    #[serde(rename = "curRomInfo", default)]
    pub cur_rom_info: RomInfoData,
    #[serde(rename = "incRomInfo", default)]
    pub inc_rom_info: RomInfoData,
    #[serde(rename = "curIconInfo", default)]
    pub cur_icon_info: Vec<IconInfoData>,
    #[serde(rename = "incIconInfo", default)]
    pub inc_icon_info: Vec<IconInfoData>,
    #[serde(rename = "curImageInfo", default)]
    pub cur_image_info: Vec<ImageInfoData>,
    #[serde(rename = "incImageInfo", default)]
    pub inc_image_info: Vec<ImageInfoData>,
    #[serde(rename = "xmsInfo", default)]
    pub xms_info: XmsInfoData,
    #[serde(rename = "noUltimateLink", default)]
    pub no_ultimate_link: bool,
    #[serde(rename = "isFallback", default)]
    pub is_fallback: bool,
    #[serde(rename = "loginData", default)]
    pub login_data: Option<LoginData>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RomDownloadResult {
    pub ok: bool,
    #[serde(default)]
    pub message: String,
    #[serde(rename = "fileName", default)]
    pub file_name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub md5: String,
    #[serde(rename = "fileSize", default)]
    pub file_size: String,
    #[serde(default)]
    pub links: DownloadLinks,
    #[serde(rename = "noUltimateLink", default)]
    pub no_ultimate_link: bool,
    #[serde(rename = "loginData", default)]
    pub login_data: Option<LoginData>,
}

#[derive(Clone, PartialEq, Message)]
struct OtaMetadataPb {
    #[prost(int32, tag = "1")]
    pub ota_type: i32,
    #[prost(bool, tag = "2")]
    pub wipe: bool,
    #[prost(bool, tag = "3")]
    pub downgrade: bool,
    #[prost(map = "string, string", tag = "4")]
    pub property_files: HashMap<String, String>,
    #[prost(message, optional, tag = "5")]
    pub precondition: Option<DeviceStatePb>,
    #[prost(message, optional, tag = "6")]
    pub postcondition: Option<DeviceStatePb>,
    #[prost(bool, tag = "7")]
    pub retrofit_dynamic_partitions: bool,
    #[prost(int64, tag = "8")]
    pub required_cache: i64,
    #[prost(bool, tag = "9")]
    pub spl_downgrade: bool,
}

#[derive(Clone, PartialEq, Message)]
struct DeviceStatePb {
    #[prost(string, repeated, tag = "1")]
    pub device: Vec<String>,
    #[prost(string, repeated, tag = "2")]
    pub build: Vec<String>,
    #[prost(string, tag = "3")]
    pub build_incremental: String,
    #[prost(int64, tag = "4")]
    pub timestamp: i64,
    #[prost(string, tag = "5")]
    pub sdk_level: String,
    #[prost(string, tag = "6")]
    pub security_patch_level: String,
    #[prost(message, repeated, tag = "7")]
    pub partition_state: Vec<PartitionStatePb>,
}

#[derive(Clone, PartialEq, Message)]
struct PartitionStatePb {
    #[prost(string, tag = "1")]
    pub partition_name: String,
    #[prost(string, repeated, tag = "2")]
    pub device: Vec<String>,
    #[prost(string, repeated, tag = "3")]
    pub build: Vec<String>,
    #[prost(string, tag = "4")]
    pub version: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[allow(dead_code)]
struct RomInfo {
    #[serde(rename = "AuthResult")]
    auth_result: Option<i64>,
    #[serde(rename = "CurrentRom")]
    current_rom: Option<Rom>,
    #[serde(rename = "LatestRom")]
    latest_rom: Option<Rom>,
    #[serde(rename = "IncrementRom")]
    increment_rom: Option<Rom>,
    #[serde(rename = "CrossRom")]
    cross_rom: Option<Rom>,
    #[serde(rename = "Icon")]
    icon: Option<HashMap<String, String>>,
    #[serde(rename = "FileMirror")]
    file_mirror: Option<FileMirror>,
    #[serde(rename = "GentleNotice")]
    gentle_notice: Option<GentleNotice>,
    #[serde(rename = "xmsUpdateInfo")]
    xms_update_info: Option<XmsUpdateInfo>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct Rom {
    bigversion: Option<String>,
    branch: Option<String>,
    #[serde(default, deserialize_with = "deserialize_changelog")]
    changelog: IndexMap<String, Vec<ChangelogItem>>,
    codebase: Option<String>,
    device: Option<String>,
    filename: Option<String>,
    filesize: Option<String>,
    md5: Option<String>,
    osbigversion: Option<String>,
    #[serde(rename = "type")]
    rom_type: Option<String>,
    version: Option<String>,
    #[serde(default, rename = "isBeta")]
    is_beta: i64,
    #[serde(default, rename = "isGov")]
    is_gov: i64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[allow(dead_code)]
struct ChangelogItem {
    #[serde(default)]
    txt: String,
    #[serde(default)]
    image: Vec<ChangelogImage>,
    #[serde(default, rename = "package")]
    package_name: Option<String>,
    #[serde(default, rename = "versionCode")]
    version_code: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ChangelogImage {
    #[serde(default)]
    path: String,
    #[serde(default)]
    h: String,
    #[serde(default)]
    w: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct FileMirror {
    #[serde(default)]
    icon: String,
    #[serde(default)]
    image: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct GentleNotice {
    #[serde(default)]
    text: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct XmsUpdateInfo {
    #[serde(default, rename = "hasXmsUpdate")]
    has_xms_update: i64,
    #[serde(default, rename = "lstVer")]
    lst_ver: Option<String>,
    #[serde(default, rename = "pkgCnt")]
    pkg_cnt: i64,
    #[serde(default, rename = "curVer")]
    cur_ver: Option<String>,
    #[serde(default)]
    prio: Option<i64>,
    #[serde(default)]
    pkgs: Vec<String>,
    #[serde(default, rename = "gentleNotice")]
    gentle_notice: Option<GentleNotice>,
    #[serde(
        default,
        rename = "changeLog",
        deserialize_with = "deserialize_changelog"
    )]
    change_log: IndexMap<String, Vec<ChangelogItem>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct XmsDto {
    #[serde(default, rename = "apkLists")]
    apk_lists: Vec<XmsApkInfo>,
    #[serde(default, rename = "mirrorList")]
    mirror_list: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct XmsApkInfo {
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "packName")]
    pack_name: Option<String>,
    #[serde(default, rename = "lastVerCode")]
    last_ver_code: Option<String>,
    #[serde(default, rename = "fileName")]
    file_name: Option<String>,
    #[serde(default)]
    md5: Option<String>,
    #[serde(default)]
    size: Option<i64>,
    #[serde(default, rename = "downloadUrls")]
    download_urls: Vec<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct RequestParams {
    region_code: String,
    carrier_code: String,
    code_name_ext: String,
    system_version_ext: String,
    branch_ext: String,
}

fn default_region() -> String {
    "Default (CN)".to_owned()
}

fn default_carrier() -> String {
    "Default (Xiaomi)".to_owned()
}

fn default_android() -> String {
    "16.0".to_owned()
}

fn login_client() -> Result<Client, String> {
    static LOGIN_CLIENT: OnceLock<Result<Client, String>> = OnceLock::new();
    LOGIN_CLIENT
        .get_or_init(|| {
            Client::builder()
                .timeout(Duration::from_secs(30))
                .cookie_store(true)
                .user_agent(USER_AGENT_VALUE)
                .build()
                .map_err(|e| e.to_string())
        })
        .clone()
}

fn update_client() -> Result<Client, String> {
    static UPDATE_CLIENT: OnceLock<Result<Client, String>> = OnceLock::new();
    UPDATE_CLIENT
        .get_or_init(|| {
            Client::builder()
                .timeout(Duration::from_secs(30))
                .user_agent(USER_AGENT_VALUE)
                .build()
                .map_err(|e| e.to_string())
        })
        .clone()
}

fn error_chain<E: Error>(error: E) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(error) = source {
        message.push_str(": ");
        message.push_str(&error.to_string());
        source = error.source();
    }
    message
}

fn deserialize_changelog<'de, D>(
    deserializer: D,
) -> Result<IndexMap<String, Vec<ChangelogItem>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    let mut result = IndexMap::new();
    let Some(object) = value.as_object() else {
        return Ok(result);
    };
    for (key, value) in object {
        let mut items = Vec::new();
        if let Some(obj) = value.as_object() {
            if let Some(txt) = obj.get("txt").and_then(Value::as_array) {
                for item in txt {
                    if let Some(text) = item.as_str() {
                        items.push(ChangelogItem {
                            txt: text.to_owned(),
                            ..Default::default()
                        });
                    }
                }
            }
        } else if let Some(array) = value.as_array() {
            for item in array {
                if let Ok(parsed) = serde_json::from_value::<ChangelogItem>(item.clone()) {
                    items.push(parsed);
                }
            }
        }
        if !items.is_empty() {
            result.insert(key.clone(), items);
        }
    }
    Ok(result)
}

fn miui_encrypt(json_request: &str, security_key: &[u8]) -> Result<String, String> {
    let encrypted = Aes128CbcEnc::new_from_slices(security_key, IV)
        .map_err(|e| e.to_string())?
        .encrypt_padded_vec_mut::<Pkcs7>(json_request.as_bytes());
    Ok(general_purpose::URL_SAFE.encode(encrypted))
}

fn miui_decrypt(encrypted_text: &str, security_key: &[u8]) -> Result<String, String> {
    let encrypted = general_purpose::STANDARD
        .decode(encrypted_text.trim())
        .or_else(|_| general_purpose::STANDARD_NO_PAD.decode(encrypted_text.trim()))
        .map_err(|e| e.to_string())?;
    let decrypted = Aes128CbcDec::new_from_slices(security_key, IV)
        .map_err(|e| e.to_string())?
        .decrypt_padded_vec_mut::<Pkcs7>(&encrypted)
        .map_err(|e| e.to_string())?;
    String::from_utf8(decrypted).map_err(|e| e.to_string())
}

fn json_scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.is_empty() && text != "null" => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn ensure_encrypted_response(body: String) -> Result<String, String> {
    ensure_encrypted_response_with_context(body, None)
}

fn ensure_encrypted_response_with_context(
    body: String,
    context: Option<&str>,
) -> Result<String, String> {
    let trimmed = body.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        let mut readable = serde_json::from_str::<Value>(trimmed)
            .ok()
            .and_then(|value| {
                let code = json_scalar_string(&value["code"]);
                let desc = json_scalar_string(&value["desc"])
                    .or_else(|| json_scalar_string(&value["description"]))
                    .or_else(|| json_scalar_string(&value["message"]));
                match (code, desc) {
                    (Some(code), Some(desc)) => Some(format!(
                        "Server returned plaintext response: code {code}, {desc}"
                    )),
                    (Some(code), None) => {
                        Some(format!("Server returned plaintext response: code {code}"))
                    }
                    (None, Some(desc)) => {
                        Some(format!("Server returned plaintext response: {desc}"))
                    }
                    (None, None) => None,
                }
            })
            .unwrap_or_else(|| format!("Server returned plaintext response: {trimmed}"));
        if let Some(context) = context.filter(|it| !it.is_empty()) {
            readable.push_str("; context: ");
            readable.push_str(context);
        }
        return Err(readable);
    }
    Ok(body)
}

fn build_request_params(query: &RomQuery) -> RequestParams {
    let region_code = region_code(&query.device_region);
    let carrier_code = carrier_code(&query.device_carrier);
    let device_code = device_code_of(
        &query.devices,
        &query.android_version,
        &query.code_name,
        &region_code,
        &carrier_code,
    );
    let region_code_name = region_code_name(&query.device_region);
    let carrier_code_name = carrier_code_name(&query.device_carrier);
    let code_name_ext = if !region_code_name.is_empty() {
        format!(
            "{}{}{}_global",
            query.code_name,
            region_code_name.replace("_global", ""),
            carrier_code_name
        )
    } else if region_code == "CN" && carrier_code == "DM" {
        format!("{}_demo", query.code_name)
    } else {
        format!("{}{}", query.code_name, carrier_code_name)
    };
    let mut system_version_ext = query.system_version.to_uppercase();
    if system_version_ext.starts_with("OS1") {
        system_version_ext = system_version_ext.replacen("OS1", "V816", 1);
    }
    if system_version_ext.ends_with("AUTO") {
        system_version_ext = system_version_ext.trim_end_matches("AUTO").to_owned() + &device_code;
    }
    let branch_ext = if query.system_version.to_uppercase().ends_with(".DEV") {
        "X"
    } else {
        "F"
    }
    .to_owned();
    RequestParams {
        region_code,
        carrier_code,
        code_name_ext,
        system_version_ext,
        branch_ext,
    }
}

struct RecoveryJsonParams<'a> {
    branch: &'a str,
    code_name_ext: &'a str,
    region_code: &'a str,
    rom_version: &'a str,
    android_version: &'a str,
    user_id: &'a str,
    security: &'a str,
    token: &'a str,
    xms_version: &'a str,
}

fn generate_json(params: RecoveryJsonParams<'_>) -> String {
    let mut value = serde_json::Map::new();
    if !params.branch.is_empty() {
        value.insert("b".to_owned(), json!(params.branch));
    }
    value.insert("c".to_owned(), json!(params.android_version));
    value.insert("d".to_owned(), json!(params.code_name_ext));
    value.insert("f".to_owned(), json!("1"));
    value.insert("id".to_owned(), json!(params.user_id));
    value.insert(
        "l".to_owned(),
        json!(if params.code_name_ext.contains("_global") {
            "en_US"
        } else {
            "zh_CN"
        }),
    );
    value.insert("ov".to_owned(), json!(params.rom_version));
    value.insert("p".to_owned(), json!(params.code_name_ext));
    value.insert("pn".to_owned(), json!(params.code_name_ext));
    value.insert("r".to_owned(), json!(params.region_code));
    value.insert("security".to_owned(), json!(params.security));
    value.insert("token".to_owned(), json!(params.token));
    value.insert("unlock".to_owned(), json!("0"));
    value.insert(
        "v".to_owned(),
        json!(format!("MIUI-{}", params.rom_version)),
    );
    if !params.xms_version.is_empty() {
        value.insert("xv".to_owned(), json!(params.xms_version));
    }
    if params.android_version.parse::<f32>().unwrap_or(0.0) >= 15.0 {
        value.insert("options".to_owned(), json!({"av": "9.1.3"}));
    }
    Value::Object(value).to_string()
}

fn decode_security_key(ssecurity: &str) -> Option<Vec<u8>> {
    let compact = ssecurity
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .collect::<String>();
    let decoded = general_purpose::STANDARD
        .decode(&compact)
        .or_else(|_| general_purpose::STANDARD_NO_PAD.decode(&compact))
        .or_else(|_| general_purpose::URL_SAFE.decode(&compact))
        .or_else(|_| general_purpose::URL_SAFE_NO_PAD.decode(&compact))
        .ok()?;
    if decoded.len() == DEFAULT_SECURITY_KEY.len() {
        Some(decoded)
    } else {
        None
    }
}

fn get_security(
    login_data: Option<&LoginData>,
) -> (String, String, String, Vec<u8>, String, String, String) {
    if let Some(login) = login_data
        && login.auth_result.as_deref() != Some("3")
    {
        let account_type = login
            .account_type
            .clone()
            .unwrap_or_else(|| "CN".to_owned());
        let ssecurity = login.ssecurity.clone().unwrap_or_default();
        let service_token = login.service_token.clone().unwrap_or_default();
        let user_id = login.user_id.clone().unwrap_or_default();
        let c_user_id = login.c_user_id.clone().unwrap_or_default();
        if !ssecurity.is_empty()
            && !service_token.is_empty()
            && !user_id.is_empty()
            && !c_user_id.is_empty()
            && let Some(security_key) = decode_security_key(&ssecurity)
        {
            return (
                account_type,
                "2".to_owned(),
                ssecurity,
                security_key,
                service_token,
                user_id,
                c_user_id,
            );
        }
    }
    (
        "CN".to_owned(),
        "1".to_owned(),
        String::new(),
        DEFAULT_SECURITY_KEY.to_vec(),
        String::new(),
        String::new(),
        String::new(),
    )
}

fn get_recovery_rom_info(
    http: &Client,
    params: &RequestParams,
    android_version: &str,
    login_data: Option<&LoginData>,
    branch_override: Option<&str>,
    xms_version: &str,
) -> Result<Option<RomInfo>, String> {
    let (account_type, port, ssecurity, security_key, service_token, user_id, c_user_id) =
        get_security(login_data);
    let branch = branch_override.unwrap_or(&params.branch_ext);
    let json_data = generate_json(RecoveryJsonParams {
        branch,
        code_name_ext: &params.code_name_ext,
        region_code: &params.region_code,
        rom_version: &params.system_version_ext,
        android_version,
        user_id: &user_id,
        security: &ssecurity,
        token: &service_token,
        xms_version,
    });
    let encrypted = miui_encrypt(&json_data, &security_key)?;
    let diagnostics = format!(
        "port={port}, accountType={account_type}, codeNameExt={}, region={}, romVersion={}, androidVersion={}, keyLen={}, userIdPresent={}, cUserIdPresent={}, serviceTokenPresent={}, ssecurityPresent={}",
        params.code_name_ext,
        params.region_code,
        params.system_version_ext,
        android_version,
        security_key.len(),
        !user_id.is_empty(),
        !c_user_id.is_empty(),
        !service_token.is_empty(),
        !ssecurity.is_empty()
    );
    let url = if account_type != "CN" {
        INTL_RECOVERY_URL
    } else {
        CN_RECOVERY_URL
    };
    let mut req =
        http.post(url)
            .form(&[("q", encrypted), ("t", service_token.clone()), ("s", port)]);
    if !service_token.is_empty() && !c_user_id.is_empty() {
        req = req.header(
            COOKIE,
            format!("serviceToken={service_token}; uid={c_user_id}; s=1"),
        );
    }
    let body = req
        .send()
        .map_err(error_chain)?
        .text()
        .map_err(error_chain)?;
    if body.trim().is_empty() {
        return Ok(None);
    }
    let body = ensure_encrypted_response_with_context(body, Some(&diagnostics))?;
    let decrypted = miui_decrypt(&body, &security_key)?;
    let rom_info =
        serde_json::from_str::<RomInfo>(&decrypted).map_err(|e| format!("{e}: {decrypted}"))?;
    Ok(Some(rom_info))
}

fn get_xms_ver_info(
    http: &Client,
    params: &RequestParams,
    android_version: &str,
    pkgs: &[String],
    cur_ver: &str,
    lst_ver: &str,
    login_data: Option<&LoginData>,
) -> Result<Option<XmsDto>, String> {
    if pkgs.is_empty() {
        return Ok(None);
    }
    let (account_type, port, _ssecurity, security_key, service_token, user_id, c_user_id) =
        get_security(login_data);
    let payload = json!({
        "b": "F",
        "c": android_version.trim_end_matches(".0"),
        "d": params.code_name_ext,
        "rv": params.system_version_ext,
        "f": "1",
        "csv": cur_ver,
        "l": if params.code_name_ext.contains("_global") { "en_US" } else { "zh_CN" },
        "lsv": lst_ver,
        "r": params.region_code,
        "id": user_id,
        "pkgs": pkgs.iter().map(|pkg| json!({"pkg": pkg, "pkgVer": "1"})).collect::<Vec<_>>()
    })
    .to_string();
    let encrypted = miui_encrypt(&payload, &security_key)?;
    let url = if account_type != "CN" {
        INTL_GETXMSVER_URL
    } else {
        CN_GETXMSVER_URL
    };
    let ts = chrono::Utc::now().timestamp_millis().to_string();
    let mut req = http.post(url).form(&[
        ("n", Uuid::new_v4().to_string()),
        ("q", encrypted),
        ("s", port),
        ("t", service_token.clone()),
        ("ts", ts),
    ]);
    if !service_token.is_empty() && !c_user_id.is_empty() {
        req = req.header(
            COOKIE,
            format!("serviceToken={service_token}; uid={c_user_id}; s=1"),
        );
    }
    let body = req
        .send()
        .map_err(error_chain)?
        .text()
        .map_err(error_chain)?;
    if body.trim().is_empty() {
        return Ok(None);
    }
    let body = ensure_encrypted_response(body)?;
    let decrypted = miui_decrypt(&body, &security_key)?;
    serde_json::from_str::<XmsDto>(&decrypted)
        .map(Some)
        .map_err(|e| format!("{e}: {decrypted}"))
}

fn refresh_service_token(login_data: &LoginData) -> Option<LoginData> {
    let pass_token = login_data.pass_token.as_ref()?;
    let user_id = login_data.user_id.as_ref()?;
    let sid = if login_data.account_type.as_deref() == Some("GL") {
        "miuiota_intl"
    } else {
        "miuiromota"
    };
    let http = login_client().ok()?;
    let response = http
        .get(format!("{ACCOUNT_URL}/pass/serviceLogin"))
        .query(&[("sid", sid), ("_json", "true")])
        .header(COOKIE, format!("passToken={pass_token};userId={user_id}"))
        .send()
        .ok()?;
    let content = response
        .text()
        .ok()?
        .trim_start_matches("&&&START&&&")
        .to_owned();
    let value: Value = serde_json::from_str(&content).ok()?;
    let ssecurity = json_scalar_string(&value["ssecurity"])?;
    let location = json_scalar_string(&value["location"])?;
    if ssecurity.is_empty() || location.is_empty() {
        return None;
    }
    let token_response = http
        .get(format!("{location}&_userIdNeedEncrypt=true"))
        .send()
        .ok()?;
    let service_token = token_response
        .cookies()
        .find(|cookie| cookie.name() == "serviceToken" && !cookie.value().is_empty())
        .map(|cookie| cookie.value().to_owned())?;
    let mut refreshed = login_data.clone();
    refreshed.auth_result = Some("1".to_owned());
    refreshed.ssecurity = Some(ssecurity);
    refreshed.service_token = Some(service_token);
    refreshed.c_user_id =
        json_scalar_string(&value["cUserId"]).or_else(|| login_data.c_user_id.clone());
    refreshed.pass_token =
        json_scalar_string(&value["passToken"]).or_else(|| login_data.pass_token.clone());
    Some(refreshed)
}

fn expired_login_data(login_data: &LoginData) -> LoginData {
    let mut expired = login_data.clone();
    expired.auth_result = Some("3".to_owned());
    expired
}

pub fn fetch_rom_info(query: RomQuery) -> Result<QueryResult, String> {
    if query.code_name.is_empty()
        || query.android_version.is_empty()
        || query.system_version.is_empty()
    {
        return Ok(QueryResult {
            ok: false,
            message: "No information!".to_owned(),
            ..Default::default()
        });
    }
    let http = update_client()?;
    let params = build_request_params(&query);
    let mut current_login_data = query.login_data.clone();
    let mut initial_session_update = None;
    let initial_recovery = match get_recovery_rom_info(
        &http,
        &params,
        &query.android_version,
        query.login_data.as_ref(),
        None,
        "",
    ) {
        Ok(Some(recovery)) => recovery,
        Ok(None) => return Err("No network connection!".to_owned()),
        Err(error) if error.contains("code 2001") => {
            let Some(login_data) = query.login_data.as_ref() else {
                return Err(error);
            };
            let Some(refreshed) = refresh_service_token(login_data) else {
                return Err(error);
            };
            let retried = get_recovery_rom_info(
                &http,
                &params,
                &query.android_version,
                Some(&refreshed),
                None,
                "",
            )?
            .ok_or_else(|| "No network connection!".to_owned())?;
            current_login_data = Some(refreshed.clone());
            initial_session_update = Some(refreshed);
            retried
        }
        Err(error) => return Err(error),
    };
    let (recovery, session_update) = if let Some(login_data) = current_login_data.as_ref() {
        if initial_recovery.auth_result == Some(1) {
            (initial_recovery, initial_session_update)
        } else if let Some(refreshed) = refresh_service_token(login_data) {
            let retried = get_recovery_rom_info(
                &http,
                &params,
                &query.android_version,
                Some(&refreshed),
                None,
                "",
            )?
            .unwrap_or_else(|| initial_recovery.clone());
            current_login_data = Some(refreshed.clone());
            (retried, Some(refreshed))
        } else {
            let expired = expired_login_data(login_data);
            current_login_data = Some(expired.clone());
            (initial_recovery, Some(expired))
        }
    } else {
        (initial_recovery, None)
    };

    let (xms_for_build, xms_apps) = if let Some(xms) = recovery.xms_update_info.clone() {
        if xms.has_xms_update == 1 && !xms.lst_ver.clone().unwrap_or_default().is_empty() {
            let follow_up = get_recovery_rom_info(
                &http,
                &params,
                &query.android_version,
                current_login_data.as_ref(),
                None,
                xms.lst_ver.as_deref().unwrap_or_default(),
            )
            .ok()
            .flatten()
            .and_then(|it| it.xms_update_info.map(|x| x.change_log));
            let apps = get_xms_ver_info(
                &http,
                &params,
                &query.android_version,
                &xms.pkgs,
                xms.cur_ver.as_deref().unwrap_or_default(),
                xms.lst_ver.as_deref().unwrap_or_default(),
                current_login_data.as_ref(),
            )
            .ok()
            .flatten()
            .map(map_xms_apps)
            .unwrap_or_default();
            let mut updated = xms;
            if let Some(change_log) = follow_up {
                updated.change_log = change_log;
            }
            (Some(updated), apps)
        } else {
            (Some(xms), Vec::new())
        }
    } else {
        (None, Vec::new())
    };
    let image_mirror = recovery
        .file_mirror
        .as_ref()
        .map(|m| m.image.clone())
        .unwrap_or_default();
    let xms_info = map_xms_info(xms_for_build.as_ref(), &image_mirror, xms_apps);

    if recovery
        .current_rom
        .as_ref()
        .and_then(|r| r.bigversion.as_ref())
        .is_some()
    {
        let cur_download = fetch_current_download(
            &http,
            &recovery,
            &params,
            &query,
            current_login_data.as_ref(),
        )?;
        let no_ultimate_link = cur_download.no_ultimate_link();
        let (mut cur_rom_info, cur_icons, cur_images) = map_rom(
            &recovery,
            recovery.current_rom.as_ref(),
            Some(&cur_download),
        );
        let metadata_url = if no_ultimate_link {
            cur_rom_info.cdn1_download.as_str()
        } else {
            cur_rom_info.official1_download.as_str()
        };
        if !metadata_url.is_empty()
            && let Some(metadata) = get_ota_metadata(&http, metadata_url)
        {
            apply_metadata(&mut cur_rom_info, &metadata);
        }
        let inc_rom = recovery
            .increment_rom
            .as_ref()
            .or(recovery.cross_rom.as_ref());
        let (inc_rom_info, inc_icons, inc_images) = map_rom(&recovery, inc_rom, None);
        return Ok(QueryResult {
            ok: true,
            message: if no_ultimate_link {
                "Unable to get ultimate link!".to_owned()
            } else {
                "Request successful!".to_owned()
            },
            cur_rom_info,
            cur_icon_info: cur_icons,
            cur_image_info: cur_images,
            inc_rom_info,
            inc_icon_info: inc_icons,
            inc_image_info: inc_images,
            xms_info,
            no_ultimate_link,
            is_fallback: false,
            login_data: session_update,
        });
    }
    let fallback_rom = recovery
        .increment_rom
        .as_ref()
        .or(recovery.cross_rom.as_ref());
    if fallback_rom.and_then(|r| r.bigversion.as_ref()).is_some() {
        let (cur_rom_info, cur_icons, cur_images) = map_rom(&recovery, fallback_rom, None);
        return Ok(QueryResult {
            ok: true,
            message: "Requested version does not exist!".to_owned(),
            cur_rom_info,
            cur_icon_info: cur_icons,
            cur_image_info: cur_images,
            xms_info,
            is_fallback: true,
            login_data: session_update,
            ..Default::default()
        });
    }
    Ok(QueryResult {
        ok: false,
        message: "No information!".to_owned(),
        login_data: session_update,
        ..Default::default()
    })
}

#[allow(dead_code)]
pub fn fetch_rom_download_links(query: RomQuery) -> Result<RomDownloadResult, String> {
    let result = fetch_rom_info(query)?;
    if !result.ok {
        return Ok(RomDownloadResult {
            ok: false,
            message: result.message,
            login_data: result.login_data,
            ..Default::default()
        });
    }

    Ok(RomDownloadResult {
        ok: true,
        message: result.message,
        file_name: result.cur_rom_info.file_name.clone(),
        version: result.cur_rom_info.version.clone(),
        md5: result.cur_rom_info.md5.clone(),
        file_size: result.cur_rom_info.file_size.clone(),
        links: DownloadLinks {
            official1: result.cur_rom_info.official1_download,
            official2: result.cur_rom_info.official2_download,
            cdn1: result.cur_rom_info.cdn1_download,
            cdn2: result.cur_rom_info.cdn2_download,
        },
        no_ultimate_link: result.no_ultimate_link,
        login_data: result.login_data,
    })
}

fn fetch_current_download(
    http: &Client,
    recovery: &RomInfo,
    params: &RequestParams,
    query: &RomQuery,
    login_data: Option<&LoginData>,
) -> Result<ResolvedDownloadPath, String> {
    let current = recovery.current_rom.as_ref().ok_or("missing CurrentRom")?;
    if current.md5 == recovery.latest_rom.as_ref().and_then(|r| r.md5.clone()) {
        return Ok(resolve_current_download(CurrentDownloadProbe {
            current_version: current.version.as_deref(),
            current_filename: current.filename.as_deref(),
            current_md5: current.md5.as_deref(),
            latest_md5: recovery.latest_rom.as_ref().and_then(|r| r.md5.as_deref()),
            latest_filename: recovery
                .latest_rom
                .as_ref()
                .and_then(|r| r.filename.as_deref()),
            ..Default::default()
        }));
    }
    let current_probe = get_recovery_rom_info(
        http,
        params,
        &query.android_version,
        login_data,
        Some(""),
        "",
    )
    .ok()
    .flatten()
    .unwrap_or_else(|| recovery.clone());
    current_download_from_probe(recovery, &current_probe)
}

fn current_download_from_probe(
    recovery: &RomInfo,
    current_probe: &RomInfo,
) -> Result<ResolvedDownloadPath, String> {
    let current = recovery.current_rom.as_ref().ok_or("missing CurrentRom")?;
    Ok(resolve_current_download(CurrentDownloadProbe {
        current_version: current.version.as_deref(),
        current_filename: current.filename.as_deref(),
        current_md5: current.md5.as_deref(),
        latest_md5: recovery.latest_rom.as_ref().and_then(|r| r.md5.as_deref()),
        latest_filename: recovery
            .latest_rom
            .as_ref()
            .and_then(|r| r.filename.as_deref()),
        probe_current_version: current_probe
            .current_rom
            .as_ref()
            .and_then(|r| r.version.as_deref()),
        probe_latest_filename: current_probe
            .latest_rom
            .as_ref()
            .and_then(|r| r.filename.as_deref()),
    }))
}

#[derive(Debug, Clone, Copy)]
struct FileInfo {
    offset: i64,
    size: i64,
}

#[derive(Debug, Clone)]
struct CdEntry {
    local_header_offset: i64,
    uncompressed_size: i64,
    method: u16,
}

fn get_ota_metadata(http: &Client, url: &str) -> Option<OtaMetadataPb> {
    extract_ota_metadata(http, url).ok().flatten()
}

fn extract_ota_metadata(http: &Client, url: &str) -> Result<Option<OtaMetadataPb>, String> {
    let file_length = get_file_length(http, url)?.unwrap_or(0);
    if file_length <= 0 {
        return Ok(None);
    }

    let end_size = file_length.min(END_BYTES_SIZE) as usize;
    let end_bytes = read_range(http, url, file_length - end_size as i64, end_size)?;
    let cd = locate_central_directory(&end_bytes, file_length);
    if cd.offset < 0 || cd.size <= 0 || cd.offset + cd.size > file_length {
        return Ok(None);
    }

    let central_directory = read_range(http, url, cd.offset, cd.size as usize)?;
    let entries = locate_entries(&central_directory, &[METADATA_PB_PATH, METADATA_PATH]);

    if let Some(entry) = entries
        .get(METADATA_PB_PATH)
        .filter(|entry| entry.method == 0)
        && let Ok(bytes) = read_entry_bytes(http, url, entry, file_length)
        && let Ok(parsed) = OtaMetadataPb::decode(bytes.as_slice())
    {
        return Ok(Some(parsed));
    }

    if let Some(entry) = entries.get(METADATA_PATH).filter(|entry| entry.method == 0) {
        let bytes = read_entry_bytes(http, url, entry, file_length)?;
        let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
        if !text.is_empty() {
            return Ok(Some(parse_text_metadata(&text)));
        }
    }

    Ok(None)
}

fn apply_metadata(rom: &mut RomInfoData, metadata: &OtaMetadataPb) {
    let Some(post) = metadata.postcondition.as_ref() else {
        return;
    };
    rom.fingerprint = post
        .partition_state
        .iter()
        .find(|partition| partition.partition_name == "odm")
        .and_then(|partition| partition.build.first())
        .cloned()
        .or_else(|| post.build.first().cloned())
        .unwrap_or_default();
    rom.security_patch_level = post.security_patch_level.clone();
    rom.sdk_level = post.sdk_level.clone();
    rom.timestamp = if post.timestamp > 0 {
        Local
            .timestamp_opt(post.timestamp, 0)
            .single()
            .map(|dt| dt.format("%Y/%-m/%-d %H:%M:%S").to_string())
            .unwrap_or_default()
    } else {
        String::new()
    };
}

fn read_entry_bytes(
    http: &Client,
    url: &str,
    entry: &CdEntry,
    file_length: i64,
) -> Result<Vec<u8>, String> {
    let header_offset = entry.local_header_offset;
    if header_offset < 0 || header_offset >= file_length {
        return Err("Invalid ZIP local header offset".to_owned());
    }
    let max_header_read = (file_length - header_offset).min(LOCAL_HEADER_SIZE) as usize;
    if max_header_read < 30 {
        return Err("Invalid ZIP local header".to_owned());
    }
    let local_header = read_range(http, url, header_offset, max_header_read)?;
    let internal_offset = locate_local_file_offset(&local_header);
    if internal_offset < 0 || internal_offset > max_header_read as i64 {
        return Err("Invalid ZIP local file offset".to_owned());
    }
    let data_offset = header_offset + internal_offset;
    let size = entry.uncompressed_size;
    if size < 0 || size > i32::MAX as i64 || data_offset + size > file_length {
        return Err("Invalid ZIP entry size".to_owned());
    }
    read_range(http, url, data_offset, size as usize)
}

fn parse_text_metadata(text: &str) -> OtaMetadataPb {
    let mut map = HashMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once('=') {
            map.insert(key.to_owned(), value.to_owned());
        }
    }

    let precondition = if map.keys().any(|key| key.starts_with("pre-")) {
        Some(DeviceStatePb {
            device: map.get("pre-device").cloned().into_iter().collect(),
            build: map.get("pre-build").cloned().into_iter().collect(),
            build_incremental: map
                .get("pre-build-incremental")
                .cloned()
                .unwrap_or_default(),
            ..Default::default()
        })
    } else {
        None
    };
    let postcondition = Some(DeviceStatePb {
        device: map.get("post-device").cloned().into_iter().collect(),
        build: map.get("post-build").cloned().into_iter().collect(),
        build_incremental: map
            .get("post-build-incremental")
            .cloned()
            .unwrap_or_default(),
        timestamp: map
            .get("post-timestamp")
            .and_then(|it| it.parse::<i64>().ok())
            .unwrap_or_default(),
        sdk_level: map.get("post-sdk-level").cloned().unwrap_or_default(),
        security_patch_level: map
            .get("post-security-patch-level")
            .cloned()
            .unwrap_or_default(),
        ..Default::default()
    });

    OtaMetadataPb {
        ota_type: match map.get("ota-type").map(String::as_str) {
            Some("AB") => 1,
            Some("BLOCK") => 2,
            Some("BRICK") => 3,
            _ => 0,
        },
        precondition,
        postcondition,
        ..Default::default()
    }
}

fn get_file_length(http: &Client, url: &str) -> Result<Option<i64>, String> {
    let response = http
        .head(url)
        .header(RANGE, "bytes=0-0")
        .send()
        .map_err(|e| e.to_string())?;
    if let Some(content_range) = response
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|h| h.to_str().ok())
        && let Some((_, length)) = content_range.rsplit_once('/')
        && let Ok(parsed) = length.parse::<i64>()
        && parsed > 0
    {
        return Ok(Some(parsed));
    }
    if let Some(content_length) = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|h| h.to_str().ok())
        && let Ok(parsed) = content_length.parse::<i64>()
        && parsed > 0
    {
        return Ok(Some(parsed));
    }
    Ok(None)
}

fn read_range(http: &Client, url: &str, start: i64, size: usize) -> Result<Vec<u8>, String> {
    if start < 0 {
        return Err("Invalid range start".to_owned());
    }
    if size == 0 {
        return Ok(Vec::new());
    }
    let end = start + size as i64 - 1;
    let bytes = http
        .get(url)
        .header(RANGE, format!("bytes={start}-{end}"))
        .send()
        .map_err(|e| e.to_string())?
        .bytes()
        .map_err(|e| e.to_string())?;
    if bytes.len() < size {
        return Err("Short range read".to_owned());
    }
    Ok(bytes[..size].to_vec())
}

fn locate_central_directory(bytes: &[u8], file_length: i64) -> FileInfo {
    const ENDSIG: u32 = 0x06054b50;
    const ZIP64_ENDSIG: u32 = 0x06064b50;
    const ZIP64_LOCSIG: u32 = 0x07064b50;
    const ZIP64_LOCHDR: usize = 20;
    const ZIP64_MAGICVAL: u32 = 0xFFFFFFFF;

    if bytes.len() < 22 {
        return FileInfo {
            offset: -1,
            size: -1,
        };
    }
    for pos in (0..=bytes.len() - 22).rev() {
        if get_u32_le(bytes, pos) == Some(ENDSIG) {
            let offset = get_u32_le(bytes, pos + 16).unwrap_or(0);
            let size = get_u32_le(bytes, pos + 12).unwrap_or(0);
            if offset == ZIP64_MAGICVAL || size == ZIP64_MAGICVAL {
                if pos >= ZIP64_LOCHDR
                    && get_u32_le(bytes, pos - ZIP64_LOCHDR) == Some(ZIP64_LOCSIG)
                {
                    let record_offset = get_i64_le(bytes, pos - ZIP64_LOCHDR + 8).unwrap_or(-1);
                    let record_pos = bytes.len() as i64 - (file_length - record_offset);
                    if record_pos >= 0 {
                        let record_pos = record_pos as usize;
                        if record_pos + 56 <= bytes.len()
                            && get_u32_le(bytes, record_pos) == Some(ZIP64_ENDSIG)
                        {
                            return FileInfo {
                                size: get_i64_le(bytes, record_pos + 40).unwrap_or(-1),
                                offset: get_i64_le(bytes, record_pos + 48).unwrap_or(-1),
                            };
                        }
                    }
                }
            } else {
                return FileInfo {
                    offset: offset as i64,
                    size: size as i64,
                };
            }
        }
    }
    FileInfo {
        offset: -1,
        size: -1,
    }
}

fn locate_entries(bytes: &[u8], names: &[&str]) -> HashMap<String, CdEntry> {
    const CENSIG: u32 = 0x02014b50;
    let mut results = HashMap::new();
    let mut pos = 0;
    while pos + 46 <= bytes.len() {
        if get_u32_le(bytes, pos) != Some(CENSIG) {
            break;
        }
        let method = get_u16_le(bytes, pos + 10).unwrap_or(0);
        let uncompressed_size = get_u32_le(bytes, pos + 24).unwrap_or(0) as i64;
        let file_name_len = get_u16_le(bytes, pos + 28).unwrap_or(0) as usize;
        let extra_len = get_u16_le(bytes, pos + 30).unwrap_or(0) as usize;
        let comment_len = get_u16_le(bytes, pos + 32).unwrap_or(0) as usize;
        let local_header_offset = get_u32_le(bytes, pos + 42).unwrap_or(0) as i64;
        let name_start = pos + 46;
        let name_end = name_start + file_name_len;
        if name_end > bytes.len() {
            break;
        }
        let file_name = String::from_utf8_lossy(&bytes[name_start..name_end]).to_string();
        if names.contains(&file_name.as_str()) {
            results.insert(
                file_name,
                CdEntry {
                    local_header_offset,
                    uncompressed_size,
                    method,
                },
            );
            if results.len() == names.len() {
                break;
            }
        }
        pos = name_end + extra_len + comment_len;
    }
    results
}

fn locate_local_file_offset(bytes: &[u8]) -> i64 {
    const LOCSIG: u32 = 0x04034b50;
    if bytes.len() < 30 || get_u32_le(bytes, 0) != Some(LOCSIG) {
        return -1;
    }
    let file_name_len = get_u16_le(bytes, 26).unwrap_or(0) as i64;
    let extra_len = get_u16_le(bytes, 28).unwrap_or(0) as i64;
    30 + file_name_len + extra_len
}

fn get_u16_le(bytes: &[u8], pos: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(pos..pos + 2)?.try_into().ok()?,
    ))
}

fn get_u32_le(bytes: &[u8], pos: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(pos..pos + 4)?.try_into().ok()?,
    ))
}

fn get_i64_le(bytes: &[u8], pos: usize) -> Option<i64> {
    Some(i64::from_le_bytes(
        bytes.get(pos..pos + 8)?.try_into().ok()?,
    ))
}

fn map_rom(
    recovery: &RomInfo,
    rom: Option<&Rom>,
    resolved_download: Option<&ResolvedDownloadPath>,
) -> (RomInfoData, Vec<IconInfoData>, Vec<ImageInfoData>) {
    let Some(rom) = rom else {
        return Default::default();
    };
    if rom.bigversion.is_none() {
        return Default::default();
    }
    let mut log = String::new();
    for (category, items) in &rom.changelog {
        if !category.is_empty() {
            log.push_str(category);
            log.push('\n');
        }
        for item in items {
            let text = item.txt.trim_end();
            if !text.is_empty() {
                log.push_str(text);
                log.push('\n');
            }
        }
        log.push('\n');
    }
    let log = log.trim_end().to_owned();
    let groups = if log.is_empty() {
        Vec::new()
    } else {
        log.split("\n\n").collect::<Vec<_>>()
    };
    let changelog_only = groups
        .iter()
        .map(|group| group.lines().skip(1).collect::<Vec<_>>().join("\n"))
        .collect::<Vec<_>>();
    let gentle_notice = clean_gentle(
        recovery
            .gentle_notice
            .as_ref()
            .map(|g| g.text.as_str())
            .unwrap_or_default(),
    );

    let osbig = rom.osbigversion.clone().unwrap_or_default();
    let mut image_info = Vec::new();
    let mut icon_info = Vec::new();
    if !osbig.is_empty() && osbig.parse::<f32>().unwrap_or(0.0) >= 3.0 {
        let image_mirror = recovery
            .file_mirror
            .as_ref()
            .map(|m| m.image.as_str())
            .unwrap_or_default();
        for (category, items) in &rom.changelog {
            for item in items {
                let image = item.image.first();
                image_info.push(ImageInfoData {
                    title: category.clone(),
                    changelog: item.txt.clone(),
                    image_url: image_link(image_mirror, image.map(|i| i.path.as_str())),
                    image_width: image.and_then(|i| i.w.parse().ok()),
                    image_height: image.and_then(|i| i.h.parse().ok()),
                });
            }
        }
    } else {
        let icon_names = groups
            .iter()
            .map(|group| group.lines().next().unwrap_or_default().to_owned())
            .collect::<Vec<_>>();
        let icon_mirror = recovery
            .file_mirror
            .as_ref()
            .map(|m| m.icon.as_str())
            .unwrap_or_default();
        for (index, name) in icon_names.iter().enumerate() {
            icon_info.push(IconInfoData {
                icon_name: name.clone(),
                icon_link: recovery
                    .icon
                    .as_ref()
                    .and_then(|icons| icons.get(name))
                    .map(|path| format!("{}{}", icon_mirror.replace("http://", "https://"), path))
                    .unwrap_or_default(),
                changelog: changelog_only.get(index).cloned().unwrap_or_default(),
            });
        }
    }
    let bigversion = rom.bigversion.clone().unwrap_or_default();
    let big_version = if !osbig.is_empty() && osbig != ".0" && osbig != "0.0" {
        format!("HyperOS {osbig}")
    } else if bigversion.contains("816") {
        bigversion.replace("816", "HyperOS 1.0")
    } else {
        format!("MIUI {bigversion}")
    };
    let file_name = kmp_nullable_string(rom.filename.as_deref())
        .split(".zip")
        .next()
        .unwrap_or_default()
        .to_owned()
        + ".zip";
    let links = DownloadLinks::for_rom(
        rom.version.as_deref(),
        rom.filename.as_deref(),
        OfficialPath::from_resolved(resolved_download),
    );
    let rom_info = RomInfoData {
        rom_type: kmp_nullable_string(rom.rom_type.as_deref()),
        device: kmp_nullable_string(rom.device.as_deref()),
        version: kmp_nullable_string(rom.version.as_deref()),
        codebase: kmp_nullable_string(rom.codebase.as_deref()),
        branch: kmp_nullable_string(rom.branch.as_deref()),
        big_version,
        file_name,
        file_size: kmp_nullable_string(rom.filesize.as_deref()),
        md5: kmp_nullable_string(rom.md5.as_deref()),
        is_beta: rom.is_beta == 1,
        is_gov: rom.is_gov == 1,
        official1_download: links.official1,
        official2_download: links.official2,
        cdn1_download: links.cdn1,
        cdn2_download: links.cdn2,
        changelog: log,
        gentle_notice,
        ..Default::default()
    };
    (rom_info, icon_info, image_info)
}

fn map_xms_info(
    info: Option<&XmsUpdateInfo>,
    image_mirror: &str,
    apps: Vec<XmsAppInfo>,
) -> XmsInfoData {
    let Some(info) = info else {
        return XmsInfoData {
            apps,
            ..Default::default()
        };
    };
    let gentle_notice = clean_gentle(
        info.gentle_notice
            .as_ref()
            .map(|g| g.text.as_str())
            .unwrap_or_default(),
    );
    let mut changelog_items = Vec::new();
    let mut flat_log = String::new();
    for (category, items) in &info.change_log {
        let joined = items
            .iter()
            .map(|i| i.txt.trim_end())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if !category.is_empty() {
            flat_log.push_str(category);
            flat_log.push('\n');
        }
        if !joined.is_empty() {
            flat_log.push_str(&joined);
            flat_log.push('\n');
        }
        flat_log.push('\n');
        for item in items {
            let image = item.image.first();
            changelog_items.push(ImageInfoData {
                title: category.clone(),
                changelog: item.txt.clone(),
                image_url: image_link(image_mirror, image.map(|i| i.path.as_str())),
                image_width: image.and_then(|i| i.w.parse().ok()),
                image_height: image.and_then(|i| i.h.parse().ok()),
            });
        }
    }
    XmsInfoData {
        has_update: info.has_xms_update == 1,
        cur_ver: info.cur_ver.clone().unwrap_or_default(),
        lst_ver: info.lst_ver.clone().unwrap_or_default(),
        pkg_cnt: info.pkg_cnt,
        prio: info.prio.unwrap_or_default(),
        apps,
        gentle_notice,
        changelog_items,
        changelog_text: flat_log.trim_end().to_owned(),
    }
}

fn map_xms_apps(dto: XmsDto) -> Vec<XmsAppInfo> {
    dto.apk_lists
        .into_iter()
        .map(|apk| {
            let urls = apk
                .download_urls
                .iter()
                .filter_map(|raw| resolve_xms_download_url(raw, &dto.mirror_list))
                .collect::<Vec<_>>();
            XmsAppInfo {
                name: apk.name.unwrap_or_default(),
                pack_name: apk.pack_name.unwrap_or_default(),
                version_code: apk.last_ver_code.unwrap_or_default(),
                file_name: apk.file_name.unwrap_or_default(),
                file_size: apk.size.map(|s| s.to_string()).unwrap_or_default(),
                md5: apk.md5.unwrap_or_default(),
                download_urls: urls,
            }
        })
        .collect()
}

fn clean_gentle(html: &str) -> String {
    let mut text = html
        .replace("<li>", "\n· ")
        .replace("</li>", "")
        .replace("<p>", "\n")
        .replace("</p>", "")
        .replace("&nbsp;", " ")
        .replace("&#160;", "");
    while let Some(start) = text.find('<') {
        if let Some(end) = text[start..].find('>') {
            text.replace_range(start..=start + end, "");
        } else {
            break;
        }
    }
    text.trim().lines().skip(1).collect::<Vec<_>>().join("\n")
}

fn image_link(mirror: &str, path: Option<&str>) -> String {
    let base = mirror.replace("http://", "https://");
    format!("{}{}", base, path.unwrap_or_default())
}

fn kmp_nullable_string(value: Option<&str>) -> String {
    value.unwrap_or("null").to_owned()
}

fn query_param(url_text: &str, key: &str) -> Option<String> {
    let url = url::Url::parse(url_text).ok()?;
    url.query_pairs()
        .find_map(|(name, value)| (name == key).then(|| value.into_owned()))
        .filter(|value| !value.is_empty())
}

fn embedded_devices() -> Vec<Device> {
    vec![
        Device {
            device_name: "Xiaomi 17".to_owned(),
            device_code_name: "pudding".to_owned(),
            device_code: "PC".to_owned(),
        },
        Device {
            device_name: "Xiaomi 17 Pro".to_owned(),
            device_code_name: "pandora".to_owned(),
            device_code: "BL".to_owned(),
        },
        Device {
            device_name: "Xiaomi 17 Pro Max".to_owned(),
            device_code_name: "popsicle".to_owned(),
            device_code: "PB".to_owned(),
        },
        Device {
            device_name: "Xiaomi 17 Ultra".to_owned(),
            device_code_name: "nezha".to_owned(),
            device_code: "PA".to_owned(),
        },
        Device {
            device_name: "Xiaomi 17 Max".to_owned(),
            device_code_name: "byron".to_owned(),
            device_code: "AF".to_owned(),
        },
    ]
}

fn remote_devices_cached() -> Option<&'static Vec<Device>> {
    REMOTE_DEVICE_CACHE
        .get_or_init(|| {
            let body = update_client()
                .ok()?
                .get(DEVICE_LIST_URL)
                .send()
                .ok()?
                .text()
                .ok()?;
            let cleaned = clean_json_trailing_commas(&body);
            let remote: RemoteDevices = serde_json::from_str(&cleaned).ok()?;
            Some(remote.devices)
        })
        .as_ref()
}

fn region_code(name: &str) -> String {
    match name {
        "Default (CN)" => "CN",
        "GL (MI)" => "MI",
        "EEA (EU)" => "EU",
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
        _ => "",
    }
    .to_owned()
}

fn region_code_name(name: &str) -> String {
    match name {
        "GL (MI)" => "_global",
        "EEA (EU)" => "_eea_global",
        "CL" => "_cl_global",
        "GT" => "_gt_global",
        "ID" => "_id_global",
        "IN" => "_in_global",
        "JP" => "_jp_global",
        "KR" => "_kr_global",
        "LM" => "_lm_global",
        "MX" => "_mx_global",
        "RU" => "_ru_global",
        "TR" => "_tr_global",
        "TW" => "_tw_global",
        "ZA" => "_za_global",
        _ => "",
    }
    .to_owned()
}

fn carrier_code(name: &str) -> String {
    match name {
        "Default (Xiaomi)" => "XM",
        "MiStore (Demo)" => "DM",
        "DeviceLockController" => "DC",
        "AT&T" => "AT",
        "Bouygues" => "BY",
        "Claro" => "CR",
        "Entel" => "EN",
        "3HK" => "HG",
        "KDDI" => "KD",
        "Movistar" => "MS",
        "MTN" => "MT",
        "Orange" => "OR",
        "SoftBank" => "SB",
        "Altice France" => "SF",
        "Telefónica" => "TF",
        "Tigo" => "TG",
        "TIM" => "TI",
        "Vodacom" => "VC",
        "Vodafone" => "VF",
        _ => "",
    }
    .to_owned()
}

fn carrier_code_name(name: &str) -> String {
    match name {
        "DeviceLockController" => "_dc",
        "AT&T" => "_at",
        "Bouygues" => "_by",
        "Claro" => "_cr",
        "Entel" => "_en",
        "3HK" => "_hg",
        "KDDI" => "_kd",
        "Movistar" => "_ms",
        "MTN" => "_mt",
        "Orange" => "_or",
        "SoftBank" => "_ti",
        "Altice France" => "_sf",
        "Telefónica" => "_tf",
        "Tigo" => "_tg",
        "TIM" => "_tm",
        "Vodacom" => "_vc",
        "Vodafone" => "_vf",
        _ => "",
    }
    .to_owned()
}

fn android_letter(version: &str) -> Option<&'static str> {
    match version {
        "17.0" => Some("X"),
        "16.0" => Some("W"),
        "15.0" => Some("V"),
        "14.0" => Some("U"),
        "13.0" => Some("T"),
        "12.0" => Some("S"),
        "11.0" => Some("R"),
        "10.0" => Some("Q"),
        "9.0" => Some("P"),
        "8.1" | "8.0" => Some("O"),
        "7.1" | "7.0" => Some("N"),
        "6.0" => Some("M"),
        "5.1" | "5.0" => Some("L"),
        "4.4" => Some("K"),
        _ => None,
    }
}

fn device_code_of(
    devices: &[Device],
    android_version: &str,
    code_name: &str,
    region_code: &str,
    carrier_code: &str,
) -> String {
    let Some(letter) = android_letter(android_version) else {
        return String::new();
    };
    let device_code = devices
        .iter()
        .find(|d| d.device_code_name == code_name)
        .map(|d| d.device_code.clone())
        .or_else(|| {
            embedded_devices()
                .into_iter()
                .find(|d| d.device_code_name == code_name)
                .map(|d| d.device_code)
        })
        .or_else(|| {
            remote_devices_cached().and_then(|devices| {
                devices
                    .iter()
                    .find(|d| d.device_code_name == code_name)
                    .map(|d| d.device_code.clone())
            })
        });
    match device_code {
        Some(code) => format!("{letter}{code}{region_code}{carrier_code}"),
        None => String::new(),
    }
}

fn response_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|e| json!({"ok": false, "message": e.to_string()}).to_string())
}

fn error_json(message: impl ToString) -> String {
    json!({"ok": false, "message": message.to_string()}).to_string()
}

fn c_string_ptr(s: String) -> *mut c_char {
    CString::new(s)
        .unwrap_or_else(|_| CString::new(error_json("Invalid string")).unwrap())
        .into_raw()
}

fn read_c_string(ptr: *const c_char) -> Result<String, String> {
    if ptr.is_null() {
        return Err("Null pointer".to_owned());
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map(str::to_owned)
        .map_err(|e| e.to_string())
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `ptr` must be a non-null pointer previously returned by this library through
/// `CString::into_raw`, and it must be freed at most once.
pub unsafe extern "C" fn updater_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe {
            let _ = CString::from_raw(ptr);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn updater_embedded_devices() -> *mut c_char {
    c_string_ptr(response_json(
        &json!({"ok": true, "devices": embedded_devices(), "version": "embedded"}),
    ))
}

#[unsafe(no_mangle)]
pub extern "C" fn updater_refresh_devices() -> *mut c_char {
    let result = (|| -> Result<String, String> {
        let body = update_client()?
            .get(DEVICE_LIST_URL)
            .send()
            .map_err(|e| e.to_string())?
            .text()
            .map_err(|e| e.to_string())?;
        let cleaned = clean_json_trailing_commas(&body);
        let remote: RemoteDevices = serde_json::from_str(&cleaned).map_err(|e| e.to_string())?;
        Ok(response_json(
            &json!({"ok": true, "devices": remote.devices, "version": remote.version}),
        ))
    })();
    c_string_ptr(result.unwrap_or_else(error_json))
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

#[unsafe(no_mangle)]
pub extern "C" fn updater_query_rom_info(query_json: *const c_char) -> *mut c_char {
    let result = (|| -> Result<String, String> {
        let json_text = read_c_string(query_json)?;
        let mut query: RomQuery = serde_json::from_str(&json_text).map_err(|e| e.to_string())?;
        if query.devices.is_empty() {
            query.devices = embedded_devices();
        }
        Ok(response_json(&fetch_rom_info(query)?))
    })();
    c_string_ptr(result.unwrap_or_else(error_json))
}

#[unsafe(no_mangle)]
pub extern "C" fn updater_login(login_json: *const c_char) -> *mut c_char {
    let result = (|| -> Result<String, String> {
        let text = read_c_string(login_json)?;
        let value: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let account = value["account"].as_str().unwrap_or_default();
        let password = value["password"].as_str().unwrap_or_default();
        let global = value["global"].as_bool().unwrap_or(false);
        let captcha = value["captcha"]
            .as_str()
            .or_else(|| value["captCode"].as_str())
            .unwrap_or_default();
        let flag = value["flag"].as_i64();
        let ticket = value["ticket"].as_str().unwrap_or_default();
        let identity_session = value["identitySession"].as_str().unwrap_or_default();
        if account.is_empty() || password.is_empty() {
            return Ok(error_json("Account or Password empty!"));
        }
        let sid = if global { "miuiota_intl" } else { "miuiromota" };
        let locale = if global { "en_US" } else { "zh_CN" };
        let hash = format!("{:X}", md5::compute(password));
        let http = login_client()?;
        if let Some(flag) = flag {
            let verify_code = verify_two_factor_ticket(&http, identity_session, flag, ticket)?;
            if verify_code == 70014 {
                return Ok(error_json("Incorrect verification code"));
            }
            if verify_code != 0 {
                return Ok(error_json("Login verification failed!"));
            }
        }
        let mut form = vec![
            ("sid", sid),
            ("hash", &hash),
            ("user", account),
            ("_json", "true"),
            ("_locale", locale),
        ];
        if !captcha.is_empty() {
            form.push(("captCode", captcha));
        }
        let response = http
            .post(format!("{ACCOUNT_URL}/pass/serviceLoginAuth2"))
            .form(&form)
            .send()
            .map_err(|e| e.to_string())?;
        let content = response
            .text()
            .map_err(|e| e.to_string())?
            .trim_start_matches("&&&START&&&")
            .to_owned();
        let data: Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;
        if let Some(captcha_url) = data["captchaUrl"]
            .as_str()
            .filter(|s| !s.is_empty() && *s != "null")
        {
            let captcha_url = account_url(captcha_url);
            let captcha_image = http
                .get(&captcha_url)
                .send()
                .ok()
                .and_then(|response| response.bytes().ok())
                .map(|bytes| general_purpose::STANDARD.encode(bytes));
            return Ok(response_json(&json!({
                "ok": false,
                "captchaRequired": true,
                "message": "Captcha required",
                "captchaUrl": captcha_url,
                "captchaImageBase64": captcha_image.unwrap_or_default()
            })));
        }
        if let Some(notification) = data["notificationUrl"]
            .as_str()
            .filter(|s| !s.is_empty() && *s != "null")
        {
            let Some(two_factor_context) = query_param(notification, "context") else {
                return Ok(error_json("Login verification failed!"));
            };
            let list_url = notification.replace("fe/service/identity/authStart", "identity/list");
            let list_response = http.get(&list_url).send().map_err(|e| e.to_string())?;
            let identity_session = list_response
                .cookies()
                .find(|cookie| cookie.name() == "identity_session" && !cookie.value().is_empty())
                .map(|cookie| cookie.value().to_owned())
                .unwrap_or_default();
            let list_text = list_response
                .text()
                .map_err(|e| e.to_string())?
                .trim_start_matches("&&&START&&&")
                .to_owned();
            let list_json: Value = serde_json::from_str(&list_text).map_err(|e| e.to_string())?;
            if list_json["twoFactorAuth"].as_bool().unwrap_or(false) {
                return Ok(error_json(
                    "Accounts with two-factor authentication enabled are not supported",
                ));
            }
            let options = list_json["options"]
                .as_array()
                .map(|items| items.iter().filter_map(Value::as_i64).collect::<Vec<_>>())
                .unwrap_or_default();
            if options.is_empty() || identity_session.is_empty() {
                return Ok(error_json("Login verification failed!"));
            }
            return Ok(response_json(&json!({
                "ok": false,
                "twoFactorRequired": true,
                "message": "Detected two-factor authentication",
                "notificationUrl": notification,
                "twoFactorContext": two_factor_context,
                "identitySession": identity_session,
                "availableOptions": options
            })));
        }
        let ssecurity = json_scalar_string(&data["ssecurity"]).unwrap_or_default();
        let location = json_scalar_string(&data["location"]).unwrap_or_default();
        if data["result"].as_str().is_some_and(|r| r != "ok")
            || ssecurity.is_empty()
            || location.is_empty()
        {
            return Ok(error_json("Login verification failed!"));
        }
        let token_response = http
            .get(format!("{location}&_userIdNeedEncrypt=true"))
            .send()
            .map_err(|e| e.to_string())?;
        let service_token = token_response
            .cookies()
            .find(|cookie| cookie.name() == "serviceToken" && !cookie.value().is_empty())
            .map(|cookie| cookie.value().to_owned())
            .unwrap_or_default();
        if service_token.is_empty() {
            return Ok(error_json("Failed to get security key"));
        }
        let login = LoginData {
            account_type: Some(if global { "GL" } else { "CN" }.to_owned()),
            auth_result: Some("1".to_owned()),
            description: Some("成功".to_owned()),
            ssecurity: Some(ssecurity.to_owned()),
            service_token: Some(service_token),
            user_id: json_scalar_string(&data["userId"]),
            c_user_id: json_scalar_string(&data["cUserId"]),
            pass_token: json_scalar_string(&data["passToken"]),
        };
        Ok(response_json(
            &json!({"ok": true, "message": "Login successful", "loginData": login}),
        ))
    })();
    c_string_ptr(result.unwrap_or_else(error_json))
}

fn account_url(path_or_url: &str) -> String {
    if path_or_url.starts_with("http://") || path_or_url.starts_with("https://") {
        path_or_url.to_owned()
    } else if path_or_url.starts_with('/') {
        format!("{ACCOUNT_URL}{path_or_url}")
    } else {
        format!("{ACCOUNT_URL}/{path_or_url}")
    }
}

fn verify_two_factor_ticket(
    http: &Client,
    identity_session: &str,
    flag: i64,
    ticket: &str,
) -> Result<i64, String> {
    if identity_session.is_empty() || ticket.is_empty() {
        return Ok(-1);
    }
    let api_path = if flag == 4 {
        "/identity/auth/verifyPhone"
    } else {
        "/identity/auth/verifyEmail"
    };
    let response = http
        .post(format!("{ACCOUNT_URL}{api_path}"))
        .query(&[("_dc", chrono::Utc::now().timestamp_millis().to_string())])
        .header(COOKIE, identity_session_cookie(identity_session))
        .form(&[
            ("_flag", flag.to_string()),
            ("ticket", ticket.to_owned()),
            ("trust", "true".to_owned()),
            ("_json", "true".to_owned()),
        ])
        .send()
        .map_err(|e| e.to_string())?;
    let text = response
        .text()
        .map_err(|e| e.to_string())?
        .trim_start_matches("&&&START&&&")
        .to_owned();
    let value: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let code = value["code"].as_i64().unwrap_or(-1);
    if code == 0
        && let Some(location) = value["location"].as_str()
    {
        let _ = http.get(location).send();
    }
    Ok(code)
}

#[unsafe(no_mangle)]
pub extern "C" fn updater_send_ticket(ticket_json: *const c_char) -> *mut c_char {
    let result = (|| -> Result<String, String> {
        let text = read_c_string(ticket_json)?;
        let value: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let flag = value["flag"].as_i64().unwrap_or(0);
        let identity_session = value["identitySession"].as_str().unwrap_or_default();
        let icode = value["icode"]
            .as_str()
            .or_else(|| value["captcha"].as_str())
            .unwrap_or_default();
        if identity_session.is_empty() {
            return Ok(error_json(
                "Failed to send verification code, please try other verification methods!",
            ));
        }
        let api_path = if flag == 4 {
            "/identity/auth/sendPhoneTicket"
        } else {
            "/identity/auth/sendEmailTicket"
        };
        let http = login_client()?;
        let response = http
            .post(format!("{ACCOUNT_URL}{api_path}"))
            .query(&[("_dc", chrono::Utc::now().timestamp_millis().to_string())])
            .header(COOKIE, identity_session_cookie(identity_session))
            .form(&[("_json", "true"), ("retry", "0"), ("icode", icode)])
            .send()
            .map_err(|e| e.to_string())?;
        let status = response.status();
        let body = response.text().map_err(|e| e.to_string())?;
        let cleaned = body.trim_start_matches("&&&START&&&").to_owned();
        let parsed: Value = serde_json::from_str(&cleaned).unwrap_or_else(|_| json!({}));
        let code = parsed["code"].as_i64();
        let description = parsed["description"]
            .as_str()
            .or_else(|| parsed["desc"].as_str())
            .or_else(|| parsed["msg"].as_str())
            .or_else(|| parsed["message"].as_str())
            .unwrap_or_default();
        if code == Some(87001) {
            let captcha_url = parsed["captchaUrl"]
                .as_str()
                .or_else(|| parsed["info"].as_str())
                .map(account_url)
                .unwrap_or_default();
            let captcha_image = if captcha_url.is_empty() {
                None
            } else {
                http.get(&captcha_url)
                    .send()
                    .ok()
                    .and_then(|response| response.bytes().ok())
                    .map(|bytes| general_purpose::STANDARD.encode(bytes))
            };
            return Ok(response_json(&json!({
                "ok": false,
                "captchaRequired": true,
                "code": code,
                "message": "Captcha required",
                "description": description,
                "captchaUrl": captcha_url,
                "captchaImageBase64": captcha_image.unwrap_or_default(),
                "raw": cleaned
            })));
        }
        let ok = status.is_success() && code.unwrap_or(0) == 0;
        Ok(response_json(&json!({
            "ok": ok,
            "code": code,
            "message": if ok { "OK" } else { "Failed to send verification code, please try other verification methods!" },
            "description": description,
            "raw": cleaned
        })))
    })();
    c_string_ptr(result.unwrap_or_else(error_json))
}

fn identity_session_cookie(identity_session: &str) -> String {
    format!("identity_session={identity_session}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn miui_crypto_roundtrip() {
        let raw = r#"{"hello":"world"}"#;
        let encrypted = miui_encrypt(raw, DEFAULT_SECURITY_KEY).unwrap();
        let mime =
            general_purpose::STANDARD.encode(general_purpose::URL_SAFE.decode(encrypted).unwrap());
        let decrypted = miui_decrypt(&mime, DEFAULT_SECURITY_KEY).unwrap();
        assert_eq!(decrypted, raw);
    }

    #[test]
    fn login_numeric_fields_are_serialized_like_kotlin_json_primitive_content() {
        let value = json!({
            "userId": 1234567890_u64,
            "cUserId": "encrypted-user",
            "passToken": "pass"
        });

        assert_eq!(
            json_scalar_string(&value["userId"]).as_deref(),
            Some("1234567890")
        );
        assert_eq!(
            json_scalar_string(&value["cUserId"]).as_deref(),
            Some("encrypted-user")
        );
        assert_eq!(json_scalar_string(&Value::Null), None);
    }

    #[test]
    fn two_factor_requests_use_identity_session_cookie_like_kmp() {
        assert_eq!(identity_session_cookie("abc123"), "identity_session=abc123");
    }

    #[test]
    fn plaintext_server_error_is_reported_readably() {
        let err = ensure_encrypted_response(
            r#"{"code":2001,"desc":"parameter error","data":null}"#.to_owned(),
        )
        .unwrap_err();

        assert_eq!(
            err,
            "Server returned plaintext response: code 2001, parameter error"
        );
    }

    #[test]
    fn plaintext_server_error_can_include_safe_query_context() {
        let err = ensure_encrypted_response_with_context(
            r#"{"code":2001,"desc":"parameter error","data":null}"#.to_owned(),
            Some("port=2, keyLen=16, serviceTokenPresent=true"),
        )
        .unwrap_err();

        assert_eq!(
            err,
            "Server returned plaintext response: code 2001, parameter error; context: port=2, keyLen=16, serviceTokenPresent=true"
        );
    }

    #[test]
    fn security_key_decoder_accepts_url_safe_and_mime_base64() {
        let key = [251_u8; 16];
        let url_safe = general_purpose::URL_SAFE.encode(key);
        assert!(url_safe.contains('-'));
        assert_eq!(decode_security_key(&url_safe), Some(key.to_vec()));

        let mime_with_whitespace = format!(
            "{}\n{}",
            &general_purpose::STANDARD.encode(DEFAULT_SECURITY_KEY)[..12],
            &general_purpose::STANDARD.encode(DEFAULT_SECURITY_KEY)[12..]
        );
        assert_eq!(
            decode_security_key(&mime_with_whitespace),
            Some(DEFAULT_SECURITY_KEY.to_vec())
        );
    }

    #[test]
    fn request_params_match_kmp_rules() {
        let query = RomQuery {
            code_name: "pudding".to_owned(),
            device_region: "EEA (EU)".to_owned(),
            device_carrier: "Vodafone".to_owned(),
            android_version: "16.0".to_owned(),
            system_version: "OS3.0.306.0.AUTO".to_owned(),
            devices: embedded_devices(),
            ..Default::default()
        };
        let params = build_request_params(&query);
        assert_eq!(params.region_code, "EU");
        assert_eq!(params.carrier_code, "VF");
        assert_eq!(params.code_name_ext, "pudding_eea_vf_global");
        assert_eq!(params.system_version_ext, "OS3.0.306.0.WPCEUVF");
    }

    #[test]
    fn request_params_cover_demo_os1_dev_and_carrier_suffix_edges() {
        let demo = RomQuery {
            code_name: "pudding".to_owned(),
            device_region: "Default (CN)".to_owned(),
            device_carrier: "MiStore (Demo)".to_owned(),
            android_version: "16.0".to_owned(),
            system_version: "os1.0.1.0.auto".to_owned(),
            devices: embedded_devices(),
            ..Default::default()
        };
        let demo_params = build_request_params(&demo);
        assert_eq!(demo_params.code_name_ext, "pudding_demo");
        assert_eq!(demo_params.system_version_ext, "V816.0.1.0.WPCCNDM");
        assert_eq!(demo_params.branch_ext, "F");

        let dev = RomQuery {
            code_name: "pudding".to_owned(),
            device_region: "GL (MI)".to_owned(),
            device_carrier: "SoftBank".to_owned(),
            android_version: "16.0".to_owned(),
            system_version: "OS3.0.1.DEV".to_owned(),
            devices: embedded_devices(),
            ..Default::default()
        };
        let dev_params = build_request_params(&dev);
        assert_eq!(dev_params.carrier_code, "SB");
        assert_eq!(dev_params.code_name_ext, "pudding_ti_global");
        assert_eq!(dev_params.system_version_ext, "OS3.0.1.DEV");
        assert_eq!(dev_params.branch_ext, "X");
    }

    #[test]
    fn query_param_extracts_two_factor_context_like_kmp_login_flow() {
        let url = "https://account.xiaomi.com/fe/service/identity/authStart?_locale=zh_CN&context=abc%20123&sid=miuiromota";
        assert_eq!(query_param(url, "context").as_deref(), Some("abc 123"));
        assert_eq!(query_param(url, "missing"), None);
        assert_eq!(query_param("not a url", "context"), None);
    }

    #[test]
    fn text_metadata_maps_to_rom_fields() {
        let metadata = parse_text_metadata(
            "ota-type=AB\npost-build=google/device/build\npost-timestamp=1700000000\npost-sdk-level=35\npost-security-patch-level=2026-05-01\n",
        );
        let mut rom = RomInfoData::default();
        apply_metadata(&mut rom, &metadata);
        assert_eq!(rom.fingerprint, "google/device/build");
        assert_eq!(rom.sdk_level, "35");
        assert_eq!(rom.security_patch_level, "2026-05-01");
        assert!(!rom.timestamp.is_empty());
    }

    #[test]
    fn protobuf_metadata_prefers_odm_partition_fingerprint() {
        let metadata = OtaMetadataPb {
            postcondition: Some(DeviceStatePb {
                build: vec!["fallback/build".to_owned()],
                partition_state: vec![PartitionStatePb {
                    partition_name: "odm".to_owned(),
                    build: vec!["odm/build".to_owned()],
                    ..Default::default()
                }],
                timestamp: 1700000000,
                sdk_level: "35".to_owned(),
                security_patch_level: "2026-05-01".to_owned(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let bytes = metadata.encode_to_vec();
        let decoded = OtaMetadataPb::decode(bytes.as_slice()).unwrap();
        let mut rom = RomInfoData::default();
        apply_metadata(&mut rom, &decoded);
        assert_eq!(rom.fingerprint, "odm/build");
    }

    #[test]
    fn expired_session_is_serialized_in_query_result() {
        let login = LoginData {
            auth_result: Some("1".to_owned()),
            user_id: Some("10000".to_owned()),
            pass_token: Some("old".to_owned()),
            ..Default::default()
        };
        let expired = expired_login_data(&login);
        assert_eq!(expired.auth_result.as_deref(), Some("3"));
        let result = QueryResult {
            ok: false,
            message: "No information!".to_owned(),
            login_data: Some(expired),
            ..Default::default()
        };
        let json = serde_json::to_value(result).unwrap();
        assert_eq!(json["loginData"]["authResult"], "3");
    }

    #[test]
    fn expired_login_uses_public_security_context() {
        let expired = LoginData {
            auth_result: Some("3".to_owned()),
            account_type: Some("GL".to_owned()),
            ssecurity: Some(general_purpose::STANDARD.encode(b"stale-security!!")),
            service_token: Some("stale-token".to_owned()),
            user_id: Some("10000".to_owned()),
            c_user_id: Some("encrypted".to_owned()),
            ..Default::default()
        };
        let (account_type, port, ssecurity, security_key, service_token, user_id, c_user_id) =
            get_security(Some(&expired));
        assert_eq!(account_type, "CN");
        assert_eq!(port, "1");
        assert!(ssecurity.is_empty());
        assert_eq!(security_key, DEFAULT_SECURITY_KEY.to_vec());
        assert!(service_token.is_empty());
        assert!(user_id.is_empty());
        assert!(c_user_id.is_empty());
    }

    #[test]
    fn incomplete_active_login_uses_public_security_context() {
        let incomplete = LoginData {
            auth_result: Some("1".to_owned()),
            account_type: Some("CN".to_owned()),
            ssecurity: Some(general_purpose::STANDARD.encode(b"stale-security!!")),
            service_token: Some("stale-token".to_owned()),
            c_user_id: Some("encrypted".to_owned()),
            ..Default::default()
        };
        let (_account_type, port, ssecurity, security_key, service_token, user_id, c_user_id) =
            get_security(Some(&incomplete));

        assert_eq!(port, "1");
        assert!(ssecurity.is_empty());
        assert_eq!(security_key, DEFAULT_SECURITY_KEY.to_vec());
        assert!(service_token.is_empty());
        assert!(user_id.is_empty());
        assert!(c_user_id.is_empty());
    }

    #[test]
    fn active_login_with_invalid_security_key_uses_public_security_context() {
        let invalid = LoginData {
            auth_result: Some("1".to_owned()),
            account_type: Some("CN".to_owned()),
            ssecurity: Some(general_purpose::STANDARD.encode(b"too-short")),
            service_token: Some("token".to_owned()),
            user_id: Some("10000".to_owned()),
            c_user_id: Some("encrypted".to_owned()),
            ..Default::default()
        };
        let (_account_type, port, ssecurity, security_key, service_token, user_id, c_user_id) =
            get_security(Some(&invalid));

        assert_eq!(port, "1");
        assert!(ssecurity.is_empty());
        assert_eq!(security_key, DEFAULT_SECURITY_KEY.to_vec());
        assert!(service_token.is_empty());
        assert!(user_id.is_empty());
        assert!(c_user_id.is_empty());
    }

    #[test]
    fn rom_mapper_matches_kotlin_nullable_to_string_fields() {
        let rom = Rom {
            bigversion: Some("V14".to_owned()),
            ..Default::default()
        };
        let recovery = RomInfo::default();
        let (info, _, _) = map_rom(&recovery, Some(&rom), None);
        assert_eq!(info.rom_type, "null");
        assert_eq!(info.device, "null");
        assert_eq!(info.version, "null");
        assert_eq!(info.codebase, "null");
        assert_eq!(info.branch, "null");
        assert_eq!(info.file_name, "null.zip");
        assert_eq!(info.file_size, "null");
        assert_eq!(info.md5, "null");
        assert_eq!(
            info.official1_download,
            "https://ultimateota.d.miui.com/null/null"
        );
        assert_eq!(
            info.cdn1_download,
            "https://bkt-sgp-miui-ota-update-alisgp.oss-ap-southeast-1.aliyuncs.com/null/null"
        );
    }

    #[test]
    fn current_download_probe_failure_falls_back_like_kmp() {
        let recovery = RomInfo {
            current_rom: Some(Rom {
                version: Some("OS3.0.1.0.TEST".to_owned()),
                filename: Some("miui_TEST_OS3.0.1.0.zip".to_owned()),
                md5: Some("current-md5".to_owned()),
                ..Default::default()
            }),
            latest_rom: Some(Rom {
                md5: Some("different-md5".to_owned()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let download = current_download_from_probe(&recovery, &recovery).unwrap();

        assert!(download.no_ultimate_link());
        assert_eq!(download.path(), "/OS3.0.1.0.TEST/miui_TEST_OS3.0.1.0.zip");
    }

    #[test]
    fn device_list_json_cleaner_removes_trailing_commas_before_closers() {
        let raw = r#"{"devices":[{"deviceName":"A","deviceCodeName":"a","deviceCode":"AA",},	],"version":"2",}"#;
        let cleaned = clean_json_trailing_commas(raw);
        let parsed: RemoteDevices = serde_json::from_str(&cleaned).unwrap();
        assert_eq!(parsed.version, "2");
        assert_eq!(parsed.devices[0].device_code_name, "a");
        assert!(!cleaned.contains(",}"));
        assert!(!cleaned.contains(",\t]"));
    }

    #[test]
    fn changelog_deserialization_preserves_server_order() {
        #[derive(Deserialize)]
        struct Wrapper {
            #[serde(deserialize_with = "deserialize_changelog")]
            changelog: IndexMap<String, Vec<ChangelogItem>>,
        }

        let parsed: Wrapper = serde_json::from_str(
            r#"{"changelog":{"Second":[{"txt":"two"}],"First":[{"txt":"one"}],"Third":{"txt":["three"]}}}"#,
        )
        .unwrap();
        let keys = parsed.changelog.keys().cloned().collect::<Vec<_>>();
        assert_eq!(keys, vec!["Second", "First", "Third"]);
    }

    #[test]
    fn zip_directory_helpers_find_metadata_entry() {
        let mut zip = Vec::new();
        let name = METADATA_PATH.as_bytes();
        let data = b"post-build=test/build\n";
        let local_offset = zip.len() as u32;
        zip.extend_from_slice(&0x04034b50u32.to_le_bytes());
        zip.extend_from_slice(&20u16.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());
        zip.extend_from_slice(&0u32.to_le_bytes());
        zip.extend_from_slice(&(data.len() as u32).to_le_bytes());
        zip.extend_from_slice(&(data.len() as u32).to_le_bytes());
        zip.extend_from_slice(&(name.len() as u16).to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());
        zip.extend_from_slice(name);
        zip.extend_from_slice(data);

        let cd_offset = zip.len() as u32;
        zip.extend_from_slice(&0x02014b50u32.to_le_bytes());
        zip.extend_from_slice(&20u16.to_le_bytes());
        zip.extend_from_slice(&20u16.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());
        zip.extend_from_slice(&0u32.to_le_bytes());
        zip.extend_from_slice(&(data.len() as u32).to_le_bytes());
        zip.extend_from_slice(&(data.len() as u32).to_le_bytes());
        zip.extend_from_slice(&(name.len() as u16).to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());
        zip.extend_from_slice(&0u32.to_le_bytes());
        zip.extend_from_slice(&local_offset.to_le_bytes());
        zip.extend_from_slice(name);
        let cd_size = zip.len() as u32 - cd_offset;

        zip.extend_from_slice(&0x06054b50u32.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());
        zip.extend_from_slice(&1u16.to_le_bytes());
        zip.extend_from_slice(&1u16.to_le_bytes());
        zip.extend_from_slice(&cd_size.to_le_bytes());
        zip.extend_from_slice(&cd_offset.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());

        let info = locate_central_directory(&zip, zip.len() as i64);
        assert_eq!(info.offset, cd_offset as i64);
        assert_eq!(info.size, cd_size as i64);
        let entries = locate_entries(
            &zip[info.offset as usize..(info.offset + info.size) as usize],
            &[METADATA_PATH],
        );
        let entry = entries.get(METADATA_PATH).unwrap();
        assert_eq!(entry.local_header_offset, local_offset as i64);
        let data_offset = entry.local_header_offset
            + locate_local_file_offset(&zip[entry.local_header_offset as usize..]);
        assert_eq!(
            &zip[data_offset as usize..data_offset as usize + data.len()],
            data
        );
    }
}
