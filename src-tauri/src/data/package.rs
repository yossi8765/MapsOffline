//! Offline data package: one `.mapsdata` file (a stored zip) holding every derived data file, so
//! a machine without internet can receive the full data set from a USB stick and load it at once.
//!
//! Layout inside the archive mirrors the data directory (`tiles/israel.pmtiles`, `routing/graph.bin`,
//! `geocode/index.sqlite`, `transit/transit.bin`, the tiles sidecar and `manifest.json`) plus a
//! `package.json` describing the package. Raw downloads (PBF, GTFS, the pmtiles CLI) are not
//! included: they are only needed to *build* data, which the offline machine never does.
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use super::{manifest::Manifest, Cancel};
use crate::paths::{atomic_replace, DataPaths};
use crate::types::{DataComponent, ProgressPhase, SatelliteArea};

pub const PACKAGE_EXT: &str = "mapsdata";
/// One file of a package may not unpack to more than this (the largest real one, govmap
/// imagery at the highest detail, is ~40 GB); together with the free-space check it stops a
/// crafted archive ("zip bomb") from filling the disk.
const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024 * 1024;

const PACKAGE_FORMAT: u32 = 1;
const CHUNK: usize = 4 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
pub struct PackageInfo {
    pub format: u32,
    pub app_version: String,
    pub created_iso: String,
    pub components: Vec<DataComponent>,
    pub graph_version: u32,
    pub transit_version: u32,
    /// Other countries packed under `regions/<folder>/` (ids as in their region.json).
    /// Older program versions ignore these fields and skip the entries.
    #[serde(default)]
    pub regions: Vec<String>,
    /// Sharp-imagery areas packed as `satellite/areas/<id>.pmtiles`, with their definitions
    /// (an import adds them to Settings).
    #[serde(default)]
    pub areas: Vec<SatelliteArea>,
}

/// What a package carries besides the components: other countries and sharp-imagery areas.
#[derive(Debug, Clone, Default)]
pub struct Extras {
    /// region ids (region.json `id`, e.g. "africa/burundi")
    pub regions: Vec<String>,
    pub areas: Vec<SatelliteArea>,
}

/// Files of a country folder a package carries (and an import accepts).
const REGION_FILES: [&str; 6] = ["region.json", "region.geojson", "map.pmtiles", "map.pmtiles.meta.json", "geocode.sqlite", "graph.bin"];

/// Folder name of a region inside `regions/` (the same sanitising as `DataPaths::region_dir`).
fn region_folder(paths: &DataPaths, id: &str) -> String {
    paths.region_dir(id).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The (archive name, file) pairs of the countries and areas in `extras` that exist on disk,
/// and the extras actually found.
fn extra_files(paths: &DataPaths, extras: &Extras) -> (Vec<(String, PathBuf)>, Extras) {
    let mut files = Vec::new();
    let mut found = Extras::default();
    for id in &extras.regions {
        if !super::regions::valid_region_id(id) {
            continue;
        }
        let dir = paths.region_dir(id);
        if !dir.join("region.json").exists() {
            continue;
        }
        let folder = region_folder(paths, id);
        for name in REGION_FILES {
            let f = dir.join(name);
            if f.exists() {
                files.push((format!("regions/{folder}/{name}"), f));
            }
        }
        found.regions.push(id.clone());
    }
    for a in &extras.areas {
        if !super::sat_areas::valid_id(&a.id) {
            continue;
        }
        let f = super::sat_areas::file(paths, &a.id);
        if f.exists() {
            files.push((format!("satellite/areas/{}.pmtiles", a.id), f));
            found.areas.push(a.clone());
        }
    }
    (files, found)
}

/// Files that make up one component, as paths relative to the data root.
fn component_files(paths: &DataPaths, c: DataComponent) -> Vec<PathBuf> {
    let mut v = vec![paths.component_file(c)];
    if matches!(c, DataComponent::Tiles | DataComponent::World) {
        v.push(super::download::meta_path(&paths.component_file(c)));
    }
    v
}

fn rel(paths: &DataPaths, p: &Path) -> anyhow::Result<String> {
    let r = p.strip_prefix(&paths.root).with_context(|| format!("{} is outside the data dir", p.display()))?;
    Ok(r.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"))
}

/// Files a package may install for `c` (its current files plus the former terrain archive name).
fn allowed_files(paths: &DataPaths, c: DataComponent) -> Vec<PathBuf> {
    let mut v = component_files(paths, c);
    v.push(super::download::meta_path(&paths.component_file(c)));
    if c == DataComponent::Terrain {
        v.push(paths.legacy_terrain_file());
    }
    v
}

/// Which component an archive entry belongs to; `None` for files we do not accept.
fn component_of(name: &str) -> Option<DataComponent> {
    super::components::by_dir(name.split('/').next()?)
}

fn copy_with_progress<R: Read, W: Write>(
    mut r: R,
    mut w: W,
    cancel: &Cancel,
    mut on_chunk: impl FnMut(u64),
) -> anyhow::Result<u64> {
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        if cancel.is_cancelled() {
            bail!("cancelled");
        }
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        w.write_all(&buf[..n])?;
        total += n as u64;
        on_chunk(total);
    }
    Ok(total)
}

/// Deflate helps the binary components (graph ~50 %, address index ~45 %, transit ~35 %); the
/// PMTiles archives are already compressed inside (vector tiles gzip, imagery JPEG) and go in stored.
fn compression_for(c: DataComponent) -> CompressionMethod {
    if super::components::spec(c).deflate {
        CompressionMethod::Deflated
    } else {
        CompressionMethod::Stored
    }
}

/// Write the installed components (all of them when `only` is empty) into `dest`.
/// Returns the components that were packed.
pub fn export(
    paths: &DataPaths,
    dest: &Path,
    only: &[DataComponent],
    progress: &(dyn Fn(DataComponent, ProgressPhase, u64, u64, &str) + Sync),
    cancel: &Cancel,
) -> anyhow::Result<Vec<DataComponent>> {
    let want = if only.is_empty() { DataComponent::ALL.to_vec() } else { only.to_vec() };
    export_with(paths, dest, &want, &Extras::default(), progress, cancel)
}

/// Pack the installed components of `want` (an explicit list: empty = none) plus the other
/// countries and sharp-imagery areas of `extras` that are on disk.
pub fn export_with(
    paths: &DataPaths,
    dest: &Path,
    want: &[DataComponent],
    extras: &Extras,
    progress: &(dyn Fn(DataComponent, ProgressPhase, u64, u64, &str) + Sync),
    cancel: &Cancel,
) -> anyhow::Result<Vec<DataComponent>> {
    let present: Vec<DataComponent> = DataComponent::ALL
        .iter()
        .copied()
        .filter(|c| want.contains(c))
        .filter(|&c| paths.component_file(c).exists())
        .collect();
    let (extra, found) = extra_files(paths, extras);
    if present.is_empty() && extra.is_empty() {
        bail!("אין נתונים מותקנים לייצוא");
    }
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p)?;
        let bytes: u64 = present
            .iter()
            .flat_map(|&c| component_files(paths, c))
            .chain(extra.iter().map(|(_, f)| f.clone()))
            .filter_map(|f| std::fs::metadata(f).ok())
            .map(|m| m.len())
            .sum();
        super::disk::ensure_free(p, bytes)?;
    }
    let tmp = dest.with_extension(format!("{PACKAGE_EXT}.tmp"));
    let result = (|| -> anyhow::Result<()> {
        let mut zip = ZipWriter::new(io::BufWriter::new(File::create(&tmp)?));
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored).large_file(true);
        // the manifest first: an import records each component's version as soon as its file is in
        if paths.manifest_file().exists() {
            zip.start_file("manifest.json", opts)?;
            zip.write_all(&std::fs::read(paths.manifest_file())?)?;
        }
        for &c in &present {
            let copts = opts.compression_method(compression_for(c));
            let files = component_files(paths, c);
            let total: u64 = files.iter().filter_map(|f| std::fs::metadata(f).ok()).map(|m| m.len()).sum();
            let mut done = 0u64;
            progress(c, ProgressPhase::Processing, 0, total, "אורז לחבילה");
            for f in files {
                if !f.exists() {
                    continue;
                }
                zip.start_file(rel(paths, &f)?, copts)?;
                let base = done;
                done += copy_with_progress(io::BufReader::new(File::open(&f)?), &mut zip, cancel, |n| {
                    progress(c, ProgressPhase::Processing, base + n, total, "אורז לחבילה");
                })?;
            }
            progress(c, ProgressPhase::Done, 1, 1, "נארז");
        }
        // countries and sharp areas: their progress shows on the map / imagery rows
        let total_of = |sat: bool| -> u64 { extra.iter().filter(|(n, _)| n.starts_with("satellite/") == sat).filter_map(|(_, f)| std::fs::metadata(f).ok()).map(|m| m.len()).sum() };
        let (reg_total, sat_total) = (total_of(false), total_of(true));
        let (mut reg_done, mut sat_done) = (0u64, 0u64);
        for (name, f) in &extra {
            let sat = name.starts_with("satellite/");
            let (c, total, msg) = if sat { (DataComponent::Satellite, sat_total, "אורז תצלומים חדים") } else { (DataComponent::Tiles, reg_total, "אורז מדינות נוספות") };
            // the map / imagery archives are compressed inside; indexes and json deflate
            let method = if name.ends_with(".pmtiles") { CompressionMethod::Stored } else { CompressionMethod::Deflated };
            zip.start_file(name.as_str(), opts.compression_method(method))?;
            let base = if sat { sat_done } else { reg_done };
            let n = copy_with_progress(io::BufReader::new(File::open(f)?), &mut zip, cancel, |n| progress(c, ProgressPhase::Processing, base + n, total, msg))?;
            if sat {
                sat_done += n;
            } else {
                reg_done += n;
            }
        }
        if !found.regions.is_empty() && !present.contains(&DataComponent::Tiles) {
            progress(DataComponent::Tiles, ProgressPhase::Done, 1, 1, "נארז");
        }
        if !found.areas.is_empty() && !present.contains(&DataComponent::Satellite) {
            progress(DataComponent::Satellite, ProgressPhase::Done, 1, 1, "נארז");
        }
        let info = PackageInfo {
            format: PACKAGE_FORMAT,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            created_iso: chrono::Local::now().to_rfc3339(),
            components: present.clone(),
            graph_version: crate::routing::graph::VERSION,
            transit_version: crate::transit::model::VERSION,
            regions: found.regions.clone(),
            areas: found.areas.clone(),
        };
        zip.start_file("package.json", opts)?;
        zip.write_all(&serde_json::to_vec_pretty(&info)?)?;
        zip.finish()?.into_inner().map_err(|e| anyhow!("flush: {e}"))?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    atomic_replace(dest, &tmp)?;
    Ok(present)
}

/// Read `package.json` and validate the archive before touching anything.
pub fn inspect(src: &Path) -> anyhow::Result<PackageInfo> {
    let mut zip = ZipArchive::new(File::open(src).with_context(|| format!("open {}", src.display()))?)
        .context("הקובץ אינו חבילת נתונים תקינה של MapsOffline")?;
    let mut s = String::new();
    zip.by_name("package.json").context("הקובץ אינו חבילת נתונים של MapsOffline (חסר package.json)")?.read_to_string(&mut s)?;
    let info: PackageInfo = serde_json::from_str(&s).context("package.json פגום")?;
    if info.format > PACKAGE_FORMAT {
        bail!("החבילה נוצרה בגרסה חדשה יותר של התוכנה ({}); יש לעדכן את התוכנה", info.app_version);
    }
    if !crate::routing::graph::readable_version(info.graph_version) && info.components.contains(&DataComponent::Routing) {
        bail!(
            "נתוני הניווט בחבילה בפורמט {} והתוכנה דורשת פורמט {}; יש ליצור את החבילה מגרסת תוכנה תואמת",
            info.graph_version,
            crate::routing::graph::VERSION
        );
    }
    if info.transit_version != crate::transit::model::VERSION && info.components.contains(&DataComponent::Transit) {
        bail!(
            "נתוני התחבורה הציבורית בחבילה בפורמט {} והתוכנה דורשת פורמט {}",
            info.transit_version,
            crate::transit::model::VERSION
        );
    }
    Ok(info)
}

/// Extract the package into the data directory, replacing existing components atomically.
/// Returns the components that were installed.
pub fn import(
    paths: &DataPaths,
    src: &Path,
    only: &[DataComponent],
    progress: &(dyn Fn(DataComponent, ProgressPhase, u64, u64, &str) + Sync),
    cancel: &Cancel,
) -> anyhow::Result<Vec<DataComponent>> {
    import_with(paths, src, only, progress, cancel).map(|(c, _)| c)
}

/// Where an extra entry (`regions/<folder>/<file>`, `satellite/areas/<id>.pmtiles`) of a package
/// goes, when the package declares that country / area and the file is one it may hold.
fn extra_dest(paths: &DataPaths, info: &PackageInfo, name: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = name.split('/').collect();
    match parts.as_slice() {
        ["regions", folder, file] => {
            let id = info.regions.iter().find(|id| super::regions::valid_region_id(id) && region_folder(paths, id) == *folder)?;
            REGION_FILES.contains(file).then(|| paths.region_dir(id).join(file))
        }
        ["satellite", "areas", file] => {
            let id = file.strip_suffix(".pmtiles")?;
            (super::sat_areas::valid_id(id) && info.areas.iter().any(|a| a.id == id)).then(|| super::sat_areas::file(paths, id))
        }
        _ => None,
    }
}

/// Is this archive entry a country / sharp-area file (handled by [`extra_dest`])?
fn is_extra_entry(name: &str) -> bool {
    name.starts_with("regions/") || name.starts_with("satellite/areas/")
}

/// [`import`] that also installs the countries and sharp-imagery areas the package carries (only
/// when importing everything, `only` empty). Returns the components and the extras installed; the
/// caller adds the areas to Settings.
pub fn import_with(
    paths: &DataPaths,
    src: &Path,
    only: &[DataComponent],
    progress: &(dyn Fn(DataComponent, ProgressPhase, u64, u64, &str) + Sync),
    cancel: &Cancel,
) -> anyhow::Result<(Vec<DataComponent>, Extras)> {
    let info = inspect(src)?;
    paths.ensure_dirs()?;
    let mut zip = ZipArchive::new(File::open(src)?)?;
    let with_extras = only.is_empty();
    // bytes per component, for progress (countries count on the map row, areas on the imagery row)
    let mut totals: [u64; DataComponent::ALL.len()] = [0; DataComponent::ALL.len()];
    for i in 0..zip.len() {
        let e = zip.by_index(i)?;
        if is_extra_entry(e.name()) {
            if with_extras && extra_dest(paths, &info, e.name()).is_some() {
                let c = if e.name().starts_with("satellite/") { DataComponent::Satellite } else { DataComponent::Tiles };
                totals[c as usize] += e.size();
            }
        } else if let Some(c) = component_of(e.name()) {
            totals[c as usize] += e.size();
        }
    }
    let wanted_bytes: u64 = DataComponent::ALL.iter().filter(|c| only.is_empty() || only.contains(c)).map(|&c| totals[c as usize]).sum::<u64>();
    super::disk::ensure_free(&paths.root, wanted_bytes)?;
    let mut extras_done = Extras::default();
    let mut done: [u64; DataComponent::ALL.len()] = [0; DataComponent::ALL.len()];
    let mut installed: Vec<DataComponent> = Vec::new();
    // versions of the packed components (read up front: older packages store it last)
    let pkg_manifest: Option<Manifest> = zip.by_name("manifest.json").ok().and_then(|mut f| {
        let mut s = String::new();
        f.read_to_string(&mut s).ok()?;
        serde_json::from_str(&s).ok()
    });
    for i in 0..zip.len() {
        if cancel.is_cancelled() {
            bail!("cancelled");
        }
        let mut e = zip.by_index(i)?;
        let name = e.name().to_string();
        if e.is_dir() {
            continue;
        }
        if name == "manifest.json" || name == "package.json" {
            continue;
        }
        if is_extra_entry(&name) {
            if !with_extras {
                continue;
            }
            let Some(dest) = extra_dest(paths, &info, &name) else {
                log::warn!("package entry {name} is not a declared country / area file; skipped");
                continue;
            };
            let c = if name.starts_with("satellite/") { DataComponent::Satellite } else { DataComponent::Tiles };
            let msg = if c == DataComponent::Satellite { "מתקין תצלומים חדים" } else { "מתקין מדינות נוספות" };
            let (total, base) = (totals[c as usize], done[c as usize]);
            let n = extract_entry(&mut e, &name, &dest, cancel, |n| progress(c, ProgressPhase::Processing, base + n, total, msg))?;
            done[c as usize] += n;
            if c == DataComponent::Satellite {
                if let Some(a) = info.areas.iter().find(|a| dest == super::sat_areas::file(paths, &a.id)) {
                    if !extras_done.areas.contains(a) {
                        extras_done.areas.push(a.clone());
                    }
                }
            } else if let Some(id) = info.regions.iter().find(|id| dest.parent() == Some(paths.region_dir(id).as_path())) {
                if !extras_done.regions.contains(id) {
                    extras_done.regions.push(id.clone());
                }
            }
            continue;
        }
        let Some(c) = component_of(&name) else { continue };
        if !only.is_empty() && !only.contains(&c) {
            continue;
        }
        // Only plain relative paths inside the known folders.
        let relp = e.enclosed_name().ok_or_else(|| anyhow!("שם קובץ לא חוקי בחבילה: {name}"))?;
        if relp.components().any(|p| !matches!(p, Component::Normal(_))) || relp.components().count() != 2 {
            bail!("שם קובץ לא חוקי בחבילה: {name}");
        }
        let dest = paths.root.join(&relp);
        // ... and only the very files this component consists of (no `geocode/other.sqlite`)
        if !allowed_files(paths, c).contains(&dest) {
            log::warn!("package entry {name} is not a file of {c:?}; skipped");
            continue;
        }
        if e.size() > MAX_ENTRY_BYTES {
            bail!("UPDATE_FILE_INVALID: the package entry {name} is implausibly large");
        }
        let declared = e.size();
        let tmp = dest.with_extension("import.tmp");
        if let Some(p) = dest.parent() {
            std::fs::create_dir_all(p)?;
        }
        let total = totals[c as usize];
        let base = done[c as usize];
        progress(c, ProgressPhase::Processing, base, total, "מתקין מהחבילה");
        {
            let mut out = io::BufWriter::new(File::create(&tmp)?);
            // never more than the entry declares (the free-space check trusted that size)
            let r = copy_with_progress((&mut e).take(declared + 1), &mut out, cancel, |n| {
                progress(c, ProgressPhase::Processing, base + n, total, "מתקין מהחבילה");
            });
            out.flush()?;
            drop(out);
            match r {
                Ok(n) if n > declared => {
                    let _ = std::fs::remove_file(&tmp);
                    bail!("UPDATE_FILE_INVALID: the package entry {name} is larger than it declares");
                }
                Ok(n) => done[c as usize] += n,
                Err(err) => {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(err);
                }
            }
        }
        if c == DataComponent::Geocode && dest == paths.geocode_file() {
            // the app may be searching this index: close its connections before the swap
            crate::geocode::replace_index(&dest, &tmp)?;
        } else {
            atomic_replace(&dest, &tmp)?;
        }
        // Record the version as soon as the file is in place: a cancel or failure later in the
        // package must not leave a new file described by the old version (manifest.json comes
        // first in exported packages; a package without it records "unknown").
        let v = pkg_manifest.as_ref().and_then(|pm| pm.entry(c)).and_then(|e| e.version.clone());
        Manifest::record(paths, c, v)?;
        if !installed.contains(&c) {
            installed.push(c);
        }
    }
    for &c in &installed {
        progress(c, ProgressPhase::Done, 1, 1, "הותקן מהחבילה");
    }
    if !extras_done.regions.is_empty() && !installed.contains(&DataComponent::Tiles) {
        progress(DataComponent::Tiles, ProgressPhase::Done, 1, 1, "הותקן מהחבילה");
    }
    if !extras_done.areas.is_empty() && !installed.contains(&DataComponent::Satellite) {
        progress(DataComponent::Satellite, ProgressPhase::Done, 1, 1, "הותקן מהחבילה");
    }
    log::info!(
        "imported data package {} ({} components, {} countries, {} sharp areas, app {})",
        src.display(),
        installed.len(),
        extras_done.regions.len(),
        extras_done.areas.len(),
        info.app_version
    );
    Ok((installed, extras_done))
}

/// Unpack one entry to `dest` through a temporary file (never more than it declares), then swap
/// it in. Returns the bytes written.
fn extract_entry<R: Read>(e: &mut zip::read::ZipFile<'_, R>, name: &str, dest: &Path, cancel: &Cancel, on_chunk: impl FnMut(u64)) -> anyhow::Result<u64> {
    if e.size() > MAX_ENTRY_BYTES {
        bail!("UPDATE_FILE_INVALID: the package entry {name} is implausibly large");
    }
    let declared = e.size();
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p)?;
    }
    let tmp = dest.with_extension("import.tmp");
    let mut out = io::BufWriter::new(File::create(&tmp)?);
    let r = copy_with_progress(e.take(declared + 1), &mut out, cancel, on_chunk).and_then(|n| {
        out.flush()?;
        Ok(n)
    });
    drop(out);
    match r {
        Ok(n) if n > declared => {
            let _ = std::fs::remove_file(&tmp);
            bail!("UPDATE_FILE_INVALID: the package entry {name} is larger than it declares")
        }
        Ok(n) => {
            atomic_replace(dest, &tmp)?;
            Ok(n)
        }
        Err(err) => {
            let _ = std::fs::remove_file(&tmp);
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_package() {
        let dir = std::env::temp_dir().join(format!("mapsoffline-pkg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = DataPaths::new(dir.join("src"));
        src.ensure_dirs().unwrap();
        std::fs::write(src.graph_file(), b"graph-bytes").unwrap();
        std::fs::write(src.transit_file(), vec![7u8; 5 * 1024 * 1024]).unwrap();
        let mut m = Manifest::load(&src);
        m.set(DataComponent::Routing, Some("v-route".into()));
        m.save(&src).unwrap();
        let pkg = dir.join("test.mapsdata");
        let noop = |_: DataComponent, _: ProgressPhase, _: u64, _: u64, _: &str| {};
        let cancel = Cancel::default();
        let packed = export(&src, &pkg, &[], &noop, &cancel).unwrap();
        assert_eq!(packed, vec![DataComponent::Routing, DataComponent::Transit]);
        let info = inspect(&pkg).unwrap();
        assert_eq!(info.components.len(), 2);
        // the 5 MB run of identical bytes must have been deflated
        assert!(std::fs::metadata(&pkg).unwrap().len() < 1024 * 1024);
        let one = dir.join("one.mapsdata");
        assert_eq!(export(&src, &one, &[DataComponent::Routing], &noop, &cancel).unwrap(), vec![DataComponent::Routing]);
        assert_eq!(inspect(&one).unwrap().components, vec![DataComponent::Routing]);

        let dst = DataPaths::new(dir.join("dst"));
        let only_route = DataPaths::new(dir.join("dst-one"));
        assert_eq!(import(&only_route, &pkg, &[DataComponent::Routing], &noop, &cancel).unwrap(), vec![DataComponent::Routing]);
        assert!(!only_route.transit_file().exists());
        let got = import(&dst, &pkg, &[], &noop, &cancel).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(std::fs::read(dst.graph_file()).unwrap(), b"graph-bytes");
        assert_eq!(std::fs::metadata(dst.transit_file()).unwrap().len(), 5 * 1024 * 1024);
        let m2 = Manifest::load(&dst);
        assert_eq!(m2.entry(DataComponent::Routing).and_then(|e| e.version.clone()).as_deref(), Some("v-route"));
        assert!(!dst.tiles_file().exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn roundtrip_countries_and_areas() {
        let dir = std::env::temp_dir().join(format!("mapsoffline-pkg-x-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = DataPaths::new(dir.join("src"));
        src.ensure_dirs().unwrap();
        let id = "africa/burundi";
        let rd = src.region_dir(id);
        std::fs::create_dir_all(&rd).unwrap();
        std::fs::write(rd.join("region.json"), br#"{"id":"africa/burundi"}"#).unwrap();
        std::fs::write(rd.join("map.pmtiles"), b"map").unwrap();
        std::fs::write(rd.join("geocode.sqlite"), b"idx").unwrap();
        std::fs::write(rd.join("stray.txt"), b"not packed").unwrap();
        let area = SatelliteArea { id: "jerusalem".into(), name: "ירושלים".into(), bbox: [35.2, 31.76, 35.24, 31.79], zoom: 17 };
        std::fs::create_dir_all(super::super::sat_areas::dir(&src)).unwrap();
        std::fs::write(super::super::sat_areas::file(&src, &area.id), b"sharp").unwrap();
        // an area without a file is left out
        let missing = SatelliteArea { id: "nowhere".into(), ..area.clone() };
        let extras = Extras { regions: vec![id.into(), "europe/absent".into()], areas: vec![area.clone(), missing] };
        let noop = |_: DataComponent, _: ProgressPhase, _: u64, _: u64, _: &str| {};
        let cancel = Cancel::default();
        let pkg = dir.join("x.mapsdata");
        // no component asked for: the package holds the country and the area only
        assert!(export_with(&src, &pkg, &[], &extras, &noop, &cancel).unwrap().is_empty());
        let info = inspect(&pkg).unwrap();
        assert_eq!(info.regions, vec![id.to_string()]);
        assert_eq!(info.areas, vec![area.clone()]);

        let dst = DataPaths::new(dir.join("dst"));
        let (comps, got) = import_with(&dst, &pkg, &[], &noop, &cancel).unwrap();
        assert!(comps.is_empty());
        assert_eq!(got.regions, vec![id.to_string()]);
        assert_eq!(got.areas, vec![area.clone()]);
        let drd = dst.region_dir(id);
        assert_eq!(std::fs::read(drd.join("map.pmtiles")).unwrap(), b"map");
        assert_eq!(std::fs::read(drd.join("geocode.sqlite")).unwrap(), b"idx");
        assert!(!drd.join("stray.txt").exists());
        assert_eq!(std::fs::read(super::super::sat_areas::file(&dst, &area.id)).unwrap(), b"sharp");
        // importing one component only leaves the extras alone
        let one = DataPaths::new(dir.join("one"));
        let (_, none) = import_with(&one, &pkg, &[DataComponent::Routing], &noop, &cancel).unwrap();
        assert!(none.regions.is_empty() && none.areas.is_empty());
        assert!(!one.region_dir(id).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
