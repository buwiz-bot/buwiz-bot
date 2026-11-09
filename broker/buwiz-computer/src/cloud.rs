//! The control API of a cloud platform: JSON over HTTPS, to a host the platform owns.
//!
//! **Trust first.** Every call finishes a TLS handshake that checks the public roots and the host's
//! name ([`tls::verified`]), and only then writes its request. So an API key, a sandbox's access
//! token or a computer's secret goes to a host the roots vouch for under the name dialled, and to
//! nothing else. No pin: a platform's certificate renews, and the roots already say who holds its
//! name. A pin is for a machine a person pointed at, which is [`crate::remote`]'s case.
//!
//! **Nothing the platform says is trusted beyond its status and its shape.** Each call has a time
//! for the whole of it; an answer is read up to [`MAX_REPLY`] bytes; a redirect is an error, so a
//! header with a key in it never follows a `Location` to a host the roots didn't check.

#[cfg(test)]
pub mod fake;

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::header::{CONNECTION, HOST};
use hyper::{Method, Request};
use hyper_util::rt::TokioIo;
use serde_json::Value;

use crate::remote::tls;
use crate::{Problem, ProviderKind, Result, Secret};

/// The most an answer may be. A platform's JSON is small; a page of listings is the largest.
pub const MAX_REPLY: usize = 1024 * 1024;

/// What came back: the status, the pagination cursor if the platform sent one, and the body.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    /// the `X-Next-Token` header, when there's another page
    pub next: Option<String>,
    pub body: Vec<u8>,
}

impl Reply {
    /// The body as JSON. Anything else is a sentence, not a crash.
    pub fn json(&self) -> Result<Value> {
        serde_json::from_slice(&self.body).map_err(|_| {
            Problem::Failed("the platform's answer wasn't the JSON it should be".to_owned())
        })
    }
}

/// A client for one platform's HTTPS. The fields that differ from the defaults are for tests: a
/// fake platform on this Mac, and a root that vouches for its certificate.
#[derive(Clone, Debug, Default)]
pub struct Api {
    to: Option<SocketAddr>,
    root: Option<Vec<u8>>,
}

impl Api {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sends every name to `to`, and takes `root` as one of the public roots (tests).
    pub fn with_test(to: SocketAddr, root: Option<Vec<u8>>) -> Self {
        Self { to: Some(to), root }
    }

    /// Where every name goes instead of its own address, for a test.
    pub fn to(&self) -> Option<SocketAddr> {
        self.to
    }

    /// The extra root a test trusts, DER.
    pub fn root(&self) -> Option<&[u8]> {
        self.root.as_deref()
    }

    /// One call to `host` (port 443): `method` on `path`, with `headers` and an optional `body`,
    /// all of it done within `patience`. Nothing is written before the handshake has checked
    /// `host`'s name.
    pub async fn call(
        &self,
        host: &str,
        method: Method,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<Bytes>,
        patience: Duration,
    ) -> Result<Reply> {
        match tokio::time::timeout(patience, self.exchange(host, method, path, headers, body)).await
        {
            Ok(done) => done,
            Err(_) => Err(Problem::Failed(format!(
                "{host} took too long to answer, so the call was stopped."
            ))),
        }
    }

    async fn exchange(
        &self,
        host: &str,
        method: Method,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<Bytes>,
    ) -> Result<Reply> {
        let tls = tls::verified(host, 443, self.to, self.root.as_deref()).await?;
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake::<_, Full<Bytes>>(TokioIo::new(tls))
                .await
                .map_err(|_| broken(host))?;
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(HOST, host)
            .header(CONNECTION, "close");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let request = request
            .body(Full::new(body.unwrap_or_default()))
            .map_err(|_| broken(host))?;
        let answer = sender
            .send_request(request)
            .await
            .map_err(|_| broken(host))?;
        let status = answer.status().as_u16();
        if (300..400).contains(&status) {
            return Err(Problem::Failed(format!(
                "{host} sent a redirect, which isn't followed."
            )));
        }
        let next = answer
            .headers()
            .get("x-next-token")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let body = Limited::new(answer.into_body(), MAX_REPLY)
            .collect()
            .await
            .map_err(|_| Problem::Failed(format!("{host}'s answer was too big, or broke off.")))?
            .to_bytes();
        Ok(Reply {
            status,
            next,
            body: body.to_vec(),
        })
    }
}

fn broken(host: &str) -> Problem {
    Problem::Failed(format!(
        "{host} didn't answer as HTTP, so nothing more was sent."
    ))
}

/// What a cloud provider is set up with: its API key, and what its computers are made from (an E2B
/// template id, or a boat image). Providers take this in their constructor and never read the
/// environment themselves, so a later source (the Keychain, a settings page) only has to produce a
/// `Settings`.
#[derive(Clone, Debug, Default)]
pub struct Settings {
    pub key: Option<Secret>,
    pub image: Option<String>,
}

impl Settings {
    pub fn new(key: Option<Secret>, image: Option<String>) -> Self {
        Self { key, image }
    }
}

/// The settings for `kind`, from the environment. A development source only: it's the one resolver,
/// so the providers don't change when the owner's settings move somewhere else.
pub fn settings(kind: ProviderKind) -> Settings {
    let (key_var, image_var) = match kind {
        ProviderKind::E2b => ("E2B_API_KEY", "BUWIZ_E2B_TEMPLATE"),
        ProviderKind::Boat => ("BOAT_API_KEY", "BUWIZ_BOAT_IMAGE"),
        _ => return Settings::default(),
    };
    let read = |name: &str| {
        std::env::var(name)
            .ok()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    Settings {
        key: read(key_var).map(Secret::new),
        image: read(image_var),
    }
}
