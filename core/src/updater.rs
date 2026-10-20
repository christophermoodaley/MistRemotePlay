//! In-app updates from this project's GitHub releases.
//!
//! * `check` lists the repository's releases (drafts excluded, pre-releases
//!   included) and returns the newest one above the running version that
//!   has a Windows installer.
//! * `download` fetches that installer and refuses it unless its SHA-256
//!   matches the digest GitHub reports for the asset (or, for older
//!   releases, the release's SHA256SUMS.txt).
//! * `launch_installer` runs it in passive update mode; the installer
//!   replaces the app and starts it again.
//!
//! The repository is private, so requests carry a read-only GitHub token
//! the user stored in Settings (DPAPI-protected on Windows). Without one,
//! anonymous requests are tried (they work if the repository is public).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// `owner/repo` whose releases are checked; `MIST_UPDATE_REPO` overrides it.
pub const DEFAULT_REPO: &str = "christophermoodaley/MistRemotePlay";
/// Suffix of the Windows installer asset produced by the release workflow.
pub const INSTALLER_SUFFIX: &str = "_x64-setup.exe";

pub fn repo() -> String {
    std::env::var("MIST_UPDATE_REPO").ok().filter(|r| r.contains('/')).unwrap_or_else(|| DEFAULT_REPO.to_owned())
}

/// The version of this build.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct UpdateInfo {
    pub version: String,
    pub tag: String,
    pub name: String,
    pub notes: String,
    pub published_at: Option<String>,
    pub prerelease: bool,
    /// Release page (needs a GitHub login with access for private repos).
    pub html_url: String,
    pub installer: Option<InstallerAsset>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct InstallerAsset {
    pub name: String,
    pub size: u64,
    /// API URL of the asset (downloadable with the token).
    pub api_url: String,
    /// Expected SHA-256 (lowercase hex) when GitHub reports it.
    pub sha256: Option<String>,
    /// API URL of SHA256SUMS.txt in the same release, if present.
    pub sums_api_url: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("no access to the releases of {0}: add a GitHub token with read access to this repository under Settings → Updates")]
    NeedsToken(String),
    #[error(
        "GitHub did not accept the token (mistyped, expired or revoked); create a new one under Settings → Updates"
    )]
    BadToken,
    #[error("the token works but cannot see {0}: when creating it, select this repository under Repository access; if the repository belongs to someone else's account, use a classic token with the \"repo\" scope instead")]
    TokenCannotSeeRepo(String),
    #[error("GitHub's request limit was reached; try again later")]
    RateLimited,
    #[error("could not reach GitHub: {0}")]
    Network(String),
    #[error("unexpected answer from GitHub: {0}")]
    Protocol(String),
    #[error("the downloaded installer is damaged or was altered (SHA-256 {got}, expected {expected}); it was deleted")]
    ChecksumMismatch { got: String, expected: String },
    #[error("the release has no checksum for the installer, so it is not installed automatically")]
    NoChecksum,
    #[error("this release has no Windows installer")]
    NoInstaller,
    #[error("automatic installation is only available on Windows; download the new version from the release page")]
    Unsupported,
    #[error("{0}")]
    Io(String),
}

pub type Result<T> = std::result::Result<T, UpdateError>;

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    draft: bool,
    prerelease: bool,
    published_at: Option<String>,
    html_url: String,
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    size: u64,
    url: String,
    digest: Option<String>,
}

fn client() -> Result<reqwest::Client> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .user_agent(concat!("MistRemotePlay/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| UpdateError::Network(e.to_string()))
}

fn authed(req: reqwest::RequestBuilder, token: Option<&str>) -> reqwest::RequestBuilder {
    let req = req.header("X-GitHub-Api-Version", "2022-11-28");
    match token.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => req.bearer_auth(t),
        None => req,
    }
}

fn status_error(status: reqwest::StatusCode, has_token: bool, repo: &str) -> UpdateError {
    match status.as_u16() {
        401 => UpdateError::BadToken,
        403 | 429 => UpdateError::RateLimited,
        // GitHub answers 404 for private repositories it won't show.
        404 if has_token => UpdateError::TokenCannotSeeRepo(repo.to_owned()),
        404 => UpdateError::NeedsToken(repo.to_owned()),
        s => UpdateError::Protocol(format!("HTTP {s}")),
    }
}

/// `v0.1.2` / `0.1.2-beta.1` → semver; anything else is ignored.
pub fn parse_version(tag: &str) -> Option<semver::Version> {
    semver::Version::parse(tag.trim().trim_start_matches(['v', 'V'])).ok()
}

/// Pick the newest non-draft release above `current` from API releases.
fn pick(releases: Vec<ApiRelease>, current: &semver::Version) -> Option<UpdateInfo> {
    let best = releases
        .into_iter()
        .filter(|r| !r.draft)
        .filter_map(|r| parse_version(&r.tag_name).map(|v| (v, r)))
        .filter(|(v, _)| v > current)
        .max_by(|a, b| a.0.cmp(&b.0))?;
    let (version, r) = best;
    let sums = r.assets.iter().find(|a| a.name.eq_ignore_ascii_case("SHA256SUMS.txt")).map(|a| a.url.clone());
    let installer = r.assets.iter().find(|a| a.name.ends_with(INSTALLER_SUFFIX)).map(|a| InstallerAsset {
        name: a.name.clone(),
        size: a.size,
        api_url: a.url.clone(),
        sha256: a
            .digest
            .as_deref()
            .and_then(|d| d.strip_prefix("sha256:"))
            .filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
            .map(str::to_ascii_lowercase),
        sums_api_url: sums.clone(),
    });
    Some(UpdateInfo {
        version: version.to_string(),
        name: r.name.filter(|n| !n.is_empty()).unwrap_or_else(|| r.tag_name.clone()),
        tag: r.tag_name,
        notes: r.body.unwrap_or_default(),
        published_at: r.published_at,
        prerelease: r.prerelease,
        html_url: r.html_url,
        installer,
    })
}

/// Look for a release newer than `current` (default: this build).
pub async fn check(token: Option<&str>, current: Option<&str>) -> Result<Option<UpdateInfo>> {
    let repo = repo();
    let current = parse_version(current.unwrap_or(current_version()))
        .ok_or_else(|| UpdateError::Protocol("invalid current version".into()))?;
    let resp = authed(client()?.get(format!("https://api.github.com/repos/{repo}/releases?per_page=30")), token)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| UpdateError::Network(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(status_error(resp.status(), token.is_some_and(|t| !t.trim().is_empty()), &repo));
    }
    let releases: Vec<ApiRelease> = resp.json().await.map_err(|e| UpdateError::Protocol(e.to_string()))?;
    Ok(pick(releases, &current))
}

/// Find `name`'s hash in a SHA256SUMS.txt body (`<hex>  <name>` lines).
pub fn sum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let (hash, file) = l.trim().split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name && hash.len() == 64).then(|| hash.to_ascii_lowercase())
    })
}

async fn get_asset(client: &reqwest::Client, url: &str, token: Option<&str>) -> Result<reqwest::Response> {
    // The API redirects to a short-lived download URL; reqwest drops the
    // Authorization header when following it to another host.
    let resp = authed(client.get(url), token)
        .header("Accept", "application/octet-stream")
        .send()
        .await
        .map_err(|e| UpdateError::Network(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(status_error(resp.status(), token.is_some_and(|t| !t.trim().is_empty()), &repo()));
    }
    Ok(resp)
}

/// Download and verify the installer into `dir`; `progress(done, total)`.
pub async fn download(
    info: &UpdateInfo,
    token: Option<&str>,
    dir: &Path,
    mut progress: impl FnMut(u64, u64),
) -> Result<PathBuf> {
    use futures::StreamExt;
    let asset = info.installer.as_ref().ok_or(UpdateError::NoInstaller)?;
    let client = client()?;
    let expected = match (&asset.sha256, &asset.sums_api_url) {
        (Some(h), _) => h.clone(),
        (None, Some(sums_url)) => {
            let body = get_asset(&client, sums_url, token)
                .await?
                .text()
                .await
                .map_err(|e| UpdateError::Network(e.to_string()))?;
            sum_for(&body, &asset.name).ok_or(UpdateError::NoChecksum)?
        }
        (None, None) => return Err(UpdateError::NoChecksum),
    };
    std::fs::create_dir_all(dir).map_err(|e| UpdateError::Io(e.to_string()))?;
    // Never trust a name from the network as a path.
    let safe_name: String = asset.name.chars().filter(|c| c.is_ascii_alphanumeric() || "._-".contains(*c)).collect();
    let path = dir.join(if safe_name.ends_with(".exe") { safe_name } else { "MistRemotePlay-setup.exe".into() });
    let resp = get_asset(&client, &asset.api_url, token).await?;
    let total = resp.content_length().unwrap_or(asset.size);
    let mut file = std::fs::File::create(&path).map_err(|e| UpdateError::Io(e.to_string()))?;
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| UpdateError::Network(e.to_string()))?;
        hasher.update(&chunk);
        std::io::Write::write_all(&mut file, &chunk).map_err(|e| UpdateError::Io(e.to_string()))?;
        done += chunk.len() as u64;
        progress(done, total);
    }
    drop(file);
    let got = hex::encode(hasher.finalize());
    if got != expected {
        let _ = std::fs::remove_file(&path);
        return Err(UpdateError::ChecksumMismatch { got, expected });
    }
    Ok(path)
}

/// Start the downloaded installer in passive update mode (progress bar, no
/// questions; Windows asks for administrator approval). The installer closes
/// the running app, replaces it and starts it again (`/R`). The caller
/// should exit right after this returns.
#[cfg(windows)]
pub fn launch_installer(path: &Path) -> Result<()> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    // The installer needs administrator rights. CreateProcess (what
    // std::process::Command uses) refuses such programs with error 740;
    // ShellExecute shows the UAC prompt instead.
    let file = HSTRING::from(path.as_os_str());
    let args = HSTRING::from("/P /R /UPDATE");
    let r = unsafe { ShellExecuteW(None, &HSTRING::from("runas"), &file, &args, PCWSTR::null(), SW_SHOWNORMAL) };
    // Values above 32 mean success; below are SE_ERR_* codes.
    match r.0 as isize {
        n if n > 32 => Ok(()),
        5 => Err(UpdateError::Io("the installer was not allowed to run (administrator approval declined)".into())),
        n => Err(UpdateError::Io(format!("cannot start the installer (ShellExecute error {n})"))),
    }
}

/// Where downloads go: a per-user cache folder.
pub fn download_dir() -> PathBuf {
    directories::ProjectDirs::from("org", "Mist", "MistRemotePlay")
        .map(|d| d.cache_dir().join("updates"))
        .unwrap_or_else(|| std::env::temp_dir().join("mist-updates"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(tag: &str, draft: bool, assets: Vec<ApiAsset>) -> ApiRelease {
        ApiRelease {
            tag_name: tag.into(),
            name: None,
            body: Some(format!("notes {tag}")),
            draft,
            prerelease: true,
            published_at: None,
            html_url: format!("https://example/{tag}"),
            assets,
        }
    }

    fn installer(tag: &str, digest: Option<&str>) -> ApiAsset {
        ApiAsset {
            name: format!("Mist.Remote.Play_{}{INSTALLER_SUFFIX}", tag.trim_start_matches('v')),
            size: 10,
            url: format!("https://api/{tag}/exe"),
            digest: digest.map(String::from),
        }
    }

    #[test]
    fn picks_newest_newer_non_draft_release() {
        let h = format!("sha256:{}", "ab".repeat(32));
        let releases = vec![
            rel("v0.1.0", false, vec![installer("v0.1.0", Some(&h))]),
            rel("v0.1.10", false, vec![installer("v0.1.10", Some(&h))]),
            rel("v0.1.9", false, vec![]),
            rel("v0.2.0", true, vec![installer("v0.2.0", Some(&h))]),
            rel("nightly", false, vec![]),
        ];
        let cur = parse_version("0.1.0").unwrap();
        let u = pick(releases, &cur).unwrap();
        assert_eq!(u.version, "0.1.10", "semver order, drafts skipped");
        let i = u.installer.unwrap();
        assert_eq!(i.sha256.as_deref(), Some("ab".repeat(32).as_str()));
        assert!(pick(vec![rel("v0.1.0", false, vec![])], &cur).is_none(), "same version is no update");
    }

    #[test]
    fn prerelease_versions_order_correctly() {
        let cur = parse_version("0.2.0-beta.1").unwrap();
        assert!(parse_version("v0.2.0").unwrap() > cur);
        assert!(parse_version("0.2.0-beta.2").unwrap() > cur);
        assert!(parse_version("garbage").is_none());
    }

    /// Serve `body` for any request on a local port (minimal HTTP/1.1).
    async fn serve_once(body: &'static [u8]) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(body).await;
            }
        });
        format!("http://{addr}/asset")
    }

    fn info_for(url: String, sha256: &str) -> UpdateInfo {
        UpdateInfo {
            version: "9.9.9".into(),
            tag: "v9.9.9".into(),
            name: "test".into(),
            notes: String::new(),
            published_at: None,
            prerelease: false,
            html_url: String::new(),
            installer: Some(InstallerAsset {
                name: "Mist.Remote.Play_9.9.9_x64-setup.exe".into(),
                size: 11,
                api_url: url,
                sha256: Some(sha256.into()),
                sums_api_url: None,
            }),
        }
    }

    #[tokio::test]
    async fn download_verifies_checksum_and_deletes_bad_files() {
        let dir = std::env::temp_dir().join(format!("mist-upd-test-{}", std::process::id()));
        let body: &'static [u8] = b"installer!!";
        let good = hex::encode(Sha256::digest(body));
        let url = serve_once(body).await;

        let mut seen = 0;
        let path = download(&info_for(url.clone(), &good), None, &dir, |d, _| seen = d).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), body);
        assert_eq!(seen, body.len() as u64);

        let wrong = "00".repeat(32);
        match download(&info_for(url, &wrong), None, &dir, |_, _| {}).await {
            Err(UpdateError::ChecksumMismatch { got, expected }) => {
                assert_eq!(got, good);
                assert_eq!(expected, wrong);
            }
            other => panic!("expected a checksum mismatch, got {other:?}"),
        }
        assert!(!path.exists(), "a file that fails verification must be deleted");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reads_sha256sums() {
        let sums =
            format!("{}  Mist.Remote.Play_0.1.1_x64-setup.exe\n{}  other.tar.gz\n", "AA".repeat(32), "bb".repeat(32));
        assert_eq!(sum_for(&sums, "Mist.Remote.Play_0.1.1_x64-setup.exe"), Some("aa".repeat(32)));
        assert_eq!(sum_for(&sums, "missing.exe"), None);
    }
}
