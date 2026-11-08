//! In-app update, the RAF way: ask only the project's GitLab release list, trust a release only
//! when its `SHA256SUMS` (first line `# version X.Y.Z`) is signed with the project's Ed25519 key
//! (public key below), download the installer to a private folder, check its SHA-256 against the
//! signed list, then run it and quit.
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::data::download::{self, http_client};
use crate::data::Cancel;
use crate::types::{AppUpdate, ProgressFn, ProgressPhase};

/// Releases: GitHub first (the project's home since 2026-11), GitLab as the fallback (kept in
/// parallel; also what versions before 2.0.6 look at). Both carry the same signed SHA256SUMS.
pub const RELEASES_API_GITHUB: &str = "https://api.github.com/repos/yossi8765/MapsOffline/releases?per_page=20";
pub const RELEASES_API: &str = "https://gitlab.com/api/v4/projects/RecoveryAdvancedFree%2FMapsOffline/releases?per_page=20";
pub const RELEASES_PAGE: &str = "https://github.com/yossi8765/MapsOffline/releases";
pub const SETUP_NAME: &str = "MapsOffline-setup.exe";
pub const PORTABLE_NAME: &str = "MapsOffline-portable.zip";
/// Linux release files: the AppImage replaces itself; the deb opens in the system's package installer.
pub const LINUX_APPIMAGE_NAME: &str = "MapsOffline-linux.AppImage";
pub const LINUX_DEB_NAME: &str = "MapsOffline-linux.deb";
/// Mac: the disk image (built by GitHub Actions, .github/workflows/build.yml); it opens in Finder.
pub const MAC_DMG_NAME: &str = "MapsOffline-mac.dmg";
pub const SUMS_NAME: &str = "SHA256SUMS";
pub const SIG_NAME: &str = "SHA256SUMS.sig";
/// Raw Ed25519 public key of the release signing key (tools/release.ps1 signs with the private one).
pub const PUBLIC_KEY_HEX: &str = "afc64b6bbd922148d7b36deb20904c29435f6cfc155bbd93b262e299dcb7f62d";
const SMALL_FILE_MAX: usize = 64 * 1024;

pub type Version = (u32, u32, u32);

/// The release file this build updates itself from.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn install_name() -> &'static str {
    SETUP_NAME
}

#[cfg(target_os = "macos")]
pub fn install_name() -> &'static str {
    MAC_DMG_NAME
}

/// Linux: the AppImage when running from one (`APPIMAGE` is its path), else the deb package.
#[cfg(target_os = "linux")]
pub fn install_name() -> &'static str {
    if std::env::var_os("APPIMAGE").is_some() {
        LINUX_APPIMAGE_NAME
    } else {
        LINUX_DEB_NAME
    }
}

pub fn parse_version(tag: &str) -> Option<Version> {
    let s = tag.trim().trim_start_matches(['v', 'V']);
    let mut it = s.split('.');
    let a = it.next()?.parse().ok()?;
    let b = it.next()?.parse().ok()?;
    let c = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((a, b, c))
}

pub fn current_version() -> Version {
    parse_version(env!("CARGO_PKG_VERSION")).unwrap_or((0, 0, 0))
}

fn hex_to_bytes(h: &str) -> Option<Vec<u8>> {
    if !h.len().is_multiple_of(2) {
        return None;
    }
    (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).ok()).collect()
}

/// The signed checksum list: version line + `sha256  name` entries.
pub struct SignedSums {
    pub version: Version,
    pub hashes: Vec<(String, [u8; 32])>,
}

/// Verify `sums` against `signature` with the embedded key and parse it. Any deviation = None.
pub fn verify_sums(sums: &[u8], signature: &[u8], public_key_hex: &str) -> Option<SignedSums> {
    let pk = hex_to_bytes(public_key_hex)?;
    let key = VerifyingKey::from_bytes(pk.as_slice().try_into().ok()?).ok()?;
    let sig = Signature::from_slice(signature).ok()?;
    key.verify(sums, &sig).ok()?;
    let text = std::str::from_utf8(sums).ok()?;
    let mut lines = text.lines();
    let first = lines.next()?.trim();
    let version = parse_version(first.strip_prefix("# version")?.trim())?;
    let mut hashes = Vec::new();
    for l in lines {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let (h, name) = l.split_once(char::is_whitespace)?;
        let name = name.trim().trim_start_matches('*');
        let bytes = hex_to_bytes(h)?;
        if bytes.len() != 32 {
            return None;
        }
        hashes.push((name.to_string(), bytes.try_into().ok()?));
    }
    Some(SignedSums { version, hashes })
}

impl SignedSums {
    pub fn hash_of(&self, name: &str) -> Option<[u8; 32]> {
        self.hashes.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, h)| *h)
    }
}

fn small_file(client: &reqwest::blocking::Client, url: &str) -> anyhow::Result<Vec<u8>> {
    let r = client.get(url).timeout(REQUEST_TIMEOUT).send()?;
    if r.status().as_u16() == 418 {
        bail!("{}", download::blocked_message(url));
    }
    let r = r.error_for_status()?;
    let b = r.bytes()?;
    if b.len() > SMALL_FILE_MAX {
        bail!("{url}: unexpectedly large");
    }
    Ok(b.to_vec())
}

struct Candidate {
    version: Version,
    tag: String,
    page: String,
    notes: String,
    links: Vec<(String, String)>, // (name, direct_asset_url)
}

fn parse_releases(json: &str) -> anyhow::Result<Vec<Candidate>> {
    let v: serde_json::Value = serde_json::from_str(json)?;
    let arr = v.as_array().ok_or_else(|| anyhow!("releases: not an array"))?;
    let mut out = Vec::new();
    for r in arr {
        let flag = |k: &str| r.get(k).and_then(|u| u.as_bool()) == Some(true);
        if flag("upcoming_release") || flag("draft") || flag("prerelease") {
            continue;
        }
        let tag = r.get("tag_name").and_then(|t| t.as_str()).unwrap_or("");
        let Some(version) = parse_version(tag) else { continue };
        // GitHub: "assets" = [{name, browser_download_url}]; GitLab: "assets.links" = [{name, direct_asset_url}]
        let github = r.get("assets").is_some_and(|a| a.is_array());
        let links = if github {
            r.get("assets")
                .and_then(|l| l.as_array())
                .map(|l| {
                    l.iter()
                        .filter_map(|x| Some((x.get("name")?.as_str()?.to_string(), x.get("browser_download_url")?.as_str()?.to_string())))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            r.pointer("/assets/links")
                .and_then(|l| l.as_array())
                .map(|l| {
                    l.iter()
                        .filter_map(|x| Some((x.get("name")?.as_str()?.to_string(), x.get("direct_asset_url")?.as_str()?.to_string())))
                        .collect()
                })
                .unwrap_or_default()
        };
        let page = if github { r.get("html_url") } else { r.pointer("/_links/self") };
        let notes = if github { r.get("body") } else { r.get("description") };
        out.push(Candidate {
            version,
            tag: tag.to_string(),
            page: page.and_then(|s| s.as_str()).unwrap_or(RELEASES_PAGE).to_string(),
            notes: notes.and_then(|d| d.as_str()).unwrap_or("").to_string(),
            links,
        });
    }
    out.sort_by_key(|c| std::cmp::Reverse(c.version));
    Ok(out)
}

/// Link whose display name starts with `name` (the release UI appends descriptions after the name).
fn link<'a>(links: &'a [(String, String)], name: &str) -> Option<&'a str> {
    links
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name) || n.to_ascii_lowercase().starts_with(&format!("{} ", name.to_ascii_lowercase())))
        .map(|(_, u)| u.as_str())
}

/// One request of the check: short, so a slow or failing server does not keep the
/// "checking" state for minutes.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// Releases tried per source when the newest ones are unsigned / incomplete.
const MAX_CANDIDATES: usize = 3;

/// The release list of one source.
fn releases_json(client: &reqwest::blocking::Client, url: &str) -> anyhow::Result<String> {
    let r = client.get(url).header("Accept", "application/vnd.github+json").timeout(REQUEST_TIMEOUT).send()?;
    if r.status().as_u16() == 418 {
        bail!("{}", download::blocked_message(url));
    }
    let json = r.error_for_status()?.text()?;
    if !parse_releases(&json).is_ok_and(|c| !c.is_empty()) {
        bail!("{url}: no releases");
    }
    Ok(json)
}

/// Newest signed release, compared with the running version: GitHub, else GitLab (a network
/// filter or an outage may hit one of them). Blocking (~1 s online, at most ~2 min worst case).
pub fn check() -> anyhow::Result<AppUpdate> {
    let client = http_client()?;
    let mut problem: Option<anyhow::Error> = None;
    for url in [RELEASES_API_GITHUB, RELEASES_API] {
        match releases_json(&client, url).and_then(|json| check_source(&client, &json)) {
            Ok(u) => return Ok(u),
            Err(e) => {
                log::warn!("update check {url}: {e:#}");
                problem = Some(e);
            }
        }
    }
    Err(problem.unwrap_or_else(|| anyhow!("no release source")).context("לא נמצא שחרור חתום"))
}

fn check_source(client: &reqwest::blocking::Client, json: &str) -> anyhow::Result<AppUpdate> {
    let current = current_version();
    let mut last_problem: Option<String> = None;
    for c in parse_releases(json)?.into_iter().take(MAX_CANDIDATES) {
        let (Some(sums_url), Some(sig_url)) = (link(&c.links, SUMS_NAME), link(&c.links, SIG_NAME)) else {
            last_problem = Some(format!("{}: unsigned release", c.tag));
            continue;
        };
        // a network failure goes to the other source (not to older releases of this one: they
        // sit on the same server, and an older release would hide the newest one)
        let sums = small_file(client, sums_url).with_context(|| c.tag.clone())?;
        let sig = small_file(client, sig_url).with_context(|| c.tag.clone())?;
        let Some(signed) = verify_sums(&sums, &sig, PUBLIC_KEY_HEX) else {
            log::warn!("release {}: signature check failed", c.tag);
            last_problem = Some(format!("{}: bad signature", c.tag));
            continue;
        };
        if signed.version != c.version {
            log::warn!("release {}: signed version {:?} differs from the tag", c.tag, signed.version);
            last_problem = Some(format!("{}: signed version differs from tag", c.tag));
            continue;
        }
        let setup_url = link(&c.links, install_name()).map(str::to_string);
        if setup_url.is_none() || signed.hash_of(install_name()).is_none() {
            last_problem = Some(format!("{}: no signed installer", c.tag));
            continue;
        }
        let vs = |v: Version| format!("{}.{}.{}", v.0, v.1, v.2);
        return Ok(AppUpdate {
            current: vs(current),
            latest: vs(c.version),
            available: c.version > current,
            page_url: c.page,
            setup_url,
            portable_url: link(&c.links, PORTABLE_NAME).map(str::to_string),
            notes: c.notes,
            checked_iso: chrono::Local::now().to_rfc3339(),
            error: None,
        });
    }
    Err(anyhow!("{}", last_problem.unwrap_or_else(|| "no signed release found".into())))
}

/// Download folder: %LOCALAPPDATA%\MapsOffline\update (recreated on every download).
pub fn update_dir() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("MapsOffline").join("update")
}

fn sha256_file(p: &Path) -> anyhow::Result<[u8; 32]> {
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(h.finalize().into())
}

/// Download the installer of the newest signed release and verify it. Returns the file path.
pub fn download(progress: ProgressFn, cancel: &Cancel) -> anyhow::Result<PathBuf> {
    let client = http_client()?;
    // the newest signed release: GitHub, else GitLab
    let newest = |url: &str| -> anyhow::Result<(Candidate, SignedSums)> {
        let json = releases_json(&client, url)?;
        let c = parse_releases(&json)?.into_iter().find(|c| link(&c.links, SUMS_NAME).is_some() && link(&c.links, SIG_NAME).is_some()).ok_or_else(|| anyhow!("no signed release"))?;
        let sums = small_file(&client, link(&c.links, SUMS_NAME).unwrap())?;
        let sig = small_file(&client, link(&c.links, SIG_NAME).unwrap())?;
        let signed = verify_sums(&sums, &sig, PUBLIC_KEY_HEX).ok_or_else(|| anyhow!("חתימת השחרור אינה תקינה"))?;
        Ok((c, signed))
    };
    let (c, signed) = match newest(RELEASES_API_GITHUB) {
        Ok(x) => x,
        Err(e) => {
            log::warn!("update download (GitHub): {e:#}");
            newest(RELEASES_API)?
        }
    };
    if signed.version != c.version || signed.version <= current_version() {
        bail!("הגרסה החתומה אינה חדשה מהמותקנת");
    }
    let expected = signed.hash_of(install_name()).ok_or_else(|| anyhow!("המתקין אינו ברשימה החתומה"))?;
    let url = link(&c.links, install_name()).ok_or_else(|| anyhow!("no installer link"))?;
    let dir = update_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(format!("MapsOffline_{}.{}.{}_{}", c.version.0, c.version.1, c.version.2, if install_name() == SETUP_NAME { "x64-setup.exe" } else { install_name() }));
    download::ensure_fresh(url, &dest, progress, cancel)?;
    progress(ProgressPhase::Finalizing, 0, 0, "מאמת את הקובץ שהורד");
    if sha256_file(&dest)? != expected {
        let _ = std::fs::remove_dir_all(&dir);
        bail!("טביעת האצבע של הקובץ שהורד אינה תואמת לרשימה החתומה");
    }
    *VERIFIED.lock() = Some((dest.canonicalize()?, expected));
    Ok(dest)
}

/// The installer `download` verified in this session, by canonical path and SHA-256.
static VERIFIED: parking_lot::Mutex<Option<(PathBuf, [u8; 32])>> = parking_lot::Mutex::new(None);

/// Start the installer detached (the caller quits the app right after). Only the exact file
/// `download` verified against the signed list may run, and its hash is checked again right
/// before it starts: the path comes from the webview, and the file sat on disk in between.
pub fn launch_installer(path: &Path) -> anyhow::Result<()> {
    let canon = path.canonicalize().context("installer not found")?;
    let dir = update_dir().canonicalize().context("update folder not found")?;
    let Some((ok_path, ok_hash)) = VERIFIED.lock().clone() else { bail!("UPDATE_FILE_INVALID") };
    if !canon.starts_with(&dir) || canon != ok_path || sha256_file(&canon)? != ok_hash {
        bail!("UPDATE_FILE_INVALID");
    }
    #[cfg(target_os = "linux")]
    linux_install(&canon)?;
    // Mac: the disk image opens in Finder (drag the new app over the old one)
    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(&canon).spawn().context("open")?;
    // Tauri NSIS flags, as RAF's "/VERYSILENT /RELAUNCH": /P = passive (a small progress window,
    // no pages or questions), /UPDATE = the old version's uninstaller runs in update mode, which
    // never deletes app data (the "delete app data" choice is skipped), /R = start the app again
    // when the install is done. The data folder itself is outside the install folder anyway.
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    std::process::Command::new(&canon)
        .args(["/P", "/UPDATE", "/R"])
        .spawn()
        .with_context(|| format!("starting {}", canon.display()))?;
    Ok(())
}

/// Linux: an AppImage is swapped for the new one (the running program keeps its open copy) and
/// started again once this one has quit (single instance: an overlap would hand over and close);
/// a deb opens in the desktop's package installer.
#[cfg(target_os = "linux")]
fn linux_install(file: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    match std::env::var_os("APPIMAGE").map(PathBuf::from) {
        Some(app) => {
            let tmp = app.with_extension("new");
            std::fs::copy(file, &tmp).with_context(|| format!("writing {}", tmp.display()))?;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
            std::fs::rename(&tmp, &app)?;
            std::process::Command::new("sh").arg("-c").arg("sleep 2; exec \"$0\"").arg(&app).spawn()?;
        }
        None => {
            std::process::Command::new("xdg-open").arg(file).spawn().context("xdg-open")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn versions() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.10.0"), Some((1, 10, 0)));
        assert_eq!(parse_version("v1.2"), None);
        assert_eq!(parse_version("v1.2.3-beta"), None);
        assert!((1, 1, 0) > (1, 0, 9));
    }

    #[test]
    fn signed_sums_roundtrip_and_tamper() {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let pk_hex: String = sk.verifying_key().to_bytes().iter().map(|b| format!("{b:02x}")).collect();
        let sums = b"# version 1.2.3\n".to_vec();
        let mut sums = sums;
        sums.extend_from_slice(format!("{}  {}\n", "ab".repeat(32), SETUP_NAME).as_bytes());
        let sig = sk.sign(&sums).to_bytes();
        let s = verify_sums(&sums, &sig, &pk_hex).expect("valid");
        assert_eq!(s.version, (1, 2, 3));
        assert_eq!(s.hash_of(SETUP_NAME), Some([0xab; 32]));
        // tampered content, wrong key, missing version line
        let mut bad = sums.clone();
        bad[2] = b'X';
        assert!(verify_sums(&bad, &sig, &pk_hex).is_none());
        assert!(verify_sums(&sums, &sig, PUBLIC_KEY_HEX).is_none());
        let noversion = b"ab\n".to_vec();
        let sig2 = sk.sign(&noversion).to_bytes();
        assert!(verify_sums(&noversion, &sig2, &pk_hex).is_none());
    }

    #[test]
    fn release_list_parsing() {
        let json = r#"[{"tag_name":"v1.1.0","description":"notes","assets":{"links":[{"name":"SHA256SUMS","direct_asset_url":"u1"},{"name":"MapsOffline-setup.exe (installer)","direct_asset_url":"u2"}]}},
            {"tag_name":"v1.0.0-beta","assets":{"links":[]}},{"tag_name":"v0.9.0","upcoming_release":true,"assets":{"links":[]}}]"#;
        let c = parse_releases(json).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].version, (1, 1, 0));
        assert_eq!(link(&c[0].links, SETUP_NAME), Some("u2"));
        assert_eq!(link(&c[0].links, SIG_NAME), None);
    }

    #[test]
    fn github_releases() {
        let json = r#"[
          {"tag_name":"v2.1.0","draft":true,"prerelease":false,"assets":[]},
          {"tag_name":"v2.0.6","draft":false,"prerelease":false,"html_url":"https://github.com/x/y/releases/tag/v2.0.6","body":"notes",
           "assets":[{"name":"SHA256SUMS","browser_download_url":"https://g/s"},{"name":"MapsOffline-setup.exe","browser_download_url":"https://g/e"}]}
        ]"#;
        let c = parse_releases(json).unwrap();
        assert_eq!(c.len(), 1, "drafts are skipped");
        assert_eq!(c[0].tag, "v2.0.6");
        assert_eq!(c[0].notes, "notes");
        assert_eq!(c[0].page, "https://github.com/x/y/releases/tag/v2.0.6");
        assert_eq!(link(&c[0].links, SETUP_NAME), Some("https://g/e"));
        assert_eq!(link(&c[0].links, SUMS_NAME), Some("https://g/s"));
    }
}
