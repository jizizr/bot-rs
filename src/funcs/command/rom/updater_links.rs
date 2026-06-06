use serde::{Deserialize, Serialize};

const ULTIMATE_OTA_HOST: &str = "https://ultimateota.d.miui.com";
const SUPER_OTA_HOST: &str = "https://superota.d.miui.com";
const ALIYUN_CDN_HOST: &str =
    "https://bkt-sgp-miui-ota-update-alisgp.oss-ap-southeast-1.aliyuncs.com";
const CDN_ORG_HOST: &str = "https://cdnorg.d.miui.com";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDownloadPath {
    path: String,
    no_ultimate_link: bool,
}

impl ResolvedDownloadPath {
    pub fn ultimate(version: Option<&str>, filename: Option<&str>) -> Self {
        Self {
            path: rom_path(version, filename),
            no_ultimate_link: false,
        }
    }

    pub fn fallback(version: Option<&str>, filename: Option<&str>) -> Self {
        Self {
            path: rom_path(version, filename),
            no_ultimate_link: true,
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn no_ultimate_link(&self) -> bool {
        self.no_ultimate_link
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CurrentDownloadProbe<'a> {
    pub current_version: Option<&'a str>,
    pub current_filename: Option<&'a str>,
    pub current_md5: Option<&'a str>,
    pub latest_md5: Option<&'a str>,
    pub latest_filename: Option<&'a str>,
    pub probe_current_version: Option<&'a str>,
    pub probe_latest_filename: Option<&'a str>,
}

pub fn resolve_current_download(probe: CurrentDownloadProbe<'_>) -> ResolvedDownloadPath {
    if probe.current_md5 == probe.latest_md5 {
        return ResolvedDownloadPath::ultimate(probe.current_version, probe.latest_filename);
    }

    if let Some(filename) = probe.probe_latest_filename {
        return ResolvedDownloadPath::ultimate(probe.probe_current_version, Some(filename));
    }

    ResolvedDownloadPath::fallback(probe.current_version, probe.current_filename)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfficialPath<'a> {
    SameAsDirect,
    Resolved(&'a str),
    Unavailable,
}

impl<'a> OfficialPath<'a> {
    pub fn from_resolved(resolved: Option<&'a ResolvedDownloadPath>) -> Self {
        match resolved {
            Some(download) if download.no_ultimate_link => Self::Unavailable,
            Some(download) => Self::Resolved(download.path()),
            None => Self::SameAsDirect,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadLinks {
    #[serde(rename = "official1Download")]
    pub official1: String,
    #[serde(rename = "official2Download")]
    pub official2: String,
    #[serde(rename = "cdn1Download")]
    pub cdn1: String,
    #[serde(rename = "cdn2Download")]
    pub cdn2: String,
}

impl DownloadLinks {
    pub fn for_rom(
        version: Option<&str>,
        filename: Option<&str>,
        official_path: OfficialPath<'_>,
    ) -> Self {
        let direct_path = rom_path(version, filename);
        let (official1, official2) = match official_path {
            OfficialPath::SameAsDirect => official_links(&direct_path),
            OfficialPath::Resolved(path) => official_links(path),
            OfficialPath::Unavailable => (String::new(), String::new()),
        };

        Self {
            official1,
            official2,
            cdn1: format!("{ALIYUN_CDN_HOST}{direct_path}"),
            cdn2: format!("{CDN_ORG_HOST}{direct_path}"),
        }
    }
}

pub fn rom_path(version: Option<&str>, filename: Option<&str>) -> String {
    format!(
        "/{}/{}",
        nullable_segment(version),
        nullable_segment(filename)
    )
}

pub fn resolve_xms_download_url(raw: &str, mirrors: &[String]) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return Some(raw.to_owned());
    }
    let mirror = mirrors.first()?.trim_end_matches('/');
    Some(format!("{mirror}/{}", raw.trim_start_matches('/')))
}

fn official_links(path: &str) -> (String, String) {
    (
        format!("{ULTIMATE_OTA_HOST}{path}"),
        format!("{SUPER_OTA_HOST}{path}"),
    )
}

fn nullable_segment(value: Option<&str>) -> &str {
    value.unwrap_or("null")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_kmp_compatible_rom_path() {
        assert_eq!(
            rom_path(Some("OS3.0.1.0"), Some("rom.zip")),
            "/OS3.0.1.0/rom.zip"
        );
        assert_eq!(rom_path(None, None), "/null/null");
    }

    #[test]
    fn builds_full_rom_download_links() {
        let links = DownloadLinks::for_rom(
            Some("OS3.0.1.0"),
            Some("rom.zip"),
            OfficialPath::SameAsDirect,
        );

        assert_eq!(
            links.official1,
            "https://ultimateota.d.miui.com/OS3.0.1.0/rom.zip"
        );
        assert_eq!(
            links.official2,
            "https://superota.d.miui.com/OS3.0.1.0/rom.zip"
        );
        assert_eq!(
            links.cdn1,
            "https://bkt-sgp-miui-ota-update-alisgp.oss-ap-southeast-1.aliyuncs.com/OS3.0.1.0/rom.zip"
        );
        assert_eq!(links.cdn2, "https://cdnorg.d.miui.com/OS3.0.1.0/rom.zip");
    }

    #[test]
    fn hides_official_links_when_ultimate_is_unavailable() {
        let resolved = ResolvedDownloadPath::fallback(Some("OS3.0.1.0"), Some("rom.zip"));
        let links = DownloadLinks::for_rom(
            Some("OS3.0.1.0"),
            Some("rom.zip"),
            OfficialPath::from_resolved(Some(&resolved)),
        );

        assert!(links.official1.is_empty());
        assert!(links.official2.is_empty());
        assert!(resolved.no_ultimate_link());
    }

    #[test]
    fn resolves_current_download_from_latest_md5_match() {
        let resolved = resolve_current_download(CurrentDownloadProbe {
            current_version: Some("OS3.0.1.0"),
            current_filename: Some("current.zip"),
            current_md5: Some("same"),
            latest_md5: Some("same"),
            latest_filename: Some("latest.zip"),
            ..Default::default()
        });

        assert!(!resolved.no_ultimate_link());
        assert_eq!(resolved.path(), "/OS3.0.1.0/latest.zip");
    }

    #[test]
    fn resolves_current_download_from_probe_latest_filename() {
        let resolved = resolve_current_download(CurrentDownloadProbe {
            current_version: Some("OS3.0.1.0"),
            current_filename: Some("current.zip"),
            current_md5: Some("current"),
            latest_md5: Some("latest"),
            probe_current_version: Some("OS3.0.2.0"),
            probe_latest_filename: Some("ultimate.zip"),
            ..Default::default()
        });

        assert!(!resolved.no_ultimate_link());
        assert_eq!(resolved.path(), "/OS3.0.2.0/ultimate.zip");
    }

    #[test]
    fn falls_back_to_direct_path_without_probe_result() {
        let resolved = resolve_current_download(CurrentDownloadProbe {
            current_version: Some("OS3.0.1.0"),
            current_filename: Some("current.zip"),
            current_md5: Some("current"),
            latest_md5: Some("latest"),
            ..Default::default()
        });

        assert!(resolved.no_ultimate_link());
        assert_eq!(resolved.path(), "/OS3.0.1.0/current.zip");
    }

    #[test]
    fn resolves_xms_download_links() {
        let mirrors = vec!["https://mirror.example.com/base/".to_owned()];

        assert_eq!(
            resolve_xms_download_url("/apk/app.apk", &mirrors).as_deref(),
            Some("https://mirror.example.com/base/apk/app.apk")
        );
        assert_eq!(
            resolve_xms_download_url("https://cdn.example.com/app.apk", &mirrors).as_deref(),
            Some("https://cdn.example.com/app.apk")
        );
        assert_eq!(resolve_xms_download_url("apk/app.apk", &[]), None);
    }
}
