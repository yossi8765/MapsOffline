//! Resumable HTTP download with progress, freshness check via a `.meta.json` sidecar.
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context};
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, CONTENT_LENGTH, CONTENT_RANGE, ETAG, LAST_MODIFIED, RANGE};
use serde::{Deserialize, Serialize};

use crate::data::Cancel;
use crate::types::{ProgressFn, ProgressPhase};

/// One User-Agent for every HTTP request (data updates, online routing, tiles): app name,
/// version and project page, as Transitous and the OSRM demo server ask for.
pub const USER_AGENT: &str = concat!("MapsOffline/", env!("CARGO_PKG_VERSION"), " (+https://github.com/yossi8765/MapsOffline)");
const CHUNK: usize = 1 << 20;
const MAX_ATTEMPTS: u32 = 5;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DownloadMeta {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub etag: Option<String>,
    #[serde(default)]
    pub last_modified: Option<String>,
    #[serde(default)]
    pub content_length: Option<u64>,
    #[serde(default)]
    pub completed: bool,
    /// Replication sequence number of the last *full* download of an OSM extract; incremental
    /// updates (`osmdiff`) keep it so the extract is re-cut from scratch now and then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_sequence: Option<i64>,
}

impl DownloadMeta {
    pub fn version(&self) -> Option<String> {
        self.last_modified.clone().or_else(|| self.etag.clone())
    }
    pub fn parse(s: &str) -> Option<Self> {
        serde_json::from_str(s).ok()
    }
    /// Do two descriptions refer to the same remote content? Uses ETag, then Last-Modified, then size.
    pub fn same_content(&self, other: &DownloadMeta) -> bool {
        if let (Some(a), Some(b)) = (&self.etag, &other.etag) {
            return a == b;
        }
        if let (Some(a), Some(b)) = (&self.last_modified, &other.last_modified) {
            return a == b;
        }
        match (self.content_length, other.content_length) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }
}

fn append_ext(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}
pub fn meta_path(dest: &Path) -> PathBuf {
    append_ext(dest, ".meta.json")
}
pub fn part_path(dest: &Path) -> PathBuf {
    append_ext(dest, ".part")
}
/// Description of the remote file a `.part` belongs to (what a resume must match). Kept apart
/// from the finished file's meta, which must survive a failed download: it says "a complete
/// copy is here" (used offline) and carries the OSM extract's `full_sequence`.
fn part_meta_path(dest: &Path) -> PathBuf {
    append_ext(dest, ".part.meta.json")
}
pub fn load_meta(dest: &Path) -> Option<DownloadMeta> {
    DownloadMeta::parse(&std::fs::read_to_string(meta_path(dest)).ok()?)
}
pub fn save_meta(dest: &Path, m: &DownloadMeta) -> anyhow::Result<()> {
    write_json_atomic(&meta_path(dest), m)
}
fn write_json_atomic(path: &Path, m: &DownloadMeta) -> anyhow::Result<()> {
    let tmp = append_ext(path, ".tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(m)?)?;
    crate::paths::atomic_replace(path, &tmp)?;
    Ok(())
}

/// The shared blocking HTTP client (connection pool + TLS set up once; cloning is cheap).
pub fn http_client() -> anyhow::Result<Client> {
    static CLIENT: std::sync::OnceLock<Client> = std::sync::OnceLock::new();
    if let Some(c) = CLIENT.get() {
        return Ok(c.clone());
    }
    let c = Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(60))
        .build()?;
    Ok(CLIENT.get_or_init(|| c).clone())
}

/// Run `f` up to `attempts` times with a growing pause in between. An error whose text is
/// exactly "paused" (the job stopped for a pause) is retried without counting; cancellation
/// returns at once. Used by every download loop that is not `ensure_fresh` itself.
pub fn retry<T>(attempts: u32, cancel: &Cancel, progress: ProgressFn, label: &str, mut f: impl FnMut() -> anyhow::Result<T>) -> anyhow::Result<T> {
    let mut attempt = 0u32;
    loop {
        cancel.wait_if_paused();
        match f() {
            Ok(v) => return Ok(v),
            Err(e) => {
                if cancel.is_cancelled() {
                    return Err(e);
                }
                if e.to_string() == "paused" {
                    continue;
                }
                attempt += 1;
                if attempt >= attempts {
                    return Err(e);
                }
                log::warn!("{label}: attempt {attempt} failed: {e:#}");
                progress(ProgressPhase::Downloading, 0, 100, &format!("ההורדה נכשלה, ניסיון חוזר ({attempt}/{attempts})"));
                for _ in 0..(3 * attempt) * 2 {
                    if cancel.is_cancelled() {
                        return Err(e);
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        }
    }
}

fn hdr(h: &HeaderMap, name: impl reqwest::header::AsHeaderName) -> Option<String> {
    h.get(name).and_then(|v| v.to_str().ok()).map(|s| s.to_string())
}

fn meta_from_headers(url: &str, h: &HeaderMap, len: Option<u64>) -> DownloadMeta {
    DownloadMeta {
        url: url.to_string(),
        etag: hdr(h, ETAG),
        last_modified: hdr(h, LAST_MODIFIED),
        content_length: len.or_else(|| hdr(h, CONTENT_LENGTH).and_then(|s| s.parse().ok())),
        completed: false,
        full_sequence: None,
    }
}

/// Content filters used in Israel (NetFree and similar) answer HTTP 418 with a JSON body for
/// sites they have not categorised. Tell the user which host to approve instead of "HTTP 418".
pub fn blocked_message(url: &str) -> String {
    let host = reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_else(|| url.to_string());
    format!("הגישה אל {host} חסומה על ידי סינון האינטרנט (כגון NetFree). יש לאשר את האתר בסינון ולנסות שוב.")
}

/// GitHub release files: `github.com/<owner>/<repo>/releases/download/<tag>/<name>` (and
/// `releases/latest/download/<name>`) is answered by an edge server that can fail for hours
/// with "504 Gateway Time-out" (seen from Israel, November 2026) while the API works. Such a URL
/// is turned into the file's signed download address through the API (the release's asset →
/// `Accept: application/octet-stream` → its redirect); any other URL, or a failure, keeps the
/// original address. Signed addresses stay valid for about an hour: cached for 20 minutes.
pub fn resolve(url: &str) -> String {
    if !url.starts_with("https://github.com/") {
        return url.to_string();
    }
    match github_asset(url) {
        Ok(u) => u,
        Err(e) => {
            log::info!("GitHub API download address for {url}: {e:#}");
            url.to_string()
        }
    }
}

fn github_asset(url: &str) -> anyhow::Result<String> {
    use std::collections::HashMap;
    static CACHE: parking_lot::Mutex<Option<HashMap<String, (Instant, String)>>> = parking_lot::Mutex::new(None);
    const TTL: Duration = Duration::from_secs(20 * 60);
    if let Some((t, u)) = CACHE.lock().as_ref().and_then(|c| c.get(url).cloned()) {
        if t.elapsed() < TTL {
            return Ok(u);
        }
    }
    let p: Vec<&str> = url.trim_start_matches("https://github.com/").split('/').collect();
    let (owner, repo, release, name) = match p.as_slice() {
        [o, r, "releases", "download", tag, name] => (*o, *r, format!("tags/{tag}"), *name),
        [o, r, "releases", "latest", "download", name] => (*o, *r, "latest".to_string(), *name),
        _ => bail!("not a release file address"),
    };
    let timeout = Duration::from_secs(15);
    let api = format!("https://api.github.com/repos/{owner}/{repo}/releases/{release}");
    let v: serde_json::Value =
        http_client()?.get(&api).header("Accept", "application/vnd.github+json").timeout(timeout).send()?.error_for_status()?.json()?;
    let asset = v["assets"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["name"].as_str() == Some(name)))
        .and_then(|x| x["url"].as_str())
        .ok_or_else(|| anyhow!("no asset {name}"))?;
    let no_redirect = Client::builder().user_agent(USER_AGENT).redirect(reqwest::redirect::Policy::none()).timeout(timeout).build()?;
    let r = no_redirect.get(asset).header("Accept", "application/octet-stream").send()?;
    if !r.status().is_redirection() {
        bail!("asset {name}: HTTP {}", r.status());
    }
    let signed = hdr(r.headers(), reqwest::header::LOCATION).ok_or_else(|| anyhow!("asset {name}: no Location"))?;
    CACHE.lock().get_or_insert_with(HashMap::new).insert(url.to_string(), (Instant::now(), signed.clone()));
    Ok(signed)
}

/// Version string of the remote file (Last-Modified, else ETag) without downloading it. Some
/// servers (gtfs.mot.gov.il) answer HEAD without validators, so fall back to a ranged GET.
pub(crate) fn remote_version(client: &Client, url: &str) -> anyhow::Result<Option<String>> {
    let m = probe(client, url)?;
    if m.version().is_some() {
        return Ok(m.version());
    }
    let r = client.get(resolve(url)).header(RANGE, "bytes=0-0").send().context("version request failed")?;
    let st = r.status();
    if st.as_u16() == 418 {
        bail!("{}", blocked_message(url));
    }
    if !st.is_success() {
        bail!("server returned HTTP {} for {}", st, url);
    }
    Ok(meta_from_headers(url, r.headers(), None).version())
}

/// Probe the remote: HEAD, falling back to GET with Range 0-0.
///
/// A HEAD answer counts only when it carries a validator (ETag / Last-Modified) besides the
/// size: gtfs.mot.gov.il answers HEAD with a 3,382-byte HTML page and nothing else, and that
/// "size" never matched the downloaded 149 MB, so the GTFS was fetched again on every update.
pub(crate) fn probe(client: &Client, url: &str) -> anyhow::Result<DownloadMeta> {
    let fetch = resolve(url);
    if let Ok(r) = client.head(&fetch).send() {
        if r.status().is_success() {
            let m = meta_from_headers(url, r.headers(), None);
            if m.content_length.is_some() && m.version().is_some() {
                return Ok(m);
            }
        }
    }
    let r = client.get(&fetch).header(RANGE, "bytes=0-0").send().context("probe request failed")?;
    let st = r.status();
    if !st.is_success() {
        if st.as_u16() == 418 {
            bail!("{}", blocked_message(url));
        }
        bail!("server returned HTTP {} for {}", st, url);
    }
    let total = if st.as_u16() == 206 { hdr(r.headers(), CONTENT_RANGE).and_then(|s| content_range(&s)).and_then(|cr| cr.total) } else { None };
    // Dropping the response without reading the body (could be a full 200).
    Ok(meta_from_headers(url, r.headers(), total))
}

/// Parsed `Content-Range`: "bytes 100-199/1234" or "bytes */1234" (start None).
#[derive(Debug, PartialEq)]
struct ContentRange {
    start: Option<u64>,
    total: Option<u64>,
}

fn content_range(v: &str) -> Option<ContentRange> {
    let rest = v.trim().strip_prefix("bytes")?.trim_start();
    let (range, total) = rest.split_once('/')?;
    let total = total.trim().parse().ok();
    let start = if range.trim() == "*" { None } else { Some(range.split_once('-')?.0.trim().parse().ok()?) };
    Some(ContentRange { start, total })
}

/// The copy described by `dest`'s meta was downloaded completely from `url`, and the server still
/// offers the same content. Lets a caller that deleted the downloaded source after using it (the
/// GTFS zip) tell "nothing new" from "must download". Ok(None) = changed or unknown; Err = the
/// server could not be asked.
pub fn unchanged_remote(url: &str, dest: &Path) -> anyhow::Result<Option<String>> {
    let Some(old) = load_meta(dest) else { return Ok(None) };
    if !old.completed || old.url != url {
        return Ok(None);
    }
    let remote = probe(&http_client()?, url)?;
    Ok(if old.same_content(&remote) { old.version() } else { None })
}

fn fmt_mb(b: u64) -> String {
    format!("{:.1}", b as f64 / 1_048_576.0)
}

enum AttemptErr {
    Retry(anyhow::Error),
    Fatal(anyhow::Error),
    /// The update was paused: the .part file stays, the next attempt resumes it after the pause.
    Paused,
}

/// Download `url` to `dest` if the remote is newer / size differs. Returns a version string
/// (Last-Modified, else ETag) when available.
pub fn ensure_fresh(
    url: &str,
    dest: &Path,
    progress: ProgressFn,
    cancel: &Cancel,
) -> anyhow::Result<Option<String>> {
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p)?;
    }
    let client = http_client()?;
    let old = load_meta(dest);
    let remote = match probe(&client, url) {
        Ok(m) => Some(m),
        Err(e) => {
            // Offline but we have a complete copy: keep it.
            if dest.exists() && old.as_ref().is_some_and(|m| m.completed) {
                progress(ProgressPhase::Processing, 1, 1, "השרת אינו זמין – נעשה שימוש בקובץ הקיים");
                return Ok(old.and_then(|m| m.version()));
            }
            log::warn!("probe failed: {e:#}");
            None
        }
    };

    if let (Some(old), Some(remote)) = (&old, &remote) {
        if old.completed && old.url == url && dest.exists() {
            let size_ok = match (old.content_length, std::fs::metadata(dest)) {
                (Some(l), Ok(md)) => md.len() == l,
                (None, Ok(_)) => true,
                _ => false,
            };
            if size_ok && old.same_content(remote) {
                progress(ProgressPhase::Processing, 1, 1, "הקובץ כבר מעודכן");
                return Ok(old.version());
            }
        }
    }

    let part = part_path(dest);
    let part_meta = part_meta_path(dest);
    let mut meta = remote.clone().unwrap_or_else(|| DownloadMeta { url: url.into(), ..Default::default() });
    meta.completed = false;
    meta.full_sequence = None;
    // resume only a .part of the very same remote content
    let old_part = std::fs::read_to_string(&part_meta).ok().and_then(|s| DownloadMeta::parse(&s));
    let resumable = match (&old_part, &remote) {
        (Some(o), Some(r)) => o.url == url && o.same_content(r),
        _ => false,
    };
    if !resumable {
        let _ = std::fs::remove_file(&part);
    }
    // the finished file's meta stays as it is until the new copy is complete
    write_json_atomic(&part_meta, &meta)?;
    if let Some(len) = meta.content_length {
        let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        super::disk::ensure_free(dest.parent().unwrap_or(dest), len.saturating_sub(have))?;
    }

    let mut last_err = anyhow!("download failed");
    let mut attempt = 0u32;
    while attempt < MAX_ATTEMPTS {
        cancel.wait_if_paused();
        if cancel.is_cancelled() {
            bail!("download cancelled");
        }
        if attempt > 0 {
            let wait = 2u64.pow(attempt).min(30);
            progress(ProgressPhase::Downloading, 0, 0, &format!("ניסיון חוזר בעוד {wait} שנ׳ ({last_err})"));
            for _ in 0..wait * 2 {
                cancel.wait_if_paused();
                if cancel.is_cancelled() {
                    bail!("download cancelled");
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        attempt += 1;
        match attempt_download(&client, &resolve(url), &part, &mut meta, progress, cancel) {
            Ok(()) => {
                let sz = std::fs::metadata(&part)?.len();
                if let Some(l) = meta.content_length {
                    if sz != l {
                        let _ = std::fs::remove_file(&part);
                        last_err = anyhow!("size mismatch: got {sz}, expected {l}");
                        continue;
                    }
                } else {
                    meta.content_length = Some(sz);
                }
                crate::paths::atomic_replace(dest, &part)?;
                meta.completed = true;
                save_meta(dest, &meta)?;
                let _ = std::fs::remove_file(&part_meta);
                return Ok(meta.version());
            }
            Err(AttemptErr::Fatal(e)) => return Err(e),
            Err(AttemptErr::Retry(e)) => {
                last_err = e;
                // keep the .part's description current (Content-Range may have corrected the size)
                let _ = write_json_atomic(&part_meta, &meta);
            }
            Err(AttemptErr::Paused) => {
                attempt -= 1; // a pause is not a failed attempt
                progress(ProgressPhase::Downloading, 0, 0, "ההורדה מושהית");
                cancel.wait_if_paused();
            }
        }
    }
    Err(last_err.context(format!("download failed after {MAX_ATTEMPTS} attempts")))
}

fn attempt_download(
    _client: &Client,
    url: &str,
    part: &Path,
    meta: &mut DownloadMeta,
    progress: ProgressFn,
    cancel: &Cancel,
) -> Result<(), AttemptErr> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AttemptErr::Fatal(e.into()))?;
    rt.block_on(attempt_download_async(url, part, meta, progress, cancel))
}

async fn attempt_download_async(
    url: &str,
    part: &Path,
    meta: &mut DownloadMeta,
    progress: ProgressFn<'_>,
    cancel: &Cancel,
) -> Result<(), AttemptErr> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| AttemptErr::Fatal(e.into()))?;
    let start = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    let mut req = client.get(url);
    if start > 0 {
        req = req.header(RANGE, format!("bytes={start}-"));
    }
    let mut resp = req.send().await.map_err(|e| AttemptErr::Retry(e.into()))?;
    let st = resp.status();
    let mut offset = start;
    let mut file: File;
    if st.as_u16() == 416 {
        // "bytes */N": a .part that already holds all N bytes (stopped right at the end) is done
        let total = hdr(resp.headers(), CONTENT_RANGE).and_then(|v| content_range(&v)).and_then(|cr| cr.total);
        if start > 0 && total == Some(start) {
            meta.content_length = Some(start);
            return Ok(());
        }
        let _ = std::fs::remove_file(part);
        return Err(AttemptErr::Retry(anyhow!("range not satisfiable, restarting")));
    } else if st.as_u16() == 206 {
        // the server must continue exactly where the .part ends, or appending would corrupt it
        let cr = hdr(resp.headers(), CONTENT_RANGE).and_then(|v| content_range(&v));
        if cr.as_ref().and_then(|c| c.start) != Some(start) {
            let _ = std::fs::remove_file(part);
            return Err(AttemptErr::Retry(anyhow!("server resumed at another offset ({cr:?}, wanted {start}), restarting")));
        }
        file = OpenOptions::new().append(true).open(part).map_err(|e| AttemptErr::Fatal(e.into()))?;
    } else if st.is_success() {
        offset = 0;
        file = File::create(part).map_err(|e| AttemptErr::Fatal(e.into()))?;
    } else if st.is_server_error() || st.as_u16() == 429 {
        return Err(AttemptErr::Retry(anyhow!("HTTP {st}")));
    } else if st.as_u16() == 418 {
        return Err(AttemptErr::Fatal(anyhow!("{}", blocked_message(url))));
    } else {
        return Err(AttemptErr::Fatal(anyhow!("HTTP {st} for {url}")));
    }
    let hdr_len: Option<u64> = hdr(resp.headers(), CONTENT_LENGTH).and_then(|s| s.parse().ok());
    if st.as_u16() == 206 {
        // Content-Range's total is authoritative: the probe's Content-Length may describe a stub
        // page (gtfs.mot.gov.il), which made a resumed download report a bogus percentage.
        let cr_total = hdr(resp.headers(), CONTENT_RANGE).and_then(|v| content_range(&v)).and_then(|cr| cr.total);
        if let Some(total) = cr_total {
            meta.content_length = Some(total);
        } else if let Some(r) = hdr_len {
            meta.content_length = Some(r + offset);
        }
    } else if let Some(l) = hdr_len {
        // The real GET response is authoritative: some servers (gtfs.mot.gov.il) answer HEAD /
        // ranged probes with a small HTML page and a misleading Content-Length.
        meta.content_length = Some(l);
    }
    if meta.etag.is_none() {
        meta.etag = hdr(resp.headers(), ETAG);
    }
    if meta.last_modified.is_none() {
        meta.last_modified = hdr(resp.headers(), LAST_MODIFIED);
    }
    let total = meta.content_length.unwrap_or(0);

    // Coalesce network chunks into ~1 MB disk writes.
    let mut buf: Vec<u8> = Vec::with_capacity(CHUNK + 65536);
    let mut done = offset;
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    loop {
        if cancel.is_cancelled() {
            let _ = file.write_all(&buf);
            return Err(AttemptErr::Fatal(anyhow!("download cancelled")));
        }
        if cancel.is_paused() {
            // Drop the connection instead of holding it open for minutes; the Range request of
            // the next attempt continues from the bytes already written.
            let _ = file.write_all(&buf);
            return Err(AttemptErr::Paused);
        }
        // Wake at least every 500 ms so cancel is honoured even on slow links.
        let next = match tokio::time::timeout(Duration::from_millis(500), resp.chunk()).await {
            Err(_) => continue,
            Ok(Ok(c)) => c,
            Ok(Err(e)) => {
                let _ = file.write_all(&buf);
                return Err(AttemptErr::Retry(e.into()));
            }
        };
        match next {
            None => break,
            Some(c) => {
                done += c.len() as u64;
                buf.extend_from_slice(&c);
                if buf.len() >= CHUNK {
                    file.write_all(&buf).map_err(|e| AttemptErr::Fatal(e.into()))?;
                    buf.clear();
                }
            }
        }
        if last_emit.elapsed() >= Duration::from_millis(250) {
            last_emit = Instant::now();
            let msg = if total > 0 {
                format!("{} / {} MB", fmt_mb(done), fmt_mb(total))
            } else {
                format!("{} MB", fmt_mb(done))
            };
            progress(ProgressPhase::Downloading, done, total, &msg);
        }
    }
    file.write_all(&buf).map_err(|e| AttemptErr::Fatal(e.into()))?;
    file.flush().map_err(|e| AttemptErr::Fatal(e.into()))?;
    if total > 0 && done < total {
        return Err(AttemptErr::Retry(anyhow!("connection closed early at {done}/{total}")));
    }
    progress(ProgressPhase::Downloading, done, done.max(total), &format!("הורדו {} MB", fmt_mb(done)));
    Ok(())
}

/// Hex digest of a file, streamed (MD5 / SHA-256 of downloads).
pub fn file_digest<D: sha2::Digest>(path: &Path) -> anyhow::Result<String> {
    use std::io::Read;
    let mut f = std::io::BufReader::with_capacity(1 << 20, File::open(path)?);
    let mut h = D::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Compare a downloaded file with the checksum the server publishes next to it (`<url>.md5`,
/// Geofabrik). No published checksum (or no network) = nothing to compare; a mismatch deletes the
/// file and its meta (so the next run downloads again) and fails with `CHECKSUM_MISMATCH`.
pub fn verify_md5_sidecar(url: &str, file: &Path) -> anyhow::Result<()> {
    let client = http_client()?;
    let Ok(r) = client.get(format!("{url}.md5")).timeout(Duration::from_secs(15)).send() else { return Ok(()) };
    if !r.status().is_success() {
        return Ok(());
    }
    let text = r.text().unwrap_or_default();
    let Some(want) = text.split_whitespace().next().filter(|h| h.len() == 32 && h.chars().all(|c| c.is_ascii_hexdigit())) else {
        return Ok(());
    };
    check_digest::<md5::Md5>(file, want)
}

/// Fail with `CHECKSUM_MISMATCH` (and remove the file) when its digest is not `want` (hex).
pub fn check_digest<D: sha2::Digest>(file: &Path, want: &str) -> anyhow::Result<()> {
    let got = file_digest::<D>(file)?;
    if !got.eq_ignore_ascii_case(want) {
        let _ = std::fs::remove_file(file);
        let _ = std::fs::remove_file(meta_path(file));
        bail!("CHECKSUM_MISMATCH: {} (expected {want}, got {got})", file.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digests() {
        let p = std::env::temp_dir().join(format!("mo-digest-{}.bin", std::process::id()));
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(file_digest::<md5::Md5>(&p).unwrap(), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(file_digest::<sha2::Sha256>(&p).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert!(check_digest::<md5::Md5>(&p, "900150983CD24FB0D6963F7D28E17F72").is_ok());
        let e = check_digest::<md5::Md5>(&p, "00000000000000000000000000000000").unwrap_err().to_string();
        assert!(e.starts_with("CHECKSUM_MISMATCH"), "{e}");
        assert!(!p.exists(), "a mismatching file is removed");
    }

    #[test]
    fn meta_roundtrip() {
        let m = DownloadMeta {
            url: "http://x/y".into(),
            etag: Some("abc".into()),
            last_modified: Some("Mon, 01 Jan 2026 00:00:00 GMT".into()),
            content_length: Some(123),
            completed: true,
            full_sequence: Some(4937),
        };
        let s = serde_json::to_string(&m).unwrap();
        assert_eq!(DownloadMeta::parse(&s).unwrap(), m);
        assert_eq!(m.version().unwrap(), "Mon, 01 Jan 2026 00:00:00 GMT");
        assert!(DownloadMeta::parse("garbage").is_none());
    }

    #[test]
    fn same_content_rules() {
        let a = DownloadMeta { etag: Some("1".into()), content_length: Some(5), ..Default::default() };
        let b = DownloadMeta { etag: Some("2".into()), content_length: Some(5), ..Default::default() };
        assert!(!a.same_content(&b));
        let c = DownloadMeta { content_length: Some(5), ..Default::default() };
        assert!(a.same_content(&c));
        let d = DownloadMeta { content_length: Some(6), ..Default::default() };
        assert!(!c.same_content(&d));
        assert!(!DownloadMeta::default().same_content(&DownloadMeta::default()));
    }

    #[test]
    fn content_range_forms() {
        assert_eq!(content_range("bytes 100-199/1234"), Some(ContentRange { start: Some(100), total: Some(1234) }));
        assert_eq!(content_range("bytes */1234"), Some(ContentRange { start: None, total: Some(1234) }));
        assert_eq!(content_range("bytes 0-0/*"), Some(ContentRange { start: Some(0), total: None }));
        assert_eq!(content_range("items 1-2/3"), None);
    }

    /// A GitHub release file through the API address (`resolve`), with the resumable download.
    /// Online – `cargo test --lib github_release_file -- --ignored`.
    #[test]
    #[ignore]
    fn github_release_file() {
        let url = "https://github.com/yossi8765/MapsOffline/releases/download/v2.0.7/SHA256SUMS";
        assert!(resolve(url).starts_with("https://release-assets.githubusercontent.com/"), "{}", resolve(url));
        let dir = std::env::temp_dir().join("mo-gh-dl-test");
        let _ = std::fs::remove_dir_all(&dir);
        let dest = dir.join("SHA256SUMS");
        let cancel = Cancel::default();
        ensure_fresh(url, &dest, &|_, _, _, _| true, &cancel).unwrap();
        assert!(std::fs::read_to_string(&dest).unwrap().starts_with("# version 2.0.7"));
        assert_eq!(load_meta(&dest).unwrap().url, url);
        // second call: same content, no new download
        ensure_fresh(url, &dest, &|_, _, _, _| true, &cancel).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn paths() {
        let p = Path::new("a/b.pbf");
        assert_eq!(meta_path(p), PathBuf::from("a/b.pbf.meta.json"));
        assert_eq!(part_path(p), PathBuf::from("a/b.pbf.part"));
    }
}
