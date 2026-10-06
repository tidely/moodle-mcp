use std::fmt::Write;
use std::path::Path;
use std::sync::Arc;

use moodle_api::Client;
use moodle_api::course::GetUsersCourses;
use moodle_api::site::GetSiteInfo;
use moodle_sync::{Course, INDEX, Options, Report, human_size};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;

use crate::cache::Cache;

#[derive(Clone)]
pub struct MoodleMcp {
    client: Arc<Client>,
    cache: Cache,
    options: Options,
    #[allow(dead_code, reason = "used by the tool_handler macro")]
    tool_router: ToolRouter<Self>,
}

impl MoodleMcp {
    pub fn new(client: Client, cache: Cache, options: Options) -> Self {
        Self {
            client: Arc::new(client),
            cache,
            options,
            tool_router: Self::tool_router(),
        }
    }

    fn course_dir(&self, course: &Course) -> std::path::PathBuf {
        self.cache
            .course_dir(self.client.host(), course.shortname(), course.id())
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SyncCourseParams {
    /// Course id, from `list_courses`.
    pub course_id: i64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DownloadParams {
    /// Course module id (`cmid`) of the activity, as shown in the course index.
    pub cmid: i64,
    /// Only download files with these names. Omit to download all files of the activity.
    #[serde(default)]
    pub files: Option<Vec<String>>,
}

#[tool_router]
impl MoodleMcp {
    #[tool(
        description = "List the Moodle courses the user is enrolled in, with course ids and local mirror paths."
    )]
    async fn list_courses(&self) -> Result<String, String> {
        let client = &self.client;
        let me = client.call(&GetSiteInfo).await.map_err(|e| e.to_string())?;
        let mut courses = client
            .call(&GetUsersCourses { userid: me.userid })
            .await
            .map_err(|e| e.to_string())?;
        courses.sort_by_key(|c| std::cmp::Reverse(c.startdate));

        let mut out = String::from("# Courses\n\n");
        for c in courses {
            let dir = self.cache.course_dir(client.host(), &c.shortname, c.id);
            let index = dir.join(INDEX);
            let state = if index.is_file() {
                format!("synced: {}", index.display())
            } else {
                "not synced".to_owned()
            };
            let _ = writeln!(
                out,
                "- {} ({}): course_id {}, {state}",
                c.fullname, c.shortname, c.id
            );
        }
        Ok(out)
    }

    #[tool(
        description = "Mirror a Moodle course to the local cache: an index.md with sections, activities and dates, and per activity a folder with its own index.md (description, dates, submission status) and its files. Read and search the mirror with your file tools. Re-run to refresh; unchanged files are kept. Large files are listed but skipped; fetch them with `download`."
    )]
    async fn sync_course(
        &self,
        Parameters(params): Parameters<SyncCourseParams>,
    ) -> Result<String, String> {
        let course = Course::fetch(&self.client, params.course_id)
            .await
            .map_err(|e| e.to_string())?;
        let dir = self.course_dir(&course);
        let report = course
            .sync(&self.client, &dir, &self.options)
            .await
            .map_err(|e| e.to_string())?;

        let mut out = format!(
            "# Synced {} ({})\nFolder: {}\n\n",
            course.fullname(),
            course.shortname(),
            dir.display()
        );
        format_report(&mut out, &report, &dir);
        Ok(out)
    }

    #[tool(
        description = "Download the files of one activity into the local course mirror, including files skipped by `sync_course` for being too large. Returns absolute paths."
    )]
    async fn download(
        &self,
        Parameters(params): Parameters<DownloadParams>,
    ) -> Result<String, String> {
        let course_id = moodle_sync::course_of(&self.client, params.cmid)
            .await
            .map_err(|e| e.to_string())?;
        let course = Course::fetch(&self.client, course_id)
            .await
            .map_err(|e| e.to_string())?;
        let dir = self.course_dir(&course);
        let report = course
            .download(
                &self.client,
                &dir,
                params.cmid,
                params.files.as_deref(),
                &self.options,
            )
            .await
            .map_err(|e| e.to_string())?;

        let mut out = format!("# Download (cmid {})\n\n", params.cmid);
        format_report(&mut out, &report, &dir);
        for path in &report.downloaded {
            let _ = writeln!(out, "- {}", path.display());
        }
        Ok(out)
    }
}

fn format_report(out: &mut String, r: &Report, dir: &Path) {
    let _ = writeln!(out, "Index: {}", r.index.display());
    let _ = writeln!(
        out,
        "Downloaded: {}, unchanged: {}, skipped (too large): {}, failed: {}",
        r.downloaded.len(),
        r.unchanged,
        r.skipped.len(),
        r.failed.len()
    );
    let rel = |p: &Path| p.strip_prefix(dir).unwrap_or(p).display().to_string();
    if !r.skipped.is_empty() {
        out.push_str("\nSkipped (use `download`):\n");
        for (path, size) in &r.skipped {
            let _ = writeln!(out, "- {} ({})", rel(path), human_size(*size));
        }
    }
    if !r.failed.is_empty() {
        out.push_str("\nFailed:\n");
        for (path, e) in &r.failed {
            let _ = writeln!(out, "- {}: {e}", rel(path));
        }
    }
    if !r.missing.is_empty() {
        let _ = writeln!(out, "\nNot found: {}", r.missing.join(", "));
    }
    if !r.warnings.is_empty() {
        out.push_str("\nWarnings:\n");
        for w in &r.warnings {
            let _ = writeln!(out, "- {w}");
        }
    }
}

#[tool_handler]
impl ServerHandler for MoodleMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("moodle-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Read-only access to the user's Moodle courses. Call `list_courses`, then \
                 `sync_course` to mirror a course to local files, and explore the mirror with \
                 your own file tools starting at the returned index.md. Re-sync before relying \
                 on dates or submission status.",
            )
    }
}
