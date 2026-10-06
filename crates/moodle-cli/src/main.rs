//! `moodle-cli`: read-only Moodle access, built for agents first.
//!
//! - Output is JSON on stdout: pretty on a terminal, compact when piped.
//! - Errors are JSON on stderr: `{"error":{"code","message","hint"}}`.
//! - Exit codes: 0 ok, 1 error, 2 partial (some files failed to download).
//! - Courses are cloned into local directories, like git, and updated with `pull`.

mod config;
mod output;

use std::collections::BTreeMap;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use moodle_api::Client;
use moodle_api::calendar::GetActionEventsByTimesort;
use moodle_api::course::GetUsersCourses;
use moodle_api::site::GetSiteInfo;
use moodle_sync::{Course, FileState, Manifest, Report};
use serde::Serialize;

use config::{Credentials, MirrorConfig};
use output::{emit, hinted, progress};

const AGENT_GUIDE: &str = "\
Workflow:
  moodle-cli todo                    What needs doing next, across all courses
  moodle-cli courses                 Find a course id
  moodle-cli clone <course_id>       Mirror it into ./<shortname> (<id>)/
  cd '<dir>' && read index.md        Course outline; each activity has <name> (<cmid>)/index.md
  grep -ri <term> .                  Search descriptions, pages and downloaded files
  moodle-cli pull                    Refresh before trusting dates or submission status
  moodle-cli status                  What was skipped (size limit) or failed, offline
  moodle-cli fetch <path|cmid>       Download skipped files

Output is JSON (compact when piped; filter with jq). Errors are JSON on stderr
with a `hint`. Exit codes: 0 ok, 1 error, 2 some files failed.
Everything is read-only: nothing is ever submitted or changed on Moodle.";

/// Read-only Moodle access for agents and humans.
#[derive(Parser)]
#[command(version, after_help = AGENT_GUIDE)]
struct Cli {
    /// Run as if started in this directory (for mirror commands).
    #[arg(short = 'C', global = true, value_name = "DIR")]
    dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Log in via browser SSO (or with --token) and store the credentials
    Login {
        /// Site root, e.g. `https://moodle.example.edu/`
        #[arg(long)]
        url: String,
        /// Raw token or moodledl://token=... URL; skips the interactive prompt
        #[arg(long)]
        token: Option<String>,
    },
    /// Delete the stored credentials
    Logout,
    /// Show the logged-in user and site
    Whoami,
    /// List enrolled courses (current ones unless --all)
    #[command(alias = "list_courses", alias = "list-courses")]
    Courses {
        /// Include past courses
        #[arg(long)]
        all: bool,
    },
    /// Upcoming deadlines and actions across courses (assignments, quizzes, ...)
    Todo {
        /// Look this many days ahead
        #[arg(long, default_value_t = 14)]
        days: i64,
        /// Include overdue items up to this many days back
        #[arg(long, default_value_t = 30)]
        overdue_days: i64,
        /// Only this course
        #[arg(long)]
        course: Option<i64>,
    },
    /// Mirror a course into a new directory (default: `./<shortname> (<id>)`)
    Clone {
        course_id: i64,
        dir: Option<PathBuf>,
        /// Skip files larger than this many MB (0 = no limit) [default: 100]
        #[arg(long)]
        max_file_mb: Option<u64>,
    },
    /// Update the current mirror from Moodle
    Pull {
        /// Change the mirror's size limit (MB, 0 = no limit)
        #[arg(long)]
        max_file_mb: Option<u64>,
    },
    /// Show the current mirror's state without contacting Moodle
    Status,
    /// Download files skipped by the size limit (or failed) in the current mirror
    Fetch {
        /// Activity cmids, activity directories or file paths
        targets: Vec<String>,
        /// Fetch every skipped or failed file
        #[arg(long, conflicts_with = "targets")]
        all: bool,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();
    match run(cli).await {
        Ok(code) => code,
        Err(e) => {
            output::emit_error(&e);
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<ExitCode> {
    let dir = cli.dir.as_deref();
    match cli.command {
        Command::Login { url, token } => login(&url, token).await,
        Command::Logout => logout(),
        Command::Whoami => whoami().await,
        Command::Courses { all } => courses(all).await,
        Command::Todo {
            days,
            overdue_days,
            course,
        } => todo(dir, days, overdue_days, course).await,
        Command::Clone {
            course_id,
            dir: target,
            max_file_mb,
        } => {
            let target = target.map(|t| match dir {
                Some(base) => base.join(t),
                None => t,
            });
            clone(course_id, target, max_file_mb).await
        }
        Command::Pull { max_file_mb } => pull(dir, max_file_mb).await,
        Command::Status => status(dir),
        Command::Fetch { targets, all } => fetch(dir, targets, all).await,
    }
}

fn ok() -> Result<ExitCode> {
    Ok(ExitCode::SUCCESS)
}

async fn login(url: &str, token: Option<String>) -> Result<ExitCode> {
    let token = match token {
        Some(t) => config::parse_token(&t)?,
        None => {
            if !std::io::stdin().is_terminal() {
                return Err(hinted(
                    "interactive_required",
                    "login needs a browser and a terminal",
                    "Ask the user to run `moodle-cli login --url <site>` themselves, or pass --token.",
                ));
            }
            let base = if url.ends_with('/') {
                url.to_owned()
            } else {
                format!("{url}/")
            };
            let launch = moodle_api::auth::LaunchRequest {
                service: "moodle_mobile_app".into(),
                passport: passport(),
                urlscheme: "moodledl".into(),
            };
            eprintln!(
                "1. Open this URL in a browser and log in:\n\n   {}\n",
                launch.url(&base)
            );
            eprintln!("2. The browser then fails to open a moodledl://token=... link.");
            eprintln!("   Copy it (e.g. from the dev tools network tab or console).\n");
            eprint!("Paste it here: ");
            std::io::stderr().flush()?;
            let mut line = String::new();
            std::io::stdin().lock().read_line(&mut line)?;
            config::parse_token(&line)?
        }
    };

    let client = Client::new(url, token.clone())?;
    let info = client
        .call(&GetSiteInfo)
        .await
        .context("the token didn't work")?;
    let path = config::save_credentials(&Credentials {
        url: client.base().to_owned(),
        token,
    })?;

    #[derive(Serialize)]
    struct Out {
        site: String,
        user: String,
        userid: i64,
        credentials: PathBuf,
    }
    emit(&Out {
        site: client.base().to_owned(),
        user: info.fullname,
        userid: info.userid,
        credentials: path,
    });
    ok()
}

fn passport() -> String {
    use std::hash::{BuildHasher, RandomState};
    let a = RandomState::new().hash_one(0u8);
    let b = RandomState::new().hash_one(1u8);
    format!("{a:016x}{b:016x}")
}

fn logout() -> Result<ExitCode> {
    let path = config::credentials_path()?;
    let removed = match std::fs::remove_file(&path) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(e).with_context(|| format!("remove {}", path.display())),
    };
    emit(&serde_json::json!({ "logged_out": removed, "credentials": path }));
    ok()
}

async fn whoami() -> Result<ExitCode> {
    let client = config::client()?;
    let info = client.call(&GetSiteInfo).await?;

    #[derive(Serialize)]
    struct Out {
        site: String,
        sitename: String,
        user: String,
        username: String,
        userid: i64,
        release: Option<String>,
    }
    emit(&Out {
        site: client.base().to_owned(),
        sitename: info.sitename,
        user: info.fullname,
        username: info.username,
        userid: info.userid,
        release: info.release,
    });
    ok()
}

async fn courses(all: bool) -> Result<ExitCode> {
    let client = config::client()?;
    let me = client.call(&GetSiteInfo).await?;
    let mut courses = client.call(&GetUsersCourses { userid: me.userid }).await?;
    let now = unix_now();
    if !all {
        courses.retain(|c| c.enddate.is_none_or(|end| end == 0 || end >= now));
    }
    courses.sort_by_key(|c| std::cmp::Reverse(c.startdate));

    #[derive(Serialize)]
    struct Out {
        id: i64,
        shortname: String,
        fullname: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        start: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        end: Option<String>,
    }
    let iso = |t: Option<i64>| t.filter(|t| *t > 0).map(moodle_sync::format_iso);
    let out: Vec<Out> = courses
        .into_iter()
        .map(|c| Out {
            id: c.id,
            start: iso(c.startdate),
            end: iso(c.enddate),
            shortname: c.shortname,
            fullname: c.fullname,
        })
        .collect();
    emit(&out);
    ok()
}

async fn todo(
    dir: Option<&Path>,
    days: i64,
    overdue_days: i64,
    course: Option<i64>,
) -> Result<ExitCode> {
    let client = config::client()?;
    let now = unix_now();
    let mut events = Vec::new();
    let mut after = None;
    // Moodle pages at up to 50 events; a few pages cover any realistic window.
    for _ in 0..10 {
        let page = client
            .call(&GetActionEventsByTimesort {
                timesortfrom: Some(now - overdue_days.max(0) * 86_400),
                timesortto: Some(now + days.max(0) * 86_400),
                aftereventid: after,
                limitnum: Some(50),
                limittononsuspendedevents: Some(true),
            })
            .await?;
        let full = page.events.len() >= 50;
        events.extend(page.events);
        match page.lastid {
            Some(id) if full => after = Some(id),
            _ => break,
        }
    }

    // Link to local activity indexes when run inside a mirror of that course.
    let mirror = config::try_find_mirror(dir)?.and_then(|(root, cfg)| {
        let activities = Manifest::load(&root)?.activities;
        Some((cfg.course_id, root, activities))
    });

    #[derive(Serialize)]
    struct Item {
        due: String,
        overdue: bool,
        course_id: Option<i64>,
        course: Option<String>,
        activity: String,
        #[serde(rename = "type")]
        kind: Option<String>,
        cmid: Option<i64>,
        /// What Moodle expects you to do, e.g. "Add submission".
        action: Option<String>,
        /// False if the action can't be taken yet (e.g. not open).
        actionable: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        index: Option<PathBuf>,
    }
    let items: Vec<Item> = events
        .into_iter()
        .filter(|e| course.is_none_or(|c| e.course.as_ref().is_some_and(|ec| ec.id == c)))
        .map(|e| {
            let cmid = e.cmid();
            let course_id = e.course.as_ref().map(|c| c.id);
            let index = mirror.as_ref().and_then(|(cid, root, acts)| {
                let a = acts
                    .iter()
                    .find(|a| Some(a.cmid) == cmid && Some(*cid) == course_id)?;
                Some(root.join(&a.dir).join(moodle_sync::INDEX))
            });
            let when = e.timesort.unwrap_or(e.timestart);
            Item {
                due: moodle_sync::format_iso(when),
                overdue: e.overdue.unwrap_or(when < now),
                course_id,
                course: e.course.and_then(|c| c.shortname.or(c.fullname)),
                activity: e.activityname.unwrap_or(e.name),
                kind: e.modulename,
                cmid,
                action: e.action.as_ref().map(|a| a.name.clone()),
                actionable: e.action.is_some_and(|a| a.actionable),
                index,
            }
        })
        .collect();
    emit(&items);
    ok()
}

async fn clone(
    course_id: i64,
    target: Option<PathBuf>,
    max_file_mb: Option<u64>,
) -> Result<ExitCode> {
    let client = config::client()?;
    progress(&format!("Fetching course {course_id}..."));
    let course = Course::fetch(&client, course_id).await?;
    let root = std::path::absolute(target.unwrap_or_else(|| PathBuf::from(course.dir_name())))?;

    if let Some(existing) = MirrorConfig::load(&root) {
        let hint = if existing.course_id == course_id {
            format!(
                "Run `moodle-cli -C '{}' pull` to update it.",
                root.display()
            )
        } else {
            "Choose another directory.".to_owned()
        };
        return Err(hinted(
            "already_cloned",
            format!(
                "{} is already a mirror of course {}",
                root.display(),
                existing.course_id
            ),
            hint,
        ));
    }
    if root.read_dir().is_ok_and(|mut d| d.next().is_some()) {
        return Err(hinted(
            "dir_not_empty",
            format!("{} exists and is not empty", root.display()),
            "Pass an empty or new directory as the second argument.",
        ));
    }

    let config = MirrorConfig {
        site: client.base().to_owned(),
        course_id,
        max_file_mb: max_file_mb.unwrap_or(default_max_mb()),
    };
    config.save(&root)?;
    progress(&format!(
        "Syncing {} into {}...",
        course.fullname(),
        root.display()
    ));
    let report = course.sync(&client, &root, &config.options()).await?;
    emit_report(&root, &course, &report)
}

async fn pull(dir: Option<&Path>, max_file_mb: Option<u64>) -> Result<ExitCode> {
    let (root, mut config) = config::find_mirror(dir)?;
    let client = config::client()?;
    config::check_site(&config, &client)?;
    if let Some(mb) = max_file_mb {
        config.max_file_mb = mb;
        config.save(&root)?;
    }
    progress(&format!("Pulling course {}...", config.course_id));
    let course = Course::fetch(&client, config.course_id).await?;
    let report = course.sync(&client, &root, &config.options()).await?;
    emit_report(&root, &course, &report)
}

fn status(dir: Option<&Path>) -> Result<ExitCode> {
    let (root, config) = config::find_mirror(dir)?;
    let manifest = Manifest::load(&root);

    #[derive(Serialize)]
    struct FileOut<'a> {
        path: &'a str,
        cmid: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
    }
    let list = |state: FileState| -> Vec<FileOut<'_>> {
        manifest
            .iter()
            .flat_map(|m| &m.files)
            .filter(|f| f.state == state)
            .map(|f| FileOut {
                path: &f.path,
                cmid: f.cmid,
                size: f.size,
            })
            .collect()
    };
    let synced = manifest.as_ref().map(|m| m.synced_at).filter(|t| *t > 0);
    let present = list(FileState::Present).len();
    emit(&serde_json::json!({
        "root": root,
        "site": config.site,
        "course_id": config.course_id,
        "max_file_mb": config.max_file_mb,
        "synced_at": synced.map(moodle_sync::format_iso),
        "activities": manifest.as_ref().map_or(0, |m| m.activities.len()),
        "files_present": present,
        "skipped": list(FileState::Skipped),
        "failed": list(FileState::Failed),
    }));
    ok()
}

async fn fetch(dir: Option<&Path>, targets: Vec<String>, all: bool) -> Result<ExitCode> {
    let (root, config) = config::find_mirror(dir)?;
    let manifest = Manifest::load(&root).ok_or_else(|| {
        hinted(
            "no_manifest",
            "this mirror has no manifest yet",
            "Run `moodle-cli pull` first.",
        )
    })?;
    if targets.is_empty() && !all {
        return Err(hinted(
            "no_targets",
            "nothing to fetch",
            "Pass cmids, activity directories or file paths (see `moodle-cli status`), or --all.",
        ));
    }

    // cmid → file paths relative to the activity dir (`None` = all files).
    let mut wanted: BTreeMap<i64, Option<Vec<String>>> = BTreeMap::new();
    let activity_dir = |cmid: i64| {
        manifest
            .activities
            .iter()
            .find(|a| a.cmid == cmid)
            .map(|a| a.dir.as_str())
    };
    let mut add = |cmid: i64, file: Option<String>| match (
        wanted.entry(cmid).or_insert(Some(Vec::new())),
        file,
    ) {
        (Some(files), Some(f)) => files.push(f),
        (slot, None) => *slot = None,
        (None, Some(_)) => {}
    };
    if all {
        for f in &manifest.files {
            if f.state != FileState::Present
                && let Some(cmid) = f.cmid
                && let Some(adir) = activity_dir(cmid)
            {
                let rel = f.path.strip_prefix(adir).unwrap_or(&f.path);
                add(cmid, Some(rel.trim_start_matches('/').to_owned()));
            }
        }
    }
    let cwd = match dir {
        Some(d) => std::path::absolute(d)?,
        None => std::env::current_dir()?,
    };
    for target in &targets {
        if let Ok(cmid) = target.parse::<i64>() {
            add(cmid, None);
            continue;
        }
        let abs = std::path::absolute(cwd.join(target))?;
        let rel = abs
            .strip_prefix(&root)
            .map(|p| {
                p.components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .map_err(|_| {
                hinted(
                    "outside_mirror",
                    format!("{target} is outside the mirror"),
                    "Use paths inside the mirror.",
                )
            })?;
        if let Some(a) = manifest.activities.iter().find(|a| a.dir == rel) {
            add(a.cmid, None);
        } else if let Some(f) = manifest.files.iter().find(|f| f.path == rel) {
            let Some((cmid, adir)) = f.cmid.and_then(|c| Some((c, activity_dir(c)?))) else {
                return Err(hinted(
                    "not_fetchable",
                    format!("{target} is not part of an activity"),
                    "Run `moodle-cli pull` instead.",
                ));
            };
            add(
                cmid,
                Some(rel[adir.len()..].trim_start_matches('/').to_owned()),
            );
        } else {
            return Err(hinted(
                "unknown_target",
                format!("{target} is not a file or activity in this mirror"),
                "See `moodle-cli status` for skipped files, or pass an activity cmid.",
            ));
        }
    }
    if wanted.is_empty() {
        emit(
            &serde_json::json!({ "root": root, "downloaded": [], "note": "nothing was skipped or failed" }),
        );
        return ok();
    }

    let client = config::client()?;
    config::check_site(&config, &client)?;
    let course = Course::fetch(&client, config.course_id).await?;
    let mut total = Report::default();
    for (cmid, names) in wanted {
        progress(&format!("Fetching activity {cmid}..."));
        let r = course
            .download(&client, &root, cmid, names.as_deref(), &config.options())
            .await?;
        total.downloaded.extend(r.downloaded);
        total.unchanged += r.unchanged;
        total.failed.extend(r.failed);
        total.missing.extend(r.missing);
        total.warnings.extend(r.warnings);
    }
    total.index = root.join(moodle_sync::INDEX);
    emit_report(&root, &course, &total)
}

/// Shared output of clone/pull/fetch: absolute root, paths relative to it.
fn emit_report(root: &Path, course: &Course, r: &Report) -> Result<ExitCode> {
    let rel = |p: &Path| moodle_rel(root, p);
    #[derive(Serialize)]
    struct Skipped {
        path: String,
        size: u64,
    }
    #[derive(Serialize)]
    struct Failed {
        path: String,
        error: String,
    }
    #[derive(Serialize)]
    struct Out {
        root: PathBuf,
        index: PathBuf,
        course_id: i64,
        course: String,
        downloaded: Vec<String>,
        unchanged: usize,
        skipped: Vec<Skipped>,
        failed: Vec<Failed>,
        removed: Vec<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        missing: Vec<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        warnings: Vec<String>,
    }
    emit(&Out {
        root: root.to_owned(),
        index: r.index.clone(),
        course_id: course.id(),
        course: course.fullname().to_owned(),
        downloaded: r.downloaded.iter().map(|p| rel(p)).collect(),
        unchanged: r.unchanged,
        skipped: (r.skipped.iter())
            .map(|(p, size)| Skipped {
                path: rel(p),
                size: *size,
            })
            .collect(),
        failed: (r.failed.iter())
            .map(|(p, e)| Failed {
                path: rel(p),
                error: e.clone(),
            })
            .collect(),
        removed: r.removed.iter().map(|p| rel(p)).collect(),
        missing: r.missing.clone(),
        warnings: r.warnings.clone(),
    });
    Ok(if r.failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    })
}

fn moodle_rel(root: &Path, p: &Path) -> String {
    let rel = p.strip_prefix(root).unwrap_or(p);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn default_max_mb() -> u64 {
    moodle_sync::Options::default()
        .max_file_size
        .map_or(0, |b| b / (1024 * 1024))
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}
