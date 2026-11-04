//! Online routing providers (used first when the routing mode is "auto"/"online"):
//! - OSRM (FOSSGIS public instances by default) for car / bike / foot.
//! - MOTIS / Transitous for public transport.
//!
//! Every function here fails fast (short timeouts) so the caller can fall back to the offline engines.
//! Providers implement [`RouteProvider`]; [`providers`] lists them in the order they are tried.

pub mod motis;
pub mod osrm;
pub mod photon;
pub mod polyline;
pub mod stride;

use std::time::Duration;

use crate::types::{Mode, RouteRequest, RouteResponse, Settings};

pub const USER_AGENT: &str = crate::data::download::USER_AGENT;

pub fn client() -> anyhow::Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        // a stalled connection must not hold every route back: ~6 s and the local engine answers
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(6))
        .build()?)
}

/// An online routing service.
pub trait RouteProvider: Sync {
    /// Short id reported as `RouteResponse.provider`.
    fn name(&self) -> &'static str;
    /// Can it answer this request (mode, options)?
    fn supports(&self, req: &RouteRequest) -> bool;
    fn route(&self, req: &RouteRequest, settings: &Settings) -> anyhow::Result<RouteResponse>;
}

pub struct Osrm;
pub struct Motis;

impl RouteProvider for Osrm {
    fn name(&self) -> &'static str {
        "osrm"
    }
    fn supports(&self, req: &RouteRequest) -> bool {
        // truck / wheelchair profiles and user-drawn avoid areas exist only in the local engine
        matches!(req.mode, Mode::Car | Mode::Bike | Mode::Foot) && !req.avoid.needs_local() && req.vehicle.is_none()
    }
    fn route(&self, req: &RouteRequest, settings: &Settings) -> anyhow::Result<RouteResponse> {
        osrm::route(req, &settings.osrm_url)
    }
}

impl RouteProvider for Motis {
    fn name(&self) -> &'static str {
        "transitous"
    }
    fn supports(&self, req: &RouteRequest) -> bool {
        req.mode == Mode::Transit && !req.accessible
    }
    fn route(&self, req: &RouteRequest, settings: &Settings) -> anyhow::Result<RouteResponse> {
        motis::plan(req, &settings.transit_api_url)
    }
}

/// Providers in the order they are tried.
pub fn providers() -> &'static [&'static dyn RouteProvider] {
    &[&Osrm, &Motis]
}

/// Can any online provider answer this request? (When not, the local engine answers directly.)
pub fn supports(req: &RouteRequest) -> bool {
    providers().iter().any(|p| p.supports(req))
}

/// Route online according to `req.mode`. Errors mean "fall back to offline".
pub fn route(req: &RouteRequest, settings: &Settings) -> anyhow::Result<RouteResponse> {
    let mut last = None;
    for p in providers().iter().filter(|p| p.supports(req)) {
        match p.route(req, settings) {
            Ok(r) => return Ok(r),
            Err(e) => {
                log::info!("online provider {} failed: {e:#}", p.name());
                last = Some(e);
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("no online provider for mode {}", req.mode.as_str())))
}

/// Convert a UTC / offset ISO timestamp to Asia/Jerusalem RFC3339 (seconds precision).
pub fn to_local_iso(iso: &str) -> Option<String> {
    let dt = chrono::DateTime::parse_from_rfc3339(iso).ok()?;
    Some(dt.with_timezone(&chrono_tz::Asia::Jerusalem).to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}
