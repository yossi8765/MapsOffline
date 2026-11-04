//! Israeli public-transport rest days: Shabbat and the Jewish holidays (Yom Tov) on which
//! buses and trains do not run, plus Yom HaAtzmaut (reduced service), from the Hebrew calendar.
//!
//! Rosh Hashana is computed with the classic molad + postponement rules (Dershowitz & Reingold,
//! "Calendrical Calculations", `HebrewCalendarElapsedDays`); every other holiday is a fixed
//! offset from a Rosh Hashana: the months from Nisan to Elul have fixed lengths, so Nisan 15 is
//! always 163 days before the next Rosh Hashana.

use chrono::{Datelike, Duration, NaiveDate, Weekday};

/// R.D. (days from 0001-01-01 = 1, chrono's `num_days_from_ce`) of the Hebrew epoch − 1.
const HEBREW_EPOCH: i64 = -1_373_429;

fn leap(year: i64) -> bool {
    (7 * year + 1).rem_euclid(19) < 7
}

fn elapsed_days(year: i64) -> i64 {
    let y = year - 1;
    let months = 235 * y.div_euclid(19) + 12 * y.rem_euclid(19) + (7 * y.rem_euclid(19) + 1) / 19;
    let parts = 204 + 793 * (months % 1080);
    let hours = 5 + 12 * months + 793 * (months / 1080) + parts / 1080;
    let day = 1 + 29 * months + hours / 24;
    let cparts = 1080 * (hours % 24) + parts % 1080;
    let alt = if cparts >= 19_440 || (day % 7 == 2 && cparts >= 9_924 && !leap(year)) || (day % 7 == 1 && cparts >= 16_789 && leap(year - 1)) {
        day + 1
    } else {
        day
    };
    if matches!(alt % 7, 0 | 3 | 5) {
        alt + 1
    } else {
        alt
    }
}

/// Gregorian date of 1 Tishrei (Rosh Hashana) of Hebrew year `year`.
pub fn rosh_hashana(year: i64) -> NaiveDate {
    let rd = 1 + elapsed_days(year) + HEBREW_EPOCH;
    NaiveDate::from_num_days_from_ce_opt(rd as i32).expect("date in range")
}

/// The holiday rest days of the Hebrew year that starts in the autumn of `year - 3761`, with
/// their Hebrew names: (date, name, full rest day = no service).
fn holidays_of(hyear: i64) -> Vec<(NaiveDate, &'static str, bool)> {
    let rh = rosh_hashana(hyear);
    let next_rh = rosh_hashana(hyear + 1);
    let d = |base: NaiveDate, days: i64| base + Duration::days(days);
    let pesach = d(next_rh, -163); // 15 Nisan
    let iyar5 = d(pesach, 20);
    // Yom HaAtzmaut: 5 Iyar, moved to Thursday when it falls on Friday or Shabbat and to
    // Tuesday when it falls on Monday (so that Yom HaZikaron does not follow Shabbat).
    let atzmaut = match iyar5.weekday() {
        Weekday::Fri => d(iyar5, -1),
        Weekday::Sat => d(iyar5, -2),
        Weekday::Mon => d(iyar5, 1),
        _ => iyar5,
    };
    vec![
        (rh, "ראש השנה", true),
        (d(rh, 1), "ראש השנה", true),
        (d(rh, 9), "יום כיפור", true),
        (d(rh, 14), "סוכות", true),
        (d(rh, 21), "שמחת תורה", true),
        (pesach, "פסח", true),
        (d(pesach, 6), "שביעי של פסח", true),
        (atzmaut, "יום העצמאות", false),
        (d(pesach, 50), "שבועות", true), // 6 Sivan
    ]
}

/// Holiday on `date`: (Hebrew name, full rest day). Yom HaAtzmaut is a holiday with reduced
/// (not absent) service.
pub fn holiday_on(date: NaiveDate) -> Option<(&'static str, bool)> {
    let hyear = date.year() as i64 + 3761;
    for y in [hyear - 1, hyear] {
        if let Some(&(_, name, rest)) = holidays_of(y).iter().find(|h| h.0 == date) {
            return Some((name, rest));
        }
    }
    None
}

/// Why service is missing around a local date and time: Shabbat or a rest-day holiday,
/// including its eve from 14:00 (the last buses leave in the afternoon).
pub enum RestDay {
    Shabbat,
    Holiday(&'static str),
}

pub fn rest_day(date: NaiveDate, hour: u32) -> Option<RestDay> {
    let rest = |d: NaiveDate| -> Option<RestDay> {
        match holiday_on(d) {
            Some((name, true)) => Some(RestDay::Holiday(name)),
            _ if d.weekday() == Weekday::Sat => Some(RestDay::Shabbat),
            _ => None,
        }
    };
    if let Some(r) = rest(date) {
        return Some(r);
    }
    if hour >= 14 {
        return rest(date + Duration::days(1));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn rosh_hashana_dates() {
        // published calendars
        assert_eq!(rosh_hashana(5785), ymd(2024, 10, 3));
        assert_eq!(rosh_hashana(5786), ymd(2025, 9, 23));
        assert_eq!(rosh_hashana(5787), ymd(2026, 9, 12));
        assert_eq!(rosh_hashana(5788), ymd(2027, 10, 2));
        assert_eq!(rosh_hashana(5790), ymd(2029, 9, 10));
        assert_eq!(rosh_hashana(5795), ymd(2034, 9, 14));
    }

    #[test]
    fn holidays_2026() {
        assert_eq!(holiday_on(ymd(2026, 4, 2)), Some(("פסח", true)));
        assert_eq!(holiday_on(ymd(2026, 4, 8)), Some(("שביעי של פסח", true)));
        assert_eq!(holiday_on(ymd(2026, 5, 22)), Some(("שבועות", true)));
        assert_eq!(holiday_on(ymd(2026, 9, 21)), Some(("יום כיפור", true)));
        assert_eq!(holiday_on(ymd(2026, 9, 26)), Some(("סוכות", true)));
        assert_eq!(holiday_on(ymd(2026, 10, 3)), Some(("שמחת תורה", true)));
        assert_eq!(holiday_on(ymd(2026, 4, 22)), Some(("יום העצמאות", false)));
        assert_eq!(holiday_on(ymd(2026, 11, 4)), None);
        // 2025: Pesach 13 April, Shavuot 2 June, Yom Kippur 2 October
        assert_eq!(holiday_on(ymd(2025, 4, 13)), Some(("פסח", true)));
        assert_eq!(holiday_on(ymd(2025, 6, 2)), Some(("שבועות", true)));
        assert_eq!(holiday_on(ymd(2025, 10, 2)), Some(("יום כיפור", true)));
    }

    #[test]
    fn yom_haatzmaut_moves() {
        // 2024: 5 Iyar was a Monday (13 May) → celebrated Tuesday 14 May
        assert_eq!(holiday_on(ymd(2024, 5, 14)), Some(("יום העצמאות", false)));
        // 2025: 5 Iyar was Friday 2 May → Thursday 1 May
        assert_eq!(holiday_on(ymd(2025, 5, 1)), Some(("יום העצמאות", false)));
    }

    #[test]
    fn rest_days() {
        assert!(matches!(rest_day(ymd(2026, 11, 7), 10), Some(RestDay::Shabbat)));
        assert!(matches!(rest_day(ymd(2026, 11, 6), 16), Some(RestDay::Shabbat)));
        assert!(rest_day(ymd(2026, 11, 6), 9).is_none());
        assert!(matches!(rest_day(ymd(2026, 9, 20), 18), Some(RestDay::Holiday("יום כיפור"))));
        assert!(rest_day(ymd(2026, 11, 4), 18).is_none());
    }
}
