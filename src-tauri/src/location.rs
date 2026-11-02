//! "My location" through the Windows location service (WinRT `Geolocator`). The first call asks
//! the user with the system's own consent dialog (a regular top-level Windows window, not a
//! browser prompt); afterwards Windows remembers the answer per app.

/// `(lon, lat, accuracy_m)` of the current position.
/// Errors: `LOCATION_DENIED` (user / system policy), `LOCATION_UNAVAILABLE` (no fix in time).
#[cfg(windows)]
pub fn locate() -> anyhow::Result<(f64, f64, f64)> {
    use anyhow::bail;
    use windows::Devices::Geolocation::{GeolocationAccessStatus, Geolocator, PositionAccuracy};
    use windows::Foundation::TimeSpan;

    let status = Geolocator::RequestAccessAsync()?.get()?;
    if status != GeolocationAccessStatus::Allowed {
        bail!("LOCATION_DENIED");
    }
    let g = Geolocator::new()?;
    g.SetDesiredAccuracy(PositionAccuracy::High)?;
    // accept a fix up to 30 s old, wait at most 20 s for a new one
    let max_age = TimeSpan { Duration: 30 * 10_000_000 };
    let timeout = TimeSpan { Duration: 20 * 10_000_000 };
    let pos = match g.GetGeopositionAsyncWithAgeAndTimeout(max_age, timeout).and_then(|op| op.get()) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("geoposition failed: {e}");
            bail!("LOCATION_UNAVAILABLE");
        }
    };
    let coord = pos.Coordinate()?;
    let p = coord.Point()?.Position()?;
    let acc = coord.Accuracy()?;
    if !(p.Longitude.is_finite() && p.Latitude.is_finite()) {
        bail!("LOCATION_UNAVAILABLE");
    }
    Ok((p.Longitude, p.Latitude, acc))
}

#[cfg(not(windows))]
pub fn locate() -> anyhow::Result<(f64, f64, f64)> {
    anyhow::bail!("LOCATION_UNAVAILABLE")
}
