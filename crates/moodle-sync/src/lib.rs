//! Mirrors a Moodle course into a local directory: its files plus markdown
//! index files, so agents can explore it with ordinary file tools.
//!
//! ```text
//! {dir}/
//! ├── .moodle/manifest.json       # what the last sync wrote (see [`Manifest`])
//! ├── index.md                    # course summary, sections → activities, dates
//! ├── _embedded/                  # images/files referenced from the course page HTML
//! └── {activity name} ({cmid})/
//!     ├── index.md                # description, dates, submission, file list
//!     ├── {filepath}/{filename}   # the activity's files
//!     └── _embedded/              # images/files referenced only from its HTML
//! ```
//!
//! ```no_run
//! # async fn run(client: &moodle_api::Client) -> Result<(), moodle_sync::Error> {
//! let course = moodle_sync::Course::fetch(client, 42).await?;
//! let dir = std::path::Path::new("mirror").join(course.dir_name());
//! let report = course.sync(client, &dir, &moodle_sync::Options::default()).await?;
//! println!("{} files downloaded", report.downloaded.len());
//! # Ok(()) }
//! ```

mod fsutil;
mod manifest;
mod render;

use std::collections::{HashMap, HashSet};
use std::fmt::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use moodle_api::assign::{
    Assignment, GetAssignments, GetSubmissionStatus, Plugin, Submission, SubmissionStatus,
};
use moodle_api::course::{self, GetCourseContents, GetCourseModule, GetCoursesByField, Module};
use moodle_api::resource::{GetPages, Page};
use moodle_api::{Client, File};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

pub use fsutil::{dir_name, parse_dir_id, sanitize};
pub use manifest::{FileState, META_DIR, Manifest, ManifestActivity, ManifestFile};
pub use render::{date as format_date, human_size, iso as format_iso};

/// Name of the index file in the course and activity directories.
pub const INDEX: &str = "index.md";

/// Subdirectory for files that Moodle only references from HTML (pasted
/// images, inline links), not in any file list.
pub const EMBEDDED_DIR: &str = "_embedded";

#[derive(Debug, Clone)]
pub struct Options {
    /// Larger files are listed in the index but not downloaded. `None` = no limit.
    pub max_file_size: Option<u64>,
    /// Maximum parallel requests.
    pub concurrency: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_file_size: Some(100 * 1024 * 1024),
            concurrency: 4,
        }
    }
}

/// Outcome of [`Course::sync`] or [`Course::download`].
#[derive(Debug, Default)]
pub struct Report {
    /// The course index (sync) or activity index (download).
    pub index: PathBuf,
    pub downloaded: Vec<PathBuf>,
    pub unchanged: usize,
    /// Not downloaded because of [`Options::max_file_size`]: path and size.
    pub skipped: Vec<(PathBuf, u64)>,
    pub failed: Vec<(PathBuf, String)>,
    /// Files and indexes deleted because they disappeared from Moodle.
    pub removed: Vec<PathBuf>,
    /// Requested file names that the activity doesn't have.
    pub missing: Vec<String>,
    /// Non-fatal problems, e.g. an endpoint the site doesn't allow.
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub enum Error {
    Api(moodle_api::Error),
    Io(PathBuf, std::io::Error),
    NotFound(String),
}

impl From<moodle_api::Error> for Error {
    fn from(e: moodle_api::Error) -> Self {
        Self::Api(e)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Api(e) => e.fmt(f),
            Self::Io(path, e) => write!(f, "{}: {e}", path.display()),
            Self::NotFound(what) => write!(f, "{what} not found"),
        }
    }
}

impl std::error::Error for Error {}

/// The course a course module (`cmid`) belongs to.
pub async fn course_of(client: &Client, cmid: i64) -> Result<i64, Error> {
    Ok(client.call(&GetCourseModule { cmid }).await?.cm.course)
}

/// A course's structure, fetched once and then written to disk.
pub struct Course {
    info: course::Course,
    sections: Vec<course::Section>,
    /// By `cmid`.
    assignments: HashMap<i64, Assignment>,
    /// By `cmid`.
    pages: HashMap<i64, Page>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone)]
struct RemoteFile {
    filepath: String,
    filename: String,
    url: String,
    size: Option<u64>,
    timemodified: Option<i64>,
}

impl RemoteFile {
    fn from_file(f: &File, prefix: &str) -> Option<Self> {
        Some(Self {
            filepath: format!("{prefix}{}", f.filepath.as_deref().unwrap_or("/")),
            filename: f.filename.clone()?,
            url: f.fileurl.clone()?,
            size: f.filesize,
            timemodified: f.timemodified,
        })
    }

    /// Path relative to the activity directory.
    fn rel(&self) -> PathBuf {
        fsutil::file_path(Path::new(""), &self.filepath, &self.filename)
    }
}

impl Course {
    /// Fetches course info, contents, assignments and pages.
    pub async fn fetch(client: &Client, course_id: i64) -> Result<Self, Error> {
        let info = client
            .call(&GetCoursesByField {
                field: Some("id".into()),
                value: Some(course_id.to_string()),
            })
            .await?
            .courses
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound(format!("course {course_id}")))?;
        let sections = client
            .call(&GetCourseContents {
                courseid: course_id,
                options: Vec::new(),
            })
            .await?;

        let mut warnings = Vec::new();
        let assignments = match client
            .call(&GetAssignments {
                courseids: vec![course_id],
            })
            .await
        {
            Ok(r) => r
                .courses
                .into_iter()
                .flat_map(|c| c.assignments)
                .map(|a| (a.cmid, a))
                .collect(),
            Err(e) => {
                warnings.push(format!("assignments: {e}"));
                HashMap::new()
            }
        };
        let pages = match client
            .call(&GetPages {
                courseids: vec![course_id],
            })
            .await
        {
            Ok(r) => r.pages.into_iter().map(|p| (p.coursemodule, p)).collect(),
            Err(e) => {
                warnings.push(format!("pages: {e}"));
                HashMap::new()
            }
        };

        Ok(Self {
            info,
            sections,
            assignments,
            pages,
            warnings,
        })
    }

    pub fn id(&self) -> i64 {
        self.info.id
    }

    pub fn shortname(&self) -> &str {
        &self.info.shortname
    }

    pub fn fullname(&self) -> &str {
        &self.info.fullname
    }

    /// Suggested directory name: `{shortname} ({id})`.
    pub fn dir_name(&self) -> String {
        dir_name(&self.info.shortname, self.info.id)
    }

    /// Writes the full mirror into `dir`: all indexes, and all files within
    /// the size limit. Unchanged files are kept; renamed activities are moved.
    pub async fn sync(&self, client: &Client, dir: &Path, opts: &Options) -> Result<Report, Error> {
        let started = now();
        create_dir(dir)?;
        let old = Manifest::load(dir);
        let mut report = Report {
            index: dir.join(INDEX),
            warnings: self.warnings.clone(),
            ..Report::default()
        };

        let assigns = self
            .modules()
            .filter(|m| mirrored(m))
            .filter_map(|m| Some((m.id, self.assignments.get(&m.id)?.id)))
            .collect();
        let statuses = statuses(client, assigns, opts.concurrency, &mut report.warnings).await;

        let existing = existing_activity_dirs(dir);
        let mut jobs = Vec::new();
        let course_files = self.course_files(client.base());
        plan(
            &course_files,
            dir,
            opts.max_file_size,
            &mut report,
            &mut jobs,
        );
        let mut planned = Vec::new();
        for m in self.modules().filter(|m| mirrored(m)) {
            let adir = dir.join(dir_name(&m.name, m.id));
            if let Some(old) = existing.get(&m.id)
                && *old != adir
                && !adir.exists()
            {
                std::fs::rename(old, &adir).map_err(|e| Error::Io(old.clone(), e))?;
            }

            let files = self.files(m, statuses.get(&m.id), client.base());
            plan(&files, &adir, opts.max_file_size, &mut report, &mut jobs);
            planned.push((m, adir, files));
        }

        download_all(client, jobs, opts.concurrency, &mut report).await;

        for (m, adir, files) in &planned {
            self.write_activity(client.base(), m, adir, files, statuses.get(&m.id), opts)?;
        }
        self.write_course_index(client.base(), dir, &course_files)?;

        let mut manifest = Manifest {
            course_id: self.id(),
            synced_at: started,
            activities: Vec::new(),
            files: entries(dir, dir, None, &course_files, opts.max_file_size),
        };
        for (m, adir, files) in &planned {
            manifest.activities.push(ManifestActivity {
                cmid: m.id,
                dir: dir_name(&m.name, m.id),
            });
            let limit = opts.max_file_size;
            manifest
                .files
                .extend(entries(dir, adir, Some(m.id), files, limit));
        }
        if let Some(old) = old {
            prune(dir, &old, &manifest, &mut report);
        }
        manifest.save(dir)?;
        Ok(report)
    }

    /// Downloads one activity's files into its directory under `dir`,
    /// ignoring the size limit, and rewrites its index.
    /// `names` restricts the download to files whose name or path relative to
    /// the activity directory (e.g. `week 1/a.pdf`) is listed.
    pub async fn download(
        &self,
        client: &Client,
        dir: &Path,
        cmid: i64,
        names: Option<&[String]>,
        opts: &Options,
    ) -> Result<Report, Error> {
        let m = self
            .modules()
            .find(|m| m.id == cmid && mirrored(m))
            .ok_or_else(|| Error::NotFound(format!("activity {cmid} in course {}", self.id())))?;
        let adir = dir.join(dir_name(&m.name, m.id));
        let mut report = Report {
            index: adir.join(INDEX),
            ..Report::default()
        };

        let assigns = self
            .assignments
            .get(&cmid)
            .map(|a| (cmid, a.id))
            .into_iter()
            .collect();
        let statuses = statuses(client, assigns, 1, &mut report.warnings).await;
        let files = self.files(m, statuses.get(&cmid), client.base());

        let matches = |f: &RemoteFile, n: &String| {
            f.filename == *n || render::rel_path(&f.rel()) == n.trim_start_matches('/')
        };
        if let Some(names) = names {
            report.missing = names
                .iter()
                .filter(|n| !files.iter().any(|f| matches(f, n)))
                .cloned()
                .collect();
        }
        let selected: Vec<RemoteFile> = files
            .iter()
            .filter(|f| names.is_none_or(|names| names.iter().any(|n| matches(f, n))))
            .cloned()
            .collect();
        let mut jobs = Vec::new();
        plan(&selected, &adir, None, &mut report, &mut jobs);
        download_all(client, jobs, opts.concurrency, &mut report).await;

        self.write_activity(client.base(), m, &adir, &files, statuses.get(&cmid), opts)?;
        if !dir.join(INDEX).exists() {
            let course_files = self.course_files(client.base());
            self.write_course_index(client.base(), dir, &course_files)?;
        }

        // Keep the manifest in step; `synced_at` stays that of the last full sync.
        let mut manifest = Manifest::load(dir).unwrap_or(Manifest {
            course_id: self.id(),
            synced_at: 0,
            activities: Vec::new(),
            files: Vec::new(),
        });
        manifest.files.retain(|f| f.cmid != Some(cmid));
        manifest
            .files
            .extend(entries(dir, &adir, Some(cmid), &files, opts.max_file_size));
        manifest.activities.retain(|a| a.cmid != cmid);
        manifest.activities.push(ManifestActivity {
            cmid,
            dir: dir_name(&m.name, m.id),
        });
        manifest.save(dir)?;
        Ok(report)
    }

    fn modules(&self) -> impl Iterator<Item = &Module> {
        self.sections.iter().flat_map(|s| &s.modules)
    }

    /// Every file of an activity, deduplicated. Only files on this site.
    fn files(&self, m: &Module, status: Option<&SubmissionStatus>, base: &str) -> Vec<RemoteFile> {
        let mut files = Vec::new();
        if let Some(p) = self.pages.get(&m.id) {
            // Page `contents` holds the page body as index.html; we render that
            // into the index instead, and only keep embedded files.
            let embedded = p.introfiles.iter().chain(&p.contentfiles);
            files.extend(embedded.filter_map(|f| RemoteFile::from_file(f, "")));
        } else {
            for c in m.contents.iter().filter(|c| c.kind == "file") {
                if let Some(url) = &c.fileurl {
                    files.push(RemoteFile {
                        filepath: c.filepath.clone().unwrap_or_else(|| "/".into()),
                        filename: c.filename.clone(),
                        url: url.clone(),
                        size: Some(c.filesize),
                        timemodified: Some(c.timemodified),
                    });
                }
            }
        }
        if let Some(a) = self.assignments.get(&m.id) {
            let intro = a.introattachments.iter().chain(&a.introfiles);
            files.extend(intro.filter_map(|f| RemoteFile::from_file(f, "")));
        }
        if let Some(s) = status {
            if let Some(sub) = submission(s) {
                files.extend(plugin_files(&sub.plugins, "/submission"));
            }
            if let Some(fb) = &s.feedback {
                files.extend(plugin_files(&fb.plugins, "/feedback"));
            }
        }
        add_embedded(&mut files, self.activity_html(m, status), base);
        finish_files(files, base)
    }

    /// Files of the course page itself: summary files, plus files referenced
    /// from the course summary, section summaries and labels.
    fn course_files(&self, base: &str) -> Vec<RemoteFile> {
        let prefix = format!("/{EMBEDDED_DIR}");
        let mut files: Vec<RemoteFile> = (self.info.summaryfiles.iter())
            .filter_map(|f| RemoteFile::from_file(f, &prefix))
            .collect();
        let labels = self.modules().filter(|m| m.modname == "label");
        let html = (self.info.summary.as_deref().into_iter())
            .chain(self.sections.iter().map(|s| s.summary.as_str()))
            .chain(labels.filter_map(|m| m.description.as_deref()));
        add_embedded(&mut files, html, base);
        finish_files(files, base)
    }

    /// The HTML shown as an activity's description.
    fn description<'a>(&'a self, m: &'a Module) -> Option<&'a str> {
        match (self.assignments.get(&m.id), self.pages.get(&m.id)) {
            (Some(a), _) => a.intro.as_deref(),
            (None, Some(p)) => Some(&p.intro),
            (None, None) => m.description.as_deref(),
        }
    }

    /// All HTML rendered into an activity's index.
    fn activity_html<'a>(
        &'a self,
        m: &'a Module,
        status: Option<&'a SubmissionStatus>,
    ) -> Vec<&'a str> {
        let mut html: Vec<&str> = self.description(m).into_iter().collect();
        html.extend(self.pages.get(&m.id).map(|p| p.content.as_str()));
        if let Some(s) = status {
            let own = submission(s).map(|s| &s.plugins);
            let feedback = s.feedback.as_ref().map(|f| &f.plugins);
            let fields = own.into_iter().chain(feedback).flatten();
            html.extend(
                fields
                    .flat_map(|p| &p.editorfields)
                    .map(|e| e.text.as_str()),
            );
        }
        html
    }

    /// Opening/due dates: Moodle's own labels if available, else assignment fields.
    fn dates(&self, m: &Module) -> Vec<(String, i64)> {
        if !m.dates.is_empty() {
            return m
                .dates
                .iter()
                .map(|d| (d.label.clone(), d.timestamp))
                .collect();
        }
        let Some(a) = self.assignments.get(&m.id) else {
            return Vec::new();
        };
        [
            ("Opens:", a.allowsubmissionsfromdate),
            ("Due:", a.duedate),
            ("Cut-off:", a.cutoffdate),
        ]
        .into_iter()
        .filter(|(_, t)| *t > 0)
        .map(|(l, t)| (l.to_owned(), t))
        .collect()
    }

    fn write_activity(
        &self,
        base: &str,
        m: &Module,
        adir: &Path,
        files: &[RemoteFile],
        status: Option<&SubmissionStatus>,
        opts: &Options,
    ) -> Result<(), Error> {
        let local = local_links(files);
        let md = |html: &str| render::markdown(html, base, &local);

        let mut out = format!("# {}\n\n", m.name);
        let _ = writeln!(out, "- Type: {}", m.modname);
        let _ = writeln!(
            out,
            "- Course: [{} ({})](../{INDEX})",
            self.info.fullname, self.info.shortname
        );
        let _ = writeln!(out, "- cmid: {}", m.id);
        if let Some(url) = &m.url {
            let _ = writeln!(out, "- Moodle: {}", render::scrub(url, base));
        }
        for (label, time) in self.dates(m) {
            let _ = writeln!(out, "- {label} {}", render::date(time));
        }
        for c in m.contents.iter().filter(|c| c.kind == "url") {
            if let Some(url) = &c.fileurl {
                let _ = writeln!(out, "- Link: {}", render::scrub(url, base));
            }
        }

        let page = self.pages.get(&m.id);
        if let Some(d) = self.description(m).map(md).filter(|d| !d.is_empty()) {
            let _ = write!(out, "\n## Description\n\n{d}\n");
        }
        if let Some(c) = page.map(|p| md(&p.content)).filter(|c| !c.is_empty()) {
            let _ = write!(out, "\n## Content\n\n{c}\n");
        }
        if let Some(s) = status {
            write_submission(&mut out, s, &md);
        }

        if !files.is_empty() {
            out.push_str("\n## Files\n\n");
            for f in files {
                let rel = render::rel_path(&f.rel());
                let path = adir.join(f.rel());
                // Embedded files have no size until downloaded.
                let size = (f.size)
                    .or_else(|| std::fs::metadata(&path).ok().map(|m| m.len()))
                    .map_or_else(|| "size unknown".into(), human_size);
                if path.is_file() {
                    let _ = writeln!(out, "- [{rel}](<{rel}>) ({size})");
                } else if opts
                    .max_file_size
                    .zip(f.size)
                    .is_some_and(|(max, s)| s > max)
                {
                    let _ = writeln!(out, "- {rel} ({size}): not downloaded, over the size limit");
                } else {
                    let _ = writeln!(out, "- {rel} ({size}): not downloaded");
                }
            }
        }

        create_dir(adir)?;
        write(&adir.join(INDEX), &out)
    }

    fn write_course_index(
        &self,
        base: &str,
        dir: &Path,
        files: &[RemoteFile],
    ) -> Result<(), Error> {
        let local = local_links(files);
        let md = |html: &str| render::markdown(html, base, &local);
        let now = now();

        let mut out = format!("# {} ({})\n\n", self.info.fullname, self.info.shortname);
        let _ = writeln!(out, "- Course id: {}", self.info.id);
        let _ = writeln!(out, "- Moodle: {base}course/view.php?id={}", self.info.id);
        let _ = writeln!(out, "- Synced: {}", render::date(now));
        if let Some(s) = self
            .info
            .summary
            .as_deref()
            .map(md)
            .filter(|s| !s.is_empty())
        {
            let _ = write!(out, "\n{s}\n");
        }

        for section in &self.sections {
            let name = match section.name.trim() {
                "" => format!("Section {}", section.section.unwrap_or_default()),
                name => name.to_owned(),
            };
            let _ = write!(out, "\n## {name}\n\n");
            let summary = md(&section.summary);
            if !summary.is_empty() {
                let _ = write!(out, "{summary}\n\n");
            }
            for m in &section.modules {
                if m.modname == "label" {
                    if let Some(text) = m.description.as_deref().map(md).filter(|t| !t.is_empty()) {
                        let _ = write!(out, "{text}\n\n");
                    }
                    continue;
                }
                if !mirrored(m) {
                    let _ = writeln!(out, "- {} ({}, not available)", m.name, m.modname);
                    continue;
                }
                let link = format!("{}/{INDEX}", dir_name(&m.name, m.id));
                let _ = write!(out, "- [{}](<{link}>) ({})", m.name, m.modname);
                for (label, time) in self.dates(m) {
                    let _ = write!(out, " · {label} {}", render::date(time));
                }
                out.push('\n');
            }
        }
        write(&dir.join(INDEX), &out)
    }
}

/// Adds Moodle files referenced from `html` that aren't in `files` yet, under
/// [`EMBEDDED_DIR`]. Colliding names (e.g. many pasted `image.png`) get a
/// short hash of their URL appended, so names stay stable across syncs.
fn add_embedded<'a>(
    files: &mut Vec<RemoteFile>,
    html: impl IntoIterator<Item = &'a str>,
    base: &str,
) {
    let mut known: HashSet<String> = (files.iter())
        .map(|f| render::file_key(&f.url).to_owned())
        .collect();
    let mut urls = Vec::new();
    for h in html {
        for url in render::file_urls(h, base) {
            if known.insert(render::file_key(&url).to_owned()) {
                urls.push(url);
            }
        }
    }
    if urls.is_empty() {
        return;
    }

    let prefix = format!("/{EMBEDDED_DIR}/");
    let names: Vec<String> = urls.iter().map(|u| render::url_filename(u)).collect();
    let mut taken: HashMap<String, usize> = HashMap::new();
    let existing = files
        .iter()
        .filter(|f| f.filepath == prefix)
        .map(|f| &f.filename);
    for name in existing.chain(&names) {
        *taken.entry(sanitize(name)).or_default() += 1;
    }
    for (url, name) in urls.into_iter().zip(names) {
        let filename = if taken[&sanitize(&name)] > 1 {
            with_suffix(&name, &render::short_hash(render::file_key(&url)))
        } else {
            name
        };
        files.push(RemoteFile {
            filepath: prefix.clone(),
            filename,
            url,
            size: None,
            timemodified: None,
        });
    }
}

/// `report-1a2b3c4d.pdf` from `report.pdf`.
fn with_suffix(name: &str, suffix: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem}-{suffix}.{ext}"),
        _ => format!("{name}-{suffix}"),
    }
}

/// Keeps only files on this site, deduplicated by local path.
fn finish_files(mut files: Vec<RemoteFile>, base: &str) -> Vec<RemoteFile> {
    files.retain(|f| f.url.starts_with(base));
    let mut seen = HashSet::new();
    files.retain(|f| seen.insert(f.rel()));
    files
}

/// File URL key → path relative to the directory the files live in.
fn local_links(files: &[RemoteFile]) -> HashMap<String, String> {
    (files.iter())
        .map(|f| {
            (
                render::file_key(&f.url).to_owned(),
                render::rel_path(&f.rel()),
            )
        })
        .collect()
}

/// Sorts files into unchanged, skipped (over `limit`) and to-download jobs.
fn plan(
    files: &[RemoteFile],
    dir: &Path,
    limit: Option<u64>,
    report: &mut Report,
    jobs: &mut Vec<(RemoteFile, PathBuf)>,
) {
    for f in files {
        let path = dir.join(f.rel());
        if fsutil::is_unchanged(&path, f.size, f.timemodified) {
            report.unchanged += 1;
        } else if let (Some(max), Some(size)) = (limit, f.size)
            && size > max
        {
            report.skipped.push((path, size));
        } else {
            jobs.push((f.clone(), path));
        }
    }
}

/// Manifest entries for `files` living under `base`, with their on-disk state.
fn entries(
    root: &Path,
    base: &Path,
    cmid: Option<i64>,
    files: &[RemoteFile],
    limit: Option<u64>,
) -> Vec<ManifestFile> {
    files
        .iter()
        .map(|f| {
            let path = base.join(f.rel());
            let state = if path.is_file() {
                FileState::Present
            } else if limit.zip(f.size).is_some_and(|(max, s)| s > max) {
                FileState::Skipped
            } else {
                FileState::Failed
            };
            ManifestFile {
                path: render::rel_path(path.strip_prefix(root).unwrap_or(&path)),
                cmid,
                size: f.size,
                timemodified: f.timemodified,
                state,
            }
        })
        .collect()
}

/// Deletes what the previous sync wrote but the current one no longer has:
/// files gone from Moodle and indexes of removed activities. Never touches
/// files the sync didn't create. Empty directories are removed afterwards.
fn prune(root: &Path, old: &Manifest, new: &Manifest, report: &mut Report) {
    let keep: HashSet<&str> = new.files.iter().map(|f| f.path.as_str()).collect();
    let live: HashSet<i64> = new.activities.iter().map(|a| a.cmid).collect();
    let mut gone: Vec<PathBuf> = (old.files.iter())
        .filter(|f| f.state == FileState::Present && !keep.contains(f.path.as_str()))
        .map(|f| root.join(&f.path))
        .collect();
    gone.extend(
        (old.activities.iter())
            .filter(|a| !live.contains(&a.cmid))
            .map(|a| root.join(&a.dir).join(INDEX)),
    );
    for path in gone {
        // Stay inside the mirror even if the manifest was tampered with.
        if path
            .components()
            .any(|c| c == std::path::Component::ParentDir)
        {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            let mut parent = path.parent();
            while let Some(p) = parent.filter(|p| *p != root && p.starts_with(root)) {
                if std::fs::remove_dir(p).is_err() {
                    break;
                }
                parent = p.parent();
            }
            report.removed.push(path);
        }
    }
    report.removed.sort();
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Modules that get their own directory.
fn mirrored(m: &Module) -> bool {
    m.modname != "label" && m.uservisible != Some(false)
}

/// The submission that counts: the user's own, or the group's.
fn submission(s: &SubmissionStatus) -> Option<&Submission> {
    let last = s.lastattempt.as_ref()?;
    let own = last.submission.as_ref();
    own.filter(|s| s.status != "new")
        .or(last.teamsubmission.as_ref())
        .or(own)
}

fn plugin_files(plugins: &[Plugin], prefix: &str) -> Vec<RemoteFile> {
    plugins
        .iter()
        .flat_map(|p| &p.fileareas)
        .flat_map(|a| &a.files)
        .filter_map(|f| RemoteFile::from_file(f, prefix))
        .collect()
}

fn write_submission(out: &mut String, s: &SubmissionStatus, md: &impl Fn(&str) -> String) {
    out.push_str("\n## Your submission\n\n");
    if let Some(sub) = submission(s) {
        let _ = writeln!(out, "- Status: {}", sub.status);
        if sub.timemodified > 0 {
            let _ = writeln!(out, "- Last modified: {}", render::date(sub.timemodified));
        }
    } else {
        out.push_str("- Status: no submission\n");
    }
    if let Some(last) = &s.lastattempt {
        let _ = writeln!(out, "- Grading: {}", last.gradingstatus);
        if let Some(t) = last.extensionduedate.filter(|t| *t > 0) {
            let _ = writeln!(out, "- Extension due: {}", render::date(t));
        }
    }
    if let Some(fb) = &s.feedback {
        let _ = writeln!(out, "- Grade: {}", md(&fb.gradefordisplay));
        if fb.gradeddate > 0 {
            let _ = writeln!(out, "- Graded: {}", render::date(fb.gradeddate));
        }
    }

    let texts = |plugins: &[Plugin], prefix: &str, out: &mut String| {
        for ef in plugins.iter().flat_map(|p| &p.editorfields) {
            let text = md(&ef.text);
            if !text.is_empty() {
                let _ = write!(out, "\n### {prefix}{}\n\n{text}\n", ef.description);
            }
        }
    };
    if let Some(sub) = submission(s) {
        texts(&sub.plugins, "", out);
    }
    if let Some(fb) = &s.feedback {
        texts(&fb.plugins, "Feedback: ", out);
    }
}

/// Submission status per assignment, keyed by `cmid`. Failures become warnings.
async fn statuses(
    client: &Client,
    assigns: Vec<(i64, i64)>,
    concurrency: usize,
    warnings: &mut Vec<String>,
) -> HashMap<i64, SubmissionStatus> {
    let sem = Arc::new(Semaphore::new(concurrency.max(1)));
    let mut set = JoinSet::new();
    for (cmid, assignid) in assigns {
        let client = client.clone();
        let sem = sem.clone();
        set.spawn(async move {
            let _permit = sem.acquire_owned().await;
            let request = GetSubmissionStatus {
                assignid,
                userid: None,
            };
            (cmid, client.call(&request).await)
        });
    }
    let mut out = HashMap::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((cmid, Ok(status))) => {
                out.insert(cmid, status);
            }
            Ok((cmid, Err(e))) => warnings.push(format!("submission status of {cmid}: {e}")),
            Err(e) => warnings.push(format!("submission status task: {e}")),
        }
    }
    out
}

async fn download_all(
    client: &Client,
    jobs: Vec<(RemoteFile, PathBuf)>,
    concurrency: usize,
    report: &mut Report,
) {
    let sem = Arc::new(Semaphore::new(concurrency.max(1)));
    let mut set = JoinSet::new();
    for (f, path) in jobs {
        let client = client.clone();
        let sem = sem.clone();
        set.spawn(async move {
            let _permit = sem.acquire_owned().await;
            let result = fsutil::download_to(&client, &f.url, &path, f.timemodified).await;
            (path, result)
        });
    }
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((path, Ok(_))) => report.downloaded.push(path),
            Ok((path, Err(e))) => report.failed.push((path, e)),
            Err(e) => report.warnings.push(format!("download task: {e}")),
        }
    }
    report.downloaded.sort();
}

/// Existing `{name} ({cmid})` directories, keyed by `cmid`.
fn existing_activity_dirs(dir: &Path) -> HashMap<i64, PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return HashMap::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| Some((fsutil::parse_dir_id(e.file_name().to_str()?)?, e.path())))
        .collect()
}

fn create_dir(dir: &Path) -> Result<(), Error> {
    std::fs::create_dir_all(dir).map_err(|e| Error::Io(dir.to_owned(), e))
}

fn write(path: &Path, contents: &str) -> Result<(), Error> {
    std::fs::write(path, contents).map_err(|e| Error::Io(path.to_owned(), e))
}
