//! Folder, page and book modules. All three take a list of course ids and
//! return every instance in those courses.

use serde::Deserialize;

use super::{File, TextFormat, Warning, WsFunction, indexed};

/// `mod_folder_get_folders_by_courses` (Moodle 3.3+).
/// The folder's files are not here; they are in
/// [`super::course::Module::contents`] from `core_course_get_contents`.
#[derive(Debug)]
pub struct GetFolders {
    pub courseids: Vec<i64>,
}

impl WsFunction for GetFolders {
    const NAME: &'static str = "mod_folder_get_folders_by_courses";
    type Response = Folders;

    fn params(&self) -> Vec<(String, String)> {
        indexed("courseids", &self.courseids)
    }
}

#[derive(Debug, Deserialize)]
pub struct Folders {
    pub folders: Vec<Folder>,
    #[serde(default)]
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Deserialize)]
pub struct Folder {
    pub id: i64,
    pub coursemodule: i64,
    pub course: i64,
    pub name: String,
    pub intro: String,
    pub introformat: TextFormat,
    #[serde(default)]
    pub introfiles: Vec<File>,
    pub timemodified: i64,
}

/// `mod_page_get_pages_by_courses` (Moodle 3.3+).
#[derive(Debug)]
pub struct GetPages {
    pub courseids: Vec<i64>,
}

impl WsFunction for GetPages {
    const NAME: &'static str = "mod_page_get_pages_by_courses";
    type Response = Pages;

    fn params(&self) -> Vec<(String, String)> {
        indexed("courseids", &self.courseids)
    }
}

#[derive(Debug, Deserialize)]
pub struct Pages {
    pub pages: Vec<Page>,
    #[serde(default)]
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Deserialize)]
pub struct Page {
    pub id: i64,
    pub coursemodule: i64,
    pub course: i64,
    pub name: String,
    pub intro: String,
    pub introformat: TextFormat,
    #[serde(default)]
    pub introfiles: Vec<File>,
    /// The page body. HTML.
    pub content: String,
    pub contentformat: TextFormat,
    /// Files embedded in `content`.
    #[serde(default)]
    pub contentfiles: Vec<File>,
    pub timemodified: i64,
}

/// `mod_book_get_books_by_courses` (Moodle 3.0+).
/// Chapters are in [`super::course::Module::contents`]: entry 0 is a JSON
/// table of contents, the rest are chapter HTML files and their attachments.
#[derive(Debug)]
pub struct GetBooks {
    pub courseids: Vec<i64>,
}

impl WsFunction for GetBooks {
    const NAME: &'static str = "mod_book_get_books_by_courses";
    type Response = Books;

    fn params(&self) -> Vec<(String, String)> {
        indexed("courseids", &self.courseids)
    }
}

#[derive(Debug, Deserialize)]
pub struct Books {
    pub books: Vec<Book>,
    #[serde(default)]
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Deserialize)]
pub struct Book {
    pub id: i64,
    pub coursemodule: i64,
    pub course: i64,
    pub name: String,
    pub intro: String,
    pub introformat: TextFormat,
    #[serde(default)]
    pub introfiles: Vec<File>,
    pub timemodified: Option<i64>,
}
