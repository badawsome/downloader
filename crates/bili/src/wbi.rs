//! WBI request signing (`w_rid` / `wts` query parameters).

use md5::{Digest, Md5};

const MIXIN_KEY_ENC_TAB: [usize; 64] = [
    46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42, 19, 29,
    28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4, 22, 25,
    54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 34, 44, 52,
];

/// Extracts the key from a `wbi_img` url: `https://.../7cd08494...077c.png`.
pub fn key_from_url(url: &str) -> &str {
    let name = url.rsplit('/').next().unwrap_or(url);
    name.split('.').next().unwrap_or(name)
}

/// Derives the 32 character mixin key from `img_key` + `sub_key`.
pub fn mixin_key(img_key: &str, sub_key: &str) -> String {
    let raw: Vec<u8> = format!("{img_key}{sub_key}").into_bytes();
    MIXIN_KEY_ENC_TAB
        .iter()
        .filter_map(|&i| raw.get(i).map(|&c| c as char))
        .take(32)
        .collect()
}

/// `encodeURIComponent` compatible percent encoding.
pub fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => out.push(b as char),
            b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Builds an (unsigned) query string the same way the signer does.
pub fn build_query(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", encode_component(k), encode_component(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Signs `params` and returns the final query string (`...&w_rid=...`).
pub fn sign(params: &[(String, String)], mixin_key: &str, wts: u64) -> String {
    let mut params: Vec<(String, String)> = params
        .iter()
        .filter(|(k, _)| k != "w_rid" && k != "wts")
        .map(|(k, v)| {
            let v: String = v.chars().filter(|c| !"!'()*".contains(*c)).collect();
            (k.clone(), v)
        })
        .collect();
    params.push(("wts".into(), wts.to_string()));
    params.sort_by(|a, b| a.0.cmp(&b.0));
    let query = build_query(&params);
    let w_rid = format!(
        "{:x}",
        Md5::digest(format!("{query}{mixin_key}").as_bytes())
    );
    format!("{query}&w_rid={w_rid}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_vector() {
        // from bilibili-API-collect's WBI documentation
        let img = key_from_url("https://i0.hdslb.com/bfs/wbi/7cd084941338484aae1ad9425b84077c.png");
        let sub = key_from_url("https://i0.hdslb.com/bfs/wbi/4932caff0ff746eab6f01bf08b70ac45.png");
        let key = mixin_key(img, sub);
        assert_eq!(key, "ea1db124af3c7062474693fa704f4ff8");
        let params = vec![
            ("foo".to_string(), "114".to_string()),
            ("bar".to_string(), "514".to_string()),
            ("zab".to_string(), "1919810".to_string()),
        ];
        assert_eq!(
            sign(&params, &key, 1702204169),
            "bar=514&foo=114&wts=1702204169&zab=1919810&w_rid=8f6f2b5b3d485fe1886cec6a0be8c5d4"
        );
    }

    #[test]
    fn encoding() {
        assert_eq!(encode_component("a b&c=d/中"), "a%20b%26c%3Dd%2F%E4%B8%AD");
        let mut sorted = MIXIN_KEY_ENC_TAB.to_vec();
        sorted.sort();
        assert_eq!(sorted, (0..64).collect::<Vec<_>>());
    }
}
