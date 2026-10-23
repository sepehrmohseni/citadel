use std::time::Duration;

use futures::future::BoxFuture;
use reqwest::header::{IF_MATCH, IF_NONE_MATCH};
use reqwest::{Client, StatusCode};
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};
use secrecy::{ExposeSecret, SecretString};
use url::Url;

use crate::http::{self, failure};
use crate::{Fetched, Remote, Result, SyncError, Version};

const PRESIGN_TTL: Duration = Duration::from_secs(300);

pub(crate) struct S3 {
    http: Client,
    bucket: Bucket,
    creds: Credentials,
    object: String,
    endpoint: String,
}

impl S3 {
    pub fn new(
        endpoint: &str,
        region: &str,
        bucket: &str,
        object: &str,
        access_key: &str,
        secret_key: &SecretString,
        path_style: bool,
    ) -> Result<Self> {
        let url = Url::parse(endpoint).map_err(|e| SyncError::Config(format!("endpoint: {e}")))?;
        http::check_scheme(&url)?;
        let object = object.trim_start_matches('/');
        if object.is_empty() || bucket.is_empty() {
            return Err(SyncError::Config(
                "bucket and object key are required".into(),
            ));
        }
        let style = if path_style {
            UrlStyle::Path
        } else {
            UrlStyle::VirtualHost
        };
        let region = if region.is_empty() {
            "us-east-1"
        } else {
            region
        };
        let bucket = Bucket::new(url.clone(), style, bucket.to_owned(), region.to_owned())
            .map_err(|e| SyncError::Config(format!("bucket: {e}")))?;
        bucket
            .object_url(object)
            .map_err(|e| SyncError::Config(format!("object key: {e}")))?;
        Ok(Self {
            http: http::client()?,
            bucket,
            creds: Credentials::new(access_key, secret_key.expose_secret()),
            object: object.to_owned(),
            endpoint: url.host_str().unwrap_or_default().to_owned(),
        })
    }

    async fn get(&self) -> Result<Fetched> {
        let url = self
            .bucket
            .get_object(Some(&self.creds), &self.object)
            .sign(PRESIGN_TTL);
        let resp = self.http.get(url).send().await?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(failure(resp).await);
        }
        let tag = http::etag(resp.headers())
            .ok_or_else(|| SyncError::Transport("S3 response carried no ETag".into()))?;
        Ok(Some((resp.bytes().await?.to_vec(), tag)))
    }

    async fn put(&self, blob: &[u8], expected: Option<&str>) -> Result<Version> {
        let url = self
            .bucket
            .put_object(Some(&self.creds), &self.object)
            .sign(PRESIGN_TTL);
        let req = self.http.put(url).body(blob.to_vec());
        let req = match expected {
            Some(tag) => req.header(IF_MATCH, tag),
            None => req.header(IF_NONE_MATCH, "*"),
        };
        let resp = req.send().await?;
        if resp.status() == StatusCode::NOT_FOUND && expected.is_some() {
            return Err(SyncError::Conflict);
        }
        if !resp.status().is_success() {
            return Err(failure(resp).await);
        }
        match http::etag(resp.headers()) {
            Some(tag) => Ok(tag),
            None => self
                .get()
                .await?
                .map(|(_, tag)| tag)
                .ok_or_else(|| SyncError::Transport("object vanished after upload".into())),
        }
    }
}

impl Remote for S3 {
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
        format!(
            "s3://{}/{} via {}",
            self.bucket.name(),
            self.object,
            self.endpoint
        )
    }
}
