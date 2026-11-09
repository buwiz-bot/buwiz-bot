//! The macOS side of a v1 open: the [`HostUi`] the bridge asks (the file
//! dialog, the pasteboard, the browser, the full-screen stack, the print
//! preview), and the open's start, from the session (on a thread of its
//! own: it waits on the broker and the plugin) to the app's load on the
//! main thread.
//!
//! Consent and the chat an attachment goes to are part B's: until it sets
//! them ([`set_consent`], [`set_chat`]), consent is declined and there is
//! no chat, so `buwiz.open({url})` and `buwiz.attach` fail closed.

use std::{
    cell::RefCell,
    collections::HashMap,
    path::PathBuf,
    rc::{Rc, Weak},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::agent_hub::hold::{Answer, Card};
use nsh_platform_macos::{
    AppView,
    objc2::{
        DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
        rc::Retained,
        runtime::{AnyClass, AnyObject, NSObject, NSObjectProtocol},
    },
    objc2_foundation::{NSArray, NSString, NSURL},
};
use buwiz_agent::OnceId;
use serde_json::Value;

use super::{
    RoxError, V1Key,
    attribution::{self, Actor},
    bridge::{self, Consent, Emit, HostUi, Live, OpenContext, PickRequest, Press},
    cards::{CardAction, CardMeta},
    document::{self, Document},
    host_api, load,
    load::Prepared,
    screens,
    session::{self, Ended, Session},
    system_open::{self, LocalFile},
};

/// Runs `f` on the main thread, later.
pub fn on_main(f: impl FnOnce() + Send + 'static) {
    dispatch2::DispatchQueue::main().exec_async(f);
}

/// Part B's consent dialogs: `answer(true)` goes ahead.
pub type ConsentFn = dyn Fn(Consent, Box<dyn FnOnce(bool) + Send>) + Send + Sync;

/// Part B's chat: whether the open `key` has one, and attaching a token
/// to it (false when it was attached already).
pub trait Chat: Send + Sync {
    fn has_chat(&self, key: &str) -> bool;
    fn attach(&self, key: &str, token: &str, title: &str) -> bool;
}

static CONSENT: Mutex<Option<Arc<ConsentFn>>> = Mutex::new(None);
static CHAT: Mutex<Option<Arc<dyn Chat>>> = Mutex::new(None);

/// Sets how the person is asked (spec 9.7): part B's dialogs.
pub fn set_consent(f: Arc<ConsentFn>) {
    *CONSENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
}

/// Sets the chat attachments go to: part B's.
pub fn set_chat(chat: Arc<dyn Chat>) {
    *CHAT.lock().unwrap_or_else(|e| e.into_inner()) = Some(chat);
}

/// The person, as one open of `key` (its module key) reaches them. `n`
/// is the open's number: what its card's buttons and status are set by.
pub struct MacUi {
    key: String,
    title: String,
    n: u64,
    /// The open's nsh surface: whose input its calls follow.
    surface: u64,
    /// A card in the chat, not a full-screen open.
    inline: bool,
}

impl HostUi for MacUi {
    fn consent(&self, ask: Consent, answer: Box<dyn FnOnce(bool) + Send>) {
        let f = CONSENT.lock().unwrap_or_else(|e| e.into_inner()).clone();
        match f {
            Some(f) => f(ask, answer),
            None => {
                log::warn!("mini v1: no consent dialog yet; declined {ask:?}");
                answer(false);
            }
        }
    }

    fn actor(&self) -> Actor {
        attribution::actor(self.surface)
    }

    fn fresh_press(&self) -> Option<u64> {
        attribution::fresh_press(self.surface)
    }

    fn spend_press(&self, press: u64) -> bool {
        attribution::spend_press(self.surface, press)
    }

    fn agent_call(&self, running: bool) {
        attribution::agent_call(running);
    }

    fn agent_write(&self, running: bool) {
        attribution::agent_write(running);
    }

    fn hold(&self, once: OnceId, card: Card, done: Box<dyn FnOnce(Answer) + Send>) {
        crate::agent_hub::show_card(self.n, once, card, done);
    }

    fn hold_over(&self, once: &OnceId) {
        crate::agent_hub::hide_card(once.clone());
    }

    fn open_app(&self, target: V1Key) {
        super::to_store(move |st| st.open_mini_token(&target.to_key(), "", ""));
    }

    fn open_url(&self, url: &str) {
        let url = url.to_owned();
        on_main(move || open_in_browser(&url));
    }

    fn has_chat(&self) -> bool {
        let chat = CHAT.lock().unwrap_or_else(|e| e.into_inner()).clone();
        chat.is_some_and(|c| c.has_chat(&self.key))
    }

    fn attach(&self, token: &str, title: &str) -> bool {
        let chat = CHAT.lock().unwrap_or_else(|e| e.into_inner()).clone();
        chat.is_some_and(|c| c.attach(&self.key, token, title))
    }

    fn pick_files(&self, pick: PickRequest, answer: Box<dyn FnOnce(Vec<PathBuf>) + Send>) {
        on_main(move || {
            let mtm = MainThreadMarker::new().expect("on the main thread");
            answer(open_panel(mtm, &pick));
        });
    }

    fn copy(&self, text: &str) {
        let text = text.to_owned();
        on_main(move || write_pasteboard(&text));
    }

    fn preview(&self, doc: bridge::Preview) -> Result<(), RoxError> {
        super::to_store(move |st| st.v1_preview(doc));
        Ok(())
    }

    fn plugin_folder(&self, plugin: &str) -> Option<PathBuf> {
        document::folder_of(std::env::var_os("HOME").as_deref(), plugin)
    }

    /// Only over the full-screen app on top: a hidden chat keeps its cards
    /// running, so a live card may be off the screen. The store decides,
    /// when the job runs, for the person may have left since the call.
    ///
    /// A picture or a PDF is copied first, from the file opened at the check,
    /// on a thread of its own, and shown from the copy (`document`).
    fn open_document(&self, doc: Document, live: Live) {
        if self.inline {
            return log::info!("mini v1: a card's document opened nothing");
        }
        let key = self.key.clone();
        if !doc.needs_copy() {
            return super::to_store(move |st| st.v1_document(&key, &doc, live()));
        }
        std::thread::spawn(move || {
            let shown = doc.clone();
            let (to, again) = (key.clone(), live.clone());
            // the store shows it, if its screen is still the person's: the
            // check and the showing in one job; a miss takes a copy this made
            let done = move |copy: system_open::Copied| {
                let doc = shown.with_copy(&copy.path);
                super::to_store(move |st| {
                    if st.v1_on_top(&to, again()) {
                        st.v1_document(&to, &doc, true);
                    } else if copy.fresh {
                        system_open::discard(&copy.path);
                    }
                });
            };
            let copy = || system_open::copy_for_view(doc.file(), doc.name(), &copies()?, system_open::CAP);
            handed("a document", system_open::copy_between_checks(&|| shows(&key, &live), copy, done));
        });
    }

    fn file_refused(&self, said: &'static str) {
        super::to_store(move |_| crate::toast::show(said));
    }

    /// A card in the chat hands nothing off: the bridge refuses it out loud.
    fn hands_off(&self) -> bool {
        !self.inline
    }

    /// On `open_document`'s terms. A marked copy of the Bot's own is made on
    /// a thread of its own, then opened on the main thread, as `open_url`
    /// opens.
    fn open_with(&self, file: LocalFile, live: Live) {
        if self.inline {
            return log::info!("mini v1: a card's file opened nothing");
        }
        let key = self.key.clone();
        std::thread::spawn(move || {
            let (to, again) = (key.clone(), live.clone());
            // main asks the store again whether the screen is the person's,
            // on top (the store's thread alone knows), and opens or takes the
            // copy away, in one go: while main waits, no input of the
            // person's can move the screen
            let done = move |copy: system_open::Copied| {
                on_main(move || {
                    let on_top = || shows_within(&to, &again, ON_MAIN_WAIT);
                    if !system_open::open_or_discard(&copy.path, on_top, system_open::open_copy) {
                        log::info!("mini v1: a file opened nothing: its screen left the top, its copy wasn't marked, or nothing opened it");
                        super::to_store(|_| crate::toast::show(bridge::OPEN_REFUSED));
                    }
                })
            };
            let copy = || system_open::copy_for_open(&file, &copies()?, system_open::CAP).map(system_open::Copied::fresh);
            handed("a file", system_open::copy_between_checks(&|| shows(&key, &live), copy, done));
        });
    }

    /// On `open_document`'s terms. The copy runs on a thread of its own (an
    /// attachment can be large), then a toast says how it went.
    fn save(&self, file: LocalFile, live: Live) {
        if self.inline {
            return log::info!("mini v1: a card's file saved nothing");
        }
        let key = self.key.clone();
        super::to_store(move |st| {
            if !st.v1_on_top(&key, live()) {
                return log::info!("mini v1: a file came after its screen left the top: nothing saved");
            }
            std::thread::spawn(move || {
                let saved = system_open::downloads().ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound)).and_then(|d| system_open::save_in(&file, &d));
                // the error's kind only: its text may hold a path
                if let Err(e) = &saved {
                    log::info!("mini v1: a file saved nothing: {:?}", e.kind());
                }
                let said = if saved.is_ok() { "Saved to Downloads" } else { "Buwiz Bot couldn't save that file to Downloads" };
                super::to_store(move |_| crate::toast::show(said));
            });
        });
    }

    fn fit(&self, height: f32) {
        let key = self.key.clone();
        super::to_store(move |st| {
            st.v1.fit.update(|f| {
                f.insert(key, height);
            })
        });
    }

    fn ready(&self) {
        let (key, n) = (self.key.clone(), self.n);
        super::to_store(move |st| {
            st.v1.ready.update(|r| {
                r.insert(key, n);
            })
        });
    }

    fn actions(&self, actions: Vec<CardAction>, press: Press) {
        let (key, owner) = (self.key.clone(), self.n);
        super::to_store(move |st| {
            st.v1.footer.update(|f| {
                f.insert(key, super::ui::Footer { owner, actions, press });
            })
        });
    }

    fn meta(&self, meta: Option<CardMeta>) {
        let (key, owner) = (self.key.clone(), self.n);
        super::to_store(move |st| {
            st.v1.meta.update(|m| match meta {
                Some(meta) => {
                    m.insert(key, (owner, meta));
                }
                None => {
                    m.remove(&key);
                }
            })
        });
    }

    fn settle(&self, line: String) {
        let key = self.key.clone();
        super::to_store(move |st| st.v1_settle(&key, line));
    }

    fn session_ended(&self, _: &V1Key, why: Ended) {
        // its buttons and status go with it, unless a newer open of the
        // same card set them since
        let (key, owner) = (self.key.clone(), self.n);
        super::to_store(move |st| st.v1_card_ended(&key, owner));
        // a restart is a new generation (13.7): "Reconnecting" until the
        // plugin runs again, then the open reopens on it
        if why == Ended::Restarted {
            let (key, title) = (self.key.clone(), self.title.clone());
            super::to_store(move |st| st.v1_reconnecting(&key, &title));
        }
    }
}

define_class!(
    /// The file dialog's delegate: files over the size limit are not
    /// offered (spec 7.7).
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = u64]
    #[name = "RoxPickSizeFilter"]
    struct SizeFilter;

    unsafe impl NSObjectProtocol for SizeFilter {}

    impl SizeFilter {
        #[unsafe(method(panel:shouldEnableURL:))]
        fn should_enable(&self, _panel: &AnyObject, url: &NSURL) -> bool {
            let max = *self.ivars();
            url.path().is_some_and(|p| std::fs::metadata(p.to_string()).is_ok_and(|m| m.is_dir() || m.len() <= max))
        }
    }
);

impl SizeFilter {
    fn new(mtm: MainThreadMarker, max: u64) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(max);
        // SAFETY: NSObject's designated `init` on a fresh allocation.
        unsafe { msg_send![super(this), init] }
    }
}

/// The app's own open panel for one picture (avatar pictures): it opens
/// over the app and leaves it in front, unlike AppleScript's chooser,
/// which hands focus to Finder when it closes. `then` gets the path, or
/// None on Cancel, on the main thread.
pub fn pick_image(then: impl FnOnce(Option<PathBuf>) + Send + 'static) {
    on_main(move || {
        let mtm = MainThreadMarker::new().expect("on the main thread");
        let pick = PickRequest {
            extensions: [".png", ".jpg", ".jpeg", ".gif", ".webp", ".heic", ".heif", ".avif", ".tif", ".tiff", ".bmp"]
                .into_iter()
                .map(String::from)
                .collect(),
            multiple: false,
            max_bytes: 64 << 20,
        };
        then(open_panel(mtm, &pick).into_iter().next());
    });
}

/// The native open panel for `pick`: the files chosen, none on Cancel.
fn open_panel(mtm: MainThreadMarker, pick: &PickRequest) -> Vec<PathBuf> {
    let Some(class) = AnyClass::get(c"NSOpenPanel") else { return Vec::new() };
    let types: Vec<Retained<NSString>> =
        pick.extensions.iter().map(|e| NSString::from_str(e.trim_start_matches('.'))).collect();
    let types = NSArray::from_retained_slice(&types);
    let filter = SizeFilter::new(mtm, pick.max_bytes);
    // SAFETY: NSOpenPanel's documented class and instance methods, with
    // arguments of their types, on the main thread.
    unsafe {
        let panel: Retained<AnyObject> = msg_send![class, openPanel];
        let () = msg_send![&*panel, setCanChooseFiles: true];
        let () = msg_send![&*panel, setCanChooseDirectories: false];
        let () = msg_send![&*panel, setAllowsMultipleSelection: pick.multiple];
        if !pick.extensions.is_empty() {
            let () = msg_send![&*panel, setAllowedFileTypes: &*types];
        }
        let () = msg_send![&*panel, setDelegate: &*filter];
        let chosen: isize = msg_send![&*panel, runModal];
        let () = msg_send![&*panel, setDelegate: None::<&AnyObject>];
        if chosen != 1 {
            return Vec::new();
        }
        let urls: Retained<NSArray<NSURL>> = msg_send![&*panel, URLs];
        urls.iter().filter_map(|u| u.path()).map(|p| PathBuf::from(p.to_string())).collect()
    }
}

/// Puts `text` on the general pasteboard.
fn write_pasteboard(text: &str) {
    let Some(class) = AnyClass::get(c"NSPasteboard") else { return };
    let (s, kind) = (NSString::from_str(text), NSString::from_str("public.utf8-plain-text"));
    // SAFETY: NSPasteboard's documented methods with arguments of their
    // types, on the main thread.
    unsafe {
        let board: Retained<AnyObject> = msg_send![class, generalPasteboard];
        let _: isize = msg_send![&*board, clearContents];
        let _: bool = msg_send![&*board, setString: &*s, forType: &*kind];
    }
}

/// [`system_open::opened_dir`], or an error to fail a copy with.
fn copies() -> std::io::Result<PathBuf> {
    system_open::opened_dir().ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
}

/// Whether the open `key` is still the person's screen, on top, and `live`,
/// as the store has it: what a hand-off that copies asks before and after its
/// copy, from the copy's own thread. No answer in time is no.
fn shows(key: &str, live: &Live) -> bool {
    shows_within(key, live, std::time::Duration::from_secs(10))
}

/// How long main waits for the store's answer before an open: short, for
/// main draws nothing meanwhile. No answer is no open.
const ON_MAIN_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// [`shows`], waiting at most `wait`.
fn shows_within(key: &str, live: &Live, wait: std::time::Duration) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    let (key, live) = (key.to_owned(), live.clone());
    super::to_store(move |st| {
        let _ = tx.send(st.v1_on_top(&key, live()));
    });
    rx.recv_timeout(wait).unwrap_or(false)
}

/// Logs how a hand-off of `what` that copies went, nothing of the file, and
/// tells the person of a copy that failed.
fn handed(what: &str, how: system_open::Handed) {
    match how {
        system_open::Handed::Done => {}
        system_open::Handed::Failed(kind) => {
            log::info!("mini v1: {what} opened nothing: {kind:?}");
            super::to_store(|_| crate::toast::show(bridge::OPEN_REFUSED));
        }
        left => log::info!("mini v1: {what} opened nothing: its screen left the top ({left:?})"),
    }
}

/// Opens an https `url` the person agreed to in the browser.
fn open_in_browser(url: &str) {
    let (Some(class), Some(url)) = (AnyClass::get(c"NSWorkspace"), NSURL::URLWithString(&NSString::from_str(url)))
    else {
        return;
    };
    // SAFETY: NSWorkspace's documented methods, on the main thread.
    unsafe {
        let ws: Retained<AnyObject> = msg_send![class, sharedWorkspace];
        let _: bool = msg_send![&*ws, openURL: &*url];
    }
}

thread_local! {
    /// The views of opens whose sessions are opening, by open number.
    static VIEWS: RefCell<HashMap<u64, Weak<AppView>>> = RefCell::new(HashMap::new());
    static NEXT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// One open's bridge, while its component lives.
pub struct V1Open {
    slot: Arc<Slot>,
    /// Its number among the opens running (12.2's banner).
    n: u64,
}

#[derive(Default)]
struct Slot {
    closed: AtomicBool,
    ctx: Mutex<Option<Arc<OpenContext>>>,
    /// A native screen's viewport: the latest one, and where it goes once
    /// the screen's bridge is up.
    viewport: Mutex<Option<Value>>,
    emit: Mutex<Option<Emit>>,
}

impl V1Open {
    /// Ends the open: its session (also one still opening) closes.
    pub fn close(&self) {
        self.slot.closed.store(true, Ordering::SeqCst);
        if let Some(ctx) = self.slot.ctx.lock().unwrap_or_else(|e| e.into_inner()).take() {
            ctx.close();
        }
    }

    /// Its number: what its card's buttons, status and ready are set by.
    pub fn n(&self) -> u64 {
        self.n
    }

    /// Where a native screen's viewport changes go (a TSX app's go to its
    /// channel): to its bridge once it is up, the first one meanwhile.
    pub fn viewport_sink(&self) -> impl Fn(Value) + 'static {
        let slot = self.slot.clone();
        move |v: Value| {
            let emit = slot.emit.lock().unwrap_or_else(|e| e.into_inner()).clone();
            match emit {
                Some(emit) => emit("buwiz.viewport", v),
                None => *slot.viewport.lock().unwrap_or_else(|e| e.into_inner()) = Some(v),
            }
        }
    }
}

impl Drop for V1Open {
    fn drop(&mut self) {
        self.close();
        let n = self.n;
        crate::agent_hub::closed(n);
        super::to_store(move |st| st.v1_stopped(n));
    }
}

/// What an open's failure shows (6.7).
fn shown(prepared: &Prepared, e: &RoxError) -> String {
    match e.code.as_str() {
        "plugin_unavailable" => load::say::unavailable(&prepared.plugin.title),
        "plugin_bad_signature" => format!("Could not open {}: {} failed its signature check.", prepared.title, prepared.plugin.title),
        _ => format!("Could not open {}: {}", prepared.title, e.message),
    }
}

/// Starts an open of `prepared` in `view` (made for it: its own channel,
/// storage and limits): the session on a thread, then the bridge, then the
/// app's load on the main thread. `key` is the module key.
pub fn start(prepared: Arc<Prepared>, view: &Rc<AppView>, key: String, viewport: Value) -> V1Open {
    let id = NEXT.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    VIEWS.with_borrow_mut(|v| v.insert(id, Rc::downgrade(view)));
    crate::agent_hub::opened(id, &key, &prepared.plugin.short, &prepared.plugin.title, &prepared.key.app, &prepared.title, Rc::downgrade(view));
    let channel = view.host();
    let surface = view.surface_key();
    let inline = viewport.get("placement").and_then(Value::as_str) == Some("inline");
    let slot = Arc::new(Slot::default());
    let s = slot.clone();
    std::thread::spawn(move || {
        session::listen_for_generations();
        let broker: Arc<dyn host_api::Broker> = Arc::new(host_api::HttpBroker::configured());
        let p = &prepared;
        let t0 = std::time::Instant::now();
        // a session opened ahead of the card (14.6), else a new one
        let pre = session::take_preopened(&p.plugin.id, &p.key.app, p.permissions_json.as_ref());
        let reused = pre.is_some();
        let opened = match pre {
            Some(s) => Ok(s),
            None => Session::open(&*broker, &p.plugin.id, &p.key.app, p.permissions_json.as_ref(), p.plugin.generation),
        };
        log::info!(
            "mini v1 timing: {}/{} session open {} ms{}",
            p.plugin.id,
            p.key.app,
            t0.elapsed().as_millis(),
            if reused { " (pre-opened)" } else { "" }
        );
        let session = match opened {
            Ok(session) => session,
            Err(e) => {
                log::warn!("mini v1: {} {}: {e}", p.plugin.id, p.key.app);
                let text = shown(p, &e);
                on_main(move || {
                    VIEWS.with_borrow_mut(|v| v.remove(&id));
                });
                return super::to_store(move |st| st.mini_err(&key).set(text));
            }
        };
        let ui = Arc::new(MacUi { key: key.clone(), title: p.title.clone(), n: id, surface, inline });
        let dir = super::plugins_dir();
        let ctx = OpenContext::new(prepared.clone(), session, broker, ui, &dir, bridge::Limits::default());
        {
            let mut held = s.ctx.lock().unwrap_or_else(|e| e.into_inner());
            if s.closed.load(Ordering::SeqCst) {
                drop(held);
                return ctx.close();
            }
            *held = Some(ctx.clone());
        }
        let app = match &prepared.native {
            None => {
                bridge::install(&channel, &ctx, viewport);
                let bundle = nsh_engine::Bundle::source(prepared.js.clone()).with_policy(prepared.permissions.policy());
                nsh_platform_macos::App::Js(bundle)
            }
            Some(_) => {
                // a screen's failure shows the banner a TSX app's uncaught
                // error does (6.7)
                let (k, title) = (key.clone(), prepared.title.clone());
                let fail = Arc::new(move |message: &str| {
                    log::warn!("mini v1: {k}: the native screen failed: {message}");
                    let (k, text) = (k.clone(), load::say::runtime(&title, message));
                    super::to_store(move |st| st.v1_error(&k, text));
                });
                let viewport = s.viewport.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or(viewport);
                match screens::mount(&ctx, viewport, fail) {
                    Ok(m) => {
                        *s.emit.lock().unwrap_or_else(|e| e.into_inner()) = Some(m.emit);
                        nsh_platform_macos::App::Rust(m.root)
                    }
                    Err(e) => {
                        log::warn!("mini v1: {} {}: {e}", p.plugin.id, p.key.app);
                        ctx.close();
                        let text = e.message;
                        on_main(move || {
                            VIEWS.with_borrow_mut(|v| v.remove(&id));
                        });
                        return super::to_store(move |st| st.mini_err(&key).set(text));
                    }
                }
            }
        };
        let fingerprint = prepared.fingerprint.clone();
        super::to_store(move |st| st.v1_running(id, &key, &fingerprint));
        let t_load = std::time::Instant::now();
        let kind = if prepared.native.is_some() { "native" } else { "tsx" };
        let (pid, aid) = (prepared.plugin.id.clone(), prepared.key.app.clone());
        on_main(move || {
            let view = VIEWS.with_borrow_mut(|v| v.remove(&id)).and_then(|w| w.upgrade());
            if let Some(view) = view {
                view.load(app);
                log::info!("mini v1 timing: {pid}/{aid} {kind} load {} ms", t_load.elapsed().as_millis());
            }
        });
    });
    V1Open { slot, n: id }
}
