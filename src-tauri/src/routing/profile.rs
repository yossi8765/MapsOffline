//! Cost profiles for car / truck / foot / wheelchair / bike: access, traversal cost, turn cost,
//! heuristic bound. Costs are per directed state (`edge * 2 + dir`) because speeds, limits and
//! climbing differ by direction.
use std::sync::Arc;

use crate::routing::graph::{flags, rmask, Class, Graph};
use crate::types::{Avoid, Mode, VehicleDims};

const KMH: f32 = 1.0 / 3.6;
pub const FOOT_SPEED: f32 = 5.0 * KMH; // m/s
pub const WHEELCHAIR_SPEED: f32 = 4.0 * KMH;
pub const BIKE_SPEED: f32 = 15.0 * KMH;
pub const CAR_MAX_SPEED: f32 = 130.0 * KMH;
/// Trucks: motorway / trunk cap and the cap elsewhere (Israeli HGV limits).
pub const TRUCK_MAX_KMH: f32 = 90.0;
pub const TRUCK_OTHER_KMH: f32 = 80.0;
/// An unpaved edge (segment between junctions) longer than this counts as a "long" dirt road.
pub const UNPAVED_LONG_M: f32 = 300.0;
/// `access=destination` roads are free within this distance (m) of a waypoint.
pub const DESTINATION_FREE_M: f32 = 500.0;

/// Extra seconds for an unsignalised left turn / U-turn when avoiding difficult junctions,
/// by the most important road involved (crossed or entered).
/// Search penalty (seconds) for passing a junction with traffic lights when the route should have
/// as few as possible: about half a red light – enough to take a parallel street, not a long detour.
pub fn traffic_light_penalty(mode: Mode) -> f32 {
    match mode {
        Mode::Car | Mode::Truck => 30.0,
        Mode::Bike => 25.0,
        _ => 20.0,
    }
}

pub fn hard_turn_penalty(class: Class) -> f32 {
    match class {
        Class::Motorway | Class::Trunk | Class::Primary => 45.0,
        Class::Secondary => 30.0,
        Class::Tertiary => 15.0,
        _ => 5.0,
    }
}

/// Preference factors per road class (index = `Class as u8`). Foot: `foot` without a sidewalk,
/// `foot_sw` with one; footways have their own factor.
#[derive(Clone, Copy, Debug)]
pub struct ClassParams {
    pub car: f32,
    pub truck: f32,
    pub bike: f32,
    pub foot: f32,
    pub foot_sw: f32,
}

const fn cp(car: f32, truck: f32, bike: f32, foot: f32, foot_sw: f32) -> ClassParams {
    ClassParams { car, truck, bike, foot, foot_sw }
}

pub const CLASS_TABLE: [ClassParams; 17] = [
    cp(1.0, 1.0, 3.0, 3.0, 3.0),   // Motorway
    cp(1.0, 1.0, 1.8, 1.4, 1.05),  // Trunk
    cp(1.0, 1.0, 1.4, 1.15, 1.0),  // Primary
    cp(1.0, 1.1, 1.2, 1.15, 1.0),  // Secondary
    cp(1.0, 1.3, 1.05, 1.05, 1.05), // Tertiary
    cp(1.0, 1.6, 1.0, 1.0, 1.0),   // Unclassified
    cp(1.0, 2.0, 1.0, 1.0, 1.0),   // Residential
    cp(1.4, 2.5, 1.0, 1.0, 1.0),   // Service
    cp(1.3, 4.0, 1.0, 1.0, 1.0),   // LivingStreet
    cp(2.5, 6.0, 1.0, 1.1, 1.1),   // Track
    cp(1.0, 1.0, 1.0, 1.0, 1.0),   // Pedestrian
    cp(1.0, 1.0, 1.0, 1.0, 1.0),   // Footway
    cp(1.0, 1.0, 1.0, 1.0, 1.0),   // Path
    cp(1.0, 1.0, 1.0, 1.0, 1.0),   // Cycleway
    cp(1.0, 1.0, 1.0, 1.0, 1.0),   // Steps
    cp(1.5, 2.0, 1.0, 1.0, 1.0),   // Road
    cp(1.0, 1.0, 1.0, 1.0, 1.0),   // Other (ferries)
];

#[derive(Clone, Debug)]
pub struct Profile {
    pub mode: Mode,
    pub avoid: Avoid,
    /// edge flags that make an edge impassable (restricted territories), from `avoid`
    pub forbid: u32,
    /// per-request forbidden edges (bitset over edges): avoided roads / areas, truck limits
    pub forbid_edges: Option<Arc<Vec<u64>>>,
    /// projected waypoints: `access=destination` roads near them cost nothing extra
    pub free_points: Vec<[f32; 2]>,
    pub vehicle: Option<VehicleDims>,
}

impl Profile {
    pub fn new(mode: Mode, avoid: Avoid) -> Self {
        // transit uses foot for access/egress
        let mode = if mode == Mode::Transit { Mode::Foot } else { mode };
        let forbid = (if avoid.area_a { flags::AREA_A } else { 0 }) | (if avoid.area_b { flags::AREA_B } else { 0 });
        Self { mode, avoid, forbid, forbid_edges: None, free_points: Vec::new(), vehicle: None }
    }

    /// Same profile without the territory restriction (used to tell "no road" from "road is
    /// in an avoided territory").
    pub fn without_areas(&self) -> Self {
        Self { forbid: 0, ..self.clone() }
    }

    pub fn avoids_areas(&self) -> bool {
        self.forbid != 0
    }

    #[inline]
    fn vehicle_mask(&self) -> u8 {
        match self.mode {
            Mode::Car => rmask::CAR,
            Mode::Truck => rmask::TRUCK,
            Mode::Bike => rmask::BIKE,
            _ => 0,
        }
    }

    /// Lower bound of cost per meter (seconds/m) used by the A* heuristic. For cars it uses the
    /// highest speed actually present in the graph (tighter than a fixed 130 km/h bound).
    pub fn heuristic_spm(&self, g: &Graph) -> f32 {
        let vmax = if g.max_car_kmh >= 5 { (g.max_car_kmh as f32 * KMH).min(CAR_MAX_SPEED) } else { CAR_MAX_SPEED };
        let v = match self.mode {
            Mode::Car => 1.0 / vmax,
            Mode::Truck => 1.0 / vmax.min(TRUCK_MAX_KMH * KMH),
            Mode::Foot | Mode::Transit => 0.9 / FOOT_SPEED,
            Mode::Wheelchair => 0.8 / WHEELCHAIR_SPEED,
            Mode::Bike => 0.8 / BIKE_SPEED,
        };
        v * 0.97 * g.proj0.hscale as f32 // projection error margin; east-west shrink away from the centre latitude
    }

    #[inline]
    fn edge_forbidden(&self, e: u32) -> bool {
        match &self.forbid_edges {
            Some(b) => b.get((e >> 6) as usize).is_some_and(|w| w & (1u64 << (e & 63)) != 0),
            None => false,
        }
    }

    /// May the directed state `s` be traversed?
    #[inline]
    pub fn allowed(&self, g: &Graph, s: u32) -> bool {
        let e = s >> 1;
        let f = g.e_flags[e as usize];
        if f & self.forbid != 0 || self.edge_forbidden(e) {
            return false;
        }
        let fwd = s & 1 == 0;
        match self.mode {
            Mode::Car => f & (if fwd { flags::CAR_FWD } else { flags::CAR_BWD }) != 0,
            Mode::Truck => f & (if fwd { flags::CAR_FWD } else { flags::CAR_BWD }) != 0 && f & flags::HGV_NO == 0,
            Mode::Bike => f & (if fwd { flags::BIKE_FWD } else { flags::BIKE_BWD }) != 0,
            Mode::Wheelchair => f & flags::FOOT != 0 && f & flags::WHEELCHAIR_NO == 0 && (f & flags::STEPS == 0 || f & flags::WHEELCHAIR_YES != 0),
            Mode::Foot | Mode::Transit => f & flags::FOOT != 0,
        }
    }

    /// May edge `e` be used in at least one direction?
    #[inline]
    pub fn allowed_edge_any(&self, g: &Graph, e: u32) -> bool {
        self.allowed(g, e * 2) || self.allowed(g, e * 2 + 1)
    }

    /// Real traversal time in seconds of directed state `s` (no preferences).
    #[inline]
    pub fn traverse_time(&self, g: &Graph, s: u32) -> f32 {
        let ei = (s >> 1) as usize;
        let len = g.e_len[ei];
        let f = g.e_flags[ei];
        let class = Class::from_u8(g.e_class[ei]);
        if f & flags::FERRY != 0 {
            return len / (g.speed_s(s).max(3) as f32 * KMH);
        }
        let (up, down) = g.climb_s(s);
        match self.mode {
            Mode::Car | Mode::Truck => {
                let mut v = (g.speed_s(s).max(5) as f32) * KMH;
                // tracks / unpaved are slower than their nominal speed
                if f & flags::UNPAVED != 0 {
                    v = v.min(25.0 * KMH);
                }
                if self.mode == Mode::Truck {
                    let cap = if matches!(class, Class::Motorway | Class::Trunk) { TRUCK_MAX_KMH } else { TRUCK_OTHER_KMH };
                    v = v.min(cap * KMH);
                }
                len / v
            }
            Mode::Bike => {
                let v = if f & flags::STEPS != 0 {
                    2.0 * KMH
                } else if matches!(class, Class::Footway | Class::Pedestrian | Class::Path) {
                    8.0 * KMH
                } else if f & flags::UNPAVED != 0 {
                    10.0 * KMH
                } else {
                    BIKE_SPEED
                };
                // ~4 s per metre climbed; braking on steep descents
                let steep_down = if down > 0.08 * len { down * 1.0 } else { 0.0 };
                len / v + up * 4.0 + steep_down
            }
            Mode::Wheelchair => len / WHEELCHAIR_SPEED + up * 10.0 + down * 2.0,
            Mode::Foot | Mode::Transit => {
                let v = if f & flags::STEPS != 0 { 2.0 * KMH } else { FOOT_SPEED };
                // Naismith: ~1 h per 600 m of ascent; steep descents slow down too
                let steep_down = if down > 0.1 * len { down * 2.0 } else { 0.0 };
                len / v + up * 6.0 + steep_down
            }
        }
    }

    /// Search cost (seconds × preference factors) of traversing directed state `s`.
    #[inline]
    pub fn cost(&self, g: &Graph, s: u32) -> f32 {
        let t = self.traverse_time(g, s);
        let ei = (s >> 1) as usize;
        let f = g.e_flags[ei];
        let class = g.e_class[ei] as usize;
        let row = CLASS_TABLE[class.min(16)];
        // "avoid unpaved" either applies to every unpaved edge or only to long stretches
        let avoid_unpaved = self.avoid.unpaved && (!self.avoid.unpaved_long_only || g.e_len[ei] > UNPAVED_LONG_M);
        let mut k = 1.0f32;
        if f & flags::FERRY != 0 && self.avoid.ferry {
            k *= 10.0;
        }
        match self.mode {
            Mode::Car | Mode::Truck => {
                k *= if self.mode == Mode::Truck { row.truck } else { row.car };
                if f & flags::UNPAVED != 0 {
                    k *= if avoid_unpaved { 5.0 } else { 1.5 };
                    if self.mode == Mode::Truck {
                        k *= 2.0;
                    }
                }
                if f & flags::TOLL != 0 && self.avoid.toll {
                    k *= 5.0;
                }
                if f & flags::MOTORWAY != 0 && self.avoid.motorway {
                    k *= 3.0;
                }
                if f & flags::DESTINATION != 0 && !self.free_points.is_empty() {
                    let v = g.vproj(g.e_from[ei]);
                    let r2 = DESTINATION_FREE_M * DESTINATION_FREE_M;
                    if !self.free_points.iter().any(|p| (p[0] - v[0]) * (p[0] - v[0]) + (p[1] - v[1]) * (p[1] - v[1]) < r2) {
                        k *= 3.0;
                    }
                }
            }
            Mode::Bike => {
                if f & flags::CYCLEWAY != 0 {
                    k *= 0.8;
                } else {
                    k *= row.bike;
                }
                if f & flags::UNPAVED != 0 {
                    k *= if avoid_unpaved { 4.0 } else { 1.3 };
                }
                if self.avoid.hills {
                    let (up, _) = g.climb_s(s);
                    k *= 1.0 + 10.0 * (up / g.e_len[ei].max(1.0)).min(0.3);
                }
            }
            Mode::Wheelchair => {
                if f & flags::WHEELCHAIR_YES != 0 {
                    k *= 0.85;
                } else if f & flags::FOOTWAY != 0 {
                    k *= 0.95;
                } else {
                    k *= if f & flags::SIDEWALK != 0 { row.foot_sw } else { row.foot };
                }
                if f & flags::UNPAVED != 0 || Class::from_u8(g.e_class[ei]) == Class::Track {
                    k *= 3.0;
                }
                if Class::from_u8(g.e_class[ei]) == Class::Path {
                    k *= 1.5;
                }
                let (up, down) = g.climb_s(s);
                let grade = up.max(down) / g.e_len[ei].max(1.0);
                if grade > 0.10 {
                    k *= 8.0;
                } else if grade > 0.06 {
                    k *= 3.0;
                }
            }
            Mode::Foot | Mode::Transit => {
                if f & flags::FOOTWAY != 0 {
                    k *= 0.9;
                } else {
                    k *= if f & flags::SIDEWALK != 0 { row.foot_sw } else { row.foot };
                }
                if self.avoid.hills {
                    let (up, _) = g.climb_s(s);
                    k *= 1.0 + 5.0 * (up / g.e_len[ei].max(1.0)).min(0.4);
                }
            }
        }
        t * k
    }

    /// Search cost of turning from state `a` (arriving at its head) into state `b` (leaving the
    /// same vertex): the real delay plus the route preference ("avoid difficult junctions").
    /// `None` = forbidden (turn restriction / illegal U-turn).
    #[inline]
    pub fn turn_cost(&self, g: &Graph, a: u32, b: u32) -> Option<f32> {
        self.turn_parts(g, a, b).map(|(time, pref)| time + pref)
    }

    /// The time a turn really takes (what reported durations add up): [`turn_cost`](Self::turn_cost)
    /// without the preference penalties, so avoiding left turns never inflates the ETA.
    #[inline]
    pub fn turn_time(&self, g: &Graph, a: u32, b: u32) -> Option<f32> {
        self.turn_parts(g, a, b).map(|(time, _)| time)
    }

    /// (real delay, preference penalty) of a turn; `None` = forbidden.
    #[inline]
    fn turn_parts(&self, g: &Graph, a: u32, b: u32) -> Option<(f32, f32)> {
        let ea = a >> 1;
        let eb = b >> 1;
        let via = g.head(a);
        let pedestrian = self.mode.is_pedestrian();
        if ea == eb {
            // reversing on the same edge
            return match self.mode {
                Mode::Car | Mode::Truck => {
                    if g.degree(via) <= 1 {
                        Some((if self.mode == Mode::Truck { 40.0 } else { 20.0 }, 0.0))
                    } else {
                        None
                    }
                }
                Mode::Bike => Some((15.0, 0.0)),
                _ => Some((0.0, 0.0)),
            };
        }
        // "as few traffic lights as possible": every signalised junction passed (a preference,
        // not time: it steers the search but stays out of the ETA)
        let lights = if self.avoid.traffic_lights && g.has_signal(via) { traffic_light_penalty(self.mode) } else { 0.0 };
        if !pedestrian {
            let m = self.vehicle_mask();
            let (mut only_any, mut only_ok) = (false, false);
            for r in g.restrictions_at(via) {
                if r.mask & m == 0 || r.from_edge != ea {
                    continue;
                }
                if r.only {
                    // several only_* from the same edge (one per lane group): any of them is fine
                    only_any = true;
                    only_ok |= r.to_edge == eb;
                } else if r.to_edge == eb {
                    return None;
                }
            }
            if only_any && !only_ok {
                return None;
            }
        }
        if pedestrian {
            return Some((0.0, lights));
        }
        let d = turn_delta(g.bearing_in(a), g.bearing_out(b));
        let fa = g.e_flags[ea as usize];
        let fb = g.e_flags[eb as usize];
        let mut c = if fa & flags::ROUNDABOUT != 0 && fb & flags::ROUNDABOUT != 0 {
            0.0
        } else {
            let ad = d.abs();
            if ad <= 20.0 {
                0.0
            } else if ad >= 160.0 {
                25.0
            } else if d > 0.0 {
                // right turn (driving on the right)
                if ad <= 100.0 { 2.0 } else { 5.0 }
            } else if ad <= 100.0 {
                6.0
            } else {
                10.0
            }
        };
        if fb & flags::ROUNDABOUT != 0 && fa & flags::ROUNDABOUT == 0 {
            c += 3.0;
        }
        // yielding when entering a bigger road from a smaller one
        if g.e_class[eb as usize] < g.e_class[ea as usize] && g.e_flags[ea as usize] & flags::LINK == 0 {
            c += 2.0;
        }
        // difficult junctions: a left turn crosses oncoming traffic (right-hand driving), a U-turn
        // crosses it twice; both are cheap at traffic lights and inside roundabouts.
        // (a preference, not time: it steers the search but stays out of the ETA)
        let mut pref = 0.0;
        if self.avoid.hard_turns && (d < -45.0 || d.abs() >= 160.0) && (fa | fb) & flags::ROUNDABOUT == 0 && !g.has_signal(via) {
            let biggest = Class::from_u8(g.e_class[ea as usize].min(g.e_class[eb as usize]));
            pref += hard_turn_penalty(biggest);
        }
        match self.mode {
            Mode::Bike => {
                c *= 0.5;
                pref *= 0.5;
            }
            Mode::Truck => c *= 1.5,
            _ => {}
        }
        Some((c, pref + lights))
    }

    /// Via-way restrictions apply to this profile?
    #[inline]
    pub fn uses_multi(&self, g: &Graph) -> bool {
        !g.multi.is_empty() && !self.mode.is_pedestrian()
    }

    #[inline]
    pub fn multi_applies(&self, mask: u8) -> bool {
        mask & self.vehicle_mask() != 0
    }

    /// Road hierarchy tier used to prune long searches: tier 0 edges are always searched,
    /// tier 1 only within the wide radius around origin / destination, tier 2 only within the
    /// local radius.
    #[inline]
    pub fn prune_tier(&self, g: &Graph, e: u32) -> u8 {
        let class = g.e_class[e as usize];
        let f = g.e_flags[e as usize];
        if f & flags::FERRY != 0 {
            return 0;
        }
        match self.mode {
            Mode::Car | Mode::Truck => {
                if class <= Class::Secondary as u8 {
                    0
                } else if class <= Class::Tertiary as u8 {
                    1
                } else {
                    2
                }
            }
            Mode::Bike => {
                if class <= Class::Unclassified as u8 || f & flags::CYCLEWAY != 0 {
                    0
                } else {
                    2
                }
            }
            _ => {
                if class <= Class::Residential as u8 {
                    0
                } else {
                    2
                }
            }
        }
    }
}

/// Signed turn angle in degrees (-180, 180]; positive = clockwise = right.
#[inline]
pub fn turn_delta(bearing_in: u16, bearing_out: u16) -> f32 {
    let mut d = bearing_out as f32 - bearing_in as f32;
    while d > 180.0 {
        d -= 360.0;
    }
    while d <= -180.0 {
        d += 360.0;
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta() {
        assert_eq!(turn_delta(0, 90), 90.0);
        assert_eq!(turn_delta(350, 10), 20.0);
        assert_eq!(turn_delta(10, 350), -20.0);
        assert_eq!(turn_delta(0, 180), 180.0);
        assert_eq!(turn_delta(90, 0), -90.0);
    }

    /// Right-hand traffic: negative delta = left turn (crosses oncoming traffic), |d| >= 160 = U-turn.
    #[test]
    fn hard_turn_classification() {
        let is_hard = |d: f32| d < -45.0 || d.abs() >= 160.0;
        assert!(is_hard(turn_delta(0, 270))); // heading north, leave westwards = left
        assert!(!is_hard(turn_delta(0, 90))); // right turn
        assert!(!is_hard(turn_delta(0, 10))); // straight
        assert!(is_hard(turn_delta(0, 180))); // U-turn
        assert!(is_hard(turn_delta(90, 275))); // sharp reversal counts as U-turn
        assert!(!is_hard(turn_delta(0, 330))); // slight left (lane change / fork) is not hard
        assert!(hard_turn_penalty(Class::Primary) > hard_turn_penalty(Class::Secondary));
        assert!(hard_turn_penalty(Class::Tertiary) > hard_turn_penalty(Class::Residential));
    }

    #[test]
    fn class_table_matches_classes() {
        assert_eq!(CLASS_TABLE.len(), Class::Other as usize + 1);
        assert_eq!(CLASS_TABLE[Class::Service as usize].car, 1.4);
        assert!(CLASS_TABLE[Class::Residential as usize].truck > CLASS_TABLE[Class::Primary as usize].truck);
        // the A* heuristic stays admissible: no factor below the bound used there
        for r in CLASS_TABLE {
            assert!(r.car >= 1.0 && r.truck >= 1.0 && r.bike >= 1.0 && r.foot >= 1.0 && r.foot_sw >= 1.0);
        }
    }
}
