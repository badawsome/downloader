//! Where the login session (cookies) is stored and how it is loaded.

use std::path::PathBuf;

use anyhow::Context;
use download_service::source_api::CookieJar;

/// `$DC_CONFIG_DIR` or the platform config dir (`~/.config/dc`,
/// `%APPDATA%\dc`, `~/Library/Application Support/dc`).
pub fn config_dir() -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("DC_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    dirs::config_dir()
        .map(|d| d.join("dc"))
        .context("cannot determine the config directory; set DC_CONFIG_DIR")
}

pub fn cookie_file() -> anyhow::Result<PathBuf> {
    Ok(config_dir()?.join("cookie.txt"))
}

/// Cookie precedence: `--cookie`/`DC_COOKIE` > `--cookie-file` > saved login.
pub fn load(cookie: Option<&str>, file: Option<&PathBuf>) -> anyhow::Result<CookieJar> {
    if let Some(c) = cookie.filter(|c| !c.trim().is_empty()) {
        return Ok(CookieJar::parse(c));
    }
    if let Some(path) = file {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read cookie file {}", path.display()))?;
        return Ok(CookieJar::parse(&text));
    }
    match std::fs::read_to_string(cookie_file()?) {
        Ok(text) => Ok(CookieJar::parse(&text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(CookieJar::new()),
        Err(e) => Err(e).context("cannot read saved session"),
    }
}

pub fn save(jar: &CookieJar) -> anyhow::Result<PathBuf> {
    let path = cookie_file()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, jar.header_value())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(path)
}

pub fn remove() -> anyhow::Result<bool> {
    match std::fs::remove_file(cookie_file()?) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
