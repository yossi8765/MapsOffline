//! Which data jobs run, so several component updates can run side by side.
//!
//! Rules:
//! - Component updates run as independent jobs at the same time, each with its own components.
//! - The OSM-derived components (routing, geocode, trails) share one PBF download and its
//!   incremental update: while a job holds any of them, a new request's OSM components are
//!   *queued* and start as soon as no running job holds OSM components any more.
//! - A component that is already running or queued is ignored.
//! - Package jobs (export, import, full-package download) are exclusive: they refuse while any
//!   component job runs or waits, and component jobs refuse while one of them runs.
//! - The cancel flags are shared: they are reset only when no job is active; a new job clears
//!   only its own components' "skip" bits.

use crate::types::DataComponent;

/// Components built from the OSM extract (one shared download).
pub const OSM_GROUP: [DataComponent; 3] = [DataComponent::Routing, DataComponent::Geocode, DataComponent::Trails];

pub fn is_osm(c: DataComponent) -> bool {
    OSM_GROUP.contains(&c)
}

/// What to do with a request, given what runs and waits now.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// start now, in a new job
    pub start: Vec<DataComponent>,
    /// wait for the running OSM work, then start
    pub queue: Vec<DataComponent>,
    /// already running or queued
    pub ignored: Vec<DataComponent>,
}

/// Pure decision: the requested components against the running and queued ones.
pub fn plan(running: &[DataComponent], queued: &[DataComponent], requested: &[DataComponent]) -> Plan {
    let osm_busy = running.iter().any(|&c| is_osm(c));
    let mut p = Plan::default();
    for &c in requested {
        if running.contains(&c) || queued.contains(&c) || p.start.contains(&c) || p.queue.contains(&c) || p.ignored.contains(&c) {
            if !p.ignored.contains(&c) && (running.contains(&c) || queued.contains(&c)) {
                p.ignored.push(c);
            }
            continue;
        }
        if is_osm(c) && osm_busy {
            p.queue.push(c);
        } else {
            p.start.push(c);
        }
    }
    p
}

/// Live registry of the data jobs (in `AppState`).
#[derive(Debug, Default)]
pub struct DataJobs {
    pub running: Vec<DataComponent>,
    pub queued: Vec<DataComponent>,
    /// a package job (export / import / full download) runs
    pub exclusive: bool,
}

impl DataJobs {
    pub fn busy(&self) -> bool {
        self.exclusive || !self.running.is_empty() || !self.queued.is_empty()
    }
    /// Queued components that may start now (no running job holds OSM components).
    pub fn startable(&self, mine: &[DataComponent]) -> bool {
        !self.running.iter().any(|&c| is_osm(c)) && mine.iter().any(|c| self.queued.contains(c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use DataComponent::*;

    #[test]
    fn nothing_running_starts_everything() {
        let p = plan(&[], &[], &[Tiles, Routing, Geocode]);
        assert_eq!(p, Plan { start: vec![Tiles, Routing, Geocode], queue: vec![], ignored: vec![] });
    }

    #[test]
    fn independent_components_start_beside_a_running_job() {
        let p = plan(&[Satellite], &[], &[Tiles, Transit, Minyanim]);
        assert_eq!(p.start, vec![Tiles, Transit, Minyanim]);
        assert!(p.queue.is_empty() && p.ignored.is_empty());
    }

    #[test]
    fn osm_components_wait_for_running_osm_work() {
        let p = plan(&[Routing, Tiles], &[], &[Geocode, Trails, World]);
        assert_eq!(p.start, vec![World]);
        assert_eq!(p.queue, vec![Geocode, Trails]);
        assert!(p.ignored.is_empty());
    }

    #[test]
    fn osm_components_start_when_only_other_jobs_run() {
        let p = plan(&[Satellite, Transit], &[], &[Routing, Geocode]);
        assert_eq!(p.start, vec![Routing, Geocode]);
    }

    #[test]
    fn running_and_queued_components_are_ignored() {
        let p = plan(&[Routing, Tiles], &[Geocode], &[Tiles, Geocode, Trails, Tiles]);
        assert_eq!(p.ignored, vec![Tiles, Geocode]);
        assert_eq!(p.queue, vec![Trails]);
        assert!(p.start.is_empty());
    }

    #[test]
    fn startable_after_osm_work_ends() {
        let mut j = DataJobs { running: vec![Routing], queued: vec![Geocode], exclusive: false };
        assert!(!j.startable(&[Geocode]));
        j.running.clear();
        assert!(j.startable(&[Geocode]));
        assert!(!j.startable(&[Trails]));
    }
}
