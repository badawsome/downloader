//! Cross platform file naming: sanitising and output templates.

use std::path::PathBuf;

use crate::track::Track;

/// Byte budget for one path component. Filesystems allow 255 bytes; the
/// rest is left for suffixes such as `.flac.tmp` or `.part.state.tmp`.
pub const MAX_COMPONENT_BYTES: usize = 200;

const WINDOWS_RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Makes `s` safe to use as a single file or directory name on Windows,
/// macOS and Linux. Forbidden characters are replaced by their full-width
/// look-alikes so titles stay readable (`AC/DC` → `AC／DC`).
pub fn sanitize_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = true; // also trims leading whitespace
    for c in s.chars() {
        let mapped = match c {
            '/' => '／',
            '\\' => '＼',
            ':' => '：',
            '*' => '＊',
            '?' => '？',
            '"' => '＂',
            '<' => '＜',
            '>' => '＞',
            '|' => '｜',
            c if c.is_control() => ' ',
            c if c.is_whitespace() => ' ',
            // zero width / BOM characters that confuse terminals and shells
            '\u{200B}'..='\u{200F}' | '\u{2028}'..='\u{202E}' | '\u{FEFF}' => continue,
            c => c,
        };
        if mapped == ' ' {
            if last_space {
                continue;
            }
            last_space = true;
        } else {
            last_space = false;
        }
        out.push(mapped);
    }

    // Byte budget, cut at a character boundary.
    if out.len() > MAX_COMPONENT_BYTES {
        let mut cut = MAX_COMPONENT_BYTES;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
    }

    // Windows strips trailing dots and spaces; a leading dot hides the file
    // on unix.
    let trimmed = out.trim_end_matches([' ', '.']).trim_start();
    let mut out = match trimmed.strip_prefix('.') {
        Some(rest) => format!("．{rest}"),
        None => trimmed.to_owned(),
    };
    if out.is_empty() {
        out.push('_');
    }
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    if WINDOWS_RESERVED.contains(&stem.as_str()) {
        out.insert(0, '_');
    }
    out
}

/// Renders an output template such as `{album}/{index} - {title}` into a
/// relative path. `/` in the template separates directories; values are
/// sanitised so they can never introduce extra directories.
///
/// Variables: `title`, `artist`, `album`, `collection`, `video_title`,
/// `part`, `bvid`, `aid`, `cid`, `page`, `index`, `total`, `date`, `year`,
/// `quality` is not available because names are fixed before downloading.
pub fn render_template(template: &str, track: &Track) -> PathBuf {
    let mut path = PathBuf::new();
    for component in template.split(['/', '\\']) {
        let rendered = render_component(component, track);
        let clean = sanitize_component(&rendered);
        if rendered.trim().is_empty() || clean == "_" && rendered.trim() != "_" {
            continue;
        }
        path.push(clean);
    }
    if path.as_os_str().is_empty() {
        path.push(sanitize_component(&format!(
            "{} - {}",
            track.artist, track.title
        )));
    }
    path
}

fn render_component(component: &str, track: &Track) -> String {
    let mut out = String::new();
    let mut rest = component;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let key = &after[..close];
                match variable(key, track) {
                    Some(v) => out.push_str(&v),
                    None => {
                        out.push('{');
                        out.push_str(key);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push_str(&rest[open..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn variable(key: &str, t: &Track) -> Option<String> {
    let width = t.total.unwrap_or(0).to_string().len().max(2);
    Some(match key {
        "title" => t.title.clone(),
        "artist" => t.artist.clone(),
        "album" => t.album.clone(),
        "collection" => t.collection.clone(),
        "video_title" => t.video_title.clone(),
        "part" => t.part.clone(),
        "bvid" => t.bvid.clone(),
        "aid" => t.aid.to_string(),
        "cid" => t.cid.to_string(),
        "page" => t.page.to_string(),
        "index" => format!("{:0width$}", t.index.unwrap_or(t.page)),
        "total" => t.total.map(|v| v.to_string()).unwrap_or_default(),
        "date" => format_date(t.pubdate).unwrap_or_default(),
        "year" => format_date(t.pubdate)
            .map(|d| d[..4].to_owned())
            .unwrap_or_default(),
        _ => return None,
    })
}

/// Formats a unix timestamp as `YYYY-MM-DD` in China Standard Time (UTC+8),
/// the time zone used by the source.
pub fn format_date(unix: i64) -> Option<String> {
    if unix <= 0 {
        return None;
    }
    let days = (unix + 8 * 3600).div_euclid(86_400);
    // Howard Hinnant's civil_from_days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::TrackKind;

    #[test]
    fn sanitize() {
        assert_eq!(
            sanitize_component("AC/DC: Back In Black?"),
            "AC／DC： Back In Black？"
        );
        assert_eq!(sanitize_component("  a \t\n b  "), "a b");
        assert_eq!(sanitize_component("name. . "), "name");
        assert_eq!(sanitize_component(".hidden"), "．hidden");
        assert_eq!(sanitize_component("con"), "_con");
        assert_eq!(sanitize_component("LPT1.txt"), "_LPT1.txt");
        assert_eq!(sanitize_component("CONSOLE"), "CONSOLE");
        assert_eq!(sanitize_component(""), "_");
        assert_eq!(sanitize_component("a\u{200B}b"), "ab");
        let long = "长".repeat(200);
        let s = sanitize_component(&long);
        assert!(s.len() <= MAX_COMPONENT_BYTES);
        assert!(s.chars().all(|c| c == '长'));
    }

    #[test]
    fn dates() {
        assert_eq!(format_date(0), None);
        assert_eq!(format_date(1_700_000_000).as_deref(), Some("2023-11-15"));
        // 2024-02-29 20:00 UTC is already March 1st in Beijing
        assert_eq!(format_date(1_709_236_800).as_deref(), Some("2024-03-01"));
    }

    fn track() -> Track {
        Track {
            kind: TrackKind::Season,
            bvid: "BV1xx411c7mD".into(),
            aid: 1,
            cid: 2,
            page: 1,
            title: "Song/Name".into(),
            video_title: "【翻唱】Song".into(),
            part: "P1".into(),
            artist: "UP".into(),
            album: "合集: 1".into(),
            album_artist: "UP".into(),
            index: Some(7),
            total: Some(120),
            pubdate: 1_700_000_000,
            cover_url: String::new(),
            description: String::new(),
            genre: String::new(),
            collection: "合集: 1".into(),
            bgm_lookup: true,
        }
    }

    #[test]
    fn templates() {
        let t = track();
        assert_eq!(
            render_template("{album}/{index} - {title}", &t),
            PathBuf::from("合集： 1").join("007 - Song／Name")
        );
        assert_eq!(
            render_template("{artist} - {title} [{bvid}] {year} {unknown}", &t),
            PathBuf::from("UP - Song／Name [BV1xx411c7mD] 2023 {unknown}")
        );
        // empty directory components are dropped
        let mut t2 = t.clone();
        t2.collection.clear();
        assert_eq!(
            render_template("{collection}/{title}", &t2),
            PathBuf::from("Song／Name")
        );
        assert_eq!(
            render_template("../{title}", &t),
            PathBuf::from("Song／Name")
        );
    }
}
