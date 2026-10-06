//! Path safety and file download helpers.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use moodle_api::Client;
use tokio::io::AsyncWriteExt;

/// Longest single path component we produce, in bytes. Leaves room for
/// the ` ({id})` suffix and `.part` within the usual 255-byte limit.
const MAX_NAME_BYTES: usize = 200;

/// `{name} ({id})`, sanitized. Used for course and activity directories.
pub fn dir_name(name: &str, id: i64) -> String {
    format!("{} ({id})", sanitize(name))
}

/// Parses the id back out of a [`dir_name`].
pub fn parse_dir_id(name: &str) -> Option<i64> {
    name.strip_suffix(')')?.rsplit_once(" (")?.1.parse().ok()
}

/// Makes `name` safe as a single path component: no separators, no `..`,
/// no control or Windows-reserved characters, bounded length.
pub fn sanitize(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = replaced.trim_matches(|c: char| c == '.' || c.is_whitespace());
    let mut end = trimmed.len().min(MAX_NAME_BYTES);
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    match trimmed[..end].trim_end() {
        "" => "_".to_owned(),
        s => s.to_owned(),
    }
}

/// `dir/{filepath}/{filename}` with every component sanitized.
/// `filepath` is Moodle's folder path, e.g. `/` or `/week 1/`.
pub fn file_path(dir: &Path, filepath: &str, filename: &str) -> PathBuf {
    let mut path = dir.to_path_buf();
    for part in filepath.split('/').filter(|p| !p.is_empty()) {
        path.push(sanitize(part));
    }
    path.push(sanitize(filename));
    path
}

/// True if `path` is a file matching the known size and modification time.
/// Unknown values aren't compared, so with neither known (files embedded in
/// HTML) an existing file counts as unchanged.
pub fn is_unchanged(path: &Path, size: Option<u64>, timemodified: Option<i64>) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() || size.is_some_and(|s| meta.len() != s) {
        return false;
    }
    let Some(time) = timemodified.filter(|t| *t > 0) else {
        return true;
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);
    mtime == Some(time)
}

/// Downloads `url` to `path` via a `.part` file, then sets the mtime to
/// Moodle's `timemodified` so [`is_unchanged`] can skip it next time.
pub async fn download_to(
    client: &Client,
    url: &str,
    path: &Path,
    timemodified: Option<i64>,
) -> Result<u64, String> {
    let dir = path.parent().ok_or("invalid target path")?;
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| format!("create {}: {e}", dir.display()))?;

    let mut part_name = path.file_name().unwrap_or_default().to_owned();
    part_name.push(".part");
    let part = path.with_file_name(part_name);

    let result = async {
        let mut download = client.download(url).await.map_err(|e| e.to_string())?;
        let mut file = tokio::fs::File::create(&part)
            .await
            .map_err(|e| e.to_string())?;
        let mut written = 0u64;
        while let Some(chunk) = download.chunk().await.map_err(|e| e.to_string())? {
            let chunk = chunk.as_ref();
            file.write_all(chunk).await.map_err(|e| e.to_string())?;
            written += chunk.len() as u64;
        }
        file.flush().await.map_err(|e| e.to_string())?;
        let file = file.into_std().await;
        if let Some(t) = timemodified.filter(|t| *t > 0) {
            file.set_modified(UNIX_EPOCH + Duration::from_secs(t as u64))
                .map_err(|e| e.to_string())?;
        }
        drop(file);
        tokio::fs::rename(&part, path)
            .await
            .map_err(|e| e.to_string())?;
        Ok(written)
    }
    .await;

    if result.is_err() {
        let _ = tokio::fs::remove_file(&part).await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_blocks_escapes() {
        assert_eq!(sanitize(".."), "_");
        assert_eq!(sanitize("."), "_");
        assert_eq!(sanitize(""), "_");
        assert_eq!(sanitize("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(sanitize("a/b\\c"), "a_b_c");
        assert_eq!(sanitize("Project 2: Optimizer?"), "Project 2_ Optimizer_");
        assert_eq!(sanitize("  report.pdf. "), "report.pdf");
        assert_eq!(sanitize("tab\there"), "tab_here");
        assert!(sanitize(&"æ".repeat(300)).len() <= MAX_NAME_BYTES);
    }

    #[test]
    fn layout() {
        assert_eq!(dir_name("DB/2026", 42), "DB_2026 (42)");
        assert_eq!(parse_dir_id("Project (2) (1337)"), Some(1337));
        assert_eq!(parse_dir_id("index.md"), None);
        let dir = Path::new("/c/Project 2 (1337)");
        assert_eq!(
            file_path(dir, "/week 1/../", "a.pdf"),
            dir.join("week 1").join("_").join("a.pdf")
        );
        assert_eq!(file_path(dir, "/", "a.pdf"), dir.join("a.pdf"));
    }

    #[test]
    fn unchanged_compares_size_and_mtime() {
        let dir = std::env::temp_dir().join(format!("moodle-mcp-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.txt");
        fs::write(&path, b"hello").unwrap();
        let file = fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_secs(1_700_000_000))
            .unwrap();
        drop(file);

        assert!(is_unchanged(&path, Some(5), Some(1_700_000_000)));
        assert!(!is_unchanged(&path, Some(6), Some(1_700_000_000)));
        assert!(!is_unchanged(&path, Some(5), Some(1_700_000_001)));
        assert!(is_unchanged(&path, Some(5), None));
        assert!(is_unchanged(&path, None, None));
        assert!(!is_unchanged(&path, Some(6), None));
        assert!(!is_unchanged(&dir.join("missing"), Some(5), Some(1)));
        fs::remove_dir_all(&dir).unwrap();
    }
}
