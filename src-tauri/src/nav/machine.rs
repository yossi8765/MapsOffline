//! Turn-by-turn state machine: snaps GPS fixes to the active route, tracks the current step and
//! the distance to the next maneuver, detects leaving the route and arrival, and decides when to
//! speak an instruction.
//!
//! Written here instead of using the `ferrostar` core crate: ferrostar 0.59 pulls in `geo`,
//! `proptest` (a regular dependency of its `std` feature), `serde_with`, `uuid`, `rand` and
//! `uniffi` by default, and expects OSRM/Valhalla responses with banner and voice instructions
//! that our engines do not produce (they would have to be synthesised anyway). The parts we need
//! – snapping, step advance, deviation and announcement timing – fit in this file.

use crate::geo::{bearing_deg, haversine_m, local_proj};
use crate::types::{Lane, LonLat, Maneuver, Route};

/// Metres from the route before a fix counts as "off the route".
pub const OFF_ROUTE_M: f64 = 40.0;
/// Consecutive off-route fixes (≈ 1 s apart) before a reroute is requested.
pub const OFF_ROUTE_FIXES: u32 = 3;
/// Distance to the end of the route that counts as arrived.
pub const ARRIVE_M: f64 = 25.0;
/// Fixes less accurate than this (m) come from Wi-Fi / cell positioning: "approximate".
pub const APPROXIMATE_M: f64 = 50.0;
/// Speed cameras are announced this far ahead (m).
pub const CAMERA_WARN_M: f64 = 400.0;
pub const CAMERA_SHOW_M: f64 = 1_500.0;

/// Off-route threshold for a fix of the given accuracy: never below [`OFF_ROUTE_M`], wider for
/// coarse positions (a laptop without GPS must not reroute every few seconds).
pub fn off_route_threshold(accuracy_m: Option<f64>) -> f64 {
    accuracy_m.map_or(OFF_ROUTE_M, |a| (a * 1.5).clamp(OFF_ROUTE_M, 400.0))
}

/// One maneuver on the flattened route.
#[derive(Debug, Clone)]
pub struct NavStep {
    pub instruction: String,
    pub maneuver: Maneuver,
    pub street: Option<String>,
    /// Distance along the route where the maneuver happens.
    pub at_m: f64,
    /// Length and duration of the stretch that *leads to* this maneuver (the previous step).
    pub lead_len_m: f64,
    pub lead_sec: f64,
    pub leg: usize,
    /// posted limit on the stretch after this maneuver
    pub speed_limit_kmh: Option<u32>,
    /// lanes before this maneuver
    pub lanes: Vec<Lane>,
}

/// The active route, flattened over its legs.
#[derive(Debug, Clone)]
pub struct NavRoute {
    pub route_id: String,
    pub pts: Vec<LonLat>,
    /// Cumulative distance at each point.
    pub cum: Vec<f64>,
    /// Leg index of the segment that starts at each point.
    pub seg_leg: Vec<usize>,
    pub steps: Vec<NavStep>,
    pub total_m: f64,
    pub total_sec: f64,
    /// Seconds from the start to each point (step durations spread over their geometry), so the
    /// remaining time follows the real speeds of the remaining stretch, not the average.
    pub cum_sec: Vec<f64>,
    /// Distance along the route of each speed camera on it.
    pub cameras: Vec<f64>,
}

impl NavRoute {
    pub fn from_route(r: &Route) -> Self {
        let mut pts: Vec<LonLat> = Vec::new();
        let mut seg_leg = Vec::new();
        // steps with their index in the flattened geometry
        let mut steps_raw: Vec<(usize, usize, &crate::types::Step)> = Vec::new();
        for (li, leg) in r.legs.iter().enumerate() {
            let skip_first = matches!((pts.last(), leg.geometry.first()), (Some(a), Some(b)) if a == b);
            let off = if skip_first { pts.len() - 1 } else { pts.len() };
            for (i, p) in leg.geometry.iter().enumerate() {
                if i == 0 && skip_first {
                    continue;
                }
                pts.push(*p);
                seg_leg.push(li);
            }
            for s in &leg.steps {
                steps_raw.push((li, off + s.geometry_start.min(leg.geometry.len().saturating_sub(1)), s));
            }
        }
        if pts.is_empty() {
            pts = r.geometry.clone();
            seg_leg = vec![0; pts.len()];
        }
        let mut cum = Vec::with_capacity(pts.len());
        let mut d = 0.0;
        for i in 0..pts.len() {
            if i > 0 {
                d += haversine_m(pts[i - 1], pts[i]);
            }
            cum.push(d);
        }
        let total_m = d;
        let mut steps: Vec<NavStep> = Vec::new();
        let mut prev: Option<&crate::types::Step> = None;
        // (point index, duration) of each step for the time profile
        let mut timing: Vec<(usize, f64)> = Vec::new();
        for (li, idx, s) in steps_raw {
            let at_m = cum.get(idx).copied().unwrap_or(total_m);
            timing.push((idx, s.duration_sec));
            steps.push(NavStep {
                instruction: s.instruction.clone(),
                maneuver: s.maneuver,
                street: s.street.clone(),
                at_m,
                lead_len_m: prev.map_or(0.0, |p| p.distance_m),
                lead_sec: prev.map_or(0.0, |p| p.duration_sec),
                leg: li,
                speed_limit_kmh: s.speed_limit_kmh,
                lanes: s.lanes.clone(),
            });
            prev = Some(s);
        }
        let total_sec = r.duration_sec.max(1.0);
        let cum_sec = time_profile(&cum, &timing, total_sec);
        // routes without steps (some online answers): a single arrive step
        if steps.iter().all(|s| s.maneuver != Maneuver::Arrive) {
            steps.push(NavStep {
                instruction: "הגעת ליעד".into(),
                maneuver: Maneuver::Arrive,
                street: None,
                at_m: total_m,
                lead_len_m: prev.map_or(total_m, |p| p.distance_m),
                lead_sec: prev.map_or(r.duration_sec, |p| p.duration_sec),
                leg: r.legs.len().saturating_sub(1),
                speed_limit_kmh: None,
                lanes: Vec::new(),
            });
        }
        Self { route_id: r.id.clone(), pts, cum, seg_leg, steps, total_m, total_sec, cum_sec, cameras: Vec::new() }
    }

    /// Seconds from the start at distance `d` along the route.
    pub fn sec_at(&self, d: f64) -> f64 {
        let n = self.cum.len();
        if n == 0 {
            return 0.0;
        }
        let i = self.cum.partition_point(|&c| c <= d);
        if i == 0 {
            return 0.0;
        }
        if i >= n {
            return self.cum_sec[n - 1];
        }
        let (c0, c1) = (self.cum[i - 1], self.cum[i]);
        let t = if c1 > c0 { (d - c0) / (c1 - c0) } else { 0.0 };
        self.cum_sec[i - 1] + (self.cum_sec[i] - self.cum_sec[i - 1]) * t
    }

    /// Place speed cameras (positions) that lie on the route (≤ 30 m from it).
    pub fn set_cameras(&mut self, cams: &[LonLat]) {
        let mut out = Vec::new();
        for &c in cams {
            let s = snap(self, c, None, 0);
            if s.dist_m <= 30.0 {
                out.push(s.along_m);
            }
        }
        out.sort_by(|a, b| a.total_cmp(b));
        out.dedup_by(|a, b| (*a - *b).abs() < 20.0);
        self.cameras = out;
    }
}

/// Cumulative seconds per point: each step's duration spread over its points by distance;
/// stretches without step information get the route's average pace. Scaled to `total_sec`.
fn time_profile(cum: &[f64], timing: &[(usize, f64)], total_sec: f64) -> Vec<f64> {
    let n = cum.len();
    if n == 0 {
        return Vec::new();
    }
    let total_m = cum[n - 1].max(1e-6);
    let mut out = vec![f64::NAN; n];
    out[0] = 0.0;
    let mut k = 0;
    while k < timing.len() {
        let (a, dur) = timing[k];
        let b = timing.get(k + 1).map_or(n - 1, |t| t.0).min(n - 1);
        if b > a && a < n {
            let (d0, d1) = (cum[a], cum[b]);
            let t0 = if out[a].is_finite() { out[a] } else { total_sec * d0 / total_m };
            for i in a..=b {
                let f = if d1 > d0 { (cum[i] - d0) / (d1 - d0) } else { 1.0 };
                out[i] = t0 + dur * f;
            }
        }
        k += 1;
    }
    // gaps: average pace
    for i in 0..n {
        if !out[i].is_finite() {
            out[i] = total_sec * cum[i] / total_m;
        }
    }
    for i in 1..n {
        if out[i] < out[i - 1] {
            out[i] = out[i - 1];
        }
    }
    let last = out[n - 1];
    if last > 0.0 {
        let k = total_sec / last;
        for v in &mut out {
            *v *= k;
        }
    }
    out
}

/// Where a fix lies on the route.
#[derive(Debug, Clone, Copy)]
pub struct Snap {
    pub seg: usize,
    pub along_m: f64,
    pub dist_m: f64,
    pub point: LonLat,
}

/// Nearest point of the route to `p`. Searches forward from the last snapped segment first (a
/// vehicle rarely jumps back), and the whole route when that window is far off. A heading that
/// disagrees with a segment by more than 100° makes that segment 25 m "further" (parallel
/// carriageways, loops).
pub fn snap(route: &NavRoute, p: LonLat, heading: Option<f64>, last_seg: usize) -> Snap {
    snap_within(route, p, heading, last_seg, OFF_ROUTE_M)
}

/// [`snap`] that keeps the forward window for fixes up to `max_window_m` off the route.
pub fn snap_within(route: &NavRoute, p: LonLat, heading: Option<f64>, last_seg: usize, max_window_m: f64) -> Snap {
    let n = route.pts.len();
    if n < 2 {
        let q = route.pts.first().copied().unwrap_or(p);
        return Snap { seg: 0, along_m: 0.0, dist_m: haversine_m(p, q), point: q };
    }
    let eval = |i: usize| -> (f64, Snap) {
        let (a, b) = (route.pts[i], route.pts[i + 1]);
        let (bx, by) = local_proj(a, b);
        let (px, py) = local_proj(a, p);
        let len2 = bx * bx + by * by;
        let t = if len2 > 0.0 { ((px * bx + py * by) / len2).clamp(0.0, 1.0) } else { 0.0 };
        let q = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
        let d = haversine_m(p, q);
        let mut score = d;
        if let Some(h) = heading {
            if len2 > 1.0 {
                let diff = ((bearing_deg(a, b) - h + 540.0) % 360.0 - 180.0).abs();
                if diff > 100.0 {
                    score += 25.0;
                }
            }
        }
        let seg_len = route.cum[i + 1] - route.cum[i];
        (score, Snap { seg: i, along_m: route.cum[i] + seg_len * t, dist_m: d, point: q })
    };
    let best_in = |lo: usize, hi: usize| -> Option<(f64, Snap)> {
        (lo..hi.min(n - 1)).map(eval).min_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal))
    };
    let lo = last_seg.saturating_sub(3);
    let window = best_in(lo, last_seg + 400);
    match window {
        Some((score, s)) if score <= OFF_ROUTE_M.max(max_window_m) => s,
        _ => {
            let all = best_in(0, n - 1).map(|x| x.1);
            all.unwrap_or(Snap { seg: 0, along_m: 0.0, dist_m: haversine_m(p, route.pts[0]), point: route.pts[0] })
        }
    }
}

/// A position fix.
#[derive(Debug, Clone, Copy)]
pub struct Fix {
    pub lon: f64,
    pub lat: f64,
    /// Degrees from north, when the source knows it (moving).
    pub heading: Option<f64>,
    pub speed_mps: Option<f64>,
    /// Horizontal accuracy (m), when the source reports it.
    pub accuracy_m: Option<f64>,
}

/// What the UI shows after each fix (event "nav-update").
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NavUpdate {
    pub route_id: String,
    pub lon: f64,
    pub lat: f64,
    pub snapped_lon: f64,
    pub snapped_lat: f64,
    /// Bearing of the route at the snapped point (for a heading-up camera).
    pub bearing: f64,
    pub step_index: usize,
    pub maneuver: Maneuver,
    pub dist_to_maneuver_m: f64,
    pub instruction: String,
    pub street: Option<String>,
    pub next_instruction: Option<String>,
    pub next_maneuver: Option<Maneuver>,
    pub remaining_m: f64,
    pub remaining_sec: f64,
    pub eta_iso: String,
    pub leg_index: usize,
    pub off_route: bool,
    /// Set once per deviation: the UI computes a new route from here.
    pub reroute_needed: bool,
    pub arrived: bool,
    /// Text spoken for this fix, if any (the backend speaks it when voice is on).
    pub speech: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_kmh: Option<f64>,
    /// posted limit on the current stretch
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_limit_kmh: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accuracy_m: Option<f64>,
    /// coarse position (Wi-Fi / cell): off-route detection is relaxed
    pub approximate: bool,
    /// next speed camera on the route, metres ahead (within 1.5 km)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera_ahead_m: Option<f64>,
    /// lanes before the next maneuver
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lanes: Vec<Lane>,
}

/// Session state between fixes.
#[derive(Debug)]
pub struct Navigator {
    pub route: NavRoute,
    last_seg: usize,
    off_count: u32,
    reroute_sent: bool,
    arrived: bool,
    /// (step index, threshold) already announced.
    announced: Vec<(usize, u32)>,
    started_announce: bool,
    /// Last sentence and where it was said: two consecutive steps with the same text (a road
    /// split into pieces) must not repeat it.
    last_speech: Option<(String, f64)>,
    /// cameras (index into `route.cameras`) already announced
    cameras_said: Vec<usize>,
}

impl Navigator {
    pub fn new(route: NavRoute) -> Self {
        Self { route, last_seg: 0, off_count: 0, reroute_sent: false, arrived: false, announced: Vec::new(), started_announce: false, last_speech: None, cameras_said: Vec::new() }
    }

    /// Thresholds before a maneuver (metres): longer on fast roads. "Continue straight" is said
    /// once (the middle distance), not three times.
    fn thresholds(step: &NavStep) -> &'static [u32] {
        let kmh = if step.lead_sec > 0.0 { step.lead_len_m / step.lead_sec * 3.6 } else { 50.0 };
        let fast = kmh >= 70.0;
        match (step.maneuver == Maneuver::Straight, fast) {
            (true, true) => &[500],
            (true, false) => &[150],
            (false, true) => &[1500, 500, 150],
            (false, false) => &[500, 150, 30],
        }
    }

    pub fn update(&mut self, fix: Fix, now: chrono::DateTime<chrono::FixedOffset>) -> NavUpdate {
        let r = &self.route;
        let p = [fix.lon, fix.lat];
        let heading = fix.heading.filter(|_| fix.speed_mps.is_none_or(|v| v > 2.0));
        let threshold = off_route_threshold(fix.accuracy_m);
        let approximate = fix.accuracy_m.is_some_and(|a| a > APPROXIMATE_M);
        let s = snap_within(r, p, heading, self.last_seg, threshold);
        let off = s.dist_m > threshold;
        if off {
            self.off_count += 1;
        } else {
            self.off_count = 0;
            self.reroute_sent = false;
            self.last_seg = s.seg;
        }
        let off_route = self.off_count >= OFF_ROUTE_FIXES;
        let reroute_needed = off_route && !self.reroute_sent;
        if reroute_needed {
            self.reroute_sent = true;
        }
        let along = s.along_m;
        let remaining_m = (r.total_m - along).max(0.0);
        if remaining_m < ARRIVE_M && !off {
            self.arrived = true;
        }
        // the next maneuver: first step after the current position (the depart step at 0 is
        // skipped once moving)
        let idx = r.steps.iter().position(|st| st.at_m > along + 3.0).unwrap_or(r.steps.len() - 1);
        let st = &r.steps[idx];
        let dist = (st.at_m - along).max(0.0);
        let next = r.steps.get(idx + 1);
        let remaining_sec = (r.total_sec - r.sec_at(along)).max(0.0);
        let eta = now + chrono::Duration::seconds(remaining_sec.round() as i64);
        let seg = s.seg.min(r.pts.len().saturating_sub(2));
        let bearing = if r.pts.len() >= 2 { bearing_deg(r.pts[seg], r.pts[seg + 1]) } else { 0.0 };

        let mut speech = None;
        if reroute_needed {
            speech = Some("יצאת מהמסלול, מחשב מסלול מחדש".to_string());
        } else if self.arrived {
            if !self.announced.contains(&(usize::MAX, 0)) {
                self.announced.push((usize::MAX, 0));
                speech = Some("הגעת ליעד".to_string());
            }
        } else if !off_route {
            if !self.started_announce {
                // first fix: say where we are heading
                self.started_announce = true;
                speech = Some(if dist > 2000.0 {
                    format!("המשך {} ואז {}", say_distance(dist), st.instruction)
                } else {
                    format!("בעוד {} {}", say_distance(dist), st.instruction)
                });
                for &t in Self::thresholds(st) {
                    if dist <= t as f64 {
                        self.announced.push((idx, t));
                    }
                }
            } else {
                // the smallest threshold already passed that was not said yet
                let ths = Self::thresholds(st);
                if let Some(&t) = ths.iter().rev().find(|&&t| dist <= t as f64) {
                    if !self.announced.contains(&(idx, t)) {
                        for &u in ths {
                            if u >= t {
                                self.announced.push((idx, u));
                            }
                        }
                        speech = Some(if t <= 30 { st.instruction.clone() } else { format!("בעוד {} {}", say_distance(dist), st.instruction) });
                    }
                }
            }
        }

        // speed cameras: shown within 1.5 km, said once at ~400 m
        let cam = r.cameras.iter().enumerate().find(|(_, &c)| c >= along - 5.0 && c - along <= CAMERA_SHOW_M).map(|(i, &c)| (i, (c - along).max(0.0)));
        if let Some((ci, d)) = cam {
            if d <= CAMERA_WARN_M && !self.cameras_said.contains(&ci) && !off_route && !self.arrived {
                self.cameras_said.push(ci);
                let warn = format!("מצלמת מהירות בעוד {}", say_distance(d));
                speech = Some(match speech {
                    Some(t) => format!("{t}. {warn}"),
                    None => warn,
                });
            }
        }

        if let Some(text) = &speech {
            let repeat = self.last_speech.as_ref().is_some_and(|(t, at)| t == text && along - at < 300.0);
            if repeat && !reroute_needed {
                speech = None;
            } else {
                self.last_speech = Some((text.clone(), along));
            }
        }

        NavUpdate {
            route_id: r.route_id.clone(),
            lon: fix.lon,
            lat: fix.lat,
            snapped_lon: s.point[0],
            snapped_lat: s.point[1],
            bearing,
            step_index: idx,
            maneuver: st.maneuver,
            dist_to_maneuver_m: dist,
            instruction: st.instruction.clone(),
            street: st.street.clone(),
            next_instruction: next.map(|n| n.instruction.clone()),
            next_maneuver: next.map(|n| n.maneuver),
            remaining_m,
            remaining_sec,
            eta_iso: eta.to_rfc3339(),
            leg_index: r.seg_leg.get(seg).copied().unwrap_or(0),
            off_route,
            reroute_needed,
            arrived: self.arrived,
            speech,
            speed_kmh: fix.speed_mps.filter(|v| v.is_finite() && *v >= 0.0).map(|v| (v * 3.6 * 10.0).round() / 10.0),
            // the stretch being driven belongs to the step before the next maneuver
            speed_limit_kmh: idx.checked_sub(1).and_then(|k| r.steps.get(k)).and_then(|x| x.speed_limit_kmh),
            accuracy_m: fix.accuracy_m,
            approximate,
            camera_ahead_m: cam.map(|c| c.1.round()),
            lanes: st.lanes.clone(),
        }
    }
}

/// Spoken Hebrew distance: "150 מטר", "קילומטר", "קילומטר וחצי", "3 קילומטר".
pub fn say_distance(m: f64) -> String {
    if m < 1000.0 {
        let r = if m < 100.0 { (m / 10.0).round() * 10.0 } else { (m / 50.0).round() * 50.0 };
        if r >= 1000.0 {
            return "קילומטר".into();
        }
        return format!("{} מטר", r.max(10.0) as i64);
    }
    let half = (m / 500.0).round() / 2.0;
    if half == 1.0 {
        "קילומטר".into()
    } else if half == 1.5 {
        "קילומטר וחצי".into()
    } else if half.fract() == 0.0 {
        format!("{} קילומטר", half as i64)
    } else {
        format!("{} קילומטר", half)
    }
}

/// Position `d` metres along the route (for the simulator).
pub fn point_at(route: &NavRoute, d: f64) -> (LonLat, f64) {
    let n = route.pts.len();
    if n == 0 {
        return ([0.0, 0.0], 0.0);
    }
    if d <= 0.0 || n == 1 {
        let b = if n > 1 { bearing_deg(route.pts[0], route.pts[1]) } else { 0.0 };
        return (route.pts[0], b);
    }
    let i = route.cum.partition_point(|&c| c <= d).clamp(1, n - 1);
    let (a, b) = (route.pts[i - 1], route.pts[i]);
    let seg = route.cum[i] - route.cum[i - 1];
    let t = if seg > 0.0 { ((d - route.cum[i - 1]) / seg).clamp(0.0, 1.0) } else { 1.0 };
    ([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t], bearing_deg(a, b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Leg, LegKind, Mode, Step};

    fn step(instr: &str, m: Maneuver, start: usize, dist: f64, sec: f64) -> Step {
        Step::basic(instr.into(), m, None, None, dist, sec, start)
    }

    /// An L-shaped city route: 1 km east, then 600 m north.
    fn l_route() -> Route {
        let o: LonLat = [34.78, 32.08];
        let east = |m: f64| [o[0] + m / (111_320.0 * o[1].to_radians().cos()), o[1]];
        let mut g: Vec<LonLat> = (0..=10).map(|i| east(i as f64 * 100.0)).collect();
        let corner = *g.last().unwrap();
        for i in 1..=6 {
            g.push([corner[0], corner[1] + i as f64 * 100.0 / 111_320.0]);
        }
        let steps = vec![
            step("צא מזרחה", Maneuver::Depart, 0, 1000.0, 120.0),
            step("פנה שמאלה לרחוב הרצל", Maneuver::TurnLeft, 10, 600.0, 72.0),
            step("הגעת ליעד", Maneuver::Arrive, 16, 0.0, 0.0),
        ];
        let leg = Leg { kind: LegKind::Drive, duration_sec: 192.0, distance_m: 1600.0, geometry: g.clone(), depart_iso: None, arrive_iso: None, steps, transit: None };
        Route { id: "r1".into(), mode: Mode::Car, duration_sec: 192.0, distance_m: 1600.0, geometry: g, legs: vec![leg], tags: vec![], depart_iso: None, arrive_iso: None, transfers: 0, walk_m: 0.0, fare_estimate_ils: None, fare_note: None, avoid_violations: vec![] }
    }

    fn now() -> chrono::DateTime<chrono::FixedOffset> {
        chrono::DateTime::parse_from_rfc3339("2026-11-04T10:00:00+02:00").unwrap()
    }

    #[test]
    fn drives_the_route_with_announcements_and_arrives() {
        let nr = NavRoute::from_route(&l_route());
        assert!((nr.total_m - 1600.0).abs() < 5.0, "{}", nr.total_m);
        let mut nav = Navigator::new(nr.clone());
        let mut spoken = Vec::new();
        let mut d = 0.0;
        let mut last = None;
        while d <= nr.total_m + 10.0 {
            let (p, b) = point_at(&nr, d);
            let u = nav.update(Fix { lon: p[0], lat: p[1], heading: Some(b), speed_mps: Some(14.0), accuracy_m: None }, now());
            assert!(!u.off_route);
            if let Some(s) = &u.speech {
                spoken.push((d.round(), s.clone()));
            }
            last = Some(u);
            d += 14.0;
        }
        let last = last.unwrap();
        assert!(last.arrived);
        let text: Vec<&str> = spoken.iter().map(|x| x.1.as_str()).collect();
        // start, then 500 / 150 / ~30 m before the left turn, then arrival announcements
        assert!(text[0].starts_with("בעוד קילומטר") || text[0].starts_with("בעוד"), "{text:?}");
        assert!(text.iter().any(|s| s.starts_with("בעוד 500 מטר פנה שמאלה")), "{text:?}");
        assert!(text.iter().any(|s| s.starts_with("בעוד 150 מטר פנה שמאלה")), "{text:?}");
        assert!(text.contains(&"פנה שמאלה לרחוב הרצל"), "{text:?}");
        assert_eq!(text.last().copied(), Some("הגעת ליעד"));
        // each sentence once
        let mut sorted = text.clone();
        sorted.dedup();
        assert_eq!(sorted.len(), text.len());
    }

    #[test]
    fn distance_to_maneuver_and_step_advance() {
        let nr = NavRoute::from_route(&l_route());
        let mut nav = Navigator::new(nr.clone());
        let (p, b) = point_at(&nr, 300.0);
        let u = nav.update(Fix { lon: p[0], lat: p[1], heading: Some(b), speed_mps: None, accuracy_m: None }, now());
        assert_eq!(u.maneuver, Maneuver::TurnLeft);
        assert!((u.dist_to_maneuver_m - 700.0).abs() < 3.0, "{}", u.dist_to_maneuver_m);
        assert!((u.remaining_m - 1300.0).abs() < 3.0);
        assert!((u.remaining_sec - 156.0).abs() < 1.0, "{}", u.remaining_sec);
        let (p, b) = point_at(&nr, 1200.0);
        let u = nav.update(Fix { lon: p[0], lat: p[1], heading: Some(b), speed_mps: None, accuracy_m: None }, now());
        assert_eq!(u.maneuver, Maneuver::Arrive);
        assert!((u.bearing - 0.0).abs() < 1.0 || (u.bearing - 360.0).abs() < 1.0, "{}", u.bearing);
    }

    #[test]
    fn leaving_the_route_requests_one_reroute() {
        let nr = NavRoute::from_route(&l_route());
        let mut nav = Navigator::new(nr.clone());
        let (p, _) = point_at(&nr, 200.0);
        nav.update(Fix { lon: p[0], lat: p[1], heading: None, speed_mps: None, accuracy_m: None }, now());
        // 80 m south of the road, three fixes
        let away = [p[0], p[1] - 80.0 / 111_320.0];
        let f = Fix { lon: away[0], lat: away[1], heading: None, speed_mps: None, accuracy_m: None };
        let a = nav.update(f, now());
        let b = nav.update(f, now());
        let c = nav.update(f, now());
        let d = nav.update(f, now());
        assert!(!a.off_route && !b.off_route);
        assert!(c.off_route && c.reroute_needed);
        assert!(c.speech.as_deref().unwrap().contains("מחשב מסלול מחדש"));
        assert!(d.off_route && !d.reroute_needed, "only one reroute per deviation");
        // back on the road resets it
        let e = nav.update(Fix { lon: p[0], lat: p[1], heading: None, speed_mps: None, accuracy_m: None }, now());
        assert!(!e.off_route);
    }

    #[test]
    fn gps_noise_stays_on_route() {
        let nr = NavRoute::from_route(&l_route());
        let mut nav = Navigator::new(nr.clone());
        for (i, d) in (0..60).map(|i| (i, i as f64 * 25.0)) {
            let (p, b) = point_at(&nr, d);
            // ±15 m zig-zag
            let off = if i % 2 == 0 { 15.0 } else { -15.0 } / 111_320.0;
            let u = nav.update(Fix { lon: p[0] + off * 0.3, lat: p[1] + off, heading: Some(b), speed_mps: Some(10.0), accuracy_m: None }, now());
            assert!(!u.off_route, "fix {i} at {d} m");
        }
    }

    /// A route that is slow at first (city, 600 s for 1 km) and fast later (600 m in 72 s): the
    /// remaining time follows the step durations, not the average speed.
    #[test]
    fn eta_by_step_seconds() {
        let mut r = l_route();
        r.legs[0].steps[0].duration_sec = 600.0;
        r.duration_sec = 672.0;
        let nr = NavRoute::from_route(&r);
        let mut nav = Navigator::new(nr.clone());
        let (p, b) = point_at(&nr, 1000.0);
        let u = nav.update(Fix { lon: p[0], lat: p[1], heading: Some(b), speed_mps: None, accuracy_m: None }, now());
        assert!((u.remaining_sec - 72.0).abs() < 2.0, "{}", u.remaining_sec);
        let (p, b) = point_at(&nr, 500.0);
        let u = nav.update(Fix { lon: p[0], lat: p[1], heading: Some(b), speed_mps: None, accuracy_m: None }, now());
        assert!((u.remaining_sec - 372.0).abs() < 3.0, "{}", u.remaining_sec);
    }

    /// A Wi-Fi position 100 m off the road is not "off route"; a GPS one is.
    #[test]
    fn coarse_fixes_do_not_reroute() {
        let nr = NavRoute::from_route(&l_route());
        let mut nav = Navigator::new(nr.clone());
        let (p, _) = point_at(&nr, 300.0);
        let away = [p[0], p[1] - 100.0 / 111_320.0];
        let mut last = None;
        for _ in 0..5 {
            last = Some(nav.update(Fix { lon: away[0], lat: away[1], heading: None, speed_mps: None, accuracy_m: Some(120.0) }, now()));
        }
        let u = last.take().unwrap();
        assert!(!u.off_route && u.approximate);
        let mut nav = Navigator::new(nr.clone());
        for _ in 0..4 {
            last = Some(nav.update(Fix { lon: away[0], lat: away[1], heading: None, speed_mps: None, accuracy_m: Some(5.0) }, now()));
        }
        assert!(last.unwrap().off_route);
    }

    #[test]
    fn camera_announced_once() {
        let mut nr = NavRoute::from_route(&l_route());
        let (cam, _) = point_at(&nr, 1300.0);
        nr.set_cameras(&[cam, [35.5, 33.0]]);
        assert_eq!(nr.cameras.len(), 1);
        let mut nav = Navigator::new(nr.clone());
        let mut said = 0;
        let mut d = 0.0;
        while d < 1290.0 {
            let (p, b) = point_at(&nr, d);
            let u = nav.update(Fix { lon: p[0], lat: p[1], heading: Some(b), speed_mps: Some(14.0), accuracy_m: Some(5.0) }, now());
            if d > 1300.0 - CAMERA_SHOW_M {
                assert!(u.camera_ahead_m.is_some());
            }
            if u.speech.as_deref().is_some_and(|s| s.contains("מצלמת מהירות")) {
                said += 1;
            }
            assert_eq!(u.speed_kmh, Some(50.4));
            d += 14.0;
        }
        assert_eq!(said, 1);
    }

    #[test]
    fn spoken_distances() {
        assert_eq!(say_distance(148.0), "150 מטר");
        assert_eq!(say_distance(32.0), "30 מטר");
        assert_eq!(say_distance(990.0), "קילומטר");
        assert_eq!(say_distance(1480.0), "קילומטר וחצי");
        assert_eq!(say_distance(3100.0), "3 קילומטר");
        assert_eq!(say_distance(2600.0), "2.5 קילומטר");
    }

    #[test]
    fn real_route_json_simulation() {
        // optional: a real route saved by `nav_dump_route` (see nav/mod.rs) – skipped if absent
        let Ok(p) = std::env::var("MO_NAV_ROUTE_JSON") else { return };
        let r: Route = serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap();
        let nr = NavRoute::from_route(&r);
        let mut nav = Navigator::new(nr.clone());
        let mut d = 0.0;
        let mut n_speech = 0;
        while d <= nr.total_m + 20.0 {
            let (p, b) = point_at(&nr, d);
            let u = nav.update(Fix { lon: p[0], lat: p[1], heading: Some(b), speed_mps: Some(15.0), accuracy_m: None }, now());
            assert!(!u.off_route, "off route at {d}");
            if let Some(s) = u.speech {
                n_speech += 1;
                println!("{:>7.0} m  {s}", d);
            }
            d += 15.0;
        }
        assert!(n_speech > 2);
    }
}
