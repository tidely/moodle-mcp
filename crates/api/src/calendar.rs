use serde::Deserialize;

use super::WsFunction;

/// `core_calendar_get_action_events_by_timesort` (Moodle 3.3+): the user's
/// to-do list across courses, as shown by the Timeline block. Only events
/// that need action (submit, attempt, ...); completed ones drop out.
#[derive(Debug, Default)]
pub struct GetActionEventsByTimesort {
    /// Unix time; events sorted before this are excluded.
    pub timesortfrom: Option<i64>,
    /// Unix time; events sorted after this are excluded.
    pub timesortto: Option<i64>,
    /// For paging: continue after this event id (`lastid` of the previous page).
    pub aftereventid: Option<i64>,
    /// Page size (Moodle caps this, typically at 50).
    pub limitnum: Option<u32>,
    /// Skip courses the user is suspended from.
    pub limittononsuspendedevents: Option<bool>,
}

impl WsFunction for GetActionEventsByTimesort {
    const NAME: &'static str = "core_calendar_get_action_events_by_timesort";
    type Response = ActionEvents;

    fn params(&self) -> Vec<(String, String)> {
        let mut p = Vec::new();
        let mut push = |k: &str, v: Option<String>| {
            if let Some(v) = v {
                p.push((k.to_owned(), v));
            }
        };
        push("timesortfrom", self.timesortfrom.map(|v| v.to_string()));
        push("timesortto", self.timesortto.map(|v| v.to_string()));
        push("aftereventid", self.aftereventid.map(|v| v.to_string()));
        push("limitnum", self.limitnum.map(|v| v.to_string()));
        push(
            "limittononsuspendedevents",
            self.limittononsuspendedevents
                .map(|v| u8::from(v).to_string()),
        );
        p
    }
}

#[derive(Debug, Deserialize)]
pub struct ActionEvents {
    #[serde(default)]
    pub events: Vec<ActionEvent>,
    pub firstid: Option<i64>,
    pub lastid: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ActionEvent {
    pub id: i64,
    /// e.g. `Project 2 is due`.
    pub name: String,
    /// The activity's name, e.g. `Project 2`.
    pub activityname: Option<String>,
    /// Module type, e.g. `assign`, `quiz`.
    pub modulename: Option<String>,
    /// Activity instance id (not the cmid).
    pub instance: Option<i64>,
    /// e.g. `due`, `close`, `expectcompletionon`.
    pub eventtype: Option<String>,
    #[serde(default)]
    pub timestart: i64,
    /// The time the event is sorted by (usually the deadline).
    pub timesort: Option<i64>,
    pub overdue: Option<bool>,
    pub course: Option<EventCourse>,
    pub action: Option<EventAction>,
    /// Link to the activity, e.g. `.../mod/assign/view.php?id={cmid}`.
    pub url: Option<String>,
}

impl ActionEvent {
    /// The course module id, parsed from [`Self::url`].
    pub fn cmid(&self) -> Option<i64> {
        let url = self.url.as_deref()?;
        let (_, query) = url.split_once('?')?;
        query
            .split('&')
            .find_map(|kv| kv.strip_prefix("id="))?
            .parse()
            .ok()
    }
}

#[derive(Debug, Deserialize)]
pub struct EventCourse {
    pub id: i64,
    pub fullname: Option<String>,
    pub shortname: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EventAction {
    /// e.g. `Add submission`, `Attempt quiz now`.
    pub name: String,
    pub itemcount: Option<i64>,
    /// False if the action exists but can't be taken yet (e.g. not open).
    #[serde(default)]
    pub actionable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_action_events() {
        let json = r#"{"events":[{"id":5,"name":"Project 2 is due","activityname":"Project 2",
            "modulename":"assign","instance":7,"eventtype":"due","timestart":1700000000,
            "timesort":1700000000,"overdue":false,"course":{"id":42,"shortname":"DB"},
            "action":{"name":"Add submission","itemcount":1,"actionable":true},
            "url":"https://m.example/mod/assign/view.php?id=1337"}],"firstid":5,"lastid":5}"#;
        let r: ActionEvents = serde_json::from_str(json).unwrap();
        assert_eq!(r.events[0].cmid(), Some(1337));
        assert!(r.events[0].action.as_ref().unwrap().actionable);
    }
}
