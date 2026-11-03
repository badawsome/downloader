//! Reads the audio track of an MP4 file — either fragmented (DASH `.m4s`, as
//! served by bilibili) or progressive (classic `.mp4` with video) — into a
//! flat list of samples pointing into the source file.

use std::io::{Read, Seek};

use crate::bmff::{self, find, iter_boxes, FourCC, Rd};
use crate::{Error, Result};

/// One encoded audio frame inside the source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Sample {
    pub offset: u64,
    pub size: u32,
    pub duration: u32,
}

/// Everything needed to rebuild the audio track in a new container.
#[derive(Debug, Clone)]
pub(crate) struct AudioTrack {
    pub track_id: u32,
    pub timescale: u32,
    pub language: u16,
    /// Complete `hdlr` box of the source track.
    pub hdlr: Vec<u8>,
    /// Complete `smhd` box, if present.
    pub smhd: Option<Vec<u8>>,
    /// Complete `dinf` box, if present.
    pub dinf: Option<Vec<u8>>,
    /// Complete `stsd` box (sample descriptions).
    pub stsd: Vec<u8>,
    /// Complete first sample entry box inside `stsd`.
    pub sample_entry: Vec<u8>,
    pub samples: Vec<Sample>,
}

impl AudioTrack {
    pub fn codec(&self) -> FourCC {
        self.sample_entry[4..8].try_into().unwrap()
    }

    pub fn total_duration(&self) -> u64 {
        self.samples.iter().map(|s| s.duration as u64).sum()
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct TrexDefaults {
    duration: u32,
    size: u32,
}

struct ParsedTrak<'a> {
    track_id: u32,
    timescale: u32,
    language: u16,
    handler: FourCC,
    hdlr: &'a [u8],
    smhd: Option<&'a [u8]>,
    dinf: Option<&'a [u8]>,
    stbl: Option<&'a [u8]>,
}

pub(crate) fn read_audio_track<R: Read + Seek>(r: &mut R) -> Result<AudioTrack> {
    let top = bmff::scan_top_level(r)?;
    let moov_box = top
        .iter()
        .find(|b| &b.typ == b"moov")
        .ok_or_else(|| Error::invalid("no 'moov' box; not an MP4 file"))?;
    let moov = bmff::read_payload(r, moov_box)?;

    // --- locate the audio track
    let mut chosen: Option<ParsedTrak> = None;
    for b in iter_boxes(&moov) {
        let b = b?;
        if &b.typ == b"trak" {
            let t = parse_trak(b.payload)?;
            if &t.handler == b"soun" {
                chosen = Some(t);
                break;
            }
        }
    }
    let trak = chosen.ok_or(Error::NoAudioTrack)?;

    let stbl = trak
        .stbl
        .ok_or_else(|| Error::invalid("audio track has no 'stbl'"))?;
    let stsd = find(stbl, b"stsd")?.ok_or_else(|| Error::invalid("audio track has no 'stsd'"))?;
    let sample_entry = {
        let mut rd = Rd::new(stsd.payload);
        rd.full_header()?;
        let count = rd.u32()?;
        if count == 0 {
            return Err(Error::invalid("empty sample description"));
        }
        let entry = iter_boxes(&stsd.payload[8..])
            .next()
            .ok_or_else(|| Error::invalid("missing sample entry"))??;
        if &entry.typ == b"enca" {
            return Err(Error::Unsupported("encrypted audio track".into()));
        }
        entry.full.to_vec()
    };

    // --- progressive samples (sample tables)
    let mut samples = samples_from_stbl(stbl)?;

    // --- fragmented samples (moof/mdat pairs)
    let trex = find_trex(&moov, trak.track_id)?;
    for b in top.iter().filter(|b| &b.typ == b"moof") {
        let moof = bmff::read_payload(r, b)?;
        samples_from_moof(&moof, b.offset, trak.track_id, trex, &mut samples)?;
    }

    let file_len = top.last().map(|b| b.offset + b.size).unwrap_or(0);
    for s in &samples {
        if s.offset + s.size as u64 > file_len {
            return Err(Error::invalid("sample data points outside of file"));
        }
    }
    if samples.is_empty() {
        return Err(Error::invalid("audio track contains no samples"));
    }

    Ok(AudioTrack {
        track_id: trak.track_id,
        timescale: trak.timescale,
        language: trak.language,
        hdlr: trak.hdlr.to_vec(),
        smhd: trak.smhd.map(<[u8]>::to_vec),
        dinf: trak.dinf.map(<[u8]>::to_vec),
        stsd: stsd.full.to_vec(),
        sample_entry,
        samples,
    })
}

fn parse_trak(trak: &[u8]) -> Result<ParsedTrak<'_>> {
    let tkhd = find(trak, b"tkhd")?.ok_or_else(|| Error::invalid("trak without tkhd"))?;
    let track_id = {
        let mut rd = Rd::new(tkhd.payload);
        let (v, _) = rd.full_header()?;
        rd.skip(if v == 1 { 16 } else { 8 })?;
        rd.u32()?
    };
    let mdia = find(trak, b"mdia")?.ok_or_else(|| Error::invalid("trak without mdia"))?;
    let mdhd = find(mdia.payload, b"mdhd")?.ok_or_else(|| Error::invalid("mdia without mdhd"))?;
    let (timescale, language) = {
        let mut rd = Rd::new(mdhd.payload);
        let (v, _) = rd.full_header()?;
        if v == 1 {
            rd.skip(16)?;
            let ts = rd.u32()?;
            rd.skip(8)?;
            (ts, rd.u16()?)
        } else {
            rd.skip(8)?;
            let ts = rd.u32()?;
            rd.skip(4)?;
            (ts, rd.u16()?)
        }
    };
    if timescale == 0 {
        return Err(Error::invalid("track timescale is 0"));
    }
    let hdlr = find(mdia.payload, b"hdlr")?.ok_or_else(|| Error::invalid("mdia without hdlr"))?;
    let handler: FourCC = {
        let mut rd = Rd::new(hdlr.payload);
        rd.full_header()?;
        rd.skip(4)?;
        rd.bytes(4)?.try_into().unwrap()
    };
    let minf = find(mdia.payload, b"minf")?;
    let (smhd, dinf, stbl) = match minf {
        Some(minf) => (
            find(minf.payload, b"smhd")?.map(|b| b.full),
            find(minf.payload, b"dinf")?.map(|b| b.full),
            find(minf.payload, b"stbl")?.map(|b| b.payload),
        ),
        None => (None, None, None),
    };
    Ok(ParsedTrak {
        track_id,
        timescale,
        language,
        handler,
        hdlr: hdlr.full,
        smhd,
        dinf,
        stbl,
    })
}

fn find_trex(moov: &[u8], track_id: u32) -> Result<TrexDefaults> {
    let Some(mvex) = find(moov, b"mvex")? else {
        return Ok(TrexDefaults::default());
    };
    for b in iter_boxes(mvex.payload) {
        let b = b?;
        if &b.typ != b"trex" {
            continue;
        }
        let mut rd = Rd::new(b.payload);
        rd.full_header()?;
        let id = rd.u32()?;
        let _sdi = rd.u32()?;
        let duration = rd.u32()?;
        let size = rd.u32()?;
        if id == track_id {
            return Ok(TrexDefaults { duration, size });
        }
    }
    Ok(TrexDefaults::default())
}

fn samples_from_stbl(stbl: &[u8]) -> Result<Vec<Sample>> {
    // sizes
    let Some(stsz) = find(stbl, b"stsz")? else {
        if find(stbl, b"stz2")?.is_some() {
            return Err(Error::Unsupported("compact sample sizes (stz2)".into()));
        }
        return Ok(Vec::new());
    };
    let sizes: Vec<u32> = {
        let mut rd = Rd::new(stsz.payload);
        rd.full_header()?;
        let fixed = rd.u32()?;
        let count = rd.u32()? as usize;
        if fixed != 0 {
            vec![fixed; count]
        } else {
            (0..count).map(|_| rd.u32()).collect::<Result<_>>()?
        }
    };
    if sizes.is_empty() {
        return Ok(Vec::new());
    }

    // chunk offsets
    let offsets: Vec<u64> = if let Some(stco) = find(stbl, b"stco")? {
        let mut rd = Rd::new(stco.payload);
        rd.full_header()?;
        let n = rd.u32()?;
        (0..n)
            .map(|_| rd.u32().map(u64::from))
            .collect::<Result<_>>()?
    } else if let Some(co64) = find(stbl, b"co64")? {
        let mut rd = Rd::new(co64.payload);
        rd.full_header()?;
        let n = rd.u32()?;
        (0..n).map(|_| rd.u64()).collect::<Result<_>>()?
    } else {
        return Err(Error::invalid("sample table without chunk offsets"));
    };

    // sample to chunk
    let stsc: Vec<(u32, u32)> = {
        let b = find(stbl, b"stsc")?.ok_or_else(|| Error::invalid("missing stsc"))?;
        let mut rd = Rd::new(b.payload);
        rd.full_header()?;
        let n = rd.u32()?;
        (0..n)
            .map(|_| {
                let first = rd.u32()?;
                let per = rd.u32()?;
                rd.u32()?;
                Ok((first, per))
            })
            .collect::<Result<_>>()?
    };

    // durations
    let mut durations: Vec<u32> = Vec::with_capacity(sizes.len());
    if let Some(stts) = find(stbl, b"stts")? {
        let mut rd = Rd::new(stts.payload);
        rd.full_header()?;
        let n = rd.u32()?;
        for _ in 0..n {
            let count = rd.u32()?;
            let delta = rd.u32()?;
            for _ in 0..count {
                if durations.len() >= sizes.len() {
                    break;
                }
                durations.push(delta);
            }
        }
    }
    let last = durations.last().copied().unwrap_or(0);
    durations.resize(sizes.len(), last);

    let mut samples = Vec::with_capacity(sizes.len());
    let mut idx = 0usize;
    'chunks: for (ci, &chunk_off) in offsets.iter().enumerate() {
        let chunk_no = ci as u32 + 1;
        let per = stsc
            .iter()
            .rev()
            .find(|(first, _)| *first <= chunk_no)
            .map(|(_, per)| *per)
            .ok_or_else(|| Error::invalid("invalid stsc table"))?;
        let mut off = chunk_off;
        for _ in 0..per {
            if idx >= sizes.len() {
                break 'chunks;
            }
            samples.push(Sample {
                offset: off,
                size: sizes[idx],
                duration: durations[idx],
            });
            off += sizes[idx] as u64;
            idx += 1;
        }
    }
    if idx != sizes.len() {
        return Err(Error::invalid(
            "sample table describes fewer samples than stsz",
        ));
    }
    Ok(samples)
}

fn samples_from_moof(
    moof: &[u8],
    moof_offset: u64,
    track_id: u32,
    trex: TrexDefaults,
    out: &mut Vec<Sample>,
) -> Result<()> {
    for traf in iter_boxes(moof) {
        let traf = traf?;
        if &traf.typ != b"traf" {
            continue;
        }
        let tfhd =
            find(traf.payload, b"tfhd")?.ok_or_else(|| Error::invalid("traf without tfhd"))?;
        let mut rd = Rd::new(tfhd.payload);
        let (_, flags) = rd.full_header()?;
        if rd.u32()? != track_id {
            continue;
        }
        let base = if flags & 0x01 != 0 {
            rd.u64()?
        } else {
            moof_offset
        };
        if flags & 0x02 != 0 {
            rd.u32()?;
        }
        let default_duration = if flags & 0x08 != 0 {
            rd.u32()?
        } else {
            trex.duration
        };
        let default_size = if flags & 0x10 != 0 {
            rd.u32()?
        } else {
            trex.size
        };

        let mut cursor = base;
        for trun in iter_boxes(traf.payload) {
            let trun = trun?;
            if &trun.typ != b"trun" {
                continue;
            }
            let mut rd = Rd::new(trun.payload);
            let (_, tf) = rd.full_header()?;
            let count = rd.u32()?;
            if tf & 0x01 != 0 {
                let data_offset = rd.u32()? as i32 as i64;
                cursor = u64::try_from(base as i64 + data_offset)
                    .map_err(|_| Error::invalid("negative trun data offset"))?;
            }
            if tf & 0x04 != 0 {
                rd.u32()?;
            }
            for _ in 0..count {
                let duration = if tf & 0x100 != 0 {
                    rd.u32()?
                } else {
                    default_duration
                };
                let size = if tf & 0x200 != 0 {
                    rd.u32()?
                } else {
                    default_size
                };
                if tf & 0x400 != 0 {
                    rd.u32()?;
                }
                if tf & 0x800 != 0 {
                    rd.u32()?;
                }
                out.push(Sample {
                    offset: cursor,
                    size,
                    duration,
                });
                cursor += size as u64;
            }
        }
    }
    Ok(())
}

/// Returns the child boxes of an audio sample entry (after the fixed fields).
pub(crate) fn sample_entry_children(entry: &[u8]) -> Result<&[u8]> {
    // 8 byte header, 6 reserved, 2 data reference index, then the
    // AudioSampleEntry fields: version(2) revision(2) vendor(4) channels(2)
    // samplesize(2) compression id(2) packet size(2) sample rate(4).
    if entry.len() < 36 {
        return Err(Error::invalid("audio sample entry too short"));
    }
    let version = u16::from_be_bytes([entry[16], entry[17]]);
    let extra = match version {
        1 => 16,
        2 => 36,
        _ => 0,
    };
    entry
        .get(36 + extra..)
        .ok_or_else(|| Error::invalid("audio sample entry too short"))
}
