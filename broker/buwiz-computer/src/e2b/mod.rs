//! E2B: one sandbox per computer, made from a template the owner builds on the computer image.
//!
//! **Image.** E2B runs templates, not container images. The owner builds one template, once, from
//! the computer image with E2B's CLI. The template's Dockerfile starts from the image and makes the
//! two things the box reads its secrets from:
//!
//! ```text
//! FROM ghcr.io/buwiz-bot/computer:<tag>
//! USER root
//! RUN mkdir -p /run/buwiz && chown box /run/buwiz
//! ENV BOX_TOKEN_FILE=/run/buwiz/token BOX_VIEWER_PASSWORD_FILE=/run/buwiz/viewer WORKSPACE_ROOT=/workspace
//! ```
//!
//! and sets the template's start command, which runs once at build and is snapshotted with the
//! sandbox, so it is already waiting when a sandbox starts:
//!
//! ```text
//! sh -c 'until [ -f /run/buwiz/go ]; do sleep 0.2; done; exec /usr/bin/tini -- /usr/local/bin/box-keeper'
//! ```
//!
//! The other `BOX_*` settings are docker-compose.yml's, minus the secrets. No secret is in a
//! template, a build argument, or the environment: the box reads the two files and unlinks them.
//!
//! **Lifecycle, on E2B's API** (`api.e2b.app`, `X-API-Key`):
//! - *create*: `POST /v2/sandboxes` with `autoPause`, so a running sandbox that runs out of its
//!   hour is paused, memory kept, and not killed. Then the secrets go in ([`E2bProvider::stage`]),
//!   the box is checked, and the sandbox is paused. A computer starts asleep.
//! - *wake*: `POST /v2/sandboxes/{id}/connect`, which resumes a paused sandbox from memory
//!   (201) or finds it running (200). Resumed means what was open is still open.
//! - *sleep*: `POST /sandboxes/{id}/pause` at either depth. A disk-only pause would cold-boot on
//!   resume, and the box would need its secrets again, which it no longer has. So deep is a pause
//!   that keeps memory too.
//! - *destroy*: `DELETE /sandboxes/{id}`. A sandbox that's gone already is fine.
//! - *power*: the held connection, else `GET /sandboxes/{id}`: `running` or `paused`, or gone.
//! - *sweep*: every sandbox whose metadata names the computer, plus the one the record names.
//! - *recover*: nothing. A box that has read its secrets can't be given new ones, so a lost record
//!   isn't recovered; its sandbox is found and stopped by sweep.
//!
//! **Reach.** A sandbox's ports are at `https://<port>-<sandboxID>.e2b.app`. The broker runs one
//! TLS relay per service ([`crate::remote::tls`]) to `1337`, `1340` and `6901`–`6905`, which end on
//! loopback here, so [`BoxGuest`], the viewer gate and the Bot's shell rules don't change.
//!
//! **Trust.** The name comes from E2B's answer to our authenticated create or connect, never from
//! a person's input. Its certificate is checked against the public roots and the name, and no
//! secret goes until that check holds: [`tls::verified`] runs before the box's token is sent, and
//! [`crate::cloud::Api`] runs it before each call's key is written. A pin isn't used, because E2B's
//! certificates renew. The threat this design answers is a network or DNS attacker, who can't get
//! a publicly trusted certificate for `e2b.app`. It can't answer E2B itself: E2B runs the
//! computer, and holds what it holds, so it is trusted with the computer's contents by using it.
//!
//! **Secrets.** Staged by envd, E2B's agent in the sandbox, at `sandbox.e2b.app`, routed by the
//! `E2b-Sandbox-Id` and `E2b-Sandbox-Port: 49983` headers and authorised by the access token that
//! E2B returned to the create call. Three uploads, in order: `token`, `viewer`, then `go`, which is
//! the trigger. The files are owned by `box`, and box-keeper unlinks them as it reads them. What
//! envd gives an upload's mode is E2B's choice, and the window before the unlink is short; that
//! residual is written here and not solved.
//!
//! **What's exposed.** `allowPublicTraffic` is on, so every port the sandbox listens on answers at
//! its URL. The image listens on `1337`, `1340` and `6901`–`6905`, and `8790` only when the egress
//! tunnel is on, which it is not by default. `1337` and `1340` need the box token, and the viewers
//! need the viewer password; nothing else in the image answers without them. E2B's traffic-token
//! header would close the ports to everyone but us, but it has to ride on each request, and the
//! relays are byte relays, so it is not used.
//!
//! **Limits and gaps.** A running sandbox lasts an hour on the Hobby plan and 24 hours on Pro, and
//! then pauses itself. Only the plan's cap is in the code; the region is E2B's to choose, and Asia
//! is not confirmed. Bots can't share a sandbox: [`Provider::users_apart`] is not proven from
//! outside, so the hub keeps the one-Bot rule. Clearing strays is not built.
//!
//! **Config.** `E2B_API_KEY` (a secret, read from the environment, never logged) and
//! `BUWIZ_E2B_TEMPLATE` (the template's id or alias). With either missing, every call is a
//! sentence that says which, and no request is made.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use hyper::Method;
use serde_json::{json, Value};

use crate::cloud::{Api, Reply};
use crate::docker::{EXEC_PORT, HOST_PORT};
use crate::remote::tls::{self, Relays};
use crate::remote::{answers, beat, clean, guest_at, guest_ports, Open, Way, BEAT, CHECK_PATIENCE};
use crate::{
    CommandOutput, Far, Input, Machine, Power, Problem, Provider, ProviderKind, Reach, Result,
    Screen, Screenshot, Secret, Settled, Sleep, Spec, Viewer, WaitFor, Woke,
};

pub const KEY_ENV: &str = "E2B_API_KEY";
pub const TEMPLATE_ENV: &str = "BUWIZ_E2B_TEMPLATE";

const API_HOST: &str = "api.e2b.app";
const ENVD_HOST: &str = "sandbox.e2b.app";
const ENVD_PORT: u16 = 49983;
const DOMAIN: &str = "e2b.app";
/// Where the box's secrets are staged, in the sandbox.
const SECRET_DIR: &str = "/run/buwiz";
const BOX_USER: &str = "box";
/// A running sandbox's time before E2B pauses it: the Hobby plan's cap.
const SESSION_SECONDS: u64 = 3600;
const CALL_PATIENCE: Duration = Duration::from_secs(30);
const UPLOAD_PATIENCE: Duration = Duration::from_secs(30);
/// How long a sandbox gets to answer as a box, from the moment it's asked, polled every [`RETRY`].
const BOX_PATIENCE: Duration = Duration::from_secs(120);
const RETRY: Duration = Duration::from_secs(2);
/// The most pages of sandboxes one sweep reads.
const MAX_PAGES: usize = 10;
const DROPPED: &str = "The connection to this computer dropped. Click it to connect again.";

/// E2B's sandboxes, one per computer, and their connections. The hub keeps one, because it holds
/// the connections.
pub struct E2bProvider {
    key: Option<Secret>,
    template: Option<String>,
    api: Api,
    links: Mutex<HashMap<PathBuf, Arc<Open>>>,
    /// one connect or disconnect at a time, per computer
    turns: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
}

impl E2bProvider {
    pub fn new(key: Option<Secret>, template: Option<String>) -> Self {
        Self {
            key,
            template,
            api: Api::new(),
            links: Mutex::default(),
            turns: Mutex::default(),
        }
    }

    /// From `E2B_API_KEY` and `BUWIZ_E2B_TEMPLATE`. Neither is read back or logged.
    pub fn from_env() -> Self {
        let key = std::env::var(KEY_ENV)
            .ok()
            .map(|k| k.trim().to_owned())
            .filter(|k| !k.is_empty())
            .map(Secret::new);
        let template = std::env::var(TEMPLATE_ENV)
            .ok()
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty());
        Self::new(key, template)
    }

    /// Sends the platform's names to a test's fake, and trusts its root (tests).
    pub fn with_api(mut self, api: Api) -> Self {
        self.api = api;
        self
    }

    fn key(&self) -> Result<&Secret> {
        self.key.as_ref().ok_or_else(|| Problem::Failed(format!("E2B needs an API key: set {KEY_ENV} where the broker starts, then start the broker again.")))
    }

    fn template(&self) -> Result<&str> {
        self.template.as_deref().ok_or_else(|| Problem::Failed(format!("E2B needs a template: build one from the computer image, and set {TEMPLATE_ENV} to its id.")))
    }

    /// One call to the control API, with the key. The key goes only after the handshake (see [`crate::cloud`]).
    async fn api(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        patience: Duration,
    ) -> Result<Reply> {
        let key = self.key()?.expose();
        let mut headers = vec![("X-API-Key", key)];
        let body = body.map(|b| Bytes::from(b.to_string()));
        if body.is_some() {
            headers.push(("Content-Type", "application/json"));
        }
        self.api
            .call(API_HOST, method, path, &headers, body, patience)
            .await
    }

    /// The state of `id`: `running`, `paused`, or `None` when E2B has no such sandbox.
    async fn state(&self, id: &str) -> Result<Option<String>> {
        let reply = self
            .api(
                Method::GET,
                &format!("/sandboxes/{id}"),
                None,
                CALL_PATIENCE,
            )
            .await?;
        match reply.status {
            200 => Ok(reply
                .json()?
                .get("state")
                .and_then(Value::as_str)
                .map(str::to_owned)),
            404 => Ok(None),
            other => Err(outcome(other)),
        }
    }

    /// Pauses `id`, keeping its memory. Already paused is fine.
    async fn pause(&self, id: &str) -> Result<()> {
        match self
            .api(
                Method::POST,
                &format!("/sandboxes/{id}/pause"),
                None,
                CALL_PATIENCE,
            )
            .await?
            .status
        {
            204 => Ok(()),
            409 if self.state(id).await?.as_deref() == Some("paused") => Ok(()),
            404 => Err(Problem::Gone),
            other => Err(outcome(other)),
        }
    }

    /// Resumes `id` (or finds it running), with a session of its own.
    async fn resume(&self, id: &str) -> Result<Woke> {
        let reply = self
            .api(
                Method::POST,
                &format!("/v2/sandboxes/{id}/connect"),
                Some(json!({"timeout": SESSION_SECONDS})),
                CALL_PATIENCE,
            )
            .await?;
        match reply.status {
            200 => Ok(Woke::Started),
            201 => Ok(Woke::Resumed),
            404 => Err(Problem::Gone),
            other => Err(outcome(other)),
        }
    }

    /// Stops `id` for good. Gone already is fine.
    async fn kill(&self, id: &str) -> Result<()> {
        match self
            .api(
                Method::DELETE,
                &format!("/sandboxes/{id}"),
                None,
                CALL_PATIENCE,
            )
            .await?
            .status
        {
            200 | 204 | 404 => Ok(()),
            other => Err(outcome(other)),
        }
    }

    /// The ids of every sandbox whose metadata names `computer`, across the pages E2B gives.
    async fn listed(&self, computer: &str) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut path = format!(
                "/v2/sandboxes?state=running,paused&limit=100&metadata=buwiz-computer%3D{computer}"
            );
            if let Some(next) = &cursor {
                path.push_str(&format!("&nextToken={next}"));
            }
            let reply = self.api(Method::GET, &path, None, CALL_PATIENCE).await?;
            if reply.status != 200 {
                return Err(outcome(reply.status));
            }
            for item in reply.json()?.as_array().into_iter().flatten() {
                if let Some(id) = item.get("sandboxID").and_then(Value::as_str) {
                    ids.push(safe_id(id)?.to_owned());
                }
            }
            match reply.next {
                Some(next)
                    if next
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_=.".contains(&b)) =>
                {
                    cursor = Some(next)
                }
                Some(_) => {
                    return Err(Problem::Failed(
                        "E2B's listing came with a cursor the broker won't use.".to_owned(),
                    ))
                }
                None => return Ok(ids),
            }
        }
        Err(Problem::Failed(
            "E2B has more sandboxes for this computer than one sweep reads, so none were stopped."
                .to_owned(),
        ))
    }

    /// The secrets, into the sandbox, through envd: `token`, `viewer`, then the `go` that starts box-keeper.
    async fn stage(&self, id: &str, envd_token: &str, machine: &Machine) -> Result<()> {
        let port = ENVD_PORT.to_string();
        let files = vec![
            ("token", machine.token.expose().as_bytes().to_vec()),
            (
                "viewer",
                machine.viewer_password.expose().as_bytes().to_vec(),
            ),
            ("go", Vec::new()),
        ];
        for (name, bytes) in files {
            let path = format!("/files?path={SECRET_DIR}/{name}&username={BOX_USER}");
            let headers = [
                ("E2b-Sandbox-Id", id),
                ("E2b-Sandbox-Port", port.as_str()),
                ("X-Access-Token", envd_token),
                ("Content-Type", "application/octet-stream"),
            ];
            let reply = self
                .api
                .call(
                    ENVD_HOST,
                    Method::POST,
                    &path,
                    &headers,
                    Some(Bytes::from(bytes)),
                    UPLOAD_PATIENCE,
                )
                .await?;
            if reply.status != 200 {
                return Err(Problem::Failed(format!("E2B wouldn't take the computer's {name} file (status {}), so the computer was stopped.", reply.status)));
            }
        }
        Ok(())
    }

    /// Trust for the box: the exec name's certificate checks out, or nothing is sent to it.
    async fn trust(&self, id: &str) -> Result<()> {
        tls::verified(&name_of(EXEC_PORT, id), 443, self.api.to(), self.api.root())
            .await
            .map(drop)
    }

    /// One try at a link: relays for every service, and a box that answers as one. Its relays are
    /// closed if it doesn't.
    async fn link(&self, machine: &Machine, id: &str) -> Result<Arc<Open>> {
        let names: HashMap<u16, String> = guest_ports().map(|p| (p, name_of(p, id))).collect();
        let connector = tls::web_connector(self.api.root())?;
        let (relays, ports) = Relays::open_on(443, &names, connector, self.api.to()).await?;
        let exec = names.get(&EXEC_PORT).cloned().unwrap_or_default();
        let host = names.get(&HOST_PORT).cloned().unwrap_or_default();
        let guest = guest_at(&ports, machine.token.clone())?.named(&exec, &host);
        let open = Arc::new(Open {
            way: Way::Tls(relays),
            guest: Some(guest),
            ports,
            names,
            silent: AtomicBool::new(false),
        });
        if let Err(p) = answers(&open).await {
            open.close().await;
            return Err(p);
        }
        beat(Arc::downgrade(&open), BEAT);
        Ok(open)
    }

    /// The box, awake and answering: trusted first, then polled until it answers or [`BOX_PATIENCE`] is up.
    async fn box_up(&self, machine: &Machine, id: &str) -> Result<Arc<Open>> {
        self.trust(id).await?;
        let started = Instant::now();
        loop {
            match self.link(machine, id).await {
                Ok(open) => return Ok(open),
                Err(_) if started.elapsed() < BOX_PATIENCE => tokio::time::sleep(RETRY).await,
                Err(p) => return Err(p),
            }
        }
    }

    fn turn(&self, machine: &Machine) -> Arc<tokio::sync::Mutex<()>> {
        self.turns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(machine.dir.clone())
            .or_default()
            .clone()
    }

    fn link_of(&self, machine: &Machine) -> Option<Arc<Open>> {
        self.links
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&machine.dir)
            .cloned()
    }

    fn up(&self, machine: &Machine) -> Result<Arc<Open>> {
        self.link_of(machine)
            .filter(|o| o.alive())
            .ok_or(Problem::Asleep)
    }

    async fn drop_link(&self, machine: &Machine) {
        let found = self
            .links
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&machine.dir);
        if let Some(open) = found {
            open.close().await;
        }
    }

    /// Hangs up `open` if it's still this computer's link: a check that took its time can come back
    /// after a person reconnected.
    async fn drop_this(&self, machine: &Machine, open: &Arc<Open>) {
        {
            let mut links = self.links.lock().unwrap_or_else(|e| e.into_inner());
            if links
                .get(&machine.dir)
                .is_some_and(|held| Arc::ptr_eq(held, open))
            {
                links.remove(&machine.dir);
            }
        }
        open.close().await;
    }
}

/// A sandbox's id as E2B gave it, checked before it goes into a path or a name.
fn safe_id(id: &str) -> Result<&str> {
    if !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        Ok(id)
    } else {
        Err(Problem::Failed(
            "E2B named a sandbox in a way the broker won't use.".to_owned(),
        ))
    }
}

/// `<port>-<id>.e2b.app`: where a sandbox's port is.
fn name_of(port: u16, id: &str) -> String {
    format!("{port}-{id}.{DOMAIN}")
}

fn sandbox_id(machine: &Machine) -> Result<&str> {
    match &machine.reach {
        Reach::Sandbox { id } => safe_id(id),
        _ => Err(Problem::Failed(
            "this computer isn't an E2B sandbox".to_owned(),
        )),
    }
}

/// What E2B's status means to a person. The body is never shown: it may say more than we should.
fn outcome(status: u16) -> Problem {
    match status {
        404 => Problem::Gone,
        401 | 403 => Problem::Failed(
            "E2B refused this broker's key, or the team can't use sandboxes. Check E2B_API_KEY."
                .to_owned(),
        ),
        409 => Problem::Failed(
            "E2B is changing this sandbox's state. Try again in a moment.".to_owned(),
        ),
        429 => Problem::Failed(
            "E2B is limiting how often this broker asks. Try again in a minute.".to_owned(),
        ),
        other => Problem::Failed(format!("E2B answered {other}. Try again in a moment.")),
    }
}

#[async_trait]
impl Provider for E2bProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::E2b
    }

    async fn check(&self) -> Result<()> {
        self.key()?;
        self.template()?;
        Ok(())
    }

    /// Makes the sandbox from the template, stages the secrets, starts the box once to be sure it
    /// answers, and pauses it. Any failure on the way kills the sandbox.
    async fn create(&self, spec: &Spec) -> Result<Machine> {
        self.check().await?;
        let template = self.template()?.to_owned();
        let body = json!({
            "templateID": template,
            "timeout": SESSION_SECONDS,
            "autoPause": true,
            "autoPauseMemory": true,
            "network": {"allowPublicTraffic": true},
            "metadata": {"buwiz-computer": spec.id},
        });
        let reply = self
            .api(Method::POST, "/v2/sandboxes", Some(body), CALL_PATIENCE)
            .await?;
        if reply.status == 404 {
            return Err(Problem::Failed(format!("E2B has no template named {template}: build it first, and set {TEMPLATE_ENV} to its id.")));
        }
        if reply.status != 201 {
            return Err(outcome(reply.status));
        }
        let made = reply.json()?;
        let id = made
            .get("sandboxID")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let envd = made
            .get("envdAccessToken")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let (Some(id), Some(envd)) = (id, envd) else {
            return Err(Problem::Failed(
                "E2B's answer to the create had no sandbox to use.".to_owned(),
            ));
        };
        safe_id(&id)?;
        let machine = Machine {
            provider: ProviderKind::E2b,
            reach: Reach::Sandbox { id: id.clone() },
            image: Some(template),
            token: Secret::fresh(24)?,
            viewer_password: Secret::fresh(24)?,
            dir: spec.dir.clone(),
            link: None,
            seen: None,
        };
        let made = async {
            self.stage(&id, &envd, &machine).await?;
            let open = self.box_up(&machine, &id).await?;
            open.close().await;
            self.pause(&id).await
        }
        .await;
        if let Err(p) = made {
            let _ = self.kill(&id).await;
            return Err(p);
        }
        Ok(machine)
    }

    /// From the held link when there is one; otherwise from E2B. A running sandbox with no link
    /// here is asleep to this broker: a wake connects to it.
    async fn power(&self, machine: &Machine) -> Result<Power> {
        if self.link_of(machine).is_some_and(|o| o.alive()) {
            return Ok(Power::Awake);
        }
        Ok(match self.state(sandbox_id(machine)?).await? {
            None => Power::Absent,
            Some(state) if state == "running" || state == "paused" => Power::Asleep,
            Some(other) => Power::Broken(other),
        })
    }

    async fn wake(&self, machine: &Machine) -> Result<Woke> {
        let turn = self.turn(machine);
        let _turn = turn.lock().await;
        if self.link_of(machine).is_some_and(|o| o.alive()) {
            return Ok(Woke::Started);
        }
        self.drop_link(machine).await;
        let id = sandbox_id(machine)?.to_owned();
        let woke = self.resume(&id).await?;
        let open = self.box_up(machine, &id).await?;
        self.links
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(machine.dir.clone(), open);
        Ok(woke)
    }

    /// Pauses, at either depth: see the module doc for why a deep sleep keeps memory too.
    async fn sleep(&self, machine: &Machine, _: Sleep) -> Result<()> {
        let turn = self.turn(machine);
        let _turn = turn.lock().await;
        self.drop_link(machine).await;
        self.pause(sandbox_id(machine)?).await
    }

    /// Kills the sandbox. The record is the broker's to remove.
    async fn destroy(&self, machine: &Machine) -> Result<()> {
        self.drop_link(machine).await;
        self.kill(sandbox_id(machine)?).await?;
        self.turns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&machine.dir);
        Ok(())
    }

    async fn sweep(&self, id: &str, last: Option<&Reach>) -> Result<()> {
        let mut found = self.listed(safe_id(id)?).await?;
        if let Some(Reach::Sandbox { id: last }) = last {
            let last = safe_id(last)?.to_owned();
            if !found.contains(&last) {
                found.push(last);
            }
        }
        for sandbox in found {
            self.kill(&sandbox).await?;
        }
        Ok(())
    }

    /// Nothing: a box that has read its secrets can't be given new ones. Sweep finds it.
    async fn recover(&self, _: &Spec, _: Option<&Reach>) -> Result<Option<Machine>> {
        Ok(None)
    }

    /// That the link is still answering, for this request. One that isn't is dropped.
    async fn guard(&self, machine: &Machine) -> Result<()> {
        let open = self.up(machine)?;
        let Some(guest) = &open.guest else {
            return Err(Problem::Asleep);
        };
        if !guest.ready_within(CHECK_PATIENCE).await {
            self.drop_this(machine, &open).await;
            return Err(Problem::Failed(DROPPED.to_owned()));
        }
        Ok(())
    }

    async fn screen_up(&self, machine: &Machine, bot: &str) -> Result<Screen> {
        let open = self.up(machine)?;
        let Some(guest) = &open.guest else {
            return Err(Problem::Asleep);
        };
        clean(machine, guest.screen_up(bot).await)
    }

    async fn screen_down(&self, machine: &Machine, bot: &str) -> Result<()> {
        let open = self.up(machine)?;
        let Some(guest) = &open.guest else {
            return Err(Problem::Asleep);
        };
        clean(machine, guest.screen_down(bot).await)
    }

    async fn screens(&self, machine: &Machine) -> Result<Vec<String>> {
        let open = self.up(machine)?;
        let Some(guest) = &open.guest else {
            return Err(Problem::Asleep);
        };
        clean(machine, guest.screens().await)
    }

    async fn viewer(&self, machine: &Machine, screen: &Screen) -> Result<Option<Viewer>> {
        let open = self.up(machine)?;
        Ok(open.ports.get(&screen.viewer_port).map(|here| Viewer {
            addr: format!("127.0.0.1:{here}"),
            password: machine.viewer_password.clone(),
            far: Some(Far {
                name: open.names.get(&screen.viewer_port).cloned(),
            }),
        }))
    }

    async fn screenshot(&self, machine: &Machine, screen: &Screen) -> Result<Screenshot> {
        let open = self.up(machine)?;
        let Some(guest) = &open.guest else {
            return Err(Problem::Asleep);
        };
        clean(machine, guest.screenshot(screen).await)
    }

    async fn input(&self, machine: &Machine, screen: &Screen, input: &Input) -> Result<()> {
        let open = self.up(machine)?;
        let Some(guest) = &open.guest else {
            return Err(Problem::Asleep);
        };
        clean(machine, guest.input(screen, input).await)
    }

    async fn run(
        &self,
        machine: &Machine,
        screen: &Screen,
        command: &str,
        timeout_ms: u64,
    ) -> Result<CommandOutput> {
        let open = self.up(machine)?;
        let Some(guest) = &open.guest else {
            return Err(Problem::Asleep);
        };
        clean(machine, guest.run(screen, command, timeout_ms).await)
    }

    async fn open_url(&self, machine: &Machine, screen: &Screen, url: &str) -> Result<()> {
        let open = self.up(machine)?;
        let Some(guest) = &open.guest else {
            return Err(Problem::Asleep);
        };
        clean(machine, guest.open_url(screen, url).await)
    }

    async fn resolve(&self, machine: &Machine, path: &str) -> Result<String> {
        let open = self.up(machine)?;
        let Some(guest) = &open.guest else {
            return Err(Problem::Asleep);
        };
        clean(machine, guest.resolve(path).await)
    }

    async fn read_file(&self, machine: &Machine, path: &str) -> Result<Vec<u8>> {
        let open = self.up(machine)?;
        let Some(guest) = &open.guest else {
            return Err(Problem::Asleep);
        };
        clean(machine, guest.read_file(path).await)
    }

    async fn write_file(&self, machine: &Machine, path: &str, content: &[u8]) -> Result<()> {
        let open = self.up(machine)?;
        let Some(guest) = &open.guest else {
            return Err(Problem::Asleep);
        };
        clean(machine, guest.write_file(path, content).await)
    }

    async fn mark(&self, machine: &Machine, screen: &Screen) -> Option<u64> {
        let open = self.up(machine).ok()?;
        open.guest.as_ref()?.mark(screen).await
    }

    async fn settle(&self, machine: &Machine, screen: &Screen, wait: WaitFor) -> Result<Settled> {
        let open = self.up(machine)?;
        let Some(guest) = &open.guest else {
            return Err(Problem::Asleep);
        };
        clean(machine, guest.settle(screen, wait).await)
    }

    async fn record_start(&self, machine: &Machine, screen: &Screen) -> Result<u64> {
        let open = self.up(machine)?;
        let Some(guest) = &open.guest else {
            return Err(Problem::Asleep);
        };
        clean(machine, guest.record_start(screen).await)
    }

    async fn record_stop(&self, machine: &Machine, screen: &Screen, id: u64) -> Result<String> {
        let open = self.up(machine)?;
        let Some(guest) = &open.guest else {
            return Err(Problem::Asleep);
        };
        clean(machine, guest.record_stop(screen, id).await)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests;
