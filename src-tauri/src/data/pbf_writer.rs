//! Minimal OSM PBF writer: dense nodes, ways, relations, zlib blobs, no element metadata.
//! Used by `osmdiff` (rewriting the Israel extract) and `obf::to_pbf` (OsmAnd → PBF); both
//! outputs are read back by the routing / geocode builders through `osmpbf`.
//!
//! Callers must write all nodes, then all ways, then all relations, each kind with ascending ids.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::Context;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use osmpbf::HeaderBBox;
use rayon::prelude::*;

const ELEMENTS_PER_BLOCK: usize = 8000;
const BLOCKS_PER_FLUSH: usize = 16;

pub fn varint(out: &mut Vec<u8>, mut v: u64) {
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
pub fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}
fn key(out: &mut Vec<u8>, field: u32, wire: u32) {
    varint(out, ((field << 3) | wire) as u64);
}
pub fn put_uint(out: &mut Vec<u8>, field: u32, v: u64) {
    key(out, field, 0);
    varint(out, v);
}
pub fn put_sint(out: &mut Vec<u8>, field: u32, v: i64) {
    put_uint(out, field, zigzag(v));
}
pub fn put_bytes(out: &mut Vec<u8>, field: u32, b: &[u8]) {
    key(out, field, 2);
    varint(out, b.len() as u64);
    out.extend_from_slice(b);
}
pub fn put_packed_uint(out: &mut Vec<u8>, field: u32, vals: impl IntoIterator<Item = u64>) {
    let mut tmp = Vec::new();
    for v in vals {
        varint(&mut tmp, v);
    }
    if !tmp.is_empty() {
        put_bytes(out, field, &tmp);
    }
}
/// Delta-coded signed packed field (ids, coordinates, member ids).
pub fn put_packed_delta(out: &mut Vec<u8>, field: u32, vals: impl IntoIterator<Item = i64>) {
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
pub struct Strings {
    index: HashMap<Vec<u8>, u32>,
    list: Vec<Vec<u8>>,
}
impl Strings {
    pub fn new() -> Self {
        let mut s = Self::default();
        s.id(b""); // index 0 is reserved for the empty string / delimiter
        s
    }
    pub fn id(&mut self, s: &[u8]) -> u32 {
        if let Some(&i) = self.index.get(s) {
            return i;
        }
        let i = self.list.len() as u32;
        self.index.insert(s.to_vec(), i);
        self.list.push(s.to_vec());
        i
    }
    pub fn encode(&self) -> Vec<u8> {
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

/// Replication fields of the header (osmosis_replication_*).
pub struct Replication<'a> {
    pub timestamp: i64,
    pub sequence: i64,
    pub base_url: &'a str,
}

pub struct PbfWriter {
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
    pub fn create(path: &Path) -> anyhow::Result<Self> {
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

    /// The OSMHeader blob; must be written first.
    pub fn write_header(&mut self, bbox: Option<&HeaderBBox>, program: &str, repl: Option<&Replication>) -> anyhow::Result<()> {
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
        put_bytes(&mut h, 16, program.as_bytes());
        if let Some(r) = repl {
            put_uint(&mut h, 32, r.timestamp as u64);
            put_uint(&mut h, 33, r.sequence as u64);
            put_bytes(&mut h, 34, r.base_url.as_bytes());
        }
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

    /// `lat` / `lon` in 1e-7 degrees.
    pub fn node<'a>(&mut self, id: i64, lat: i32, lon: i32, tags: impl Iterator<Item = (&'a str, &'a str)>) -> anyhow::Result<()> {
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

    pub fn way<'a>(&mut self, id: i64, refs: impl Iterator<Item = i64>, tags: impl Iterator<Item = (&'a str, &'a str)>) -> anyhow::Result<()> {
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

    /// Members as (type 0 node / 1 way / 2 relation, id, role).
    pub fn relation<'a>(&mut self, id: i64, members: impl Iterator<Item = (u8, i64, &'a str)>, tags: impl Iterator<Item = (&'a str, &'a str)>) -> anyhow::Result<()> {
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

    pub fn finish(mut self) -> anyhow::Result<()> {
        self.end_block()?;
        self.flush_blocks()?;
        self.out.flush()?;
        Ok(())
    }
}

/// Blob message: raw_size + zlib_data.
pub fn compress_blob(raw: &[u8]) -> Vec<u8> {
    let mut enc = ZlibEncoder::new(Vec::with_capacity(raw.len() / 2), Compression::new(4));
    enc.write_all(raw).expect("in-memory write");
    let z = enc.finish().expect("in-memory finish");
    let mut blob = Vec::with_capacity(z.len() + 16);
    put_uint(&mut blob, 2, raw.len() as u64);
    put_bytes(&mut blob, 3, &z);
    blob
}
