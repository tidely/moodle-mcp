//! `{dir}/.moodle/manifest.json`: every file the last sync knew about.
//!
//! Lets a later sync remove files that disappeared from Moodle (only files it
//! wrote itself), and lets tools report skipped or failed files without
//! calling Moodle.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Error;

/// Metadata directory inside a mirror. Never touched by pruning.
pub const META_DIR: &str = ".moodle";
const MANIFEST: &str = "manifest.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub course_id: i64,
    /// Unix time the last full sync started. 0 = never fully synced.
    pub synced_at: i64,
    /// Activity directories the sync created, so removed activities can be cleaned up.
    #[serde(default)]
    pub activities: Vec<ManifestActivity>,
    #[serde(default)]
    pub files: Vec<ManifestFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestActivity {
    pub cmid: i64,
    /// Directory name relative to the mirror root.
    pub dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestFile {
    /// Relative to the mirror root, `/`-separated.
    pub path: String,
    /// Activity the file belongs to; `None` for course page files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmid: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timemodified: Option<i64>,
    pub state: FileState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileState {
    /// Downloaded and on disk.
    Present,
    /// Not downloaded because of the size limit.
    Skipped,
    /// Download attempted and failed.
    Failed,
}

impl Manifest {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join(META_DIR).join(MANIFEST)
    }

    /// The manifest in `dir`, if there is a readable one.
    pub fn load(dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(Self::path(dir)).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub(crate) fn save(&self, dir: &Path) -> Result<(), Error> {
        let path = Self::path(dir);
        let meta = dir.join(META_DIR);
        std::fs::create_dir_all(&meta).map_err(|e| Error::Io(meta, e))?;
        let json = serde_json::to_string_pretty(self).expect("manifest serializes");
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(|e| Error::Io(tmp.clone(), e))?;
        std::fs::rename(&tmp, &path).map_err(|e| Error::Io(path, e))
    }
}
