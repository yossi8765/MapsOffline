//! Online routing providers (used first when the routing mode is "auto"/"online"):
//! - OSRM (FOSSGIS public instances by default) for car / bike / foot.
//! - MOTIS / Transitous for public transport.
//! Every function here fails fast (short timeouts) so the caller can fall back to the offline engines.

pub mod motis;
pub mod osrm;
pub mod polyline;

use std::time::Duration;

use crate::types::{Mode, RouteRequest, RouteResponse, Settings};

pub const USER_AGENT: &str = crate::data::download::USER_AGENT;

pub fn client() -> anyhow::Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(4))
        .timeout(Duration::from_secs(15))
        .build()?)
}

/// Route online according to `req.mode`. Errors mean "fall back to offline".
pub fn route(req: &RouteRequest, settings: &Settings) -> anyhow::Result<RouteResponse> {
    match req.mode {
        Mode::Transit => motis::plan(req, &settings.transit_api_url),
        Mode::Car | Mode::Bike | Mode::Foot => osrm::route(req, &settings.osrm_url),
    }
}

/// Convert a UTC / offset ISO timestamp to Asia/Jerusalem RFC3339 (seconds precision).
pub fn to_local_iso(iso: &str) -> Option<String> {
    let dt = chrono::DateTime::parse_from_rfc3339(iso).ok()?;
    Some(dt.with_timezone(&chrono_tz::Asia::Jerusalem).to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}
