//! bilibili web API client focused on audio downloading.
//!
//! * anonymous and cookie / QR-code login sessions,
//! * WBI request signing,
//! * video / page / season (合集) / favorites metadata,
//! * DASH audio stream discovery including Dolby Atmos and Hi-Res FLAC,
//!   with quality selection ([`AudioPreference`]).

mod audio;
mod client;
mod cookie;
mod error;
mod id;
mod models;
pub mod wbi;

pub use audio::{AudioPreference, AudioQuality, AudioStream, StreamSource};
pub use client::{normalize_url, Client, QrCode, QrStatus, REFERER, USER_AGENT};
pub use cookie::CookieJar;
pub use error::{Error, Result};
pub use id::VideoId;
pub use models::*;
