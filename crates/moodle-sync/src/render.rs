//! Markdown helpers for the index files.

use std::collections::HashMap;
use std::path::Path;

/// Replaces Moodle file URLs that weren't mapped to a local file.
const NOT_MIRRORED: &str = "#file-not-mirrored";

/// Converts Moodle HTML to markdown.
///
/// `local` maps file URLs (by [`file_key`]) to paths relative to the index
/// file; images and links to those files then point at the local copy.
/// Any remaining Moodle file URL (or anything carrying `token=`) is removed.
pub fn markdown(html: &str, base: &str, local: &HashMap<String, String>) -> String {
    let mut rewritten = String::with_capacity(html.len());
    let mut last = 0;
    for (start, end) in attr_values(html) {
        let rel = moodle_file_url(&html[start..end], base).and_then(|u| local.get(file_key(&u)));
        if let Some(rel) = rel {
            rewritten.push_str(&html[last..start]);
            rewritten.push_str(&encode_link(rel));
            last = end;
        }
    }
    rewritten.push_str(&html[last..]);
    let md = htmd::convert(&rewritten).unwrap_or(rewritten);
    scrub(md.trim(), base)
}

/// Moodle file URLs referenced by `src`, `href` or `data` attributes, normalized
/// with [`moodle_file_url`]. In document order, may contain duplicates.
pub fn file_urls(html: &str, base: &str) -> Vec<String> {
    attr_values(html)
        .into_iter()
        .filter_map(|(s, e)| moodle_file_url(&html[s..e], base))
        .collect()
}

/// A downloadable `{base}webservice/pluginfile.php/...` URL, if `raw` (an
/// HTML attribute value) points at a file on this site.
pub fn moodle_file_url(raw: &str, base: &str) -> Option<String> {
    let url = raw.trim().replace("&amp;", "&");
    let ws = format!("{base}webservice/pluginfile.php/");
    if url.starts_with(&ws) {
        return Some(url);
    }
    // Not rewritten for web services; the token only works on the webservice path.
    let rest = url.strip_prefix(&format!("{base}pluginfile.php/"))?;
    Some(format!("{ws}{rest}"))
}

/// Identity of a file URL for matching: without query string.
pub fn file_key(url: &str) -> &str {
    url.split_once('?').map_or(url, |(u, _)| u)
}

/// Decoded last path segment of a file URL, e.g. `my image.png`.
pub fn url_filename(url: &str) -> String {
    let segment = file_key(url).rsplit('/').next().unwrap_or_default();
    match percent_decode(segment) {
        name if name.is_empty() => "file".to_owned(),
        name => name,
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && let (Some(&h), Some(&l)) = (bytes.get(i + 1), bytes.get(i + 2))
            && let (Some(h), Some(l)) = (hex(h), hex(l))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Byte ranges of `src=`, `href=` and `data=` attribute values, in order.
fn attr_values(html: &str) -> Vec<(usize, usize)> {
    // ASCII lowercasing keeps byte offsets.
    let lower = html.to_ascii_lowercase();
    let mut ranges = Vec::new();
    for attr in ["src=", "href=", "data="] {
        let mut from = 0;
        while let Some(i) = lower[from..].find(attr) {
            let at = from + i;
            from = at + attr.len();
            // Must be a whole attribute name, not e.g. `data-src=`.
            if at == 0 || !lower.as_bytes()[at - 1].is_ascii_whitespace() {
                continue;
            }
            let rest = &html[from..];
            let range = match rest.chars().next() {
                Some(q @ ('"' | '\'')) => match rest[1..].find(q) {
                    Some(len) => (from + 1, from + 1 + len),
                    None => continue,
                },
                _ => {
                    let len = rest
                        .find(|c: char| c.is_whitespace() || c == '>')
                        .unwrap_or(rest.len());
                    (from, from + len)
                }
            };
            ranges.push(range);
        }
    }
    ranges.sort_unstable();
    ranges
}

/// Short stable hash, for disambiguating file names.
pub fn short_hash(s: &str) -> String {
    // FNV-1a: stable across Rust versions, unlike `DefaultHasher`.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{:08x}", h as u32)
}

/// Removes URLs under `base` that point at files or carry a token.
pub fn scrub(text: &str, base: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(base) {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '>' | '"' | '\'' | ']'))
            .unwrap_or(tail.len());
        let url = &tail[..end];
        if url.contains("pluginfile.php") || url.contains("token=") {
            out.push_str(NOT_MIRRORED);
        } else {
            out.push_str(url);
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// A relative path as a markdown link target: `/`-separated, spaces encoded.
pub fn encode_link(rel: &str) -> String {
    rel.replace('%', "%25")
        .replace(' ', "%20")
        .replace('(', "%28")
        .replace(')', "%29")
}

/// `a/b/c` from a relative path, regardless of platform separator.
pub fn rel_path(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// `2026-10-20 21:59 UTC`.
pub fn date(unix: i64) -> String {
    let (year, month, day, secs) = civil(unix);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        secs / 3600,
        secs % 3600 / 60
    )
}

/// RFC 3339 in UTC: `2026-10-20T21:59:00Z`.
pub fn iso(unix: i64) -> String {
    let (year, month, day, secs) = civil(unix);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

/// (year, month, day, seconds into the day) in UTC.
fn civil(unix: i64) -> (i64, i64, i64, i64) {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    // Civil-from-days, https://howardhinnant.github.io/date_algorithms.html
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day, secs)
}

/// `412.0 KB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://m.example/";

    #[test]
    fn dates() {
        assert_eq!(date(0), "1970-01-01 00:00 UTC");
        assert_eq!(date(951_782_400), "2000-02-29 00:00 UTC");
        assert_eq!(date(1_700_000_000), "2023-11-14 22:13 UTC");
        assert_eq!(iso(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn finds_file_urls() {
        let html = r#"<img src="https://m.example/webservice/pluginfile.php/1/a.png?time=1">
            <img data-src="https://m.example/webservice/pluginfile.php/1/lazy.png">
            <a href='https://m.example/pluginfile.php/2/b%20c.pdf?x=1&amp;y=2'>b</a>
            <a href="https://elsewhere.example/x.png">x</a>"#;
        let urls = file_urls(html, BASE);
        assert_eq!(
            urls,
            [
                "https://m.example/webservice/pluginfile.php/1/a.png?time=1",
                "https://m.example/webservice/pluginfile.php/2/b%20c.pdf?x=1&y=2",
            ]
        );
        assert_eq!(url_filename(&urls[1]), "b c.pdf");
        assert_eq!(
            file_key(&urls[0]),
            "https://m.example/webservice/pluginfile.php/1/a.png"
        );
    }

    #[test]
    fn rewrites_and_scrubs_file_urls() {
        let html = r#"<p>See <img src="https://m.example/webservice/pluginfile.php/1/intro/a b.png?time=5">
            and <a href="https://m.example/webservice/pluginfile.php/1/intro/other.pdf">this</a>
            on <a href="https://m.example/course/view.php?id=4">the course</a>.</p>"#;
        let local = HashMap::from([(
            "https://m.example/webservice/pluginfile.php/1/intro/a b.png".to_owned(),
            "a b.png".to_owned(),
        )]);
        let md = markdown(html, BASE, &local);
        assert!(md.contains("(a%20b.png)"), "{md}");
        assert!(md.contains("[this](#file-not-mirrored)"), "{md}");
        assert!(
            md.contains("https://m.example/course/view.php?id=4"),
            "{md}"
        );
        assert!(!md.contains("pluginfile"), "{md}");
    }

    #[test]
    fn scrubs_tokens() {
        let s = scrub("x https://m.example/a?token=abc y", BASE);
        assert_eq!(s, "x #file-not-mirrored y");
    }
}
