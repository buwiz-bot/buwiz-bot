//! Windows session and UI bridge for v1 mini apps.

use std::{
    cell::RefCell,
    collections::HashMap,
    path::PathBuf,
    rc::{Rc, Weak},
    sync::{Arc, Mutex, atomic::{AtomicBool, AtomicIsize, Ordering}},
};

use buwiz_agent::OnceId;
use nsh_platform_windows::{AppView, windows::{
    core::{w, HSTRING, PCWSTR},
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::{Shell::ShellExecuteW, WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, HWND_MESSAGE, PostMessageW, RegisterClassW,
            SW_SHOWNORMAL, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WNDCLASSW,
        }},
    },
}};
use serde_json::Value;

use crate::agent_hub::hold::{Answer, Card};
use super::{
    MiniError, V1Key,
    attribution,
    bridge::{self, Consent, Emit, HostUi, OpenContext, PickRequest, Press},
    cards::{CardAction, CardMeta},
    document, host_api, load, load::Prepared, screens,
    session::{self, Ended, Session},
};

const CALL: u32 = WM_APP + 1;
static DISPATCHER: AtomicIsize = AtomicIsize::new(0);

/// Called by AppView's component-registration hook on the UI thread.
pub fn init() {
    if DISPATCHER.load(Ordering::Acquire) != 0 {
        return;
    }
    // SAFETY: the class and message-only window live on nsh's UI thread.
    unsafe {
        let instance = GetModuleHandleW(None).expect("application module");
        let class = WNDCLASSW {
            lpfnWndProc: Some(dispatch),
            hInstance: instance.into(),
            lpszClassName: w!("BuwizUiDispatcher"),
            ..Default::default()
        };
        RegisterClassW(&class);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(), class.lpszClassName, w!(""), WINDOW_STYLE::default(),
            0, 0, 0, 0, Some(HWND_MESSAGE), None, Some(instance.into()), None,
        ).expect("UI dispatcher");
        DISPATCHER.store(hwnd.0 as isize, Ordering::Release);
    }
}

/// Posts to the same Windows message loop AppView uses.
pub fn on_main(f: impl FnOnce() + Send + 'static) {
    let hwnd = DISPATCHER.load(Ordering::Acquire);
    if hwnd == 0 {
        return log::error!("mini v1: UI dispatcher is not initialized");
    }
    let job: Box<Box<dyn FnOnce() + Send>> = Box::new(Box::new(f));
    let raw = Box::into_raw(job);
    // SAFETY: the dispatcher owns the box after a successful post.
    unsafe {
        if PostMessageW(Some(HWND(hwnd as *mut _)), CALL, WPARAM(0), LPARAM(raw as isize)).is_err() {
            drop(Box::from_raw(raw));
        }
    }
}

unsafe extern "system" fn dispatch(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == CALL {
        // SAFETY: only on_main sends CALL, transferring this box.
        let job = unsafe { Box::from_raw(lparam.0 as *mut Box<dyn FnOnce() + Send>) };
        job();
        return LRESULT(0);
    }
    // SAFETY: unhandled messages use the system procedure.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// The backend's picker is private; no selection is safer than a shell chooser.
pub fn pick_image(then: impl FnOnce(Option<PathBuf>) + Send + 'static) {
    on_main(move || then(None));
}

fn open_in_browser(url: &str) {
    let url = HSTRING::from(url);
    // SAFETY: strings outlive the call; no shell command is parsed.
    let result = unsafe { ShellExecuteW(None, w!("open"), PCWSTR(url.as_ptr()), None, None, SW_SHOWNORMAL) };
    if result.0 as isize <= 32 {
        log::warn!("mini v1: Windows could not open the URL");
    }
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


struct WindowsUi {
    key: String,
    title: String,
    n: u64,
    surface: u64,
    inline: bool,
}

impl HostUi for WindowsUi {
    attribution::host_methods!(surface);

    fn consent(&self, ask: Consent, answer: Box<dyn FnOnce(bool) + Send>) {
        match CONSENT.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            Some(f) => f(ask, answer),
            None => answer(false),
        }
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
        CHAT.lock().unwrap_or_else(|e| e.into_inner()).clone().is_some_and(|c| c.has_chat(&self.key))
    }

    fn attach(&self, token: &str, title: &str) -> bool {
        CHAT.lock().unwrap_or_else(|e| e.into_inner()).clone().is_some_and(|c| c.attach(&self.key, token, title))
    }

    fn pick_files(&self, _pick: PickRequest, answer: Box<dyn FnOnce(Vec<PathBuf>) + Send>) {
        on_main(move || answer(Vec::new()));
    }

    fn copy(&self, text: &str) {
        let text = text.to_owned();
        super::to_store(move |_| nsh_core::prelude::set_clipboard(text));
    }

    fn plugin_folder(&self, plugin: &str) -> Option<PathBuf> {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
        document::folder_of(home.as_deref(), plugin)
    }

    fn hands_off(&self) -> bool {
        !self.inline
    }

    fn file_refused(&self, said: &'static str) {
        super::to_store(move |_| crate::toast::show(said));
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
        VIEWS.with_borrow_mut(|v| v.remove(&n));
        crate::agent_hub::closed(n);
        super::to_store(move |st| st.v1_stopped(n));
    }
}

/// What an open's failure shows (6.7).
fn shown(prepared: &Prepared, e: &MiniError) -> String {
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
    crate::agent_hub::opened(id);
    let channel = view.host();
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
        let ui = Arc::new(WindowsUi { key: key.clone(), title: p.title.clone(), n: id, surface: 0, inline });
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
                nsh_platform_windows::App::Js(bundle)
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
                match screens::mount_from(&ctx, viewport, fail, super::cards::origin(&key)) {
                    Ok(m) => {
                        *s.emit.lock().unwrap_or_else(|e| e.into_inner()) = Some(m.emit);
                        nsh_platform_windows::App::Rust(m.root)
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
        let (fingerprint, version) = (prepared.fingerprint.clone(), prepared.plugin.version.clone());
        super::to_store(move |st| st.v1_running(id, &key, &fingerprint, &version));
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
