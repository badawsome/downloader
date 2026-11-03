# Release notes

## v0.2.0

Feature:
- audio only: download DASH audio streams, video tracks are never kept (progressive MP4 fallback is demuxed locally)
- quality: best anonymous quality (192K AAC, `try_look`), Hi-Res FLAC and Dolby Atmos when logged in; `--quality`, `--no-dolby`
- login: `dc login` (QR code), `dc logout`, `dc status`, `--cookie`, `--cookie-file`, `DC_COOKIE`
- metadata: title / artist / album / album artist / track number / date / genre / description / source url / BVID + embedded cover, for M4A (iTunes atoms) and FLAC (Vorbis comments)
- file names: templates (`--name`), cross-platform sanitising (full-width replacements, Windows reserved names, length limit), de-duplication
- downloads: parallel ranged chunks, global connection limit, per-chunk retries with backoff, idle timeout, mirror health tracking
- resume: interrupted downloads continue from the last finished chunk (`.part.state`), also after Ctrl-C
- inputs: video urls with `?p=`, b23.tv short links, collections (合集), favorites (收藏夹), multi-part videos (`--pages`)
- `dc info` shows planned file names and available audio qualities
- `--items` selects entries of a collection / favorites list (e.g. `1-10,97`)
- BGM titles: unwrap `发现《…》`, drop backing-track markers such as `(伴奏)`
- multi-part titles: drop numbering that duplicates the part index
- genre falls back to `tname_v2` / category id (the API often returns an empty `tname`)
- WBI request signing, buvid cookies, risk-control retry

Refract:
- crates: `bili` (API), `network-tools` (download engine), `media` (pure Rust MP4/FLAC muxing + tags), `bili-service` (pipeline), `dc` (CLI)
- no FFmpeg / OpenSSL / protoc needed; TLS via rustls

Fix:
- seasons with more than one section only downloaded the first section
- `.mp4` files with video were saved; only audio is written now

## v0.1.2

Fix: 
- bili: range download
- bili: async stream api
- cli: file `sync_all` before close

Refract:
- bili: impls struct

Feature:
- bili: music api

## v0.1.1

## v0.1.0