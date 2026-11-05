//! The viewer gate: how a live desktop reaches the Bot's web view with the password left out.
//!
//! The app asks the broker to open a computer, and gets a URL on this gate holding a one-time
//! ticket. That URL is all it ever has, so what the tree, the logs and the `web_view` prop can
//! show is a ticket that's already spent. The first request swaps it for a cookie and a clean
//! URL. Every later request is passed to the viewer with the password put in here, and
//! the Host and Origin it expects (voxl refuses others): a page, its assets and `/ws`.
//!
//! The gate is a plain byte pipe once the head is rewritten, so a WebSocket needs nothing more.
//! One connection per request: a head is only ever read once. Its port is in the range the Bot's
//! shell is kept off; a computer's own traffic never comes here.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

use crate::{random_hex, Problem, Result, Viewer, DOCKER_PORT_LAST, PORT_LAST};

/// How long a ticket waits for the web view.
const TICKET_LIFE: Duration = Duration::from_secs(60);
const COOKIE: &str = "buwiz_viewer";
const MAX_HEAD: usize = 32 * 1024;
const HEAD_PATIENCE: Duration = Duration::from_secs(15);
/// How long the gate waits after an accept failed.
const ACCEPT_PAUSE: Duration = Duration::from_millis(100);

struct Session {
    bot: String,
    viewer: Viewer,
    ticket: Option<(String, Instant)>,
    cookie: String,
    /// dropped with the session, which ends every relay it has running
    open: watch::Sender<()>,
}

#[derive(Default)]
struct Sessions {
    open: HashMap<String, Session>,
    /// per Bot: the viewers connected right now (their `/ws`)
    watching: HashMap<String, usize>,
}

/// One per broker, listening on loopback.
pub struct Gate {
    port: u16,
    sessions: Arc<Mutex<Sessions>>,
}

impl Gate {
    /// Binds the first free port of the gate's part of the range and starts serving.
    pub async fn start() -> Result<Self> {
        for port in DOCKER_PORT_LAST + 1..=PORT_LAST {
            if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)).await {
                let sessions = Arc::new(Mutex::new(Sessions::default()));
                tokio::spawn(serve(listener, sessions.clone()));
                return Ok(Self { port, sessions });
            }
        }
        Err(Problem::Failed("no free port for the viewer".to_owned()))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Sessions> {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A URL that opens `viewer` once. An older session of `bot` is closed.
    pub fn open(&self, bot: &str, viewer: Viewer) -> Result<String> {
        let (id, ticket, cookie) = (random_hex(16)?, random_hex(16)?, random_hex(24)?);
        let mut s = self.lock();
        s.open.retain(|_, o| o.bot != bot);
        s.open.insert(id.clone(), Session { bot: bot.to_owned(), viewer, ticket: Some((ticket.clone(), Instant::now() + TICKET_LIFE)), cookie, open: watch::channel(()).0 });
        Ok(format!("http://127.0.0.1:{}/s/{id}/?ticket={ticket}", self.port))
    }

    /// The computer went to sleep or away: its session ends.
    pub fn close(&self, bot: &str) {
        self.lock().open.retain(|_, o| o.bot != bot);
    }

    /// Whether a person is watching `bot`'s computer: it doesn't sleep under them.
    pub fn watched(&self, bot: &str) -> bool {
        self.lock().watching.get(bot).is_some_and(|n| *n > 0)
    }
}

async fn serve(listener: TcpListener, sessions: Arc<Mutex<Sessions>>) {
    keep_accepting(|| listener.accept(), sessions, ACCEPT_PAUSE).await;
}

/// Serves what `accept` brings, for good: one bad accept, such as running out of file
/// descriptors, doesn't end every viewer to come.
async fn keep_accepting<F, Fut>(mut accept: F, sessions: Arc<Mutex<Sessions>>, pause: Duration)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::io::Result<(TcpStream, std::net::SocketAddr)>>,
{
    loop {
        match accept().await {
            Ok((socket, _)) => {
                let sessions = sessions.clone();
                tokio::spawn(async move {
                    let _ = connection(socket, sessions).await;
                });
            }
            Err(e) => {
                eprintln!("buwiz-computer: the viewer gate couldn't accept: {e}");
                tokio::time::sleep(pause).await;
            }
        }
    }
}

/// What a request gets, once its head is read.
enum Verdict {
    /// refused, with its status line
    No(&'static str),
    /// the ticket is good: to the clean URL, with the cookie
    Enter { location: String, cookie: String },
    Pass { bot: String, viewer: Viewer, path: String, open: watch::Receiver<()> },
}

/// `target` (`/s/<id>/rest?query`) with `cookies` against the open sessions.
fn judge(sessions: &mut Sessions, target: &str, cookies: Option<&str>) -> Verdict {
    let Some(rest) = target.strip_prefix("/s/") else { return Verdict::No("404 Not Found") };
    let Some((id, tail)) = rest.split_once('/') else { return Verdict::No("404 Not Found") };
    let Some(session) = sessions.open.get_mut(id) else { return Verdict::No("403 Forbidden") };
    let (path, query) = tail.split_once('?').unwrap_or((tail, ""));
    if let Some(given) = query.split('&').find_map(|kv| kv.strip_prefix("ticket=")) {
        return match session.ticket.take() {
            Some((ticket, until)) if ticket == given && Instant::now() < until => Verdict::Enter { location: format!("/s/{id}/{path}"), cookie: format!("{COOKIE}={}; HttpOnly; SameSite=Strict; Path=/s/{id}/", session.cookie) },
            _ => Verdict::No("403 Forbidden"),
        };
    }
    let wanted = format!("{COOKIE}={}", session.cookie);
    if !cookies.unwrap_or_default().split(';').any(|c| c.trim() == wanted) {
        return Verdict::No("403 Forbidden");
    }
    Verdict::Pass { bot: session.bot.clone(), viewer: session.viewer.clone(), path: format!("/{tail}"), open: session.open.subscribe() }
}

/// The request's head for the viewer: its own path, Host, Origin and password, and none of the
/// app's cookie.
fn rewrite(lines: &[&str], method: &str, path: &str, viewer: &Viewer) -> (String, bool) {
    let header = |line: &&str| line.split_once(':').map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()));
    let upgrade = lines.iter().filter_map(header).any(|(k, v)| k == "upgrade" && !v.is_empty());
    let mut out = format!("{method} {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\n", viewer.addr, viewer.password.expose());
    for line in lines {
        let Some((name, _)) = header(line) else { continue };
        match name.as_str() {
            "host" | "authorization" | "cookie" => {}
            "origin" => out.push_str(&format!("Origin: http://{}\r\n", viewer.addr)),
            "connection" if !upgrade => {}
            _ => {
                out.push_str(line);
                out.push_str("\r\n");
            }
        }
    }
    if !upgrade {
        out.push_str("Connection: close\r\n");
    }
    out.push_str("\r\n");
    (out, upgrade)
}

async fn answer(socket: &mut TcpStream, status: &str, extra: &str) -> std::io::Result<()> {
    socket.write_all(format!("HTTP/1.1 {status}\r\n{extra}Content-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await
}

/// Reads up to the end of the head: (head, what came after it).
async fn read_head(socket: &mut TcpStream) -> Option<(String, Vec<u8>)> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 2048];
    loop {
        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..at]).into_owned();
            return Some((head, buf[at + 4..].to_vec()));
        }
        if buf.len() > MAX_HEAD {
            return None;
        }
        let n = tokio::time::timeout(HEAD_PATIENCE, socket.read(&mut chunk)).await.ok()?.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

async fn connection(mut socket: TcpStream, sessions: Arc<Mutex<Sessions>>) -> std::io::Result<()> {
    let Some((head, rest)) = read_head(&mut socket).await else { return Ok(()) };
    let lines: Vec<&str> = head.split("\r\n").collect();
    let mut first = lines.first().copied().unwrap_or_default().split(' ');
    let (method, target) = (first.next().unwrap_or_default(), first.next().unwrap_or_default());
    let cookies = lines.iter().skip(1).find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("cookie")).map(|(_, v)| v.trim()));
    let verdict = judge(&mut sessions.lock().unwrap_or_else(|e| e.into_inner()), target, cookies);
    match verdict {
        Verdict::No(status) => answer(&mut socket, status, "").await,
        Verdict::Enter { location, cookie } => answer(&mut socket, "302 Found", &format!("Location: {location}\r\nSet-Cookie: {cookie}\r\nCache-Control: no-store\r\n")).await,
        Verdict::Pass { bot, viewer, path, mut open } => {
            let Ok(mut upstream) = TcpStream::connect(&viewer.addr).await else {
                return answer(&mut socket, "502 Bad Gateway", "").await;
            };
            let (out, upgrade) = rewrite(&lines[1..], method, &path, &viewer);
            upstream.write_all(out.as_bytes()).await?;
            upstream.write_all(&rest).await?;
            if upgrade {
                *sessions.lock().unwrap_or_else(|e| e.into_inner()).watching.entry(bot.clone()).or_default() += 1;
            }
            // until either side hangs up, or the session ends: a sleeping computer's viewer goes
            let piped = tokio::select! {
                done = tokio::io::copy_bidirectional(&mut socket, &mut upstream) => done.map(|_| ()),
                _ = open.changed() => Ok(()),
            };
            if upgrade {
                let mut s = sessions.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(n) = s.watching.get_mut(&bot) {
                    *n = n.saturating_sub(1);
                }
            }
            piped
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::Secret;

    /// A viewer that answers each connection with what it was sent.
    async fn echo_viewer() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let (head, _) = read_head(&mut s).await.unwrap();
                    let body = head.replace("\r\n", "\n");
                    let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).await;
                    if head.to_lowercase().contains("upgrade: websocket") {
                        let mut b = [0u8; 1];
                        let _ = s.read(&mut b).await;
                    }
                });
            }
        });
        addr
    }

    async fn get(port: u16, target: &str, extra: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{extra}\r\n").as_bytes()).await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        out
    }

    fn parts(url: &str) -> (u16, String) {
        let tail = url.strip_prefix("http://127.0.0.1:").unwrap();
        let (port, path) = tail.split_once('/').unwrap();
        (port.parse().unwrap(), format!("/{path}"))
    }

    fn cookie_of(reply: &str) -> String {
        reply.lines().find_map(|l| l.strip_prefix("Set-Cookie: ")).unwrap().split(';').next().unwrap().to_owned()
    }

    #[tokio::test]
    async fn a_ticket_opens_once_and_the_password_is_put_in_here() {
        let gate = Gate::start().await.unwrap();
        let viewer = Viewer { addr: echo_viewer().await, password: Secret::new("sesame") };
        let url = gate.open("b_1", viewer.clone()).unwrap();
        assert!(!url.contains("sesame"), "the URL holds no password");
        let (port, target) = parts(&url);
        let clean = target.split('?').next().unwrap().to_owned();

        // no ticket and no cookie: refused
        assert!(get(port, &clean, "").await.starts_with("HTTP/1.1 403"));

        let entered = get(port, &target, "").await;
        assert!(entered.starts_with("HTTP/1.1 302"), "{entered}");
        assert!(!entered.contains("sesame"), "{entered}");
        let cookie = cookie_of(&entered);

        // the ticket is spent
        assert!(get(port, &target, "").await.starts_with("HTTP/1.1 403"));

        let page = get(port, &format!("{clean}assets/a.js"), &format!("Cookie: {cookie}\r\nOrigin: http://127.0.0.1:{port}\r\nAuthorization: Bearer wrong\r\n")).await;
        assert!(page.starts_with("HTTP/1.1 200"), "{page}");
        assert!(page.contains("GET /assets/a.js HTTP/1.1"), "{page}");
        assert!(page.contains("Authorization: Bearer sesame"), "{page}");
        assert!(page.contains(&format!("Host: {}", viewer.addr)), "{page}");
        assert!(page.contains(&format!("Origin: http://{}", viewer.addr)), "{page}");
        assert!(!page.contains("Bearer wrong") && !page.contains("buwiz_viewer"), "{page}");
    }

    #[tokio::test]
    async fn a_wrong_cookie_a_stale_ticket_and_a_closed_session_are_refused() {
        let gate = Gate::start().await.unwrap();
        let viewer = Viewer { addr: echo_viewer().await, password: Secret::new("x") };
        let (port, target) = parts(&gate.open("b_1", viewer.clone()).unwrap());
        let clean = target.split('?').next().unwrap().to_owned();
        assert!(get(port, &clean, "Cookie: buwiz_viewer=nope\r\n").await.starts_with("HTTP/1.1 403"));
        // a wrong ticket burns it
        assert!(get(port, &format!("{clean}?ticket=nope"), "").await.starts_with("HTTP/1.1 403"));
        assert!(get(port, &target, "").await.starts_with("HTTP/1.1 403"));

        let (_, target) = parts(&gate.open("b_1", viewer).unwrap());
        let cookie = cookie_of(&get(port, &target, "").await);
        let clean = target.split('?').next().unwrap().to_owned();
        assert!(get(port, &clean, &format!("Cookie: {cookie}\r\n")).await.starts_with("HTTP/1.1 200"));
        gate.close("b_1");
        assert!(get(port, &clean, &format!("Cookie: {cookie}\r\n")).await.starts_with("HTTP/1.1 403"));
    }

    #[tokio::test]
    async fn a_failed_accept_doesnt_end_the_gate() {
        let listener = Arc::new(TcpListener::bind("127.0.0.1:0").await.unwrap());
        let addr = listener.local_addr().unwrap();
        let tries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (l, t) = (listener.clone(), tries.clone());
        let accept = move || {
            let (l, n) = (l.clone(), t.fetch_add(1, std::sync::atomic::Ordering::SeqCst));
            async move {
                if n == 0 {
                    return Err(std::io::Error::other("no more file descriptors"));
                }
                l.accept().await
            }
        };
        tokio::spawn(keep_accepting(accept, Arc::new(Mutex::new(Sessions::default())), Duration::from_millis(1)));
        // answered at all means the loop came back: an unknown session is a 404
        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_all(b"GET /nothing HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
        let mut out = String::new();
        tokio::time::timeout(Duration::from_secs(2), s.read_to_string(&mut out)).await.expect("the gate is gone").unwrap();
        assert!(out.starts_with("HTTP/1.1 404"), "{out}");
    }

    #[tokio::test]
    async fn closing_a_session_cuts_a_live_viewer() {
        let gate = Gate::start().await.unwrap();
        let viewer = Viewer { addr: echo_viewer().await, password: Secret::new("x") };
        let (port, target) = parts(&gate.open("b_1", viewer).unwrap());
        let cookie = cookie_of(&get(port, &target, "").await);
        let clean = target.split('?').next().unwrap().to_owned();
        let mut ws = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        ws.write_all(format!("GET {clean}ws HTTP/1.1\r\nHost: x\r\nCookie: {cookie}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n").as_bytes()).await.unwrap();
        let mut head = [0u8; 16];
        ws.read_exact(&mut head).await.unwrap();
        assert!(gate.watched("b_1"));
        gate.close("b_1");
        let mut rest = Vec::new();
        let ended = tokio::time::timeout(Duration::from_secs(2), ws.read_to_end(&mut rest)).await;
        assert!(ended.is_ok(), "the relay kept running after the session closed");
        for _ in 0..100 {
            if !gate.watched("b_1") {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("a cut viewer still counts as watching");
    }

    #[tokio::test]
    async fn an_open_websocket_counts_as_watching() {
        let gate = Gate::start().await.unwrap();
        let viewer = Viewer { addr: echo_viewer().await, password: Secret::new("x") };
        let (port, target) = parts(&gate.open("b_1", viewer).unwrap());
        let cookie = cookie_of(&get(port, &target, "").await);
        let clean = target.split('?').next().unwrap().to_owned();
        assert!(!gate.watched("b_1"));
        let mut ws = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        ws.write_all(format!("GET {clean}ws HTTP/1.1\r\nHost: x\r\nCookie: {cookie}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n").as_bytes()).await.unwrap();
        let mut b = [0u8; 16];
        ws.read_exact(&mut b).await.unwrap();
        assert!(gate.watched("b_1"));
        drop(ws);
        for _ in 0..100 {
            if !gate.watched("b_1") {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the viewer left and the gate still counts it");
    }
}
