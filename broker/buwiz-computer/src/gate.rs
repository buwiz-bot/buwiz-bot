//! The viewer gate: how a live desktop reaches the Bot's web view with the password left out.
//!
//! The app asks the broker to open a computer, and gets a URL on this gate holding a one-time
//! ticket. That URL is all it ever has, so what the tree, the logs and the `web_view` prop can
//! show is a ticket that's already spent. The first request swaps it for a cookie and a clean
//! URL. Every later request is passed to the viewer with the password put in here, and
//! the Host and Origin it expects (voxl refuses others): a page, its assets and `/ws`.
//!
//! A ticket names one Bot's screen and a [`Mode`]. What comes back from the viewer is a plain byte
//! pipe, but what goes to it is read frame by frame, because this is the only place outside the
//! box that sees a person's input. A watcher's input stops here, whatever the page does, and a
//! driver's is noted before it passes: that is what pauses the Bot's agent ([`Gate::driving`]). A
//! frame that can't be read ends the connection. A page and its assets are the only other thing
//! that passes, by GET, with nothing read after the head, so the viewer's other routes are shut.
//!
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
/// The most a frame to the viewer may hold: a pasted picture. Past this the connection ends.
const MAX_FRAME: usize = 16 * 1024 * 1024;
/// What of a frame is read to know what it is. Every command that matters fits.
const FRAME_HEAD: usize = 32;
/// All a viewer needs to show a page and a screen: voxl's page, what it loads, and its socket.
const VIEWER_PATHS: [&str; 9] = ["/", "/index.html", "/client.js", "/worker.js", "/audio.js", "/mic.js", "/favicon.ico", "/inter.woff2", SOCKET];
const SOCKET: &str = "/ws";
/// What a viewer says that doesn't act on the desktop: acks, video and audio on and off, keyframes
/// and ping replies. voxl's own list for `--read-only`; everything else is input. It has no switch
/// per viewer, which is why it's kept here.
const WATCHING: [&str; 5] = ["a", "v", "au", "kf", "po"];
/// voxl's release-all. The page sends it when it's hidden, so it's no sign of a person.
const RELEASE: &str = "r";

/// What a ticket lets a person do with a screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Watch,
    Drive,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Watch => "watch",
            Self::Drive => "drive",
        }
    }

    pub fn parse(word: &str) -> Option<Self> {
        [Self::Watch, Self::Drive].into_iter().find(|m| m.as_str() == word)
    }
}

struct Session {
    bot: String,
    mode: Mode,
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
    /// per Bot: what its drive viewers have done
    drivers: HashMap<String, Driver>,
}

/// A person at a Bot's screen.
#[derive(Default)]
struct Driver {
    /// their connected viewers: with none, they've gone, whatever came last
    viewers: usize,
    /// when a viewer connected or sent input, whichever came last
    last_seen: Option<Instant>,
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

    /// A URL that opens `viewer` once, for `bot`'s screen alone and in `mode`. An older session of
    /// `bot` is closed.
    pub fn open(&self, bot: &str, viewer: Viewer, mode: Mode) -> Result<String> {
        let (id, ticket, cookie) = (random_hex(16)?, random_hex(16)?, random_hex(24)?);
        let mut s = self.lock();
        s.open.retain(|_, o| o.bot != bot);
        s.open.insert(id.clone(), Session { bot: bot.to_owned(), mode, viewer, ticket: Some((ticket.clone(), Instant::now() + TICKET_LIFE)), cookie, open: watch::channel(()).0 });
        Ok(format!("http://127.0.0.1:{}/s/{id}/?ticket={ticket}", self.port))
    }

    /// The computer went to sleep or away: its session ends.
    pub fn close(&self, bot: &str) {
        self.lock().open.retain(|_, o| o.bot != bot);
    }

    /// That computer went to sleep: every viewer of it goes, whoever opened it. They share its
    /// password, so this needs no list of its Bots.
    pub fn close_computer(&self, password: &crate::Secret) {
        self.lock().open.retain(|_, o| o.viewer.password != *password);
    }

    /// Whether a person is watching `bot`'s computer: it doesn't sleep under them.
    pub fn watched(&self, bot: &str) -> bool {
        self.lock().watching.get(bot).is_some_and(|n| *n > 0)
    }

    /// The mode of `bot`'s session, if it has one.
    pub fn mode(&self, bot: &str) -> Option<Mode> {
        self.lock().open.values().find(|o| o.bot == bot).map(|o| o.mode)
    }

    /// Whether a person is at `bot`'s screen with a viewer that can drive it: one is connected, and
    /// it connected or sent input less than `quiet` ago. A person between clicks is still there,
    /// so an open viewer counts until it closes or `quiet` runs out. Leaving is stopping.
    pub fn driving(&self, bot: &str, quiet: Duration) -> bool {
        let s = self.lock();
        let Some(d) = s.drivers.get(bot).filter(|d| d.viewers > 0) else { return false };
        d.last_seen.is_some_and(|at| at.elapsed() < quiet)
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
    Pass { bot: String, mode: Mode, viewer: Viewer, path: String, open: watch::Receiver<()> },
}

/// `method` and `target` (`/s/<id>/rest?query`) with `cookies` against the open sessions.
fn judge(sessions: &mut Sessions, method: &str, target: &str, cookies: Option<&str>) -> Verdict {
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
    if method != "GET" || !VIEWER_PATHS.contains(&format!("/{path}").as_str()) {
        return Verdict::No("403 Forbidden");
    }
    Verdict::Pass { bot: session.bot.clone(), mode: session.mode, viewer: session.viewer.clone(), path: format!("/{tail}"), open: session.open.subscribe() }
}

/// The request's head for the viewer: its own path, Host, Origin and password, and none of the
/// app's cookie. Only the socket is upgraded, and never to a compression this gate couldn't read.
fn rewrite(lines: &[&str], method: &str, path: &str, viewer: &Viewer) -> (String, bool) {
    let header = |line: &&str| line.split_once(':').map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()));
    let upgrade = path.split('?').next() == Some(SOCKET) && lines.iter().filter_map(header).any(|(k, v)| k == "upgrade" && !v.is_empty());
    let mut out = format!("{method} {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\n", viewer.addr, viewer.password.expose());
    for line in lines {
        let Some((name, _)) = header(line) else { continue };
        match name.as_str() {
            "host" | "authorization" | "cookie" | "sec-websocket-extensions" => {}
            "origin" => out.push_str(&format!("Origin: http://{}\r\n", viewer.addr)),
            "connection" | "upgrade" if !upgrade => {}
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

/// One frame on its way to the viewer, as far as it needs reading.
#[derive(Debug, PartialEq, Eq)]
struct Frame {
    /// its whole length in the buffer
    len: usize,
    opcode: u8,
    fin: bool,
    /// the start of what it holds, unmasked
    start: Vec<u8>,
}

/// The first frame in `buf`, when all of it is there. `Err` for what a page never sends: no mask,
/// reserved bits, or more than [`MAX_FRAME`].
fn next_frame(buf: &[u8]) -> std::result::Result<Option<Frame>, ()> {
    let (Some(&first), Some(&second)) = (buf.first(), buf.get(1)) else { return Ok(None) };
    if first & 0x70 != 0 || second & 0x80 == 0 {
        return Err(());
    }
    let (size, mask_at) = match second & 0x7f {
        126 => {
            let Some(n) = buf.get(2..4) else { return Ok(None) };
            (u64::from(u16::from_be_bytes([n[0], n[1]])), 4)
        }
        127 => {
            let Some(n) = buf.get(2..10) else { return Ok(None) };
            (n.iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b)), 10)
        }
        n => (u64::from(n), 2),
    };
    let size = usize::try_from(size).ok().filter(|n| *n <= MAX_FRAME).ok_or(())?;
    let (at, len) = (mask_at + 4, mask_at + 4 + size);
    if buf.len() < len {
        return Ok(None);
    }
    let (mask, body) = (&buf[mask_at..at], &buf[at..len]);
    let start = body.iter().take(FRAME_HEAD).enumerate().map(|(i, b)| b ^ mask[i % 4]).collect();
    Ok(Some(Frame { len, opcode: first & 0x0f, fin: first & 0x80 != 0, start }))
}

/// A viewer's command word: what comes before its first comma.
fn word(text: &[u8]) -> &[u8] {
    text.split(|b| *b == b',').next().unwrap_or_default()
}

/// Decides each frame of one viewer.
struct Filter {
    bot: String,
    mode: Mode,
    sessions: Arc<Mutex<Sessions>>,
    /// the message under way was stopped: its later pieces go with it
    cut: bool,
}

impl Filter {
    /// Whether `frame` goes on. A driver's input is noted first, in the same lock [`Gate::driving`]
    /// reads, so once it's on its way the agent can't be told nobody's there.
    fn passes(&mut self, frame: &Frame) -> bool {
        match frame.opcode {
            // close, ping and pong
            0x8..=0xf => true,
            0x0 => {
                let pass = !self.cut;
                self.cut &= !frame.fin;
                pass
            }
            opcode => {
                let text = opcode == 0x1;
                let watching = text && WATCHING.iter().any(|w| w.as_bytes() == word(&frame.start));
                match self.mode {
                    // in pieces, the first alone doesn't say what the message is
                    Mode::Watch => {
                        let pass = watching && frame.fin;
                        self.cut = !pass && !frame.fin;
                        pass
                    }
                    Mode::Drive => {
                        if !watching {
                            if !(text && word(&frame.start) == RELEASE.as_bytes()) {
                                let mut s = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
                                s.drivers.entry(self.bot.clone()).or_default().last_seen = Some(Instant::now());
                            }
                        }
                        true
                    }
                }
            }
        }
    }

    /// Everything from the viewer's page, frame by frame, starting with `buf`. Ends when the page
    /// hangs up, or says something that can't be read.
    async fn run<R, W>(mut self, mut from: R, mut to: W, mut buf: Vec<u8>) -> std::io::Result<()>
    where
        R: tokio::io::AsyncRead + Unpin,
        W: tokio::io::AsyncWrite + Unpin,
    {
        let mut chunk = [0u8; 8192];
        loop {
            while let Some(frame) = next_frame(&buf).map_err(|()| std::io::Error::other("a frame that can't be read"))? {
                if self.passes(&frame) {
                    to.write_all(&buf[..frame.len]).await?;
                }
                buf.drain(..frame.len);
            }
            let n = from.read(&mut chunk).await?;
            if n == 0 {
                return Ok(());
            }
            buf.extend_from_slice(&chunk[..n]);
        }
    }
}

/// A connected socket, counted for as long as it's up.
struct Viewing {
    bot: String,
    mode: Mode,
    sessions: Arc<Mutex<Sessions>>,
}

impl Viewing {
    fn new(bot: String, mode: Mode, sessions: Arc<Mutex<Sessions>>) -> Self {
        {
            let mut s = sessions.lock().unwrap_or_else(|e| e.into_inner());
            *s.watching.entry(bot.clone()).or_default() += 1;
            if mode == Mode::Drive {
                let driver = s.drivers.entry(bot.clone()).or_default();
                driver.viewers += 1;
                driver.last_seen = Some(Instant::now());
            }
        }
        Self { bot, mode, sessions }
    }
}

impl Drop for Viewing {
    fn drop(&mut self) {
        let mut s = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = s.watching.get_mut(&self.bot) {
            *n = n.saturating_sub(1);
        }
        if self.mode == Mode::Drive {
            let gone = s.drivers.get_mut(&self.bot).is_some_and(|d| {
                d.viewers = d.viewers.saturating_sub(1);
                d.viewers == 0
            });
            if gone {
                s.drivers.remove(&self.bot);
            }
        }
    }
}

async fn connection(mut socket: TcpStream, sessions: Arc<Mutex<Sessions>>) -> std::io::Result<()> {
    let Some((head, rest)) = read_head(&mut socket).await else { return Ok(()) };
    let lines: Vec<&str> = head.split("\r\n").collect();
    let mut first = lines.first().copied().unwrap_or_default().split(' ');
    let (method, target) = (first.next().unwrap_or_default(), first.next().unwrap_or_default());
    let cookies = lines.iter().skip(1).find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("cookie")).map(|(_, v)| v.trim()));
    let verdict = judge(&mut sessions.lock().unwrap_or_else(|e| e.into_inner()), method, target, cookies);
    match verdict {
        Verdict::No(status) => answer(&mut socket, status, "").await,
        Verdict::Enter { location, cookie } => answer(&mut socket, "302 Found", &format!("Location: {location}\r\nSet-Cookie: {cookie}\r\nCache-Control: no-store\r\n")).await,
        Verdict::Pass { bot, mode, viewer, path, mut open } => {
            let Ok(mut upstream) = TcpStream::connect(&viewer.addr).await else {
                return answer(&mut socket, "502 Bad Gateway", "").await;
            };
            let (out, upgrade) = rewrite(&lines[1..], method, &path, &viewer);
            upstream.write_all(out.as_bytes()).await?;
            if !upgrade {
                // a GET: nothing the page says after its head reaches the viewer
                return tokio::select! {
                    biased;
                    _ = open.changed() => Ok(()),
                    done = tokio::io::copy(&mut upstream, &mut socket) => done.map(|_| ()),
                };
            }
            let filter = Filter { bot: bot.clone(), mode, sessions: sessions.clone(), cut: false };
            let _viewing = Viewing::new(bot, mode, sessions);
            let (page_in, mut page_out) = socket.split();
            let (mut viewer_in, viewer_out) = upstream.split();
            // until either side hangs up, or the session ends: a sleeping computer's viewer goes
            tokio::select! {
                biased;
                _ = open.changed() => Ok(()),
                done = filter.run(page_in, viewer_out, rest) => done,
                done = tokio::io::copy(&mut viewer_in, &mut page_out) => done.map(|_| ()),
            }
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
        let url = gate.open("b_1", viewer.clone(), Mode::Watch).unwrap();
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

        let page = get(port, &format!("{clean}client.js"), &format!("Cookie: {cookie}\r\nOrigin: http://127.0.0.1:{port}\r\nAuthorization: Bearer wrong\r\n")).await;
        assert!(page.starts_with("HTTP/1.1 200"), "{page}");
        assert!(page.contains("GET /client.js HTTP/1.1"), "{page}");
        assert!(page.contains("Authorization: Bearer sesame"), "{page}");
        assert!(page.contains(&format!("Host: {}", viewer.addr)), "{page}");
        assert!(page.contains(&format!("Origin: http://{}", viewer.addr)), "{page}");
        assert!(!page.contains("Bearer wrong") && !page.contains("buwiz_viewer"), "{page}");
    }

    #[tokio::test]
    async fn a_wrong_cookie_a_stale_ticket_and_a_closed_session_are_refused() {
        let gate = Gate::start().await.unwrap();
        let viewer = Viewer { addr: echo_viewer().await, password: Secret::new("x") };
        let (port, target) = parts(&gate.open("b_1", viewer.clone(), Mode::Watch).unwrap());
        let clean = target.split('?').next().unwrap().to_owned();
        assert!(get(port, &clean, "Cookie: buwiz_viewer=nope\r\n").await.starts_with("HTTP/1.1 403"));
        // a wrong ticket burns it
        assert!(get(port, &format!("{clean}?ticket=nope"), "").await.starts_with("HTTP/1.1 403"));
        assert!(get(port, &target, "").await.starts_with("HTTP/1.1 403"));

        let (_, target) = parts(&gate.open("b_1", viewer, Mode::Watch).unwrap());
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
    async fn closing_a_computer_closes_its_viewers_and_no_others() {
        let gate = Gate::start().await.unwrap();
        let addr = echo_viewer().await;
        let (ours, theirs) = (Secret::new("ours"), Secret::new("theirs"));
        let (port, ann) = parts(&gate.open("ann", Viewer { addr: addr.clone(), password: ours.clone() }, Mode::Watch).unwrap());
        let (_, bea) = parts(&gate.open("bea", Viewer { addr: addr.clone(), password: ours.clone() }, Mode::Watch).unwrap());
        let (_, cal) = parts(&gate.open("cal", Viewer { addr, password: theirs }, Mode::Watch).unwrap());
        gate.close_computer(&ours);
        assert!(get(port, &ann, "").await.starts_with("HTTP/1.1 403"));
        assert!(get(port, &bea, "").await.starts_with("HTTP/1.1 403"));
        assert!(get(port, &cal, "").await.starts_with("HTTP/1.1 302"), "another computer's viewer stays");
    }

    #[tokio::test]
    async fn closing_a_session_cuts_a_live_viewer() {
        let gate = Gate::start().await.unwrap();
        let viewer = Viewer { addr: echo_viewer().await, password: Secret::new("x") };
        let (port, target) = parts(&gate.open("b_1", viewer, Mode::Watch).unwrap());
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
        let (port, target) = parts(&gate.open("b_1", viewer, Mode::Watch).unwrap());
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

    /// A viewer that writes down every byte after a head, as the real one would get it.
    async fn listening_viewer() -> (String, Arc<Mutex<Vec<u8>>>, Arc<Mutex<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (heard, heads) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(String::new())));
        let (h, hd) = (heard.clone(), heads.clone());
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                let (h, hd) = (h.clone(), hd.clone());
                tokio::spawn(async move {
                    let (head, rest) = read_head(&mut s).await.unwrap();
                    hd.lock().unwrap().push_str(&head);
                    h.lock().unwrap().extend(rest);
                    let _ = s.write_all(b"HTTP/1.1 101 Switching Protocols\r\n\r\n").await;
                    let mut b = [0u8; 4096];
                    while let Ok(n) = s.read(&mut b).await {
                        if n == 0 {
                            break;
                        }
                        h.lock().unwrap().extend_from_slice(&b[..n]);
                    }
                });
            }
        });
        (addr, heard, heads)
    }

    /// A masked frame, as a page sends it.
    fn frame(first: u8, body: &[u8]) -> Vec<u8> {
        let mask = [0x12, 0x34, 0x56, 0x78];
        let mut out = vec![first];
        match body.len() {
            n if n < 126 => out.push(0x80 | n as u8),
            n if n < 65536 => {
                out.push(0x80 | 126);
                out.extend((n as u16).to_be_bytes());
            }
            n => {
                out.push(0x80 | 127);
                out.extend((n as u64).to_be_bytes());
            }
        }
        out.extend(mask);
        out.extend(body.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        out
    }

    fn text(command: &str) -> Vec<u8> {
        frame(0x81, command.as_bytes())
    }

    /// A socket through a `mode` ticket of `bot`.
    async fn socket(gate: &Gate, bot: &str, addr: &str, mode: Mode) -> TcpStream {
        let (port, target) = parts(&gate.open(bot, Viewer { addr: addr.to_owned(), password: Secret::new("x") }, mode).unwrap());
        let cookie = cookie_of(&get(port, &target, "").await);
        let clean = target.split('?').next().unwrap().to_owned();
        let mut ws = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        ws.write_all(format!("GET {clean}ws HTTP/1.1\r\nHost: x\r\nCookie: {cookie}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Extensions: permessage-deflate\r\n\r\n").as_bytes()).await.unwrap();
        let mut head = [0u8; 16];
        ws.read_exact(&mut head).await.unwrap();
        ws
    }

    /// Until `heard` holds `want`, for a moment.
    async fn hears(heard: &Arc<Mutex<Vec<u8>>>, want: &[u8]) -> bool {
        for _ in 0..200 {
            if heard.lock().unwrap().windows(want.len()).any(|w| w == want) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        false
    }

    const QUIET: Duration = Duration::from_secs(30);

    #[tokio::test]
    async fn a_watcher_is_shown_the_screen_and_nothing_it_sends_acts_on_it() {
        let gate = Gate::start().await.unwrap();
        let (addr, heard, heads) = listening_viewer().await;
        let mut ws = socket(&gate, "b_1", &addr, Mode::Watch).await;
        assert_eq!(gate.mode("b_1"), Some(Mode::Watch));
        for input in ["m,10,20", "b,1,1", "b,1,0", "k,65,1", "s,0,1", "sp,0.0,12.0", "c,pasted", "r", "mi,1", "sz,640,480", "a-new-word,1", "m"] {
            ws.write_all(&text(input)).await.unwrap();
        }
        ws.write_all(&frame(0x82, &[0x11, 1, 2, 3])).await.unwrap();
        // in pieces: "k" then ",65,1", and "a" then "bc": what the first piece says isn't enough
        ws.write_all(&frame(0x01, b"k")).await.unwrap();
        ws.write_all(&frame(0x80, b",65,1")).await.unwrap();
        ws.write_all(&frame(0x01, b"a")).await.unwrap();
        ws.write_all(&frame(0x80, b",1")).await.unwrap();
        let watching = [text("a,1"), text("v,1"), text("au,0"), text("kf"), text("po,12"), frame(0x89, b"hi")].concat();
        ws.write_all(&watching).await.unwrap();
        assert!(hears(&heard, &watching).await, "what watching needs was stopped");
        let got = heard.lock().unwrap().clone();
        assert_eq!(got.len(), watching.len(), "a watcher's input got through: {got:?}");
        assert!(!gate.driving("b_1", QUIET), "watching isn't driving");
        assert!(!heads.lock().unwrap().to_lowercase().contains("sec-websocket-extensions"), "compressed frames couldn't be read");
    }

    #[tokio::test]
    async fn a_drive_viewer_counts_as_a_person_until_it_closes_or_goes_quiet() {
        let gate = Gate::start().await.unwrap();
        let (addr, heard, _) = listening_viewer().await;
        let mut ws = socket(&gate, "ann", &addr, Mode::Drive).await;
        assert_eq!(gate.mode("ann"), Some(Mode::Drive));
        assert!(gate.driving("ann", QUIET), "open and idle: they're about to press something");
        assert!(!gate.driving("bea", QUIET), "another Bot's screen isn't being driven");

        // past the quiet, and a page's own talk doesn't bring them back
        tokio::time::sleep(Duration::from_millis(30)).await;
        let short = Duration::from_millis(20);
        assert!(!gate.driving("ann", short), "gone quiet");
        for said in ["a,1", "po,1", "r"] {
            ws.write_all(&text(said)).await.unwrap();
            assert!(hears(&heard, &text(said)).await);
        }
        assert!(!gate.driving("ann", short), "a page's own talk isn't a person");

        ws.write_all(&text("k,65,1")).await.unwrap();
        assert!(hears(&heard, &text("k,65,1")).await);
        assert!(gate.driving("ann", short), "input brings them back");

        drop(ws);
        for _ in 0..100 {
            if !gate.driving("ann", QUIET) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("leaving is stopping");
    }

    #[tokio::test]
    async fn a_new_ticket_replaces_the_old_mode_and_a_closed_session_stops_driving() {
        let gate = Gate::start().await.unwrap();
        let (addr, heard, _) = listening_viewer().await;
        let mut driver = socket(&gate, "ann", &addr, Mode::Drive).await;
        driver.write_all(&text("m,1,2")).await.unwrap();
        assert!(hears(&heard, &text("m,1,2")).await);
        let _watcher = socket(&gate, "ann", &addr, Mode::Watch).await;
        assert_eq!(gate.mode("ann"), Some(Mode::Watch));
        for _ in 0..100 {
            if !gate.driving("ann", QUIET) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(!gate.driving("ann", QUIET), "the replaced session still counts as driving");
        let mut rest = Vec::new();
        assert!(tokio::time::timeout(Duration::from_secs(2), driver.read_to_end(&mut rest)).await.is_ok(), "the old driver is still connected");
        gate.close("ann");
        assert_eq!(gate.mode("ann"), None);
    }

    #[tokio::test]
    async fn a_ticket_names_one_screen() {
        let gate = Gate::start().await.unwrap();
        let (ann, ann_heard, _) = listening_viewer().await;
        let (bea, bea_heard, _) = listening_viewer().await;
        let mut ws = socket(&gate, "ann", &ann, Mode::Drive).await;
        let _other = socket(&gate, "bea", &bea, Mode::Drive).await;
        ws.write_all(&text("k,65,1")).await.unwrap();
        assert!(hears(&ann_heard, &text("k,65,1")).await);
        assert!(gate.driving("ann", QUIET) && !gate.driving("bea", QUIET));
        assert!(bea_heard.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn what_cant_be_read_ends_the_connection_and_reaches_nothing() {
        for bad in [vec![0x81, 0x01, b'k'], vec![0xc1, 0x81, 0, 0, 0, 0, b'k'], [vec![0x81, 0x80 | 127], ((MAX_FRAME + 1) as u64).to_be_bytes().to_vec()].concat()] {
            let gate = Gate::start().await.unwrap();
            let (addr, heard, _) = listening_viewer().await;
            let mut ws = socket(&gate, "b_1", &addr, Mode::Drive).await;
            ws.write_all(&bad).await.unwrap();
            let mut rest = Vec::new();
            assert!(tokio::time::timeout(Duration::from_secs(2), ws.read_to_end(&mut rest)).await.is_ok(), "{bad:?} left it open");
            assert!(heard.lock().unwrap().is_empty(), "{bad:?}");
        }
    }

    #[test]
    fn frames_are_read_however_long_and_however_they_arrive() {
        let long = vec![b'x'; 70_000];
        for (f, len) in [(text("k,65,1"), 6), (frame(0x81, &vec![b'y'; 300]), 300), (frame(0x82, &long), 70_000)] {
            let found = next_frame(&f).unwrap().unwrap();
            assert_eq!(found.len, f.len());
            assert_eq!(found.start.len(), len.min(FRAME_HEAD));
            for cut in 0..f.len().min(40) {
                assert_eq!(next_frame(&f[..cut]), Ok(None), "cut at {cut}");
            }
        }
        assert_eq!(next_frame(&text("k,65,1")).unwrap().unwrap().start, b"k,65,1");
    }

    #[tokio::test]
    async fn a_ticket_opens_the_page_and_the_socket_and_none_of_the_viewers_other_routes() {
        let gate = Gate::start().await.unwrap();
        let viewer = Viewer { addr: echo_viewer().await, password: Secret::new("x") };
        for mode in [Mode::Watch, Mode::Drive] {
            let (port, target) = parts(&gate.open("b_1", viewer.clone(), mode).unwrap());
            let cookie = format!("Cookie: {}\r\n", cookie_of(&get(port, &target, "").await));
            let clean = target.split('?').next().unwrap().to_owned();
            for path in ["", "index.html", "client.js", "worker.js", "audio.js", "mic.js", "favicon.ico", "inter.woff2"] {
                assert!(get(port, &format!("{clean}{path}"), &cookie).await.starts_with("HTTP/1.1 200"), "{path}");
            }
            for path in ["screenshot", "record", "record/start", "wait", "stats", "files/list", "files/download", "healthz", "../screenshot"] {
                assert!(get(port, &format!("{clean}{path}"), &cookie).await.starts_with("HTTP/1.1 403"), "{path}");
            }
            let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            s.write_all(format!("POST {clean} HTTP/1.1\r\nHost: x\r\n{cookie}Content-Length: 0\r\n\r\n").as_bytes()).await.unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).await.unwrap();
            assert!(out.starts_with("HTTP/1.1 403"), "{out}");
        }
    }

    #[tokio::test]
    async fn a_page_cant_send_a_second_request_after_its_first() {
        let gate = Gate::start().await.unwrap();
        let (addr, heard, heads) = listening_viewer().await;
        let (port, target) = parts(&gate.open("b_1", Viewer { addr, password: Secret::new("x") }, Mode::Watch).unwrap());
        let cookie = cookie_of(&get(port, &target, "").await);
        let clean = target.split('?').next().unwrap().to_owned();
        let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(format!("GET {clean}client.js HTTP/1.1\r\nHost: x\r\nCookie: {cookie}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\nPOST /record/start HTTP/1.1\r\n\r\n").as_bytes()).await.unwrap();
        for _ in 0..200 {
            if !heads.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        s.write_all(b"POST /record/stop HTTP/1.1\r\n\r\n").await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let got = heard.lock().unwrap().clone();
        assert!(got.is_empty(), "{:?}", String::from_utf8_lossy(&got));
        let head = heads.lock().unwrap().to_lowercase();
        assert!(head.contains("connection: close") && !head.contains("upgrade"), "{head}");
    }
}
