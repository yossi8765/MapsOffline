//! OsmAnd `.obf` map files: a reader written from the format description (not from OsmAnd's
//! GPL code) and converters into MapsOffline's own formats, so a country downloaded from
//! download.osmand.net becomes an ordinary country folder (`regions/<id>/`):
//!   - `to_pbf`: OBF (address, POI, routing, map objects) → OSM PBF, which the existing
//!     builders turn into `geocode.sqlite` (search) and `graph.bin` (routing);
//!   - `tiles`: OBF map section → Protomaps-schema vector tiles → `map.pmtiles`.
//!
//! Layout: `reader` (varints, OsmAnd's wire type 6, section table), `map`, `address`, `poi`,
//! `route` (one decoder per section), `to_pbf`, and `tiles` / `mvt` / `pmtiles_out` (map build).

pub mod reader;
pub mod map;
pub mod address;
pub mod poi;
pub mod route;
pub mod to_pbf;

pub mod tiles;
pub mod mvt;
pub mod pmtiles_out;
