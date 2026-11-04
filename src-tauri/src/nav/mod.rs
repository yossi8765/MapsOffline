//! Live navigation: a background session feeds position fixes (Windows location service, or a
//! simulator that drives along the route) into the state machine and emits "nav-update" events;
//! spoken instructions go to `crate::voice`.

pub mod machine;
pub mod track;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tauri::{AppHandle, Emitter};

use crate::types::{LonLat, Route};
use machine::{Fix, NavRoute, Navigator};

/// Where fixes come from.
#[derive(Debug, Clone, Copy)]
pub enum Source {
    /// Windows location service, one fix per second.
    Gps,
    /// Drive along the route at this speed (km/h): testing without a GPS.
    Simulate { speed_kmh: f64 },
}

struct Session {
    stop: Arc<AtomicBool>,
    nav: Arc<Mutex<Navigator>>,
    /// Bumped when the route is replaced, so the simulator restarts from the new route's start.
    route_gen: Arc<std::sync::atomic::AtomicU64>,
    voice: Arc<AtomicBool>,
}

fn nav_route(route: &Route, cameras: &[LonLat]) -> NavRoute {
    let mut nr = NavRoute::from_route(route);
    if !cameras.is_empty() {
        nr.set_cameras(cameras);
    }
    nr
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

/// Start navigating `route` (replaces a running session). `cameras`: speed cameras near the
/// route (those on it are announced).
pub fn start(app: AppHandle, route: &Route, source: Source, voice: bool, cameras: &[LonLat]) -> anyhow::Result<()> {
    anyhow::ensure!(route.geometry.len() >= 2 || route.legs.iter().any(|l| l.geometry.len() >= 2), "ROUTE_EMPTY");
    stop();
    let stop_flag = Arc::new(AtomicBool::new(false));
    let nav = Arc::new(Mutex::new(Navigator::new(nav_route(route, cameras))));
    let route_gen = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let voice_flag = Arc::new(AtomicBool::new(voice));
    *SESSION.lock() = Some(Session { stop: stop_flag.clone(), nav: nav.clone(), route_gen: route_gen.clone(), voice: voice_flag.clone() });
    std::thread::Builder::new().name("nav".into()).spawn(move || run(app, source, stop_flag, nav, route_gen, voice_flag))?;
    Ok(())
}

/// Replace the active route (after a reroute) without stopping the session.
pub fn set_route(route: &Route, cameras: &[LonLat]) -> anyhow::Result<()> {
    let g = SESSION.lock();
    let s = g.as_ref().ok_or_else(|| anyhow::anyhow!("NAV_NOT_RUNNING"))?;
    *s.nav.lock() = Navigator::new(nav_route(route, cameras));
    s.route_gen.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

pub fn set_voice(on: bool) {
    if let Some(s) = SESSION.lock().as_ref() {
        s.voice.store(on, Ordering::SeqCst);
    }
    if !on {
        crate::voice::silence();
    }
}

pub fn stop() {
    if let Some(s) = SESSION.lock().take() {
        s.stop.store(true, Ordering::SeqCst);
    }
    crate::voice::silence();
}

pub fn running() -> bool {
    SESSION.lock().is_some()
}

fn now() -> chrono::DateTime<chrono::FixedOffset> {
    chrono::Utc::now().with_timezone(&chrono_tz::Asia::Jerusalem).fixed_offset()
}

fn run(app: AppHandle, source: Source, stop: Arc<AtomicBool>, nav: Arc<Mutex<Navigator>>, route_gen: Arc<std::sync::atomic::AtomicU64>, voice: Arc<AtomicBool>) {
    let mut gps = match source {
        Source::Gps => match gps::Gps::open() {
            Ok(g) => Some(g),
            Err(e) => {
                let _ = app.emit("nav-error", e.to_string());
                *SESSION.lock() = None;
                return;
            }
        },
        Source::Simulate { .. } => None,
    };
    let tick = Duration::from_millis(1000);
    let mut sim_d = 0.0_f64;
    let mut sim_gen = route_gen.load(Ordering::SeqCst);
    let mut last_gps_err = 0u32;
    while !stop.load(Ordering::SeqCst) {
        let t0 = std::time::Instant::now();
        let fix = match source {
            Source::Simulate { speed_kmh } => {
                let gen = route_gen.load(Ordering::SeqCst);
                if gen != sim_gen {
                    sim_gen = gen;
                    sim_d = 0.0;
                }
                let n = nav.lock();
                let (p, b) = machine::point_at(&n.route, sim_d);
                drop(n);
                sim_d += speed_kmh / 3.6 * tick.as_secs_f64();
                Some(Fix { lon: p[0], lat: p[1], heading: Some(b), speed_mps: Some(speed_kmh / 3.6), accuracy_m: Some(5.0) })
            }
            Source::Gps => match gps.as_mut().map(|g| g.fix()) {
                Some(Ok(f)) => {
                    last_gps_err = 0;
                    Some(f)
                }
                Some(Err(e)) => {
                    last_gps_err += 1;
                    if last_gps_err == 5 {
                        let _ = app.emit("nav-error", format!("LOCATION_UNAVAILABLE: {e}"));
                    }
                    None
                }
                None => None,
            },
        };
        if let Some(fix) = fix {
            let u = nav.lock().update(fix, now());
            if let Some(text) = &u.speech {
                if voice.load(Ordering::SeqCst) {
                    crate::voice::say(text);
                }
            }
            let arrived = u.arrived;
            let _ = app.emit("nav-update", &u);
            if arrived {
                // keep the arrival on screen; the session ends by itself
                std::thread::sleep(Duration::from_secs(2));
                break;
            }
        }
        let spent = t0.elapsed();
        if spent < tick {
            std::thread::sleep(tick - spent);
        }
    }
    let mut g = SESSION.lock();
    if g.as_ref().is_some_and(|s| Arc::ptr_eq(&s.stop, &stop)) {
        *g = None;
    }
    let _ = app.emit("nav-ended", ());
}

#[cfg(windows)]
pub(crate) mod gps {
    use super::Fix;
    use windows::Devices::Geolocation::{GeolocationAccessStatus, Geolocator, PositionAccuracy};
    use windows::Foundation::TimeSpan;

    pub struct Gps {
        g: Geolocator,
    }

    impl Gps {
        pub fn open() -> anyhow::Result<Self> {
            crate::voice::init_winrt_thread();
            let status = Geolocator::RequestAccessAsync()?.get()?;
            if status != GeolocationAccessStatus::Allowed {
                anyhow::bail!("LOCATION_DENIED");
            }
            let g = Geolocator::new()?;
            g.SetDesiredAccuracy(PositionAccuracy::High)?;
            Ok(Self { g })
        }

        pub fn fix(&mut self) -> anyhow::Result<Fix> {
            let age = TimeSpan { Duration: 10_000_000 };
            let timeout = TimeSpan { Duration: 3 * 10_000_000 };
            let pos = self.g.GetGeopositionAsyncWithAgeAndTimeout(age, timeout)?.get()?;
            let c = pos.Coordinate()?;
            let p = c.Point()?.Position()?;
            let heading = c.Heading().and_then(|h| h.Value()).ok().filter(|h| h.is_finite());
            let speed = c.Speed().and_then(|s| s.Value()).ok().filter(|s| s.is_finite());
            let accuracy = c.Accuracy().ok().filter(|a| a.is_finite() && *a > 0.0);
            anyhow::ensure!(p.Longitude.is_finite() && p.Latitude.is_finite(), "no fix");
            Ok(Fix { lon: p.Longitude, lat: p.Latitude, heading, speed_mps: speed, accuracy_m: accuracy })
        }
    }
}

#[cfg(not(windows))]
pub(crate) mod gps {
    use super::Fix;
    pub struct Gps;
    impl Gps {
        pub fn open() -> anyhow::Result<Self> {
            anyhow::bail!("LOCATION_UNAVAILABLE")
        }
        pub fn fix(&mut self) -> anyhow::Result<Fix> {
            anyhow::bail!("LOCATION_UNAVAILABLE")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::machine::{point_at, Fix, NavRoute, Navigator};
    use crate::types::*;

    /// Drive a real local route (Tel Aviv -> Jerusalem) through the state machine and print what
    /// would be spoken. Needs the installed graph: `cargo test --lib nav::tests -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn simulate_real_route() {
        let paths = crate::paths::DataPaths::new(crate::paths::DataPaths::default_root());
        let g = crate::routing::load_graph(&paths.graph_file()).expect("graph.bin installed");
        let req = RouteRequest {
            waypoints: vec![Waypoint { lon: 34.7745, lat: 32.0853, name: None }, Waypoint { lon: 35.2137, lat: 31.7795, name: None }],
            mode: Mode::Car,
            time: TimeSpec::default(),
            accessible: false,
            avoid: Avoid::default(),
            alternatives: 1,
            ..Default::default()
        };
        let r = g.route(&req).unwrap().routes.remove(0);
        let nr = NavRoute::from_route(&r);
        println!("route {:.1} km, {} steps, {:.0} min", nr.total_m / 1000.0, nr.steps.len(), r.duration_sec / 60.0);
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-04T10:00:00+02:00").unwrap();
        let mut nav = Navigator::new(nr.clone());
        let mut d = 0.0;
        let (mut spoken, mut off) = (0, 0);
        let mut last = None;
        while d <= nr.total_m + 30.0 {
            let (p, b) = point_at(&nr, d);
            let u = nav.update(Fix { lon: p[0], lat: p[1], heading: Some(b), speed_mps: Some(16.7), accuracy_m: None }, now);
            if u.off_route {
                off += 1;
            }
            if let Some(s) = &u.speech {
                spoken += 1;
                if spoken <= 25 {
                    println!("{:>8.0} m  {s}", d);
                }
            }
            last = Some(u);
            d += 16.7;
        }
        let last = last.unwrap();
        println!("spoken {spoken}, off-route fixes {off}, arrived {}", last.arrived);
        assert_eq!(off, 0);
        assert!(last.arrived);
        assert!(spoken > 5);
    }
}
