//! Obtaining a `wstoken`. These are not `webservice/rest/server.php` calls.

use std::fmt;

use base64::Engine;
use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use serde::Deserialize;

/// SSO login, opened in the user's browser:
///
/// ```text
/// GET {base}admin/tool/mobile/launch.php?service=moodle_mobile_app&passport={passport}&urlscheme=moodledl
/// ```
///
/// After SSO, Moodle redirects to `moodledl://token={base64}`, which decodes
/// to an [`SsoToken`].
#[derive(Debug)]
pub struct LaunchRequest {
    /// Always `moodle_mobile_app`.
    pub service: String,
    /// Random nonce; Moodle-DL uses 32 hex chars.
    pub passport: String,
    /// Scheme of the redirect URL; `moodledl` or `moodlemobile`.
    pub urlscheme: String,
}

impl LaunchRequest {
    pub fn url(&self, base: &str) -> String {
        format!(
            "{base}admin/tool/mobile/launch.php?service={}&passport={}&urlscheme={}",
            self.service, self.passport, self.urlscheme
        )
    }
}

/// Decoded SSO redirect payload: `base64("{signature}:::{token}[:::{private_token}]")`.
/// See `Moodle-DL/moodle_dl/moodle/moodle_service.py::extract_token`.
pub struct SsoToken {
    /// `md5(site_url + passport)`; lets the client verify the redirect.
    pub signature: String,
    /// The `wstoken` used for every web service call.
    pub token: String,
    /// Only needed for `tool_mobile_get_autologin_key` (browser cookies).
    pub private_token: Option<String>,
}

impl SsoToken {
    /// Parses the redirect URL (`moodledl://token=...`) or just its base64 part.
    pub fn parse(input: &str) -> Option<Self> {
        let encoded = input.split_once("token=").map_or(input, |(_, rest)| rest);
        // Stop at anything that isn't base64, e.g. a trailing quote or newline.
        let encoded: String = encoded
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='))
            .collect();
        let engine = GeneralPurpose::new(
            &alphabet::STANDARD,
            GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
        );
        let decoded = String::from_utf8(engine.decode(encoded).ok()?).ok()?;

        let mut parts = decoded.split(":::");
        let signature = parts.next()?.to_owned();
        let clean = |s: &str| -> String { s.chars().filter(char::is_ascii_alphanumeric).collect() };
        let token = clean(parts.next()?);
        if token.is_empty() {
            return None;
        }
        let private_token = parts.next().map(clean).filter(|t| !t.is_empty());
        Some(Self {
            signature,
            token,
            private_token,
        })
    }
}

impl fmt::Debug for SsoToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SsoToken")
            .field("signature", &self.signature)
            .field("token", &"<redacted>")
            .field(
                "private_token",
                &self.private_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sso_redirect() {
        // base64("sig:::abc123:::priv456")
        let t = SsoToken::parse("moodledl://token=c2lnOjo6YWJjMTIzOjo6cHJpdjQ1Ng==").unwrap();
        assert_eq!(t.signature, "sig");
        assert_eq!(t.token, "abc123");
        assert_eq!(t.private_token.as_deref(), Some("priv456"));

        // base64("sig:::abc123"), unpadded, no URL prefix.
        let t = SsoToken::parse("c2lnOjo6YWJjMTIz").unwrap();
        assert_eq!(t.token, "abc123");
        assert!(t.private_token.is_none());

        assert!(SsoToken::parse("moodledl://token=bm90aGluZw").is_none());
    }
}

/// Username/password login. Does not work with SSO-only accounts.
///
/// `POST {base}login/token.php` with form fields below.
#[derive(Debug)]
pub struct TokenRequest {
    pub username: String,
    pub password: String,
    /// Always `moodle_mobile_app`.
    pub service: String,
}

impl TokenRequest {
    pub fn params(&self) -> Vec<(String, String)> {
        vec![
            ("username".into(), self.username.clone()),
            ("password".into(), self.password.clone()),
            ("service".into(), self.service.clone()),
        ]
    }
}

/// Response to [`TokenRequest`].
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum TokenResponse {
    Ok {
        token: String,
        privatetoken: Option<String>,
    },
    /// `login/token.php` uses `error` rather than `exception`.
    Err {
        error: String,
        errorcode: Option<String>,
        debuginfo: Option<String>,
    },
}
