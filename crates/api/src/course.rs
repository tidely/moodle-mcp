use serde::Deserialize;

use super::{File, TextFormat, Warning, WsFunction};

/// `core_enrol_get_users_courses`: courses the user is enrolled in.
#[derive(Debug)]
pub struct GetUsersCourses {
    /// From [`super::site::SiteInfo::userid`].
    pub userid: i64,
}

impl WsFunction for GetUsersCourses {
    const NAME: &'static str = "core_enrol_get_users_courses";
    /// A bare JSON array.
    type Response = Vec<EnrolledCourse>;

    fn params(&self) -> Vec<(String, String)> {
        vec![("userid".into(), self.userid.to_string())]
    }
}

#[derive(Debug, Deserialize)]
pub struct EnrolledCourse {
    pub id: i64,
    pub shortname: String,
    pub fullname: String,
    pub summary: Option<String>,
    pub summaryformat: Option<TextFormat>,
    /// 1 = visible, 0 = hidden.
    pub visible: Option<i32>,
    pub startdate: Option<i64>,
    pub enddate: Option<i64>,
    #[serde(default)]
    pub overviewfiles: Vec<File>,
}

/// `core_course_get_courses_by_field` (Moodle 3.2+).
/// With no field/value it returns every course visible on the site.
#[derive(Debug, Default)]
pub struct GetCoursesByField {
    /// `id`, `ids`, `shortname`, `idnumber` or `category`.
    pub field: Option<String>,
    /// For `ids`: comma-separated, e.g. `"12,34"`.
    pub value: Option<String>,
}

impl WsFunction for GetCoursesByField {
    const NAME: &'static str = "core_course_get_courses_by_field";
    type Response = CoursesByField;

    fn params(&self) -> Vec<(String, String)> {
        let mut p = Vec::new();
        if let Some(field) = &self.field {
            p.push(("field".into(), field.clone()));
        }
        if let Some(value) = &self.value {
            p.push(("value".into(), value.clone()));
        }
        p
    }
}

#[derive(Debug, Deserialize)]
pub struct CoursesByField {
    pub courses: Vec<Course>,
    #[serde(default)]
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Deserialize)]
pub struct Course {
    pub id: i64,
    pub fullname: String,
    pub shortname: String,
    pub categoryname: Option<String>,
    pub summary: Option<String>,
    pub summaryformat: Option<TextFormat>,
    pub visible: Option<i32>,
    #[serde(default)]
    pub summaryfiles: Vec<File>,
    #[serde(default)]
    pub overviewfiles: Vec<File>,
}

/// `core_course_get_course_module`: resolves a `cmid` to its course, module
/// type and instance id.
#[derive(Debug)]
pub struct GetCourseModule {
    pub cmid: i64,
}

impl WsFunction for GetCourseModule {
    const NAME: &'static str = "core_course_get_course_module";
    type Response = CourseModuleInfo;

    fn params(&self) -> Vec<(String, String)> {
        vec![("cmid".into(), self.cmid.to_string())]
    }
}

#[derive(Debug, Deserialize)]
pub struct CourseModuleInfo {
    pub cm: CourseModule,
    #[serde(default)]
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Deserialize)]
pub struct CourseModule {
    /// The `cmid`.
    pub id: i64,
    pub course: i64,
    pub name: String,
    /// Module type: `assign`, `resource`, `folder`, ...
    pub modname: String,
    /// Instance id, matches `id` in the per-module responses.
    pub instance: i64,
    pub sectionnum: Option<i32>,
}

/// `core_course_get_contents`: the course page, as sections containing modules.
#[derive(Debug)]
pub struct GetCourseContents {
    pub courseid: i64,
    /// Sent as `options[i][name]` / `options[i][value]`. Known names:
    /// `excludemodules`, `excludecontents`, `includestealthmodules`,
    /// `sectionid`, `sectionnumber`, `cmid`, `modname`, `modid`.
    pub options: Vec<(String, String)>,
}

impl WsFunction for GetCourseContents {
    const NAME: &'static str = "core_course_get_contents";
    /// A bare JSON array.
    type Response = Vec<Section>;

    fn params(&self) -> Vec<(String, String)> {
        let mut p = vec![("courseid".into(), self.courseid.to_string())];
        for (i, (name, value)) in self.options.iter().enumerate() {
            p.push((format!("options[{i}][name]"), name.clone()));
            p.push((format!("options[{i}][value]"), value.clone()));
        }
        p
    }
}

#[derive(Debug, Deserialize)]
pub struct Section {
    pub id: i64,
    pub name: String,
    /// Section number (0 = general section).
    pub section: Option<i32>,
    pub summary: String,
    pub summaryformat: TextFormat,
    pub visible: Option<i32>,
    pub uservisible: Option<bool>,
    #[serde(default)]
    pub modules: Vec<Module>,
}

/// A course module ("activity" or "resource") on the course page.
#[derive(Debug, Deserialize)]
pub struct Module {
    /// Course module id (`cmid`). The per-module endpoints call this
    /// `coursemodule`/`cmid`; their own `id` is the instance id.
    pub id: i64,
    pub name: String,
    /// Module type: `assign`, `resource`, `folder`, `page`, `book`, `url`,
    /// `label`, `forum`, `quiz`, ...
    pub modname: String,
    /// Instance id, matches `id` in the per-module responses.
    pub instance: Option<i64>,
    pub url: Option<String>,
    /// HTML description shown on the course page (labels, resources, ...).
    pub description: Option<String>,
    pub visible: Option<i32>,
    pub uservisible: Option<bool>,
    /// Activity dates, e.g. "Opened:" / "Due:".
    #[serde(default)]
    pub dates: Vec<ModuleDate>,
    /// Files for `resource`/`folder`/`book`, the link for `url`.
    #[serde(default)]
    pub contents: Vec<ModuleContent>,
}

#[derive(Debug, Deserialize)]
pub struct ModuleDate {
    pub label: String,
    pub timestamp: i64,
}

/// An entry in [`Module::contents`]. Similar to [`File`] but with a `type`.
#[derive(Debug, Deserialize)]
pub struct ModuleContent {
    /// `file`, `url` or `content`.
    #[serde(rename = "type")]
    pub kind: String,
    pub filename: String,
    pub filepath: Option<String>,
    pub filesize: u64,
    pub fileurl: Option<String>,
    /// Inline content. For books, the first entry is a JSON table of contents.
    pub content: Option<String>,
    pub timecreated: Option<i64>,
    pub timemodified: i64,
    pub mimetype: Option<String>,
    pub isexternalfile: Option<bool>,
}
