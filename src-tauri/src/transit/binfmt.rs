//! Raw little-endian array I/O for `transit.bin` v3 (same scheme as graph.bin v6: every array
//! is `u64 element count` + the elements' bytes; strings are one byte blob + u32 offsets).
//! Reading bounds every allocation by the bytes left in the file, so a damaged file fails with
//! an error instead of a huge allocation or a panic.

use std::io::{Read, Write};

use anyhow::{bail, Result};

/// # Safety
/// Implementors must be `Copy` types without padding for which every bit pattern is valid.
pub(crate) unsafe trait Pod: Copy + Default {}
unsafe impl Pod for u8 {}
unsafe impl Pod for u16 {}
unsafe impl Pod for u32 {}
unsafe impl Pod for i32 {}
unsafe impl Pod for u64 {}
unsafe impl Pod for f64 {}

#[cfg(not(target_endian = "little"))]
compile_error!("transit file I/O assumes a little-endian target");

fn as_bytes<T: Pod>(v: &[T]) -> &[u8] {
    // SAFETY: T is Pod (no padding), so its memory is a valid initialised byte slice.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

fn as_bytes_mut<T: Pod>(v: &mut [T]) -> &mut [u8] {
    // SAFETY: T is Pod: every byte pattern written through this view is a valid T.
    unsafe { std::slice::from_raw_parts_mut(v.as_mut_ptr() as *mut u8, std::mem::size_of_val(v)) }
}

pub(crate) fn put<T: Pod, W: Write>(w: &mut W, v: &[T]) -> std::io::Result<()> {
    w.write_all(&(v.len() as u64).to_le_bytes())?;
    w.write_all(as_bytes(v))
}

#[cfg(test)]
pub(crate) fn put_u64<W: Write>(w: &mut W, v: u64) -> std::io::Result<()> {
    w.write_all(&v.to_le_bytes())
}

/// Strings as one UTF-8 blob + `n + 1` offsets.
pub(crate) fn put_strings<W: Write>(w: &mut W, v: &[String]) -> std::io::Result<()> {
    let mut offs = Vec::with_capacity(v.len() + 1);
    let mut blob = Vec::new();
    offs.push(0u32);
    for s in v {
        blob.extend_from_slice(s.as_bytes());
        offs.push(blob.len() as u32);
    }
    put(w, &offs)?;
    put(w, &blob)
}

/// Reader that knows how many bytes are left, so array lengths can be checked before allocating.
pub(crate) struct Reader<R: Read> {
    r: R,
    left: u64,
}

impl<R: Read> Reader<R> {
    pub fn new(r: R, len: u64) -> Self {
        Reader { r, left: len }
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        if buf.len() as u64 > self.left {
            bail!("file truncated");
        }
        self.r.read_exact(buf).map_err(|e| anyhow::anyhow!("file truncated ({e})"))?;
        self.left -= buf.len() as u64;
        Ok(())
    }

    pub fn u64(&mut self) -> Result<u64> {
        let mut b = [0u8; 8];
        self.read_exact(&mut b)?;
        Ok(u64::from_le_bytes(b))
    }

    pub fn magic(&mut self) -> Result<[u8; 4]> {
        let mut b = [0u8; 4];
        self.read_exact(&mut b)?;
        Ok(b)
    }

    pub fn get<T: Pod>(&mut self) -> Result<Vec<T>> {
        let n = self.u64()?;
        let bytes = n.saturating_mul(std::mem::size_of::<T>() as u64);
        if bytes > self.left {
            bail!("array of {n} elements does not fit in the file");
        }
        let mut v = vec![T::default(); n as usize];
        self.read_exact(as_bytes_mut(&mut v))?;
        Ok(v)
    }

    pub fn strings(&mut self) -> Result<Vec<String>> {
        let offs: Vec<u32> = self.get()?;
        let blob: Vec<u8> = self.get()?;
        if offs.is_empty() || offs[0] != 0 || *offs.last().unwrap() as usize != blob.len() {
            bail!("bad string table");
        }
        let mut out = Vec::with_capacity(offs.len() - 1);
        for w in offs.windows(2) {
            let (a, b) = (w[0] as usize, w[1] as usize);
            if a > b {
                bail!("bad string table");
            }
            out.push(std::str::from_utf8(&blob[a..b]).map_err(|_| anyhow::anyhow!("bad UTF-8 in string table"))?.to_string());
        }
        Ok(out)
    }

    pub fn left(&self) -> u64 {
        self.left
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut buf = Vec::new();
        put(&mut buf, &[1u32, 2, 3]).unwrap();
        put_strings(&mut buf, &["אבג".to_string(), String::new(), "x".to_string()]).unwrap();
        put(&mut buf, &[1.5f64]).unwrap();
        let len = buf.len() as u64;
        let mut r = Reader::new(&buf[..], len);
        assert_eq!(r.get::<u32>().unwrap(), vec![1, 2, 3]);
        assert_eq!(r.strings().unwrap(), vec!["אבג".to_string(), String::new(), "x".to_string()]);
        assert_eq!(r.get::<f64>().unwrap(), vec![1.5]);
        assert_eq!(r.left(), 0);
    }

    #[test]
    fn huge_length_is_rejected() {
        let mut buf = Vec::new();
        put_u64(&mut buf, u64::MAX / 2).unwrap();
        let len = buf.len() as u64;
        assert!(Reader::new(&buf[..], len).get::<u32>().is_err());
    }
}
