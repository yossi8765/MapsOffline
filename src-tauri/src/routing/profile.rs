//! Cost profiles for car / foot / bike: access, traversal cost, turn cost, heuristic bound.
use crate::routing::graph::{flags, Class, Graph};
use crate::types::{Avoid, Mode};

const KMH: f32 = 1.0 / 3.6;
pub const FOOT_SPEED: f32 = 5.0 * KMH; // m/s
pub const BIKE_SPEED: f32 = 15.0 * KMH;
pub const CAR_MAX_SPEED: f32 = 130.0 * KMH;
/// An unpaved edge (segment between junctions) longer than this counts as a "long" dirt road.
pub const UNPAVED_LONG_M: f32 = 300.0;

/// Extra seconds for an unsignalised left turn / U-turn when avoiding difficult junctions,
/// by the most important road involved (crossed or entered).
pub fn hard_turn_penalty(class: Class) -> f32 {
    match class {
        Class::Motorway | Class::Trunk | Class::Primary => 45.0,
        Class::Secondary => 30.0,
        Class::Tertiary => 15.0,
        _ => 5.0,
    }
}

#[derive(Clone, Debug)]
pub struct Profile {
    pub mode: Mode,
    pub avoid: Avoid,
    /// edge flags that make an edge impassable (restricted territories), from `avoid`
    pub forbid: u32,
}

impl Profile {
    pub fn new(mode: Mode, avoid: Avoid) -> Self {
        // transit uses foot for access/egress
        let mode = if mode == Mode::Transit { Mode::Foot } else { mode };
        let forbid = (if avoid.area_a { flags::AREA_A } else { 0 }) | (if avoid.area_b { flags::AREA_B } else { 0 });
        Self { mode, avoid, forbid }
    }

    /// Same profile without the territory restriction (used to tell "no road" from "road is
    /// in an avoided territory").
    pub fn without_areas(&self) -> Self {
        Self { mode: self.mode, avoid: self.avoid.clone(), forbid: 0 }
    }

    pub fn avoids_areas(&self) -> bool {
        self.forbid != 0
    }

    /// Lower bound of cost per meter (seconds/m) used by the A* heuristic. For cars it uses the
    /// highest speed actually present in the graph (tighter than a fixed 130 km/h bound).
    pub fn heuristic_spm(&self, g: &Graph) -> f32 {
        let v = match self.mode {
            Mode::Car => {
                let vmax = if g.max_car_kmh >= 5 { (g.max_car_kmh as f32 * KMH).min(CAR_MAX_SPEED) } else { CAR_MAX_SPEED };
                1.0 / vmax
            }
            Mode::Foot => 0.9 / FOOT_SPEED,
            Mode::Bike => 0.8 / BIKE_SPEED,
            Mode::Transit => 0.9 / FOOT_SPEED,
        };
        v * 0.97 // projection error margin
    }

    /// May the directed state `s` be traversed?
    #[inline]
    pub fn allowed(&self, g: &Graph, s: u32) -> bool {
        let f = g.e_flags[(s >> 1) as usize];
        if f & self.forbid != 0 {
            return false;
        }
        let fwd = s & 1 == 0;
        match self.mode {
            Mode::Car => f & (if fwd { flags::CAR_FWD } else { flags::CAR_BWD }) != 0,
            Mode::Bike => f & (if fwd { flags::BIKE_FWD } else { flags::BIKE_BWD }) != 0,
            _ => f & flags::FOOT != 0,
        }
    }

    /// May edge `e` be used in at least one direction?
    #[inline]
    pub fn allowed_edge_any(&self, g: &Graph, e: u32) -> bool {
        self.allowed(g, e * 2) || self.allowed(g, e * 2 + 1)
    }

    /// Real traversal time in seconds (no preferences).
    #[inline]
    pub fn traverse_time(&self, g: &Graph, e: u32) -> f32 {
        let ei = e as usize;
        let len = g.e_len[ei];
        let f = g.e_flags[ei];
        let class = Class::from_u8(g.e_class[ei]);
        match self.mode {
            Mode::Car => {
                let v = (g.e_speed[ei].max(5) as f32) * KMH;
                // tracks / unpaved are slower than their nominal speed
                let v = if f & flags::UNPAVED != 0 { v.min(25.0 * KMH) } else { v };
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
                len / v
            }
            _ => {
                let v = if f & flags::STEPS != 0 { 2.0 * KMH } else { FOOT_SPEED };
                len / v
            }
        }
    }

    /// Search cost (seconds × preference factors) of traversing edge `e`.
    #[inline]
    pub fn cost(&self, g: &Graph, e: u32) -> f32 {
        let t = self.traverse_time(g, e);
        let ei = e as usize;
        let f = g.e_flags[ei];
        let class = Class::from_u8(g.e_class[ei]);
        // "avoid unpaved" either applies to every unpaved edge or only to long stretches
        let avoid_unpaved = self.avoid.unpaved && (!self.avoid.unpaved_long_only || g.e_len[ei] > UNPAVED_LONG_M);
        let mut k = 1.0f32;
        match self.mode {
            Mode::Car => {
                match class {
                    Class::Service => k *= 1.4,
                    Class::LivingStreet => k *= 1.3,
                    Class::Track => k *= 2.5,
                    Class::Road => k *= 1.5,
                    _ => {}
                }
                if f & flags::UNPAVED != 0 {
                    k *= if avoid_unpaved { 5.0 } else { 1.5 };
                }
                if f & flags::TOLL != 0 && self.avoid.toll {
                    k *= 5.0;
                }
                if f & flags::MOTORWAY != 0 && self.avoid.motorway {
                    k *= 3.0;
                }
                if f & flags::FERRY != 0 && self.avoid.ferry {
                    k *= 10.0;
                }
            }
            Mode::Bike => {
                if f & flags::CYCLEWAY != 0 {
                    k *= 0.8;
                } else {
                    match class {
                        Class::Primary => k *= 1.4,
                        Class::Secondary => k *= 1.2,
                        Class::Tertiary => k *= 1.05,
                        Class::Trunk => k *= 1.8,
                        Class::Motorway => k *= 3.0,
                        _ => {}
                    }
                }
                if f & flags::UNPAVED != 0 {
                    k *= if avoid_unpaved { 4.0 } else { 1.3 };
                }
            }
            _ => {
                if f & flags::FOOTWAY != 0 {
                    k *= 0.9;
                } else {
                    match class {
                        Class::Primary | Class::Secondary => k *= if f & flags::SIDEWALK != 0 { 1.0 } else { 1.15 },
                        Class::Tertiary => k *= 1.05,
                        Class::Trunk => k *= 1.6,
                        Class::Motorway => k *= 3.0,
                        Class::Track => k *= 1.1,
                        _ => {}
                    }
                }
            }
        }
        t * k
    }

    /// Cost of turning from state `a` (arriving at its head) into state `b` (leaving the same vertex).
    /// `None` = forbidden (turn restriction / illegal U-turn).
    #[inline]
    pub fn turn_cost(&self, g: &Graph, a: u32, b: u32) -> Option<f32> {
        let ea = a >> 1;
        let eb = b >> 1;
        let via = g.head(a);
        if ea == eb {
            // reversing on the same edge
            return match self.mode {
                Mode::Car => {
                    if g.degree(via) <= 1 {
                        Some(20.0)
                    } else {
                        None
                    }
                }
                Mode::Bike => Some(15.0),
                _ => Some(0.0),
            };
        }
        if self.mode != Mode::Foot && self.mode != Mode::Transit {
            let bike = self.mode == Mode::Bike;
            for r in g.restrictions_at(via) {
                if bike && !r.bike {
                    continue;
                }
                if r.from_edge != ea {
                    continue;
                }
                if r.only {
                    if r.to_edge != eb {
                        return None;
                    }
                } else if r.to_edge == eb {
                    return None;
                }
            }
        }
        if self.mode == Mode::Foot || self.mode == Mode::Transit {
            return Some(0.0);
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
        if self.avoid.hard_turns
            && (d < -45.0 || d.abs() >= 160.0)
            && (fa | fb) & flags::ROUNDABOUT == 0
            && !g.has_signal(via)
        {
            let biggest = Class::from_u8(g.e_class[ea as usize].min(g.e_class[eb as usize]));
            c += hard_turn_penalty(biggest);
        }
        if self.mode == Mode::Bike {
            c *= 0.5;
        }
        Some(c)
    }

    /// Road hierarchy tier used to prune long searches: tier 0 edges are always searched,
    /// tier 1 only within the wide radius around origin / destination, tier 2 only within the
    /// local radius.
    #[inline]
    pub fn prune_tier(&self, g: &Graph, e: u32) -> u8 {
        let class = g.e_class[e as usize];
        match self.mode {
            Mode::Car => {
                if class <= Class::Secondary as u8 {
                    0
                } else if class <= Class::Tertiary as u8 {
                    1
                } else {
                    2
                }
            }
            Mode::Bike => {
                if class <= Class::Unclassified as u8 || g.e_flags[e as usize] & flags::CYCLEWAY != 0 {
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
}
