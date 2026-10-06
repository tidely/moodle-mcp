//! The global cache agents read documents from: `{root}/files/{host}/{course dir}/`.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Cache {
    root: PathBuf,
}

impl Cache {
    /// `$MOODLE_MCP_DIR`, or `~/.moodle-mcp`. Always absolute.
    pub fn from_env() -> Result<Self, String> {
        let root = match std::env::var_os("MOODLE_MCP_DIR") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => std::env::home_dir()
                .ok_or("could not determine home directory; set MOODLE_MCP_DIR")?
                .join(".moodle-mcp"),
        };
        let root = std::path::absolute(&root).map_err(|e| format!("MOODLE_MCP_DIR: {e}"))?;
        Ok(Self { root })
    }

    pub fn course_dir(&self, host: &str, shortname: &str, course_id: i64) -> PathBuf {
        self.root
            .join("files")
            .join(sync::sanitize(host))
            .join(sync::dir_name(shortname, course_id))
    }
}
