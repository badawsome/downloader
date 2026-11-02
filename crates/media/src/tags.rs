//! Audio metadata model and serialisation to iTunes (`ilst`) atoms and FLAC
//! (Vorbis comment / PICTURE) metadata blocks.

use crate::bmff::{make_box, make_container, make_full_box};

/// Tag values written into the output file. Empty fields are skipped.
#[derive(Debug, Clone, Default)]
pub struct Metadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    /// `YYYY`, `YYYY-MM-DD` or a full ISO 8601 timestamp.
    pub date: Option<String>,
    pub genre: Option<String>,
    pub comment: Option<String>,
    pub description: Option<String>,
    /// (track number, total tracks)
    pub track: Option<(u32, Option<u32>)>,
    pub cover: Option<Picture>,
    /// Custom key/value pairs (freeform MP4 atoms / Vorbis comments).
    pub custom: Vec<(String, String)>,
    /// Name of the encoder/tool.
    pub tool: Option<String>,
}

/// Embedded cover art.
#[derive(Debug, Clone)]
pub struct Picture {
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Jpeg,
    Png,
    Bmp,
    Gif,
    Webp,
}

impl ImageFormat {
    pub fn mime(&self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Png => "image/png",
            ImageFormat::Bmp => "image/bmp",
            ImageFormat::Gif => "image/gif",
            ImageFormat::Webp => "image/webp",
        }
    }

    pub fn detect(data: &[u8]) -> Option<Self> {
        match data {
            [0xFF, 0xD8, 0xFF, ..] => Some(ImageFormat::Jpeg),
            [0x89, b'P', b'N', b'G', ..] => Some(ImageFormat::Png),
            [b'B', b'M', ..] => Some(ImageFormat::Bmp),
            [b'G', b'I', b'F', b'8', ..] => Some(ImageFormat::Gif),
            [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => {
                Some(ImageFormat::Webp)
            }
            _ => None,
        }
    }
}

impl Picture {
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    pub fn format(&self) -> Option<ImageFormat> {
        ImageFormat::detect(&self.data)
    }

    /// (width, height, bits per pixel) when they can be determined cheaply.
    pub fn dimensions(&self) -> Option<(u32, u32, u32)> {
        match self.format()? {
            ImageFormat::Png => png_dimensions(&self.data),
            ImageFormat::Jpeg => jpeg_dimensions(&self.data),
            _ => None,
        }
    }
}

fn png_dimensions(d: &[u8]) -> Option<(u32, u32, u32)> {
    if d.len() < 26 || &d[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(d[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(d[20..24].try_into().ok()?);
    let channels = match d[25] {
        0 | 3 => 1,
        2 => 3,
        4 => 2,
        6 => 4,
        _ => 1,
    };
    Some((w, h, d[24] as u32 * channels))
}

fn jpeg_dimensions(d: &[u8]) -> Option<(u32, u32, u32)> {
    let mut i = 2;
    while i + 4 <= d.len() {
        if d[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = d[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if (0xD0..=0xD9).contains(&marker) || marker == 0x01 {
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes([d[i + 2], d[i + 3]]) as usize;
        let is_sof = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_sof {
            let s = d.get(i + 4..i + 10)?;
            let precision = s[0] as u32;
            let h = u16::from_be_bytes([s[1], s[2]]) as u32;
            let w = u16::from_be_bytes([s[3], s[4]]) as u32;
            return Some((w, h, precision * s[5] as u32));
        }
        i += 2 + len;
    }
    None
}

fn non_empty(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

// ---------------------------------------------------------------- MP4 / iTunes

const DATA_UTF8: u32 = 1;
const DATA_IMPLICIT: u32 = 0;
const DATA_JPEG: u32 = 13;
const DATA_PNG: u32 = 14;
const DATA_BMP: u32 = 27;

fn data_atom(kind: u32, payload: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(payload.len() + 8);
    p.extend_from_slice(&kind.to_be_bytes());
    p.extend_from_slice(&0u32.to_be_bytes()); // locale
    p.extend_from_slice(payload);
    make_box(b"data", &p)
}

fn text_item(name: &[u8; 4], value: &str) -> Vec<u8> {
    make_box(name, &data_atom(DATA_UTF8, value.as_bytes()))
}

fn freeform_item(name: &str, value: &str) -> Vec<u8> {
    let mean = make_full_box(b"mean", 0, 0, b"com.apple.iTunes");
    let name = make_full_box(b"name", 0, 0, name.as_bytes());
    make_container(
        b"----",
        &[mean, name, data_atom(DATA_UTF8, value.as_bytes())],
    )
}

/// Builds a `udta` box holding iTunes style metadata.
pub(crate) fn mp4_udta(meta: &Metadata) -> Vec<u8> {
    let mut items: Vec<Vec<u8>> = Vec::new();
    let texts: [(&[u8; 4], &Option<String>); 9] = [
        (b"\xa9nam", &meta.title),
        (b"\xa9ART", &meta.artist),
        (b"aART", &meta.album_artist),
        (b"\xa9alb", &meta.album),
        (b"\xa9day", &meta.date),
        (b"\xa9gen", &meta.genre),
        (b"\xa9cmt", &meta.comment),
        (b"desc", &meta.description),
        (b"\xa9too", &meta.tool),
    ];
    for (name, v) in texts {
        if let Some(v) = non_empty(v) {
            items.push(text_item(name, v));
        }
    }
    if let Some((n, total)) = meta.track {
        let n = n.min(u16::MAX as u32) as u16;
        let total = total.unwrap_or(0).min(u16::MAX as u32) as u16;
        let mut p = vec![0u8, 0];
        p.extend_from_slice(&n.to_be_bytes());
        p.extend_from_slice(&total.to_be_bytes());
        p.extend_from_slice(&[0, 0]);
        items.push(make_box(b"trkn", &data_atom(DATA_IMPLICIT, &p)));
    }
    if let Some(pic) = &meta.cover {
        let kind = match pic.format() {
            Some(ImageFormat::Jpeg) => Some(DATA_JPEG),
            Some(ImageFormat::Png) => Some(DATA_PNG),
            Some(ImageFormat::Bmp) => Some(DATA_BMP),
            _ => None,
        };
        if let Some(kind) = kind {
            items.push(make_box(b"covr", &data_atom(kind, &pic.data)));
        }
    }
    for (k, v) in &meta.custom {
        if !k.is_empty() && !v.is_empty() {
            items.push(freeform_item(k, v));
        }
    }

    let mut hdlr = Vec::new();
    hdlr.extend_from_slice(&0u32.to_be_bytes()); // pre_defined
    hdlr.extend_from_slice(b"mdir");
    hdlr.extend_from_slice(b"appl");
    hdlr.extend_from_slice(&[0u8; 8]);
    hdlr.push(0); // empty name
    let hdlr = make_full_box(b"hdlr", 0, 0, &hdlr);
    let ilst = make_container(b"ilst", &items);
    let meta_box = make_full_box(b"meta", 0, 0, &[hdlr, ilst].concat());
    make_container(b"udta", &[meta_box])
}

// ---------------------------------------------------------------- FLAC

pub(crate) const FLAC_STREAMINFO: u8 = 0;
pub(crate) const FLAC_PADDING: u8 = 1;
pub(crate) const FLAC_VORBIS_COMMENT: u8 = 4;
pub(crate) const FLAC_PICTURE: u8 = 6;

/// Builds the payload of a VORBIS_COMMENT metadata block.
pub(crate) fn vorbis_comment(meta: &Metadata, vendor: &str) -> Vec<u8> {
    let mut fields: Vec<String> = Vec::new();
    let mut push = |k: &str, v: &Option<String>| {
        if let Some(v) = non_empty(v) {
            fields.push(format!("{k}={v}"));
        }
    };
    push("TITLE", &meta.title);
    push("ARTIST", &meta.artist);
    push("ALBUMARTIST", &meta.album_artist);
    push("ALBUM", &meta.album);
    push("DATE", &meta.date);
    push("GENRE", &meta.genre);
    push("COMMENT", &meta.comment);
    push("DESCRIPTION", &meta.description);
    push("ENCODER", &meta.tool);
    if let Some((n, total)) = meta.track {
        fields.push(format!("TRACKNUMBER={n}"));
        if let Some(t) = total {
            fields.push(format!("TRACKTOTAL={t}"));
            fields.push(format!("TOTALTRACKS={t}"));
        }
    }
    for (k, v) in &meta.custom {
        // Vorbis field names: printable ASCII 0x20..0x7D except '='.
        let key: String = k
            .chars()
            .filter(|c| (' '..='}').contains(c) && *c != '=')
            .collect::<String>()
            .to_ascii_uppercase();
        if !key.is_empty() && !v.is_empty() {
            fields.push(format!("{key}={v}"));
        }
    }

    let mut out = Vec::new();
    out.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    out.extend_from_slice(vendor.as_bytes());
    out.extend_from_slice(&(fields.len() as u32).to_le_bytes());
    for f in fields {
        out.extend_from_slice(&(f.len() as u32).to_le_bytes());
        out.extend_from_slice(f.as_bytes());
    }
    out
}

/// Builds the payload of a PICTURE metadata block (front cover).
pub(crate) fn flac_picture(pic: &Picture) -> Option<Vec<u8>> {
    let fmt = pic.format()?;
    let mime = fmt.mime().as_bytes();
    let (w, h, depth) = pic.dimensions().unwrap_or((0, 0, 0));
    let mut out = Vec::with_capacity(pic.data.len() + 64);
    out.extend_from_slice(&3u32.to_be_bytes()); // front cover
    out.extend_from_slice(&(mime.len() as u32).to_be_bytes());
    out.extend_from_slice(mime);
    out.extend_from_slice(&0u32.to_be_bytes()); // description
    out.extend_from_slice(&w.to_be_bytes());
    out.extend_from_slice(&h.to_be_bytes());
    out.extend_from_slice(&depth.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes()); // indexed colors
    out.extend_from_slice(&(pic.data.len() as u32).to_be_bytes());
    out.extend_from_slice(&pic.data);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_images() {
        let png = [
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0,
            0, 1, 0, 0, 0, 0, 200, 8, 2,
        ];
        let p = Picture::new(png.to_vec());
        assert_eq!(p.format(), Some(ImageFormat::Png));
        assert_eq!(p.dimensions(), Some((256, 200, 24)));

        // SOI, APP0 (len 4), SOF0
        let jpg = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xC0, 0x00, 0x11, 8, 0x01, 0x2C,
            0x02, 0x58, 3,
        ];
        let p = Picture::new(jpg.to_vec());
        assert_eq!(p.format(), Some(ImageFormat::Jpeg));
        assert_eq!(p.dimensions(), Some((600, 300, 24)));
    }

    #[test]
    fn vorbis_comment_layout() {
        let meta = Metadata {
            title: Some("标题".into()),
            artist: Some("  ".into()),
            track: Some((3, Some(10))),
            custom: vec![("bvid".into(), "BV1xx".into())],
            ..Default::default()
        };
        let vc = vorbis_comment(&meta, "dc");
        assert_eq!(&vc[0..4], &2u32.to_le_bytes());
        assert_eq!(&vc[4..6], b"dc");
        let count = u32::from_le_bytes(vc[6..10].try_into().unwrap());
        // TITLE, TRACKNUMBER, TRACKTOTAL, TOTALTRACKS, BVID (blank artist skipped)
        assert_eq!(count, 5);
        let text = String::from_utf8_lossy(&vc);
        assert!(text.contains("TITLE=标题"));
        assert!(text.contains("BVID=BV1xx"));
        assert!(!text.contains("ARTIST="));
    }
}
