//! Low-level OBF reader: protobuf varints, OsmAnd's wire type 6, the top-level section table.
//!
//! OBF is protobuf with two OsmAnd twists:
//!   - wire type 6 = a length-delimited field whose length is a 4-byte BIG-endian integer (top
//!     bit set: 8-byte length with that bit cleared), so writers can patch lengths in place;
//!   - some "shift" fields (offsets to data blocks) are 4-byte big-endian integers, written
//!     either as wire type 5 or 6. Decoders name those fields in `shifts` so they come out as
//!     [`Val::Shift`] instead of a length-delimited body.
//!
//! The file is memory-mapped; every [`Span`] is an absolute byte range in it.
//!
//! `.obf.zip` (as served by download.osmand.net): [`ObfFile::open_auto`] extracts the single
//! `.obf` entry into a work directory and deletes the extracted copy again when the
//! [`ObfFile`] is dropped. [`ObfFile::open`] takes an already extracted `.obf`.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, bail, Context, Result};

// ---------------------------------------------------------------- primitives

pub fn varint(buf: &[u8], pos: &mut usize) -> Result<u64> {
    let mut r = 0u64;
    let mut shift = 0u32;
    loop {
        let Some(&b) = buf.get(*pos) else { bail!("OBF: truncated varint at {}", *pos) };
        *pos += 1;
        if shift < 64 {
            r |= u64::from(b & 0x7f) << shift;
        }
        if b < 0x80 {
            return Ok(r);
        }
        shift += 7;
        if shift > 70 {
            bail!("OBF: varint too long at {}", *pos);
        }
    }
}

pub fn zigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

pub fn be_u32(buf: &[u8], pos: usize) -> Result<u32> {
    let b = buf.get(pos..pos + 4).ok_or_else(|| anyhow!("OBF: truncated fixed32 at {pos}"))?;
    Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// Length of a wire-type-6 field at `pos`: 4 bytes big-endian, or 8 when the top bit is set.
/// Returns (length, header size).
pub fn wire6_len(buf: &[u8], pos: usize) -> Result<(u64, usize)> {
    let l = be_u32(buf, pos)?;
    if l & 0x8000_0000 == 0 {
        return Ok((u64::from(l), 4));
    }
    let b = buf.get(pos..pos + 8).ok_or_else(|| anyhow!("OBF: truncated 8-byte length at {pos}"))?;
    let v = u64::from_be_bytes(b.try_into().expect("8 bytes")) & 0x7fff_ffff_ffff_ffff;
    Ok((v, 8))
}

/// Absolute byte range of a message body; `head` is where its length prefix starts (some
/// OsmAnd offsets are measured from there).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub head: usize,
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn len(&self) -> usize {
        self.end - self.start
    }
    pub fn is_empty(&self) -> bool {
        self.end == self.start
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Val {
    Int(u64),
    Fixed64(u64),
    /// little-endian fixed32 (standard protobuf)
    Fixed32(u32),
    Bytes(Span),
    /// big-endian 4-byte value of a field listed in `shifts`
    Shift(u32),
}

impl Val {
    pub fn int(&self) -> u64 {
        match *self {
            Val::Int(v) | Val::Fixed64(v) => v,
            Val::Fixed32(v) | Val::Shift(v) => u64::from(v),
            Val::Bytes(_) => 0,
        }
    }
    pub fn sint(&self) -> i64 {
        zigzag(self.int())
    }
    pub fn span(&self) -> Option<Span> {
        match *self {
            Val::Bytes(s) => Some(s),
            _ => None,
        }
    }
}

/// Fields of one message body.
pub struct Fields<'a> {
    buf: &'a [u8],
    pos: usize,
    end: usize,
    shifts: &'static [u32],
}

impl<'a> Fields<'a> {
    pub fn new(buf: &'a [u8], span: Span, shifts: &'static [u32]) -> Self {
        Self { buf, pos: span.start, end: span.end.min(buf.len()), shifts }
    }

    fn read(&mut self) -> Result<(u32, Val)> {
        let tag = varint(self.buf, &mut self.pos)?;
        let field = (tag >> 3) as u32;
        let wire = tag & 7;
        let shift = self.shifts.contains(&field);
        let v = match wire {
            0 => Val::Int(varint(self.buf, &mut self.pos)?),
            1 => {
                let b = self.buf.get(self.pos..self.pos + 8).ok_or_else(|| anyhow!("OBF: truncated fixed64"))?;
                self.pos += 8;
                Val::Fixed64(u64::from_le_bytes(b.try_into().expect("8 bytes")))
            }
            5 | 6 if shift => {
                let v = be_u32(self.buf, self.pos)?;
                self.pos += 4;
                Val::Shift(v)
            }
            5 => {
                let b = self.buf.get(self.pos..self.pos + 4).ok_or_else(|| anyhow!("OBF: truncated fixed32"))?;
                self.pos += 4;
                Val::Fixed32(u32::from_le_bytes(b.try_into().expect("4 bytes")))
            }
            2 => {
                let head = self.pos;
                let l = varint(self.buf, &mut self.pos)? as usize;
                let start = self.pos;
                let end = start.checked_add(l).filter(|e| *e <= self.end).ok_or_else(|| anyhow!("OBF: field {field} overruns its message at {start}"))?;
                self.pos = end;
                Val::Bytes(Span { head, start, end })
            }
            6 => {
                let head = self.pos;
                let (l, hs) = wire6_len(self.buf, self.pos)?;
                let start = self.pos + hs;
                let end = usize::try_from(l).ok().and_then(|l| start.checked_add(l)).filter(|e| *e <= self.end).ok_or_else(|| anyhow!("OBF: field {field} overruns its message at {start}"))?;
                self.pos = end;
                Val::Bytes(Span { head, start, end })
            }
            w => bail!("OBF: unknown wire type {w} (field {field}) at {}", self.pos),
        };
        Ok((field, v))
    }
}

impl Iterator for Fields<'_> {
    type Item = Result<(u32, Val)>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.end {
            return None;
        }
        let r = self.read();
        if r.is_err() {
            self.pos = self.end;
        }
        Some(r)
    }
}

/// The message whose varint length prefix starts at `pos` (blocks reached through a shift).
pub fn message_at(buf: &[u8], pos: usize) -> Result<Span> {
    let mut p = pos;
    let l = varint(buf, &mut p)? as usize;
    let end = p.checked_add(l).filter(|e| *e <= buf.len()).ok_or_else(|| anyhow!("OBF: block at {pos} overruns the file"))?;
    Ok(Span { head: pos, start: p, end })
}

/// Packed varints of a byte range.
pub fn packed(buf: &[u8], span: Span) -> Result<Vec<u64>> {
    let mut out = Vec::new();
    let mut p = span.start;
    while p < span.end {
        out.push(varint(buf, &mut p)?);
    }
    Ok(out)
}

pub fn string(buf: &[u8], span: Span) -> String {
    String::from_utf8_lossy(&buf[span.start..span.end]).into_owned()
}

/// `StringTable` message: field 1 strings.
pub fn string_table(buf: &[u8], span: Span) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for f in Fields::new(buf, span, &[]) {
        if let (1, Val::Bytes(s)) = f? {
            out.push(string(buf, s));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- coordinates

/// 31-zoom x → longitude.
pub fn lon31(x: u32) -> f64 {
    f64::from(x) / 2f64.powi(31) * 360.0 - 180.0
}
/// 31-zoom y (Web Mercator) → latitude.
pub fn lat31(y: u32) -> f64 {
    let n = std::f64::consts::PI * (1.0 - 2.0 * f64::from(y) / 2f64.powi(31));
    n.sinh().atan().to_degrees()
}
pub fn x31(lon: f64) -> u32 {
    (((lon + 180.0) / 360.0) * 2f64.powi(31)).clamp(0.0, 2_147_483_647.0) as u32
}
pub fn y31(lat: f64) -> u32 {
    let l = lat.clamp(-85.051_128, 85.051_128).to_radians();
    let y = (1.0 - (l.tan() + 1.0 / l.cos()).ln() / std::f64::consts::PI) / 2.0;
    (y * 2f64.powi(31)).clamp(0.0, 2_147_483_647.0) as u32
}
/// Clamp a decoded 31-zoom coordinate into range.
pub fn c31(v: i64) -> u32 {
    v.clamp(0, (1i64 << 31) - 1) as u32
}

// ---------------------------------------------------------------- file

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionKind {
    Map,
    Routing,
    Address,
    Poi,
    Transport,
    HhRouting,
    Other(u32),
}

impl SectionKind {
    fn of(field: u32) -> Self {
        match field {
            6 => Self::Map,
            9 => Self::Routing,
            7 => Self::Address,
            8 => Self::Poi,
            4 => Self::Transport,
            10 => Self::HhRouting,
            f => Self::Other(f),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Section {
    pub kind: SectionKind,
    pub span: Span,
    /// the section's `name` field (map: 2, the others: 1); empty when absent
    pub name: String,
}

enum Backing {
    Mmap(memmap2::Mmap),
    Vec(Vec<u8>),
    None,
}

/// An open `.obf` file.
pub struct ObfFile {
    data: Backing,
    pub path: PathBuf,
    /// OBF format version (2 for current files)
    pub version: u32,
    /// creation time, ms since the epoch
    pub date_created: i64,
    pub sections: Vec<Section>,
    /// extracted from a zip by `open_auto`: deleted on drop
    temp: Option<PathBuf>,
    pub(crate) map_cache: OnceLock<std::result::Result<Vec<super::map::MapSection>, String>>,
}

impl Drop for ObfFile {
    fn drop(&mut self) {
        self.data = Backing::None; // unmap before deleting (Windows)
        if let Some(t) = self.temp.take() {
            let _ = std::fs::remove_file(t);
        }
    }
}

impl ObfFile {
    /// Memory-map and index an extracted `.obf` file.
    pub fn open(path: &Path) -> Result<Self> {
        let f = File::open(path).with_context(|| format!("open {}", path.display()))?;
        // SAFETY: the file is opened read-only and not modified while mapped (our own temp
        // copy or a downloaded file nobody else writes).
        let map = unsafe { memmap2::Mmap::map(&f) }.with_context(|| format!("map {}", path.display()))?;
        Self::index(Backing::Mmap(map), path.to_path_buf())
    }

    /// In-memory file (tests).
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        Self::index(Backing::Vec(bytes), PathBuf::new())
    }

    /// `.obf` or `.obf.zip`: a zip is extracted into `work_dir` first (the copy is deleted when
    /// the returned file is dropped).
    pub fn open_auto(path: &Path, work_dir: &Path) -> Result<Self> {
        if !is_zip(path)? {
            return Self::open(path);
        }
        let obf = extract_zip(path, work_dir)?;
        match Self::open(&obf) {
            Ok(mut f) => {
                f.temp = Some(obf);
                Ok(f)
            }
            Err(e) => {
                let _ = std::fs::remove_file(&obf);
                Err(e)
            }
        }
    }

    fn index(data: Backing, path: PathBuf) -> Result<Self> {
        let mut f = Self { data, path, version: 0, date_created: 0, sections: Vec::new(), temp: None, map_cache: OnceLock::new() };
        let buf = f.bytes();
        let whole = Span { head: 0, start: 0, end: buf.len() };
        let (mut version, mut date, mut sections, mut confirm) = (0u32, 0i64, Vec::new(), None);
        for fld in Fields::new(buf, whole, &[]) {
            let (field, v) = fld?;
            match (field, v) {
                (1, Val::Int(x)) => version = x as u32,
                (18, Val::Int(x)) => date = x as i64,
                (32, Val::Int(x)) => confirm = Some(x as u32),
                (field, Val::Bytes(span)) => {
                    let kind = SectionKind::of(field);
                    let name_field = if kind == SectionKind::Map { 2 } else { 1 };
                    let name = match kind {
                        SectionKind::HhRouting | SectionKind::Other(_) => String::new(),
                        _ => first_string(buf, span, name_field).unwrap_or_default(),
                    };
                    sections.push(Section { kind, span, name });
                }
                _ => {}
            }
        }
        if version == 0 && sections.is_empty() {
            bail!("not an OsmAnd .obf file");
        }
        if confirm.is_some_and(|c| c != version) {
            bail!("OBF: damaged file (version {version}, confirmation {confirm:?})");
        }
        f.version = version;
        f.date_created = date;
        f.sections = sections;
        Ok(f)
    }

    pub fn bytes(&self) -> &[u8] {
        match &self.data {
            Backing::Mmap(m) => m,
            Backing::Vec(v) => v,
            Backing::None => &[],
        }
    }

    pub fn sections_of(&self, kind: SectionKind) -> impl Iterator<Item = &Section> {
        self.sections.iter().filter(move |s| s.kind == kind)
    }

    pub fn fields(&self, span: Span, shifts: &'static [u32]) -> Fields<'_> {
        Fields::new(self.bytes(), span, shifts)
    }

    /// Area covered, (left, top, right, bottom) in 31-zoom units: the map levels, else the
    /// routing root boxes, else the POI / address boundaries.
    pub fn bbox31(&self) -> Option<[u32; 4]> {
        let mut acc: Option<[u32; 4]> = None;
        fn add(acc: &mut Option<[u32; 4]>, b: [u32; 4]) {
            if b[0] >= b[2] || b[1] >= b[3] {
                return;
            }
            *acc = Some(match *acc {
                None => b,
                Some(a) => [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])],
            });
        }
        if let Ok(maps) = self.map_sections() {
            for l in maps.iter().flat_map(|m| m.levels.iter()) {
                add(&mut acc, l.level.bbox31);
            }
        }
        if acc.is_none() {
            if let Ok(rs) = super::route::route_sections(self) {
                for r in &rs {
                    if let Ok(Some(b)) = r.bbox31(self) {
                        add(&mut acc, b);
                    }
                }
            }
        }
        if acc.is_none() {
            let buf = self.bytes();
            for s in &self.sections {
                let field = match s.kind {
                    SectionKind::Poi => 2,
                    SectionKind::Address => 3,
                    _ => continue,
                };
                for f in self.fields(s.span, &[]).flatten() {
                    if let (fl, Val::Bytes(b)) = f {
                        if fl == field {
                            if let Some(tb) = tile_box(buf, b) {
                                add(&mut acc, tb);
                            }
                        }
                    }
                }
            }
        }
        acc
    }

    /// [west, south, east, north] in degrees.
    pub fn bbox(&self) -> Option<[f64; 4]> {
        self.bbox31().map(|b| [lon31(b[0]), lat31(b[3]), lon31(b[2]), lat31(b[1])])
    }

    /// The raw name of the data ("Burundi_africa"): map, else any section name, else the file stem.
    pub fn raw_name(&self) -> String {
        self.sections_of(SectionKind::Map)
            .chain(self.sections.iter())
            .map(|s| s.name.clone())
            .find(|n| !n.is_empty())
            .unwrap_or_else(|| self.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())
    }

    /// A readable name: "Burundi_africa" → "Burundi", "Us_california_san-diego_northamerica" →
    /// "Us california san-diego".
    pub fn display_name(&self) -> String {
        display_name(&self.raw_name())
    }
}

pub fn display_name(raw: &str) -> String {
    const CONTINENTS: [&str; 9] = ["africa", "asia", "europe", "northamerica", "southamerica", "centralamerica", "australia-oceania", "oceania", "russia"];
    let mut s = raw.trim().trim_end_matches(".obf").to_string();
    if let Some(x) = s.strip_suffix("_2") {
        s = x.to_string();
    }
    let mut parts: Vec<&str> = s.split('_').filter(|p| !p.is_empty()).collect();
    if parts.len() > 1 && parts.last().is_some_and(|l| CONTINENTS.contains(&l.to_ascii_lowercase().as_str())) {
        parts.pop();
    }
    let joined = parts.join(" ");
    let mut c = joined.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => raw.to_string(),
    }
}

fn first_string(buf: &[u8], span: Span, field: u32) -> Option<String> {
    for f in Fields::new(buf, span, &[]) {
        match f {
            Ok((fl, Val::Bytes(s))) if fl == field => return Some(string(buf, s)),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    None
}

/// `OsmAndTileBox` (left 1, right 2, top 3, bottom 4) → [left, top, right, bottom].
fn tile_box(buf: &[u8], span: Span) -> Option<[u32; 4]> {
    let mut v = [0u32; 4];
    for f in Fields::new(buf, span, &[]) {
        let (fl, val) = f.ok()?;
        match fl {
            1 => v[0] = val.int() as u32,
            2 => v[2] = val.int() as u32,
            3 => v[1] = val.int() as u32,
            4 => v[3] = val.int() as u32,
            _ => {}
        }
    }
    Some(v)
}

// ---------------------------------------------------------------- zip

fn is_zip(path: &Path) -> Result<bool> {
    let mut f = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut magic = [0u8; 4];
    let n = f.read(&mut magic)?;
    Ok(n == 4 && magic == *b"PK\x03\x04")
}

/// Extract the `.obf` entry of an OsmAnd zip into `work_dir` (an existing, complete copy is
/// reused) and return its path.
pub fn extract_zip(zip_path: &Path, work_dir: &Path) -> Result<PathBuf> {
    let f = File::open(zip_path).with_context(|| format!("open {}", zip_path.display()))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(f)).with_context(|| format!("read zip {}", zip_path.display()))?;
    let idx = (0..zip.len())
        .find(|&i| zip.by_index(i).map(|e| e.is_file() && e.name().to_ascii_lowercase().ends_with(".obf")).unwrap_or(false))
        .ok_or_else(|| anyhow!("{} holds no .obf file", zip_path.display()))?;
    let mut entry = zip.by_index(idx)?;
    // only the file name, restricted to safe characters
    let base = entry.name().rsplit(['/', '\\']).next().unwrap_or("map.obf").to_string();
    let safe: String = base.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).collect();
    let safe = if safe.trim_matches('.').is_empty() { "map.obf".to_string() } else { safe };
    std::fs::create_dir_all(work_dir).with_context(|| format!("create {}", work_dir.display()))?;
    let out = work_dir.join(&safe);
    let size = entry.size();
    if std::fs::metadata(&out).is_ok_and(|m| m.len() == size) {
        return Ok(out);
    }
    let part = work_dir.join(format!("{safe}.part"));
    {
        let mut w = std::io::BufWriter::with_capacity(1 << 20, File::create(&part).with_context(|| format!("create {}", part.display()))?);
        let n = std::io::copy(&mut entry, &mut w).context("extract .obf")?;
        w.flush()?;
        if n != size {
            bail!("extracted {n} of {size} bytes");
        }
    }
    std::fs::rename(&part, &out).with_context(|| format!("rename {}", part.display()))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc_varint(mut v: u64) -> Vec<u8> {
        let mut o = Vec::new();
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                o.push(b);
                return o;
            }
            o.push(b | 0x80);
        }
    }

    #[test]
    fn varints_and_zigzag() {
        for v in [0u64, 1, 127, 128, 300, 1 << 35, u64::MAX] {
            let b = enc_varint(v);
            let mut p = 0;
            assert_eq!(varint(&b, &mut p).unwrap(), v);
            assert_eq!(p, b.len());
        }
        let mut p = 0;
        assert!(varint(&[0x80, 0x80], &mut p).is_err());
        assert_eq!(zigzag(0), 0);
        assert_eq!(zigzag(1), -1);
        assert_eq!(zigzag(2), 1);
        assert_eq!(zigzag(3), -2);
        assert_eq!(zigzag(u64::MAX), i64::MIN);
    }

    #[test]
    fn wire6_and_shifts() {
        // field 6 wt 6 (4-byte BE length 3) + field 5 wt 5 as a shift (BE) + field 7 wt 5 (LE)
        let mut b = vec![(6 << 3) | 6, 0, 0, 0, 3, 0x08, 0x96, 0x01];
        b.extend([(5 << 3) | 5, 0, 0, 1, 2]);
        b.extend([(7 << 3) | 5, 2, 1, 0, 0]);
        let all = Span { head: 0, start: 0, end: b.len() };
        let v: Vec<(u32, Val)> = Fields::new(&b, all, &[5]).map(|f| f.unwrap()).collect();
        assert_eq!(v[0], (6, Val::Bytes(Span { head: 1, start: 5, end: 8 })));
        assert_eq!(v[1], (5, Val::Shift(258)));
        assert_eq!(v[2], (7, Val::Fixed32(258)));
        let inner: Vec<(u32, Val)> = Fields::new(&b, v[0].1.span().unwrap(), &[]).map(|f| f.unwrap()).collect();
        assert_eq!(inner, vec![(1, Val::Int(150))]);
        // 8-byte length: top bit set
        let l8 = [0x80u8, 0, 0, 0, 0, 0, 0x01, 0x00];
        assert_eq!(wire6_len(&l8, 0).unwrap(), (256, 8));
        // a body running past its parent is an error, not a panic
        let bad = [(6u8 << 3) | 6, 0, 0, 0, 9, 1];
        assert!(Fields::new(&bad, Span { head: 0, start: 0, end: bad.len() }, &[]).next().unwrap().is_err());
    }

    #[test]
    fn coordinates() {
        let (lon, lat) = (29.3675, -3.36381);
        let (x, y) = (x31(lon), y31(lat));
        assert!((lon31(x) - lon).abs() < 1e-6 && (lat31(y) - lat).abs() < 1e-6);
        // Bujumbura as stored in the Burundi file
        assert!((lon31(1_248_925_801) - 29.3675).abs() < 1e-4);
        assert!((lat31(1_093_819_284) + 3.36381).abs() < 1e-4);
        assert_eq!(x31(-180.0), 0);
        assert!(lat31(1 << 30).abs() < 1e-9);
        assert_eq!(c31(-5), 0);
        assert_eq!(c31(1 << 40), (1 << 31) - 1);
    }

    #[test]
    fn names() {
        assert_eq!(display_name("Burundi_africa"), "Burundi");
        assert_eq!(display_name("Burundi_africa_2.obf"), "Burundi");
        assert_eq!(display_name("Us_california_san-diego_northamerica"), "Us california san-diego");
        assert_eq!(display_name("israel"), "Israel");
    }
}
