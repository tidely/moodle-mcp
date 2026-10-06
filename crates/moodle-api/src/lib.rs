//! Moodle web service protocol types and a thin async [`Client`].
//!
//! These mirror what `../Moodle-DL` sends and receives. Response structs only
//! list the fields we care about; serde ignores the rest.
//!
//! # Transport
//!
//! Every web service call is a form-encoded POST with the MoodleMobile
//! User-Agent ([`USER_AGENT`]):
//!
//! ```text
//! POST {base}webservice/rest/server.php?moodlewsrestformat=json&wsfunction={NAME}
//! Content-Type: application/x-www-form-urlencoded
//!
//! moodlewssettingfilter=true&moodlewssettingfileurl=true&{params}&wsfunction={NAME}&wstoken={token}
//! ```
//!
//! Arrays and nested objects are flattened PHP-style: `courseids[0]=12&courseids[1]=34`,
//! `options[0][name]=excludemodules&options[0][value]=true`.
//!
//! The HTTP status is 200 even on failure. A failed call returns a
//! [`MoodleException`] body instead of the expected response;
//! [`Client::call`] turns that into [`Error::Moodle`].
//!
//! # Request/response pairs
//!
//! Each request type implements [`WsFunction`], whose `Response` associated
//! type is its pair. Summary:
//!
//! | Request                         | Endpoint / `wsfunction`              | Response                        |
//! |---------------------------------|--------------------------------------|---------------------------------|
//! | [`auth::LaunchRequest`]         | `GET admin/tool/mobile/launch.php`   | [`auth::SsoToken`] (redirect)   |
//! | [`auth::TokenRequest`]          | `POST login/token.php`               | [`auth::TokenResponse`]         |
//! | [`site::GetSiteInfo`]           | `core_webservice_get_site_info`      | [`site::SiteInfo`]              |
//! | [`course::GetUsersCourses`]     | `core_enrol_get_users_courses`       | `Vec<`[`course::EnrolledCourse`]`>` |
//! | [`course::GetCoursesByField`]   | `core_course_get_courses_by_field`   | [`course::CoursesByField`]      |
//! | [`course::GetCourseContents`]   | `core_course_get_contents`           | `Vec<`[`course::Section`]`>`    |
//! | [`course::GetCourseModule`]     | `core_course_get_course_module`      | [`course::CourseModuleInfo`]    |
//! | [`assign::GetAssignments`]      | `mod_assign_get_assignments`         | [`assign::Assignments`]         |
//! | [`assign::GetSubmissionStatus`] | `mod_assign_get_submission_status`   | [`assign::SubmissionStatus`]    |
//! | [`resource::GetFolders`]        | `mod_folder_get_folders_by_courses`  | [`resource::Folders`]           |
//! | [`resource::GetPages`]          | `mod_page_get_pages_by_courses`      | [`resource::Pages`]             |
//! | [`resource::GetBooks`]          | `mod_book_get_books_by_courses`      | [`resource::Books`]             |
//! | [`calendar::GetActionEventsByTimesort`] | `core_calendar_get_action_events_by_timesort` | [`calendar::ActionEvents`] |
//! | any [`File::fileurl`]           | `GET {fileurl}?token={wstoken}`      | raw file bytes ([`Client::download`]) |
//!
//! Typical student flow: `GetSiteInfo` (user id) → `GetUsersCourses` →
//! `GetCourseContents` (sections, modules, resource files) → the per-module
//! `Get*` calls for assignments/folders/pages/books → download `fileurl`s.

pub mod assign;
pub mod auth;
pub mod calendar;
mod client;
pub mod course;
pub mod resource;
pub mod site;

pub use client::{Client, Error, FileDownload};

use serde::Deserialize;
use serde::de::DeserializeOwned;

/// Moodle-DL's User-Agent. Some instances reject anything without `MoodleMobile`.
pub const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 7.1.1; Moto G Play Build/NPIS26.48-43-2; wv) \
    AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/71.0.3578.99 Mobile Safari/537.36 MoodleMobile";

/// A Moodle web service function: a request type paired with its response type.
pub trait WsFunction {
    /// Value of the `wsfunction` parameter.
    const NAME: &'static str;
    /// The JSON body Moodle returns on success.
    type Response: DeserializeOwned;
    /// Function-specific form fields (excluding `wstoken`/`wsfunction`).
    fn params(&self) -> Vec<(String, String)>;
}

/// `{base}webservice/rest/server.php?moodlewsrestformat=json&wsfunction={NAME}`.
/// `base` must end with `/`.
pub fn rest_url<F: WsFunction>(base: &str) -> String {
    format!(
        "{base}webservice/rest/server.php?moodlewsrestformat=json&wsfunction={}",
        F::NAME
    )
}

/// The complete form body for a web service call.
pub fn form_body<F: WsFunction>(token: &str, request: &F) -> Vec<(String, String)> {
    let mut body = vec![
        // Apply Moodle text filters and rewrite pluginfile URLs to webservice/pluginfile.php.
        ("moodlewssettingfilter".into(), "true".into()),
        ("moodlewssettingfileurl".into(), "true".into()),
    ];
    body.extend(request.params());
    body.push(("wsfunction".into(), F::NAME.into()));
    body.push(("wstoken".into(), token.into()));
    body
}

/// Flattens a list into PHP-style indexed fields: `name[0]=a&name[1]=b`.
pub(crate) fn indexed<T: ToString>(
    name: &str,
    items: impl IntoIterator<Item = T>,
) -> Vec<(String, String)> {
    items
        .into_iter()
        .enumerate()
        .map(|(i, v)| (format!("{name}[{i}]"), v.to_string()))
        .collect()
}

/// Error body returned by `webservice/rest/server.php` (with HTTP 200).
#[derive(Debug, Deserialize)]
pub struct MoodleException {
    /// PHP exception class, e.g. `moodle_exception`.
    pub exception: String,
    /// e.g. `invalidtoken` (token expired), `ex_unabletolock` (retryable).
    pub errorcode: String,
    pub message: String,
    pub debuginfo: Option<String>,
}

/// Non-fatal per-item problem, returned alongside partial results.
#[derive(Debug, Deserialize)]
pub struct Warning {
    pub item: Option<String>,
    pub itemid: Option<i64>,
    pub warningcode: String,
    pub message: String,
}

/// Moodle's shared `external_files` structure (intro files, attachments,
/// submission files, ...). Download with `?token={wstoken}` appended to `fileurl`.
#[derive(Debug, Deserialize)]
pub struct File {
    pub filename: Option<String>,
    pub filepath: Option<String>,
    pub filesize: Option<u64>,
    pub fileurl: Option<String>,
    pub timemodified: Option<i64>,
    pub mimetype: Option<String>,
    pub isexternalfile: Option<bool>,
}

/// Format of HTML-ish text fields (`intro`, `summary`, `content`, ...).
/// 0 = Moodle auto, 1 = HTML, 2 = plain, 4 = Markdown.
pub type TextFormat = i32;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_body_flattens_arrays_and_appends_auth() {
        let req = assign::GetAssignments {
            courseids: vec![12, 34],
        };
        let body = form_body("TOKEN", &req);
        let pairs: Vec<(&str, &str)> = body.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(
            pairs,
            [
                ("moodlewssettingfilter", "true"),
                ("moodlewssettingfileurl", "true"),
                ("courseids[0]", "12"),
                ("courseids[1]", "34"),
                ("wsfunction", "mod_assign_get_assignments"),
                ("wstoken", "TOKEN"),
            ]
        );
        assert_eq!(
            rest_url::<assign::GetAssignments>("https://m.example/"),
            "https://m.example/webservice/rest/server.php?moodlewsrestformat=json&wsfunction=mod_assign_get_assignments"
        );
    }

    #[test]
    fn decodes_exception() {
        let json = r#"{"exception":"moodle_exception","errorcode":"invalidtoken","message":"Invalid token"}"#;
        let e: MoodleException = serde_json::from_str(json).unwrap();
        assert_eq!(e.errorcode, "invalidtoken");
    }

    #[test]
    fn decodes_course_contents() {
        let json = r#"[{"id":1,"name":"General","section":0,"summary":"","summaryformat":1,
            "modules":[{"id":7,"name":"Slides","modname":"resource","instance":3,
              "contents":[{"type":"file","filename":"a.pdf","filepath":"/","filesize":10,
                "fileurl":"https://m.example/webservice/pluginfile.php/1/a.pdf","timemodified":0}]}]}]"#;
        let sections: Vec<course::Section> = serde_json::from_str(json).unwrap();
        assert_eq!(sections[0].modules[0].contents[0].filename, "a.pdf");
    }
}
