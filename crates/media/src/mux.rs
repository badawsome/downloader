//! Writers producing a self-contained, non fragmented `.m4a` or a native
//! `.flac` file from an [`AudioTrack`].

use std::io::{Read, Seek, SeekFrom, Write};

use crate::bmff::{find, make_box, make_container, make_full_box};
use crate::demux::{sample_entry_children, AudioTrack, Sample};
use crate::tags::{self, Metadata};
use crate::{Error, Result};

const MOVIE_TIMESCALE: u32 = 1000;
const SAMPLES_PER_CHUNK: usize = 32;
const UNITY_MATRIX: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];

fn scale(v: u64, from: u32, to: u32) -> u64 {
    (v as u128 * to as u128 / from.max(1) as u128) as u64
}

fn matrix() -> Vec<u8> {
    UNITY_MATRIX.iter().flat_map(|v| v.to_be_bytes()).collect()
}

fn mvhd(duration: u64, next_track_id: u32) -> Vec<u8> {
    let mut p = Vec::new();
    let v1 = duration > u32::MAX as u64;
    if v1 {
        p.extend_from_slice(&0u64.to_be_bytes());
        p.extend_from_slice(&0u64.to_be_bytes());
        p.extend_from_slice(&MOVIE_TIMESCALE.to_be_bytes());
        p.extend_from_slice(&duration.to_be_bytes());
    } else {
        p.extend_from_slice(&0u32.to_be_bytes());
        p.extend_from_slice(&0u32.to_be_bytes());
        p.extend_from_slice(&MOVIE_TIMESCALE.to_be_bytes());
        p.extend_from_slice(&(duration as u32).to_be_bytes());
    }
    p.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // rate 1.0
    p.extend_from_slice(&0x0100u16.to_be_bytes()); // volume 1.0
    p.extend_from_slice(&[0u8; 10]);
    p.extend_from_slice(&matrix());
    p.extend_from_slice(&[0u8; 24]);
    p.extend_from_slice(&next_track_id.to_be_bytes());
    make_full_box(b"mvhd", v1 as u8, 0, &p)
}

fn tkhd(track_id: u32, duration: u64) -> Vec<u8> {
    let mut p = Vec::new();
    let v1 = duration > u32::MAX as u64;
    if v1 {
        p.extend_from_slice(&[0u8; 16]);
        p.extend_from_slice(&track_id.to_be_bytes());
        p.extend_from_slice(&0u32.to_be_bytes());
        p.extend_from_slice(&duration.to_be_bytes());
    } else {
        p.extend_from_slice(&[0u8; 8]);
        p.extend_from_slice(&track_id.to_be_bytes());
        p.extend_from_slice(&0u32.to_be_bytes());
        p.extend_from_slice(&(duration as u32).to_be_bytes());
    }
    p.extend_from_slice(&[0u8; 8]);
    p.extend_from_slice(&0u16.to_be_bytes()); // layer
    p.extend_from_slice(&1u16.to_be_bytes()); // alternate group
    p.extend_from_slice(&0x0100u16.to_be_bytes()); // volume
    p.extend_from_slice(&0u16.to_be_bytes());
    p.extend_from_slice(&matrix());
    p.extend_from_slice(&0u32.to_be_bytes()); // width
    p.extend_from_slice(&0u32.to_be_bytes()); // height
    make_full_box(b"tkhd", v1 as u8, 0x3, &p) // enabled | in movie
}

fn mdhd(timescale: u32, duration: u64, language: u16) -> Vec<u8> {
    let mut p = Vec::new();
    let v1 = duration > u32::MAX as u64;
    if v1 {
        p.extend_from_slice(&[0u8; 16]);
        p.extend_from_slice(&timescale.to_be_bytes());
        p.extend_from_slice(&duration.to_be_bytes());
    } else {
        p.extend_from_slice(&[0u8; 8]);
        p.extend_from_slice(&timescale.to_be_bytes());
        p.extend_from_slice(&(duration as u32).to_be_bytes());
    }
    p.extend_from_slice(&language.to_be_bytes());
    p.extend_from_slice(&0u16.to_be_bytes());
    make_full_box(b"mdhd", v1 as u8, 0, &p)
}

fn default_dinf() -> Vec<u8> {
    let url = make_full_box(b"url ", 0, 1, &[]);
    let mut dref = 1u32.to_be_bytes().to_vec();
    dref.extend_from_slice(&url);
    make_container(b"dinf", &[make_full_box(b"dref", 0, 0, &dref)])
}

fn stts(samples: &[Sample]) -> Vec<u8> {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for s in samples {
        match runs.last_mut() {
            Some((count, delta)) if *delta == s.duration => *count += 1,
            _ => runs.push((1, s.duration)),
        }
    }
    let mut p = (runs.len() as u32).to_be_bytes().to_vec();
    for (count, delta) in runs {
        p.extend_from_slice(&count.to_be_bytes());
        p.extend_from_slice(&delta.to_be_bytes());
    }
    make_full_box(b"stts", 0, 0, &p)
}

fn stsc(sample_count: usize) -> Vec<u8> {
    let full = sample_count / SAMPLES_PER_CHUNK;
    let rest = sample_count % SAMPLES_PER_CHUNK;
    let mut entries: Vec<(u32, u32)> = Vec::new();
    if full > 0 {
        entries.push((1, SAMPLES_PER_CHUNK as u32));
    }
    if rest > 0 {
        entries.push((full as u32 + 1, rest as u32));
    }
    let mut p = (entries.len() as u32).to_be_bytes().to_vec();
    for (first, per) in entries {
        p.extend_from_slice(&first.to_be_bytes());
        p.extend_from_slice(&per.to_be_bytes());
        p.extend_from_slice(&1u32.to_be_bytes());
    }
    make_full_box(b"stsc", 0, 0, &p)
}

fn stsz(samples: &[Sample]) -> Vec<u8> {
    let first = samples[0].size;
    let mut p = Vec::new();
    if samples.iter().all(|s| s.size == first) {
        p.extend_from_slice(&first.to_be_bytes());
        p.extend_from_slice(&(samples.len() as u32).to_be_bytes());
    } else {
        p.extend_from_slice(&0u32.to_be_bytes());
        p.extend_from_slice(&(samples.len() as u32).to_be_bytes());
        for s in samples {
            p.extend_from_slice(&s.size.to_be_bytes());
        }
    }
    make_full_box(b"stsz", 0, 0, &p)
}

fn chunk_offsets(samples: &[Sample], data_start: u64, co64: bool) -> Vec<u8> {
    let mut p = (samples.len().div_ceil(SAMPLES_PER_CHUNK) as u32)
        .to_be_bytes()
        .to_vec();
    let mut off = data_start;
    for chunk in samples.chunks(SAMPLES_PER_CHUNK) {
        if co64 {
            p.extend_from_slice(&off.to_be_bytes());
        } else {
            p.extend_from_slice(&(off as u32).to_be_bytes());
        }
        off += chunk.iter().map(|s| s.size as u64).sum::<u64>();
    }
    make_full_box(if co64 { b"co64" } else { b"stco" }, 0, 0, &p)
}

fn moov(t: &AudioTrack, meta: &Metadata, data_start: u64, co64: bool) -> Vec<u8> {
    let media_duration = t.total_duration();
    let movie_duration = scale(media_duration, t.timescale, MOVIE_TIMESCALE);
    let track_id = t.track_id.max(1);

    let stbl = make_container(
        b"stbl",
        &[
            t.stsd.clone(),
            stts(&t.samples),
            stsc(t.samples.len()),
            stsz(&t.samples),
            chunk_offsets(&t.samples, data_start, co64),
        ],
    );
    let smhd = t
        .smhd
        .clone()
        .unwrap_or_else(|| make_full_box(b"smhd", 0, 0, &[0u8; 4]));
    let dinf = t.dinf.clone().unwrap_or_else(default_dinf);
    let minf = make_container(b"minf", &[smhd, dinf, stbl]);
    let mdia = make_container(
        b"mdia",
        &[
            mdhd(t.timescale, media_duration, t.language),
            t.hdlr.clone(),
            minf,
        ],
    );
    let trak = make_container(b"trak", &[tkhd(track_id, movie_duration), mdia]);
    make_container(
        b"moov",
        &[
            mvhd(movie_duration, track_id + 1),
            trak,
            tags::mp4_udta(meta),
        ],
    )
}

fn ftyp(codec: &[u8; 4]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(b"M4A ");
    p.extend_from_slice(&0x200u32.to_be_bytes());
    p.extend_from_slice(b"M4A ");
    p.extend_from_slice(b"isom");
    p.extend_from_slice(b"iso2");
    p.extend_from_slice(b"mp41");
    if codec != b"mp4a" {
        p.extend_from_slice(b"mp42");
    }
    make_box(b"ftyp", &p)
}

/// Copies the sample payloads (coalescing contiguous runs) to `out`.
fn copy_samples<R: Read + Seek, W: Write>(
    samples: &[Sample],
    input: &mut R,
    out: &mut W,
) -> Result<()> {
    let mut i = 0;
    while i < samples.len() {
        let start = samples[i].offset;
        let mut end = start + samples[i].size as u64;
        let mut j = i + 1;
        while j < samples.len() && samples[j].offset == end {
            end += samples[j].size as u64;
            j += 1;
        }
        input.seek(SeekFrom::Start(start))?;
        let copied = std::io::copy(&mut (&mut *input).take(end - start), out)?;
        if copied != end - start {
            return Err(Error::invalid(
                "unexpected end of file while copying samples",
            ));
        }
        i = j;
    }
    Ok(())
}

pub(crate) fn write_m4a<R: Read + Seek, W: Write>(
    t: &AudioTrack,
    input: &mut R,
    out: &mut W,
    meta: &Metadata,
) -> Result<()> {
    let data_len: u64 = t.samples.iter().map(|s| s.size as u64).sum();
    let ftyp = ftyp(&t.codec());
    let large_mdat = data_len + 8 > u32::MAX as u64;
    let mdat_header = if large_mdat { 16 } else { 8 };

    // The moov size does not depend on the offset values, so build it once
    // with placeholders to learn where the media data will start.
    let probe = moov(t, meta, 0, false);
    let co64 = ftyp.len() as u64 + probe.len() as u64 + mdat_header + data_len > u32::MAX as u64;
    let probe_len = if co64 {
        moov(t, meta, 0, true).len()
    } else {
        probe.len()
    };
    let data_start = (ftyp.len() + probe_len) as u64 + mdat_header;
    let moov = moov(t, meta, data_start, co64);
    debug_assert_eq!(moov.len(), probe_len);

    out.write_all(&ftyp)?;
    out.write_all(&moov)?;
    if large_mdat {
        out.write_all(&1u32.to_be_bytes())?;
        out.write_all(b"mdat")?;
        out.write_all(&(data_len + 16).to_be_bytes())?;
    } else {
        out.write_all(&((data_len + 8) as u32).to_be_bytes())?;
        out.write_all(b"mdat")?;
    }
    copy_samples(&t.samples, input, out)?;
    out.flush()?;
    Ok(())
}

fn flac_block_header(kind: u8, len: usize, last: bool) -> Result<[u8; 4]> {
    if len >= 1 << 24 {
        return Err(Error::Unsupported("FLAC metadata block too large".into()));
    }
    let len = len as u32;
    Ok([
        kind | if last { 0x80 } else { 0 },
        (len >> 16) as u8,
        (len >> 8) as u8,
        len as u8,
    ])
}

pub(crate) fn write_flac<R: Read + Seek, W: Write>(
    t: &AudioTrack,
    input: &mut R,
    out: &mut W,
    meta: &Metadata,
    vendor: &str,
) -> Result<()> {
    let children = sample_entry_children(&t.sample_entry)?;
    let dfla = find(children, b"dfLa")?
        .ok_or_else(|| Error::invalid("FLAC sample entry without 'dfLa' box"))?;
    let mut data = dfla
        .payload
        .get(4..)
        .ok_or_else(|| Error::invalid("dfLa too short"))?;

    let mut blocks: Vec<(u8, Vec<u8>)> = Vec::new();
    while data.len() >= 4 {
        let last = data[0] & 0x80 != 0;
        let kind = data[0] & 0x7f;
        let len = u32::from_be_bytes([0, data[1], data[2], data[3]]) as usize;
        let body = data
            .get(4..4 + len)
            .ok_or_else(|| Error::invalid("truncated FLAC metadata block"))?;
        blocks.push((kind, body.to_vec()));
        data = &data[4 + len..];
        if last {
            break;
        }
    }
    match blocks.first_mut() {
        Some((tags::FLAC_STREAMINFO, si)) if si.len() == 34 => {
            let rate = ((si[10] as u32) << 12) | ((si[11] as u32) << 4) | ((si[12] as u32) >> 4);
            let total = (((si[13] & 0x0f) as u64) << 32)
                | u32::from_be_bytes(si[14..18].try_into().unwrap()) as u64;
            if total == 0 && rate == t.timescale {
                let n = t.total_duration().min((1u64 << 36) - 1);
                si[13] = (si[13] & 0xf0) | ((n >> 32) as u8 & 0x0f);
                si[14..18].copy_from_slice(&(n as u32).to_be_bytes());
            }
        }
        _ => return Err(Error::invalid("FLAC STREAMINFO block missing")),
    }

    blocks.retain(|(k, _)| {
        !matches!(
            *k,
            tags::FLAC_PADDING | tags::FLAC_VORBIS_COMMENT | tags::FLAC_PICTURE
        )
    });
    blocks.push((
        tags::FLAC_VORBIS_COMMENT,
        tags::vorbis_comment(meta, vendor),
    ));
    if let Some(pic) = meta.cover.as_ref().and_then(tags::flac_picture) {
        if pic.len() < 1 << 24 {
            blocks.push((tags::FLAC_PICTURE, pic));
        }
    }
    // Leave room so tag editors can update metadata without a full rewrite.
    blocks.push((tags::FLAC_PADDING, vec![0u8; 4096]));

    out.write_all(b"fLaC")?;
    let n = blocks.len();
    for (i, (kind, body)) in blocks.iter().enumerate() {
        out.write_all(&flac_block_header(*kind, body.len(), i + 1 == n)?)?;
        out.write_all(body)?;
    }
    copy_samples(&t.samples, input, out)?;
    out.flush()?;
    Ok(())
}
