//! Structured command errors: every Tauri command returns `Result<T, AppError>`, which reaches
//! the webview as `{ code, message, details? }`.
//!
//! `code` is a stable machine-readable id the frontend translates; `message` is the full English
//! context chain (`{e:#}`) for logs and the "details" fold, and it still contains the code text,
//! so older substring checks on the message keep working; `details` carries extra data (the
//! blocked host, the panic location…).
//!
//! Codes produced today:
//! - data missing: `NO_ROUTING_DATA`, `NO_ROUTING_DATA_REGION`, `NO_TRANSIT_DATA`,
//!   `NO_GEOCODE_DATA`, `NO_TERRAIN_DATA`, `NO_MINYANIM_DATA`, `UNKNOWN_REGION`
//! - routing: `NEED_TWO_WAYPOINTS`, `TOO_MANY_WAYPOINTS`, `ORDER_NOT_FOR_TRANSIT`,
//!   `NO_ROAD_NEAR_WAYPOINT`, `NO_ROUTE_AVOID_AREAS`, `ONLINE_FAILED`
//! - transit: `NO_TRANSIT_SERVICE_ON_DATE`, `LINE_NOT_FOUND`, `NO_SERVICE_TODAY`, `STOP_NOT_FOUND`
//! - location / navigation: `LOCATION_DENIED`, `LOCATION_UNAVAILABLE`, `NAV_NOT_RUNNING`
//! - data jobs: `UPDATE_RUNNING`, `REGION_BUSY`, `UPDATE_FILE_INVALID`, `SAVE_FAILED`
//! - classified from the cause chain: `NETWORK` (any reqwest error), `BLOCKED_HOST` (HTTP 418 /
//!   content filter; `details` = host), `DISK_FULL`, `PERMISSION`
//! - `INTERNAL_PANIC` (a command panicked; `details` = location), `INTERNAL` (anything else)
//! - R1 (2026-11): `DISK_FULL` also as an explicit check before large writes (`details` =
//!   "needed=<bytes> free=<bytes> (<folder>)"), `CHECKSUM_MISMATCH` (download did not match the
//!   server's MD5), `INVALID_DATA_DIR`, `INVALID_PATH` (save path outside the allowed places or
//!   wrong type), `INVALID_LAYER` / `INVALID_COLOR` (user layers, favorites); `details` = the
//!   text after the code for the first four and `INVALID_LAYER`
//!
//! Any message (or cause in the chain) that starts with an `UPPER_SNAKE` word containing an
//! underscore, followed by end of text, `:` or a space, yields that word as the code – so new
//! `bail!("SOME_CODE: …")` codes need no change here.
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

pub type R<T> = Result<T, AppError>;

impl AppError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        AppError { code: code.into(), message: message.into(), details: None }
    }

    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }

    /// Classify plain text (a code, `CODE: detail`, or free text).
    pub fn from_text(text: impl Into<String>) -> Self {
        let message: String = text.into();
        if let Some(host) = blocked_host(&message) {
            return AppError::new("BLOCKED_HOST", message).with_details(host);
        }
        let code = leading_code(&message).unwrap_or("INTERNAL").to_string();
        AppError { code, message, details: None }
    }

    /// Classify an error by its whole cause chain.
    pub fn from_anyhow(e: &anyhow::Error) -> Self {
        let message = format!("{e:#}");
        // explicit codes win (outermost first: `bail!("NO_TRANSIT_DATA")` under a context)
        for cause in e.chain() {
            let s = cause.to_string();
            if let Some(code) = leading_code(&s) {
                let err = AppError::new(code, message.clone());
                // codes whose text after "CODE: " is data the UI shows (sizes, the folder…)
                if matches!(code, "DISK_FULL" | "CHECKSUM_MISMATCH" | "INVALID_DATA_DIR" | "INVALID_PATH" | "INVALID_LAYER") {
                    if let Some(rest) = s[code.len()..].strip_prefix(':').map(str::trim).filter(|r| !r.is_empty()) {
                        return err.with_details(rest);
                    }
                }
                return err;
            }
        }
        if let Some(host) = blocked_host(&message) {
            return AppError::new("BLOCKED_HOST", message).with_details(host);
        }
        for cause in e.chain() {
            if let Some(tauri::Error::JoinError(j)) = cause.downcast_ref::<tauri::Error>() {
                if j.is_panic() {
                    return AppError::new("INTERNAL_PANIC", message);
                }
            }
            if let Some(j) = cause.downcast_ref::<tokio::task::JoinError>() {
                if j.is_panic() {
                    return AppError::new("INTERNAL_PANIC", message);
                }
            }
            if let Some(re) = cause.downcast_ref::<reqwest::Error>() {
                if re.status().map(|s| s.as_u16()) == Some(418) {
                    let host = re.url().and_then(|u| u.host_str()).unwrap_or("").to_string();
                    return AppError::new("BLOCKED_HOST", message).with_details(host);
                }
                return AppError::new("NETWORK", message);
            }
            if let Some(io) = cause.downcast_ref::<std::io::Error>() {
                if is_disk_full(io) {
                    return AppError::new("DISK_FULL", message);
                }
                if io.kind() == std::io::ErrorKind::PermissionDenied {
                    return AppError::new("PERMISSION", message);
                }
            }
        }
        AppError::new("INTERNAL", message)
    }
}

fn is_disk_full(io: &std::io::Error) -> bool {
    // ERROR_HANDLE_DISK_FULL (39), ERROR_DISK_FULL (112); ENOSPC (28) elsewhere
    matches!(io.raw_os_error(), Some(39) | Some(112)) || (cfg!(unix) && io.raw_os_error() == Some(28)) || io.kind() == std::io::ErrorKind::StorageFull
}

/// `NO_ROUTING_DATA`, `ONLINE_FAILED: …`, `UNKNOWN_REGION: xx` → the code; `OSRM: …` (no
/// underscore) or free text → None.
fn leading_code(s: &str) -> Option<&str> {
    let end = s.find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')).unwrap_or(s.len());
    let word = &s[..end];
    let rest = &s[end..];
    let ok_shape = word.len() >= 3
        && word.starts_with(|c: char| c.is_ascii_uppercase())
        && word.contains('_')
        && !word.ends_with('_')
        && !word.starts_with('_');
    let ok_follow = rest.is_empty() || rest.starts_with(':') || rest.starts_with(' ');
    (ok_shape && ok_follow).then_some(word)
}

/// The host named by `data::download::blocked_message` (content filter such as NetFree).
fn blocked_host(s: &str) -> Option<String> {
    let i = s.find("הגישה אל ")? + "הגישה אל ".len();
    let rest = &s[i..];
    let j = rest.find(" חסומה")?;
    Some(rest[..j].to_string())
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError::from_anyhow(&e)
    }
}

impl From<String> for AppError {
    fn from(s: String) -> Self {
        AppError::from_text(s)
    }
}

impl From<&str> for AppError {
    fn from(s: &str) -> Self {
        AppError::from_text(s)
    }
}

impl From<tauri::Error> for AppError {
    fn from(e: tauri::Error) -> Self {
        AppError::from_anyhow(&anyhow::Error::new(e))
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AppError {}

/// Run a synchronous body; a panic becomes `INTERNAL_PANIC` instead of unwinding further.
///
/// Release builds use `panic = "abort"` (unwind measured +40 % on mapsoffline.exe, see
/// docs/SIZE.md), so there a panic still ends the process – after `logging`'s hook wrote it to the
/// log file. These guards act in dev / test builds and are ready should the profile change; the
/// real protection in release is validating untrusted input (data files, IPC arguments).
pub fn guard<T>(f: impl FnOnce() -> R<T>) -> R<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(p) => Err(AppError::new("INTERNAL_PANIC", format!("internal error: {}", panic_text(&p)))),
    }
}

/// Like [`guard`] for an async command body: polls the future inside `catch_unwind`.
pub async fn guard_async<T, F>(fut: F) -> R<T>
where
    F: std::future::Future<Output = R<T>>,
{
    CatchUnwind { fut: Box::pin(fut) }.await
}

struct CatchUnwind<F> {
    fut: std::pin::Pin<Box<F>>,
}

impl<T, F: std::future::Future<Output = R<T>>> std::future::Future for CatchUnwind<F> {
    type Output = R<T>;
    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<R<T>> {
        let fut = self.fut.as_mut();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fut.poll(cx))) {
            Ok(p) => p,
            Err(p) => std::task::Poll::Ready(Err(AppError::new("INTERNAL_PANIC", format!("internal error: {}", panic_text(&p))))),
        }
    }
}

pub fn panic_text(p: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_from_text() {
        assert_eq!(AppError::from("NO_ROUTING_DATA").code, "NO_ROUTING_DATA");
        assert_eq!(AppError::from("NO_ROUTING_DATA_REGION: at").code, "NO_ROUTING_DATA_REGION");
        assert_eq!(AppError::from("ONLINE_FAILED: timeout".to_string()).code, "ONLINE_FAILED");
        assert_eq!(AppError::from("OSRM: bad").code, "INTERNAL");
        assert_eq!(AppError::from("something broke").code, "INTERNAL");
        assert_eq!(AppError::from("SAVE_FAILED: unsupported file type").message, "SAVE_FAILED: unsupported file type");
    }

    #[test]
    fn codes_from_chain() {
        let e = anyhow::anyhow!("NO_TRANSIT_DATA").context("loading transit");
        let a = AppError::from(e);
        assert_eq!(a.code, "NO_TRANSIT_DATA");
        assert_eq!(a.message, "loading transit: NO_TRANSIT_DATA");
        let io = std::io::Error::from_raw_os_error(112);
        assert_eq!(AppError::from(anyhow::Error::new(io).context("write")).code, "DISK_FULL");
        let pd = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert_eq!(AppError::from(anyhow::Error::new(pd)).code, "PERMISSION");
        let b = AppError::from(anyhow::anyhow!(crate::data::download::blocked_message("https://gtfs.mot.gov.il/x.zip")));
        assert_eq!(b.code, "BLOCKED_HOST");
        assert_eq!(b.details.as_deref(), Some("gtfs.mot.gov.il"));
    }

    #[test]
    fn json_shape() {
        let j = serde_json::to_string(&AppError::from("NO_GEOCODE_DATA")).unwrap();
        assert_eq!(j, r#"{"code":"NO_GEOCODE_DATA","message":"NO_GEOCODE_DATA"}"#);
    }

    #[test]
    fn guard_catches_panics() {
        let r: R<()> = guard(|| panic!("boom"));
        assert_eq!(r.unwrap_err().code, "INTERNAL_PANIC");
        let r: R<u8> = tauri::async_runtime::block_on(guard_async(async { panic!("async boom") }));
        assert_eq!(r.unwrap_err().code, "INTERNAL_PANIC");
    }
}
