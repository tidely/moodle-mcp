use serde::Deserialize;

use super::{File, TextFormat, Warning, WsFunction, indexed};

/// `mod_assign_get_assignments` (Moodle 2.4+): all assignments in the given courses.
#[derive(Debug)]
pub struct GetAssignments {
    /// Sent as `courseids[i]`. Empty = all enrolled courses.
    pub courseids: Vec<i64>,
}

impl WsFunction for GetAssignments {
    const NAME: &'static str = "mod_assign_get_assignments";
    type Response = Assignments;

    fn params(&self) -> Vec<(String, String)> {
        indexed("courseids", &self.courseids)
    }
}

#[derive(Debug, Deserialize)]
pub struct Assignments {
    pub courses: Vec<AssignmentCourse>,
    #[serde(default)]
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Deserialize)]
pub struct AssignmentCourse {
    pub id: i64,
    pub fullname: String,
    pub shortname: String,
    pub assignments: Vec<Assignment>,
}

#[derive(Debug, Deserialize)]
pub struct Assignment {
    /// Instance id. Pass to [`GetSubmissionStatus::assignid`].
    pub id: i64,
    /// Course module id, matches [`super::course::Module::id`].
    pub cmid: i64,
    pub course: i64,
    pub name: String,
    /// The assignment description (the "project description"). HTML.
    pub intro: Option<String>,
    pub introformat: Option<TextFormat>,
    /// Files embedded in `intro`.
    #[serde(default)]
    pub introfiles: Vec<File>,
    /// "Additional files" attached to the assignment.
    #[serde(default)]
    pub introattachments: Vec<File>,
    /// Unix timestamps; 0 = not set.
    pub allowsubmissionsfromdate: i64,
    pub duedate: i64,
    pub cutoffdate: i64,
    pub gradingduedate: Option<i64>,
    /// Max grade, or negative scale id.
    pub grade: i64,
    /// 1 = no online submission required.
    pub nosubmissions: i32,
    pub teamsubmission: i32,
    pub maxattempts: i32,
    pub timemodified: i64,
}

/// `mod_assign_get_submission_status` (Moodle 3.1+): own submission, grade and feedback.
#[derive(Debug)]
pub struct GetSubmissionStatus {
    /// [`Assignment::id`], not the `cmid`.
    pub assignid: i64,
    /// Defaults to the token's user.
    pub userid: Option<i64>,
}

impl WsFunction for GetSubmissionStatus {
    const NAME: &'static str = "mod_assign_get_submission_status";
    type Response = SubmissionStatus;

    fn params(&self) -> Vec<(String, String)> {
        let mut p = vec![("assignid".into(), self.assignid.to_string())];
        if let Some(userid) = self.userid {
            p.push(("userid".into(), userid.to_string()));
        }
        p
    }
}

#[derive(Debug, Deserialize)]
pub struct SubmissionStatus {
    pub lastattempt: Option<LastAttempt>,
    pub feedback: Option<Feedback>,
    #[serde(default)]
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Deserialize)]
pub struct LastAttempt {
    pub submission: Option<Submission>,
    /// Set instead of/in addition to `submission` for group assignments.
    pub teamsubmission: Option<Submission>,
    pub submissionsenabled: bool,
    pub locked: bool,
    pub graded: bool,
    pub cansubmit: bool,
    /// Personal extension, 0 = none.
    pub extensionduedate: Option<i64>,
    /// `notgraded`, `graded`, or a marking workflow state.
    pub gradingstatus: String,
}

#[derive(Debug, Deserialize)]
pub struct Submission {
    pub id: i64,
    pub userid: i64,
    pub groupid: i64,
    pub attemptnumber: i32,
    /// `new`, `draft`, `submitted`, `reopened`.
    pub status: String,
    pub timecreated: i64,
    pub timemodified: i64,
    #[serde(default)]
    pub plugins: Vec<Plugin>,
}

#[derive(Debug, Deserialize)]
pub struct Feedback {
    pub grade: Option<Grade>,
    /// Rendered grade, e.g. `"85.00 / 100.00"`. May contain HTML.
    pub gradefordisplay: String,
    pub gradeddate: i64,
    /// Feedback comments/files from the teacher.
    #[serde(default)]
    pub plugins: Vec<Plugin>,
}

#[derive(Debug, Deserialize)]
pub struct Grade {
    pub id: i64,
    pub userid: i64,
    pub grader: i64,
    /// Decimal as a string, e.g. `"85.00000"`.
    pub grade: String,
    pub timemodified: i64,
}

/// Submission or feedback plugin data (`file`, `onlinetext`, `comments`, ...).
#[derive(Debug, Deserialize)]
pub struct Plugin {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub fileareas: Vec<FileArea>,
    #[serde(default)]
    pub editorfields: Vec<EditorField>,
}

#[derive(Debug, Deserialize)]
pub struct FileArea {
    pub area: String,
    #[serde(default)]
    pub files: Vec<File>,
}

/// Rich-text field, e.g. online text submission or feedback comments.
#[derive(Debug, Deserialize)]
pub struct EditorField {
    pub name: String,
    pub description: String,
    pub text: String,
    pub format: TextFormat,
}
