//! Free disk space checks before large writes (downloads, builds, package import / export):
//! a clear `DISK_FULL` error before the work starts instead of "os error 112" at 97 %.
use std::path::Path;

/// Space kept free on top of what a job needs (the system and other programs need room too).
pub const MARGIN: u64 = 256 * 1024 * 1024;

/// Bytes available to this user on the volume holding `dir` (the nearest existing ancestor is
/// asked). `None` when unknown (other systems, or the call failed): callers then skip the check.
pub fn free_bytes(dir: &Path) -> Option<u64> {
    let mut d = dir;
    while !d.exists() {
        d = d.parent()?;
    }
    #[cfg(windows)]
    {
        use windows::core::HSTRING;
        use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        let mut avail = 0u64;
        let path = HSTRING::from(d.as_os_str());
        unsafe { GetDiskFreeSpaceExW(&path, Some(&mut avail), None, None) }.ok()?;
        Some(avail)
    }
    #[cfg(not(windows))]
    {
        let _ = d;
        None
    }
}

/// Fail with `DISK_FULL: needed=<bytes> free=<bytes>` when `dir`'s volume cannot take `needed`
/// more bytes plus [`MARGIN`].
pub fn ensure_free(dir: &Path, needed: u64) -> anyhow::Result<()> {
    if let Some(free) = free_bytes(dir) {
        let want = needed.saturating_add(MARGIN);
        if free < want {
            anyhow::bail!("DISK_FULL: needed={want} free={free} ({})", dir.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_free_space() {
        let tmp = std::env::temp_dir();
        if let Some(free) = free_bytes(&tmp.join("does/not/exist/yet")) {
            assert!(free > 0);
            assert!(ensure_free(&tmp, 1).is_ok() || free < MARGIN);
            let e = ensure_free(&tmp, u64::MAX / 2).unwrap_err().to_string();
            assert!(e.starts_with("DISK_FULL: needed="), "{e}");
        }
    }
}
