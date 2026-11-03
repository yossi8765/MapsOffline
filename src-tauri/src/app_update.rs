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

pub const RELEASES_API: &str = "https://gitlab.com/api/v4/projects/RecoveryAdvancedFree%2FMapsOffline/releases?per_page=20";
pub const RELEASES_PAGE: &str = "https://gitlab.com/RecoveryAdvancedFree/MapsOffline/-/releases";
pub const SETUP_NAME: &str = "MapsOffline-setup.exe";
pub const PORTABLE_NAME: &str = "MapsOffline-portable.zip";
pub const SUMS_NAME: &str = "SHA256SUMS";
pub const SIG_NAME: &str = "SHA256SUMS.sig";
/// Raw Ed25519 public key of the release signing key (tools/release.ps1 signs with the private one).
pub const PUBLIC_KEY_HEX: &str = "afc64b6bbd922148d7b36deb20904c29435f6cfc155bbd93b262e299dcb7f62d";
const SMALL_FILE_MAX: usize = 64 * 1024;

pub type Version = (u32, u32, u32);

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
    let r = client.get(url).send()?;
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
        if r.get("upcoming_release").and_then(|u| u.as_bool()) == Some(true) {
            continue;
        }
        let tag = r.get("tag_name").and_then(|t| t.as_str()).unwrap_or("");
        let Some(version) = parse_version(tag) else { continue };
        let links = r
            .pointer("/assets/links")
            .and_then(|l| l.as_array())
            .map(|l| {
                l.iter()
                    .filter_map(|x| Some((x.get("name")?.as_str()?.to_string(), x.get("direct_asset_url")?.as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        out.push(Candidate {
            version,
            tag: tag.to_string(),
            page: r.pointer("/_links/self").and_then(|s| s.as_str()).unwrap_or(RELEASES_PAGE).to_string(),
            notes: r.get("description").and_then(|d| d.as_str()).unwrap_or("").to_string(),
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

/// Newest signed release, compared with the running version. Blocking (~1 s online).
pub fn check() -> anyhow::Result<AppUpdate> {
    let client = http_client()?;
    let r = client.get(RELEASES_API).send()?;
    if r.status().as_u16() == 418 {
        bail!("{}", download::blocked_message(RELEASES_API));
    }
    let json = r.error_for_status().context("GitLab releases")?.text()?;
    let current = current_version();
    let mut last_problem: Option<String> = None;
    for c in parse_releases(&json)? {
        let (Some(sums_url), Some(sig_url)) = (link(&c.links, SUMS_NAME), link(&c.links, SIG_NAME)) else {
            last_problem = Some(format!("{}: unsigned release", c.tag));
            continue;
        };
        // a transient failure on one release must not end the whole check (5.30)
        let (sums, sig) = match (small_file(&client, sums_url), small_file(&client, sig_url)) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(e), _) | (_, Err(e)) => {
                log::warn!("release {}: {e:#}", c.tag);
                last_problem = Some(format!("{}: {e}", c.tag));
                continue;
            }
        };
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
        let setup_url = link(&c.links, SETUP_NAME).map(str::to_string);
        if setup_url.is_none() || signed.hash_of(SETUP_NAME).is_none() {
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
    Err(anyhow!("{}", last_problem.unwrap_or_else(|| "no signed release found".into())).context("לא נמצא שחרור חתום"))
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
    let r = client.get(RELEASES_API).send()?;
    if r.status().as_u16() == 418 {
        bail!("{}", download::blocked_message(RELEASES_API));
    }
    let json = r.error_for_status()?.text()?;
    let c = parse_releases(&json)?.into_iter().find(|c| link(&c.links, SUMS_NAME).is_some() && link(&c.links, SIG_NAME).is_some()).ok_or_else(|| anyhow!("no signed release"))?;
    let sums = small_file(&client, link(&c.links, SUMS_NAME).unwrap())?;
    let sig = small_file(&client, link(&c.links, SIG_NAME).unwrap())?;
    let signed = verify_sums(&sums, &sig, PUBLIC_KEY_HEX).ok_or_else(|| anyhow!("חתימת השחרור אינה תקינה"))?;
    if signed.version != c.version || signed.version <= current_version() {
        bail!("הגרסה החתומה אינה חדשה מהמותקנת");
    }
    let expected = signed.hash_of(SETUP_NAME).ok_or_else(|| anyhow!("המתקין אינו ברשימה החתומה"))?;
    let url = link(&c.links, SETUP_NAME).ok_or_else(|| anyhow!("no installer link"))?;
    let dir = update_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(format!("MapsOffline_{}.{}.{}_x64-setup.exe", c.version.0, c.version.1, c.version.2));
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
    std::process::Command::new(&canon).spawn().with_context(|| format!("starting {}", canon.display()))?;
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
}
