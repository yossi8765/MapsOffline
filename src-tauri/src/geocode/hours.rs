//! OSM `opening_hours` of points of interest: "open now", today's closing time, next opening.
//! Evaluated in Asia/Jerusalem local time with the `opening-hours` crate (full OSM syntax).
//! A value it cannot read gives no answer (the raw text is still shown) – never a wrong one.
use chrono::{Datelike, NaiveDateTime, Timelike};
use opening_hours::{OpeningHours, RuleKind};

/// What the hours say at one moment.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HoursNow {
    /// None = unknown (unparseable, "unknown" rule, or a comment-only value).
    pub open_now: Option<bool>,
    /// While open: "HH:MM" of the next closing (None when open 24/7 or beyond a week).
    pub closes_at: Option<String>,
    /// While closed: next opening, "HH:MM" today or "<day> HH:MM" ("א׳ 08:00" = Sunday) later.
    pub opens_at: Option<String>,
}

/// Now in Israel (local wall-clock time).
pub fn now_il() -> NaiveDateTime {
    chrono::Utc::now().with_timezone(&chrono_tz::Asia::Jerusalem).naive_local()
}

const DAY_HE: [&str; 7] = ["א׳", "ב׳", "ג׳", "ד׳", "ה׳", "ו׳", "ש׳"];

fn fmt_at(t: NaiveDateTime, now: NaiveDateTime) -> String {
    let hm = format!("{:02}:{:02}", t.hour(), t.minute());
    if t.date() == now.date() {
        hm
    } else {
        // the week starts on Sunday (א׳)
        format!("{} {hm}", DAY_HE[t.weekday().num_days_from_sunday() as usize])
    }
}

const DAYS: [&str; 8] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su", "PH"];

/// "Mo-Th, Su 08:00-20:00": by the spec ", " starts an *additional rule* (Mo-Th all day!), but
/// mappers mean one list of days. Join a day list split by ", " back into "Mo-Th,Su".
fn join_day_lists(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(i) = rest.find(", ") {
        let (head, tail) = rest.split_at(i);
        out.push_str(head);
        let after = &tail[2..];
        let before_day = DAYS.iter().any(|d| head.ends_with(d));
        let after_day = DAYS.iter().any(|d| after.starts_with(d));
        out.push_str(if before_day && after_day { "," } else { ", " });
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Parsed values by raw text: "open now" over a category in a wide area evaluates thousands of
/// POIs, and most of them share a few dozen distinct values. Unreadable values are cached as None.
/// The map is emptied when it grows past [`CACHE_MAX`] (a cheap bound instead of a full LRU).
static PARSED: std::sync::OnceLock<parking_lot::Mutex<rustc_hash::FxHashMap<String, Option<std::sync::Arc<OpeningHours>>>>> = std::sync::OnceLock::new();
const CACHE_MAX: usize = 20_000;

fn parsed(raw: &str) -> Option<std::sync::Arc<OpeningHours>> {
    let m = PARSED.get_or_init(Default::default);
    if let Some(v) = m.lock().get(raw) {
        return v.clone();
    }
    let v = join_day_lists(raw).parse::<OpeningHours>().ok().map(std::sync::Arc::new);
    let mut g = m.lock();
    if g.len() >= CACHE_MAX {
        g.clear();
    }
    g.insert(raw.to_string(), v.clone());
    v
}

/// Evaluate `raw` at `now`. Changes more than 8 days away are not reported.
pub fn eval(raw: &str, now: NaiveDateTime) -> HoursNow {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 400 {
        return HoursNow::default();
    }
    let Some(oh) = parsed(raw) else { return HoursNow::default() };
    let (kind, _) = oh.state(now);
    let horizon = now + chrono::Duration::days(8);
    let next = oh.next_change(now).filter(|t| *t <= horizon);
    match kind {
        RuleKind::Open => HoursNow { open_now: Some(true), closes_at: next.map(|t| fmt_at(t, now)), opens_at: None },
        RuleKind::Closed => HoursNow { open_now: Some(false), closes_at: None, opens_at: next.map(|t| fmt_at(t, now)) },
        RuleKind::Unknown => HoursNow::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(h, min, 0).unwrap()
    }

    #[test]
    fn israeli_week() {
        // 2026-11-04 is a Wednesday (ד׳), 2026-11-06 a Friday, 2026-11-07 a Saturday
        let oh = "Su-Th 08:00-20:00; Fr 08:00-14:00";
        assert_eq!(eval(oh, at(2026, 11, 4, 10, 0)), HoursNow { open_now: Some(true), closes_at: Some("20:00".into()), opens_at: None });
        assert_eq!(eval(oh, at(2026, 11, 4, 21, 0)), HoursNow { open_now: Some(false), closes_at: None, opens_at: Some("ה׳ 08:00".into()) });
        assert_eq!(eval(oh, at(2026, 11, 6, 13, 0)).closes_at.as_deref(), Some("14:00"));
        // Saturday: closed until Sunday morning
        assert_eq!(eval(oh, at(2026, 11, 7, 12, 0)).opens_at.as_deref(), Some("א׳ 08:00"));
        assert_eq!(eval(oh, at(2026, 11, 4, 7, 0)).opens_at.as_deref(), Some("08:00"));
    }

    #[test]
    fn day_list_with_space() {
        assert_eq!(join_day_lists("Mo-Th, Su 08:00-20:00; Fr 08:00-13:00"), "Mo-Th,Su 08:00-20:00; Fr 08:00-13:00");
        assert_eq!(join_day_lists("Su-Th 08:00-13:00, 16:00-19:00"), "Su-Th 08:00-13:00, 16:00-19:00");
        // Wednesday 03:00: closed (it used to read "Mo-Th" as all day)
        let v = eval("Mo-Th, Su 08:00-20:00; Fr 08:00-13:00", at(2026, 11, 4, 3, 0));
        assert_eq!(v, HoursNow { open_now: Some(false), closes_at: None, opens_at: Some("08:00".into()) });
    }

    #[test]
    fn past_midnight_always_and_garbage() {
        let late = "Mo-Su 18:00-02:00";
        assert_eq!(eval(late, at(2026, 11, 4, 1, 0)).open_now, Some(true));
        assert_eq!(eval(late, at(2026, 11, 4, 1, 0)).closes_at.as_deref(), Some("02:00"));
        assert_eq!(eval("24/7", at(2026, 11, 4, 3, 0)), HoursNow { open_now: Some(true), closes_at: None, opens_at: None });
        // unreadable: no answer at all, never a guess
        assert_eq!(eval("בתיאום מראש", at(2026, 11, 4, 10, 0)), HoursNow::default());
        assert_eq!(eval("", at(2026, 11, 4, 10, 0)), HoursNow::default());
        assert_eq!(eval("Mo-Fr 09:00-17:00 off", at(2026, 11, 4, 10, 0)).open_now, Some(false));
    }
}
