//! Turn-by-turn instruction generation (Hebrew).
use crate::routing::astar::PathDetails;
use crate::routing::graph::{flags, Graph};
use crate::routing::profile::{turn_delta, Profile};
use crate::types::{Maneuver, Step};

/// "לרחוב הרצל" / "לכביש 1" / "" – preposition + street.
pub fn to_street(name: Option<&str>) -> String {
    match name {
        None => String::new(),
        Some(n) => {
            let n = n.trim();
            if n.is_empty() {
                return String::new();
            }
            let bare = ["כביש", "שדרות", "שד'", "שד׳", "דרך", "כיכר", "סמטת", "מחלף", "רח'", "רח׳", "רחוב", "מעלה", "נתיב", "שביל", "טיילת"];
            if bare.iter().any(|p| n.starts_with(p)) || n.chars().next().map(|c| c.is_ascii_alphanumeric()).unwrap_or(false) {
                format!(" ל{n}")
            } else {
                format!(" לרחוב {n}")
            }
        }
    }
}

/// "ברחוב הרצל" / "בכביש 1".
pub fn on_street(name: Option<&str>) -> String {
    match name {
        None => String::new(),
        Some(n) => {
            let n = n.trim();
            if n.is_empty() {
                return String::new();
            }
            let bare = ["כביש", "שדרות", "שד'", "שד׳", "דרך", "כיכר", "סמטת", "מחלף", "רח'", "רח׳", "רחוב", "מעלה", "נתיב", "שביל", "טיילת"];
            if bare.iter().any(|p| n.starts_with(p)) || n.chars().next().map(|c| c.is_ascii_alphanumeric()).unwrap_or(false) {
                format!(" ב{n}")
            } else {
                format!(" ברחוב {n}")
            }
        }
    }
}

fn ordinal_he(n: u32) -> String {
    match n {
        1 => "הראשונה".into(),
        2 => "השנייה".into(),
        3 => "השלישית".into(),
        4 => "הרביעית".into(),
        5 => "החמישית".into(),
        6 => "השישית".into(),
        _ => format!("ה-{n}"),
    }
}

pub fn instruction_text(m: Maneuver, street: Option<&str>, exit: Option<u32>) -> String {
    match m {
        Maneuver::Depart => format!("צא לדרך{}", on_street(street)),
        Maneuver::TurnLeft => format!("פנה שמאלה{}", to_street(street)),
        Maneuver::TurnRight => format!("פנה ימינה{}", to_street(street)),
        Maneuver::SlightLeft => format!("פנה קלות שמאלה{}", to_street(street)),
        Maneuver::SlightRight => format!("פנה קלות ימינה{}", to_street(street)),
        Maneuver::SharpLeft => format!("פנה בחדות שמאלה{}", to_street(street)),
        Maneuver::SharpRight => format!("פנה בחדות ימינה{}", to_street(street)),
        Maneuver::Straight => format!("המשך ישר{}", to_street(street)),
        Maneuver::Uturn => format!("בצע פניית פרסה{}", to_street(street)),
        Maneuver::Roundabout => match exit {
            Some(n) => format!("היכנס לכיכר וצא ביציאה {}{}", ordinal_he(n), to_street(street)),
            None => format!("היכנס לכיכר{}", to_street(street)),
        },
        Maneuver::ExitRoundabout => format!("צא מהכיכר{}", to_street(street)),
        Maneuver::Arrive => "הגעת ליעד".into(),
        Maneuver::Merge => format!("השתלב{}", on_street(street)),
        Maneuver::Fork => format!("המשך בהתפצלות{}", to_street(street)),
    }
}

/// Classify a turn angle (positive = right).
pub fn classify(delta: f32) -> Maneuver {
    let a = delta.abs();
    if a <= 25.0 {
        Maneuver::Straight
    } else if a >= 170.0 {
        Maneuver::Uturn
    } else if delta > 0.0 {
        if a <= 60.0 {
            Maneuver::SlightRight
        } else if a <= 135.0 {
            Maneuver::TurnRight
        } else {
            Maneuver::SharpRight
        }
    } else if a <= 60.0 {
        Maneuver::SlightLeft
    } else if a <= 135.0 {
        Maneuver::TurnLeft
    } else {
        Maneuver::SharpLeft
    }
}

struct Cur {
    maneuver: Maneuver,
    street: Option<String>,
    exit: Option<u32>,
    dist: f64,
    dur: f64,
    start: usize,
}

fn flush(steps: &mut Vec<Step>, c: Cur) {
    steps.push(Step {
        instruction: instruction_text(c.maneuver, c.street.as_deref(), c.exit),
        maneuver: c.maneuver,
        roundabout_exit: c.exit,
        street: c.street,
        distance_m: c.dist,
        duration_sec: c.dur,
        geometry_start: c.start,
    });
}

/// Count exits of a roundabout at vertex `v` (reached by state `s_in`): outgoing non-roundabout
/// edges usable by the profile.
fn exits_at(g: &Graph, prof: &Profile, v: u32, s_in: u32) -> u32 {
    let mut n = 0;
    for &e in g.incident(v) {
        if e == (s_in >> 1) || g.e_flags[e as usize] & flags::ROUNDABOUT != 0 {
            continue;
        }
        if prof.allowed(g, g.state_from(v, e)) {
            n += 1;
        }
    }
    n.min(1)
}

/// Build steps for a path. `min_step_m` suppresses micro-steps (e.g. 1 m stubs) by merging them.
pub fn build_steps(g: &Graph, prof: &Profile, states: &[u32], det: &PathDetails, min_step_m: usize) -> Vec<Step> {
    let mut steps: Vec<Step> = Vec::new();
    if states.is_empty() {
        return steps;
    }
    let name_of = |s: u32| -> Option<String> { g.name(s >> 1).map(|n| n.to_string()) };
    let same_name = |a: &Option<String>, b: &Option<String>| -> bool {
        match (a, b) {
            (Some(x), Some(y)) => x == y,
            (None, None) => true,
            _ => false,
        }
    };
    let mut cur = Cur { maneuver: Maneuver::Depart, street: name_of(states[0]), exit: None, dist: det.state_dist[0], dur: det.state_dur[0], start: 0 };
    let mut i = 1;
    let n = states.len();
    while i < n {
        let prev = states[i - 1];
        let s = states[i];
        let fp = g.e_flags[(prev >> 1) as usize];
        let fs = g.e_flags[(s >> 1) as usize];
        let v = g.head(prev);

        // ---- roundabout entry
        if fs & flags::ROUNDABOUT != 0 && fp & flags::ROUNDABOUT == 0 {
            flush(&mut steps, cur);
            let start = det.state_starts[i];
            let mut dist = 0.0;
            let mut dur = 0.0;
            let mut exits = 0u32;
            let mut j = i;
            while j < n && g.e_flags[(states[j] >> 1) as usize] & flags::ROUNDABOUT != 0 {
                dist += det.state_dist[j];
                dur += det.state_dur[j];
                let hv = g.head(states[j]);
                exits += exits_at(g, prof, hv, states[j]);
                if j + 1 < n && g.e_flags[(states[j + 1] >> 1) as usize] & flags::ROUNDABOUT == 0 {
                    break;
                }
                j += 1;
            }
            // j = last roundabout state; exit onto states[j+1]
            let exit_street = if j + 1 < n { name_of(states[j + 1]) } else { None };
            cur = Cur { maneuver: Maneuver::Roundabout, street: exit_street, exit: Some(exits.max(1)), dist, dur, start };
            i = j + 1;
            if i < n {
                // the stretch after the roundabout belongs to this step
                cur.dist += det.state_dist[i];
                cur.dur += det.state_dur[i];
                i += 1;
            }
            continue;
        }

        let name = name_of(s);
        let delta = turn_delta(g.bearing_in(prev), g.bearing_out(s));
        let mut m = classify(delta);
        let degree = g.degree(v);
        // entering / leaving links
        if (m == Maneuver::Straight || matches!(m, Maneuver::SlightLeft | Maneuver::SlightRight))
            && fp & flags::LINK != 0 && fs & flags::LINK == 0 && g.e_class[(s >> 1) as usize] <= 4 {
                m = Maneuver::Merge;
            }
        let slight = matches!(m, Maneuver::SlightLeft | Maneuver::SlightRight);
        let continue_same = (m == Maneuver::Straight && (same_name(&cur.street, &name) || name.is_none() || degree <= 2))
            || (slight && name.is_some() && same_name(&cur.street, &name));
        let tiny = det.state_dist[i] < min_step_m as f64 && i + 1 < n;
        if continue_same || tiny && m == Maneuver::Straight {
            cur.dist += det.state_dist[i];
            cur.dur += det.state_dur[i];
            if cur.street.is_none() {
                cur.street = name;
            }
        } else {
            flush(&mut steps, cur);
            cur = Cur { maneuver: m, street: name, exit: None, dist: det.state_dist[i], dur: det.state_dur[i], start: det.state_starts[i] };
        }
        i += 1;
    }
    flush(&mut steps, cur);
    // merge unnamed micro-steps (crossings, stubs) into the previous step
    let mut merged: Vec<Step> = Vec::with_capacity(steps.len());
    for st in steps.into_iter() {
        let micro = st.street.is_none() && st.distance_m < (min_step_m as f64) * 4.0 && st.maneuver != Maneuver::Roundabout;
        match merged.last_mut() {
            Some(prev) if micro => {
                prev.distance_m += st.distance_m;
                prev.duration_sec += st.duration_sec;
            }
            _ => merged.push(st),
        }
    }
    let mut steps = merged;
    steps.push(Step {
        instruction: instruction_text(Maneuver::Arrive, None, None),
        maneuver: Maneuver::Arrive,
        roundabout_exit: None,
        street: None,
        distance_m: 0.0,
        duration_sec: 0.0,
        geometry_start: det.geometry.len().saturating_sub(1),
    });
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts() {
        assert_eq!(instruction_text(Maneuver::TurnRight, Some("הרצל"), None), "פנה ימינה לרחוב הרצל");
        assert_eq!(instruction_text(Maneuver::TurnLeft, Some("כביש 1"), None), "פנה שמאלה לכביש 1");
        assert_eq!(instruction_text(Maneuver::Roundabout, Some("ז'בוטינסקי"), Some(2)), "היכנס לכיכר וצא ביציאה השנייה לרחוב ז'בוטינסקי");
        assert_eq!(instruction_text(Maneuver::Depart, None, None), "צא לדרך");
        assert_eq!(instruction_text(Maneuver::Arrive, None, None), "הגעת ליעד");
    }

    #[test]
    fn classes() {
        assert_eq!(classify(0.0), Maneuver::Straight);
        assert_eq!(classify(40.0), Maneuver::SlightRight);
        assert_eq!(classify(-90.0), Maneuver::TurnLeft);
        assert_eq!(classify(150.0), Maneuver::SharpRight);
        assert_eq!(classify(178.0), Maneuver::Uturn);
    }
}
