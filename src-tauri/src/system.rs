//! Windows integration and small runtime services: the system accent colour and text scale
//! (Fluent look), an online check that does not trust `navigator.onLine`, and the bounded
//! worker pools behind the custom URI schemes.
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::types::SystemAccent;

// ---------------------------------------------------------------- accent / text scale

#[cfg(windows)]
mod win {
    use windows::UI::ViewManagement::{UIColorType, UISettings};

    /// WinRT needs an initialised apartment on the calling thread (idempotent).
    fn init() {
        unsafe {
            let _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED);
        }
    }

    fn hex(c: windows::UI::Color) -> String {
        format!("#{:02x}{:02x}{:02x}", c.R, c.G, c.B)
    }

    pub fn accent() -> Option<super::SystemAccent> {
        init();
        let s = UISettings::new().ok()?;
        let get = |t: UIColorType| s.GetColorValue(t).ok().map(hex);
        Some(super::SystemAccent {
            accent: get(UIColorType::Accent)?,
            light1: get(UIColorType::AccentLight1)?,
            light2: get(UIColorType::AccentLight2)?,
            dark1: get(UIColorType::AccentDark1)?,
        })
    }

    pub fn text_scale() -> Option<f64> {
        init();
        UISettings::new().ok()?.TextScaleFactor().ok()
    }
}

/// The Windows accent colours, or `None` (other systems, or WinRT unavailable).
pub fn accent() -> Option<SystemAccent> {
    #[cfg(windows)]
    {
        win::accent()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Windows "Text size" (Settings → Accessibility), 1.0 ..= 2.25; 1.0 elsewhere.
pub fn text_scale() -> f64 {
    #[cfg(windows)]
    {
        win::text_scale().unwrap_or(1.0)
    }
    #[cfg(not(windows))]
    {
        1.0
    }
}

/// Emit `system-accent-changed` whenever the Windows accent changes (polled every 5 s: the
/// WinRT change event fires on a thread without an apartment we control, polling is simpler and
/// costs nothing measurable).
pub fn watch_accent(app: tauri::AppHandle) {
    use tauri::Emitter;
    let _ = std::thread::Builder::new().name("accent-watch".into()).spawn(move || {
        let mut last = accent();
        loop {
            std::thread::sleep(Duration::from_secs(5));
            let now = accent();
            if now.is_some() && now != last {
                let _ = app.emit("system-accent-changed", now.clone());
                last = now;
            }
        }
    });
}

// ---------------------------------------------------------------- connectivity

const PROBE_URL: &str = "https://build.protomaps.com/";
const CACHE_FOR: Duration = Duration::from_secs(30);

/// True when the internet answers (any HTTP status counts – a content filter's 418 means the
/// line works). One HEAD with a 1.5 s budget, remembered for 30 s.
pub fn connectivity() -> bool {
    static LAST: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    if let Some((t, ok)) = *LAST.lock() {
        if t.elapsed() < CACHE_FOR {
            return ok;
        }
    }
    static CLIENT: OnceLock<Option<reqwest::blocking::Client>> = OnceLock::new();
    let client = CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .user_agent(crate::data::download::USER_AGENT)
            .connect_timeout(Duration::from_millis(1500))
            .timeout(Duration::from_millis(1500))
            .build()
            .ok()
    });
    let ok = client.as_ref().is_some_and(|c| c.head(PROBE_URL).send().is_ok());
    *LAST.lock() = Some((Instant::now(), ok));
    ok
}

// ---------------------------------------------------------------- worker pools

type Job = Box<dyn FnOnce() + Send + 'static>;

/// A fixed set of worker threads fed from one queue. The URI schemes used to start a thread per
/// request; a screen of satellite tiles meant hundreds of threads waiting on the network at once.
pub struct Pool {
    tx: Mutex<Sender<Job>>,
}

impl Pool {
    pub fn new(name: &str, workers: usize) -> Self {
        let (tx, rx) = channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..workers {
            let rx = rx.clone();
            let _ = std::thread::Builder::new().name(format!("{name}-{i}")).spawn(move || loop {
                let job = rx.lock().recv();
                match job {
                    Ok(job) => job(),
                    Err(_) => break,
                }
            });
        }
        Pool { tx: Mutex::new(tx) }
    }

    pub fn run(&self, job: impl FnOnce() + Send + 'static) {
        if let Err(e) = self.tx.lock().send(Box::new(job)) {
            // the workers are gone (should not happen): run the job on its own thread
            let job = e.0;
            std::thread::spawn(job);
        }
    }
}

/// Requests that read local files (merged map, local custom layers).
pub fn local_pool() -> &'static Pool {
    static P: OnceLock<Pool> = OnceLock::new();
    P.get_or_init(|| Pool::new("tiles-local", 8))
}

/// Requests that may go to the network (online map proxy, live satellite, online custom layers).
pub fn net_pool() -> &'static Pool {
    static P: OnceLock<Pool> = OnceLock::new();
    P.get_or_init(|| Pool::new("tiles-net", 16))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_runs_all_jobs_with_bounded_threads() {
        let pool = Pool::new("test", 3);
        let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (tx, rx) = channel();
        for _ in 0..50 {
            let n = n.clone();
            let tx = tx.clone();
            pool.run(move || {
                n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _ = tx.send(());
            });
        }
        for _ in 0..50 {
            rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        assert_eq!(n.load(std::sync::atomic::Ordering::SeqCst), 50);
    }
}
