//! Turn-by-turn instruction generation (Hebrew).
use crate::routing::astar::PathDetails;
use crate::routing::graph::{flags, Class, Graph};
use crate::routing::profile::{turn_delta, Profile};
use crate::types::{Lane, Maneuver, Step};

const BARE: [&str; 15] = ["כביש", "שדרות", "שד'", "שד׳", "דרך", "כיכר", "סמטת", "מחלף", "רח'", "רח׳", "רחוב", "מעלה", "נתיב", "שביל", "טיילת"];

fn bare(n: &str) -> bool {
    BARE.iter().any(|p| n.starts_with(p)) || n.chars().next().map(|c| c.is_ascii_alphanumeric()).unwrap_or(false)
}

/// "לרחוב הרצל" / "לכביש 1" / "" – preposition + street.
pub fn to_street(name: Option<&str>) -> String {
    match name.map(str::trim).filter(|n| !n.is_empty()) {
        None => String::new(),
        Some(n) if bare(n) => format!(" ל{n}"),
        Some(n) => format!(" לרחוב {n}"),
    }
}

/// "ברחוב הרצל" / "בכביש 1".
pub fn on_street(name: Option<&str>) -> String {
    match name.map(str::trim).filter(|n| !n.is_empty()) {
        None => String::new(),
        Some(n) if bare(n) => format!(" ב{n}"),
        Some(n) => format!(" ברחוב {n}"),
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
        Maneuver::Straight => {
            // "המשך בכביש 1" reads better than "המשך ישר לכביש 1" on a numbered road
            if street.is_some_and(|s| s.trim_start().starts_with("כביש")) {
                format!("המשך{}", on_street(street))
            } else {
                format!("המשך ישר{}", to_street(street))
            }
        }
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

/// Motorway exits: "צא ביציאה 5 לכיוון ירושלים", "צא לכיוון חיפה", "צא ביציאה 12 לכביש 4".
pub fn exit_text(exit: Option<&str>, destination: Option<&str>, street: Option<&str>) -> Option<String> {
    match (exit.filter(|e| !e.is_empty()), destination.filter(|d| !d.is_empty())) {
        (Some(e), Some(d)) => Some(format!("צא ביציאה {e} לכיוון {d}")),
        (None, Some(d)) => Some(format!("צא לכיוון {d}")),
        (Some(e), None) => Some(format!("צא ביציאה {e}{}", to_street(street))),
        (None, None) => None,
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

/// Lanes of a raw `turn:lanes` value with the ones leading to maneuver `m` marked active.
pub fn lanes_for(raw: &str, m: Maneuver) -> Vec<Lane> {
    let mut lanes: Vec<Lane> = raw
        .split('|')
        .map(|l| {
            let d: Vec<String> = l.split(';').map(str::trim).filter(|x| !x.is_empty()).map(str::to_string).collect();
            Lane { directions: if d.is_empty() { vec!["none".into()] } else { d }, active: false }
        })
        .collect();
    let groups: &[&[&str]] = match m {
        Maneuver::TurnLeft => &[&["left"], &["sharp_left", "slight_left"]],
        Maneuver::SharpLeft => &[&["sharp_left"], &["left"]],
        Maneuver::SlightLeft => &[&["slight_left"], &["left", "merge_to_left"]],
        Maneuver::TurnRight => &[&["right"], &["sharp_right", "slight_right"]],
        Maneuver::SharpRight => &[&["sharp_right"], &["right"]],
        Maneuver::SlightRight => &[&["slight_right"], &["right", "merge_to_right"]],
        Maneuver::Uturn => &[&["reverse"], &["left"]],
        _ => &[&["through"], &["none"]],
    };
    for want in groups {
        let mut hit = false;
        for l in &mut lanes {
            if l.directions.iter().any(|d| want.contains(&d.as_str())) {
                l.active = true;
                hit = true;
            }
        }
        if hit {
            break;
        }
    }
    lanes
}

/// What a step shows for its road: motorways / trunk roads with a number are named by it
/// ("כביש 1 (כביש ירושלים)"), everything else by its name.
pub fn street_label(g: &Graph, e: u32) -> Option<String> {
    let name = g.name(e).map(str::to_string);
    let r = g.road_ref(e).filter(|r| r.chars().all(|c| c.is_ascii_digit() || c == ';'));
    let class = g.class(e);
    match (r, matches!(class, Class::Motorway | Class::Trunk)) {
        (Some(r), true) => {
            let r = r.split(';').next().unwrap_or(r);
            let num = format!("כביש {r}");
            match name {
                Some(n) if n != num && !n.contains(r) => Some(format!("{num} ({n})")),
                _ => Some(num),
            }
        }
        _ => name,
    }
}

struct Cur {
    maneuver: Maneuver,
    street: Option<String>,
    exit: Option<u32>,
    dist: f64,
    dur: f64,
    start: usize,
    /// first state of the step (ref / limit)
    state: u32,
    /// state arriving at the maneuver (lanes)
    approach: Option<u32>,
    junction: Option<String>,
    destination: Option<String>,
}

fn flush(g: &Graph, steps: &mut Vec<Step>, c: Cur) {
    let instruction = exit_text(c.junction.as_deref(), c.destination.as_deref(), c.street.as_deref())
        .unwrap_or_else(|| instruction_text(c.maneuver, c.street.as_deref(), c.exit));
    let e = c.state >> 1;
    let lanes = match (c.approach, c.maneuver) {
        (Some(a), m) if !matches!(m, Maneuver::Depart | Maneuver::Arrive) => g.lanes_s(a).map(|raw| lanes_for(raw, m)).unwrap_or_default(),
        _ => Vec::new(),
    };
    let mut st = Step::basic(instruction, c.maneuver, c.exit, c.street, c.dist, c.dur, c.start);
    st.road_ref = g.road_ref(e).map(str::to_string);
    st.exit = c.junction;
    st.destination = c.destination;
    st.speed_limit_kmh = g.limit_s(c.state).map(u32::from);
    st.lanes = lanes;
    steps.push(st);
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
    let pedestrian = prof.mode.is_pedestrian();
    let name_of = |s: u32| -> Option<String> { street_label(g, s >> 1) };
    let same_name = |a: &Option<String>, b: &Option<String>| -> bool {
        match (a, b) {
            (Some(x), Some(y)) => x == y,
            (None, None) => true,
            _ => false,
        }
    };
    let new_cur = |m: Maneuver, street: Option<String>, i: usize, approach: Option<u32>| Cur {
        maneuver: m,
        street,
        exit: None,
        dist: det.state_dist[i],
        dur: det.state_dur[i],
        start: det.state_starts[i],
        state: states[i],
        approach,
        junction: None,
        destination: None,
    };
    let mut cur = new_cur(Maneuver::Depart, name_of(states[0]), 0, None);
    let mut i = 1;
    let n = states.len();
    while i < n {
        let prev = states[i - 1];
        let s = states[i];
        let fp = g.e_flags[(prev >> 1) as usize];
        let fs = g.e_flags[(s >> 1) as usize];
        let v = g.head(prev);

        // ---- roundabout entry: one step from the entry to the next maneuver after the exit
        if fs & flags::ROUNDABOUT != 0 && fp & flags::ROUNDABOUT == 0 {
            flush(g, &mut steps, cur);
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
            let first_after = if j + 1 < n { states[j + 1] } else { states[j.min(n - 1)] };
            cur = Cur {
                maneuver: Maneuver::Roundabout,
                street: exit_street,
                exit: Some(exits.max(1)),
                dist,
                dur,
                start,
                state: first_after,
                approach: Some(prev),
                junction: None,
                destination: None,
            };
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
        let cls_s = g.e_class[(s >> 1) as usize];
        // entering / leaving links
        if (m == Maneuver::Straight || matches!(m, Maneuver::SlightLeft | Maneuver::SlightRight)) && fp & flags::LINK != 0 && fs & flags::LINK == 0 && cls_s <= 4 {
            m = Maneuver::Merge;
        }
        let entering_link = fs & flags::LINK != 0 && fp & flags::LINK == 0;
        let slight = matches!(m, Maneuver::SlightLeft | Maneuver::SlightRight);
        let ref_prev = g.road_ref(prev >> 1);
        let ref_cur = g.road_ref(s >> 1);
        let same_ref = ref_prev.is_some() && ref_prev == ref_cur;
        let both_motorway = fp & flags::MOTORWAY != 0 && fs & flags::MOTORWAY != 0 && fp & flags::LINK == 0 && fs & flags::LINK == 0;
        let continue_same = !entering_link
            && ((m == Maneuver::Straight && (same_name(&cur.street, &name) || name.is_none() || degree <= 2 || same_ref || both_motorway))
                || (slight && name.is_some() && same_name(&cur.street, &name))
                // the same numbered road under another name (כביש 1 "תל אביב ירושלים" / "ירושלים תל אביב")
                || ((m == Maneuver::Straight || slight) && (same_ref || (both_motorway && degree <= 2)))
                // unnamed paths: a gentle bend is not a turn
                || (slight && name.is_none() && cur.street.is_none() && (degree <= 2 || (pedestrian && delta.abs() < 45.0))));
        let tiny = det.state_dist[i] < min_step_m as f64 && i + 1 < n;
        if continue_same || tiny && m == Maneuver::Straight {
            cur.dist += det.state_dist[i];
            cur.dur += det.state_dur[i];
            if cur.street.is_none() {
                cur.street = name;
            }
        } else {
            flush(g, &mut steps, cur);
            cur = new_cur(m, name, i, Some(prev));
            if entering_link {
                cur.junction = g.junction_ref(s >> 1).map(str::to_string);
                // the signposted destination of the link, or of the next link piece
                cur.destination = g.destination_s(s).or_else(|| states.get(i + 1).and_then(|&n2| g.destination_s(n2))).map(str::to_string);
                if cur.street.is_none() {
                    // a link has no name: name it after the road it leads to
                    cur.street = states[i..].iter().find(|&&x| g.e_flags[(x >> 1) as usize] & flags::LINK == 0).and_then(|&x| name_of(x));
                }
            }
        }
        i += 1;
    }
    flush(g, &mut steps, cur);
    // merge unnamed micro-steps (crossings, stubs) into the previous step
    let mut merged: Vec<Step> = Vec::with_capacity(steps.len());
    for st in steps.into_iter() {
        let micro = st.street.is_none() && st.distance_m < (min_step_m as f64) * 4.0 && st.maneuver != Maneuver::Roundabout && st.exit.is_none();
        match merged.last_mut() {
            Some(prev) if micro => {
                prev.distance_m += st.distance_m;
                prev.duration_sec += st.duration_sec;
            }
            _ => merged.push(st),
        }
    }
    let mut steps = merged;
    steps.push(Step::basic(instruction_text(Maneuver::Arrive, None, None), Maneuver::Arrive, None, None, 0.0, 0.0, det.geometry.len().saturating_sub(1)));
    steps
}

/// Merge a roundabout step with the "exit roundabout" step that follows it (online answers
/// split them): one step from the entry to the next maneuver.
pub fn merge_roundabout_exits(steps: Vec<Step>) -> Vec<Step> {
    let mut out: Vec<Step> = Vec::with_capacity(steps.len());
    for st in steps {
        if st.maneuver == Maneuver::ExitRoundabout {
            if let Some(prev) = out.last_mut().filter(|p| p.maneuver == Maneuver::Roundabout) {
                prev.distance_m += st.distance_m;
                prev.duration_sec += st.duration_sec;
                if st.street.is_some() {
                    prev.street = st.street.clone();
                }
                prev.instruction = instruction_text(Maneuver::Roundabout, prev.street.as_deref(), prev.roundabout_exit);
                continue;
            }
        }
        out.push(st);
    }
    out
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
        assert_eq!(instruction_text(Maneuver::Straight, Some("כביש 1"), None), "המשך בכביש 1");
        assert_eq!(instruction_text(Maneuver::Straight, Some("הרצל"), None), "המשך ישר לרחוב הרצל");
        assert_eq!(exit_text(Some("5"), Some("ירושלים"), None).as_deref(), Some("צא ביציאה 5 לכיוון ירושלים"));
        assert_eq!(exit_text(None, Some("חיפה"), None).as_deref(), Some("צא לכיוון חיפה"));
        assert_eq!(exit_text(Some("12"), None, Some("כביש 4")).as_deref(), Some("צא ביציאה 12 לכביש 4"));
        assert_eq!(exit_text(None, None, Some("x")), None);
    }

    #[test]
    fn classes() {
        assert_eq!(classify(0.0), Maneuver::Straight);
        assert_eq!(classify(40.0), Maneuver::SlightRight);
        assert_eq!(classify(-90.0), Maneuver::TurnLeft);
        assert_eq!(classify(150.0), Maneuver::SharpRight);
        assert_eq!(classify(178.0), Maneuver::Uturn);
    }

    #[test]
    fn lanes() {
        let l = lanes_for("left|through|through;right|right", Maneuver::TurnRight);
        assert_eq!(l.iter().map(|x| x.active).collect::<Vec<_>>(), vec![false, false, true, true]);
        let l = lanes_for("left|through|through;right|right", Maneuver::Straight);
        assert_eq!(l.iter().map(|x| x.active).collect::<Vec<_>>(), vec![false, true, true, false]);
        let l = lanes_for("through|through|slight_right", Maneuver::SlightRight);
        assert_eq!(l.iter().map(|x| x.active).collect::<Vec<_>>(), vec![false, false, true]);
        let l = lanes_for("|right", Maneuver::Straight);
        assert_eq!(l[0].directions, vec!["none"]);
        assert!(l[0].active && !l[1].active);
    }

    #[test]
    fn roundabout_exit_merged() {
        let s = |m: Maneuver, street: Option<&str>, d: f64, exit: Option<u32>| Step::basic(String::new(), m, exit, street.map(String::from), d, d / 10.0, 0);
        let steps = vec![
            s(Maneuver::Depart, Some("הרצל"), 100.0, None),
            s(Maneuver::Roundabout, None, 60.0, Some(2)),
            s(Maneuver::ExitRoundabout, Some("כביש 45"), 652.0, None),
            s(Maneuver::TurnRight, Some("כביש 45"), 3300.0, None),
        ];
        let m = merge_roundabout_exits(steps);
        assert_eq!(m.len(), 3);
        assert_eq!(m[1].maneuver, Maneuver::Roundabout);
        assert!((m[1].distance_m - 712.0).abs() < 1e-9);
        assert_eq!(m[1].instruction, "היכנס לכיכר וצא ביציאה השנייה לכביש 45");
    }
}
