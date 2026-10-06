//! Minimal mock Moodle over local HTTP, shared by the sync and CLI tests.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub const TOKEN: &str = "secrettoken123";

fn file_json(base: &str, name: &str, size: usize) -> String {
    format!(
        r#"{{"type":"file","filename":"{name}","filepath":"/","filesize":{size},"timemodified":1700000000,"fileurl":"{base}webservice/pluginfile.php/1/{name}"}}"#
    )
}

fn ws_response(function: &str, base: &str, slides_removed: bool) -> String {
    let slides = if slides_removed {
        String::new()
    } else {
        format!(
            r#",{{"id":1400,"name":"Slides","modname":"resource","instance":8,"contents":[{},{}]}}"#,
            file_json(base, "small.pdf", 5),
            file_json(base, "big.mp4", 100)
        )
    };
    match function {
        "core_webservice_get_site_info" => r#"{"sitename":"Mock","siteurl":"x","username":"stud",
            "fullname":"Stu Dent","userid":2,"release":"4.1"}"#
            .to_owned(),
        "core_enrol_get_users_courses" => {
            r#"[{"id":42,"shortname":"DB/2026","fullname":"Databases",
            "startdate":1690000000,"enddate":0},{"id":7,"shortname":"OLD","fullname":"Old course",
            "startdate":1500000000,"enddate":1510000000}]"#
                .to_owned()
        }
        "core_calendar_get_action_events_by_timesort" => format!(
            r#"{{"events":[{{"id":5,"name":"Project 2 is due","activityname":"Project 2",
            "modulename":"assign","instance":7,"eventtype":"due","timestart":1700000000,
            "timesort":1700000000,"overdue":true,"course":{{"id":42,"shortname":"DB/2026"}},
            "action":{{"name":"Add submission","itemcount":1,"actionable":true}},
            "url":"{base}mod/assign/view.php?id=1337"}}],"firstid":5,"lastid":5}}"#
        ),
        "core_course_get_courses_by_field" => format!(
            r#"{{"courses":[{{"id":42,"fullname":"Databases","shortname":"DB/2026",
            "summary":"<p>Welcome to <b>DB</b>.</p><img src=\"{base}pluginfile.php/7/course/summary/logo.png\">",
            "summaryformat":1}}]}}"#
        ),
        "core_course_get_contents" => format!(
            r#"[{{"id":1,"name":"General","section":0,"summaryformat":1,
                "summary":"<img src=\"{base}webservice/pluginfile.php/5/course/section/1/image.png\">",
                "modules":[
                {{"id":1300,"name":"Note","modname":"label",
                  "description":"<p>Read the brief first.</p><img src=\"{base}webservice/pluginfile.php/6/mod_label/intro/image.png\">"}},
                {{"id":1337,"name":"Project 2","modname":"assign","instance":7,
                  "url":"{base}mod/assign/view.php?id=1337","dates":[{{"label":"Due:","timestamp":1700000000}}]}}{slides}]}}]"#,
        ),
        "mod_assign_get_assignments" => format!(
            r#"{{"courses":[{{"id":42,"fullname":"Databases","shortname":"DB/2026","assignments":[
                {{"id":7,"cmid":1337,"course":42,"name":"Project 2",
                  "intro":"<p>Build it. <a href=\"{base}webservice/pluginfile.php/1/brief.pdf\">Brief</a>, <a href=\"{base}webservice/pluginfile.php/9/secret.pdf\">other</a></p>",
                  "introattachments":[{}],
                  "allowsubmissionsfromdate":0,"duedate":1700000000,"cutoffdate":0,"grade":100,
                  "nosubmissions":0,"teamsubmission":0,"maxattempts":-1,"timemodified":0}}]}}]}}"#,
            file_json(base, "brief.pdf", 5)
        ),
        "mod_page_get_pages_by_courses" => r#"{"pages":[]}"#.to_owned(),
        "mod_assign_get_submission_status" => r#"{"lastattempt":{"submission":{"id":1,"userid":2,
            "groupid":0,"attemptnumber":0,"status":"submitted","timecreated":0,
            "timemodified":1700000000,"plugins":[]},"submissionsenabled":true,"locked":false,
            "graded":false,"cansubmit":true,"gradingstatus":"notgraded"}}"#
            .to_owned(),
        other => panic!("unexpected wsfunction {other}"),
    }
}

async fn handle(mut stream: TcpStream, base: String, slides_removed: Arc<AtomicBool>) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut chunk).await.unwrap();
        assert!(n > 0, "connection closed mid-request");
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let length = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while buf.len() < header_end + length {
        let n = stream.read(&mut chunk).await.unwrap();
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = String::from_utf8_lossy(&buf[header_end..]).to_string();
    let target = head.split_whitespace().nth(1).unwrap();

    let (status, payload) = if let Some(rest) = target.split("wsfunction=").nth(1) {
        assert!(body.contains(&format!("wstoken={TOKEN}")));
        (
            "200 OK",
            ws_response(rest, &base, slides_removed.load(Ordering::SeqCst)),
        )
    } else if let Some(path) = target.strip_prefix("/webservice/pluginfile.php/") {
        match path.split_once("?token=") {
            Some((path, TOKEN)) => {
                let size = if path.ends_with("big.mp4") { 100 } else { 5 };
                ("200 OK", "x".repeat(size))
            }
            _ => ("403 Forbidden", String::new()),
        }
    } else {
        ("404 Not Found", String::new())
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    stream.write_all(response.as_bytes()).await.unwrap();
}

pub struct Mock {
    pub base: String,
    /// When set, the "Slides" activity disappears from the course.
    pub slides_removed: Arc<AtomicBool>,
}

pub async fn mock_moodle() -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let slides_removed = Arc::new(AtomicBool::new(false));
    let (b, flag) = (base.clone(), slides_removed.clone());
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(handle(stream, b.clone(), flag.clone()));
        }
    });
    Mock {
        base,
        slides_removed,
    }
}
