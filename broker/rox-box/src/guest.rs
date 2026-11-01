//! The thinnest client for the services inside a desktop box (the `grok-box` image's
//! `box-host` on 1340 and `box-exec` on 1337): `/v1/info` and a PNG screenshot, over the
//! loopback ports Docker published. Plain HTTP/1 on 127.0.0.1 — no TLS, no redirects.
//!
//! Wire: hexuria/box crates/grok-box/src/lib.rs (`info`, `screenshot_png`, the bearer header).

use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Method, Request};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;

use crate::{BoxError, BoxResult};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// A screenshot of a busy desktop can take a few seconds to encode.
const SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(20);
/// A PNG is at most this big before we call the guest broken.
const MAX_BODY: usize = 32 * 1024 * 1024;

pub struct Guest {
    exec_url: String,
    host_url: String,
    token: String,
    http: Client<HttpConnector, Full<Bytes>>,
}

impl Guest {
    pub fn connect(exec_url: &str, host_url: &str, token: &str) -> Self {
        let mut connector = HttpConnector::new();
        connector.set_connect_timeout(Some(CONNECT_TIMEOUT));
        let http = Client::builder(TokioExecutor::new()).build(connector);
        Self {
            exec_url: exec_url.trim_end_matches('/').to_string(),
            host_url: host_url.trim_end_matches('/').to_string(),
            token: token.to_string(),
            http,
        }
    }

    /// `GET /v1/info` on box-host; the body is the guest's own report (versions, the egress
    /// tunnel, its container-local endpoints).
    pub async fn info(&self) -> BoxResult<serde_json::Value> {
        let url = format!("{}/v1/info", self.host_url);
        let bytes = self
            .send(Method::GET, &url, "application/json", SCREENSHOT_TIMEOUT)
            .await?;
        serde_json::from_slice(&bytes).map_err(|e| BoxError::Refused {
            status: 502,
            body: format!("guest info is not JSON: {e}"),
        })
    }

    /// `POST /v1/cua/screenshot` on box-exec, asking for the PNG itself.
    pub async fn screenshot_png(&self) -> BoxResult<Vec<u8>> {
        let url = format!("{}/v1/cua/screenshot", self.exec_url);
        let bytes = self
            .send(Method::POST, &url, "image/png", SCREENSHOT_TIMEOUT)
            .await?;
        Ok(bytes.to_vec())
    }

    async fn send(
        &self,
        method: Method,
        url: &str,
        accept: &str,
        patience: Duration,
    ) -> BoxResult<Bytes> {
        let request = Request::builder()
            .method(method)
            .uri(url)
            .header("authorization", format!("Bearer {}", self.token))
            .header("accept", accept)
            .body(Full::new(Bytes::new()))
            .map_err(|e| BoxError::Unreachable(format!("bad guest request: {e}")))?;
        let response = tokio::time::timeout(patience, self.http.request(request))
            .await
            .map_err(|_| {
                BoxError::Interrupted(format!(
                    "the guest took longer than {}s",
                    patience.as_secs()
                ))
            })?
            .map_err(|e| BoxError::Unreachable(format!("guest at {url}: {e}")))?;
        let status = response.status().as_u16();
        let body = http_body_util::Limited::new(response.into_body(), MAX_BODY)
            .collect()
            .await
            .map_err(|e| BoxError::Unreachable(format!("reading the guest's answer: {e}")))?
            .to_bytes();
        if !(200..300).contains(&status) {
            return Err(BoxError::Refused {
                status,
                body: String::from_utf8_lossy(&body).into_owned(),
            });
        }
        Ok(body)
    }
}

/// Width and height from a PNG's IHDR chunk (always first, big-endian, right after the
/// 8-byte signature): `None` for anything that is not a PNG.
pub fn png_dimensions(png: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if png.len() < 24 || &png[..8] != SIGNATURE || &png[12..16] != b"IHDR" {
        return None;
    }
    let be = |at: usize| -> Option<u32> {
        let bytes: [u8; 4] = png.get(at..at + 4)?.try_into().ok()?;
        Some(u32::from_be_bytes(bytes))
    };
    Some((be(16)?, be(20)?))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn png_header_gives_the_size() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&1280u32.to_be_bytes());
        png.extend_from_slice(&800u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0]);
        assert_eq!(png_dimensions(&png), Some((1280, 800)));
        assert_eq!(png_dimensions(b"not a png at all, sorry about that"), None);
        assert_eq!(png_dimensions(b""), None);
    }

    #[test]
    fn urls_lose_their_trailing_slash() {
        let guest = Guest::connect("http://127.0.0.1:1337/", "http://127.0.0.1:1340/", "t");
        assert_eq!(guest.exec_url, "http://127.0.0.1:1337");
        assert_eq!(guest.host_url, "http://127.0.0.1:1340");
    }
}
