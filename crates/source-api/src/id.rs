use std::fmt;

/// A source video id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum VideoId {
    Aid(u64),
    Bvid(String),
}

impl VideoId {
    /// Parses `BV1xx411c7mD`, `av170001` or a bare number (aid).
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if let Some(bv) = parse_bvid(s) {
            return Some(VideoId::Bvid(bv));
        }
        let digits = s
            .strip_prefix("av")
            .or_else(|| s.strip_prefix("AV"))
            .unwrap_or(s);
        if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
            return digits.parse().ok().map(VideoId::Aid);
        }
        None
    }

    /// Finds the first video id inside an arbitrary text such as an url.
    pub fn find_in(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        for i in 0..bytes.len() {
            if i + 12 <= bytes.len() && bytes[i..i + 2].eq_ignore_ascii_case(b"bv") {
                if let Some(bv) = parse_bvid(&text[i..i + 12]) {
                    let boundary = bytes.get(i + 12).is_none_or(|b| !b.is_ascii_alphanumeric());
                    if boundary {
                        return Some(VideoId::Bvid(bv));
                    }
                }
            }
        }
        // av ids only inside an url path: /video/av123
        let lower = text.to_ascii_lowercase();
        if let Some(pos) = lower.find("/video/av") {
            let digits: String = lower[pos + 9..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            return digits.parse().ok().map(VideoId::Aid);
        }
        None
    }

    /// The query parameter identifying this video.
    pub fn query_pair(&self) -> (String, String) {
        match self {
            VideoId::Aid(aid) => ("aid".into(), aid.to_string()),
            VideoId::Bvid(bvid) => ("bvid".into(), bvid.clone()),
        }
    }
}

fn parse_bvid(s: &str) -> Option<String> {
    if s.len() != 12 || !s.is_char_boundary(2) || !s[..2].eq_ignore_ascii_case("bv") {
        return None;
    }
    let rest = &s[2..];
    if rest.starts_with('1') && rest.bytes().all(|b| b.is_ascii_alphanumeric()) {
        Some(format!("BV{rest}"))
    } else {
        None
    }
}

impl fmt::Display for VideoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VideoId::Aid(id) => write!(f, "av{id}"),
            VideoId::Bvid(id) => f.write_str(id),
        }
    }
}

impl std::str::FromStr for VideoId {
    type Err = crate::Error;
    fn from_str(s: &str) -> crate::Result<Self> {
        VideoId::parse(s)
            .or_else(|| VideoId::find_in(s))
            .ok_or_else(|| crate::Error::InvalidId(s.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ids() {
        assert_eq!(
            VideoId::parse("BV1Vh4y1v7qn"),
            Some(VideoId::Bvid("BV1Vh4y1v7qn".into()))
        );
        assert_eq!(
            VideoId::parse("bv1Vh4y1v7qn"),
            Some(VideoId::Bvid("BV1Vh4y1v7qn".into()))
        );
        assert_eq!(VideoId::parse("av170001"), Some(VideoId::Aid(170001)));
        assert_eq!(VideoId::parse("170001"), Some(VideoId::Aid(170001)));
        assert_eq!(VideoId::parse("BV1Vh4y1v7q"), None);
        assert_eq!(VideoId::parse("hello"), None);
        assert_eq!(
            VideoId::find_in("https://www.example.com/video/BV1Vh4y1v7qn/?p=2&spm=x"),
            Some(VideoId::Bvid("BV1Vh4y1v7qn".into()))
        );
        assert_eq!(
            VideoId::find_in("https://m.example.com/video/av170001?p=1"),
            Some(VideoId::Aid(170001))
        );
        assert_eq!(
            VideoId::find_in("https://space.example.com/1/favlist"),
            None
        );
        assert_eq!(VideoId::Aid(1).to_string(), "av1");
        assert!("nope".parse::<VideoId>().is_err());
    }
}
