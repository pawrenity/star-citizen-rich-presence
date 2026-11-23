//! Update checks against GitHub Releases.
//!
//! The only requests are to api.github.com (latest release) and, when the user clicks
//! "Update now", the release's own installer and SHA256SUMS.txt. Nothing about the
//! player is sent. The installer is verified against SHA256SUMS.txt before it runs.
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

pub const LATEST_RELEASE: &str =
    "https://api.github.com/repos/pawrenity/star-citizen-rich-presence/releases/latest";
pub const INSTALLER: &str = "star-citizen-rich-presence-setup-x64.exe";
const SUMS: &str = "SHA256SUMS.txt";
/// Release downloads may only come from GitHub.
const ALLOWED_HOSTS: [&str; 3] = [
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub notes_url: String,
    pub installer_url: Option<String>,
    pub sums_url: Option<String>,
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

/// `x.y.z` from a tag such as `v3.1.2` (also accepts the old `v.2.1.0` spelling).
pub fn parse_version(tag: &str) -> Option<(u64, u64, u64)> {
    let tag = tag.trim().trim_start_matches(['v', 'V', '.']);
    let mut parts = tag.split('.').map(|p| p.parse::<u64>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

pub fn is_newer(current: &str, candidate: &str) -> bool {
    matches!((parse_version(current), parse_version(candidate)), (Some(a), Some(b)) if b > a)
}

fn allowed(url: &str) -> bool {
    url.strip_prefix("https://")
        .and_then(|rest| rest.split('/').next())
        .is_some_and(|host| ALLOWED_HOSTS.contains(&host))
}

/// The newer release described by GitHub's "latest release" JSON, if any.
pub fn parse_latest(json: &str, current: &str) -> Result<Option<Release>> {
    let release: GitHubRelease = serde_json::from_str(json).context("Unexpected release data")?;
    if release.draft || release.prerelease || !is_newer(current, &release.tag_name) {
        return Ok(None);
    }
    let asset = |name: &str| {
        release
            .assets
            .iter()
            .find(|a| a.name == name && allowed(&a.browser_download_url))
            .map(|a| a.browser_download_url.clone())
    };
    let (major, minor, patch) = parse_version(&release.tag_name).context("Bad release tag")?;
    Ok(Some(Release {
        version: format!("{major}.{minor}.{patch}"),
        notes_url: if release.html_url.starts_with("https://github.com/") {
            release.html_url.clone()
        } else {
            "https://github.com/pawrenity/star-citizen-rich-presence/releases/latest".into()
        },
        installer_url: asset(INSTALLER),
        sums_url: asset(SUMS),
    }))
}

/// The expected SHA-256 (lowercase hex) for `file` in a SHA256SUMS.txt listing.
pub fn expected_hash(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.trim().split_once(char::is_whitespace)?;
        let name = name.trim().trim_start_matches('*');
        (name == file && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

/// True when this copy was installed by the installer (it can update itself).
pub fn installed_copy() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("unins000.exe").is_file()))
        .unwrap_or(false)
}

#[cfg(windows)]
pub fn check() -> Result<Option<Release>> {
    let body = crate::net::get(LATEST_RELEASE, 1 << 20)?;
    parse_latest(
        std::str::from_utf8(&body).context("Release data is not UTF-8")?,
        env!("CARGO_PKG_VERSION"),
    )
}

/// Downloads and verifies the installer, returning its path in the temp folder.
#[cfg(windows)]
pub fn download(release: &Release) -> Result<std::path::PathBuf> {
    let (Some(installer), Some(sums)) = (&release.installer_url, &release.sums_url) else {
        bail!("This release has no installer to update with");
    };
    let sums = crate::net::get(sums, 64 << 10)?;
    let expected = expected_hash(&String::from_utf8_lossy(&sums), INSTALLER)
        .context("The release checksums do not list the installer")?;
    let data = crate::net::get(installer, 200 << 20)?;
    let actual = sha256_hex(&data)?;
    ensure!(
        actual == expected,
        "The downloaded installer failed its checksum, so it was not run"
    );
    let dir = std::env::temp_dir().join("StarCitizenRichPresence-update");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("setup-{}.exe", release.version));
    std::fs::write(&path, data).context("Could not save the installer")?;
    Ok(path)
}

/// Starts the verified installer silently in the background; it replaces the exe and auto-restarts with no user input.
#[cfg(windows)]
pub fn launch(installer: &std::path::Path) -> Result<()> {
    std::process::Command::new(installer)
        .args([
            "/SP-",
            "/VERYSILENT",
            "/SUPPRESSMSGBOXES",
            "/NORESTART",
            "/CURRENTUSER",
            "/CLOSEAPPLICATIONS",
            "/FORCECLOSEAPPLICATIONS",
            "/UPDATE",
        ])
        .spawn()
        .context("Could not start the installer")?;
    Ok(())
}

#[cfg(windows)]
fn sha256_hex(data: &[u8]) -> Result<String> {
    use windows_sys::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};
    let mut hash = [0u8; 32];
    let status = unsafe {
        BCryptHash(
            BCRYPT_SHA256_ALG_HANDLE,
            std::ptr::null(),
            0,
            data.as_ptr(),
            u32::try_from(data.len()).context("Installer too large")?,
            hash.as_mut_ptr(),
            32,
        )
    };
    ensure!(status == 0, "Could not hash the installer ({status:#x})");
    Ok(hash.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(parse_version("v3.1.10"), Some((3, 1, 10)));
        assert_eq!(parse_version("v.2.1.0"), Some((2, 1, 0)));
        assert_eq!(parse_version("3.1"), None);
        assert_eq!(parse_version("v3.1.0-beta"), None);
        assert!(is_newer("3.1.1", "v3.1.10"));
        assert!(is_newer("3.1.1", "v4.0.0"));
        assert!(!is_newer("3.1.1", "v3.1.1"));
        assert!(!is_newer("3.1.1", "v3.0.9"));
        assert!(!is_newer("3.1.1", "nonsense"));
    }

    #[test]
    fn latest_release_json_yields_only_newer_github_assets() {
        let json = r#"{
            "tag_name": "v3.2.0", "html_url": "https://github.com/pawrenity/star-citizen-rich-presence/releases/tag/v3.2.0",
            "draft": false, "prerelease": false,
            "assets": [
                {"name": "star-citizen-rich-presence-setup-x64.exe", "browser_download_url": "https://github.com/pawrenity/star-citizen-rich-presence/releases/download/v3.2.0/star-citizen-rich-presence-setup-x64.exe"},
                {"name": "SHA256SUMS.txt", "browser_download_url": "https://evil.example/SHA256SUMS.txt"}
            ]
        }"#;
        let release = parse_latest(json, "3.1.1").unwrap().unwrap();
        assert_eq!(release.version, "3.2.0");
        assert!(release.installer_url.is_some());
        // Assets hosted anywhere but GitHub are ignored.
        assert_eq!(release.sums_url, None);
        assert_eq!(parse_latest(json, "3.2.0").unwrap(), None);
        let pre = json.replace("\"prerelease\": false", "\"prerelease\": true");
        assert_eq!(parse_latest(&pre, "3.1.1").unwrap(), None);
    }

    /// Talks to GitHub: `cargo test --lib live_release -- --ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn live_release_downloads_and_verifies() {
        let body = crate::net::get(LATEST_RELEASE, 1 << 20).unwrap();
        let release = parse_latest(std::str::from_utf8(&body).unwrap(), "0.0.1")
            .unwrap()
            .expect("a published release");
        assert!(release.installer_url.is_some() && release.sums_url.is_some());
        let path = download(&release).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 1_000_000);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn checksum_lines_are_matched_by_exact_file_name() {
        let hash = "a".repeat(64);
        let sums = format!(
            "{}  star-citizen-rich-presence-windows-x64.zip\r\n{hash}  {INSTALLER}\r\n",
            "b".repeat(64)
        );
        assert_eq!(expected_hash(&sums, INSTALLER), Some(hash));
        assert_eq!(expected_hash("short  file.exe", "file.exe"), None);
        assert_eq!(expected_hash(&sums, "other.exe"), None);
    }
}
