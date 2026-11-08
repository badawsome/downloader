//! Wire-level values required to connect to the source.
//! Keep product names and user-facing wording out of this module.

pub const COOKIE_DOMAIN: &str = "bilibili.com";
pub const REFERER: &str = "https://www.bilibili.com/";
pub const API_BASE: &str = "https://api.bilibili.com";
pub const PASSPORT_BASE: &str = "https://passport.bilibili.com";
pub const SPACE_HOST: &str = "space.bilibili.com";
pub const SHORT_LINK_HOSTS: &[&str] = &["b23.tv", "bili2233.cn"];
pub const CSRF_COOKIE: &str = "bili_jct";
