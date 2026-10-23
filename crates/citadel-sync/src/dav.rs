use futures::future::BoxFuture;
use reqwest::header::{IF_MATCH, IF_NONE_MATCH};
use reqwest::{Client, Method, StatusCode};
use secrecy::{ExposeSecret, SecretString};
use url::Url;

use crate::http::{self, failure};
use crate::{Fetched, Remote, Result, SyncError, Version};

pub(crate) struct WebDav {
    http: Client,
    url: Url,
    user: String,
    password: SecretString,
}

impl WebDav {
    pub fn new(url: &str, user: &str, password: &SecretString) -> Result<Self> {
        let url = Url::parse(url).map_err(|e| SyncError::Config(format!("url: {e}")))?;
        http::check_scheme(&url)?;
        if url.path().ends_with('/') {
            return Err(SyncError::Config(
                "WebDAV url must name a file, not a collection".into(),
            ));
        }
        Ok(Self {
            http: http::client()?,
            url,
            user: user.to_owned(),
            password: password.clone(),
        })
    }

    fn request(&self, method: Method, url: Url) -> reqwest::RequestBuilder {
        let req = self.http.request(method, url);
        if self.user.is_empty() {
            req
        } else {
            req.basic_auth(&self.user, Some(self.password.expose_secret()))
        }
    }

    fn parent(&self) -> Option<Url> {
        let mut parent = self.url.clone();
        parent.path_segments_mut().ok()?.pop().push("");
        (parent.path() != "/").then_some(parent)
    }

    async fn get(&self) -> Result<Fetched> {
        let resp = self.request(Method::GET, self.url.clone()).send().await?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(failure(resp).await);
        }
        let tag = http::etag(resp.headers()).unwrap_or_default();
        Ok(Some((resp.bytes().await?.to_vec(), tag)))
    }

    async fn attempt(&self, blob: &[u8], expected: Option<&str>) -> Result<reqwest::Response> {
        let req = self
            .request(Method::PUT, self.url.clone())
            .body(blob.to_vec());
        let req = match expected {
            Some("") => req,
            Some(tag) => req.header(IF_MATCH, tag),
            None => req.header(IF_NONE_MATCH, "*"),
        };
        Ok(req.send().await?)
    }

    async fn put(&self, blob: &[u8], expected: Option<&str>) -> Result<Version> {
        let mut resp = self.attempt(blob, expected).await?;
        let missing_parent = matches!(resp.status(), StatusCode::CONFLICT | StatusCode::NOT_FOUND);
        if missing_parent && expected.is_none() {
            if let Some(parent) = self.parent() {
                let mkcol = Method::from_bytes(b"MKCOL")
                    .map_err(|e| SyncError::Transport(e.to_string()))?;
                let made = self.request(mkcol, parent).send().await?;
                if !made.status().is_success() && made.status() != StatusCode::METHOD_NOT_ALLOWED {
                    return Err(failure(made).await);
                }
                resp = self.attempt(blob, expected).await?;
            }
        } else if missing_parent {
            return Err(SyncError::Conflict);
        }
        if !resp.status().is_success() {
            return Err(failure(resp).await);
        }
        if let Some(tag) = http::etag(resp.headers()) {
            return Ok(tag);
        }
        Ok(self.get().await?.map(|(_, tag)| tag).unwrap_or_default())
    }
}

impl Remote for WebDav {
    fn fetch(&self) -> BoxFuture<'_, Result<Fetched>> {
        Box::pin(self.get())
    }

    fn store<'a>(
        &'a self,
        blob: &'a [u8],
        expected: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Version>> {
        Box::pin(self.put(blob, expected))
    }

    fn describe(&self) -> String {
        let mut shown = self.url.clone();
        let _ = shown.set_password(None);
        let _ = shown.set_username("");
        format!("webdav {shown}")
    }
}
