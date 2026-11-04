//! "Why is there no bus?" – a notice for transit answers with no (or only a much later) ride:
//! Shabbat, a holiday, late night, or simply no service, with the first departure from the
//! origin's stops after the break. The UI builds the sentence from `kind` (texts in i18n).

use chrono::Duration;

use super::holidays::{rest_day, RestDay};
use super::journey::{candidate_stops, iso_at, resolve_time};
use super::model::{Transit, NO_PICKUP};
use crate::types::{LegKind, NoticeKind, Route, RouteRequest, RouteResponse, ServiceNotice};

/// A ride this much later than asked counts as "no service now".
pub const LATE_GAP_S: i32 = 3 * 3600;

/// Earliest departure (pickup allowed) of any of `cands` = (pattern, position) at or after GTFS
/// time `secs` of `date`, over the previous service day (its trips after midnight) and the next
/// four. Returns the ISO time.
pub fn first_departure(t: &Transit, cands: &[(u32, u32)], date: chrono::NaiveDate, secs: i32, accessible: bool) -> Option<String> {
    let mut best: Option<(i64, String)> = None;
    for k in -1i64..=4 {
        let d = date + Duration::days(k);
        let Some(day) = t.day_index(d) else { continue };
        let start = secs as i64 - k * 86_400;
        let origin = super::journey::service_origin(d).timestamp();
        for &(p, pos) in cands {
            let pt = &t.patterns[p as usize];
            if pos + 1 >= pt.n || t.pflags(p)[pos as usize] & NO_PICKUP != 0 {
                continue;
            }
            // trips of a pattern never overtake: departures at `pos` are sorted
            let (mut lo, mut hi) = (0u32, pt.ntrips);
            while lo < hi {
                let mid = (lo + hi) / 2;
                if (t.dep_at(p, mid, pos) as i64) < start {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            for tr in lo..pt.ntrips {
                let g = pt.trips_start + tr;
                if !t.service_active(t.trip_service[g as usize], day) || (accessible && !t.trip_accessible(g)) {
                    continue;
                }
                let dep = t.dep_at(p, tr, pos);
                let abs = origin + dep as i64;
                if best.as_ref().is_none_or(|b| abs < b.0) {
                    best = Some((abs, iso_at(d, dep)));
                }
                break;
            }
        }
    }
    best.map(|b| b.1)
}

/// (pattern, position) pairs of every pattern calling at the stops.
pub fn stop_cands(t: &Transit, stops: &[u32]) -> Vec<(u32, u32)> {
    stops.iter().flat_map(|&s| t.stop_patterns(s)).collect()
}

fn first_transit_departure(routes: &[Route]) -> Option<i64> {
    routes
        .iter()
        .filter_map(|r| r.legs.iter().find(|l| l.kind == LegKind::Transit))
        .filter_map(|l| l.depart_iso.as_deref())
        .filter_map(|iso| chrono::DateTime::parse_from_rfc3339(iso).ok().map(|d| d.timestamp()))
        .min()
}

/// The notice for a transit answer, None when service looks normal.
pub fn service_notice(t: &Transit, req: &RouteRequest, routes: &[Route]) -> Option<ServiceNotice> {
    let (date, secs, arrive_by) = resolve_time(&req.time).ok()?;
    let asked = super::journey::service_origin(date).timestamp() + secs as i64;
    match first_transit_departure(routes) {
        Some(_) if arrive_by => return None,
        Some(dep) if dep - asked <= LATE_GAP_S as i64 => return None,
        _ => {}
    }
    let origin = req.waypoints.first()?;
    let stops = candidate_stops(t, [origin.lon, origin.lat], req.accessible);
    let next = if stops.is_empty() { None } else { first_departure(t, &stop_cands(t, &stops), date, secs, req.accessible) };
    let hour = (secs.rem_euclid(86_400) / 3600) as u32;
    let (kind, holiday_name) = match rest_day(date, hour) {
        Some(RestDay::Holiday(name)) => (NoticeKind::Holiday, Some(name.to_string())),
        Some(RestDay::Shabbat) => (NoticeKind::Shabbat, None),
        None if !(5..23).contains(&hour) => (NoticeKind::LateNight, None),
        None => (NoticeKind::NoService, None),
    };
    Some(ServiceNotice { kind, next_service_iso: next, holiday_name })
}

/// Add the notice to a response computed elsewhere (online journeys).
pub fn attach_notice(t: &Transit, req: &RouteRequest, resp: &mut RouteResponse) {
    if resp.notice.is_none() {
        resp.notice = service_notice(t, req, &resp.routes);
    }
}
