//! The client for the services inside a box: `box-exec` (commands, files, input, screenshots,
//! waits and recordings) and `box-host` (readiness, what it can do, its screens). Every provider
//! that runs the computer image reaches them the same way, over whatever address it got. Plain
//! HTTP/1 on loopback: no TLS, no redirects.
//!
//! The contract is `computer.md`'s and the image's `docs/openapi.yaml`. Bearer on everything; a
//! guest's error is `{error: {code, message}}`. What acts on a screen says which with
//! `?display=:N`; the broker's model tools can't set it, the broker does, from the Bot.

use std::time::Duration;

use base64::Engine;
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Method, Request};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use serde_json::{json, Value};

use crate::{CommandOutput, Input, Problem, Result, Screen, Screenshot, Secret, Settled, WaitFor};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// What a call that isn't a command gets.
const PATIENCE: Duration = Duration::from_secs(30);
/// What the guest may answer with before it counts as broken.
const MAX_BODY: usize = 64 * 1024 * 1024;

/// One computer's services.
pub struct BoxGuest {
    exec: String,
    host: String,
    token: Secret,
    http: Client<HttpConnector, Full<Bytes>>,
}

impl BoxGuest {
    /// `exec` and `host` are `http://…` bases.
    pub fn connect(exec: &str, host: &str, token: Secret) -> Self {
        let mut connector = HttpConnector::new();
        connector.set_connect_timeout(Some(CONNECT_TIMEOUT));
        Self { exec: exec.trim_end_matches('/').to_owned(), host: host.trim_end_matches('/').to_owned(), token, http: Client::builder(TokioExecutor::new()).build(connector) }
    }

    /// Whether it answers `/v1/ready`: its services and its screen are up.
    pub async fn ready(&self) -> bool {
        self.send(Method::GET, &format!("{}/v1/ready", self.host), "application/json", None, Duration::from_secs(2)).await.is_ok()
    }

    /// `GET /v1/info`.
    pub async fn info(&self) -> Result<Value> {
        parse(&self.send(Method::GET, &format!("{}/v1/info", self.host), "application/json", None, PATIENCE).await?)
    }

    /// `box-screen up <bot>`: the Bot's screen, started or found.
    pub async fn screen_up(&self, bot: &str) -> Result<Screen> {
        let out = self.run_argv(&["box-screen", "up", bot]).await?;
        parse_screen(&out.stdout)
    }

    pub async fn screen_down(&self, bot: &str) -> Result<()> {
        self.run_argv(&["box-screen", "down", bot]).await.map(|_| ())
    }

    pub async fn screenshot(&self, screen: &Screen) -> Result<Screenshot> {
        let url = format!("{}/v1/cua/screenshot?display={}", self.exec, query(&screen.display));
        let png = self.send(Method::POST, &url, "image/png", None, PATIENCE).await?;
        let (width, height) = png_dimensions(&png).ok_or_else(|| Problem::Guest { status: 502, message: "its screenshot isn't a PNG".to_owned() })?;
        Ok(Screenshot { png: png.to_vec(), width, height })
    }

    pub async fn input(&self, screen: &Screen, input: &Input) -> Result<()> {
        let (path, body) = input_call(input);
        self.post(&on(path, screen), &body, PATIENCE).await.map(|_| ())
    }

    /// A shell command for `screen`: its display, in its folder.
    pub async fn run(&self, screen: &Screen, command: &str, timeout_ms: u64) -> Result<CommandOutput> {
        let patience = Duration::from_millis(timeout_ms) + Duration::from_secs(5);
        let v = self.post("/v1/exec", &json!({"command": command, "timeout_ms": timeout_ms, "cwd": screen.folder, "env": screen_env(screen)}), patience).await?;
        Ok(command_output(&v))
    }

    /// Opens `url` in `screen`'s browser. No shell, so nothing in it is read as one.
    pub async fn open_url(&self, screen: &Screen, url: &str) -> Result<()> {
        self.post("/v1/exec", &json!({"command": ["box-chromium", url], "detach": true, "cwd": screen.folder, "env": screen_env(screen)}), PATIENCE).await.map(|_| ())
    }

    async fn run_argv(&self, argv: &[&str]) -> Result<CommandOutput> {
        let v = self.post("/v1/exec", &json!({"command": argv, "timeout_ms": 30000}), Duration::from_secs(35)).await?;
        let out = command_output(&v);
        if out.exit_code != Some(0) {
            let said = serde_json::from_str::<Value>(out.stdout.trim()).ok().and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_owned));
            return Err(Problem::Guest { status: 500, message: said.unwrap_or_else(|| format!("{} failed", argv.first().unwrap_or(&""))) });
        }
        Ok(out)
    }

    /// The bytes of a file under the workspace.
    pub async fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        let url = format!("{}/v1/files?path={}&encoding=base64", self.exec, query(path));
        let v = parse(&self.send(Method::GET, &url, "application/json", None, PATIENCE).await?)?;
        if v.get("kind").and_then(Value::as_str) == Some("directory") {
            return Err(Problem::Guest { status: 400, message: format!("{path} is a folder") });
        }
        let content = v.get("content").and_then(Value::as_str).unwrap_or_default();
        base64::engine::general_purpose::STANDARD.decode(content).map_err(|e| Problem::Guest { status: 502, message: format!("the file came back unreadable: {e}") })
    }

    pub async fn write_file(&self, path: &str, content: &[u8]) -> Result<()> {
        let body = json!({"path": path, "content": base64::engine::general_purpose::STANDARD.encode(content), "encoding": "base64", "create_dirs": true});
        let url = format!("{}/v1/files", self.exec);
        self.send(Method::PUT, &url, "application/json", Some(body.to_string()), PATIENCE).await.map(|_| ())
    }

    /// `screen`'s change mark, right now.
    pub async fn mark(&self, screen: &Screen) -> Option<u64> {
        let v = self.post(&on("/v1/cua/wait", screen), &json!({"change": true, "timeout": 0}), Duration::from_secs(3)).await.ok()?;
        v.get("seq").and_then(Value::as_u64)
    }

    /// First what `since` says changed (when given), then stillness. If the screen's viewer can't
    /// say, a fixed pause, and the answer says so.
    pub async fn settle(&self, screen: &Screen, wait: WaitFor) -> Result<Settled> {
        let path = on("/v1/cua/wait", screen);
        let (mut changed, mut waited) = (false, 0);
        if let Some(since) = wait.since {
            let Ok(v) = self.post(&path, &json!({"change": true, "since": since, "timeout": 1000}), Duration::from_secs(6)).await else { return Ok(pause().await) };
            changed = v.get("changed").and_then(Value::as_bool).unwrap_or(false);
            waited = v.get("waited_ms").and_then(Value::as_u64).unwrap_or(0);
            if !changed {
                return Ok(Settled { waited_ms: waited, seq: v.get("seq").and_then(Value::as_u64), ..Settled::default() });
            }
        }
        let patience = Duration::from_millis(wait.timeout_ms) + Duration::from_secs(5);
        let Ok(v) = self.post(&path, &json!({"idle": wait.idle_ms, "timeout": wait.timeout_ms}), patience).await else { return Ok(pause().await) };
        Ok(Settled {
            changed: changed || v.get("changed").and_then(Value::as_bool).unwrap_or(false),
            timed_out: v.get("timed_out").and_then(Value::as_bool).unwrap_or(false),
            waited_ms: waited + v.get("waited_ms").and_then(Value::as_u64).unwrap_or(0),
            seq: v.get("seq").and_then(Value::as_u64),
            fallback: false,
        })
    }

    pub async fn record_start(&self, screen: &Screen) -> Result<u64> {
        let v = self.post(&on("/v1/cua/record/start", screen), &json!({}), PATIENCE).await?;
        v.get("id").and_then(Value::as_u64).ok_or_else(|| Problem::Guest { status: 502, message: "the recording has no id".to_owned() })
    }

    /// The recording's path, relative to the workspace.
    pub async fn record_stop(&self, screen: &Screen, id: u64) -> Result<String> {
        let v = self.post(&on("/v1/cua/record/stop", screen), &json!({"id": id}), Duration::from_secs(120)).await?;
        v.get("path").and_then(Value::as_str).map(str::to_owned).ok_or_else(|| Problem::Guest { status: 502, message: "the recording has no path".to_owned() })
    }

    async fn post(&self, path: &str, body: &Value, patience: Duration) -> Result<Value> {
        let url = format!("{}{path}", self.exec);
        let bytes = self.send(Method::POST, &url, "application/json", Some(body.to_string()), patience).await?;
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        parse(&bytes)
    }

    async fn send(&self, method: Method, url: &str, accept: &str, body: Option<String>, patience: Duration) -> Result<Bytes> {
        let mut request = Request::builder().method(method).uri(url).header("authorization", format!("Bearer {}", self.token.expose())).header("accept", accept);
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        let request = request.body(Full::new(Bytes::from(body.unwrap_or_default()))).map_err(|e| Problem::Failed(format!("a bad request: {e}")))?;
        let response = tokio::time::timeout(patience, self.http.request(request))
            .await
            .map_err(|_| Problem::Guest { status: 504, message: format!("no answer in {}s", patience.as_secs()) })?
            .map_err(|_| Problem::Guest { status: 503, message: "it isn't answering".to_owned() })?;
        let status = response.status().as_u16();
        let body = Limited::new(response.into_body(), MAX_BODY).collect().await.map_err(|_| Problem::Guest { status: 502, message: "its answer broke off".to_owned() })?.to_bytes();
        if !(200..300).contains(&status) {
            return Err(Problem::Guest { status, message: error_text(&body, status) });
        }
        Ok(body)
    }
}

async fn pause() -> Settled {
    tokio::time::sleep(Duration::from_millis(500)).await;
    Settled { waited_ms: 500, fallback: true, ..Settled::default() }
}

/// `path` on `screen`.
fn on(path: &str, screen: &Screen) -> String {
    format!("{path}?display={}", query(&screen.display))
}

/// What a command needs to land on `screen`.
fn screen_env(screen: &Screen) -> Value {
    let mut env = json!({"DISPLAY": screen.display, "BOX_DISPLAY": screen.display});
    if let Some(profile) = &screen.profile {
        env["BOX_CHROME_PROFILE"] = json!(profile);
    }
    env
}

/// `box-screen up`'s line.
pub fn parse_screen(stdout: &str) -> Result<Screen> {
    let v: Value = serde_json::from_str(stdout.trim()).map_err(|_| Problem::Guest { status: 502, message: "box-screen's answer wasn't JSON".to_owned() })?;
    let field = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    let port = v.get("viewer_port").and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok());
    match (field("display"), port, field("workspace")) {
        (Some(display), Some(viewer_port), Some(folder)) => Ok(Screen { display, viewer_port, folder, profile: field("profile") }),
        _ => Err(Problem::Guest { status: 502, message: "box-screen didn't name the screen".to_owned() }),
    }
}

fn parse(body: &[u8]) -> Result<Value> {
    serde_json::from_slice(body).map_err(|_| Problem::Guest { status: 502, message: "its answer wasn't JSON".to_owned() })
}

/// A guest error's message, else the status.
fn error_text(body: &[u8], status: u16) -> String {
    serde_json::from_slice::<Value>(body).ok().and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_owned)).unwrap_or_else(|| format!("status {status}"))
}

/// `value` percent-encoded for a query.
fn query(value: &str) -> String {
    value.bytes().fold(String::new(), |mut out, b| {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
        out
    })
}

/// The route and body of `input` (`docs/API.md`).
pub fn input_call(input: &Input) -> (&'static str, Value) {
    match input {
        Input::Click { x, y, button, double: false } => ("/v1/cua/click", json!({"x": x, "y": y, "button": button})),
        Input::Click { x, y, button, double: true } => ("/v1/cua/double-click", json!({"x": x, "y": y, "button": button})),
        Input::Move { x, y } => ("/v1/cua/move", json!({"x": x, "y": y})),
        Input::Drag { from, to } => ("/v1/cua/drag", json!({"x1": from.0, "y1": from.1, "x2": to.0, "y2": to.1, "button": 1})),
        Input::Type { text } => ("/v1/cua/type", json!({"text": text})),
        Input::Key { key } => ("/v1/cua/key", json!({"key": key})),
        Input::Scroll { x, y, dx, dy } => ("/v1/cua/scroll", json!({"x": x, "y": y, "dx": dx, "dy": dy})),
    }
}

/// `POST /v1/exec`'s answer.
pub fn command_output(v: &Value) -> CommandOutput {
    let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
    let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    CommandOutput {
        exit_code: v.get("exit_code").and_then(Value::as_i64).and_then(|c| i32::try_from(c).ok()),
        stdout: text("stdout"),
        stderr: text("stderr"),
        truncated: flag("truncated"),
        timed_out: flag("timed_out"),
    }
}

/// Width and height from a PNG's IHDR chunk: `None` for anything that isn't a PNG.
pub fn png_dimensions(png: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if png.len() < 24 || &png[..8] != SIGNATURE || &png[12..16] != b"IHDR" {
        return None;
    }
    let be = |at: usize| -> Option<u32> { Some(u32::from_be_bytes(png.get(at..at + 4)?.try_into().ok()?)) };
    Some((be(16)?, be(20)?))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn a_png_header_gives_the_size() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&1280u32.to_be_bytes());
        png.extend_from_slice(&800u32.to_be_bytes());
        assert_eq!(png_dimensions(&png), Some((1280, 800)));
        assert_eq!(png_dimensions(b"not a png at all, sorry about that"), None);
    }

    #[test]
    fn box_screens_answer_is_a_screen() {
        let line = r#"{"bot":"b_1","slot":2,"display":":2","viewer_port":6902,"workspace":"agents/b_1","profile":"/home/box/chrome-profile/bots/b_1","running":true}"#;
        let s = parse_screen(line).unwrap();
        assert_eq!((s.display.as_str(), s.viewer_port, s.folder.as_str()), (":2", 6902, "agents/b_1"));
        assert_eq!(screen_env(&s)["DISPLAY"], ":2");
        assert_eq!(on("/v1/cua/click", &s), "/v1/cua/click?display=%3A2");
        assert!(parse_screen(r#"{"error":"full"}"#).is_err());
    }

    #[test]
    fn input_goes_to_the_documented_routes() {
        assert_eq!(input_call(&Input::Click { x: 1, y: 2, button: 1, double: true }).0, "/v1/cua/double-click");
        assert_eq!(input_call(&Input::Drag { from: (1, 2), to: (3, 4) }).1, json!({"x1": 1, "y1": 2, "x2": 3, "y2": 4, "button": 1}));
        assert_eq!(input_call(&Input::Key { key: "Return".into() }), ("/v1/cua/key", json!({"key": "Return"})));
    }

    #[test]
    fn a_killed_command_has_no_exit_code() {
        let out = command_output(&json!({"stdout": "a", "stderr": "", "exit_code": null, "timed_out": true}));
        assert_eq!((out.exit_code, out.timed_out, out.stdout.as_str()), (None, true, "a"));
    }

    #[test]
    fn a_path_is_encoded_for_the_query() {
        assert_eq!(query("notes/a b&c.txt"), "notes/a%20b%26c.txt");
    }
}
