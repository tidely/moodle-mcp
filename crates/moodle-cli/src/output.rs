//! Machine-first output: JSON on stdout (pretty on a terminal, compact
//! otherwise), JSON errors with a `code` and a `hint` on stderr.

use std::fmt;
use std::io::IsTerminal;

use serde::Serialize;

pub fn emit<T: Serialize>(value: &T) {
    let json = if std::io::stdout().is_terminal() {
        serde_json::to_string_pretty(value)
    } else {
        serde_json::to_string(value)
    };
    println!("{}", json.expect("output serializes"));
}

/// Progress for humans; silent when stderr isn't a terminal (agents, pipes).
pub fn progress(msg: &str) {
    if std::io::stderr().is_terminal() {
        eprintln!("{msg}");
    }
}

/// An error that tells the caller what to do next.
#[derive(Debug)]
pub struct Hinted {
    pub code: &'static str,
    pub message: String,
    pub hint: String,
}

impl fmt::Display for Hinted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Hinted {}

pub fn hinted(
    code: &'static str,
    message: impl Into<String>,
    hint: impl Into<String>,
) -> anyhow::Error {
    Hinted {
        code,
        message: message.into(),
        hint: hint.into(),
    }
    .into()
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    code: &'a str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<&'a str>,
}

/// Writes `{"error": {...}}` to stderr.
pub fn emit_error(err: &anyhow::Error) {
    let message = format!("{err:#}");
    let (code, hint) = classify(err);
    let body = serde_json::json!({ "error": ErrorBody { code, message, hint } });
    eprintln!("{body}");
}

fn classify(err: &anyhow::Error) -> (&str, Option<&str>) {
    if let Some(h) = err.downcast_ref::<Hinted>() {
        return (h.code, Some(&h.hint));
    }
    let api = err
        .downcast_ref::<moodle_api::Error>()
        .or_else(|| match err.downcast_ref() {
            Some(moodle_sync::Error::Api(e)) => Some(e),
            _ => None,
        });
    match api {
        Some(moodle_api::Error::Moodle(e)) if e.errorcode == "invalidtoken" => (
            "invalidtoken",
            Some("The token expired or was revoked. Run `moodle-cli login` again."),
        ),
        Some(moodle_api::Error::Moodle(e)) if e.errorcode == "accessexception" => (
            "accessexception",
            Some("This site doesn't allow that web service function for your account."),
        ),
        Some(moodle_api::Error::Moodle(e)) => (e.errorcode.as_str(), None),
        Some(moodle_api::Error::Http(_)) => (
            "network",
            Some(
                "Could not reach Moodle. Check the network and the site URL (`moodle-cli whoami`).",
            ),
        ),
        Some(_) => ("api", None),
        None => match err.downcast_ref::<moodle_sync::Error>() {
            Some(moodle_sync::Error::NotFound(_)) => ("not_found", None),
            Some(moodle_sync::Error::Io(..)) => ("io", None),
            _ => ("error", None),
        },
    }
}
