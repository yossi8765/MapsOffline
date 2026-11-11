//! One table describing every data component: its key (manifest, package and event name),
//! folder and file in the data directory, how a package compresses it, and the format tag its
//! manifest version carries. Adding a component means one row here (plus its builder) instead of
//! a `match` in paths, package, prebuilt and the update check each.
use crate::types::DataComponent;

pub struct Spec {
    pub component: DataComponent,
    /// serde name: manifest key, package file name, `data-progress` component
    pub key: &'static str,
    /// folder inside the data directory
    pub dir: &'static str,
    /// the component's main file inside `dir`
    pub file: &'static str,
    /// Deflate in `.mapsdata` packages (PMTiles archives are compressed inside and go in stored)
    pub deflate: bool,
}

pub const TABLE: [Spec; 10] = [
    Spec { component: DataComponent::Tiles, key: "tiles", dir: "tiles", file: "israel.pmtiles", deflate: false },
    Spec { component: DataComponent::World, key: "world", dir: "world", file: "world.pmtiles", deflate: false },
    Spec { component: DataComponent::Satellite, key: "satellite", dir: "satellite", file: "israel-s2.pmtiles", deflate: false },
    Spec { component: DataComponent::Terrain, key: "terrain", dir: "terrain", file: "israel-dem-mapterhorn.pmtiles", deflate: false },
    Spec { component: DataComponent::Routing, key: "routing", dir: "routing", file: "graph.bin", deflate: true },
    Spec { component: DataComponent::Geocode, key: "geocode", dir: "geocode", file: "index.sqlite", deflate: true },
    Spec { component: DataComponent::Transit, key: "transit", dir: "transit", file: "transit.bin", deflate: true },
    Spec { component: DataComponent::Trails, key: "trails", dir: "trails", file: "trails.geojson", deflate: true },
    Spec { component: DataComponent::Minyanim, key: "minyanim", dir: "minyanim", file: "minyanim.json", deflate: true },
    Spec { component: DataComponent::Kohanim, key: "kohanim", dir: "kohanim", file: "areas.json", deflate: true },
];

pub fn spec(c: DataComponent) -> &'static Spec {
    TABLE.iter().find(|s| s.component == c).expect("every component has a row")
}

/// The component whose folder is `dir` (package entries, `tiles/…`).
pub fn by_dir(dir: &str) -> Option<DataComponent> {
    TABLE.iter().find(|s| s.dir == dir).map(|s| s.component)
}

/// The format tail of a built component's manifest version (" g6.2", " s3.3", " t2", " r1"):
/// a file built by another program version may be unreadable, so update checks and ready-made
/// packages compare it. `None` for downloaded components (their version is the source's).
pub fn format_suffix(c: DataComponent) -> Option<String> {
    match c {
        DataComponent::Routing => Some(format!(" g{}.{}", crate::routing::graph::VERSION, crate::routing::graph::BUILD_REV)),
        DataComponent::Geocode => Some(format!(" s{}.{}", crate::geocode::SCHEMA_VERSION, crate::geocode::BUILD_REV)),
        DataComponent::Transit => Some(format!(" t{}", crate::transit::model::VERSION)),
        DataComponent::Trails => Some(format!(" r{}", super::trails::BUILD_REV)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_matches_the_enum() {
        assert_eq!(TABLE.len(), DataComponent::ALL.len());
        for c in DataComponent::ALL {
            let s = spec(c);
            assert_eq!(serde_json::to_value(c).unwrap(), s.key, "key of {c:?}");
            assert_eq!(by_dir(s.dir), Some(c));
        }
    }
}
