//! Log file of the GUI app: `<data root>\logs\mapsoffline.log`, rotated at 2 MB, the last 5
//! files kept (`mapsoffline.1.log` … `.4.log`). Release builds have no console, so this file is
//! the only trace of what happened; debug builds also print to stderr.
//!
//! Level: `info` by default (our crate `debug` with `RUST_LOG=debug`); dependencies are capped
//! at `warn` so HTTP / TLS chatter stays out. A panic hook writes the message, the location and a
//! backtrace before the default handling.
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use log::{Level, LevelFilter, Log, Metadata, Record};
use parking_lot::Mutex;

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const KEEP: usize = 5;
const FILE: &str = "mapsoffline.log";

static DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

struct FileLogger {
    out: Mutex<Option<(File, u64)>>,
    dir: PathBuf,
    level: LevelFilter,
}

impl FileLogger {
    fn open(dir: &Path) -> Option<(File, u64)> {
        fs::create_dir_all(dir).ok()?;
        let f = OpenOptions::new().create(true).append(true).open(dir.join(FILE)).ok()?;
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        Some((f, len))
    }

    fn rotate(&self) -> Option<(File, u64)> {
        for i in (1..KEEP).rev() {
            let from = if i == 1 { self.dir.join(FILE) } else { self.dir.join(format!("mapsoffline.{}.log", i - 1)) };
            let _ = fs::rename(&from, self.dir.join(format!("mapsoffline.{i}.log")));
        }
        Self::open(&self.dir)
    }

    fn write_line(&self, line: &str) {
        if cfg!(debug_assertions) {
            eprint!("{line}");
        }
        let mut g = self.out.lock();
        if let Some((_, len)) = g.as_ref() {
            if *len + line.len() as u64 > MAX_BYTES {
                *g = None; // close before renaming (Windows)
                *g = self.rotate();
            }
        }
        if let Some((f, len)) = g.as_mut() {
            if f.write_all(line.as_bytes()).is_ok() {
                *len += line.len() as u64;
            }
        }
    }
}

impl Log for FileLogger {
    fn enabled(&self, m: &Metadata) -> bool {
        let ours = m.target().starts_with("mapsoffline");
        m.level() <= if ours { self.level } else { self.level.min(LevelFilter::Warn) }
    }

    fn log(&self, r: &Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let lvl = match r.level() {
            Level::Error => "ERROR",
            Level::Warn => "WARN ",
            Level::Info => "INFO ",
            Level::Debug => "DEBUG",
            Level::Trace => "TRACE",
        };
        self.write_line(&format!("{ts} {lvl} {}: {}\n", r.target(), r.args()));
    }

    fn flush(&self) {
        if let Some((f, _)) = self.out.lock().as_mut() {
            let _ = f.flush();
        }
    }
}

/// Install the file logger and the panic hook (once; later calls are ignored).
pub fn init(dir: &Path) {
    let level = match std::env::var("RUST_LOG").ok().as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("trace") => LevelFilter::Trace,
        Some("debug") => LevelFilter::Debug,
        Some("warn") => LevelFilter::Warn,
        Some("error") => LevelFilter::Error,
        Some("off") => LevelFilter::Off,
        _ => LevelFilter::Info,
    };
    let logger = FileLogger { out: Mutex::new(FileLogger::open(dir)), dir: dir.to_path_buf(), level };
    if log::set_boxed_logger(Box::new(logger)).is_err() {
        return;
    }
    log::set_max_level(level);
    *DIR.lock() = Some(dir.to_path_buf());

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic".into());
        let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let thread = std::thread::current().name().unwrap_or("?").to_string();
        let bt = std::backtrace::Backtrace::force_capture();
        log::error!("PANIC in thread '{thread}' at {at}: {msg}\n{bt}");
        log::logger().flush();
        default_hook(info);
    }));

    log::info!(
        "MapsOffline {} starting ({} {}, log level {level})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
}

/// Folder of the log files (the one `init` used), e.g. for "open the logs folder".
pub fn dir() -> Option<PathBuf> {
    DIR.lock().clone()
}

