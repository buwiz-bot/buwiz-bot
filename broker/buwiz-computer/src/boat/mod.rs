//! boat.dev: one sandbox per computer. A sandbox is a persistent Ubuntu VM with Docker inside, and
//! the computer image runs in Docker in that VM. The box's container is made as Docker's and EC2's
//! are: root for box-keeper alone with only the capabilities a box needs, `no-new-privileges`, the
//! same limits and settings, a volume for `/workspace` and one for all of `/home/box`, and its ports
//! on the VM's loopback ([`prep`]).
//!
//! **What boat lacks next to Docker.** There's no holder container and no network wall: the box
//! has Docker's default bridge in the VM and reaches whatever the VM reaches, boat's network
//! included. Out of scope here, and not claimed.
//!
//! **Reach: SSH only, no public port.** The VM's ports are never published. The broker logs in over
//! SSH, the way [`crate::remote`] does with a plain host, and forwards the box's loopback ports
//! (`1337`, `1340`, `6901`–`6905`) to ports here, so [`BoxGuest`], the gate and the shell rules don't
//! change. Nothing of this computer is reachable from the internet, so there is no port auth to
//! trust. Boat's own public URLs (`host <port>`) are never used.
//!
//! **Trust.** The host key comes from boat's API, on a call that finishes a TLS handshake against the
//! public roots and `boat.dev`'s name ([`crate::cloud::Api`]). It's the sandbox's own key, returned
//! there, so the SSH login pins it: [`ssh::Master::open`] refuses any other key. Nothing is sent to
//! the machine before that: no secret, and no command. The SSH key the login uses is registered
//! with `POST /sandboxes/{id}/sshkey` first, so the only account that can log in is ours.
//!
//! **The SSH key is the token's own.** It's Ed25519, from a seed that is SHA-256 of a fixed label
//! and the computer's `token`. Nothing else is stored: the key is rebuilt from the record each time
//! it's needed, and the public half is the only part sent to boat. A key that's been seen by boat
//! reveals nothing about the token.
//!
//! **Secrets.** `Secret::fresh` for the token and the viewer password. They go to the VM on stdin of
//! the SSH command, never in argv or the environment, and land in `/dev/shm/buwiz-start` (RAM, not
//! on the disk boat saves), a folder only the container's root can read: owned by root, or by the
//! login's own user when the VM's Docker is rootless (its root is that user then). The container
//! mounts it read-only, as Docker's and EC2's do, so nothing in the box can plant or change a file
//! there, and box-keeper only reads them. The broker removes them over SSH once the box answers, or
//! when its start fails, and the VM itself does five minutes on, as on EC2 (`ec2/instance.rs`), so a
//! broker gone mid-start leaves nothing. Each start writes them again, because a stop drops the
//! memory that held what box-keeper had read.
//!
//! **Lifecycle, on boat's API** (`https://boat.dev/api/v1`, `Authorization: Bearer <boat_…>`):
//! - *create*: `POST /sandboxes` with `ttlSeconds` (one hour) and a `buwiz-computer` label, then wait
//!   for `state` `ready`. Then the SSH login, `docker pull` and `docker create` of the image, the
//!   secrets and `docker start`, the box checked, and `POST /sandboxes/{id}/stop`. A computer starts
//!   asleep. Any failure deletes the sandbox.
//! - *wake*: `POST /sandboxes/{id}/resume` if it is archived, then wait for `ready`, the SSH login,
//!   the secrets again and `docker start`, and the box checked. **A stop keeps the disk and drops
//!   memory, so wake is a cold boot of the box**: its processes and screens are gone, and its
//!   workspace and home (the browser profile in it) are the ones on disk. A computer made before
//!   the home volume keeps the container it was made with.
//! - *sleep*: `POST /sandboxes/{id}/stop`. Only Docker containers get a stop signal, with 30 s to
//!   exit. Both depths are the same, since boat keeps no memory at either.
//! - *destroy*: `DELETE /sandboxes/{id}` with `X-Ascii-Confirm-Delete: {id}`. Irreversible.
//! - *power*: the held link, else `GET /sandboxes/{id}`: `ready`/`idle`/`running` and
//!   `archived`/`archiving` are asleep to this broker (wake brings them up), the provisioning states
//!   are waiting, `error` and `cancelled` are broken.
//! - *sweep*: `GET /sandboxes?label=buwiz-computer%3D<id>` with its cursor, every one deleted, and
//!   the one the record names.
//! - *recover*: the same listing, else `GET /sandboxes/{id}` for the sandbox the record named,
//!   passing over any boat calls `error` or `cancelled`, and a new token and viewer password for the
//!   one taken. Nothing is stopped or deleted: the next start stages the new secrets, and the login
//!   registers the new token's key.
//!
//! **Where it runs: EU only.** boat's sandboxes are in Germany, Finland and France today, and the
//! provider's label says so. Asia isn't offered.
//!
//! **What's exposed.** Only the SSH daemon, on boat's public address, to the key registered above.
//! The VM's own ports answer to nothing outside it. Anyone with a shell on the VM is root there
//! (boat's `user` has passwordless sudo, and the staging uses it), so that shell is the owner's and
//! the broker's, not a Bot's: a Bot's commands run in the container.
//!
//! **Unverified against a live account.** That the sandbox's Docker is usable by `user` without
//! sudo (the script uses `docker` as is); that `/dev/shm` is tmpfs there; that boat's `ready` means
//! the VM is ready for the SSH login; how long the image pull takes; whether `ghcr.io/buwiz-bot/computer`
//! is pullable without a login; and boat's machine size for the box (`default`, which boat's docs
//! don't give in vCPU or RAM). The offline tests cover the API shapes and the refusals.
//!
//! **Config.** `BOAT_API_KEY` (a secret, read from the environment, never logged) and
//! `BUWIZ_BOAT_IMAGE` (default `ghcr.io/buwiz-bot/computer:latest`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine;
use bytes::Bytes;
use hyper::Method;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::cloud::{Api, Reply, Settings};
use crate::docker::BOX_CAPS;
use crate::Shared;
use crate::remote::ssh;
use crate::remote::{
    answers, beat, clean, free_ports, guest_at, guest_ports, Open, Way, BEAT, CHECK_PATIENCE,
};
use crate::{
    CommandOutput, Far, Input, Machine, Power, Problem, Provider, ProviderKind, Reach, Result,
    Screen, Screenshot, Secret, Settled, Sleep, Spec, Viewer, WaitFor, Woke,
};

pub const DEFAULT_IMAGE: &str = "ghcr.io/buwiz-bot/computer:latest";

const HOST: &str = "boat.dev";
const API: &str = "/api/v1";
/// The user the VM logs in as, and whose uid the box's secrets are owned by.
const SHELL_USER: &str = "user";
/// One hour, the default auto-stop, which stops the sandbox (disk kept) rather than deleting it.
const SESSION_SECONDS: u64 = 3600;
const CALL_PATIENCE: Duration = Duration::from_secs(30);
/// Waiting for a sandbox to be `ready`, polled every [`POLL`].
const READY_PATIENCE: Duration = Duration::from_secs(600);
const POLL: Duration = Duration::from_secs(3);
/// A box's answer after its start, polled every [`RETRY`].
const BOX_PATIENCE: Duration = Duration::from_secs(300);
const RETRY: Duration = Duration::from_secs(2);
/// The image pull and the container's creation, over the login.
const PREP_PATIENCE: Duration = Duration::from_secs(900);
const STAGE_PATIENCE: Duration = Duration::from_secs(60);
const LOGIN_TRIES: usize = 4;
const MAX_PAGES: usize = 10;
const DROPPED: &str = "The connection to this computer dropped. Click it to connect again.";
/// The label each sandbox carries, with its computer's id: what a sweep and a recover look for.
pub const LABEL: &str = "buwiz-computer";
/// Below this a port takes a capability, inside the box (`docker.rs`).
const FIRST_OPEN_PORT: u16 = 6906;

/// Makes the container and its volumes, as root of the VM's Docker, run once per computer after
/// its image is pulled. The box itself is set up as Docker and EC2 set theirs up (`docker.rs`
/// `create_args`, `ec2/instance.rs`): root for box-keeper alone with only [`BOX_CAPS`], which a box
/// can't start with fewer of, a `1g` `/dev/shm` for Chromium, the same five settings, one volume for
/// the workspace and one for all of `/home/box`. Everything else is the image's own default. Below
/// [`FIRST_OPEN_PORT`] a port takes a capability, so no command can listen on a service's port. Its
/// ports are on the VM's loopback alone. The secrets' folder is the container root's alone
/// ([`STAGE`]), and mounted read-only, as Docker's is: [`CLEAR`] removes the two files.
///
/// A computer made before this layout keeps its container as it was (a `-chrome` volume, the old
/// settings, and no capabilities, which today's image may not start with): wake only starts what
/// create made, and nothing here deletes a person's container or volumes. Only new computers get
/// this one. Arguments: image, container name, computer id.
fn prep() -> String {
    let caps: String = BOX_CAPS.iter().map(|c| format!(" --cap-add {c}")).collect();
    format!(
        r#"set -e
o=0; g=0
if docker info --format '{{{{.SecurityOptions}}}}' 2>/dev/null | grep -q rootless; then o=$(id -u); g=$(id -g); fi
sudo install -d -m 0700 -o "$o" -g "$g" /dev/shm/buwiz-start
docker pull "$1" >/dev/null
docker create --name "$2" --label app.buwiz.computer=1 --label "app.buwiz.computer.id=$3" \
  --user 0 --cap-drop ALL{caps} --security-opt no-new-privileges:true --pids-limit 2048 --memory 4g --shm-size 1g \
  --sysctl net.ipv6.conf.all.disable_ipv6=1 --sysctl net.ipv4.ip_unprivileged_port_start={FIRST_OPEN_PORT} \
  -e BOX_TOKEN_FILE=/run/buwiz-computer/token -e BOX_VIEWER_PASSWORD_FILE=/run/buwiz-computer/viewer \
  -e BOX_DESKTOP=1 -e BOX_DESKTOP_REQUIRED=1 -e BOX_CHROME=0 \
  -v "$2-workspace:/workspace" -v "$2-home:/home/box" -v /dev/shm/buwiz-start:/run/buwiz-computer:ro \
  -p 127.0.0.1:1337:1337 -p 127.0.0.1:1340:1340 -p 127.0.0.1:6901-6905:6901-6905 "$1" >/dev/null"#
    )
}

/// Writes the two secrets from stdin (token, then viewer password, one per line) into the staging
/// folder, the container root's alone (root's, or the login's user's under a rootless Docker, whose
/// root that is), and starts the container. A failed start removes them, and so does the VM five
/// minutes on whatever happens; [`CLEAR`] does once the box answers. Argument: the container name.
const STAGE: &str = r#"# buwiz:stage
set -e
d=/dev/shm/buwiz-start
o=0; g=0
if docker info --format '{{.SecurityOptions}}' 2>/dev/null | grep -q rootless; then o=$(id -u); g=$(id -g); fi
sudo install -d -m 0700 -o "$o" -g "$g" "$d"
sudo sh -c 'umask 077 && read -r t && read -r v && rm -f -- "$1/token" "$1/viewer" && printf "%s" "$t" > "$1/token" && printf "%s" "$v" > "$1/viewer" && chown "$2:$3" "$1/token" "$1/viewer"' sh "$d" "$o" "$g"
sudo setsid -f /bin/sh -c 'sleep 300; rm -f -- "$0/token" "$0/viewer"' "$d" </dev/null >/dev/null 2>&1
docker start "$1" >/dev/null || { sudo rm -f -- "$d/token" "$d/viewer"; exit 1; }"#;

/// The staged secrets, gone: box-keeper has read them, or the start failed. Safe to repeat.
const CLEAR: &str = r#"# buwiz:clear
sudo rm -f -- /dev/shm/buwiz-start/token /dev/shm/buwiz-start/viewer"#;

/// [`CLEAR`] over `master`. A clear that fails leaves them to the VM's own, five minutes on.
async fn clear(master: &ssh::Master) {
    let _ = master.run(CLEAR, &[], None, STAGE_PATIENCE, 4096).await;
}

fn wire(field: &[u8]) -> Vec<u8> {
    let mut out = (field.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(field);
    out
}

/// The SSH key for a computer: Ed25519, from SHA-256 of a label and `token`. Returns the private key
/// in OpenSSH's format, and the public line that goes to boat.
pub fn ssh_key(token: &Secret) -> Result<(Secret, String)> {
    let mut hasher = Sha256::new();
    hasher.update(b"buwiz-boat-ssh-v1\0");
    hasher.update(token.expose().as_bytes());
    let seed: [u8; 32] = hasher.finalize().into();
    let pair = Ed25519KeyPair::from_seed_unchecked(&seed)
        .map_err(|_| Problem::Failed("this computer's SSH key couldn't be made".to_owned()))?;
    let public = pair.public_key().as_ref().to_vec();
    let mut public_blob = wire(b"ssh-ed25519");
    public_blob.extend(wire(&public));
    let line = format!(
        "ssh-ed25519 {}",
        base64::engine::general_purpose::STANDARD.encode(&public_blob)
    );

    // the private section: two equal check words, the key, a comment, then padding to 8 bytes
    let mut section = [0x42u8; 8].to_vec();
    section.extend(wire(b"ssh-ed25519"));
    section.extend(wire(&public));
    section.extend(wire(&[seed.as_slice(), public.as_slice()].concat()));
    section.extend(wire(b""));
    let mut pad = 1u8;
    while section.len() % 8 != 0 {
        section.push(pad);
        pad += 1;
    }
    let mut body = b"openssh-key-v1\0".to_vec();
    for field in [b"none".as_slice(), b"none".as_slice(), b"".as_slice()] {
        body.extend(wire(field));
    }
    body.extend(1u32.to_be_bytes());
    body.extend(wire(&public_blob));
    body.extend(wire(&section));
    let encoded = base64::engine::general_purpose::STANDARD.encode(&body);
    let wrapped: Vec<&str> = encoded
        .as_bytes()
        .chunks(70)
        .filter_map(|c| std::str::from_utf8(c).ok())
        .collect();
    let text = format!(
        "-----BEGIN OPENSSH PRIVATE KEY-----\n{}\n-----END OPENSSH PRIVATE KEY-----\n",
        wrapped.join("\n")
    );
    Ok((Secret::new(text), line))
}

/// A sandbox's id as boat gave it, checked before it goes into a path, a label or a name.
fn safe_id(id: &str) -> Result<&str> {
    if !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        Ok(id)
    } else {
        Err(Problem::Failed(
            "boat named a sandbox in a way the broker won't use.".to_owned(),
        ))
    }
}

/// What boat's status means to a person. The message is never shown: it may say more than we should.
fn outcome(reply: &Reply) -> Problem {
    let code = reply
        .json()
        .ok()
        .and_then(|v| v.get("code").and_then(Value::as_str).map(str::to_owned))
        .filter(|c| c.len() <= 64 && c.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'));
    match (reply.status, code.as_deref()) {
        (404, _) => Problem::Gone,
        (401, _) => {
            Problem::Failed("boat refused this broker's key. Check the API key.".to_owned())
        }
        (402, _) => {
            Problem::Failed("boat says this account can't operate sandboxes right now.".to_owned())
        }
        (403, _) => Problem::Failed("boat won't let this key do that.".to_owned()),
        (409, Some("sandbox_stopping")) => Problem::Failed(
            "boat is still stopping this computer. Try again in a minute.".to_owned(),
        ),
        (409, _) => Problem::Failed(
            "boat is changing this sandbox's state. Try again in a moment.".to_owned(),
        ),
        (429, _) => Problem::Failed(
            "boat is limiting how often this broker asks. Try again in a minute.".to_owned(),
        ),
        (503, _) => Problem::Failed(
            "boat has no machine free for this computer right now. Try again later.".to_owned(),
        ),
        (400, Some(code)) => Problem::Failed(format!("boat refused the request ({code}).")),
        (other, _) => Problem::Failed(format!("boat answered {other}. Try again in a moment.")),
    }
}

/// One login's worth of forwards and the SSH master that holds them.
struct Login {
    master: ssh::Master,
    ports: HashMap<u16, u16>,
}

/// boat.dev's sandboxes, one per computer, and their logins. The hub keeps one, because it holds
/// the connections.
pub struct BoatProvider {
    /// the key and the image, read at each use: a person can change them
    settings: Shared<Settings>,
    api: Api,
    ssh: PathBuf,
    links: Mutex<HashMap<PathBuf, Arc<Open>>>,
    turns: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
}

impl BoatProvider {
    /// The provider with its settings (see [`crate::cloud::settings`]). Nothing is read from the
    /// environment here. An unset image is the default computer image.
    pub fn new(settings: Settings) -> Self {
        Self::shared(Shared::new(settings))
    }

    /// The provider reading its settings from `settings`, which the broker changes when a person
    /// saves new ones.
    pub fn shared(settings: Shared<Settings>) -> Self {
        Self {
            settings,
            api: Api::new(),
            ssh: PathBuf::from(ssh::SSH),
            links: Mutex::default(),
            turns: Mutex::default(),
        }
    }

    /// Sends the platform's name to a test's fake, and trusts its root (tests).
    pub fn with_api(mut self, api: Api) -> Self {
        self.api = api;
        self
    }

    /// The `ssh` that stands in for the system's, for tests.
    pub fn with_ssh(mut self, ssh: PathBuf) -> Self {
        self.ssh = ssh;
        self
    }

    fn key(&self) -> Result<Secret> {
        self.settings.get().key.ok_or_else(|| Problem::Refused { code: "no_key", message: "boat needs an API key before it can make a computer. Add it in Settings › Computers.".to_owned() })
    }

    /// What its computers are made from: the default computer image when none is set.
    fn image(&self) -> String {
        self.settings.get().image.unwrap_or_else(|| DEFAULT_IMAGE.to_owned())
    }

    /// One call to boat's API with the bearer and any extra headers. The key goes only after the
    /// handshake that checks `boat.dev`'s name (see [`crate::cloud`]).
    async fn call(
        &self,
        method: Method,
        path: &str,
        extra: &[(&str, &str)],
        body: Option<Value>,
        patience: Duration,
    ) -> Result<Reply> {
        let bearer = format!("Bearer {}", self.key()?.expose());
        let mut headers = vec![("Authorization", bearer.as_str())];
        headers.extend_from_slice(extra);
        let body = body.map(|b| Bytes::from(b.to_string()));
        if body.is_some() {
            headers.push(("Content-Type", "application/json"));
        }
        self.api
            .call(
                HOST,
                method,
                &format!("{API}{path}"),
                &headers,
                body,
                patience,
            )
            .await
    }

    /// The sandbox object for `id`, or `None` when boat has no such sandbox.
    async fn sandbox(&self, id: &str) -> Result<Option<Value>> {
        let reply = self
            .call(
                Method::GET,
                &format!("/sandboxes/{id}"),
                &[],
                None,
                CALL_PATIENCE,
            )
            .await?;
        match reply.status {
            200 => Ok(reply.json()?.get("sandbox").cloned()),
            404 => Ok(None),
            _ => Err(outcome(&reply)),
        }
    }

    async fn state(&self, id: &str) -> Result<Option<String>> {
        Ok(self
            .sandbox(id)
            .await?
            .and_then(|s| s.get("state").and_then(Value::as_str).map(str::to_owned)))
    }

    /// Polls until the sandbox is `ready`, `idle` or `running`. `error` and `cancelled` are final.
    async fn wait_ready(&self, id: &str) -> Result<()> {
        let started = Instant::now();
        loop {
            match self.state(id).await?.as_deref() {
                Some("ready" | "idle" | "running") => return Ok(()),
                Some("error" | "cancelled") => {
                    return Err(Problem::Failed(
                        "boat couldn't start this computer's sandbox.".to_owned(),
                    ))
                }
                None => return Err(Problem::Gone),
                _ if started.elapsed() >= READY_PATIENCE => {
                    return Err(Problem::Failed(
                        "boat's sandbox took too long to start.".to_owned(),
                    ))
                }
                _ => tokio::time::sleep(POLL).await,
            }
        }
    }

    async fn resume(&self, id: &str) -> Result<()> {
        let reply = self
            .call(
                Method::POST,
                &format!("/sandboxes/{id}/resume"),
                &[],
                Some(json!({"ttlSeconds": SESSION_SECONDS})),
                CALL_PATIENCE,
            )
            .await?;
        match reply.status {
            200 | 202 => Ok(()),
            _ => Err(outcome(&reply)),
        }
    }

    async fn stop(&self, id: &str) -> Result<()> {
        let reply = self
            .call(
                Method::POST,
                &format!("/sandboxes/{id}/stop"),
                &[],
                Some(json!({})),
                CALL_PATIENCE,
            )
            .await?;
        match reply.status {
            200 | 202 => Ok(()),
            _ => Err(outcome(&reply)),
        }
    }

    /// Deletes `id` for good. Gone already is fine.
    async fn delete(&self, id: &str) -> Result<()> {
        let reply = self
            .call(
                Method::DELETE,
                &format!("/sandboxes/{id}"),
                &[("X-Ascii-Confirm-Delete", id)],
                None,
                CALL_PATIENCE,
            )
            .await?;
        match reply.status {
            200 | 202 | 404 => Ok(()),
            _ => Err(outcome(&reply)),
        }
    }

    /// Every sandbox labelled with computer `id`, across boat's pages, with its state when the
    /// listing says it. More than [`MAX_PAGES`] is an error, never a part of the list: a recover or
    /// a sweep that acted on a part would miss what's past it.
    async fn labelled(&self, id: &str) -> Result<Vec<(String, Option<String>)>> {
        let computer = safe_id(id)?;
        let mut found = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut path = format!("/sandboxes?label={LABEL}%3D{computer}&limit=100");
            if let Some(next) = &cursor {
                path.push_str(&format!("&cursor={next}"));
            }
            let reply = self
                .call(Method::GET, &path, &[], None, CALL_PATIENCE)
                .await?;
            if reply.status != 200 {
                return Err(outcome(&reply));
            }
            let page = reply.json()?;
            for item in page
                .get("sandboxes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(sandbox) = item.get("id").and_then(Value::as_str) {
                    let sandbox = safe_id(sandbox)?.to_owned();
                    let state = item.get("state").and_then(Value::as_str).map(str::to_owned);
                    if !found.iter().any(|(s, _)| *s == sandbox) {
                        found.push((sandbox, state));
                    }
                }
            }
            match page.pointer("/pageInfo/nextCursor").and_then(Value::as_str) {
                Some(next)
                    if next
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_=.".contains(&b)) =>
                {
                    cursor = Some(next.to_owned())
                }
                Some(_) => {
                    return Err(Problem::Failed(
                        "boat's listing came with a cursor the broker won't use.".to_owned(),
                    ))
                }
                None => return Ok(found),
            }
        }
        Err(Problem::Failed(format!(
            "boat listed more than {MAX_PAGES} pages of sandboxes for this computer, so nothing was done with them. Try again later."
        )))
    }

    /// Registers `public` as a login key for `id` and returns where and who the machine is now:
    /// its host key, and where its SSH answers. Nothing is logged in yet.
    async fn register(&self, id: &str, public: &str) -> Result<(String, ssh::At)> {
        let reply = self
            .call(
                Method::POST,
                &format!("/sandboxes/{id}/sshkey"),
                &[],
                Some(json!({"key": public})),
                CALL_PATIENCE,
            )
            .await?;
        if reply.status != 200 {
            return Err(outcome(&reply));
        }
        let answer = reply.json()?;
        let host_key = answer
            .get("hostKey")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                Problem::Failed(
                    "boat's answer had no host key to check the login against.".to_owned(),
                )
            })?;
        let (host, port) = match answer.get("sshEndpoint").and_then(Value::as_str) {
            Some(endpoint) => {
                let (host, port) = endpoint.rsplit_once(':').ok_or_else(|| {
                    Problem::Failed("boat's SSH address isn't host:port.".to_owned())
                })?;
                (
                    host.to_owned(),
                    Some(port.parse::<u16>().map_err(|_| {
                        Problem::Failed("boat's SSH port isn't a port.".to_owned())
                    })?),
                )
            }
            None => {
                let ip = answer
                    .get("machineIp")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        Problem::Failed("boat's answer had no address for the login.".to_owned())
                    })?;
                (ip.to_owned(), None)
            }
        };
        let user = answer
            .get("sshUser")
            .and_then(Value::as_str)
            .unwrap_or(SHELL_USER)
            .to_owned();
        Ok((
            host_key,
            ssh::At {
                host,
                user: Some(user),
                port,
            },
        ))
    }

    /// Logs in over SSH as the registered key, pinned to the host key boat gave, and forwards the
    /// box's ports. Refused before any login if the host key isn't one the login can pin.
    async fn login(&self, machine: &Machine, id: &str) -> Result<Login> {
        let (key, public) = ssh_key(&machine.token)?;
        let (host_key, at) = self.register(id, &public).await?;
        ssh::check_pin(&host_key)?;
        let mut tries = 0;
        loop {
            let ports = free_ports(guest_ports().count())?;
            let forwards: Vec<(u16, u16)> = ports.iter().copied().zip(guest_ports()).collect();
            match ssh::Master::open(
                &self.ssh,
                &machine.dir,
                &at,
                &host_key,
                Some(&key),
                &forwards,
            )
            .await
            {
                Ok(master) => {
                    return Ok(Login {
                        master,
                        ports: forwards
                            .into_iter()
                            .map(|(here, there)| (there, here))
                            .collect(),
                    })
                }
                Err(ssh::Failed::PortTaken) if tries < LOGIN_TRIES => tries += 1,
                Err(ssh::Failed::PortTaken) => {
                    return Err(Problem::Remote(
                        "This Mac has no free ports left for a connection.".to_owned(),
                    ))
                }
                Err(ssh::Failed::Said(p)) => return Err(p),
            }
        }
    }

    /// Runs a setup step on the VM and fails with a sentence if it exits non-zero. What it printed
    /// is kept out of the sentence.
    async fn step(
        login: &Login,
        script: &str,
        args: &[&str],
        stdin: Option<&[u8]>,
        patience: Duration,
        what: &str,
    ) -> Result<()> {
        let ran = login
            .master
            .run(script, args, stdin, patience, 4096)
            .await?;
        if ran.code == Some(0) {
            Ok(())
        } else {
            Err(Problem::Failed(format!(
                "the sandbox couldn't {what} (exit {:?}).",
                ran.code
            )))
        }
    }

    /// The secrets to the VM, and the container started. The secrets are on stdin only. A failed
    /// start leaves none of them staged.
    async fn start_box(machine: &Machine, login: &Login, name: &str) -> Result<()> {
        let stdin = format!(
            "{}\n{}\n",
            machine.token.expose(),
            machine.viewer_password.expose()
        );
        let started = Self::step(
            login,
            STAGE,
            &[name],
            Some(stdin.as_bytes()),
            STAGE_PATIENCE,
            "start the computer",
        )
        .await;
        if started.is_err() {
            clear(&login.master).await;
        }
        started
    }

    /// The box, as a link: forwards, the box's answer checked, polled until it answers or [`BOX_PATIENCE`].
    async fn finish(&self, machine: &Machine, login: Login) -> Result<Arc<Open>> {
        let guest = guest_at(&login.ports, machine.token.clone())?;
        let open = Arc::new(Open {
            way: Way::Ssh(Box::new(login.master)),
            guest: Some(guest),
            ports: login.ports,
            names: HashMap::new(),
            silent: AtomicBool::new(false),
        });
        let started = Instant::now();
        loop {
            match answers(&open).await {
                Ok(()) => break,
                Err(_) if started.elapsed() < BOX_PATIENCE => tokio::time::sleep(RETRY).await,
                Err(p) => {
                    if let Ok(master) = open.master() {
                        clear(master).await;
                    }
                    open.close().await;
                    return Err(p);
                }
            }
        }
        // box-keeper read them to answer, and can't unlink them on its read-only mount
        if let Ok(master) = open.master() {
            clear(master).await;
        }
        beat(Arc::downgrade(&open), BEAT);
        Ok(open)
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

    /// Hangs up `open` if it's still this computer's link.
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

fn sandbox_id(machine: &Machine) -> Result<&str> {
    match &machine.reach {
        Reach::Sandbox { id } => safe_id(id),
        _ => Err(Problem::Failed(
            "this computer isn't a boat sandbox".to_owned(),
        )),
    }
}

/// What a boat state means to this broker when no link is held: boat's word, grouped.
fn power_of(state: &str) -> Power {
    match state {
        "ready" | "idle" | "running" | "archived" | "archiving" => Power::Asleep,
        "init" | "provisioning" | "provisioned" | "cloning" => Power::Waking,
        other => Power::Broken(other.to_owned()),
    }
}

/// Whether a sandbox in boat's `state` could be woken and used: anything [`power_of`] doesn't call
/// broken.
fn usable(state: &str) -> bool {
    !matches!(power_of(state), Power::Broken(_))
}

#[async_trait]
impl Provider for BoatProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Boat
    }

    async fn check(&self) -> Result<()> {
        self.key()?;
        Ok(())
    }

    /// The key works: boat lists this account's sandboxes for it.
    async fn verify(&self) -> Result<()> {
        self.check().await?;
        let reply = self.call(Method::GET, "/sandboxes", &[], None, CALL_PATIENCE).await?;
        match reply.status {
            200 => Ok(()),
            401 | 403 => Err(Problem::Refused { code: "bad_key", message: "boat didn't take this API key. Copy it again from boat.dev, then check again.".to_owned() }),
            _ => Err(outcome(&reply)),
        }
    }

    /// Makes the sandbox, sets the box up in its Docker, checks the box once, and stops it. Any
    /// failure on the way deletes the sandbox.
    async fn create(&self, spec: &Spec) -> Result<Machine> {
        self.check().await?;
        let id = safe_id(&spec.id)?.to_owned();
        let body = json!({"ttlSeconds": SESSION_SECONDS, "labels": {"buwiz-computer": id}});
        let reply = self
            .call(Method::POST, "/sandboxes", &[], Some(body), CALL_PATIENCE)
            .await?;
        if !matches!(reply.status, 200..=202) {
            return Err(outcome(&reply));
        }
        let made = reply.json()?;
        let sandbox = made
            .get("sandbox")
            .and_then(|s| s.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let Some(sandbox) = sandbox else {
            return Err(Problem::Failed(
                "boat's answer to the create had no sandbox to use.".to_owned(),
            ));
        };
        safe_id(&sandbox)?;
        let image = self.image();
        let machine = Machine {
            provider: ProviderKind::Boat,
            reach: Reach::Sandbox {
                id: sandbox.clone(),
            },
            image: Some(image.clone()),
            token: Secret::fresh(24)?,
            viewer_password: Secret::fresh(24)?,
            dir: spec.dir.clone(),
            link: None,
            seen: None,
        };
        let name = container_name(&sandbox);
        let set_up = async {
            self.wait_ready(&sandbox).await?;
            let login = self.login(&machine, &sandbox).await?;
            Self::step(
                &login,
                &prep(),
                &[image.as_str(), name.as_str(), spec.id.as_str()],
                None,
                PREP_PATIENCE,
                "make the computer",
            )
            .await?;
            Self::start_box(&machine, &login, &name).await?;
            let open = self.finish(&machine, login).await?;
            open.close().await;
            self.stop(&sandbox).await
        }
        .await;
        if let Err(p) = set_up {
            let _ = self.delete(&sandbox).await;
            return Err(p);
        }
        Ok(machine)
    }

    /// From the held link when there is one; otherwise from boat's word for the sandbox.
    async fn power(&self, machine: &Machine) -> Result<Power> {
        if self.link_of(machine).is_some_and(|o| o.alive()) {
            return Ok(Power::Awake);
        }
        Ok(match self.state(sandbox_id(machine)?).await? {
            None => Power::Absent,
            Some(state) => power_of(&state),
        })
    }

    /// A stopped sandbox is resumed, a running one is used as it is; then its box is started again.
    /// A cold boot: a stop kept the disk and dropped the memory.
    async fn wake(&self, machine: &Machine) -> Result<Woke> {
        let turn = self.turn(machine);
        let _turn = turn.lock().await;
        if self.link_of(machine).is_some_and(|o| o.alive()) {
            return Ok(Woke::Started);
        }
        self.drop_link(machine).await;
        let id = sandbox_id(machine)?.to_owned();
        match self.state(&id).await?.as_deref() {
            Some("archived" | "archiving") => self.resume(&id).await?,
            Some(_) => {}
            None => return Err(Problem::Gone),
        }
        self.wait_ready(&id).await?;
        let login = self.login(machine, &id).await?;
        Self::start_box(machine, &login, &container_name(&id)).await?;
        let open = self.finish(machine, login).await?;
        self.links
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(machine.dir.clone(), open);
        Ok(Woke::Started)
    }

    /// Stops the sandbox. Both depths are the same: boat keeps no memory either way.
    async fn sleep(&self, machine: &Machine, _: Sleep) -> Result<()> {
        let turn = self.turn(machine);
        let _turn = turn.lock().await;
        self.drop_link(machine).await;
        self.stop(sandbox_id(machine)?).await
    }

    async fn destroy(&self, machine: &Machine) -> Result<()> {
        self.drop_link(machine).await;
        self.delete(sandbox_id(machine)?).await?;
        self.turns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&machine.dir);
        Ok(())
    }

    /// Deletes every sandbox labelled with the computer, and the one the record names.
    async fn sweep(&self, id: &str, last: Option<&Reach>) -> Result<()> {
        let mut found: Vec<String> = self.labelled(id).await?.into_iter().map(|(s, _)| s).collect();
        if let Some(Reach::Sandbox { id: last }) = last {
            let last = safe_id(last)?.to_owned();
            if !found.contains(&last) {
                found.push(last);
            }
        }
        for sandbox in found {
            self.delete(&sandbox).await?;
        }
        Ok(())
    }

    /// Any sandbox labelled with the computer, or the one the record named, as sweep finds them.
    async fn finds(&self, id: &str, last: Option<&Reach>) -> Result<Option<bool>> {
        if !self.labelled(id).await?.is_empty() {
            return Ok(Some(true));
        }
        match last {
            Some(Reach::Sandbox { id: last }) => {
                Ok(Some(self.sandbox(safe_id(last)?).await?.is_some()))
            }
            _ => Ok(Some(false)),
        }
    }

    /// The computer's sandbox, found by its label, else the one the record named if boat still has
    /// it, with new secrets. Its disk kept the workspace and the home, and the secrets are made
    /// again, not kept: each start stages them anew and the SSH key is rebuilt from the new token
    /// and registered at the login. Nothing is stopped or deleted here. Only a usable sandbox is
    /// taken: one boat calls broken (`error`, `cancelled`, as [`Provider::power`] reads them) is
    /// passed over. Of several usable ones, the one the record named, else the first by id; the
    /// others are left as they are. `None` when boat has nothing usable for this computer.
    async fn recover(&self, spec: &Spec, last: Option<&Reach>) -> Result<Option<Machine>> {
        self.check().await?;
        let mut found = self.labelled(&spec.id).await?;
        found.sort_unstable();
        let last = match last {
            Some(Reach::Sandbox { id }) => Some(safe_id(id)?.to_owned()),
            _ => None,
        };
        // the one the record named first, then the rest by id; the named one even when it isn't
        // labelled, if boat still has it
        if let Some(named) = &last {
            match found.iter().position(|(s, _)| s == named) {
                Some(at) => {
                    let named = found.remove(at);
                    found.insert(0, named);
                }
                None => found.push((named.clone(), None)),
            }
        }
        let mut sandbox = None;
        for (id, listed) in found {
            let state = match listed {
                Some(state) => Some(state),
                None => self.state(&id).await?,
            };
            if state.is_some_and(|s| usable(&s)) {
                sandbox = Some(id);
                break;
            }
        }
        let Some(sandbox) = sandbox else { return Ok(None) };
        Ok(Some(Machine {
            provider: ProviderKind::Boat,
            reach: Reach::Sandbox { id: sandbox },
            image: Some(self.image()),
            token: Secret::fresh(24)?,
            viewer_password: Secret::fresh(24)?,
            dir: spec.dir.clone(),
            link: None,
            seen: None,
        }))
    }

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
            far: Some(Far { name: None }),
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

/// The container's name in the VM: from the sandbox's id, which the record keeps, so create and wake
/// name it the same.
fn container_name(sandbox: &str) -> String {
    format!("buwiz-{sandbox}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests;
