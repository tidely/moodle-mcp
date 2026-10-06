//! Credentials and mirror metadata.
//!
//! - Credentials: `$MOODLE_URL`/`$MOODLE_TOKEN` (environment or `.env`) take
//!   precedence; otherwise `{config dir}/moodle-cli/credentials.json`, written
//!   by `moodle-cli login` with owner-only permissions.
//! - Mirror: `{mirror}/.moodle/mirror.json`, written by `clone`. Commands find
//!   it by walking up from the working directory, like git.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use moodle_api::Client;
use moodle_api::auth::SsoToken;
use serde::{Deserialize, Serialize};

use crate::output::hinted;

#[derive(Serialize, Deserialize)]
pub struct Credentials {
    pub url: String,
    pub token: String,
}

pub fn credentials_path() -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => std::env::home_dir()
            .context("could not determine the home directory")?
            .join(".config"),
    };
    Ok(base.join("moodle-cli").join("credentials.json"))
}

/// Accepts a raw token or a `moodledl://token=...` redirect URL.
pub fn parse_token(input: &str) -> Result<String> {
    let input = input.trim();
    if input.contains("token=") {
        let sso = SsoToken::parse(input).context("could not decode the token from that URL")?;
        Ok(sso.token)
    } else if !input.is_empty() && input.chars().all(|c| c.is_ascii_alphanumeric()) {
        Ok(input.to_owned())
    } else {
        Err(hinted(
            "bad_token",
            "that doesn't look like a token",
            "Paste the full moodledl://token=... URL or the raw token.",
        ))
    }
}

pub fn load_credentials() -> Result<Credentials> {
    if let (Ok(url), Ok(token)) = (std::env::var("MOODLE_URL"), std::env::var("MOODLE_TOKEN"))
        && !token.is_empty()
    {
        return Ok(Credentials {
            url,
            token: parse_token(&token)?,
        });
    }
    let path = credentials_path()?;
    let text = std::fs::read_to_string(&path).map_err(|_| {
        hinted(
            "not_logged_in",
            "not logged in",
            "Run `moodle-cli login --url https://your.moodle/` (or set MOODLE_URL and MOODLE_TOKEN).",
        )
    })?;
    serde_json::from_str(&text).with_context(|| format!("{} is corrupt", path.display()))
}

pub fn save_credentials(creds: &Credentials) -> Result<PathBuf> {
    let path = credentials_path()?;
    let dir = path.parent().expect("credentials path has a parent");
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let json = serde_json::to_string_pretty(creds)?;
    let mut file = std::fs::OpenOptions::new();
    file.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut file, 0o600);
    let mut file = file
        .open(&path)
        .with_context(|| format!("write {}", path.display()))?;
    std::io::Write::write_all(&mut file, json.as_bytes())?;
    Ok(path)
}

pub fn client() -> Result<Client> {
    let creds = load_credentials()?;
    Ok(Client::new(&creds.url, creds.token)?)
}

const MIRROR_FILE: &str = "mirror.json";

#[derive(Serialize, Deserialize)]
pub struct MirrorConfig {
    /// Site root this mirror belongs to.
    pub site: String,
    pub course_id: i64,
    /// Size limit for `pull`; 0 = no limit.
    pub max_file_mb: u64,
}

impl MirrorConfig {
    pub fn path(root: &Path) -> PathBuf {
        root.join(moodle_sync::META_DIR).join(MIRROR_FILE)
    }

    pub fn load(root: &Path) -> Option<Self> {
        serde_json::from_str(&std::fs::read_to_string(Self::path(root)).ok()?).ok()
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        let path = Self::path(root);
        std::fs::create_dir_all(path.parent().expect("has parent"))?;
        std::fs::write(&path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("write {}", path.display()))
    }

    pub fn options(&self) -> moodle_sync::Options {
        moodle_sync::Options {
            max_file_size: (self.max_file_mb > 0).then_some(self.max_file_mb * 1024 * 1024),
            ..moodle_sync::Options::default()
        }
    }
}

/// The mirror containing `start` (or the working directory), and its config.
pub fn find_mirror(start: Option<&Path>) -> Result<(PathBuf, MirrorConfig)> {
    try_find_mirror(start)?.ok_or_else(|| {
        hinted(
            "not_a_mirror",
            "not inside a course mirror",
            "Run this inside a directory created by `moodle-cli clone <course_id>`, or pass -C <dir>.",
        )
    })
}

pub fn try_find_mirror(start: Option<&Path>) -> Result<Option<(PathBuf, MirrorConfig)>> {
    let start = match start {
        Some(p) => std::path::absolute(p)?,
        None => std::env::current_dir()?,
    };
    for dir in start.ancestors() {
        if let Some(config) = MirrorConfig::load(dir) {
            return Ok(Some((dir.to_owned(), config)));
        }
    }
    Ok(None)
}

/// Fails if the mirror belongs to a different site than the logged-in one.
pub fn check_site(config: &MirrorConfig, client: &Client) -> Result<()> {
    if config.site.trim_end_matches('/') != client.base().trim_end_matches('/') {
        return Err(hinted(
            "wrong_site",
            format!(
                "this mirror belongs to {}, but you are logged in to {}",
                config.site,
                client.base()
            ),
            "Log in to the mirror's site with `moodle-cli login --url <site>`.",
        ));
    }
    Ok(())
}
