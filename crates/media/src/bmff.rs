//! Minimal ISO base media file format (MP4) box reading & writing helpers.

use std::io::{Read, Seek, SeekFrom};

use crate::{Error, Result};

pub(crate) type FourCC = [u8; 4];

/// A box parsed from an in-memory buffer.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RawBox<'a> {
    pub typ: FourCC,
    /// The box content after the (possibly 64-bit) header.
    pub payload: &'a [u8],
    /// The complete box including its header.
    pub full: &'a [u8],
}

/// Iterates the boxes contained in `data`.
pub(crate) fn iter_boxes(data: &[u8]) -> BoxIter<'_> {
    BoxIter {
        data,
        pos: 0,
        failed: false,
    }
}

pub(crate) struct BoxIter<'a> {
    data: &'a [u8],
    pos: usize,
    failed: bool,
}

impl<'a> Iterator for BoxIter<'a> {
    type Item = Result<RawBox<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.pos >= self.data.len() {
            return None;
        }
        let rest = &self.data[self.pos..];
        let res = (|| {
            if rest.len() < 8 {
                return Err(Error::invalid("truncated box header"));
            }
            let size32 = u32::from_be_bytes(rest[0..4].try_into().unwrap()) as u64;
            let typ: FourCC = rest[4..8].try_into().unwrap();
            let (hlen, size) = match size32 {
                0 => (8u64, rest.len() as u64),
                1 => {
                    if rest.len() < 16 {
                        return Err(Error::invalid("truncated large box header"));
                    }
                    (16, u64::from_be_bytes(rest[8..16].try_into().unwrap()))
                }
                s => (8, s),
            };
            if size < hlen || size > rest.len() as u64 {
                return Err(Error::invalid(format!(
                    "box '{}' has invalid size {}",
                    fourcc_str(&typ),
                    size
                )));
            }
            Ok(RawBox {
                typ,
                payload: &rest[hlen as usize..size as usize],
                full: &rest[..size as usize],
            })
        })();
        match res {
            Ok(b) => {
                self.pos += b.full.len();
                Some(Ok(b))
            }
            Err(e) => {
                self.failed = true;
                Some(Err(e))
            }
        }
    }
}

/// Finds the first child box of type `typ` in `data`.
pub(crate) fn find<'a>(data: &'a [u8], typ: &FourCC) -> Result<Option<RawBox<'a>>> {
    for b in iter_boxes(data) {
        let b = b?;
        if &b.typ == typ {
            return Ok(Some(b));
        }
    }
    Ok(None)
}

#[cfg_attr(not(test), allow(dead_code))]
/// Follows a path of nested container boxes, e.g. `[b"mdia", b"minf"]`.
pub(crate) fn find_path<'a>(data: &'a [u8], path: &[&FourCC]) -> Result<Option<RawBox<'a>>> {
    let mut cur = data;
    let mut found = None;
    for typ in path {
        match find(cur, typ)? {
            Some(b) => {
                cur = b.payload;
                found = Some(b);
            }
            None => return Ok(None),
        }
    }
    Ok(found)
}

pub(crate) fn fourcc_str(t: &FourCC) -> String {
    t.iter()
        .map(|&c| {
            if c.is_ascii_graphic() || c == b' ' {
                c as char
            } else {
                '?'
            }
        })
        .collect()
}

/// A top level box located in a seekable stream.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TopBox {
    pub typ: FourCC,
    pub offset: u64,
    pub header_len: u64,
    pub size: u64,
}

impl TopBox {
    pub fn payload_offset(&self) -> u64 {
        self.offset + self.header_len
    }
    pub fn payload_len(&self) -> u64 {
        self.size - self.header_len
    }
}

/// Lists the top level boxes of a file without reading their contents.
pub(crate) fn scan_top_level<R: Read + Seek>(r: &mut R) -> Result<Vec<TopBox>> {
    let file_len = r.seek(SeekFrom::End(0))?;
    let mut pos = 0u64;
    let mut out = Vec::new();
    while pos < file_len {
        if file_len - pos < 8 {
            return Err(Error::invalid("trailing garbage after last box"));
        }
        r.seek(SeekFrom::Start(pos))?;
        let mut hdr = [0u8; 8];
        r.read_exact(&mut hdr)?;
        let size32 = u32::from_be_bytes(hdr[0..4].try_into().unwrap()) as u64;
        let typ: FourCC = hdr[4..8].try_into().unwrap();
        let (header_len, size) = match size32 {
            0 => (8, file_len - pos),
            1 => {
                let mut ext = [0u8; 8];
                r.read_exact(&mut ext)?;
                (16, u64::from_be_bytes(ext))
            }
            s => (8, s),
        };
        if size < header_len {
            return Err(Error::invalid(format!(
                "top level box '{}' has invalid size {}",
                fourcc_str(&typ),
                size
            )));
        }
        if pos + size > file_len {
            return Err(Error::invalid(format!(
                "top level box '{}' is truncated (file incomplete?)",
                fourcc_str(&typ)
            )));
        }
        out.push(TopBox {
            typ,
            offset: pos,
            header_len,
            size,
        });
        pos += size;
    }
    Ok(out)
}

/// Reads the payload of a (small) top level box into memory.
pub(crate) fn read_payload<R: Read + Seek>(r: &mut R, b: &TopBox) -> Result<Vec<u8>> {
    const LIMIT: u64 = 256 * 1024 * 1024;
    if b.payload_len() > LIMIT {
        return Err(Error::Unsupported(format!(
            "box '{}' too large ({} bytes)",
            fourcc_str(&b.typ),
            b.payload_len()
        )));
    }
    r.seek(SeekFrom::Start(b.payload_offset()))?;
    let mut buf = vec![0u8; b.payload_len() as usize];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

/// Big endian cursor over a byte slice.
pub(crate) struct Rd<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Rd<'a> {
    pub fn new(d: &'a [u8]) -> Self {
        Self { d, p: 0 }
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.d.len() - self.p < n {
            return Err(Error::invalid("unexpected end of box data"));
        }
        let s = &self.d[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.bytes(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.bytes(8)?.try_into().unwrap()))
    }
    /// Reads the version + flags header of a full box.
    pub fn full_header(&mut self) -> Result<(u8, u32)> {
        let v = self.u32()?;
        Ok(((v >> 24) as u8, v & 0x00ff_ffff))
    }
}

/// Builds a box with a 32-bit size header.
pub(crate) fn make_box(typ: &FourCC, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(payload.len() + 8);
    v.extend_from_slice(&((payload.len() + 8) as u32).to_be_bytes());
    v.extend_from_slice(typ);
    v.extend_from_slice(payload);
    v
}

pub(crate) fn make_full_box(typ: &FourCC, version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(payload.len() + 4);
    p.extend_from_slice(&(((version as u32) << 24) | (flags & 0x00ff_ffff)).to_be_bytes());
    p.extend_from_slice(payload);
    make_box(typ, &p)
}

pub(crate) fn make_container(typ: &FourCC, children: &[Vec<u8>]) -> Vec<u8> {
    make_box(typ, &children.concat())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iterate_nested_boxes() {
        let inner = make_box(b"abcd", &[1, 2, 3]);
        let outer = make_container(b"moov", &[inner.clone(), make_box(b"free", &[])]);
        let boxes: Vec<_> = iter_boxes(&outer).collect::<Result<_>>().unwrap();
        assert_eq!(boxes.len(), 1);
        assert_eq!(&boxes[0].typ, b"moov");
        let children: Vec<_> = iter_boxes(boxes[0].payload).collect::<Result<_>>().unwrap();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].payload, &[1, 2, 3]);
        assert_eq!(children[0].full, inner.as_slice());
        assert!(find_path(&outer, &[b"moov", b"free"]).unwrap().is_some());
        assert!(find_path(&outer, &[b"moov", b"nope"]).unwrap().is_none());
    }

    #[test]
    fn truncated_box_is_error() {
        let mut b = make_box(b"abcd", &[0; 10]);
        b.truncate(12);
        assert!(iter_boxes(&b).next().unwrap().is_err());
    }
}
