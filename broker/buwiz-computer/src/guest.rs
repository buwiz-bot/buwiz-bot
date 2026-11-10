//! The client for the services inside a box: `box-exec` (commands, files, input, screenshots,
//! waits and recordings) and `box-host` (readiness, what it can do, its screens). Every provider
//! that runs the computer image reaches them the same way, over whatever address it got. Plain
//! HTTP/1 on loopback: no TLS, no redirects. A remote box is reached through a tunnel that ends on
//! loopback here, so this is the same client. What answers can be anything there: every call has
//! a limit that covers its whole answer, and what the far end says is cut to a size.
//!
//! The contract is `computer.md`'s and the image's `docs/openapi.yaml`. Bearer on everything; a
//! guest's error is `{error: {code, message}}`. What acts on a screen says which with
//! `?display=:N`; the broker's model tools can't set it, the broker does, from the Bot.
//!
//! What runs in the box runs as a user of its own, chosen there from the Bot named here: `bot` on
//! a command or a file, `/v1/screen` for what takes root. A command can't take over these ports,
//! which sit below the box's unprivileged range. Even so, nothing here is believed for a security
//! decision: `box-screen`'s answer is checked, and no capability the box claims widens what a tool
//! may do. What the Bot relies on, it reads from outside ([`crate::Provider::users_apart`]).

use std::time::Duration;

use base64::Engine;
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Method, Request};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use serde_json::{json, Value};

use crate::{ActiveTab, BrowserOp, BrowserReply, CommandOutput, Input, Problem, Result, Screen, Screenshot, Secret, Settled, WaitFor};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// What a call that isn't a command gets.
const PATIENCE: Duration = Duration::from_secs(30);
/// What a browser call gets: the box gives one 45 s, and a few more for the answer to come.
const BROWSER_PATIENCE: Duration = Duration::from_secs(55);
/// What a screen request gets: `box-screen` waits its turn, then for an X server and a viewer.
const SCREEN_PATIENCE: Duration = Duration::from_secs(50);
/// What the guest may answer with before it counts as broken.
const MAX_BODY: usize = 64 * 1024 * 1024;
/// The most pixels a screenshot may say it has: unpacking takes four bytes each, on this Mac.
const MAX_PIXELS: u64 = 64 * 1024 * 1024;
/// The most of what a guest says that goes on: to a person, or the model.
const MAX_SAID: usize = 300;

/// One computer's services.
pub struct BoxGuest {
    exec: String,
    host: String,
    token: Secret,
    http: Client<HttpConnector, Full<Bytes>>,
    /// the names to ask for, when `exec` and `host` are a tunnel to a proxy that goes by them
    names: Option<(String, String)>,
}

impl BoxGuest {
    /// `exec` and `host` are `http://…` bases.
    pub fn connect(exec: &str, host: &str, token: Secret) -> Self {
        let mut connector = HttpConnector::new();
        connector.set_connect_timeout(Some(CONNECT_TIMEOUT));
        Self { exec: exec.trim_end_matches('/').to_owned(), host: host.trim_end_matches('/').to_owned(), token, http: Client::builder(TokioExecutor::new()).build(connector), names: None }
    }

    /// Asks for `exec` and `host` by these names. A tunnel's address means nothing at the far end.
    pub fn named(mut self, exec: &str, host: &str) -> Self {
        self.names = Some((exec.to_owned(), host.to_owned()));
        self
    }

    /// Whether it answers `/v1/ready`: its services and its screen are up.
    pub async fn ready(&self) -> bool {
        self.ready_within(Duration::from_secs(2)).await
    }

    /// [`BoxGuest::ready`], over a link that may be slower than loopback.
    pub async fn ready_within(&self, patience: Duration) -> bool {
        self.send(Method::GET, &format!("{}/v1/ready", self.host), "application/json", None, patience).await.is_ok()
    }

    /// `GET /v1/info`.
    pub async fn info(&self) -> Result<Value> {
        parse(&self.send(Method::GET, &format!("{}/v1/info", self.host), "application/json", None, PATIENCE).await?)
    }

    /// `GET /v1/screens`.
    pub async fn listed(&self) -> Result<Value> {
        parse(&self.send(Method::GET, &format!("{}/v1/screens", self.host), "application/json", None, PATIENCE).await?)
    }

    /// `box-screen`, through `POST /v1/screen`: what it printed, or the error it gave.
    async fn screen(&self, body: Value) -> Result<String> {
        let url = format!("{}/v1/screen", self.exec);
        let bytes = self.send(Method::POST, &url, "application/json", Some(body.to_string()), SCREEN_PATIENCE).await?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        match serde_json::from_str::<Value>(text.trim()).ok().and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_owned)) {
            Some(said) => Err(Problem::Guest { status: 500, message: short(&said) }),
            None => Ok(text),
        }
    }

    /// `box-screen up <bot>`: the Bot's screen, started or found.
    pub async fn screen_up(&self, bot: &str) -> Result<Screen> {
        parse_screen(&self.screen(json!({"action": "up", "bot": bot})).await?, bot)
    }

    /// Stops `bot`'s screen and frees its slot. Its commands are asked to stop, and get two seconds.
    pub async fn screen_down(&self, bot: &str) -> Result<()> {
        self.screen(json!({"action": "forget", "bot": bot})).await.map(|_| ())
    }

    /// `box-screen list`: the Bots with a slot.
    pub async fn screens(&self) -> Result<Vec<String>> {
        Ok(parse_screens(&self.screen(json!({"action": "list"})).await?))
    }

    /// `path`, relative to the workspace, with its links followed, as far as its owner can see.
    pub async fn resolve(&self, path: &str) -> Result<String> {
        let full = format!("/workspace/{path}");
        Ok(self.run_argv(owner_of(path), &["realpath", "-m", "--", &full]).await?.stdout.trim_end_matches('\n').to_owned())
    }

    pub async fn screenshot(&self, screen: &Screen) -> Result<Screenshot> {
        let url = format!("{}/v1/cua/screenshot?display={}", self.exec, query(&screen.display));
        let png = self.send(Method::POST, &url, "image/png", None, PATIENCE).await?;
        let (width, height) = png_dimensions(&png).ok_or_else(|| Problem::Guest { status: 502, message: "its screenshot isn't a PNG".to_owned() })?;
        if u64::from(width) * u64::from(height) > MAX_PIXELS {
            return Err(Problem::Guest { status: 502, message: "its screenshot is too big".to_owned() });
        }
        Ok(Screenshot { png: png.to_vec(), width, height })
    }

    pub async fn input(&self, screen: &Screen, input: &Input) -> Result<()> {
        let (path, body) = input_call(input);
        self.post(&on(path, screen), &body, PATIENCE).await.map(|_| ())
    }

    /// A shell command of `screen`'s Bot, as its user, in its folder. It gets no display: only
    /// what the box's services do reaches a screen.
    pub async fn run(&self, screen: &Screen, command: &str, timeout_ms: u64) -> Result<CommandOutput> {
        let patience = Duration::from_millis(timeout_ms) + Duration::from_secs(5);
        let v = self.post("/v1/exec", &json!({"command": command, "timeout_ms": timeout_ms, "cwd": screen.folder, "bot": screen.bot()}), patience).await?;
        Ok(command_output(&v))
    }

    /// Opens `url` in `screen`'s browser, as its desktop's user. No shell, so nothing in it is read
    /// as one.
    pub async fn open_url(&self, screen: &Screen, url: &str) -> Result<()> {
        self.screen(open_request(screen, url)?).await.map(|_| ())
    }

    /// `POST /v1/browser/<op>?display=` with `body`: the answer, a refusal with its code included.
    /// A box that couldn't be reached, or that answered with no JSON, is a [`Problem`].
    pub async fn browser(&self, screen: &Screen, op: BrowserOp, body: &Value) -> Result<BrowserReply> {
        let url = format!("{}{}", self.exec, on(op.path(), screen));
        let (status, bytes) = self.exchange(Method::POST, &url, "application/json", Some(body.to_string()), BROWSER_PATIENCE).await?;
        let body = if bytes.is_empty() { Value::Null } else { parse(&bytes)? };
        Ok(BrowserReply { status, body })
    }

    /// `GET /v1/chrome/active-tab?display=` (box-host).
    pub async fn active_tab(&self, screen: &Screen) -> Result<ActiveTab> {
        let url = format!("{}{}", self.host, on("/v1/chrome/active-tab", screen));
        let v = parse(&self.send(Method::GET, &url, "application/json", None, PATIENCE).await?)?;
        Ok(active_tab_of(&v))
    }

    /// `GET /v1/info`'s `capabilities` (`Null` when it has none).
    pub async fn capabilities(&self) -> Result<Value> {
        Ok(self.info().await?.get("capabilities").cloned().unwrap_or(Value::Null))
    }

    async fn run_argv(&self, bot: Option<&str>, argv: &[&str]) -> Result<CommandOutput> {
        let v = self.post("/v1/exec", &json!({"command": argv, "timeout_ms": 30000, "bot": bot}), Duration::from_secs(35)).await?;
        let out = command_output(&v);
        if out.exit_code != Some(0) {
            let said = serde_json::from_str::<Value>(out.stdout.trim()).ok().and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_owned));
            return Err(Problem::Guest { status: 500, message: said.map_or_else(|| format!("{} failed", argv.first().unwrap_or(&"")), |s| short(&s)) });
        }
        Ok(out)
    }

    /// The bytes of a file under the workspace.
    pub async fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        let bot = owner_of(path).map(|b| format!("&bot={}", query(b))).unwrap_or_default();
        let url = format!("{}/v1/files?path={}&encoding=base64{bot}", self.exec, query(path));
        let v = parse(&self.send(Method::GET, &url, "application/json", None, PATIENCE).await?)?;
        if v.get("kind").and_then(Value::as_str) == Some("directory") {
            return Err(Problem::Guest { status: 400, message: format!("{path} is a folder") });
        }
        let content = v.get("content").and_then(Value::as_str).unwrap_or_default();
        base64::engine::general_purpose::STANDARD.decode(content).map_err(|e| Problem::Guest { status: 502, message: format!("the file came back unreadable: {e}") })
    }

    pub async fn write_file(&self, path: &str, content: &[u8]) -> Result<()> {
        let body = json!({"path": path, "content": base64::engine::general_purpose::STANDARD.encode(content), "encoding": "base64", "create_dirs": true, "bot": owner_of(path)});
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
        v.get("path").and_then(Value::as_str).map(short).ok_or_else(|| Problem::Guest { status: 502, message: "the recording has no path".to_owned() })
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
        let (status, body) = self.exchange(method, url, accept, body, patience).await?;
        if !(200..300).contains(&status) {
            return Err(Problem::Guest { status, message: error_text(&body, status) });
        }
        Ok(body)
    }

    /// One request, and its status and whole answer whatever the status.
    async fn exchange(&self, method: Method, url: &str, accept: &str, body: Option<String>, patience: Duration) -> Result<(u16, Bytes)> {
        let mut request = Request::builder().method(method).uri(url).header("authorization", format!("Bearer {}", self.token.expose())).header("accept", accept);
        if let Some((exec, host)) = &self.names {
            request = request.header("host", if url.starts_with(&self.exec) { exec } else { host });
        }
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        let request = request.body(Full::new(Bytes::from(body.unwrap_or_default()))).map_err(|e| Problem::Failed(format!("a bad request: {e}")))?;
        // the whole answer, body included: a far end that stops halfway mustn't hold a call for good
        tokio::time::timeout(patience, async {
            let response = self.http.request(request).await.map_err(|_| Problem::Guest { status: 503, message: "it isn't answering".to_owned() })?;
            let status = response.status().as_u16();
            let body = Limited::new(response.into_body(), MAX_BODY).collect().await.map_err(|_| Problem::Guest { status: 502, message: "its answer broke off".to_owned() })?.to_bytes();
            Ok((status, body))
        })
        .await
        .map_err(|_| Problem::Guest { status: 504, message: format!("no answer in {}s", patience.as_secs()) })?
    }
}

async fn pause() -> Settled {
    tokio::time::sleep(Duration::from_millis(500)).await;
    Settled { waited_ms: 500, fallback: true, ..Settled::default() }
}

fn open_request(screen: &Screen, url: &str) -> Result<Value> {
    let bot = screen.bot().ok_or_else(|| Problem::Failed("this screen has no browser of its own".to_owned()))?;
    Ok(json!({"action": "open", "bot": bot, "url": url}))
}

/// The Bot whose folder `path` is in, with or without the workspace in front. Files anywhere else
/// are the shared user's.
fn owner_of(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/workspace/").unwrap_or(path).strip_prefix("agents/")?;
    rest.split('/').next().filter(|b| !b.is_empty())
}

/// `path` on `screen`.
fn on(path: &str, screen: &Screen) -> String {
    format!("{path}?display={}", query(&screen.display))
}

/// `box-screen up`'s line for `bot`. Each field is checked, and the folder is the one a Bot has
/// whatever the box says: this comes from inside.
pub fn parse_screen(stdout: &str, bot: &str) -> Result<Screen> {
    let bad = || Problem::Guest { status: 502, message: "box-screen didn't name the screen".to_owned() };
    let v: Value = serde_json::from_str(stdout.trim()).map_err(|_| Problem::Guest { status: 502, message: "box-screen's answer wasn't JSON".to_owned() })?;
    let display = v.get("display").and_then(Value::as_str).filter(|d| d.strip_prefix(':').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))).ok_or_else(bad)?;
    let viewer_port = v.get("viewer_port").and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok()).filter(|p| (6901..=6905).contains(p)).ok_or_else(bad)?;
    let profile = v.get("profile").and_then(Value::as_str).map(str::to_owned);
    Ok(Screen { display: display.to_owned(), viewer_port, folder: format!("agents/{bot}"), profile })
}

/// `GET /v1/chrome/active-tab`'s answer. What isn't the right type is taken as not said.
pub fn active_tab_of(v: &Value) -> ActiveTab {
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    ActiveTab { url: text("url").filter(|u| !u.is_empty()), focus: text("focus"), seq: v.get("seq").and_then(Value::as_u64) }
}

/// `box-screen list`'s Bots.
pub fn parse_screens(stdout: &str) -> Vec<String> {
    let v: Value = serde_json::from_str(stdout.trim()).unwrap_or(Value::Null);
    v.get("screens").and_then(Value::as_array).into_iter().flatten().filter_map(|s| s.get("bot")?.as_str().map(str::to_owned)).collect()
}

fn parse(body: &[u8]) -> Result<Value> {
    serde_json::from_slice(body).map_err(|_| Problem::Guest { status: 502, message: "its answer wasn't JSON".to_owned() })
}

/// A guest error's message, else the status.
fn error_text(body: &[u8], status: u16) -> String {
    serde_json::from_slice::<Value>(body).ok().and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(short)).unwrap_or_else(|| format!("status {status}"))
}

/// What a guest said, cut to a size that fits a sentence.
fn short(said: &str) -> String {
    said.chars().take(MAX_SAID).collect()
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
        let s = parse_screen(line, "b_1").unwrap();
        assert_eq!((s.display.as_str(), s.viewer_port, s.folder.as_str()), (":2", 6902, "agents/b_1"));
        assert_eq!(s.bot(), Some("b_1"));
        assert_eq!(on("/v1/cua/click", &s), "/v1/cua/click?display=%3A2");
        assert!(parse_screen(r#"{"error":"full"}"#, "b_1").is_err());
    }

    #[test]
    fn what_box_screen_says_isnt_believed() {
        let forged = r#"{"display":":2","viewer_port":6902,"workspace":"../.."}"#;
        assert_eq!(parse_screen(forged, "b_1").unwrap().folder, "agents/b_1", "the folder is the Bot's own");
        assert!(parse_screen(r#"{"display":":2","viewer_port":1337}"#, "b_1").is_err(), "a control port isn't a viewer");
        assert!(parse_screen(r#"{"display":":2;x","viewer_port":6902}"#, "b_1").is_err());
    }

    #[test]
    fn a_page_opens_through_the_screens_own_route() {
        let screen = parse_screen(r#"{"display":":2","viewer_port":6902}"#, "b_1").unwrap();
        assert_eq!(open_request(&screen, "https://a.example").unwrap(), json!({"action": "open", "bot": "b_1", "url": "https://a.example"}));
        let shared = Screen { folder: "shared".into(), ..screen };
        assert!(open_request(&shared, "https://a.example").is_err());
    }

    #[test]
    fn a_file_belongs_to_the_bot_whose_folder_its_in() {
        assert_eq!(owner_of("agents/b_1/notes/a.txt"), Some("b_1"));
        assert_eq!(owner_of("/workspace/agents/b_1/a.txt"), Some("b_1"));
        assert_eq!(owner_of("kept.txt"), None);
        assert_eq!(owner_of("/etc/passwd"), None);
        assert_eq!(owner_of("agents/"), None);
    }

    #[test]
    fn the_list_names_the_bots() {
        assert_eq!(parse_screens(r#"{"screens":[{"bot":"a"},{"bot":"b"}]}"#), ["a", "b"]);
        assert_eq!(parse_screens("junk"), Vec::<String>::new());
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

    /// A guest on loopback that answers every connection with `reply`, then holds it open.
    async fn answering(reply: Vec<u8>) -> BoxGuest {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                let reply = reply.clone();
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut head = [0u8; 4096];
                    let _ = s.read(&mut head).await;
                    let _ = s.write_all(&reply).await;
                    tokio::time::sleep(Duration::from_secs(30)).await;
                });
            }
        });
        BoxGuest::connect(&addr, &addr, Secret::new("t"))
    }

    fn http(status: &str, extra: &str, body: &[u8]) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        out.extend_from_slice(body);
        out
    }

    #[tokio::test]
    async fn a_guest_that_stops_halfway_through_its_answer_doesnt_hold_the_call() {
        let guest = answering(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nhalf".to_vec()).await;
        let done = tokio::time::timeout(Duration::from_secs(5), guest.ready_within(Duration::from_millis(200))).await;
        assert!(!done.expect("the call hung on the body"));
    }

    #[tokio::test]
    async fn a_browser_refusal_comes_back_with_its_code_and_the_front_tab_is_read() {
        let refusal = br#"{"error":{"code":"confirm_required","message":"x","details":{"label":"Pay $1","confirm":"t"}}}"#;
        let guest = answering(http("409 Conflict", "", refusal)).await;
        let screen = Screen { display: ":3".into(), viewer_port: 6903, folder: "agents/b".into(), profile: None };
        let reply = guest.browser(&screen, BrowserOp::Act, &json!({"ref": "@e1", "action": "key", "text": "Enter"})).await.unwrap();
        assert_eq!((reply.status, reply.code()), (409, Some("confirm_required")));
        assert!(!format!("{reply:?}").contains("Pay"), "a reply's Debug shows no page text: {reply:?}");
        let tab = active_tab_of(&json!({"url": "https://shop.example/login", "focus": "password", "seq": 7}));
        assert_eq!(tab, ActiveTab { url: Some("https://shop.example/login".into()), focus: Some("password".into()), seq: Some(7) });
        assert_eq!(active_tab_of(&json!({"url": null, "seq": "x"})), ActiveTab::default());
    }

    #[tokio::test]
    async fn a_redirect_isnt_followed_and_is_a_no() {
        let elsewhere = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let to = format!("Location: http://{}/v1/info\r\n", elsewhere.local_addr().unwrap());
        let guest = answering(http("302 Found", &to, b"")).await;
        assert!(matches!(guest.info().await, Err(Problem::Guest { status: 302, .. })));
        assert!(tokio::time::timeout(Duration::from_millis(100), elsewhere.accept()).await.is_err(), "the token went to the other host");
    }

    #[tokio::test]
    async fn what_a_guest_says_is_cut_to_a_size() {
        let said = "x".repeat(100_000);
        let guest = answering(http("500 No", "", json!({"error": {"message": said}}).to_string().as_bytes())).await;
        match guest.info().await {
            Err(Problem::Guest { message, .. }) => assert_eq!(message.len(), MAX_SAID),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn a_screenshot_that_claims_to_be_huge_is_refused_before_anything_unpacks_it() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&60_000u32.to_be_bytes());
        png.extend_from_slice(&60_000u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0]);
        let guest = answering(http("200 OK", "Content-Type: image/png\r\n", &png)).await;
        let screen = parse_screen(r#"{"display":":2","viewer_port":6902}"#, "b_1").unwrap();
        assert!(matches!(guest.screenshot(&screen).await, Err(Problem::Guest { status: 502, .. })));
    }

    #[tokio::test]
    async fn a_named_guest_asks_for_each_service_by_its_name() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        let other = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let exec = format!("http://{}", other.local_addr().unwrap());
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        for l in [listener, other] {
            let tx = tx.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let (mut s, _) = l.accept().await.unwrap();
                let mut head = vec![0u8; 4096];
                let n = s.read(&mut head).await.unwrap();
                tx.send(String::from_utf8_lossy(&head[..n]).to_lowercase()).await.unwrap();
                s.write_all(&http("200 OK", "", b"{}")).await.unwrap();
            });
        }
        let guest = BoxGuest::connect(&exec, &addr, Secret::new("t")).named("exec.a.example", "host.a.example");
        guest.info().await.unwrap();
        assert!(rx.recv().await.unwrap().contains("host: host.a.example"));
        guest.post("/v1/x", &json!({}), Duration::from_secs(2)).await.unwrap();
        assert!(rx.recv().await.unwrap().contains("host: exec.a.example"));
    }

    #[test]
    fn a_path_is_encoded_for_the_query() {
        assert_eq!(query("notes/a b&c.txt"), "notes/a%20b%26c.txt");
    }
}
