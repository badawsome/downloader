//! A tiny cookie jar for source credentials.

use std::collections::BTreeMap;

use crate::protocol::COOKIE_DOMAIN;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CookieJar {
    values: BTreeMap<String, String>,
}

impl CookieJar {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses a `Cookie:` header style string (`a=1; b=2`). A bare value
    /// without `=` is taken as `SESSDATA` for convenience. Lines of a
    /// Netscape `cookies.txt` export are understood as well.
    pub fn parse(s: &str) -> Self {
        let mut jar = Self::new();
        for line in s.lines() {
            let line = line.trim();
            if line.is_empty() || (line.starts_with('#') && !line.starts_with("#HttpOnly_")) {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() == 7 {
                // domain, include subdomains, path, secure, expiry, name, value
                if fields[0].contains(COOKIE_DOMAIN) {
                    jar.set(fields[5], fields[6]);
                }
                continue;
            }
            for part in line.split(';') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                match part.split_once('=') {
                    Some((k, v)) => jar.set(k.trim(), v.trim()),
                    None if jar.values.is_empty() => jar.set("SESSDATA", part),
                    None => {}
                }
            }
        }
        jar
    }

    /// Adds the `name=value` pair of a `Set-Cookie` header.
    pub fn add_set_cookie(&mut self, header: &str) {
        if let Some((k, v)) = header.split(';').next().and_then(|kv| kv.split_once('=')) {
            self.set(k.trim(), v.trim());
        }
    }

    pub fn set(&mut self, name: &str, value: &str) {
        if !name.is_empty() {
            self.values.insert(name.to_owned(), value.to_owned());
        }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Whether the jar carries a login session.
    pub fn has_session(&self) -> bool {
        self.get("SESSDATA").is_some_and(|v| !v.is_empty())
    }

    /// Serialises to a `Cookie:` header value.
    pub fn header_value(&self) -> String {
        self.values
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

impl std::fmt::Display for CookieJar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.header_value())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_variants() {
        let jar = CookieJar::parse("SESSDATA=abc%2C123; csrf_token=xyz ;buvid3=q");
        assert_eq!(jar.get("SESSDATA"), Some("abc%2C123"));
        assert_eq!(jar.get("csrf_token"), Some("xyz"));
        assert!(jar.has_session());
        assert_eq!(
            jar.header_value(),
            "SESSDATA=abc%2C123; buvid3=q; csrf_token=xyz"
        );

        let bare = CookieJar::parse("  rawsessdata ");
        assert_eq!(bare.get("SESSDATA"), Some("rawsessdata"));

        let netscape = CookieJar::parse(
            &format!("# Netscape HTTP Cookie File\n.{COOKIE_DOMAIN}\tTRUE\t/\tTRUE\t0\tSESSDATA\tv1\n.example.com\tTRUE\t/\tFALSE\t0\tx\ty\n"),
        );
        assert_eq!(netscape.get("SESSDATA"), Some("v1"));
        assert!(!netscape.contains("x"));

        let mut jar = CookieJar::new();
        jar.add_set_cookie("SESSDATA=s; Path=/; Domain=example.com; HttpOnly");
        assert_eq!(jar.get("SESSDATA"), Some("s"));
        assert!(!CookieJar::parse("").has_session());
    }
}
