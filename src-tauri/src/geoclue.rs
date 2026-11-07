//! Location on Linux: GeoClue 2 over the system D-Bus (the desktop's location service; GNOME and
//! KDE show their own permission question the first time). Used by "my location"
//! (`location::locate`) and by live navigation (`nav::gps`).

use std::time::{Duration, Instant};

use anyhow::Context;
use zbus::blocking::{proxy::Builder, Connection, Proxy};
use zbus::proxy::CacheProperties;
use zbus::zvariant::OwnedObjectPath;

const BUS: &str = "org.freedesktop.GeoClue2";
/// The .desktop file the deb / AppImage installs (GeoClue's agent shows its name and icon).
const DESKTOP_ID: &str = "MapsOffline";
/// GClueAccuracyLevel: EXACT (GPS when there is one; Wi-Fi / IP otherwise).
const ACCURACY_EXACT: u32 = 8;

/// One reading: lon, lat, accuracy (m), speed (m/s), heading (degrees from north).
#[derive(Debug, Clone, Copy)]
pub struct Reading {
    pub lon: f64,
    pub lat: f64,
    pub accuracy_m: Option<f64>,
    pub speed_mps: Option<f64>,
    pub heading: Option<f64>,
}

/// A started GeoClue client; stopped when dropped.
pub struct Client {
    conn: Connection,
    path: OwnedObjectPath,
}

fn proxy<'a>(conn: &Connection, path: &'a str, iface: &'a str) -> zbus::Result<Proxy<'a>> {
    // no property cache: every read asks the service (a cache would need its signals pumped)
    Builder::new(conn).destination(BUS)?.path(path)?.interface(iface)?.cache_properties(CacheProperties::No).build()
}

impl Client {
    pub fn start() -> anyhow::Result<Self> {
        let conn = Connection::system().context("LOCATION_UNAVAILABLE: no system D-Bus")?;
        let mgr = proxy(&conn, "/org/freedesktop/GeoClue2/Manager", "org.freedesktop.GeoClue2.Manager")?;
        let path: OwnedObjectPath = mgr.call("GetClient", &()).context("LOCATION_UNAVAILABLE: GeoClue is not installed or not running")?;
        {
            let cl = proxy(&conn, path.as_str(), "org.freedesktop.GeoClue2.Client")?;
            cl.set_property("DesktopId", DESKTOP_ID)?;
            cl.set_property("RequestedAccuracyLevel", ACCURACY_EXACT)?;
            cl.set_property("DistanceThreshold", 0u32)?;
            cl.call_method("Start", &()).map_err(|e| {
                // refused by the desktop's location setting / the user
                if e.to_string().contains("AccessDenied") {
                    anyhow::anyhow!("LOCATION_DENIED")
                } else {
                    anyhow::anyhow!("LOCATION_UNAVAILABLE: {e}")
                }
            })?;
        }
        Ok(Client { conn, path })
    }

    /// The latest location GeoClue has for this client (None until the first one).
    pub fn reading(&self) -> anyhow::Result<Option<Reading>> {
        let cl = proxy(&self.conn, self.path.as_str(), "org.freedesktop.GeoClue2.Client")?;
        let loc: OwnedObjectPath = cl.get_property("Location")?;
        if loc.as_str() == "/" {
            return Ok(None);
        }
        let l = proxy(&self.conn, loc.as_str(), "org.freedesktop.GeoClue2.Location")?;
        let lat: f64 = l.get_property("Latitude")?;
        let lon: f64 = l.get_property("Longitude")?;
        // GeoClue: -1 = unknown speed / heading
        let known = |v: f64| (v.is_finite() && v >= 0.0).then_some(v);
        let accuracy_m = l.get_property::<f64>("Accuracy").ok().and_then(|a| (a.is_finite() && a > 0.0).then_some(a));
        let speed_mps = l.get_property::<f64>("Speed").ok().and_then(known);
        let heading = l.get_property::<f64>("Heading").ok().and_then(known);
        if !lon.is_finite() || !lat.is_finite() {
            return Ok(None);
        }
        Ok(Some(Reading { lon, lat, accuracy_m, speed_mps, heading }))
    }

    /// Wait up to `timeout` for a first reading.
    pub fn wait(&self, timeout: Duration) -> anyhow::Result<Reading> {
        let t0 = Instant::now();
        loop {
            if let Some(r) = self.reading()? {
                return Ok(r);
            }
            if t0.elapsed() > timeout {
                anyhow::bail!("LOCATION_UNAVAILABLE: no fix yet");
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Ok(cl) = proxy(&self.conn, self.path.as_str(), "org.freedesktop.GeoClue2.Client") {
            let _ = cl.call_method("Stop", &());
        }
    }
}
