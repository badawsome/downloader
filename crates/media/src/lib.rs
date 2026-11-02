//! Pure Rust audio extraction for MP4 based media.
//!
//! * reads the audio track of fragmented (DASH `.m4s`) or progressive MP4
//!   files — video tracks are dropped;
//! * remuxes AAC / E-AC-3 (Dolby) / other codecs into a standard,
//!   non-fragmented, "fast start" `.m4a`;
//! * unwraps FLAC-in-MP4 (bilibili Hi-Res) into a native `.flac` stream;
//! * embeds metadata (iTunes atoms or Vorbis comments) and cover art.
//!
//! No FFmpeg or any other native library is needed.

mod bmff;
mod demux;
mod error;
mod mux;
mod tags;

use std::ffi::OsString;
use std::io::{BufReader, BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub use error::{Error, Result};
pub use tags::{ImageFormat, Metadata, Picture};

/// Audio codec of the extracted track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Codec {
    Aac,
    Ac3,
    Eac3,
    Flac,
    Alac,
    Opus,
    Other(String),
}

impl std::fmt::Display for Codec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Codec::Aac => f.write_str("AAC"),
            Codec::Ac3 => f.write_str("AC-3"),
            Codec::Eac3 => f.write_str("E-AC-3"),
            Codec::Flac => f.write_str("FLAC"),
            Codec::Alac => f.write_str("ALAC"),
            Codec::Opus => f.write_str("Opus"),
            Codec::Other(s) => f.write_str(s),
        }
    }
}

/// Container written by [`AudioExtractor::write`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    M4a,
    Flac,
}

impl OutputFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            OutputFormat::M4a => "m4a",
            OutputFormat::Flac => "flac",
        }
    }
}

/// The audio track of an MP4 file, ready to be written to a new container.
#[derive(Debug, Clone)]
pub struct AudioExtractor {
    track: demux::AudioTrack,
}

impl AudioExtractor {
    /// Parses the container structure. Media data is not read.
    pub fn open<R: Read + Seek>(input: &mut R) -> Result<Self> {
        Ok(Self {
            track: demux::read_audio_track(input)?,
        })
    }

    pub fn codec(&self) -> Codec {
        match &self.track.codec() {
            b"mp4a" => Codec::Aac,
            b"ac-3" => Codec::Ac3,
            b"ec-3" => Codec::Eac3,
            b"fLaC" => Codec::Flac,
            b"alac" => Codec::Alac,
            b"Opus" => Codec::Opus,
            other => Codec::Other(bmff::fourcc_str(other)),
        }
    }

    pub fn output_format(&self) -> OutputFormat {
        match self.codec() {
            Codec::Flac => OutputFormat::Flac,
            _ => OutputFormat::M4a,
        }
    }

    pub fn duration(&self) -> Duration {
        let ticks = self.track.total_duration();
        let ts = self.track.timescale as u64;
        Duration::from_secs(ticks / ts) + Duration::from_nanos((ticks % ts) * 1_000_000_000 / ts)
    }

    pub fn sample_count(&self) -> usize {
        self.track.samples.len()
    }

    /// Writes the audio track to `out` in [`Self::output_format`] with `meta`
    /// embedded. `input` must be the same stream passed to [`Self::open`].
    pub fn write<R: Read + Seek, W: Write>(
        &self,
        input: &mut R,
        out: &mut W,
        meta: &Metadata,
    ) -> Result<()> {
        match self.output_format() {
            OutputFormat::M4a => mux::write_m4a(&self.track, input, out, meta),
            OutputFormat::Flac => {
                let vendor = meta.tool.as_deref().unwrap_or("media-rs");
                mux::write_flac(&self.track, input, out, meta, vendor)
            }
        }
    }
}

/// Appends `.ext` to a path without touching dots already in the file name.
pub fn with_appended_extension(path: &Path, ext: &str) -> PathBuf {
    let mut s: OsString = path.as_os_str().to_owned();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

/// Result of [`extract_audio_file`].
#[derive(Debug, Clone)]
pub struct ExtractedFile {
    pub path: PathBuf,
    pub codec: Codec,
    pub duration: Duration,
}

/// Extracts the audio of the media file `input` into `<output_stem>.<ext>`
/// (extension chosen by codec).
///
/// The output is written to a temporary file first and atomically renamed,
/// so an interrupted run never leaves a half written result behind.
pub fn extract_audio_file(
    input: &Path,
    output_stem: &Path,
    meta: &Metadata,
) -> Result<ExtractedFile> {
    let mut reader = BufReader::with_capacity(256 * 1024, std::fs::File::open(input)?);
    let extractor = AudioExtractor::open(&mut reader)?;
    let target = with_appended_extension(output_stem, extractor.output_format().extension());
    let tmp = with_appended_extension(&target, "tmp");
    let res = (|| {
        let file = std::fs::File::create(&tmp)?;
        let mut writer = BufWriter::with_capacity(256 * 1024, file);
        extractor.write(&mut reader, &mut writer, meta)?;
        let file = writer.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, &target)?;
        Ok(ExtractedFile {
            path: target,
            codec: extractor.codec(),
            duration: extractor.duration(),
        })
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

#[cfg(test)]
mod tests;
