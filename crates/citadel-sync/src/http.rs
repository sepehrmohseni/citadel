use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{ETAG, HeaderMap};
use reqwest::{Client, Response, StatusCode};
use rustls_platform_verifier::BuilderVerifierExt;

use crate::{Result, SyncError};

const TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const BODY_PREVIEW: usize = 512;
pub(crate) const MAX_BODY: usize = 128 << 20;

pub(crate) async fn body(mut resp: Response, cap: usize) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if out.len() + chunk.len() > cap {
            return Err(SyncError::Oversize(cap));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

pub(crate) fn client() -> Result<Client> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| SyncError::Transport(e.to_string()))?
        .with_platform_verifier()
        .map_err(|e| SyncError::Transport(e.to_string()))?
        .with_no_client_auth();
    tls.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(Client::builder()
        .use_preconfigured_tls(tls)
        .timeout(TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(concat!("citadel-sync/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

pub(crate) fn check_scheme(url: &url::Url) -> Result<()> {
    match url.scheme() {
        "https" | "http" if url.host_str().is_some() => Ok(()),
        other => Err(SyncError::Config(format!(
            "unsupported endpoint scheme {other:?}"
        ))),
    }
}

pub(crate) fn etag(headers: &HeaderMap) -> Option<String> {
    headers
        .get(ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

pub(crate) async fn failure(resp: Response) -> SyncError {
    let status = resp.status();
    if status == StatusCode::PRECONDITION_FAILED || status == StatusCode::CONFLICT {
        return SyncError::Conflict;
    }
    let mut body = String::new();
    let mut resp = resp;
    while body.len() <= BODY_PREVIEW {
        match resp.chunk().await {
            Ok(Some(chunk)) => body.push_str(&String::from_utf8_lossy(&chunk)),
            _ => break,
        }
    }
    if body.len() > BODY_PREVIEW {
        let mut cut = BODY_PREVIEW;
        while !body.is_char_boundary(cut) {
            cut -= 1;
        }
        body.truncate(cut);
    }
    SyncError::Http {
        status: status.as_u16(),
        body,
    }
}
