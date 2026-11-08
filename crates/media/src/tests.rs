use std::io::Cursor;
use std::path::PathBuf;
use std::process::Command;

use super::*;
use crate::bmff::{find_path, iter_boxes, make_box, make_container, make_full_box};

// ------------------------------------------------------------ fixture builder

fn audio_entry(kind: &[u8; 4], rate: u32, children: &[Vec<u8>]) -> Vec<u8> {
    let mut p = vec![0u8; 6];
    p.extend_from_slice(&1u16.to_be_bytes()); // data reference index
    p.extend_from_slice(&[0u8; 8]);
    p.extend_from_slice(&2u16.to_be_bytes()); // channels
    p.extend_from_slice(&16u16.to_be_bytes()); // sample size
    p.extend_from_slice(&[0u8; 4]);
    p.extend_from_slice(&(rate << 16).to_be_bytes());
    p.extend_from_slice(&children.concat());
    make_box(kind, &p)
}

fn flac_entry(rate: u32) -> Vec<u8> {
    let mut si = vec![0u8; 34];
    si[0..2].copy_from_slice(&4096u16.to_be_bytes());
    si[2..4].copy_from_slice(&4096u16.to_be_bytes());
    // 20 bit sample rate, 3 bit channels-1 (=1), 5 bit bps-1 (=15), 36 bit total = 0
    si[10] = (rate >> 12) as u8;
    si[11] = (rate >> 4) as u8;
    si[12] = ((rate & 0xf) << 4) as u8 | (1 << 1);
    si[13] = 0xF0;
    let mut dfla = vec![0x80, 0, 0, 34];
    dfla.extend_from_slice(&si);
    audio_entry(b"fLaC", rate, &[make_full_box(b"dfLa", 0, 0, &dfla)])
}

fn aac_entry(rate: u32) -> Vec<u8> {
    audio_entry(b"mp4a", rate, &[make_full_box(b"esds", 0, 0, &[3, 0, 0])])
}

/// Builds a DASH-like fragmented MP4 with a video-less audio track whose
/// samples are `frames` (payload, duration), split across `fragments`.
fn fragmented(
    entry: Vec<u8>,
    timescale: u32,
    frames: &[(Vec<u8>, u32)],
    fragments: usize,
) -> Vec<u8> {
    let empty_table = |t: &[u8; 4], extra: usize| make_full_box(t, 0, 0, &vec![0u8; 4 + extra]);
    let mut stsd = 1u32.to_be_bytes().to_vec();
    stsd.extend_from_slice(&entry);
    let stbl = make_container(
        b"stbl",
        &[
            make_full_box(b"stsd", 0, 0, &stsd),
            empty_table(b"stts", 0),
            empty_table(b"stsc", 0),
            empty_table(b"stsz", 4),
            empty_table(b"stco", 0),
        ],
    );
    let mut mdhd = vec![0u8; 8];
    mdhd.extend_from_slice(&timescale.to_be_bytes());
    mdhd.extend_from_slice(&0u32.to_be_bytes());
    mdhd.extend_from_slice(&0x55C4u16.to_be_bytes());
    mdhd.extend_from_slice(&[0, 0]);
    let mut hdlr = vec![0u8; 4];
    hdlr.extend_from_slice(b"soun");
    hdlr.extend_from_slice(&[0u8; 13]);
    let mut tkhd = vec![0u8; 8];
    tkhd.extend_from_slice(&7u32.to_be_bytes()); // track id 7
    tkhd.extend_from_slice(&[0u8; 68]);
    let trak = make_container(
        b"trak",
        &[
            make_full_box(b"tkhd", 0, 3, &tkhd),
            make_container(
                b"mdia",
                &[
                    make_full_box(b"mdhd", 0, 0, &mdhd),
                    make_full_box(b"hdlr", 0, 0, &hdlr),
                    make_container(b"minf", &[make_full_box(b"smhd", 0, 0, &[0; 4]), stbl]),
                ],
            ),
        ],
    );
    let mut trex = 7u32.to_be_bytes().to_vec();
    trex.extend_from_slice(&[0, 0, 0, 1]);
    trex.extend_from_slice(&[0u8; 12]);
    let moov = make_container(
        b"moov",
        &[
            make_full_box(b"mvhd", 0, 0, &[0u8; 96]),
            trak,
            make_container(b"mvex", &[make_full_box(b"trex", 0, 0, &trex)]),
        ],
    );

    let mut out = make_box(b"ftyp", b"iso5\0\0\0\x01iso6mp41");
    out.extend_from_slice(&moov);
    out.extend_from_slice(&make_box(b"sidx", &[0u8; 12]));

    let per = frames.len().div_ceil(fragments);
    for (seq, group) in frames.chunks(per).enumerate() {
        let build = |data_offset: u32| {
            let mut tfhd = 7u32.to_be_bytes().to_vec();
            let _ = &mut tfhd;
            let mut trun = (group.len() as u32).to_be_bytes().to_vec();
            trun.extend_from_slice(&data_offset.to_be_bytes());
            for (data, dur) in group {
                trun.extend_from_slice(&dur.to_be_bytes());
                trun.extend_from_slice(&(data.len() as u32).to_be_bytes());
            }
            make_container(
                b"moof",
                &[
                    make_full_box(b"mfhd", 0, 0, &(seq as u32 + 1).to_be_bytes()),
                    make_container(
                        b"traf",
                        &[
                            make_full_box(b"tfhd", 0, 0x20000, &tfhd),
                            make_full_box(b"tfdt", 0, 0, &[0u8; 4]),
                            make_full_box(b"trun", 0, 0x301, &trun),
                        ],
                    ),
                ],
            )
        };
        let moof_len = build(0).len() as u32;
        out.extend_from_slice(&build(moof_len + 8));
        let payload: Vec<u8> = group.iter().flat_map(|(d, _)| d.clone()).collect();
        out.extend_from_slice(&make_box(b"mdat", &payload));
    }
    out
}

fn frames(n: usize, dur: u32) -> Vec<(Vec<u8>, u32)> {
    (0..n)
        .map(|i| ((0..(10 + i % 7)).map(|b| (i * 31 + b) as u8).collect(), dur))
        .collect()
}

fn sample_meta() -> Metadata {
    Metadata {
        title: Some("一样的月光".into()),
        artist: Some("UP主".into()),
        album: Some("合集".into()),
        album_artist: Some("UP主".into()),
        date: Some("2024-05-01".into()),
        genre: Some("翻唱".into()),
        comment: Some("https://www.example.com/video/BV1xx411c7mD".into()),
        description: Some("desc".into()),
        track: Some((2, Some(12))),
        cover: Some(Picture::new(vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 2, 0xFF, 0xD9])),
        custom: vec![("BVID".into(), "BV1xx411c7mD".into())],
        tool: Some("dc-test".into()),
    }
}

// ------------------------------------------------------------ pure tests

#[test]
fn remux_fragmented_aac_to_m4a() {
    let fr = frames(100, 1024);
    let src = fragmented(aac_entry(48000), 48000, &fr, 7);
    let mut input = Cursor::new(src);
    let ex = AudioExtractor::open(&mut input).unwrap();
    assert_eq!(ex.codec(), Codec::Aac);
    assert_eq!(ex.output_format(), OutputFormat::M4a);
    assert_eq!(ex.sample_count(), 100);
    assert_eq!(ex.duration().as_millis(), 100 * 1024 * 1000 / 48000);

    let mut out = Vec::new();
    ex.write(&mut input, &mut out, &sample_meta()).unwrap();

    // top level layout: ftyp, moov, mdat (fast start)
    let top: Vec<_> = iter_boxes(&out).map(|b| b.unwrap().typ).collect();
    assert_eq!(top, vec![*b"ftyp", *b"moov", *b"mdat"]);
    let moov = find_path(&out, &[b"moov"]).unwrap().unwrap().payload;
    assert!(find_path(moov, &[b"mvex"]).unwrap().is_none());
    let ilst = find_path(moov, &[b"udta", b"meta"])
        .unwrap()
        .unwrap()
        .payload;
    let ilst = find_path(&ilst[4..], &[b"ilst"]).unwrap().unwrap().payload;
    let names: Vec<_> = iter_boxes(ilst).map(|b| b.unwrap().typ).collect();
    for n in [
        b"\xa9nam", b"\xa9ART", b"aART", b"\xa9alb", b"trkn", b"covr", b"----",
    ] {
        assert!(
            names.contains(n),
            "missing {:?}",
            String::from_utf8_lossy(n)
        );
    }

    // re-read the output: the samples must be identical and in order
    let mut again = Cursor::new(out);
    let ex2 = AudioExtractor::open(&mut again).unwrap();
    assert_eq!(ex2.sample_count(), 100);
    assert_eq!(ex2.duration(), ex.duration());
    let data = again.into_inner();
    for (s, (payload, dur)) in ex2.track.samples.iter().zip(&fr) {
        assert_eq!(
            &data[s.offset as usize..][..s.size as usize],
            payload.as_slice()
        );
        assert_eq!(s.duration, *dur);
    }
}

#[test]
fn unwrap_flac_in_mp4() {
    let fr = frames(33, 4096);
    let src = fragmented(flac_entry(44100), 44100, &fr, 3);
    let mut input = Cursor::new(src);
    let ex = AudioExtractor::open(&mut input).unwrap();
    assert_eq!(ex.codec(), Codec::Flac);
    assert_eq!(ex.output_format(), OutputFormat::Flac);
    let mut out = Vec::new();
    ex.write(&mut input, &mut out, &sample_meta()).unwrap();

    assert_eq!(&out[0..4], b"fLaC");
    let mut pos = 4;
    let mut kinds = Vec::new();
    loop {
        let hdr = &out[pos..pos + 4];
        let len = u32::from_be_bytes([0, hdr[1], hdr[2], hdr[3]]) as usize;
        kinds.push(hdr[0] & 0x7f);
        if hdr[0] & 0x7f == 0 {
            let si = &out[pos + 4..pos + 4 + 34];
            let total = (((si[13] & 0x0f) as u64) << 32)
                | u32::from_be_bytes(si[14..18].try_into().unwrap()) as u64;
            assert_eq!(total, 33 * 4096);
        }
        pos += 4 + len;
        if hdr[0] & 0x80 != 0 {
            break;
        }
    }
    assert_eq!(kinds, vec![0, 4, 6, 1]);
    let expected: Vec<u8> = fr.iter().flat_map(|(d, _)| d.clone()).collect();
    assert_eq!(&out[pos..], expected.as_slice());
}

#[test]
fn rejects_non_mp4() {
    let mut input = Cursor::new(b"FLV\x01\x05\0\0\0\x09".to_vec());
    assert!(AudioExtractor::open(&mut input).is_err());
    let mut input = Cursor::new(Vec::new());
    assert!(AudioExtractor::open(&mut input).is_err());
}

#[test]
fn appended_extension_keeps_dots() {
    let p = with_appended_extension(Path::new("dir/A. B v1.0"), "m4a");
    assert_eq!(p, PathBuf::from("dir/A. B v1.0.m4a"));
}

// ------------------------------------------------------------ ffmpeg based
// These validate the output against a real decoder and are skipped when
// ffmpeg is not installed.

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().is_ok()
        && Command::new("ffprobe").arg("-version").output().is_ok()
}

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("media-test-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ffmpeg(args: &[&str]) -> bool {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .output()
        .unwrap();
    if !out.status.success() {
        eprintln!("ffmpeg failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    out.status.success()
}

fn probe(path: &Path) -> (String, f64, usize, usize) {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_name,codec_type:stream_disposition=attached_pic:format=duration",
            "-of",
            "default=nw=1",
        ])
        .arg(path)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let codec = text
        .lines()
        .find_map(|l| l.strip_prefix("codec_name="))
        .unwrap_or_default()
        .to_string();
    let dur = text
        .lines()
        .find_map(|l| l.strip_prefix("duration="))
        .and_then(|d| d.parse().ok())
        .unwrap_or(0.0);
    // embedded cover art shows up as an "attached picture" video stream
    let pics = text
        .lines()
        .filter(|l| *l == "DISPOSITION:attached_pic=1")
        .count();
    let streams = text
        .lines()
        .filter(|l| l.starts_with("codec_type="))
        .count()
        - pics;
    (codec, dur, streams, pics)
}

fn decodes_cleanly(path: &Path) -> bool {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:a", "-f", "null", "-"])
        .output()
        .unwrap();
    out.status.success() && out.stderr.is_empty()
}

fn ffmpeg_case(name: &str, gen: &[&str], expect_codec: &str, expect_ext: &str) {
    if !have_ffmpeg() {
        eprintln!("ffmpeg not found, skipping {name}");
        return;
    }
    let dir = tmpdir(name);
    let src = dir.join("source.m4s");
    let mut args = gen.to_vec();
    let src_s = src.to_str().unwrap().to_string();
    args.push(&src_s);
    if !ffmpeg(&args) {
        eprintln!("ffmpeg cannot produce {name} fixture, skipping");
        return;
    }
    let cover = dir.join("cover.jpg");
    let cover_s = cover.to_str().unwrap();
    assert!(ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "color=c=red:s=64x48",
        "-frames:v",
        "1",
        cover_s
    ]));
    let mut meta = sample_meta();
    meta.cover = Some(Picture::new(std::fs::read(&cover).unwrap()));
    assert_eq!(
        meta.cover.as_ref().unwrap().dimensions(),
        Some((64, 48, 24))
    );

    let written = extract_audio_file(&src, &dir.join("out. 测试"), &meta).unwrap();
    assert!((written.duration.as_secs_f64() - 3.0).abs() < 0.2);
    let written = written.path;
    assert_eq!(written.extension().unwrap(), expect_ext);
    let (codec, dur, streams, pics) = probe(&written);
    assert_eq!(codec, expect_codec);
    assert_eq!(streams, 1, "only the audio stream must remain");
    assert_eq!(pics, 1, "cover art must be embedded");
    assert!((dur - 3.0).abs() < 0.2, "duration {dur}");
    assert!(
        decodes_cleanly(&written),
        "decode errors in {}",
        written.display()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

const SINE: [&str; 4] = ["-f", "lavfi", "-i", "sine=frequency=440:duration=3"];
const DASH_FLAGS: [&str; 4] = [
    "-f",
    "mp4",
    "-movflags",
    "+frag_keyframe+empty_moov+default_base_moof+dash",
];

#[test]
fn ffmpeg_fragmented_aac() {
    let args = [
        &SINE[..],
        &["-c:a", "aac", "-b:a", "128k", "-frag_duration", "500000"],
        &DASH_FLAGS[..],
    ]
    .concat();
    ffmpeg_case("aac", &args, "aac", "m4a");
}

#[test]
fn ffmpeg_fragmented_flac() {
    let args = [
        &SINE[..],
        &["-c:a", "flac", "-strict", "-2", "-frag_duration", "500000"],
        &DASH_FLAGS[..],
    ]
    .concat();
    ffmpeg_case("flac", &args, "flac", "flac");
}

#[test]
fn ffmpeg_fragmented_eac3() {
    // ffmpeg cannot write an empty moov for E-AC-3; the first fragment then
    // lives in the moov sample tables, which also covers mixed files.
    let args = [
        &SINE[..],
        &["-c:a", "eac3", "-b:a", "384k", "-frag_duration", "500000"],
        &["-f", "mp4", "-movflags", "+frag_keyframe+default_base_moof"],
    ]
    .concat();
    ffmpeg_case("eac3", &args, "eac3", "m4a");
}

#[test]
fn ffmpeg_progressive_mp4_with_video() {
    let args = [
        "-f",
        "lavfi",
        "-i",
        "testsrc=duration=3:size=160x120:rate=25",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=3",
        "-c:v",
        "mpeg4",
        "-c:a",
        "aac",
        "-shortest",
        "-f",
        "mp4",
    ];
    ffmpeg_case("progressive", &args, "aac", "m4a");
}
