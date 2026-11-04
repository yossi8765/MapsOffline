//! CLI to build / inspect MapsOffline data without the GUI.
//!
//! mapsoffline-data build --all | --tiles --routing --geocode --transit [--dir <data dir>]
//! mapsoffline-data status [--dir <data dir>]
//! mapsoffline-data route --from lon,lat --to lon,lat [--mode car|foot|bike|transit]
//! mapsoffline-data search "<query>"
#![allow(clippy::type_complexity, clippy::too_many_arguments, clippy::needless_range_loop)]
use std::sync::Arc;

use clap::{Parser, Subcommand};
use mapsoffline_lib::data::{self, Cancel};
use mapsoffline_lib::paths::DataPaths;
use mapsoffline_lib::types::*;

#[derive(Parser)]
#[command(name = "mapsoffline-data", about = "Build and inspect MapsOffline data")]
struct Cli {
    /// Data directory (default: %APPDATA%\MapsOffline\data or $MAPSOFFLINE_DATA_DIR)
    #[arg(long, global = true)]
    dir: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Download sources and build derived data files
    Build {
        #[arg(long)]
        all: bool,
        #[arg(long)]
        tiles: bool,
        #[arg(long)]
        world: bool,
        #[arg(long)]
        satellite: bool,
        #[arg(long)]
        terrain: bool,
        #[arg(long)]
        routing: bool,
        #[arg(long)]
        geocode: bool,
        #[arg(long)]
        transit: bool,
        /// marked hiking / cycling trails (from the OSM PBF)
        #[arg(long)]
        trails: bool,
        /// synagogue prayer times (Nedarim Plus, ~10-15 min of polite requests)
        #[arg(long)]
        minyanim: bool,
    },
    /// Re-encode the installed satellite archive's tiles to WebP with the `cwebp` tool (CI:
    /// smaller ready-made package). `cwebp` must be on PATH (Debian/Ubuntu package "webp").
    WebpSatellite {
        #[arg(long, default_value_t = 80)]
        quality: u8,
    },
    /// Other countries: `region list [filter]`, `region install <id> [--map-only|--search-only]`,
    /// `region delete <id>`, `region search <id> <query>`
    Region {
        action: String,
        args: Vec<String>,
        #[arg(long)]
        map_only: bool,
        #[arg(long)]
        search_only: bool,
        #[arg(long)]
        routing_only: bool,
    },
    /// Show status of data files
    Status,
    /// Ask the servers whether newer data is available (no download)
    Check,
    /// Bring the local OSM extract up to date – daily Geofabrik diffs when possible – without rebuilding
    OsmUpdate,
    /// Pack installed data into one .mapsdata file (all components, or only the listed ones)
    Export {
        dest: String,
        /// Components to include: tiles, satellite, routing, geocode, transit
        #[arg(long, value_delimiter = ',')]
        only: Vec<String>,
    },
    /// Install a .mapsdata file into the data dir
    Import {
        src: String,
    },
    /// Compute a route
    Route {
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        #[arg(long, default_value = "car")]
        mode: String,
        #[arg(long)]
        time: Option<String>,
        #[arg(long)]
        arrive: bool,
        #[arg(long, default_value_t = 3)]
        alternatives: u32,
        /// auto | online | offline
        #[arg(long, default_value = "offline")]
        source: String,
        /// Override the OSRM base URL (for testing the offline fallback)
        #[arg(long)]
        osrm_url: Option<String>,
        /// Override the transit API base URL
        #[arg(long)]
        transit_url: Option<String>,
        /// Avoid restricted territories: a (Area A, Hebron H1, Gaza) | ab (Areas A and B)
        #[arg(long)]
        avoid_areas: Option<String>,
        /// Avoid toll roads
        #[arg(long)]
        avoid_toll: bool,
        /// Unpaved roads: allow | avoidLong | avoid
        #[arg(long, default_value = "allow")]
        unpaved: String,
        /// Avoid difficult junctions (unsignalised left turns / U-turns)
        #[arg(long)]
        hard_turns: bool,
    },
    /// Geocode a query (addresses, places, POIs and – when transit data exists – bus stops)
    Search {
        query: String,
        /// Rank by distance from this point: lon,lat
        #[arg(long)]
        near: Option<String>,
    },
    /// Which restricted territory (Area A / B) contains a point, and the flags of the nearest road
    Area {
        /// lon,lat
        at: String,
    },
    /// Departures of a transit line for the rest of today (favorites view)
    LineTimes {
        agency: String,
        line: String,
        /// Any route_long_name of the line, to pick one of the agency's same-numbered lines
        #[arg(long)]
        long_name: Option<String>,
    },
    /// Departure board of a stop by stop code (favorites view)
    StopBoard {
        code: String,
    },
    /// Elevation at points from the installed DEM: lon,lat [lon,lat ...]
    Elev {
        at: Vec<String>,
    },
    /// Ask GitLab for the newest signed program release (the in-app update check)
    AppCheck,
    /// Download + verify the newest signed installer into the update folder (does not run it)
    AppDownload,
    /// Convert a transit.bin v2 (bincode) into the current format, no GTFS zip needed
    TransitConvert {
        src: String,
        dst: String,
    },
    /// Build transit.bin from a GTFS zip; stop-to-stop transfers are walked on the installed
    /// road graph unless --no-graph
    TransitBuild {
        gtfs: String,
        out: String,
        #[arg(long)]
        no_graph: bool,
    },
    /// Transit planning latencies (TA→Jerusalem depart/arrive, Haifa→Beersheba, isochrone,
    /// stops layer) on the installed or a given transit.bin (use --release)
    TransitBench {
        #[arg(long)]
        file: Option<String>,
        #[arg(long, default_value_t = 5)]
        iters: usize,
    },
    /// Measure load / query latencies on the installed data (use --release)
    Bench {
        #[arg(long, default_value_t = 5)]
        iters: usize,
    },
}

fn parse_ll(s: &str) -> anyhow::Result<Waypoint> {
    let mut it = s.split(',');
    let lon: f64 = it.next().ok_or_else(|| anyhow::anyhow!("bad coord"))?.trim().parse()?;
    let lat: f64 = it.next().ok_or_else(|| anyhow::anyhow!("bad coord"))?.trim().parse()?;
    Ok(Waypoint { lon, lat, name: None })
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();
    let root = cli.dir.map(std::path::PathBuf::from).unwrap_or_else(DataPaths::default_root);
    let paths = DataPaths::new(root);
    paths.ensure_dirs()?;

    match cli.cmd {
        Cmd::Region { action, args, map_only, search_only, routing_only } => {
            use mapsoffline_lib::data::regions;
            use mapsoffline_lib::types::RegionPart;
            match action.as_str() {
                "list" => {
                    let f = args.first().map(|s| s.to_lowercase()).unwrap_or_default();
                    for r in regions::catalog(&paths, false)?.iter().filter(|r| r.entry.id.contains(&f) || r.entry.name.to_lowercase().contains(&f)) {
                        println!("{:<28} {:<20} {:<4} {:?}", r.entry.id, r.entry.parent.clone().unwrap_or_default(), r.entry.iso.clone().unwrap_or_default(), r.entry.bbox);
                    }
                    for r in regions::installed(&paths, &|_| false) {
                        println!("installed: {} map={:?} ({} B) search={:?} ({} B)", r.id, r.map.version, r.map.size_bytes, r.search.version, r.search.size_bytes);
                    }
                }
                "install" => {
                    let id = args.first().ok_or_else(|| anyhow::anyhow!("region id missing"))?;
                    let parts = if map_only {
                        vec![RegionPart::Map]
                    } else if search_only {
                        vec![RegionPart::Search]
                    } else if routing_only {
                        vec![RegionPart::Routing]
                    } else {
                        vec![RegionPart::Map, RegionPart::Search, RegionPart::Routing]
                    };
                    let t0 = std::time::Instant::now();
                    let emit = |p: mapsoffline_lib::types::RegionProgress| {
                        let pct = if p.total > 0 { format!("{:5.1}%", p.done as f64 * 100.0 / p.total as f64) } else { "     ".into() };
                        eprintln!("[{} {:?}] {:?} {} {}", p.id, p.part, p.phase, pct, p.message);
                    };
                    regions::install(&paths, id, &parts, &emit, &data::Cancel::default())?;
                    println!("installed {id} in {:?}", t0.elapsed());
                }
                "delete" => regions::delete(&paths, args.first().ok_or_else(|| anyhow::anyhow!("region id missing"))?)?,
                "search" => {
                    let id = args.first().ok_or_else(|| anyhow::anyhow!("region id missing"))?;
                    let q = args[1..].join(" ");
                    let g = mapsoffline_lib::geocode::open(&regions::geocode_file(&paths.region_dir(id)))?;
                    let t0 = std::time::Instant::now();
                    for r in g.search(&q, None, 10)? {
                        println!("{:?} {} | {} {} | {:.5},{:.5}", r.kind, r.name, r.city.unwrap_or_default(), r.category.unwrap_or_default(), r.lon, r.lat);
                    }
                    println!("({:?})", t0.elapsed());
                }
                "locate" => {
                    let q = args.join(" ");
                    let cat = regions::catalog(&paths, false)?;
                    let name = |id: &str| cat.iter().find(|f| f.entry.id == id).map(|f| f.entry.name.clone()).unwrap_or_else(|| id.to_string());
                    for r in regions::locate(&paths, &q, "he")? {
                        println!("{} | {} ({:.3},{:.3}) -> {}", r.name, r.detail, r.lon, r.lat, r.regions.iter().map(|i| name(i)).collect::<Vec<_>>().join(" < "));
                    }
                }
                "route" => {
                    // region route <id> lon,lat lon,lat [car|foot|bike]
                    let id = args.first().ok_or_else(|| anyhow::anyhow!("region id missing"))?;
                    let pt = |s: &str| -> anyhow::Result<mapsoffline_lib::types::Waypoint> {
                        let (a, b) = s.split_once(',').ok_or_else(|| anyhow::anyhow!("lon,lat expected"))?;
                        Ok(serde_json::from_value(serde_json::json!({"lon": a.trim().parse::<f64>()?, "lat": b.trim().parse::<f64>()?}))?)
                    };
                    let mode = args.get(3).map(String::as_str).unwrap_or("car");
                    let req: mapsoffline_lib::types::RouteRequest = serde_json::from_value(serde_json::json!({
                        "waypoints": [pt(&args[1])?, pt(&args[2])?], "mode": mode, "time": {"kind": "now"}, "alternatives": 2
                    }))?;
                    let t0 = std::time::Instant::now();
                    let g = mapsoffline_lib::routing::load_graph(&regions::graph_file(&paths.region_dir(id)))?;
                    println!("graph loaded in {:?} (proj {:?})", t0.elapsed(), g.proj0);
                    let t1 = std::time::Instant::now();
                    let resp = g.route(&req)?;
                    for r in &resp.routes {
                        println!("{:.1} km, {} min, {} legs", r.distance_m / 1000.0, (r.duration_sec / 60.0).round(), r.legs.len());
                    }
                    if let Some(st) = resp.routes.first().and_then(|r| r.legs.first()).map(|l| &l.steps) {
                        for s in st.iter().take(6) {
                            println!("  {}", s.instruction);
                        }
                    }
                    println!("routed in {:?}", t1.elapsed());
                }
                other => anyhow::bail!("unknown region action {other}"),
            }
        }
        Cmd::WebpSatellite { quality } => {
            let file = paths.satellite_file();
            anyhow::ensure!(file.exists(), "no satellite archive at {}", file.display());
            let dir = std::env::temp_dir().join(format!("mo-webp-{}", std::process::id()));
            std::fs::create_dir_all(&dir)?;
            let n = std::sync::atomic::AtomicU64::new(0);
            let conv = |jpg: &[u8]| -> anyhow::Result<Option<Vec<u8>>> {
                // only JPEG tiles are re-encoded (PNG / WebP stay as they are)
                if !(jpg.len() > 2 && jpg[0] == 0xFF && jpg[1] == 0xD8) {
                    return Ok(None);
                }
                let k = n.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let (src, dst) = (dir.join(format!("{k}.jpg")), dir.join(format!("{k}.webp")));
                std::fs::write(&src, jpg)?;
                let st = std::process::Command::new("cwebp").args(["-quiet", "-q", &quality.to_string(), "-m", "6"]).arg(&src).arg("-o").arg(&dst).status()?;
                anyhow::ensure!(st.success(), "cwebp failed ({st})");
                let out = std::fs::read(&dst)?;
                let _ = std::fs::remove_file(&src);
                let _ = std::fs::remove_file(&dst);
                // every tile converts (even the rare one that grows): the archive's tile type is
                // WebP, and the app checks a sample tile's signature against it
                Ok(Some(out))
            };
            let tmp = file.with_extension("webp.pmtiles");
            let (before, after) = mapsoffline_lib::data::satellite::transcode(&file, &tmp, 4, &conv)?;
            mapsoffline_lib::paths::atomic_replace(&file, &tmp)?;
            let _ = std::fs::remove_dir_all(&dir);
            println!("satellite tiles {:.1} MB -> {:.1} MB ({:+.0} %)", before as f64 / 1e6, after as f64 / 1e6, (after as f64 / before.max(1) as f64 - 1.0) * 100.0);
        }
        Cmd::Build { all, tiles, world, satellite, terrain, routing, geocode, transit, trails, minyanim } => {
            let mut comps = vec![];
            if all || tiles {
                comps.push(DataComponent::Tiles);
            }
            if all || world {
                comps.push(DataComponent::World);
            }
            if all || satellite {
                comps.push(DataComponent::Satellite);
            }
            if all || terrain {
                comps.push(DataComponent::Terrain);
            }
            if all || routing {
                comps.push(DataComponent::Routing);
            }
            if all || geocode {
                comps.push(DataComponent::Geocode);
            }
            if all || transit {
                comps.push(DataComponent::Transit);
            }
            if all || trails {
                comps.push(DataComponent::Trails);
            }
            if all || minyanim {
                comps.push(DataComponent::Minyanim);
            }
            if comps.is_empty() {
                anyhow::bail!("nothing selected; use --all or component flags");
            }
            let emit: data::Emit = Arc::new(|p: DataProgress| {
                let pct = if p.total > 0 { format!("{:5.1}%", p.done as f64 * 100.0 / p.total as f64) } else { "     ".into() };
                eprintln!("[{:?}] {:?} {} {}", p.component, p.phase, pct, p.message);
            });
            data::update(&comps, &paths, emit, &Cancel::default())?;
            println!("done");
        }
        Cmd::Status => {
            let s = data::status(&paths);
            println!("{}", serde_json::to_string_pretty(&s)?);
        }
        Cmd::Export { dest, only } => {
            let only = only
                .iter()
                .map(|s| serde_json::from_value::<DataComponent>(serde_json::Value::String(s.clone())).map_err(|_| anyhow::anyhow!("unknown component: {s}")))
                .collect::<anyhow::Result<Vec<_>>>()?;
            package_cmd(&paths, &dest, Some(&only))?
        }
        Cmd::Import { src } => package_cmd(&paths, &src, None)?,
        Cmd::Check => {
            let t0 = std::time::Instant::now();
            let c = data::check(&paths);
            println!("{}", serde_json::to_string_pretty(&c)?);
            eprintln!("checked in {:?}", t0.elapsed());
        }
        Cmd::Route { from, to, mode, time, arrive, alternatives, source, osrm_url, transit_url, avoid_areas, avoid_toll, unpaved, hard_turns } => {
            let mode = match mode.as_str() {
                "car" => Mode::Car,
                "foot" => Mode::Foot,
                "bike" => Mode::Bike,
                "transit" => Mode::Transit,
                m => anyhow::bail!("unknown mode {m}"),
            };
            let avoid = Avoid {
                toll: avoid_toll,
                area_a: matches!(avoid_areas.as_deref(), Some("a") | Some("ab")),
                area_b: matches!(avoid_areas.as_deref(), Some("ab")),
                unpaved: unpaved != "allow",
                unpaved_long_only: unpaved == "avoidLong",
                hard_turns,
                ..Default::default()
            };
            let req = RouteRequest {
                waypoints: vec![parse_ll(&from)?, parse_ll(&to)?],
                mode,
                time: match time {
                    Some(iso) => TimeSpec { kind: if arrive { TimeKind::Arrive } else { TimeKind::Depart }, iso: Some(iso) },
                    None => TimeSpec::default(),
                },
                accessible: false,
                avoid,
                alternatives,
            };
            // graph is needed for offline routing and for checking online routes against territories
            let graph = mapsoffline_lib::routing::load_graph(&paths.graph_file()).ok();
            let online = if source != "offline" {
                let mut settings = Settings::default();
                if let Some(u) = osrm_url {
                    settings.osrm_url = u;
                }
                if let Some(u) = transit_url {
                    settings.transit_api_url = u;
                }
                match mapsoffline_lib::online::route(&req, &settings) {
                    Ok(mut r) => {
                        if let Some(g) = &graph {
                            let dropped = g.drop_routes_in_avoided_areas(&req, &mut r);
                            if dropped > 0 {
                                eprintln!("online routes entering avoided territories dropped: {dropped}");
                            }
                        }
                        if r.routes.is_empty() {
                            eprintln!("all online routes enter avoided territories; using local routing");
                            None
                        } else if matches!(req.mode, Mode::Foot | Mode::Bike) && {
                            let local = graph.as_ref().map(|g| g.route(&req));
                            if let Some(Err(e)) = &local {
                                eprintln!("local comparison route failed: {e:#}");
                            }
                            local.and_then(|l| l.ok()).and_then(|l| l.routes.first().map(|x| x.duration_sec)).is_some_and(|ld| ld < r.routes[0].duration_sec * 0.7)
                        } {
                            // same rule as commands::route_hybrid: OSRM's foot/bike profiles detour around trunk roads
                            eprintln!("online {:?} route is far longer than the local one; using local routing", req.mode);
                            None
                        } else {
                            Some(r)
                        }
                    }
                    Err(e) => {
                        eprintln!("online routing failed: {e:#}");
                        if source == "online" {
                            return Err(e);
                        }
                        None
                    }
                }
            } else {
                None
            };
            let resp = match online {
                Some(r) => r,
                None => match mode {
                    Mode::Transit => {
                        let t = mapsoffline_lib::transit::load(&paths.transit_file())?;
                        match &graph {
                            Some(g) => t.route(&req, g)?,
                            None => t.route(&req, &mapsoffline_lib::transit::NoWalk)?,
                        }
                    }
                    _ => {
                        let g = graph.as_ref().ok_or_else(|| anyhow::anyhow!("routing graph not available: {}", paths.graph_file().display()))?;
                        g.route(&req)?
                    }
                },
            };
            println!("source: {:?} provider: {:?}", resp.source, resp.provider);
            if let Some(g) = &graph {
                println!("graph: {} signalised junctions", g.count_signals());
            }
            for r in &resp.routes {
                let unpaved_m = graph.as_ref().map(|g| format!("{:.0} m", g.unpaved_length_m(&r.geometry))).unwrap_or_else(|| "?".into());
                let areas = graph
                    .as_ref()
                    .map(|g| match g.areas.route_first_hit(&r.geometry) {
                        Some((p, k)) => format!("{} (first at {:.5},{:.5})", mapsoffline_lib::routing::areas::kinds_label(k), p[0], p[1]),
                        None => "none".into(),
                    })
                    .unwrap_or_else(|| "?".into());
                println!(
                    "route {} tags={:?} {:.1} min, {:.2} km, {} legs, transfers={} areas: {} unpaved: {}",
                    r.id,
                    r.tags,
                    r.duration_sec / 60.0,
                    r.distance_m / 1000.0,
                    r.legs.len(),
                    r.transfers,
                    areas,
                    unpaved_m
                );
                for leg in &r.legs {
                    if let Some(t) = &leg.transit {
                        let code = |i: usize| t.stops.get(i).and_then(|s| s.code.clone()).map(|c| format!(" [{c}]")).unwrap_or_default();
                        println!(
                            "   {:?} {} {}{} -> {}{} ({} - {}) {} stops",
                            t.vehicle, t.route_short_name, t.from_stop, code(0), t.to_stop, code(t.stops.len().saturating_sub(1)), t.depart_iso, t.arrive_iso, t.num_stops
                        );
                    } else {
                        println!("   {:?} {:.0} m {:.1} min", leg.kind, leg.distance_m, leg.duration_sec / 60.0);
                        for s in &leg.steps {
                            println!("      {:?} {:.0}m  {}", s.maneuver, s.distance_m, s.instruction);
                        }
                    }
                }
            }
            println!("computed in {} ms", resp.compute_ms);
        }
        Cmd::LineTimes { agency, line, long_name } => {
            let t = mapsoffline_lib::transit::load(&paths.transit_file())?;
            let t0 = std::time::Instant::now();
            match mapsoffline_lib::transit::board::line_times_of(&t, &line, &agency, None, long_name.as_deref()) {
                None => println!("no service today / unknown line"),
                Some(lt) => {
                    println!("{} {} ({}) directions={} in {:?}", lt.agency, lt.short_name, lt.long_name, lt.directions.len(), t0.elapsed());
                    for d in &lt.directions {
                        let times: Vec<String> = d.departures.iter().take(12).map(|(rel, iso)| format!("{}({:+}m)", &iso[11..16], rel / 60)).collect();
                        println!("  -> {} [{} -> {}] {} min, {} departures: {} ...", d.headsign, d.from_stop, d.to_stop, d.duration_sec / 60, d.departures.len(), times.join(" "));
                    }
                }
            }
        }
        Cmd::OsmUpdate => {
            let t0 = std::time::Instant::now();
            let progress = |ph: ProgressPhase, d: u64, t: u64, m: &str| -> bool {
                if t == 0 || t < 1000 || d % (1 << 20) < 65536 {
                    eprintln!("[{ph:?}] {d}/{t} {m}");
                }
                true
            };
            let pbf = paths.pbf_file();
            let v = mapsoffline_lib::data::osmdiff::update_pbf(&pbf, &progress, &Cancel::default())?;
            let (st, _) = mapsoffline_lib::data::osmdiff::header_info(&pbf)?;
            println!("version {v:?} header {st:?} size {} MB in {:?}", std::fs::metadata(&pbf)?.len() >> 20, t0.elapsed());
        }
        Cmd::AppCheck => {
            let t0 = std::time::Instant::now();
            let u = mapsoffline_lib::app_update::check()?;
            println!("current {} latest {} available={} setup={:?} in {:?}", u.current, u.latest, u.available, u.setup_url, t0.elapsed());
        }
        Cmd::AppDownload => {
            let progress = |ph: ProgressPhase, d: u64, t: u64, m: &str| -> bool {
                if t > 0 && d % (1 << 20) < 65536 || t == 0 {
                    eprintln!("[{ph:?}] {} / {} MB {m}", d >> 20, t >> 20);
                }
                true
            };
            let cancel = Cancel::default();
            match mapsoffline_lib::app_update::download(&progress, &cancel) {
                Ok(p) => println!("verified installer at {}", p.display()),
                Err(e) => println!("download refused: {e:#}"),
            }
        }
        Cmd::Elev { at } => {
            let pts: Vec<[f64; 2]> = at.iter().map(|a| parse_ll(a).map(|w| [w.lon, w.lat])).collect::<anyhow::Result<_>>()?;
            let t0 = std::time::Instant::now();
            let e = mapsoffline_lib::data::terrain::elevations(&paths.terrain_file(), &pts)?;
            for (p, h) in pts.iter().zip(e) {
                println!("{:.4},{:.4} -> {}", p[0], p[1], h.map(|v| format!("{v:.0} m")).unwrap_or_else(|| "no data".into()));
            }
            println!("in {:?}", t0.elapsed());
        }
        Cmd::StopBoard { code } => {
            let t = mapsoffline_lib::transit::load(&paths.transit_file())?;
            let t0 = std::time::Instant::now();
            match mapsoffline_lib::transit::board::stop_board(&t, Some(&code), 0.0, 0.0) {
                None => println!("stop not found"),
                Some(b) => {
                    println!("{} [{}] lines={} in {:?}", b.stop.name, b.stop.code.unwrap_or_default(), b.lines.len(), t0.elapsed());
                    for l in b.lines.iter().take(15) {
                        let next: Vec<String> = l.next.iter().map(|(rel, iso)| format!("{}({:+}m)", &iso[11..16], rel / 60)).collect();
                        println!("  {} {} -> {} {} today={} next: {}", l.agency, l.short_name, l.headsign, if l.terminus { "(terminus)" } else { "" }, l.total_today, next.join(" "));
                    }
                }
            }
        }
        Cmd::Area { at } => {
            use mapsoffline_lib::routing::{areas, graph::flags};
            let w = parse_ll(&at)?;
            let p = [w.lon, w.lat];
            let g = mapsoffline_lib::routing::load_graph(&paths.graph_file())?;
            println!("rings: {}", g.areas.num_rings());
            println!("point: exact {} / with tolerance {}", areas::kinds_label(g.areas.classify(p, 0.0)), areas::kinds_label(g.areas.classify(p, areas::EDGE_TOL_M)));
            let any = |_e: u32| true;
            for s in g.snap_candidates(p, &any, 5) {
                let f = g.flags(s.edge);
                let k = (if f & flags::AREA_A != 0 { areas::KIND_A } else { 0 }) | (if f & flags::AREA_B != 0 { areas::KIND_B } else { 0 });
                let pts = g.edge_points(s.edge);
                let inside: Vec<String> = pts.iter().map(|q| areas::kinds_label(g.areas.classify(*q, areas::EDGE_TOL_M))).collect();
                println!(
                    "edge {} {:?} {:.0} m away, {} pts, flagged: {}, car: {}, samples: {}",
                    s.edge,
                    g.name(s.edge).unwrap_or("-"),
                    s.dist_m,
                    pts.len(),
                    areas::kinds_label(k),
                    f & (flags::CAR_FWD | flags::CAR_BWD) != 0,
                    inside.join(" ")
                );
            }
        }
        Cmd::Search { query, near } => {
            let near = match near {
                Some(s) => {
                    let w = parse_ll(&s)?;
                    Some([w.lon, w.lat])
                }
                None => None,
            };
            let g = mapsoffline_lib::geocode::open(&paths.geocode_file())?;
            let t0 = std::time::Instant::now();
            let geo = g.search(&query, near, 10)?;
            let geo_ms = t0.elapsed().as_secs_f64() * 1e3;
            let mut stops = Vec::new();
            let mut stop_ms = 0.0;
            if paths.transit_file().exists() {
                let t = mapsoffline_lib::transit::load(&paths.transit_file())?;
                let t1 = std::time::Instant::now();
                stops = t.search_stops(&query, near, 5);
                stop_ms = t1.elapsed().as_secs_f64() * 1e3;
            }
            for r in mapsoffline_lib::geocode::merge_with_stops(geo, stops, 10) {
                println!(
                    "{:?}/{:?} {} [{}] ({:.5},{:.5}) score={:.1}",
                    r.kind,
                    r.precision,
                    r.name,
                    r.category.as_deref().unwrap_or(""),
                    r.lon,
                    r.lat,
                    r.score
                );
            }
            println!("geocoder {geo_ms:.1} ms, stops {stop_ms:.1} ms");
        }
        Cmd::Bench { iters } => bench::run(&paths, iters.max(1))?,
        Cmd::TransitConvert { src, dst } => {
            let t0 = std::time::Instant::now();
            let t = mapsoffline_lib::transit::legacy::load_v2(std::path::Path::new(&src))?;
            mapsoffline_lib::transit::model::save(&t, std::path::Path::new(&dst))?;
            println!("converted in {:?}: {} stops, {} patterns, {} bytes", t0.elapsed(), t.stops.len(), t.patterns.len(), std::fs::metadata(&dst)?.len());
        }
        Cmd::TransitBuild { gtfs, out, no_graph } => {
            let graph = if no_graph { None } else { mapsoffline_lib::routing::load_graph(&paths.graph_file()).ok() };
            println!("road graph for transfers: {}", if graph.is_some() { "yes" } else { "no" });
            let t0 = std::time::Instant::now();
            let progress = |_: ProgressPhase, done: u64, total: u64, msg: &str| {
                if total == 0 || done == total {
                    eprintln!("{msg}");
                }
                true
            };
            mapsoffline_lib::transit::build_with_graph(std::path::Path::new(&gtfs), std::path::Path::new(&out), &progress, graph.as_ref())?;
            println!("built in {:?}: {} bytes", t0.elapsed(), std::fs::metadata(&out)?.len());
        }
        Cmd::TransitBench { file, iters } => bench::transit(&paths, file.as_deref(), iters.max(1))?,
    }
    Ok(())
}

mod bench {
    use std::time::Instant;

    use mapsoffline_lib::paths::DataPaths;
    use mapsoffline_lib::types::*;

    fn req(from: [f64; 2], to: [f64; 2], mode: Mode, alternatives: u32) -> RouteRequest {
        RouteRequest {
            waypoints: vec![Waypoint { lon: from[0], lat: from[1], name: None }, Waypoint { lon: to[0], lat: to[1], name: None }],
            mode,
            time: TimeSpec::default(),
            accessible: false,
            avoid: Avoid::default(),
            alternatives,
        }
    }

    /// Transit scenarios: load, depart-at / arrive-by plans with the road-network walker,
    /// an isochrone and the stops layer.
    pub fn transit(paths: &DataPaths, file: Option<&str>, iters: usize) -> anyhow::Result<()> {
        use chrono::{Datelike, Duration, Weekday};
        let path = file.map(std::path::PathBuf::from).unwrap_or_else(|| paths.transit_file());
        let t0 = Instant::now();
        let t = mapsoffline_lib::transit::load(&path)?;
        println!("transit load            {:8.1} ms  ({} stops, {} patterns, {} trips, {} MB file)", ms(t0), t.stops.len(), t.patterns.len(), t.trip_service.len(), std::fs::metadata(&path)?.len() / 1_000_000);
        let graph = mapsoffline_lib::routing::load_graph(&paths.graph_file()).ok();
        let walker: &dyn mapsoffline_lib::transit::WalkRouter = match &graph {
            Some(g) => g,
            None => &mapsoffline_lib::transit::NoWalk,
        };
        // a Tuesday inside the feed
        let mut date = chrono::Utc::now().date_naive();
        while date.weekday() != Weekday::Tue || t.day_index(date).is_none() {
            date += Duration::days(1);
            if date > chrono::Utc::now().date_naive() + Duration::days(60) {
                anyhow::bail!("no Tuesday in the feed's date range");
            }
        }
        let at = |h: u32| format!("{date}T{h:02}:00:00");
        let scen: [(&str, [f64; 2], [f64; 2], TimeKind, String); 4] = [
            ("TA Azrieli → Jerusalem CBS dep 08:00", [34.7925, 32.0745], [35.2033, 31.7890], TimeKind::Depart, at(8)),
            ("TA Azrieli → Jerusalem CBS arr 10:00", [34.7925, 32.0745], [35.2033, 31.7890], TimeKind::Arrive, at(10)),
            ("Haifa Merkaz → Beersheba dep 08:00 ", [34.9965, 32.8220], [34.7913, 31.2430], TimeKind::Depart, at(8)),
            ("Bnei Brak → Ramat Gan dep 17:00    ", [34.8338, 32.0860], [34.8125, 32.0700], TimeKind::Depart, at(17)),
        ];
        for (name, a, b, kind, iso) in scen {
            let mut r = req(a, b, Mode::Transit, 3);
            r.time = TimeSpec { kind, iso: Some(iso) };
            let mut first = None;
            let (med, max) = time(iters, || {
                let resp = t.route(&r, walker);
                if first.is_none() {
                    first = Some(resp);
                }
            });
            let summary = match first {
                Some(Ok(resp)) => resp
                    .routes
                    .first()
                    .map(|x| format!("{} routes, best {} → {} ({} transfers)", resp.routes.len(), x.depart_iso.as_deref().map(|s| &s[11..16]).unwrap_or("?"), x.arrive_iso.as_deref().map(|s| &s[11..16]).unwrap_or("?"), x.transfers))
                    .unwrap_or_else(|| format!("no route, notice {:?}", resp.notice)),
                Some(Err(e)) => format!("error {e:#}"),
                None => String::new(),
            };
            println!("{name} {med:8.1} ms (max {max:.1})  {summary}");
        }
        let tspec = TimeSpec { kind: TimeKind::Depart, iso: Some(at(8)) };
        let (med, max) = time(iters, || {
            let _ = t.isochrone([34.7925, 32.0745], &tspec, &[15, 30, 45], false);
        });
        let bands = t.isochrone([34.7925, 32.0745], &tspec, &[15, 30, 45], false)?;
        println!("isochrone TA 15/30/45   {med:8.1} ms (max {max:.1})  rings {:?}", bands.iter().map(|b| b.rings.len()).collect::<Vec<_>>());
        let (med, max) = time(iters, || {
            let _ = t.stops_in_bbox([34.74, 32.04, 34.84, 32.11], 2000);
        });
        println!("stops layer (Tel Aviv)  {med:8.3} ms (max {max:.3})  {} stops", t.stops_in_bbox([34.74, 32.04, 34.84, 32.11], 2000).len());
        Ok(())
    }

    fn ms(t: Instant) -> f64 {
        t.elapsed().as_secs_f64() * 1000.0
    }

    /// (median, max) in ms of `iters` runs of `f`.
    fn time<F: FnMut()>(iters: usize, mut f: F) -> (f64, f64) {
        let mut v: Vec<f64> = (0..iters)
            .map(|_| {
                let t = Instant::now();
                f();
                ms(t)
            })
            .collect();
        v.sort_by(|a, b| a.total_cmp(b));
        (v[v.len() / 2], v[v.len() - 1])
    }

    /// Working set of this process in MB (Windows only; 0 elsewhere).
    #[cfg(windows)]
    pub fn rss_mb() -> f64 {
        #[repr(C)]
        #[derive(Default)]
        struct Pmc {
            cb: u32,
            page_fault_count: u32,
            peak_working_set: usize,
            working_set: usize,
            qp_peak_paged: usize,
            qp_paged: usize,
            qp_peak_nonpaged: usize,
            qp_nonpaged: usize,
            pagefile: usize,
            peak_pagefile: usize,
        }
        extern "system" {
            fn GetCurrentProcess() -> isize;
            fn K32GetProcessMemoryInfo(h: isize, pmc: *mut Pmc, cb: u32) -> i32;
        }
        let mut p = Pmc { cb: std::mem::size_of::<Pmc>() as u32, ..Default::default() };
        // SAFETY: plain Win32 call with a correctly sized, initialised out-struct.
        let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut p, p.cb) };
        if ok != 0 {
            p.working_set as f64 / 1e6
        } else {
            0.0
        }
    }
    #[cfg(not(windows))]
    pub fn rss_mb() -> f64 {
        0.0
    }

    pub fn run(paths: &DataPaths, iters: usize) -> anyhow::Result<()> {
        println!("rss at start: {:.0} MB", rss_mb());
        // ---- geocoder
        let t = Instant::now();
        let geo = mapsoffline_lib::geocode::open(&paths.geocode_file())?;
        println!("geocoder open: {:.1} ms", ms(t));
        let t = Instant::now();
        let _ = geo.search("הרצל 12 תל אביב", None, 10);
        println!("first search (cold): {:.1} ms", ms(t));
        let queries = [
            "הרצל 12 תל אביב",
            "יפו 1 ירושלים",
            "חיפה",
            "דיזנגוף",
            "קניון עזריאלי",
            "באר",
            "רוטשילד 99 תל אביב",
            "רמת גן",
            "ב",
            "32.0853, 34.7818",
        ];
        let mut all = Vec::new();
        for q in queries {
            let (med, max) = time(iters, || {
                let _ = geo.search(q, None, 10);
            });
            let n = geo.search(q, None, 10)?.len();
            println!("  search {:<24} median {:6.2} ms  max {:6.2} ms  ({n} results)", q, med, max);
            all.push(med);
        }
        all.sort_by(|a, b| a.total_cmp(b));
        println!("search median over queries: {:.2} ms, worst {:.2} ms", all[all.len() / 2], all[all.len() - 1]);
        let (med, _) = time(iters, || {
            let _ = geo.reverse(34.7818, 32.0853);
        });
        println!("reverse: {:.2} ms", med);
        println!("rss after geocoder: {:.0} MB", rss_mb());

        // ---- graph
        let t = Instant::now();
        let g = mapsoffline_lib::routing::load_graph(&paths.graph_file())?;
        println!("graph load: {:.0} ms ({} vertices, {} edges, max car speed {} km/h)", ms(t), g.num_vertices(), g.num_edges(), g.max_car_kmh);
        println!("rss after graph: {:.0} MB", rss_mb());
        let tlv = [34.7818, 32.0853];
        let jlm = [35.2137, 31.7683];
        let haifa = [34.9896, 32.7940];
        let eilat = [34.9519, 29.5577];
        let ks = [35.5695, 33.2075];
        let cases: [(&str, [f64; 2], [f64; 2], Mode, u32); 8] = [
            ("car TLV-JLM alt3", tlv, jlm, Mode::Car, 3),
            ("car TLV-JLM alt1", tlv, jlm, Mode::Car, 1),
            ("car Eilat-KS alt3", eilat, ks, Mode::Car, 3),
            ("car TLV-Haifa alt3", tlv, haifa, Mode::Car, 3),
            ("car urban TLV", tlv, [34.7700, 32.0700], Mode::Car, 3),
            ("foot urban TLV", tlv, [34.7700, 32.0700], Mode::Foot, 3),
            ("bike TLV", tlv, [34.8100, 32.1100], Mode::Bike, 3),
            ("foot JLM 3km", jlm, [35.2300, 31.7800], Mode::Foot, 1),
        ];
        for (name, a, b, mode, alts) in cases {
            let r = req(a, b, mode, alts);
            let _ = g.route(&r); // warm-up
            let (med, max) = time(iters, || {
                let _ = g.route(&r);
            });
            let resp = g.route(&r)?;
            let summary: Vec<String> = resp.routes.iter().map(|x| format!("{:.1}km/{:.0}min", x.distance_m / 1000.0, x.duration_sec / 60.0)).collect();
            println!("  route {:<20} median {:7.2} ms  max {:7.2} ms  {}", name, med, max, summary.join(" | "));
        }
        let (med, max) = time(iters * 10, || {
            let _ = g.walk([34.7818, 32.0853], [34.7850, 32.0870]);
        });
        println!("  walk_leg 350m          median {:7.2} ms  max {:7.2} ms", med, max);
        println!("rss end: {:.0} MB", rss_mb());
        Ok(())
    }
}

/// `export` (Some(components), empty = all) / `import` (None) subcommands: one .mapsdata package.
fn package_cmd(paths: &DataPaths, file: &str, export_only: Option<&[DataComponent]>) -> anyhow::Result<()> {
    let t0 = std::time::Instant::now();
    let last = std::sync::Mutex::new(std::time::Instant::now());
    let progress = |c: DataComponent, phase: ProgressPhase, done: u64, total: u64, msg: &str| {
        let mut l = last.lock().unwrap();
        if l.elapsed().as_millis() > 500 || total == 0 || done == total {
            *l = std::time::Instant::now();
            eprintln!("[{c:?}] {phase:?} {}/{} MB {msg}", done >> 20, total >> 20);
        }
    };
    let cancel = Cancel::default();
    let p = std::path::Path::new(file);
    let is_export = export_only.is_some();
    let comps = match export_only {
        Some(only) => data::package::export(paths, p, only, &progress, &cancel)?,
        None => data::package::import(paths, p, &[], &progress, &cancel)?,
    };
    let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    println!("{} {:?} ({} MB) in {:?}", if is_export { "exported" } else { "imported" }, comps, size >> 20, t0.elapsed());
    Ok(())
}
