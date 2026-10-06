use std::fmt;

use crate::{MoodleException, USER_AGENT, WsFunction, form_body, rest_url};

/// Async web service client. Holds the `wstoken`; never prints it.
/// Cheap to clone; clones share the connection pool.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    host: String,
    token: String,
}

impl Client {
    /// `base` is the site root, e.g. `https://moodle.example.edu/`.
    /// A missing trailing `/` is added.
    pub fn new(base: &str, token: impl Into<String>) -> Result<Self, Error> {
        let mut base = base.to_owned();
        if !base.ends_with('/') {
            base.push('/');
        }
        let url = reqwest::Url::parse(&base).map_err(|_| Error::InvalidBaseUrl)?;
        let mut host = url.host_str().ok_or(Error::InvalidBaseUrl)?.to_owned();
        if let Some(port) = url.port() {
            host = format!("{host}:{port}");
        }
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .map_err(Error::http)?;
        Ok(Self {
            http,
            base,
            host,
            token: token.into(),
        })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// Host (and port, if any) of the site, e.g. `moodle.example.edu`.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Calls a web service function and decodes its paired response.
    pub async fn call<F: WsFunction>(&self, request: &F) -> Result<F::Response, Error> {
        let body = self
            .http
            .post(rest_url::<F>(&self.base))
            .form(&form_body(&self.token, request))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(Error::http)?
            .bytes()
            .await
            .map_err(Error::http)?;

        // Moodle signals failure with HTTP 200 and an exception body.
        if let Ok(e) = serde_json::from_slice::<MoodleException>(&body) {
            return Err(Error::Moodle(e));
        }
        serde_json::from_slice(&body).map_err(|source| Error::Decode {
            function: F::NAME,
            source,
        })
    }

    /// Starts downloading a `fileurl` from a web service response.
    ///
    /// The token is only ever sent to this site: URLs outside `base` are
    /// rejected with [`Error::ForeignUrl`].
    pub async fn download(&self, fileurl: &str) -> Result<FileDownload, Error> {
        if !fileurl.starts_with(&self.base) {
            return Err(Error::ForeignUrl);
        }
        let sep = if fileurl.contains('?') { '&' } else { '?' };
        let response = self
            .http
            .get(format!("{fileurl}{sep}token={}", self.token))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(Error::http)?;
        Ok(FileDownload(response))
    }
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base", &self.base)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// An in-progress file download from [`Client::download`].
#[derive(Debug)]
pub struct FileDownload(reqwest::Response);

impl FileDownload {
    /// The next chunk of the body, or `None` when done.
    pub async fn chunk(&mut self) -> Result<Option<impl AsRef<[u8]>>, Error> {
        self.0.chunk().await.map_err(Error::http)
    }
}

#[derive(Debug)]
pub enum Error {
    /// The site URL could not be parsed.
    InvalidBaseUrl,
    /// A file URL points outside the site; the token is not sent there.
    ForeignUrl,
    /// Connection failure or non-2xx status.
    Http(reqwest::Error),
    /// Moodle rejected the call, e.g. `invalidtoken`.
    Moodle(MoodleException),
    /// The response didn't match the expected type.
    Decode {
        function: &'static str,
        source: serde_json::Error,
    },
}

impl Error {
    fn http(e: reqwest::Error) -> Self {
        // File URLs carry `?token=`; never let them reach error messages.
        Self::Http(e.without_url())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBaseUrl => write!(f, "invalid Moodle site URL"),
            Self::ForeignUrl => write!(f, "file URL is not on the configured Moodle site"),
            Self::Http(e) => write!(f, "HTTP error: {e}"),
            Self::Moodle(e) => write!(f, "Moodle error {}: {}", e.errorcode, e.message),
            Self::Decode { function, source } => {
                write!(f, "unexpected response from {function}: {source}")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Http(e) => Some(e),
            Self::Decode { source, .. } => Some(source),
            _ => None,
        }
    }
}
