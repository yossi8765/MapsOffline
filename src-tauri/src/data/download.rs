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

pub const USER_AGENT: &str = "MapsOffline/0.1";
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
pub fn load_meta(dest: &Path) -> Option<DownloadMeta> {
    DownloadMeta::parse(&std::fs::read_to_string(meta_path(dest)).ok()?)
}
pub fn save_meta(dest: &Path, m: &DownloadMeta) -> anyhow::Result<()> {
    std::fs::write(meta_path(dest), serde_json::to_vec_pretty(m)?)?;
    Ok(())
}

pub fn http_client() -> anyhow::Result<Client> {
    Ok(Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(60))
        .build()?)
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

/// Version string of the remote file (Last-Modified, else ETag) without downloading it. Some
/// servers (gtfs.mot.gov.il) answer HEAD without validators, so fall back to a ranged GET.
pub(crate) fn remote_version(client: &Client, url: &str) -> anyhow::Result<Option<String>> {
    let m = probe(client, url)?;
    if m.version().is_some() {
        return Ok(m.version());
    }
    let r = client.get(url).header(RANGE, "bytes=0-0").send().context("version request failed")?;
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
pub(crate) fn probe(client: &Client, url: &str) -> anyhow::Result<DownloadMeta> {
    if let Ok(r) = client.head(url).send() {
        if r.status().is_success() {
            let m = meta_from_headers(url, r.headers(), None);
            if m.content_length.is_some() {
                return Ok(m);
            }
        }
    }
    let r = client.get(url).header(RANGE, "bytes=0-0").send().context("probe request failed")?;
    let st = r.status();
    if !st.is_success() {
        if st.as_u16() == 418 {
            bail!("{}", blocked_message(url));
        }
        bail!("server returned HTTP {} for {}", st, url);
    }
    let total = if st.as_u16() == 206 {
        hdr(r.headers(), CONTENT_RANGE).and_then(|s| s.rsplit('/').next().and_then(|t| t.parse().ok()))
    } else {
        None
    };
    // Dropping the response without reading the body (could be a full 200).
    Ok(meta_from_headers(url, r.headers(), total))
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
            if dest.exists() && old.as_ref().map_or(false, |m| m.completed) {
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
    let mut meta = remote.clone().unwrap_or_else(|| DownloadMeta { url: url.into(), ..Default::default() });
    meta.completed = false;
    let resumable = match (&old, &remote) {
        (Some(o), Some(r)) => !o.completed && o.url == url && o.same_content(r),
        _ => false,
    };
    if !resumable {
        let _ = std::fs::remove_file(&part);
    }
    save_meta(dest, &meta)?;

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
        match attempt_download(&client, url, &part, &mut meta, progress, cancel) {
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
                if dest.exists() {
                    std::fs::remove_file(dest)?;
                }
                std::fs::rename(&part, dest)?;
                meta.completed = true;
                save_meta(dest, &meta)?;
                return Ok(meta.version());
            }
            Err(AttemptErr::Fatal(e)) => return Err(e),
            Err(AttemptErr::Retry(e)) => last_err = e,
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
        let _ = std::fs::remove_file(part);
        return Err(AttemptErr::Retry(anyhow!("range not satisfiable, restarting")));
    } else if st.as_u16() == 206 {
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
        let cr_total = hdr(resp.headers(), CONTENT_RANGE).and_then(|v| v.rsplit('/').next().and_then(|t| t.trim().parse::<u64>().ok()));
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn paths() {
        let p = Path::new("a/b.pbf");
        assert_eq!(meta_path(p), PathBuf::from("a/b.pbf.meta.json"));
        assert_eq!(part_path(p), PathBuf::from("a/b.pbf.part"));
    }
}
