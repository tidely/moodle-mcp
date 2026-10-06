use serde::Deserialize;

use super::WsFunction;

/// `core_webservice_get_site_info`. No parameters.
/// Used first to learn the user id and Moodle version.
#[derive(Debug)]
pub struct GetSiteInfo;

impl WsFunction for GetSiteInfo {
    const NAME: &'static str = "core_webservice_get_site_info";
    type Response = SiteInfo;

    fn params(&self) -> Vec<(String, String)> {
        Vec::new()
    }
}

#[derive(Debug, Deserialize)]
pub struct SiteInfo {
    pub sitename: String,
    pub siteurl: String,
    pub username: String,
    pub fullname: String,
    pub userid: i64,
    /// e.g. `"4.1.2 (Build: 20230313)"`.
    pub release: Option<String>,
    /// e.g. `"2022112802.00"`. The integer part gates features
    /// (Moodle-DL compares against values like `2017051500` = 3.3).
    pub version: Option<String>,
    /// Web service functions this token may call.
    #[serde(default)]
    pub functions: Vec<SiteFunction>,
}

#[derive(Debug, Deserialize)]
pub struct SiteFunction {
    pub name: String,
    pub version: String,
}
