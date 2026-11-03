//! Timetable views for the favorites panel: a line's departures for the rest of the service day,
//! and a stop's departure board with the next vehicle of every line (schedule-based "countdown";
//! there is no real-time feed offline).
use std::collections::BTreeMap;

use chrono::{Datelike, Timelike, Utc};
use chrono_tz::Asia::Jerusalem;

use super::journey::iso_at;
use super::model::{line_group, Transit, NONE};
use crate::types::{LineDirection, LineSummary, LineTimes, StopBoard, StopInfo, StopLine};

/// (day index, seconds since that service day's midnight) for the current moment, plus the
/// previous service day whose late trips (after 24:00) may still be ahead of us.
fn now_in_feed(t: &Transit) -> Option<Vec<(u32, i32)>> {
    let now = Utc::now().with_timezone(&Jerusalem);
    let secs = now.num_seconds_from_midnight() as i32;
    let today = t.day_index(now.date_naive())?;
    let mut v = vec![(today, secs)];
    if today > 0 {
        v.push((today - 1, secs + 86_400));
    }
    Some(v)
}

fn agency_of(t: &Transit, route: u32) -> &str {
    let r = &t.routes[route as usize];
    if r.agency == NONE {
        ""
    } else {
        t.s(t.agencies[r.agency as usize])
    }
}

fn color_of(t: &Transit, route: u32) -> Option<String> {
    t.routes[route as usize].color.map(|c| format!("#{c:06x}"))
}

/// Agencies in the feed, sorted.
pub fn agencies(t: &Transit) -> Vec<String> {
    let mut v: Vec<String> = t.agencies.iter().map(|&a| t.s(a).to_string()).filter(|a| !a.is_empty()).collect();
    v.sort();
    v.dedup();
    v
}

/// Numeric-aware ordering for line numbers ("1", "2", "10", "10א").
fn line_key(s: &str) -> (u64, String) {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    (digits.parse().unwrap_or(u64::MAX), s.to_string())
}

/// Lines of one agency with their headsigns (first trip of every pattern). Same-numbered lines
/// in different towns ("Egged 1" in Gedera, Eilat, Jerusalem...) are separate entries; their
/// `long_name` tells them apart and is what [`line_times_of`] / `line_shape_of` filter by.
pub fn lines(t: &Transit, agency: &str) -> Vec<LineSummary> {
    let mut map: BTreeMap<(String, String), LineSummary> = BTreeMap::new();
    for pt in &t.patterns {
        if agency_of(t, pt.route) != agency {
            continue;
        }
        let r = &t.routes[pt.route as usize];
        let short = t.s(r.short_name).to_string();
        let e = map.entry((short.clone(), line_group(t.s(r.long_name)))).or_insert_with(|| LineSummary { short_name: short, long_name: t.s(r.long_name).to_string(), color: color_of(t, pt.route), headsigns: Vec::new() });
        if pt.ntrips > 0 {
            let hs = t.s(t.trip_headsign[pt.trips_start as usize]).to_string();
            if !hs.is_empty() && !e.headsigns.contains(&hs) {
                e.headsigns.push(hs);
            }
        }
    }
    let mut v: Vec<LineSummary> = map.into_values().collect();
    v.sort_by_key(|l| line_key(&l.short_name));
    v
}

/// Every departure of the line from now until the end of today's service, per direction.
pub fn line_times(t: &Transit, short_name: &str, agency: &str, headsign: Option<&str>) -> Option<LineTimes> {
    line_times_of(t, short_name, agency, headsign, None)
}

/// [`line_times`] of one line among the agency's same-numbered lines: `long_name` is any
/// route_long_name of it (favorites store it), see [`line_group`].
pub fn line_times_of(t: &Transit, short_name: &str, agency: &str, headsign: Option<&str>, long_name: Option<&str>) -> Option<LineTimes> {
    let group = long_name.filter(|l| !l.is_empty()).map(line_group);
    let days = now_in_feed(t)?;
    let mut dirs: BTreeMap<String, LineDirection> = BTreeMap::new();
    let mut dir_len: BTreeMap<String, u32> = BTreeMap::new(); // stops of the pattern that named the direction's terminals
    let mut long_name = String::new();
    let mut color = None;
    for (pi, pt) in t.patterns.iter().enumerate() {
        let r = &t.routes[pt.route as usize];
        if t.s(r.short_name) != short_name || agency_of(t, pt.route) != agency {
            continue;
        }
        if group.as_ref().is_some_and(|g| line_group(t.s(r.long_name)) != *g) {
            continue;
        }
        long_name = t.s(r.long_name).to_string();
        color = color_of(t, pt.route);
        let stops = t.pstops(pi as u32);
        let (first, last) = (stops[0], stops[stops.len() - 1]);
        for trip in 0..pt.ntrips {
            let svc = t.trip_service[(pt.trips_start + trip) as usize];
            let hs = t.s(t.trip_headsign[(pt.trips_start + trip) as usize]).to_string();
            if headsign.is_some_and(|h| !h.is_empty() && h != hs) {
                continue;
            }
            for &(day, now_secs) in &days {
                if !t.service_active(svc, day) {
                    continue;
                }
                let dep = t.dep_at(pi as u32, trip, 0);
                if dep < now_secs - 60 {
                    continue;
                }
                let d = dirs.entry(hs.clone()).or_insert_with(|| LineDirection {
                    headsign: hs.clone(),
                    from_stop: String::new(),
                    to_stop: String::new(),
                    departures: Vec::new(),
                    duration_sec: 0,
                });
                // terminals and duration come from the direction's longest pattern (short
                // variants exist: a few trips start mid-route)
                if dir_len.get(&hs).is_none_or(|&n| pt.n > n) {
                    dir_len.insert(hs.clone(), pt.n);
                    d.from_stop = t.stop_name(first).to_string();
                    d.to_stop = t.stop_name(last).to_string();
                    d.duration_sec = (t.arr_at(pi as u32, trip, pt.n - 1) - dep).max(0) as u32;
                }
                d.departures.push((dep - now_secs, iso_at(t.date_of_day(day), dep)));
            }
        }
    }
    let mut directions: Vec<LineDirection> = dirs
        .into_values()
        .map(|mut d| {
            d.departures.sort();
            d.departures.dedup();
            d
        })
        .collect();
    directions.sort_by_key(|d| d.departures.first().map(|x| x.0).unwrap_or(i32::MAX));
    Some(LineTimes { short_name: short_name.to_string(), long_name, agency: agency.to_string(), color, directions })
}

/// The stop for a favorite: by stop code when known, else the nearest served stop within 40 m.
fn find_stop(t: &Transit, code: Option<&str>, lon: f64, lat: f64) -> Option<u32> {
    if let Some(code) = code.filter(|c| !c.is_empty()) {
        if let Some(i) = t.stops.iter().position(|s| t.s(s.code) == code) {
            return Some(i as u32);
        }
    }
    t.stops_near([lon, lat], 40.0).into_iter().map(|(i, _)| i).find(|&i| t.stop_is_served(i))
}

/// Departure board: every line calling at the stop today, with its next departures.
pub fn stop_board(t: &Transit, code: Option<&str>, lon: f64, lat: f64) -> Option<StopBoard> {
    let s = find_stop(t, code, lon, lat)?;
    let days = now_in_feed(t)?;
    let today = days[0].0;
    let ll = t.lonlat(s);
    let stop = StopInfo { id: s, name: t.stop_name(s).to_string(), code: t.stop_code(s), lon: ll[0], lat: ll[1] };
    let mut map: BTreeMap<(String, String, String), StopLine> = BTreeMap::new();
    for (p, pos) in t.stop_patterns(s) {
        let pt = &t.patterns[p as usize];
        let r = &t.routes[pt.route as usize];
        let last_pos = pt.n - 1;
        for trip in 0..pt.ntrips {
            let svc = t.trip_service[(pt.trips_start + trip) as usize];
            let hs = t.s(t.trip_headsign[(pt.trips_start + trip) as usize]).to_string();
            let key = (t.s(r.short_name).to_string(), agency_of(t, pt.route).to_string(), hs.clone());
            for &(day, now_secs) in &days {
                if !t.service_active(svc, day) {
                    continue;
                }
                let tm = if pos == last_pos { t.arr_at(p, trip, pos) } else { t.dep_at(p, trip, pos) };
                let e = map.entry(key.clone()).or_insert_with(|| StopLine {
                    short_name: key.0.clone(),
                    agency: key.1.clone(),
                    headsign: hs.clone(),
                    color: color_of(t, pt.route),
                    terminus: pos == last_pos,
                    total_today: 0,
                    next: Vec::new(),
                });
                // today's service day only: yesterday's entry is there for its trips after
                // midnight that are still ahead, not to be counted a second time
                if day == today {
                    e.total_today += 1;
                }
                let rel = tm - now_secs; // seconds from now
                if rel >= -60 {
                    e.next.push((rel, iso_at(t.date_of_day(day), tm)));
                }
            }
        }
    }
    let mut lines: Vec<StopLine> = map
        .into_values()
        .map(|mut l| {
            l.next.sort();
            l.next.dedup();
            l.next.truncate(4);
            l
        })
        .collect();
    lines.sort_by_key(|l| (l.next.first().map(|x| x.0).unwrap_or(i32::MAX), line_key(&l.short_name)));
    Some(StopBoard { stop, lines, generated_iso: Utc::now().with_timezone(&Jerusalem).format("%Y-%m-%dT%H:%M:%S%:z").to_string(), weekday: Utc::now().with_timezone(&Jerusalem).weekday().num_days_from_sunday() })
}
