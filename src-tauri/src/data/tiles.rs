//! Vector tiles: extract Israel from the latest Protomaps build using the pmtiles CLI.
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};

use crate::data::download::{self, http_client};
use crate::data::Cancel;
use crate::paths::{atomic_replace, DataPaths, BBOX_ISRAEL};
use crate::types::{ProgressFn, ProgressPhase};

const BUILDS_URLS: [&str; 2] = [
    "https://build-metadata.protomaps.dev/builds.json",
    "https://build.protomaps.com/builds.json",
];
pub const BUILD_BASE: &str = "https://build.protomaps.com/";
const RELEASE_API: &str = "https://api.github.com/repos/protomaps/go-pmtiles/releases/latest";
const MIN_TILES_SIZE: u64 = 10 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize, Default)]
struct TilesMeta {
    version: String,
    bbox: [f64; 4],
    maxzoom: u8,
}

/// One extract of the Protomaps world build: the Israel map (bbox, zoom <= 15) or the minimal
/// world basemap (whole planet, zoom <= 6, ~45 MB) that gives context outside Israel.
pub struct Extract {
    pub file: std::path::PathBuf,
    /// None = the whole world
    pub bbox: Option<[f64; 4]>,
    pub maxzoom: u8,
    pub label: String,
    /// GeoJSON (Multi)Polygon file: extract this shape instead of `bbox` (countries); `bbox`
    /// is then the shape's bounding box, used for the sidecar check.
    pub region: Option<std::path::PathBuf>,
    /// Smallest plausible output (tiny countries make small archives).
    pub min_size: u64,
}

pub const WORLD_BBOX: [f64; 4] = [-180.0, -85.0, 180.0, 85.0];
pub const WORLD_MAX_ZOOM: u8 = 6;

impl Extract {
    pub fn israel(paths: &DataPaths) -> Self {
        Self { file: paths.tiles_file(), bbox: Some(BBOX_ISRAEL), maxzoom: 15, label: "אריחי המפה".into(), region: None, min_size: MIN_TILES_SIZE }
    }
    pub fn world(paths: &DataPaths) -> Self {
        Self { file: paths.world_file(), bbox: None, maxzoom: WORLD_MAX_ZOOM, label: "מפת העולם".into(), region: None, min_size: MIN_TILES_SIZE }
    }
    fn sidecar(&self) -> std::path::PathBuf {
        download::meta_path(&self.file)
    }
    fn meta_bbox(&self) -> [f64; 4] {
        self.bbox.unwrap_or(WORLD_BBOX)
    }
    /// Build key of the installed extract ("YYYYMMDD.pmtiles"), if the file and its sidecar are intact.
    pub fn installed_build(&self) -> Option<String> {
        if !self.file.exists() {
            return None;
        }
        let s = std::fs::read_to_string(self.sidecar()).ok()?;
        let m = serde_json::from_str::<TilesMeta>(&s).ok()?;
        (m.bbox == self.meta_bbox() && m.maxzoom == self.maxzoom && verify_tiles(&self.file, self.min_size).is_ok()).then_some(m.version)
    }
}

/// Build key of the installed Israel tiles.
pub(crate) fn installed_build(paths: &DataPaths) -> Option<String> {
    Extract::israel(paths).installed_build()
}

/// Protomaps publishes a new world build every day, but Israel changes little from one day to
/// the next: an installed build counts as current until it is `TILES_STALE_DAYS` behind.
pub const TILES_STALE_DAYS: i64 = 7;

fn build_date(key: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(key.get(..8)?, "%Y%m%d").ok()
}

/// Does the latest build warrant an update of the installed one (missing, unparsable, or a week older)?
pub fn build_is_stale(installed: Option<&str>, latest: &str) -> bool {
    match (installed.and_then(build_date), build_date(latest)) {
        (Some(have), Some(new)) => (new - have).num_days() >= TILES_STALE_DAYS,
        (Some(_), None) => installed != Some(latest),
        (None, _) => true,
    }
}

/// Pick the newest `.pmtiles` key from the builds.json text.
pub fn pick_latest_build(json: &str) -> anyhow::Result<String> {
    let v: serde_json::Value = serde_json::from_str(json).context("invalid builds.json")?;
    let arr = v.as_array().ok_or_else(|| anyhow!("builds.json is not an array"))?;
    let mut keys: Vec<String> = arr
        .iter()
        .filter_map(|e| e.get("key").and_then(|k| k.as_str()))
        .filter(|k| k.ends_with(".pmtiles"))
        .map(|s| s.to_string())
        .collect();
    // Prefer plain daily builds (YYYYMMDD.pmtiles); names sort chronologically.
    let daily: Vec<String> = keys
        .iter()
        .filter(|k| k.len() == 16 && k[..8].chars().all(|c| c.is_ascii_digit()))
        .cloned()
        .collect();
    if !daily.is_empty() {
        keys = daily;
    }
    keys.into_iter().max().ok_or_else(|| anyhow!("no .pmtiles build found in builds.json"))
}

/// Newest daily build key ("YYYYMMDD.pmtiles"). builds.json first; if the metadata host is
/// unreachable (content filters such as NetFree block it while the build host itself is open),
/// probe the build host for the last two weeks of daily names.
pub fn latest_build_key(client: &reqwest::blocking::Client) -> anyhow::Result<String> {
    let mut last: Option<anyhow::Error> = None;
    let mut blocked: Option<anyhow::Error> = None;
    for u in BUILDS_URLS {
        match client.get(u).send() {
            Ok(r) if r.status().is_success() => match r.text().ok().and_then(|t| pick_latest_build(&t).ok()) {
                Some(k) => return Ok(k),
                None => last = Some(anyhow!("invalid builds.json from {u}")),
            },
            Ok(r) if r.status().as_u16() == 418 => blocked = Some(anyhow!("{}", download::blocked_message(u))),
            Ok(r) => last = Some(anyhow!("HTTP {} for {u}", r.status())),
            Err(e) => last = Some(e.into()),
        }
    }
    if blocked.is_some() {
        last = blocked.take();
    }
    log::warn!("builds.json unavailable ({:#}); probing daily builds directly", last.as_ref().map(|e| e.to_string()).unwrap_or_default());
    let today = chrono::Utc::now().date_naive();
    for back in 0..14 {
        let d = today - chrono::Duration::days(back);
        let key = format!("{}.pmtiles", d.format("%Y%m%d"));
        let url = format!("{BUILD_BASE}{key}");
        match client.head(&url).send() {
            Ok(r) if r.status().is_success() => return Ok(key),
            Ok(r) if r.status().as_u16() == 418 => {
                last = Some(anyhow!("{}", download::blocked_message(&url)));
                break;
            }
            _ => {}
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("no daily build found")).context("איתור גרסת המפה העדכנית נכשל"))
}

/// Parse "NN%" out of a progress-bar line (last percentage wins).
pub fn parse_percent(line: &str) -> Option<u64> {
    let idx = line.rfind('%')?;
    let digits: String = line[..idx].chars().rev().take_while(|c| c.is_ascii_digit()).collect::<Vec<_>>().into_iter().rev().collect();
    digits.parse::<u64>().ok().filter(|p| *p <= 100)
}

fn ensure_cli(
    paths: &DataPaths,
    progress: ProgressFn,
    cancel: &Cancel,
) -> anyhow::Result<()> {
    let exe = paths.pmtiles_cli();
    if exe.exists() {
        return Ok(());
    }
    progress(ProgressPhase::Processing, 0, 0, "מאתר את כלי ההורדה pmtiles");
    let client = http_client()?;
    let rel: serde_json::Value = client
        .get(RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .send()?
        .error_for_status()
        .context("GitHub release lookup failed")?
        .json()?;
    let assets = rel["assets"].as_array().ok_or_else(|| anyhow!("no assets in release"))?;
    let url = assets
        .iter()
        .find(|a| a["name"].as_str().map_or(false, |n| n.to_lowercase().ends_with("windows_x86_64.zip")))
        .and_then(|a| a["browser_download_url"].as_str())
        .ok_or_else(|| anyhow!("no windows_x86_64.zip asset in latest go-pmtiles release"))?
        .to_string();
    log::info!("pmtiles CLI release {} -> {url}", rel["tag_name"]);
    let zip_path = paths.downloads_dir().join("go-pmtiles_windows_x86_64.zip");
    download::ensure_fresh(&url, &zip_path, progress, cancel)?;
    let f = std::fs::File::open(&zip_path)?;
    let mut z = zip::ZipArchive::new(f)?;
    let mut found = false;
    for i in 0..z.len() {
        let mut e = z.by_index(i)?;
        let name = e.name().to_lowercase();
        if name.ends_with("pmtiles.exe") {
            let tmp = exe.with_extension("exe.tmp");
            let mut out = std::fs::File::create(&tmp)?;
            std::io::copy(&mut e, &mut out)?;
            out.flush()?;
            drop(out);
            atomic_replace(&exe, &tmp)?;
            found = true;
            break;
        }
    }
    if !found {
        bail!("pmtiles.exe not found inside {}", zip_path.display());
    }
    Ok(())
}

fn verify_tiles(p: &Path, min_size: u64) -> anyhow::Result<()> {
    let md = std::fs::metadata(p).context("tiles output missing")?;
    if md.len() <= min_size {
        bail!("tiles output too small ({} bytes)", md.len());
    }
    let mut magic = [0u8; 7];
    std::fs::File::open(p)?.read_exact(&mut magic)?;
    if &magic != b"PMTiles" {
        bail!("tiles output has no PMTiles header");
    }
    Ok(())
}

fn run_extract(
    paths: &DataPaths,
    src: &str,
    tmp: &Path,
    args: &[String],
    label: &str,
    progress: ProgressFn,
    cancel: &Cancel,
) -> anyhow::Result<()> {
    let mut cmd = Command::new(paths.pmtiles_cli());
    cmd.arg("extract").arg(src).arg(tmp).args(args);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().context("failed to start pmtiles.exe")?;

    let (tx, rx) = mpsc::channel::<String>();
    let mut readers = Vec::new();
    fn spawn_reader<R: Read + Send + 'static>(mut r: R, tx: mpsc::Sender<String>) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut cur = Vec::new();
            loop {
                match r.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        for &c in &buf[..n] {
                            if c == b'\r' || c == b'\n' {
                                if !cur.is_empty() {
                                    let _ = tx.send(String::from_utf8_lossy(&cur).to_string());
                                    cur.clear();
                                }
                            } else {
                                cur.push(c);
                            }
                        }
                    }
                }
            }
            if !cur.is_empty() {
                let _ = tx.send(String::from_utf8_lossy(&cur).to_string());
            }
        })
    }
    if let Some(o) = child.stdout.take() {
        readers.push(spawn_reader(o, tx.clone()));
    }
    if let Some(e) = child.stderr.take() {
        readers.push(spawn_reader(e, tx.clone()));
    }
    drop(tx);

    let mut tail: Vec<String> = Vec::new();
    let mut last_pct = 0u64;
    let mut last_emit = Instant::now();
    let status = loop {
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(tmp);
            bail!("tiles extraction cancelled");
        }
        if cancel.is_paused() {
            // pmtiles extract cannot be suspended; stop it and start over after the pause
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(tmp);
            progress(ProgressPhase::Downloading, 0, 0, "ההורדה מושהית");
            cancel.wait_if_paused();
            bail!("paused");
        }
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(line) => {
                let line = line.trim().to_string();
                if let Some(p) = parse_percent(&line) {
                    if p != last_pct || last_emit.elapsed() >= Duration::from_secs(2) {
                        last_pct = p;
                        last_emit = Instant::now();
                        progress(ProgressPhase::Downloading, p, 100, &format!("{label} – {p}%"));
                    }
                } else if !line.is_empty() {
                    progress(ProgressPhase::Downloading, last_pct, 100, &line);
                    tail.push(line);
                    if tail.len() > 20 {
                        tail.remove(0);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break child.wait()?,
        }
    };
    for r in readers {
        let _ = r.join();
    }
    if !status.success() {
        let _ = std::fs::remove_file(tmp);
        bail!("pmtiles extract failed ({status}): {}", tail.join(" | "));
    }
    Ok(())
}

pub fn update_tiles(paths: &DataPaths, progress: ProgressFn, cancel: &Cancel) -> anyhow::Result<Option<String>> {
    update_extract(paths, &Extract::israel(paths), progress, cancel)
}

pub fn update_world(paths: &DataPaths, progress: ProgressFn, cancel: &Cancel) -> anyhow::Result<Option<String>> {
    update_extract(paths, &Extract::world(paths), progress, cancel)
}

pub(crate) fn update_extract(paths: &DataPaths, ex: &Extract, progress: ProgressFn, cancel: &Cancel) -> anyhow::Result<Option<String>> {
    paths.ensure_dirs()?;
    ensure_cli(paths, progress, cancel)?;
    if cancel.is_cancelled() {
        bail!("cancelled");
    }

    progress(ProgressPhase::Processing, 0, 0, "מחפש את גרסת המפה העדכנית (Protomaps)");
    let client = http_client()?;
    let key = latest_build_key(&client)?;

    if ex.installed_build().as_deref() == Some(key.as_str()) {
        progress(ProgressPhase::Processing, 1, 1, "המפה כבר מעודכנת");
        return Ok(Some(key));
    }

    let tmp = ex.file.with_extension("pmtiles.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut args: Vec<String> = Vec::new();
    if let Some(r) = &ex.region {
        args.push(format!("--region={}", r.display()));
    } else if let Some(b) = ex.bbox {
        args.push(format!("--bbox={},{},{},{}", b[0], b[1], b[2], b[3]));
    }
    args.push(format!("--maxzoom={}", ex.maxzoom));
    let src = format!("{BUILD_BASE}{key}");
    progress(ProgressPhase::Downloading, 0, 100, &format!("{}, גרסה {key}", &ex.label));

    let mut last_err = None;
    let mut attempt = 0;
    while attempt < 3 {
        match run_extract(paths, &src, &tmp, &args, &ex.label, progress, cancel) {
            Ok(()) => {
                last_err = None;
                break;
            }
            Err(e) => {
                if cancel.is_cancelled() {
                    return Err(e);
                }
                if e.to_string() == "paused" {
                    continue;
                }
                attempt += 1;
                log::warn!("pmtiles extract attempt {attempt} failed: {e:#}");
                progress(ProgressPhase::Downloading, 0, 100, &format!("ההורדה נכשלה, ניסיון חוזר ({attempt}/3)"));
                last_err = Some(e);
                std::thread::sleep(Duration::from_secs(3));
            }
        }
    }
    if let Some(e) = last_err {
        return Err(e);
    }
    progress(ProgressPhase::Finalizing, 0, 0, "מאמת את קובץ המפה");
    if let Err(e) = verify_tiles(&tmp, ex.min_size) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    atomic_replace(&ex.file, &tmp)?;
    let meta = TilesMeta { version: key.clone(), bbox: ex.meta_bbox(), maxzoom: ex.maxzoom };
    std::fs::write(ex.sidecar(), serde_json::to_vec_pretty(&meta)?)?;
    Ok(Some(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_newest_daily() {
        let j = r#"[{"key":"20261030.pmtiles","size":1,"uploaded":"x"},
            {"key":"20261031.pmtiles","size":1,"uploaded":"x"},
            {"key":"20261031.json","size":1,"uploaded":"x"},
            {"key":"foo/20270101.pmtiles","size":1,"uploaded":"x"}]"#;
        assert_eq!(pick_latest_build(j).unwrap(), "20261031.pmtiles");
        assert!(pick_latest_build("[]").is_err());
        assert!(pick_latest_build("{}").is_err());
    }

    #[test]
    fn weekly_staleness() {
        assert!(!build_is_stale(Some("20261101.pmtiles"), "20261102.pmtiles"));
        assert!(!build_is_stale(Some("20261101.pmtiles"), "20261107.pmtiles"));
        assert!(build_is_stale(Some("20261101.pmtiles"), "20261108.pmtiles"));
        assert!(build_is_stale(None, "20261108.pmtiles"));
        assert!(build_is_stale(Some("weird"), "20261108.pmtiles"));
    }

    #[test]
    fn percent_parsing() {
        assert_eq!(parse_percent("fetching chunks  42% |███   | (1/2)"), Some(42));
        assert_eq!(parse_percent("100%|"), Some(100));
        assert_eq!(parse_percent("no progress"), None);
    }
}
