//! Incremental updates of the OSM extract.
//!
//! Geofabrik publishes, next to the full Israel extract (~120 MB), one change file per day
//! (`NNN/NNN/NNN.osc.gz`, ~350 KB) plus `state.txt` with the current replication sequence number.
//! The PBF header carries the sequence number the file was cut at, so the local copy knows how
//! many days it is behind. When that is at most `MAX_DIFFS`, the missing days are downloaded,
//! merged (later changes win) and applied by streaming the old PBF into a new one; otherwise the
//! full extract is downloaded as before. Both paths end with a PBF whose header names the
//! sequence number, which is also the version string ("OSM 2026-11-02 #4937") the derived
//! components (routing graph, address index) are tagged with and that `data::check` compares.
//!
//! The writer here produces a plain PBF (dense nodes, ways, relations, zlib blocks, no element
//! metadata) that the two builders read with `osmpbf` like the original.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use flate2::read::GzDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use osmpbf::{BlobDecode, BlobReader, Element, HeaderBBox, RelMemberType};
use quick_xml::events::{BytesStart, Event};
use rayon::prelude::*;
use reqwest::blocking::Client;

use crate::data::download;
use crate::data::{Cancel, PBF_URL};
use crate::types::{ProgressFn, ProgressPhase};

pub const UPDATES_URL: &str = "https://download.geofabrik.de/asia/israel-and-palestine-updates/";
/// More missing days than this and the full extract is the better deal (also bounds memory:
/// the merged changes are held in RAM while the PBF is rewritten).
pub const MAX_DIFFS: i64 = 120;
/// Geofabrik's region diffs are cut from the planet's minutely diffs, so elements that drift in or
/// out of the extract area are not tracked perfectly. Re-download the full extract after this
/// many incremental days so the local copy never strays far from a fresh cut.
pub const FULL_REFRESH_DAYS: i64 = 180;
const ELEMENTS_PER_BLOCK: usize = 8000;
const BLOCKS_PER_FLUSH: usize = 16;
/// Input blobs decoded in parallel per batch.
const DECODE_CHUNK: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplState {
    pub sequence: i64,
    /// Unix seconds of the OSM data the file is current to.
    pub timestamp: i64,
}

impl ReplState {
    /// Version string shared by the full download and the incremental path.
    pub fn version(&self) -> String {
        let date = chrono::DateTime::from_timestamp(self.timestamp, 0).map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_else(|| "?".into());
        format!("OSM {date} #{}", self.sequence)
    }
}

/// Replication state and bounding box from a PBF's header block.
pub fn header_info(pbf: &Path) -> anyhow::Result<(Option<ReplState>, Option<HeaderBBox>)> {
    let mut reader = BlobReader::from_path(pbf).with_context(|| format!("open {}", pbf.display()))?;
    let blob = reader.next().ok_or_else(|| anyhow!("empty PBF"))??;
    match blob.decode()? {
        BlobDecode::OsmHeader(h) => {
            let state = match (h.osmosis_replication_sequence_number(), h.osmosis_replication_timestamp()) {
                (Some(sequence), Some(timestamp)) => Some(ReplState { sequence, timestamp }),
                _ => None,
            };
            Ok((state, h.bbox()))
        }
        _ => bail!("PBF does not start with a header block"),
    }
}

/// Parse Geofabrik's `state.txt` (`sequenceNumber=4937`, `timestamp=2026-11-02T21\:21\:26Z`).
pub fn parse_state(text: &str) -> anyhow::Result<ReplState> {
    let mut sequence = None;
    let mut timestamp = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("sequenceNumber=") {
            sequence = Some(v.trim().parse::<i64>().context("sequenceNumber")?);
        } else if let Some(v) = line.strip_prefix("timestamp=") {
            let iso = v.trim().replace('\\', "");
            timestamp = Some(chrono::DateTime::parse_from_rfc3339(&iso).with_context(|| format!("timestamp {iso}"))?.timestamp());
        }
    }
    match (sequence, timestamp) {
        (Some(sequence), Some(timestamp)) => Ok(ReplState { sequence, timestamp }),
        _ => bail!("state.txt without sequenceNumber/timestamp"),
    }
}

pub fn remote_state(client: &Client) -> anyhow::Result<ReplState> {
    let url = format!("{UPDATES_URL}state.txt");
    let text = fetch(client, &url, &Cancel::default()).context("replication state")?;
    parse_state(&String::from_utf8_lossy(&text))
}

pub fn diff_url(sequence: i64) -> String {
    format!("{UPDATES_URL}{:03}/{:03}/{:03}.osc.gz", sequence / 1_000_000, (sequence / 1000) % 1000, sequence % 1000)
}

fn fetch(client: &Client, url: &str, cancel: &Cancel) -> anyhow::Result<Vec<u8>> {
    let mut last = anyhow!("download failed");
    for attempt in 0..3u32 {
        cancel.wait_if_paused();
        if cancel.is_cancelled() {
            bail!("download cancelled");
        }
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(2 << attempt));
        }
        match client.get(url).send() {
            Ok(r) if r.status().is_success() => match r.bytes() {
                Ok(b) => return Ok(b.to_vec()),
                Err(e) => last = e.into(),
            },
            Ok(r) if r.status().as_u16() == 418 => bail!("{}", download::blocked_message(url)),
            Ok(r) if r.status().is_client_error() => bail!("HTTP {} for {url}", r.status()),
            Ok(r) => last = anyhow!("HTTP {} for {url}", r.status()),
            Err(e) => last = e.into(),
        }
    }
    Err(last.context(format!("fetch {url}")))
}

// ---------------------------------------------------------------- change files

pub struct NodeRec {
    /// decimicro degrees (1e-7)
    pub lat: i32,
    pub lon: i32,
    pub tags: Vec<(String, String)>,
}
pub struct WayRec {
    pub refs: Vec<i64>,
    pub tags: Vec<(String, String)>,
}
pub struct RelRec {
    /// (member type 0 node / 1 way / 2 relation, id, role)
    pub members: Vec<(u8, i64, String)>,
    pub tags: Vec<(String, String)>,
}

/// Net effect of one or more change files: `None` = deleted, `Some` = created or modified.
#[derive(Default)]
pub struct Changes {
    pub nodes: HashMap<i64, Option<NodeRec>>,
    pub ways: HashMap<i64, Option<WayRec>>,
    pub relations: HashMap<i64, Option<RelRec>>,
}

impl Changes {
    pub fn len(&self) -> usize {
        self.nodes.len() + self.ways.len() + self.relations.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

enum Cur {
    Node { id: i64, lat: i32, lon: i32, tags: Vec<(String, String)> },
    Way { id: i64, refs: Vec<i64>, tags: Vec<(String, String)> },
    Rel { id: i64, members: Vec<(u8, i64, String)>, tags: Vec<(String, String)> },
}

fn decimicro(deg: &str) -> anyhow::Result<i32> {
    let v: f64 = deg.parse().with_context(|| format!("coordinate {deg}"))?;
    Ok((v * 1e7).round() as i32)
}

/// Apply one gzip-compressed osmChange file on top of `into` (in file order, later wins).
pub fn parse_osc(gz: &[u8], into: &mut Changes) -> anyhow::Result<()> {
    let mut reader = quick_xml::Reader::from_reader(BufReader::new(GzDecoder::new(gz)));
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut deleting = false;
    let mut cur: Option<Cur> = None;

    fn attr(e: &BytesStart, key: &str) -> anyhow::Result<Option<String>> {
        for a in e.attributes() {
            let a = a?;
            if a.key.as_ref() == key {
                return Ok(Some(a.normalized_value(quick_xml::XmlVersion::Implicit1_0)?.into_owned()));
            }
        }
        Ok(None)
    }
    fn need(e: &BytesStart, key: &str) -> anyhow::Result<String> {
        attr(e, key)?.ok_or_else(|| anyhow!("<{}> without {key}", e.name().as_ref()))
    }

    let start = |e: &BytesStart, deleting: &mut bool, cur: &mut Option<Cur>| -> anyhow::Result<()> {
        match e.name().as_ref() {
            "create" | "modify" => *deleting = false,
            "delete" => *deleting = true,
            "node" => {
                let id = need(e, "id")?.parse()?;
                let (lat, lon) = if *deleting {
                    (0, 0)
                } else {
                    (decimicro(&need(e, "lat")?)?, decimicro(&need(e, "lon")?)?)
                };
                *cur = Some(Cur::Node { id, lat, lon, tags: vec![] });
            }
            "way" => *cur = Some(Cur::Way { id: need(e, "id")?.parse()?, refs: vec![], tags: vec![] }),
            "relation" => *cur = Some(Cur::Rel { id: need(e, "id")?.parse()?, members: vec![], tags: vec![] }),
            "nd" => {
                if let Some(Cur::Way { refs, .. }) = cur {
                    refs.push(need(e, "ref")?.parse()?);
                }
            }
            "tag" => {
                let (k, v) = (need(e, "k")?, attr(e, "v")?.unwrap_or_default());
                match cur {
                    Some(Cur::Node { tags, .. }) | Some(Cur::Way { tags, .. }) | Some(Cur::Rel { tags, .. }) => tags.push((k, v)),
                    None => {}
                }
            }
            "member" => {
                if let Some(Cur::Rel { members, .. }) = cur {
                    let ty = match need(e, "type")?.as_str() {
                        "node" => 0,
                        "way" => 1,
                        "relation" => 2,
                        t => bail!("unknown member type {t}"),
                    };
                    members.push((ty, need(e, "ref")?.parse()?, attr(e, "role")?.unwrap_or_default()));
                }
            }
            _ => {}
        }
        Ok(())
    };
    let end = |name: &str, deleting: bool, cur: &mut Option<Cur>, into: &mut Changes| {
        match name {
            "node" | "way" | "relation" => match cur.take() {
                Some(Cur::Node { id, lat, lon, tags }) => {
                    into.nodes.insert(id, (!deleting).then_some(NodeRec { lat, lon, tags }));
                }
                Some(Cur::Way { id, refs, tags }) => {
                    into.ways.insert(id, (!deleting).then_some(WayRec { refs, tags }));
                }
                Some(Cur::Rel { id, members, tags }) => {
                    into.relations.insert(id, (!deleting).then_some(RelRec { members, tags }));
                }
                None => {}
            },
            _ => {}
        }
    };

    loop {
        match reader.read_event_into(&mut buf).context("osmChange XML")? {
            Event::Start(e) => start(&e, &mut deleting, &mut cur)?,
            Event::Empty(e) => {
                start(&e, &mut deleting, &mut cur)?;
                end(e.name().as_ref(), deleting, &mut cur, into);
            }
            Event::End(e) => end(e.name().as_ref(), deleting, &mut cur, into),
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(())
}

// ---------------------------------------------------------------- PBF writer

fn varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}
fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}
fn key(out: &mut Vec<u8>, field: u32, wire: u32) {
    varint(out, ((field << 3) | wire) as u64);
}
fn put_uint(out: &mut Vec<u8>, field: u32, v: u64) {
    key(out, field, 0);
    varint(out, v);
}
fn put_sint(out: &mut Vec<u8>, field: u32, v: i64) {
    put_uint(out, field, zigzag(v));
}
fn put_bytes(out: &mut Vec<u8>, field: u32, b: &[u8]) {
    key(out, field, 2);
    varint(out, b.len() as u64);
    out.extend_from_slice(b);
}
fn put_packed_uint(out: &mut Vec<u8>, field: u32, vals: impl IntoIterator<Item = u64>) {
    let mut tmp = Vec::new();
    for v in vals {
        varint(&mut tmp, v);
    }
    if !tmp.is_empty() {
        put_bytes(out, field, &tmp);
    }
}
/// Delta-coded signed packed field (ids, coordinates, member ids).
fn put_packed_delta(out: &mut Vec<u8>, field: u32, vals: impl IntoIterator<Item = i64>) {
    let mut last = 0i64;
    put_packed_uint(
        out,
        field,
        vals.into_iter().map(|v| {
            let d = v - last;
            last = v;
            zigzag(d)
        }),
    );
}

#[derive(Default)]
struct Strings {
    index: HashMap<Vec<u8>, u32>,
    list: Vec<Vec<u8>>,
}
impl Strings {
    fn new() -> Self {
        let mut s = Self::default();
        s.id(b""); // index 0 is reserved for the empty string / delimiter
        s
    }
    fn id(&mut self, s: &[u8]) -> u32 {
        if let Some(&i) = self.index.get(s) {
            return i;
        }
        let i = self.list.len() as u32;
        self.index.insert(s.to_vec(), i);
        self.list.push(s.to_vec());
        i
    }
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for s in &self.list {
            put_bytes(&mut out, 1, s);
        }
        out
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Kind {
    Nodes,
    Ways,
    Relations,
}

struct PendingNode {
    id: i64,
    lat: i32,
    lon: i32,
    /// alternating key id / value id, terminated by 0 in the dense encoding
    kv: Vec<u32>,
}

struct PbfWriter {
    out: BufWriter<File>,
    strings: Strings,
    kind: Kind,
    nodes: Vec<PendingNode>,
    /// encoded Way / Relation messages of the current block
    msgs: Vec<Vec<u8>>,
    /// finished, uncompressed primitive blocks awaiting compression
    raw_blocks: Vec<Vec<u8>>,
    any_tags: bool,
}

impl PbfWriter {
    fn create(path: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            out: BufWriter::with_capacity(1 << 20, File::create(path).with_context(|| format!("create {}", path.display()))?),
            strings: Strings::new(),
            kind: Kind::Nodes,
            nodes: Vec::with_capacity(ELEMENTS_PER_BLOCK),
            msgs: Vec::with_capacity(ELEMENTS_PER_BLOCK),
            raw_blocks: Vec::new(),
            any_tags: false,
        })
    }

    fn header(&mut self, bbox: Option<&HeaderBBox>, state: &ReplState) -> anyhow::Result<()> {
        let mut h = Vec::new();
        if let Some(b) = bbox {
            let mut bb = Vec::new();
            let nano = |d: f64| (d * 1e9).round() as i64;
            put_sint(&mut bb, 1, nano(b.left));
            put_sint(&mut bb, 2, nano(b.right));
            put_sint(&mut bb, 3, nano(b.top));
            put_sint(&mut bb, 4, nano(b.bottom));
            put_bytes(&mut h, 1, &bb);
        }
        put_bytes(&mut h, 4, b"OsmSchema-V0.6");
        put_bytes(&mut h, 4, b"DenseNodes");
        put_bytes(&mut h, 16, b"MapsOffline osmdiff");
        put_uint(&mut h, 32, state.timestamp as u64);
        put_uint(&mut h, 33, state.sequence as u64);
        put_bytes(&mut h, 34, UPDATES_URL.as_bytes());
        let blob = compress_blob(&h);
        self.write_blob(b"OSMHeader", &blob)
    }

    fn write_blob(&mut self, kind: &[u8], blob: &[u8]) -> anyhow::Result<()> {
        let mut bh = Vec::new();
        put_bytes(&mut bh, 1, kind);
        put_uint(&mut bh, 3, blob.len() as u64);
        self.out.write_all(&(bh.len() as u32).to_be_bytes())?;
        self.out.write_all(&bh)?;
        self.out.write_all(blob)?;
        Ok(())
    }

    fn switch(&mut self, kind: Kind) -> anyhow::Result<()> {
        if self.kind != kind {
            self.end_block()?;
            self.kind = kind;
        }
        Ok(())
    }

    fn tag_ids<'a>(&mut self, tags: impl Iterator<Item = (&'a str, &'a str)>) -> (Vec<u32>, Vec<u32>) {
        let (mut k, mut v) = (Vec::new(), Vec::new());
        for (a, b) in tags {
            k.push(self.strings.id(a.as_bytes()));
            v.push(self.strings.id(b.as_bytes()));
        }
        (k, v)
    }

    fn node<'a>(&mut self, id: i64, lat: i32, lon: i32, tags: impl Iterator<Item = (&'a str, &'a str)>) -> anyhow::Result<()> {
        self.switch(Kind::Nodes)?;
        let mut kv = Vec::new();
        for (k, v) in tags {
            kv.push(self.strings.id(k.as_bytes()));
            kv.push(self.strings.id(v.as_bytes()));
        }
        if !kv.is_empty() {
            self.any_tags = true;
        }
        self.nodes.push(PendingNode { id, lat, lon, kv });
        self.after_element()
    }

    fn way<'a>(&mut self, id: i64, refs: impl Iterator<Item = i64>, tags: impl Iterator<Item = (&'a str, &'a str)>) -> anyhow::Result<()> {
        self.switch(Kind::Ways)?;
        let (k, v) = self.tag_ids(tags);
        let mut m = Vec::new();
        put_uint(&mut m, 1, id as u64);
        put_packed_uint(&mut m, 2, k.into_iter().map(u64::from));
        put_packed_uint(&mut m, 3, v.into_iter().map(u64::from));
        put_packed_delta(&mut m, 8, refs);
        self.msgs.push(m);
        self.after_element()
    }

    fn relation<'a>(&mut self, id: i64, members: impl Iterator<Item = (u8, i64, &'a str)>, tags: impl Iterator<Item = (&'a str, &'a str)>) -> anyhow::Result<()> {
        self.switch(Kind::Relations)?;
        let (k, v) = self.tag_ids(tags);
        let (mut roles, mut ids, mut types) = (Vec::new(), Vec::new(), Vec::new());
        for (t, id, role) in members {
            roles.push(u64::from(self.strings.id(role.as_bytes())));
            ids.push(id);
            types.push(u64::from(t));
        }
        let mut m = Vec::new();
        put_uint(&mut m, 1, id as u64);
        put_packed_uint(&mut m, 2, k.into_iter().map(u64::from));
        put_packed_uint(&mut m, 3, v.into_iter().map(u64::from));
        put_packed_uint(&mut m, 8, roles);
        put_packed_delta(&mut m, 9, ids);
        put_packed_uint(&mut m, 10, types);
        self.msgs.push(m);
        self.after_element()
    }

    fn after_element(&mut self) -> anyhow::Result<()> {
        if self.nodes.len() + self.msgs.len() >= ELEMENTS_PER_BLOCK {
            self.end_block()?;
        }
        Ok(())
    }

    /// Encode the pending elements as one PrimitiveBlock.
    fn end_block(&mut self) -> anyhow::Result<()> {
        if self.nodes.is_empty() && self.msgs.is_empty() {
            return Ok(());
        }
        let mut group = Vec::new();
        match self.kind {
            Kind::Nodes => {
                let mut dense = Vec::new();
                put_packed_delta(&mut dense, 1, self.nodes.iter().map(|n| n.id));
                put_packed_delta(&mut dense, 8, self.nodes.iter().map(|n| i64::from(n.lat)));
                put_packed_delta(&mut dense, 9, self.nodes.iter().map(|n| i64::from(n.lon)));
                if self.any_tags {
                    put_packed_uint(&mut dense, 10, self.nodes.iter().flat_map(|n| n.kv.iter().copied().chain(std::iter::once(0)).map(u64::from)));
                }
                put_bytes(&mut group, 2, &dense);
            }
            Kind::Ways => {
                for m in &self.msgs {
                    put_bytes(&mut group, 3, m);
                }
            }
            Kind::Relations => {
                for m in &self.msgs {
                    put_bytes(&mut group, 4, m);
                }
            }
        }
        let mut block = Vec::new();
        put_bytes(&mut block, 1, &self.strings.encode());
        put_bytes(&mut block, 2, &group);
        self.raw_blocks.push(block);
        self.nodes.clear();
        self.msgs.clear();
        self.strings = Strings::new();
        self.any_tags = false;
        if self.raw_blocks.len() >= BLOCKS_PER_FLUSH {
            self.flush_blocks()?;
        }
        Ok(())
    }

    fn flush_blocks(&mut self) -> anyhow::Result<()> {
        let raw = std::mem::take(&mut self.raw_blocks);
        let blobs: Vec<Vec<u8>> = raw.par_iter().map(|b| compress_blob(b)).collect();
        for b in &blobs {
            self.write_blob(b"OSMData", b)?;
        }
        Ok(())
    }

    fn finish(mut self) -> anyhow::Result<()> {
        self.end_block()?;
        self.flush_blocks()?;
        self.out.flush()?;
        Ok(())
    }
}

/// Blob message: raw_size + zlib_data.
fn compress_blob(raw: &[u8]) -> Vec<u8> {
    let mut enc = ZlibEncoder::new(Vec::with_capacity(raw.len() / 2), Compression::new(4));
    enc.write_all(raw).expect("in-memory write");
    let z = enc.finish().expect("in-memory finish");
    let mut blob = Vec::with_capacity(z.len() + 16);
    put_uint(&mut blob, 2, raw.len() as u64);
    put_bytes(&mut blob, 3, &z);
    blob
}

// ---------------------------------------------------------------- apply

fn tags_of(t: &[(String, String)]) -> impl Iterator<Item = (&str, &str)> {
    t.iter().map(|(k, v)| (k.as_str(), v.as_str()))
}

/// Stream `src` into `dest`, replacing / dropping the changed elements and appending the new ones
/// (nodes before the first way, ways before the first relation, relations at the end).
pub fn apply(src: &Path, dest: &Path, mut ch: Changes, bbox: Option<&HeaderBBox>, state: &ReplState, progress: ProgressFn) -> anyhow::Result<()> {
    let mut w = PbfWriter::create(dest)?;
    w.header(bbox, state)?;
    let mut phase = 0u8; // 0 nodes, 1 ways, 2 relations
    let mut seen = 0u64;

    fn new_nodes(w: &mut PbfWriter, ch: &mut Changes) -> anyhow::Result<()> {
        let mut rest: Vec<(i64, NodeRec)> = ch.nodes.drain().filter_map(|(id, r)| r.map(|r| (id, r))).collect();
        rest.sort_by_key(|(id, _)| *id);
        for (id, n) in &rest {
            w.node(*id, n.lat, n.lon, tags_of(&n.tags))?;
        }
        Ok(())
    }
    fn new_ways(w: &mut PbfWriter, ch: &mut Changes) -> anyhow::Result<()> {
        let mut rest: Vec<(i64, WayRec)> = ch.ways.drain().filter_map(|(id, r)| r.map(|r| (id, r))).collect();
        rest.sort_by_key(|(id, _)| *id);
        for (id, way) in &rest {
            w.way(*id, way.refs.iter().copied(), tags_of(&way.tags))?;
        }
        Ok(())
    }
    fn new_relations(w: &mut PbfWriter, ch: &mut Changes) -> anyhow::Result<()> {
        let mut rest: Vec<(i64, RelRec)> = ch.relations.drain().filter_map(|(id, r)| r.map(|r| (id, r))).collect();
        rest.sort_by_key(|(id, _)| *id);
        for (id, r) in &rest {
            w.relation(*id, r.members.iter().map(|(t, id, role)| (*t, *id, role.as_str())), tags_of(&r.tags))?;
        }
        Ok(())
    }
    fn enter(phase: &mut u8, target: u8, w: &mut PbfWriter, ch: &mut Changes) -> anyhow::Result<()> {
        while *phase < target {
            match *phase {
                0 => new_nodes(w, ch)?,
                1 => new_ways(w, ch)?,
                _ => {}
            }
            *phase += 1;
        }
        Ok(())
    }

    // Decode the input blobs in parallel (zlib + protobuf are the expensive part), handle the
    // elements in file order.
    let mut blobs = BlobReader::from_path(src).with_context(|| format!("open {}", src.display()))?;
    let mut handle = |el: Element| -> anyhow::Result<()> {
        match el {
            Element::DenseNode(n) => match ch.nodes.remove(&n.id()) {
                Some(Some(rec)) => w.node(n.id(), rec.lat, rec.lon, tags_of(&rec.tags)),
                Some(None) => Ok(()),
                None => w.node(n.id(), n.decimicro_lat(), n.decimicro_lon(), n.tags()),
            },
            Element::Node(n) => match ch.nodes.remove(&n.id()) {
                Some(Some(rec)) => w.node(n.id(), rec.lat, rec.lon, tags_of(&rec.tags)),
                Some(None) => Ok(()),
                None => w.node(n.id(), n.decimicro_lat(), n.decimicro_lon(), n.tags()),
            },
            Element::Way(way) => {
                enter(&mut phase, 1, &mut w, &mut ch)?;
                match ch.ways.remove(&way.id()) {
                    Some(Some(rec)) => w.way(way.id(), rec.refs.iter().copied(), tags_of(&rec.tags)),
                    Some(None) => Ok(()),
                    None => w.way(way.id(), way.refs(), way.tags()),
                }
            }
            Element::Relation(rel) => {
                enter(&mut phase, 2, &mut w, &mut ch)?;
                match ch.relations.remove(&rel.id()) {
                    Some(Some(rec)) => w.relation(rel.id(), rec.members.iter().map(|(t, id, role)| (*t, *id, role.as_str())), tags_of(&rec.tags)),
                    Some(None) => Ok(()),
                    None => {
                        let members: Vec<(u8, i64, &str)> = rel
                            .members()
                            .map(|m| {
                                let t = match m.member_type {
                                    RelMemberType::Node => 0,
                                    RelMemberType::Way => 1,
                                    RelMemberType::Relation => 2,
                                };
                                (t, m.member_id, m.role().unwrap_or(""))
                            })
                            .collect();
                        w.relation(rel.id(), members.into_iter(), rel.tags())
                    }
                }
            }
        }
    };
    loop {
        let chunk: Vec<osmpbf::Blob> = blobs.by_ref().take(DECODE_CHUNK).collect::<Result<_, _>>()?;
        if chunk.is_empty() {
            break;
        }
        let decoded: Vec<BlobDecode> = chunk.par_iter().map(|b| b.decode()).collect::<Result<_, _>>()?;
        for d in &decoded {
            if let BlobDecode::OsmData(block) = d {
                for el in block.elements() {
                    seen += 1;
                    if seen.is_multiple_of(1_000_000) && !progress(ProgressPhase::Processing, seen, 0, "מחיל את השינויים על נתוני OSM") {
                        bail!("cancelled");
                    }
                    handle(el)?;
                }
            }
        }
    }
    enter(&mut phase, 2, &mut w, &mut ch)?;
    new_relations(&mut w, &mut ch)?;
    w.finish()
}

// ---------------------------------------------------------------- orchestration

/// Bring the local extract up to date: nothing, the missing daily diffs, or the full file.
/// Returns the version string of the resulting PBF.
pub fn update_pbf(dest: &Path, progress: ProgressFn, cancel: &Cancel) -> anyhow::Result<Option<String>> {
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p)?;
    }
    let client = download::http_client()?;
    let meta = download::load_meta(dest);
    let local = match (&meta, dest.exists()) {
        (Some(m), true) if m.completed => header_info(dest).ok().and_then(|(s, _)| s),
        _ => None,
    };
    let remote = match remote_state(&client) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("replication state unavailable: {e:#}");
            if let Some(l) = local {
                progress(ProgressPhase::Processing, 1, 1, "השרת אינו זמין – נעשה שימוש בקובץ הקיים");
                return Ok(Some(l.version()));
            }
            return full_download(dest, progress, cancel);
        }
    };
    if let Some(l) = local {
        if l.sequence >= remote.sequence {
            progress(ProgressPhase::Processing, 1, 1, "הקובץ כבר מעודכן");
            return Ok(Some(l.version()));
        }
        let missing = remote.sequence - l.sequence;
        let since_full = remote.sequence - meta.as_ref().and_then(|m| m.full_sequence).unwrap_or(l.sequence);
        if missing <= MAX_DIFFS && since_full <= FULL_REFRESH_DAYS {
            match apply_diffs(&client, dest, &l, &remote, progress, cancel) {
                Ok(v) => return Ok(Some(v)),
                Err(e) if cancel.is_cancelled() => return Err(e),
                Err(e) => {
                    log::warn!("incremental OSM update failed, falling back to the full extract: {e:#}");
                    progress(ProgressPhase::Downloading, 0, 0, &format!("העדכון החלקי נכשל ({e:#}) – מוריד את הקובץ המלא"));
                }
            }
        }
    }
    full_download(dest, progress, cancel)
}

fn full_download(dest: &Path, progress: ProgressFn, cancel: &Cancel) -> anyhow::Result<Option<String>> {
    let v = download::ensure_fresh(PBF_URL, dest, progress, cancel)?;
    progress(ProgressPhase::Downloading, 1, 1, "מאמת את הקובץ שהורד");
    download::verify_md5_sidecar(PBF_URL, dest)?;
    let state = header_info(dest).ok().and_then(|(s, _)| s);
    if let (Some(st), Some(mut m)) = (state, download::load_meta(dest)) {
        if m.full_sequence != Some(st.sequence) {
            m.full_sequence = Some(st.sequence);
            let _ = download::save_meta(dest, &m);
        }
    }
    Ok(state.map(|s| s.version()).or(v))
}

fn apply_diffs(client: &Client, dest: &Path, local: &ReplState, remote: &ReplState, progress: ProgressFn, cancel: &Cancel) -> anyhow::Result<String> {
    let total = (remote.sequence - local.sequence) as u64;
    let mut changes = Changes::default();
    let mut bytes = 0u64;
    for (i, seq) in (local.sequence + 1..=remote.sequence).enumerate() {
        if !progress(ProgressPhase::Downloading, i as u64, total, &format!("מוריד שינויים יומיים ({}/{total})", i + 1)) {
            bail!("cancelled");
        }
        let gz = fetch(client, &diff_url(seq), cancel)?;
        bytes += gz.len() as u64;
        parse_osc(&gz, &mut changes).with_context(|| format!("change file #{seq}"))?;
    }
    log::info!("{total} daily diffs, {} KB, {} changed elements", bytes >> 10, changes.len());
    if !progress(ProgressPhase::Processing, 0, 0, &format!("מחיל {} שינויים על נתוני OSM", changes.len())) {
        bail!("cancelled");
    }
    let (_, bbox) = header_info(dest)?;
    let tmp = dest.with_extension("pbf.tmp");
    let _ = std::fs::remove_file(&tmp);
    let res = apply(dest, &tmp, changes, bbox.as_ref(), remote, progress);
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    // sanity: the result must open and carry the new state
    match header_info(&tmp) {
        Ok((Some(st), _)) if st == *remote => {}
        other => {
            let _ = std::fs::remove_file(&tmp);
            bail!("rewritten PBF has an unexpected header: {other:?}");
        }
    }
    crate::paths::atomic_replace(dest, &tmp)?;
    let mut meta = download::load_meta(dest).unwrap_or_default();
    meta.url = PBF_URL.into();
    meta.etag = None;
    meta.last_modified = None;
    meta.content_length = std::fs::metadata(dest).ok().map(|m| m.len());
    meta.completed = true;
    if meta.full_sequence.is_none() {
        meta.full_sequence = Some(local.sequence);
    }
    download::save_meta(dest, &meta)?;
    Ok(remote.version())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use osmpbf::ElementReader;

    fn gz(s: &str) -> Vec<u8> {
        let mut e = GzEncoder::new(Vec::new(), Compression::default());
        e.write_all(s.as_bytes()).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn state_and_urls() {
        let st = parse_state("# comment\ntimestamp=2026-11-02T21\\:21\\:26Z\nsequenceNumber=4937\n").unwrap();
        assert_eq!(st.sequence, 4937);
        assert_eq!(st.version(), "OSM 2026-11-02 #4937");
        assert_eq!(diff_url(4937), format!("{UPDATES_URL}000/004/937.osc.gz"));
        assert_eq!(diff_url(1_002_003), format!("{UPDATES_URL}001/002/003.osc.gz"));
    }

    #[test]
    fn varints() {
        let mut v = Vec::new();
        varint(&mut v, 300);
        assert_eq!(v, [0xac, 0x02]);
        assert_eq!(zigzag(0), 0);
        assert_eq!(zigzag(-1), 1);
        assert_eq!(zigzag(1), 2);
        assert_eq!(zigzag(-2), 3);
    }

    const OSC: &str = r#"<?xml version='1.0' encoding='UTF-8'?>
<osmChange version="0.6" generator="test">
<create>
  <node id="100000" version="1" lat="32.0800000" lon="34.7800000"><tag k="amenity" v="cafe"/><tag k="name" v="קפה &quot;חדש&quot;"/></node>
  <node id="100001" lat="32.0810000" lon="34.7810000"/>
  <way id="500"><nd ref="100000"/><nd ref="100001"/><tag k="highway" v="residential"/><tag k="name" v="רחוב חדש"/></way>
</create>
<modify>
  <node id="1" lat="31.5000000" lon="35.0000000"><tag k="name" v="moved"/></node>
  <relation id="900"><member type="way" ref="500" role="outer"/><member type="node" ref="100000" role=""/><tag k="type" v="multipolygon"/></relation>
</modify>
<delete>
  <node id="2" lat="0" lon="0"/>
  <way id="20"/>
</delete>
</osmChange>"#;

    /// A small base file written by our own writer, then a change file applied on top: the
    /// result must read back with osmpbf exactly as the merge implies.
    #[test]
    fn roundtrip_apply() {
        let dir = std::env::temp_dir().join(format!("mo-osmdiff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("base.pbf");
        let out = dir.join("out.pbf");
        let bbox = HeaderBBox { left: 34.2, right: 35.95, top: 33.35, bottom: 29.45 };
        let s0 = ReplState { sequence: 10, timestamp: 1_700_000_000 };
        {
            let mut w = PbfWriter::create(&base).unwrap();
            w.header(Some(&bbox), &s0).unwrap();
            for id in 1..=3i64 {
                w.node(id, 320_000_000 + id as i32, 347_000_000, [("k", "v")].into_iter().filter(|_| id == 3)).unwrap();
            }
            // many untagged nodes so more than one block is written
            for id in 4..=20_000i64 {
                w.node(id, 320_000_000, 347_000_000 + id as i32, std::iter::empty()).unwrap();
            }
            w.way(20, [1i64, 2].into_iter(), [("highway", "primary")].into_iter()).unwrap();
            w.way(21, [2i64, 3].into_iter(), [("highway", "track")].into_iter()).unwrap();
            w.relation(900, [(1u8, 21i64, "outer")].into_iter(), [("type", "route")].into_iter()).unwrap();
            w.finish().unwrap();
        }
        let (st, bb) = header_info(&base).unwrap();
        assert_eq!(st, Some(s0));
        let bb = bb.unwrap();
        assert!((bb.left - 34.2).abs() < 1e-9 && (bb.bottom - 29.45).abs() < 1e-9);

        let mut ch = Changes::default();
        parse_osc(&gz(OSC), &mut ch).unwrap();
        assert_eq!(ch.len(), 7);
        let s1 = ReplState { sequence: 11, timestamp: 1_700_086_400 };
        let progress = |_: ProgressPhase, _: u64, _: u64, _: &str| true;
        apply(&base, &out, ch, Some(&bbox), &s1, &progress).unwrap();

        assert_eq!(header_info(&out).unwrap().0, Some(s1));
        let mut nodes: Vec<(i64, i32, i32, Vec<(String, String)>)> = vec![];
        let mut ways: Vec<(i64, Vec<i64>, Vec<(String, String)>)> = vec![];
        let mut rels: Vec<(i64, Vec<(RelMemberType, i64, String)>, Vec<(String, String)>)> = vec![];
        ElementReader::from_path(&out)
            .unwrap()
            .for_each(|el| match el {
                Element::DenseNode(n) => nodes.push((n.id(), n.decimicro_lat(), n.decimicro_lon(), n.tags().map(|(k, v)| (k.into(), v.into())).collect())),
                Element::Node(n) => nodes.push((n.id(), n.decimicro_lat(), n.decimicro_lon(), n.tags().map(|(k, v)| (k.into(), v.into())).collect())),
                Element::Way(w) => ways.push((w.id(), w.refs().collect(), w.tags().map(|(k, v)| (k.into(), v.into())).collect())),
                Element::Relation(r) => rels.push((
                    r.id(),
                    r.members()
                        .map(|m| {
                            let role: String = m.role().unwrap().into();
                            (m.member_type, m.member_id, role)
                        })
                        .collect(),
                    r.tags().map(|(k, v)| (k.into(), v.into())).collect(),
                )),
            })
            .unwrap();
        // 20000 base nodes - deleted #2 + new #100000, #100001 (appended after the existing nodes)
        assert_eq!(nodes.len(), 20_001);
        assert!(nodes.iter().all(|n| n.0 != 2));
        let n1 = nodes.iter().find(|n| n.0 == 1).unwrap();
        assert_eq!((n1.1, n1.2), (315_000_000, 350_000_000));
        assert_eq!(n1.3, vec![("name".to_string(), "moved".to_string())]);
        let n3 = nodes.iter().find(|n| n.0 == 3).unwrap();
        assert_eq!(n3.3, vec![("k".to_string(), "v".to_string())]);
        assert_eq!(nodes[nodes.len() - 2].0, 100_000);
        let n100 = nodes.iter().find(|n| n.0 == 100_000).unwrap();
        assert_eq!((n100.1, n100.2), (320_800_000, 347_800_000));
        assert_eq!(n100.3[1], ("name".to_string(), "קפה \"חדש\"".to_string()));
        // untagged node far into the file keeps its coordinates
        let n9999 = nodes.iter().find(|n| n.0 == 9999).unwrap();
        assert_eq!((n9999.1, n9999.2), (320_000_000, 347_009_999));
        assert!(n9999.3.is_empty());
        assert_eq!(ways.len(), 2); // #20 deleted, #21 kept, #500 new
        assert_eq!(ways[0].0, 21);
        assert_eq!(ways[1], (500, vec![100_000, 100_001], vec![("highway".to_string(), "residential".to_string()), ("name".to_string(), "רחוב חדש".to_string())]));

        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].0, 900);
        assert_eq!(rels[0].1, vec![(RelMemberType::Way, 500, "outer".to_string()), (RelMemberType::Node, 100_000, String::new())]);
        assert_eq!(rels[0].2, vec![("type".to_string(), "multipolygon".to_string())]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
