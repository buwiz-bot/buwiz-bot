//! The macOS side of a v1 open: the [`HostUi`] the bridge asks (the file
//! dialog, the pasteboard, the browser, the full-screen stack), and the
//! open's start, from the session (on a thread of its own: it waits on the
//! broker and the plugin) to the app's load on the main thread.
//!
//! Consent and the chat an attachment goes to are part B's: until it sets
//! them ([`set_consent`], [`set_chat`]), consent is declined and there is
//! no chat, so `rox.open({url})` and `rox.attach` fail closed.

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

use nsh_platform_macos::{
    AppView,
    objc2::{
        DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
        rc::Retained,
        runtime::{AnyClass, AnyObject, NSObject, NSObjectProtocol},
    },
    objc2_foundation::{NSArray, NSString, NSURL},
};
use serde_json::Value;

use super::{
    RoxError, V1Key,
    bridge::{self, Consent, HostUi, OpenContext, PickRequest},
    host_api, load,
    load::Prepared,
    session::{self, Ended, Session},
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

/// The person, as one open of `key` (its module key) reaches them.
pub struct MacUi {
    key: String,
    title: String,
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

    fn session_ended(&self, _: &V1Key, why: Ended) {
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
}

impl V1Open {
    /// Ends the open: its session (also one still opening) closes.
    pub fn close(&self) {
        self.slot.closed.store(true, Ordering::SeqCst);
        if let Some(ctx) = self.slot.ctx.lock().unwrap_or_else(|e| e.into_inner()).take() {
            ctx.close();
        }
    }

    /// The channel's viewport event, once the bridge is up.
    pub fn viewport(&self, view: &AppView, viewport: Value) {
        if self.slot.ctx.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
            let _ = view.host().send("rox.viewport", viewport);
        }
    }
}

impl Drop for V1Open {
    fn drop(&mut self) {
        self.close();
        let n = self.n;
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
    let channel = view.host();
    let slot = Arc::new(Slot::default());
    let s = slot.clone();
    std::thread::spawn(move || {
        session::listen_for_generations();
        let broker: Arc<dyn host_api::Broker> = Arc::new(host_api::HttpBroker::configured());
        let p = &prepared;
        let opened = Session::open(&*broker, &p.plugin.id, &p.key.app, p.permissions_json.as_ref(), p.plugin.generation);
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
        let ui = Arc::new(MacUi { key: key.clone(), title: p.title.clone() });
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
        bridge::install(&channel, &ctx, viewport);
        let (js, policy) = (prepared.js.clone(), prepared.permissions.policy());
        let fingerprint = prepared.fingerprint.clone();
        super::to_store(move |st| st.v1_running(id, &key, &fingerprint));
        on_main(move || {
            let view = VIEWS.with_borrow_mut(|v| v.remove(&id)).and_then(|w| w.upgrade());
            if let Some(view) = view {
                let bundle = nsh_engine::Bundle::source(js).with_policy(policy);
                view.load(nsh_platform_macos::App::Js(bundle));
            }
        });
    });
    V1Open { slot, n: id }
}
