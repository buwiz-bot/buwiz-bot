//! The mini-app host: Grok Shell's generated widgets, compiled on demand
//! and run as real apps inside the chat.
//!
//! A widget is a `generated/R<n>.tsx` module (`export default function
//! R()`); `GET /module?name=` returns its source. [`compile_widget`]
//! wraps it with `mount(R)` (the same wrapper `pipeline.mjs`'s chunk
//! emits) and hands it to `nsh_bundler::compile_module` — TypeScript
//! stripped, JSX compiled, `nsh:*` imports left for the runtime.
//!
//! The element `native("rox/mini-app")` mounts a registered native
//! component whose NSView *is* an `AppView`: each mini app gets its own
//! engine + QuickJS inside the Rust tree — the pure-Rust root is untouched.
//! `mini_bundle` selects a deny-all policy for generated widgets and an
//! app-server-only policy for `mcp:` modules. The `source` prop carries the
//! module key and compiled JS; a changed pair reloads the app.
//!
//! A v1 plugin's app (`v1:<plugin id>/<app id>`, `crate::mini_v1`) gets an
//! `AppView` of its own per open instead: its own `nsh/host` channel with
//! the bridge's handlers, its own engine storage, the spec's budgets, and
//! the policy its permissions compile to. It loads once its session is
//! open.

use nsh_core::JsonValue;
use nsh_core::prelude::*;

use crate::live::store;

/// Whether the live mini apps placed under it can't be seen now: a
/// chat row out of the viewport or under a full-screen app, a full-screen
/// app under another. Provided by what places apps; [`mini_app`] holds
/// its app (frames and timers) while it reads true.
#[derive(Clone)]
pub(crate) struct OutOfSight(pub(crate) std::rc::Rc<dyn Fn() -> bool>);

/// A turn card's live widget: `app` is the module name (`R<n>`); `h` the
/// card body's height, or `<= 0` to fill its parent. The `source` prop
/// updates as the store compiles. `design` is the size the app lays out
/// at (its declared `w×h`): the element's box shows it scaled uniformly
/// to fit, as the TSX card scales an oversize app (shell.tsx:911).
/// `on_natural` hears the app's natural content height as it changes.
pub(crate) fn mini_app(
    app: &str,
    h: f32,
    design: Option<std::rc::Rc<dyn Fn() -> (f32, f32)>>,
    on_natural: Option<std::rc::Rc<dyn Fn(f32)>>,
) -> impl IntoElement {
    let name = app.to_owned();
    let name2 = app.to_owned();
    let hidden = use_context::<OutOfSight>();
    native("rox/mini-app")
        .prop("paused", move || JsonValue::Bool(hidden.as_ref().is_some_and(|h| (h.0)())))
        .prop("design", move || match &design {
            Some(d) => {
                let (w, h) = d();
                serde_json::json!([w, h])
            }
            None => JsonValue::Null,
        })
        .on_native_event(move |e: &NativeEvent| {
            if &*e.name == "natural" {
                if let (Some(f), Some(h)) = (&on_natural, e.data.as_f64()) {
                    f(h as f32);
                }
            }
        })
        .prop("source", move || {
            serde_json::json!({
                "module": name.clone(),
                "js": store().mini_source(&name).get(),
            })
        })
        .prop(
            "app",
            move || JsonValue::String(store().mini_source_title(&name2).get()),
        )
        .prop("dev_type", || JsonValue::String(store().mini_dev_type.get()))
        // every mini app follows Rox's theme (`mini_appearance`)
        .prop("theme", || JsonValue::String(store().theme_pref.get()))
        .w_full()
        .when(h > 0.0, |v| v.h(px(h)))
        .when(h <= 0.0, |v| v.flex_1())
}

/// Where an inline card's hold is (`mini_v1::settle`): nothing over the
/// held app at first, the placeholder once the hold is long enough to
/// see, its fade once the app shows, then nothing again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hold {
    Quiet,
    Placeholder,
    Fading,
    Done,
}

/// A v1 app inline in a chat (spec 6.5): `w×h`, its viewport saying
/// `inline`. `h` is read live (the app's `rox.fit` moves it): the view
/// resizes in place, and the app lays out at the new size. `h` is also
/// the height the card keeps while its app is held (its last `rox.fit`,
/// else the manifest's): the app stays hidden until its layout settles,
/// with `title`'s placeholder over it past 100 ms, then shows.
pub(crate) fn mini_app_inline(app: &str, w: f32, h: impl Fn() -> f32 + 'static, title: String) -> impl IntoElement {
    use crate::mini_v1::settle::{self, Reveal};
    let name = app.to_owned();
    let name2 = app.to_owned();
    let name3 = app.to_owned();
    let hidden = use_context::<OutOfSight>();
    let hold = signal(Hold::Quiet);
    let owner = Owner::current();
    let h = std::rc::Rc::new(h);
    let h2 = h.clone();
    let app_view = native("rox/mini-app")
        .prop("placement", || JsonValue::String("inline".to_owned()))
        .prop("theme", || JsonValue::String(store().theme_pref.get()))
        .prop("paused", move || JsonValue::Bool(hidden.as_ref().is_some_and(|h| (h.0)())))
        .prop("source", move || {
            serde_json::json!({
                "module": name.clone(),
                "js": store().mini_source(&name).get(),
            })
        })
        .prop("app", move || JsonValue::String(store().mini_source_title(&name2).get()))
        // the open whose app called `rox.ready()`
        .prop("ready", move || store().v1.ready.with(|r| r.get(&name3).map_or(JsonValue::Null, |n| (*n).into())))
        .on_native_event(move |e: &NativeEvent| match (&*e.name, hold.get_untracked()) {
            ("placeholder", Hold::Quiet) => hold.set(Hold::Placeholder),
            // settled before the placeholder was due: nothing to take down
            ("settled", Hold::Quiet) => hold.set(Hold::Done),
            ("settled", Hold::Placeholder) => match settle::reveal(reduce_motion()) {
                Reveal::Swap => hold.set(Hold::Done),
                Reveal::Fade => {
                    hold.set(Hold::Fading);
                    owner.run(|| {
                        set_timeout(std::time::Duration::from_millis(settle::FADE_MS as u64), move || hold.set(Hold::Done))
                    });
                }
            },
            _ => {}
        })
        .w(px(w))
        .h(move || px(h()));
    let key = format!("settle:{app}");
    {
        // the grid's progress counts from this hold, not from one before
        let (k1, k2) = (key.clone(), key.clone());
        effect(move || {
            if hold.get() == Hold::Done {
                store().art_started.borrow_mut().remove(&k1);
            }
        });
        cleanup(move || {
            store().art_started.borrow_mut().remove(&k2);
        });
    }
    view().w(px(w)).h(move || px(h2())).child(app_view).child(when(
        move || matches!(hold.get(), Hold::Placeholder | Hold::Fading),
        move || {
            // made shown, then faded by flipping its opacity: a transition
            // does not start from the default style
            let grid = if reduce_motion() {
                crate::dotgrid::dot_grid_still(key.clone()).into_element()
            } else {
                crate::dotgrid::dot_grid(key.clone(), crate::dotgrid::TAU_APP).into_element()
            };
            crate::chat::app_placeholder(title.clone(), String::new(), view().flex_1().child(grid))
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0))
                .bg(crate::theme::CARD)
                .pointer_events(PointerEvents::None)
                .opacity(move || if hold.get() == Hold::Fading { 0.0f32 } else { 1.0 })
                .transition_opacity()
                .duration(settle::FADE_MS)
                .into_element()
        },
    ))
}

/// The appearance every mini app (generated `R<n>` widgets, v0 `mcp:` and
/// v1 plugin apps, full screen and in cards) takes under Rox's theme
/// `pref`: `Some(true)` dark, `Some(false)` light, `None` the system's
/// (the window's effective appearance) for System or anything else.
pub(crate) fn mini_appearance(pref: &str) -> Option<bool> {
    match pref {
        "dark" => Some(true),
        "light" => Some(false),
        _ => None,
    }
}

/// `source` with its imports moved to where nsh 0.3 serves them (it keeps
/// no aliases): `nsh/ios` and `nsh/android` are `nsh/mobile`,
/// `nsh/ui/mobile` is `nsh/base`, the other submodules (`nsh/canvas`,
/// `nsh/image`, `nsh/keymap`, `nsh/clipboard`, …) are `nsh`, and a name
/// `nsh/base` exports (`Input`, `Button`, …) comes from there. The widgets
/// the server generated before, and its prompts, still use the old homes.
pub(crate) fn current_homes(source: &str) -> String {
    thread_local! {
        static BASE: Vec<String> = {
            let d = nsh_core::element::description::SetDescription::from(nsh_base::registry());
            d.exports
                .into_iter()
                .map(|e| e.name)
                .chain(d.reexported_values)
                .chain(d.native_values)
                .collect()
        };
    }
    let mut out = String::with_capacity(source.len() + 64);
    let mut rest = source;
    while let Some(at) = rest.find("import") {
        let starts = at == 0 || rest[..at].ends_with(['\n', ';', ' ', '\t']);
        let Some(stmt) = starts.then(|| import_at(&rest[at..])).flatten() else {
            out.push_str(&rest[..at + 6]);
            rest = &rest[at + 6..];
            continue;
        };
        out.push_str(&rest[..at]);
        out.push_str(&BASE.with(|base| rehome(&stmt, base)));
        rest = &rest[at + stmt.len..];
    }
    out.push_str(rest);
    out
}

/// An `import … from "nsh…";` statement at the start of `s`.
struct Import {
    /// Its length in `s`, the `;` included.
    len: usize,
    /// What is imported: `{ a, b as c }`, `* as x` or a default name.
    clause: String,
    spec: String,
}

fn import_at(s: &str) -> Option<Import> {
    let from = s.find(" from ")?;
    let clause = s[6..from].trim();
    // an `import` keyword of another statement, or a side-effect import
    if clause.is_empty() || clause.contains(';') || clause.starts_with('(') {
        return None;
    }
    let tail = s[from + 6..].trim_start();
    let quote = tail.chars().next().filter(|q| *q == '"' || *q == '\'')?;
    let close = tail[1..].find(quote)? + 1;
    let spec = &tail[1..close];
    if spec != "nsh" && !spec.starts_with("nsh/") {
        return None;
    }
    let mut len = s.len() - tail.len() + close + 1;
    if s[len..].starts_with(';') {
        len += 1;
    }
    Some(Import { len, clause: clause.to_owned(), spec: spec.to_owned() })
}

/// The statement(s) importing `stmt`'s names from their current homes.
fn rehome(stmt: &Import, base: &[String]) -> String {
    let module = match stmt.spec.as_str() {
        s @ ("nsh/base" | "nsh/mobile" | "nsh/desktop") => s,
        "nsh/ios" | "nsh/android" => "nsh/mobile",
        "nsh/ui/mobile" => "nsh/base",
        _ => "nsh",
    };
    let names = match stmt.clause.strip_prefix('{').and_then(|c| c.strip_suffix('}')) {
        Some(names) if module == "nsh" => names,
        _ => return format!("import {} from \"{module}\";", stmt.clause),
    };
    let (mut core, mut moved) = (Vec::new(), Vec::new());
    for name in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        let own = name.trim_start_matches("type ").split(" as ").next().unwrap_or(name).trim();
        if base.iter().any(|b| b == own) { moved.push(name) } else { core.push(name) }
    }
    let mut out = String::new();
    if !core.is_empty() {
        out.push_str(&format!("import {{ {} }} from \"nsh\";", core.join(", ")));
    }
    if !moved.is_empty() {
        out.push_str(&format!("import {{ {} }} from \"nsh/base\";", moved.join(", ")));
    }
    out
}

/// TSX → the module the runtime runs: the source plus the wrapper
/// `pipeline.mjs` writes into every emitted chunk.
pub(crate) fn compile_widget(name: &str, source: &str) -> Result<String, String> {
    let source = current_homes(source);
    let wrapped = format!("{source}\nimport {{ mount }} from \"nsh\";\nmount(R);\n");
    let sets: Vec<nsh_core::element::description::SetDescription> = vec![
        nsh_base::registry().into(),
        crate::registry().into(),
    ];
    // a server's app name (`mcp:<server>/<app>|<props>`) is no file name
    let file: String = name
        .chars()
        .take_while(|&c| c != '|')
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
        .collect();
    nsh_bundler::compile_module(&format!("{file}.tsx"), &wrapped, &sets)
}

/// `GET /module`'s answer: the module's source, or why there is none. The
/// server answers `{source: "", error}` when the plugin has no such app or
/// is off; its `error` is what the person sees, not "empty module".
pub(crate) fn module_source(answer: &serde_json::Value) -> Result<String, String> {
    let text = |k: &str| answer.get(k).and_then(|x| x.as_str()).unwrap_or_default().trim();
    match (text("source"), text("error")) {
        (source, _) if !source.is_empty() => Ok(answer["source"].as_str().unwrap_or(source).to_owned()),
        (_, error) if !error.is_empty() => Err(error.to_owned()),
        _ => Err("the server sent an empty module".to_owned()),
    }
}

/// What [`compile_widget`]'s error shows: its first line that says
/// something.
pub(crate) fn compile_error(error: &str) -> String {
    let first = error.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("no details");
    format!("it failed to compile: {first}")
}

thread_local! {
    /// The full-screen apps ✕ / ⌘W / ⌘[ took off, latest last: ⌘] puts
    /// them back, until another app opens.
    static FORWARD: std::cell::RefCell<Vec<crate::store::MiniOpen>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// `open` was taken off the stack: ⌘] can put it back.
pub(crate) fn left(open: crate::store::MiniOpen) {
    FORWARD.with_borrow_mut(|f| f.push(open));
}

/// An app opened anew: nothing to go forward to.
pub(crate) fn forget_forward() {
    FORWARD.with_borrow_mut(Vec::clear);
}

/// ⌘[ / ⌘] over the full-screen apps, as the desktop's Back and Forward:
/// back takes the top app off (Tax Profile back to the Forms under it, the
/// last one back to the chat), forward reopens what back left. False when
/// there's nothing to step through here (the chat's history then).
pub(crate) fn step(d: i64) -> bool {
    let st = store();
    if d < 0 {
        if st.mini_full.with_untracked(Vec::is_empty) {
            return false;
        }
        st.close_mini();
        return true;
    }
    let Some(next) = FORWARD.with_borrow_mut(Vec::pop) else { return false };
    // reopening clears what's ahead; keep the rest for the next ⌘]
    let rest = FORWARD.with_borrow_mut(std::mem::take);
    st.open_mini_token(&next.key, &next.title, &next.token);
    FORWARD.with_borrow_mut(|f| *f = rest);
    true
}

/// The mini apps an MCP server ships, full screen over the window
/// (`store.mini_full`), each over the one it was opened from: a title bar
/// and the app filling the rest. A server opens one from a tool result
/// (`rox/open`) or a `rox://app/…` link. The apps under the top one stay
/// mounted, so ✕ comes back to them as they were.
pub(crate) fn full_layer() -> impl IntoElement {
    each(|| store().mini_full.get(), |o| o.n, |o, _| full_app(o.get_untracked()))
}

/// Stack entry `n`'s title (tracked): its opener's, or its token's
/// once resolved; "" for the app's own.
fn open_title(n: u64) -> String {
    store().mini_full.with(|v| v.iter().find(|o| o.n == n).map(|o| o.title.clone()).unwrap_or_default())
}

/// One full-screen app.
fn full_app(open: crate::store::MiniOpen) -> impl IntoElement {
    use crate::theme::tc;
    let crate::store::MiniOpen { key, token, n, .. } = open;
    // under another app of the stack, it holds
    provide_context(OutOfSight(std::rc::Rc::new(move || {
        store().mini_full.with(|v| v.last().map(|o| o.n)) != Some(n)
    })));
    let k2 = key.clone();
    let k3 = key.clone();
    view()
        .absolute()
        .size_full()
        .bg(tc("#161616"))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .h(px(52.0))
                .pl(px(84.0))
                .pr(px(12.0))
                .border_b(px(0.5))
                .border_b_color(tc("#f5f5f51a"))
                .child(
                    view()
                        .pointer_events(PointerEvents::None)
                        .absolute()
                        .left(px(84.0))
                        .right(px(84.0))
                        .top(px(0.0))
                        .bottom(px(0.0))
                        .items_center()
                        .justify_center()
                        .child(
                            // the opener's title (a resolved return's), its token's
                            // once resolved, else the app's
                            text(move || {
                                let t = open_title(n);
                                if !t.is_empty() { t } else { store().mini_source_title(&k2).get() }
                            })
                            .number_of_lines(1)
                            .text_size(px(14.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(tc("#f5f5f5")),
                        ),
                )
                .child(view().flex_1())
                .child(pin_button(&key, n))
                .when(crate::slash::attachable(&token), |v| v.child(attach_button(&key, n, &token)))
                .child(
                    view()
                        .label("Close")
                        .size(px(32.0))
                        .rounded(px(16.0))
                        .items_center()
                        .justify_center()
                        .bg(tc("#f5f5f50f"))
                        .child(text("✕").text_size(px(13.0)).text_color(tc("#f5f5f5a8")))
                        .on_press(|| store().close_mini()),
                ),
        )
        .child(
            view()
                .flex_1()
                .child(when(
                    move || !store().mini_err(&k3).get().is_empty(),
                    {
                        let k = key.clone();
                        move || {
                            let (k, k2) = (k.clone(), k.clone());
                            // the plugin's own words, under which app of
                            // which plugin failed
                            view()
                                .flex_1()
                                .items_center()
                                .justify_center()
                                .px_6()
                                .child(
                                    view()
                                        .max_w(px(560.0))
                                        .gap_2()
                                        .child(
                                            text(move || {
                                                if crate::links::app_key_parts(&k2).is_none() {
                                                    return "Couldn't open this app".to_owned();
                                                }
                                                let (app, plugin) = store()
                                                    .mcp_manifest
                                                    .with(|ms| crate::links::app_names(&k2, ms));
                                                format!("Couldn't open {app} from {plugin}")
                                            })
                                            .text_size(px(15.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(tc("#f5f5f5")),
                                        )
                                        .child(
                                            text(move || store().mini_err(&k).get())
                                                .number_of_lines(8)
                                                .text_size(px(13.0))
                                                .text_color(tc("#f5f5f5c8")),
                                        ),
                                )
                                .into_element()
                        }
                    },
                ))
                .child(crate::mini_v1::ui::banners(&key, true))
                .child(mini_app(&key, 0.0, None, None))
                .child(crate::mini_v1::ui::error_overlay(&key)),
        )
}

/// The full-screen app's Attach (beside Pin, for an app opened from a
/// token its server lets attach): the palette over the chats and bots;
/// the one picked gets the token's card, and the server's tools there
/// default to it.
fn attach_button(key: &str, n: u64, token: &str) -> impl IntoElement {
    use crate::theme::tc;
    let (key, token) = (key.to_owned(), token.to_owned());
    view()
        .label("Attach to chat or bot…")
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .h(px(32.0))
        .px(px(12.0))
        .rounded(px(16.0))
        .bg(tc("#f5f5f50f"))
        .child(crate::icons::icon_ink("chainLink", 14.0, || tc("#f5f5f5a8")))
        .child(text("Attach").text_size(px(13.0)).text_color(tc("#f5f5f5a8")))
        .on_press(move || {
            let title = untracked(|| open_title(n));
            let title = if title.is_empty() { store().mini_source_title(&key).get_untracked() } else { title };
            crate::slash::attach_picker(&token, &title);
        })
}

/// The full-screen app's Pin: its groups to put it in or take it out of,
/// or a new one (a group per client, say). A pin keeps the app's module
/// name — its `rox://app/` link — and its title; the sidebar opens it
/// from there.
fn pin_button(key: &str, n: u64) -> impl IntoElement {
    use crate::theme::tc;
    // a v1 app's pin is Rox's own: {plugin, app, ticket?, placement} (12.3)
    if crate::mini_v1::is_v1(key) {
        return crate::mini_v1::ui::pin_button(key, n).into_element();
    }
    let node = element_ref();
    let key = key.to_owned();
    let k2 = key.clone();
    let pinned = move || {
        store().groups.with(|gs| gs.iter().any(|g| g.items.iter().any(|it| it.kind == "app" && it.id == k2)))
    };
    view()
        .element_ref(&node)
        .label("Pin to group")
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .h(px(32.0))
        .px(px(12.0))
        .rounded(px(16.0))
        .bg(tc("#f5f5f50f"))
        .child(dynamic(move || {
            crate::icons::icon_ink(if pinned() { "pinFilled" } else { "pin" }, 14.0, || tc("#f5f5f5a8"))
        }))
        .child(text("Pin").text_size(px(13.0)).text_color(tc("#f5f5f5a8")))
        .on_press(move || {
            // the title the app shows now (it may have set its own)
            let title = untracked(|| open_title(n));
            let shown = store().mini_source_title(&key).get_untracked();
            let title = if title.is_empty() { shown } else { title };
            let items = crate::ctx::group_picks("app", &key, &title);
            Owner::detached().run(|| {
                spawn(async move {
                    if let Some(m) = node.measure().await {
                        let (x, y, h) = (m.page.x.0, m.page.y.0, m.frame.size.height.0);
                        crate::ctx::open_near(x, y, y + h, items);
                    }
                })
            });
        })
    .into_element()
}

/// Denies capability-gated access for generated widgets: network, web views,
/// clipboard, picker, status bar, open URLs, host functions, files, processes
/// and sockets. NSH does not yet gate `nsh/desktop` window/menu commands or
/// `os.homedir()`.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn widget_policy() -> nsh_engine::Policy {
    nsh_engine::Policy {
        declared: Some(nsh_core::access::Capabilities::default()),
        ..nsh_engine::Policy::default()
    }
}

/// The app server's routes an MCP app may call: its tools (`/mcp/call`),
/// another app or a new chat (`/mcp/open`) and the file picker
/// (`/attach/pick`), all POST.
#[cfg(any(target_os = "macos", test))]
const MCP_APP_POSTS: [&str; 3] = ["/mcp/call", "/mcp/open", "/attach/pick"];

/// What an MCP app may read: generated files and the asset base's images.
#[cfg(any(target_os = "macos", test))]
const MCP_APP_GETS: [&str; 2] = ["/file/raw/", "/asset/generated/assets/"];

/// MCP apps call the server for their data; allow only the routes they use
/// on its HTTP origin (`client::backend()`), and no other capabilities. Server
/// settings, `/data/*`, `/state/*` and the chats stay out of reach. NSH
/// checks a WebSocket as a GET of the same URL, so only the read prefixes
/// would pass one; the server has no WebSocket routes.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn mcp_app_policy() -> Result<nsh_engine::Policy, String> {
    let (host, port) = mcp_app_server_host_port()?;
    let host = nsh_core::access::HostPattern::new(&host)?;
    let grant = |method: &str| {
        let mut grant = nsh_core::access::HttpGrant::default();
        grant.host = host.clone();
        grant.methods = vec![method.into()];
        grant.schemes = vec!["http".into()];
        grant.port = Some(port);
        grant
    };
    let mut posts = grant("POST");
    posts.paths = MCP_APP_POSTS.iter().map(|p| (*p).to_owned()).collect();
    let mut gets = grant("GET");
    gets.path_prefixes = MCP_APP_GETS.iter().map(|p| (*p).to_owned()).collect();

    let mut declared = nsh_core::access::Capabilities::default();
    declared.network.http.push(posts);
    declared.network.http.push(gets);
    Ok(nsh_engine::Policy {
        declared: Some(declared),
        ..nsh_engine::Policy::default()
    })
}

#[cfg(any(target_os = "macos", test))]
fn mcp_app_server_host_port() -> Result<(String, u16), String> {
    let origin = nsh_core::access::parse_url(crate::client::backend())
        .ok_or_else(|| "invalid app server origin".to_owned())?;
    if origin.scheme != "http" {
        return Err("app server origin must use http".to_owned());
    }
    let port = origin
        .port
        .ok_or_else(|| "app server origin must specify a port".to_owned())?;
    Ok((origin.host, port))
}

/// The module key, not the server-supplied title, selects the bundle policy.
/// Any origin-configuration error for an MCP app falls back to deny-all.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn mini_bundle(module: &str, js: String) -> nsh_engine::Bundle {
    let policy = if module.starts_with("mcp:") {
        mcp_app_policy().unwrap_or_else(|_| widget_policy())
    } else {
        widget_policy()
    };
    nsh_engine::Bundle::source(js).with_policy(policy)
}

// ---------------------------------------------------------------------------
// The macOS component: an AppView per element, loaded from the `source` prop.
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod imp {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use nsh_core::JsonValue;
    use nsh_platform_macos::{
        AppOptions, AppView, ComponentContext, NativeComponent,
        objc2::{
            DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
            rc::Retained,
            ClassType,
            runtime::{AnyObject, NSObject, NSObjectProtocol},
            sel,
        },
        objc2_app_kit::{
            NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
            NSAutoresizingMaskOptions, NSView,
            NSScreen, NSWindow, NSWindowDidChangeOcclusionStateNotification,
            NSWindowDidMoveNotification, NSWindowOcclusionState,
        },
        objc2_foundation::{NSNotificationCenter, NSPoint, NSRect, NSSize},
    };

    /// How often a live mini app's natural height is checked (frame reads
    /// only; an event goes out only when it changed).
    const MEASURE_EVERY: f64 = 0.4;

    pub struct FitIvars {
        /// Consecutive ticks spent collapsed (see `collapsed`).
        collapsed_ticks: Cell<u32>,
        design: Cell<(f64, f64)>,
        natural: Cell<f64>,
        alive: Cell<bool>,
        ctx: RefCell<Option<ComponentContext>>,
        /// The element's `paused` prop: the app is out of sight in Rox.
        held: Cell<bool>,
        /// Its window can't be seen (minimized, covered, on another
        /// space or workspace).
        unseen: Cell<bool>,
        /// The window whose occlusion it watches.
        watching: Cell<usize>,
        app: RefCell<Option<Rc<AppView>>>,
        /// The element's `placement` prop (`full` or `inline`), for a v1
        /// app's `rox.viewport`.
        placement: RefCell<String>,
        /// Rox's theme (`theme` prop): the app follows it
        /// (`mini_appearance`).
        theme: RefCell<String>,
        /// A v1 open waiting for its first frame: its module and when its
        /// load began (the timing log, 14.6).
        first_frame: RefCell<Option<(String, std::time::Instant)>>,
        /// The size last sent as a v1 app's `rox.viewport`.
        viewport: Cell<(f64, f64)>,
        /// Hears the size a v1 app lays out at when it changes.
        on_viewport: RefCell<Option<Box<dyn Fn(serde_json::Value)>>>,
        /// An inline card's hold until its app's layout settles
        /// (`mini_v1::settle`): armed by the mount, not by the app's loads.
        settle: RefCell<Option<crate::mini_v1::settle::Settle>>,
    }

    define_class!(
        /// The element's view: holds the mini app at its design size and
        /// keeps its bounds at that size whatever frame layout gives it,
        /// so AppKit scales the app — drawing and hit-testing alike.
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[ivars = FitIvars]
        #[name = "RoxMiniFitView"]
        struct FitView;

        unsafe impl NSObjectProtocol for FitView {}

        impl FitView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }

            #[unsafe(method(setFrameSize:))]
            fn set_frame_size(&self, size: NSSize) {
                // SAFETY: NSView's own `setFrameSize:` with its argument.
                let () = unsafe { msg_send![super(self), setFrameSize: size] };
                self.fit();
            }

            #[unsafe(method(viewDidMoveToWindow))]
            fn did_move_to_window(&self) {
                // SAFETY: NSView's own `viewDidMoveToWindow`.
                let () = unsafe { msg_send![super(self), viewDidMoveToWindow] };
                self.sync_appearance();
                self.watch_window();
                self.check_window();
            }

            #[unsafe(method(roxWindowOcclusion:))]
            fn window_changed(&self, _note: Option<&AnyObject>) {
                self.check_window();
            }

            #[unsafe(method(roxMeasure:))]
            fn measure(&self, _arg: Option<&AnyObject>) {
                if !self.ivars().alive.get() {
                    return;
                }
                self.sync_appearance();
                self.fit();
                let folded = collapsed(self);
                self.settle_tick(folded);
                if folded {
                    self.ivars().collapsed_ticks.set(self.ivars().collapsed_ticks.get() + 1);
                    self.schedule();
                    return;
                }
                self.ivars().collapsed_ticks.set(0);
                self.tell_viewport();
                // a v1 app's first frame: its views have a size
                if self.ivars().first_frame.borrow().is_some()
                    && self.subviews().firstObject().and_then(|app| natural(&app)).is_some_and(|h| h > 0.0)
                    && let Some((module, at)) = self.ivars().first_frame.borrow_mut().take()
                {
                    log::info!("mini v1 timing: {module} view to first frame {} ms (session, engine start, module, layout)", at.elapsed().as_millis());
                }
                if let Some(h) = self.subviews().firstObject().and_then(|app| natural(&app)) {
                    if (h - self.ivars().natural.get()).abs() >= 1.0 {
                        self.ivars().natural.set(h);
                        if let Some(ctx) = self.ivars().ctx.borrow().as_ref() {
                            ctx.emit("natural", serde_json::json!(h.ceil()));
                        }
                    }
                }
                self.schedule();
            }
        }
    );

    /// A window all but off every screen — where a tiling manager
    /// (AeroSpace) parks the windows of the workspaces not shown, a
    /// sliver left on screen, so AppKit still calls it visible.
    fn parked(window: &NSWindow, mtm: MainThreadMarker) -> bool {
        let f = window.frame();
        let area = f.size.width * f.size.height;
        if area <= 0.0 {
            return true;
        }
        let shown: f64 = NSScreen::screens(mtm)
            .iter()
            .map(|s| {
                let r = s.frame();
                let w = (f.origin.x + f.size.width).min(r.origin.x + r.size.width) - f.origin.x.max(r.origin.x);
                let h = (f.origin.y + f.size.height).min(r.origin.y + r.size.height) - f.origin.y.max(r.origin.y);
                w.max(0.0) * h.max(0.0)
            })
            .sum();
        shown < area * 0.02
    }

    /// The app's natural content height — what Grok gets by laying the
    /// app out with unbounded height. The app's root element(s) are the
    /// surface root's children; a root sized to its content measures as
    /// itself. A root that fills its frame (a fixed height) can have its
    /// content spill out of it: then the content's own bottom edge counts,
    /// plus the root's top inset as the matching bottom padding. Once the
    /// card grows to it, the root fills exactly that — a fixed point, so
    /// the card never steps up or down by itself.
    fn natural(container: &NSView) -> Option<f64> {
        let root = container.subviews().firstObject()?;
        let apps = root.subviews();
        if apps.count() == 0 {
            return None;
        }
        let mut bottom = 0.0f64;
        let mut budget = 4000usize;
        for app in apps.iter() {
            let f = app.frame();
            let edge = f.origin.y + f.size.height;
            bottom = bottom.max(edge);
            let (mut content, mut top) = (0.0f64, f64::MAX);
            extent(&app, f.origin.y, &mut content, &mut top, &mut budget);
            if content > edge + 0.5 {
                let pad = (top - f.origin.y).clamp(0.0, 48.0);
                bottom = bottom.max(content + pad);
            }
        }
        Some(bottom)
    }

    /// The vertical extent of `view`'s visible descendants, in its
    /// parent's coordinates (`y0` is the view's top there; nsh's views are
    /// flipped). A view that clips (a scroll view, `overflow: hidden`)
    /// counts as its own box: a long scrolling list must not grow the card.
    fn extent(view: &NSView, y0: f64, bottom: &mut f64, top: &mut f64, budget: &mut usize) {
        for sub in view.subviews().iter() {
            if *budget == 0 {
                return;
            }
            *budget -= 1;
            let f = sub.frame();
            if sub.isHidden() || f.size.height <= 0.0 || f.size.width <= 0.0 {
                continue;
            }
            let y = y0 + f.origin.y;
            *bottom = bottom.max(y + f.size.height);
            *top = top.min(y);
            if !clips(&sub) {
                extent(&sub, y, bottom, top, budget);
            }
        }
    }

    /// A hash of the origin and size of every visible view under the
    /// app's surface, while the app has a sized frame: what a settling
    /// card compares tick to tick. Unlike [`natural`] it goes into views
    /// that clip, since what pops in inside a list pops in all the same.
    fn signature(container: &NSView) -> Option<u64> {
        use std::hash::{Hash, Hasher};
        let root = container.subviews().firstObject()?;
        let mut h = std::hash::DefaultHasher::new();
        let (mut budget, mut sized) = (4000usize, false);
        frames(&root, &mut h, &mut budget, &mut sized);
        sized.then(|| {
            budget.hash(&mut h);
            h.finish()
        })
    }

    fn frames(view: &NSView, h: &mut std::hash::DefaultHasher, budget: &mut usize, sized: &mut bool) {
        use std::hash::Hash;
        let subs = view.subviews();
        subs.count().hash(h);
        for sub in subs.iter() {
            if *budget == 0 {
                return;
            }
            *budget -= 1;
            if sub.isHidden() {
                continue;
            }
            let f = sub.frame();
            *sized |= f.size.width > 0.0 && f.size.height > 0.0;
            // half points: what can be seen to move
            [f.origin.x, f.origin.y, f.size.width, f.size.height].map(|v| (v * 2.0).round() as i64).hash(h);
            frames(&sub, h, budget, sized);
        }
    }

    fn clips(view: &NSView) -> bool {
        // SAFETY: NSView's `clipsToBounds` (macOS 14+) and CALayer's
        // `masksToBounds`, both BOOL getters.
        unsafe {
            let own: bool = msg_send![view, clipsToBounds];
            if own {
                return true;
            }
            let layer: Option<Retained<AnyObject>> = msg_send![view, layer];
            layer.is_some_and(|l| {
                let masks: bool = msg_send![&*l, masksToBounds];
                masks
            })
        }
    }

    /// Whether a view above `view` has an empty frame: a branch nsh has
    /// collapsed out of layout. Nothing clips there, so the app would draw
    /// at that branch's origin — over whatever the cell shows now (a
    /// reused list cell keeps its old card's views at 0×0) — and a cell
    /// not laid out yet is collapsed for a moment too.
    fn collapsed(view: &NSView) -> bool {
        // SAFETY: reading the superview chain on the main thread.
        let mut cur = unsafe { view.superview() };
        while let Some(p) = cur {
            if p.window().is_some_and(|w| w.contentView().as_deref() == Some(&*p)) {
                return false;
            }
            let f = p.frame().size;
            if f.width < 1.0 || f.height < 1.0 {
                return true;
            }
            cur = unsafe { p.superview() };
        }
        false
    }

    impl FitView {
        fn new(mtm: MainThreadMarker, ctx: ComponentContext) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(FitIvars {
                collapsed_ticks: Cell::new(0),
                design: Cell::new((0.0, 0.0)),
                natural: Cell::new(0.0),
                alive: Cell::new(true),
                ctx: RefCell::new(Some(ctx)),
                held: Cell::new(false),
                unseen: Cell::new(false),
                watching: Cell::new(0),
                app: RefCell::new(None),
                placement: RefCell::new("full".to_owned()),
                theme: RefCell::new(String::new()),
                first_frame: RefCell::new(None),
                viewport: Cell::new((0.0, 0.0)),
                on_viewport: RefCell::new(None),
                settle: RefCell::new(None),
            });
            // SAFETY: NSView's designated `init` on a fresh allocation.
            let this: Retained<Self> = unsafe { msg_send![super(this), init] };
            this.schedule();
            this
        }

        /// Holds the app while Rox has it out of sight or its window
        /// can't be seen: its frames and timers stop (a game loop costs
        /// nothing behind other windows).
        fn hold(&self) {
            let held = self.ivars().held.get() || self.ivars().unseen.get();
            if let Some(app) = self.ivars().app.borrow().as_ref() {
                app.set_paused(held);
            }
        }

        /// Whether its window can be seen, then holds or runs the app.
        fn check_window(&self) {
            let unseen = self.window().is_none_or(|w| {
                w.isMiniaturized()
                    || !w.occlusionState().contains(NSWindowOcclusionState::Visible)
                    || parked(&w, self.mtm())
            });
            self.ivars().unseen.set(unseen);
            self.hold();
        }

        /// Hears its window's occlusion changes (once per window; the
        /// center holds observers weakly).
        fn watch_window(&self) {
            let Some(window) = self.window() else { return };
            let key = Retained::as_ptr(&window) as usize;
            if self.ivars().watching.replace(key) == key {
                return;
            }
            // SAFETY: selector-based observers; `roxWindowOcclusion:`
            // takes the notification.
            unsafe {
                let center = NSNotificationCenter::defaultCenter();
                for name in [NSWindowDidChangeOcclusionStateNotification, NSWindowDidMoveNotification] {
                    center.addObserver_selector_name_object(
                        self,
                        sel!(roxWindowOcclusion:),
                        Some(name),
                        Some(&window),
                    );
                }
            }
        }

        /// Every mini app follows Rox's theme: Light or Dark as Rox is,
        /// the window's (the system's) under Rox's System
        /// (`mini_appearance`). The window itself carries the system's, so
        /// the view sets its own; the app's surface background (what shows
        /// before it draws) follows the same scheme.
        fn sync_appearance(&self) {
            let Some(window) = self.window() else {
                return;
            };
            let want = super::mini_appearance(&self.ivars().theme.borrow());
            let appearance = match want {
                // SAFETY: immutable AppKit constants.
                Some(true) => NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua }),
                Some(false) => NSAppearance::appearanceNamed(unsafe { NSAppearanceNameAqua }),
                None => None,
            }
            .unwrap_or_else(|| window.effectiveAppearance());
            let current = self.appearance().map(|a| a.name());
            if current.as_deref() != Some(&*appearance.name()) {
                self.setAppearance(Some(&appearance));
            }
        }

        fn schedule(&self) {
            // A collapsed card checks again soon at first (a new cell is
            // laid out within a few frames), then at the usual pace (a
            // leftover from a reused cell stays collapsed).
            let n = self.ivars().collapsed_ticks.get();
            let delay = if self.ivars().first_frame.borrow().is_some() {
                // a v1 open's first frame is timed to the frame
                0.008
            } else if self.holding() {
                // a settling card reads its layout often, until it shows
                crate::mini_v1::settle::TICK.as_secs_f64()
            } else if n > 0 && n < 40 {
                0.05
            } else {
                MEASURE_EVERY
            };
            // SAFETY: `roxMeasure:` is this class's method taking one
            // object argument (nil here); NSObject's class method cancels
            // the one pending, so a kick (a load, a mount) never starts a
            // second loop.
            unsafe {
                let () = msg_send![
                    NSObject::class(),
                    cancelPreviousPerformRequestsWithTarget: self,
                    selector: sel!(roxMeasure:),
                    object: None::<&AnyObject>
                ];
                let () = msg_send![
                    self,
                    performSelector: sel!(roxMeasure:),
                    withObject: None::<&AnyObject>,
                    afterDelay: delay
                ];
            }
        }

        /// The size the app lays out at, and its placement, as a v1 app's
        /// `rox.viewport` says it.
        fn viewport(&self) -> serde_json::Value {
            let (w, h) = self.ivars().design.get();
            let (w, h) = if w > 0.0 && h > 0.0 { (w, h) } else { (self.frame().size.width, self.frame().size.height) };
            serde_json::json!({ "width": w.round(), "height": h.round(), "placement": *self.ivars().placement.borrow() })
        }

        /// Sends the viewport to a v1 app when its size changed.
        fn tell_viewport(&self) {
            let Some(f) = &*self.ivars().on_viewport.borrow() else { return };
            let v = self.viewport();
            let size = (v["width"].as_f64().unwrap_or(0.0), v["height"].as_f64().unwrap_or(0.0));
            if size != self.ivars().viewport.replace(size) {
                f(v);
            }
        }

        /// Whether an inline card holds its app hidden while it settles.
        fn holding(&self) -> bool {
            self.ivars().settle.borrow().as_ref().is_some_and(|s| s.holding())
        }

        /// Arms an inline card's hold: its mount.
        fn arm(&self) {
            if self.ivars().settle.borrow().is_some() {
                return;
            }
            *self.ivars().settle.borrow_mut() = Some(crate::mini_v1::settle::Settle::new(std::time::Instant::now()));
            self.fit();
            self.schedule();
        }

        /// A settling card's tick: its layout's signature (none while
        /// collapsed: only the cap can end that hold), then the card hears
        /// `placeholder` when the hold is long enough to see and `settled`
        /// when the app shows. A settled card skips all of it.
        fn settle_tick(&self, folded: bool) {
            use crate::mini_v1::settle::Phase;
            let phase = {
                let mut settle = self.ivars().settle.borrow_mut();
                let Some(s) = settle.as_mut().filter(|s| s.holding()) else { return };
                let sig = if folded { None } else { self.subviews().firstObject().and_then(|app| signature(&app)) };
                s.tick(std::time::Instant::now(), sig)
            };
            let Some(phase) = phase else { return };
            if phase == Phase::Settled {
                self.fit();
            }
            if let Some(ctx) = self.ivars().ctx.borrow().as_ref() {
                ctx.emit(if phase == Phase::Settled { "settled" } else { "placeholder" }, serde_json::Value::Null);
            }
        }

        /// Design size, or the frame's own size (no scaling) without one.
        fn fit(&self) {
            let frame = self.frame().size;
            // An empty frame (no scale can be set on it) or a collapsed
            // branch above: the app would draw where it should not — over
            // the transcript's top. Hide it. A settling card's app is
            // hidden too (hidden, not transparent: an autofocused field
            // can't take the focus).
            let empty = frame.width < 1.0 || frame.height < 1.0 || collapsed(self);
            if let Some(app) = self.subviews().firstObject() {
                app.setHidden(empty || self.holding());
            }
            if empty {
                return;
            }
            let (w, h) = self.ivars().design.get();
            let size = if w > 0.0 && h > 0.0 { NSSize::new(w, h) } else { frame };
            self.setBoundsSize(size);
        }
    }

    /// One mini app: its own AppView (engine + QuickJS) hosted in the
    /// element's fit view.
    pub struct MiniApp {
        fit: Retained<FitView>,
        view: Rc<AppView>,
        last: Option<(String, String)>,
        /// A v1 app's open: its session and bridge.
        v1: Option<crate::mini_v1::native::V1Open>,
    }

    impl MiniApp {
        pub fn new(ctx: ComponentContext) -> MiniApp {
            let mtm = MainThreadMarker::new().expect("a component is made on the main thread");
            let view = AppView::new(
                mtm,
                AppOptions {
                    title: "Rox mini app".to_owned(),
                    app_id: "dev.goldcoders.rox.mini".to_owned(),
                    size: (340, 200),
                    // `asset:/generated/assets/…` (generated images) are the
                    // server's files, served under /asset/.
                    asset_base: Some(nsh_platform_macos::AssetBase::Http(format!(
                        "{}/asset/",
                        crate::client::backend()
                    ))),
                    ..Default::default()
                },
            );
            // The size the layout takes when the style leaves one open.
            ctx.set_size(340.0, 200.0);
            let fit = FitView::new(mtm, ctx);
            // Nothing of the app draws outside the card's box.
            // SAFETY: NSView's `clipsToBounds` (macOS 14+), a BOOL setter.
            let () = unsafe { msg_send![&*fit, setClipsToBounds: true] };
            let app = view.view();
            app.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(340.0, 200.0)));
            app.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            fit.addSubview(&app);
            *fit.ivars().app.borrow_mut() = Some(view.clone());
            MiniApp {
                fit,
                view,
                last: None,
                v1: None,
            }
        }

        /// Runs `view` in place of the app it hosted, which stops.
        fn replace_view(&mut self, view: Rc<AppView>) {
            self.view.stop();
            self.view.view().removeFromSuperview();
            self.fit.addSubview(&view.view());
            *self.fit.ivars().app.borrow_mut() = Some(view.clone());
            self.view = view;
            let (w, h) = self.fit.ivars().design.get();
            self.set_design((w > 0.0 && h > 0.0).then_some((w, h)));
            self.fit.hold();
        }

        /// Opens v1 app `module` (prepared by the store as `js`): a view of
        /// its own, then its session, bridge and load
        /// (`mini_v1::native::start`).
        fn load_v1(&mut self, module: &str, js: &str) {
            let Some(prepared) = crate::mini_v1::prepared_js(module, js) else {
                log::warn!("mini v1: {module} was not prepared");
                // never a blank app: say so, with the store's Retry
                let key = module.to_owned();
                return crate::mini_v1::to_store(move |st| {
                    st.mini_err(&key).set("Could not open this app: it was not ready. Close it and open it again.".to_owned())
                });
            };
            // the previous open's session ends first
            self.v1.take();
            let mtm = MainThreadMarker::new().expect("a component loads on the main thread");
            let root = crate::mini_v1::plugins_dir();
            let view = AppView::new(
                mtm,
                AppOptions {
                    title: prepared.title.clone(),
                    app_id: "dev.goldcoders.rox.mini.v1".to_owned(),
                    size: (340, 200),
                    // the spec's budgets (14.2): load 10 s, a JS call 2 s,
                    // heap 256 MB, stack 4 MB
                    limits: nsh_engine::Limits {
                        memory_bytes: 256 << 20,
                        stack_bytes: 4 << 20,
                        load_budget: std::time::Duration::from_secs(10),
                        call_budget: std::time::Duration::from_secs(2),
                        ..Default::default()
                    },
                    storage: Some(crate::mini_v1::storage::engine_storage(&root, &prepared.plugin.id, &prepared.key.app)),
                    ..Default::default()
                },
            );
            self.replace_view(view);
            if let Some(s) = self.fit.ivars().settle.borrow_mut().as_mut() {
                let app = prepared.plugin.app(&prepared.key.app).cloned().unwrap_or_default();
                s.loaded(crate::mini_v1::cards::wait_ready(&app));
            }
            *self.fit.ivars().first_frame.borrow_mut() = Some((module.to_owned(), std::time::Instant::now()));
            self.fit.schedule();
            // a running app's error shows the spec's message (6.7) over it,
            // with Reload and Close
            let (key, title) = (module.to_owned(), prepared.title.clone());
            self.view.set_error_handler(move |text| {
                if let Some(message) = crate::mini_v1::lifecycle::error_message(&title, text) {
                    let key = key.clone();
                    crate::mini_v1::to_store(move |st| st.v1_error(&key, message));
                }
            });
            let channel = self.view.host();
            *self.fit.ivars().on_viewport.borrow_mut() = Some(Box::new(move |v| {
                let _ = channel.send("rox.viewport", v);
            }));
            let viewport = self.fit.viewport();
            self.v1 = Some(crate::mini_v1::native::start(prepared, &self.view, module.to_owned(), viewport));
        }

        /// Lays the app out at `w×h` and scales it into the frame; `None`
        /// fills the frame unscaled.
        fn set_design(&self, design: Option<(f64, f64)>) {
            let app = self.view.view();
            match design {
                Some((w, h)) if w > 0.0 && h > 0.0 => {
                    app.setAutoresizingMask(NSAutoresizingMaskOptions::ViewNotSizable);
                    app.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h)));
                    self.fit.ivars().design.set((w, h));
                }
                _ => {
                    self.fit.ivars().design.set((0.0, 0.0));
                    app.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), self.fit.frame().size));
                    app.setAutoresizingMask(
                        NSAutoresizingMaskOptions::ViewWidthSizable
                            | NSAutoresizingMaskOptions::ViewHeightSizable,
                    );
                }
            }
            self.fit.fit();
        }
    }

    impl Drop for MiniApp {
        fn drop(&mut self) {
            // Ends the measuring loop; the pending perform releases the view.
            self.fit.ivars().alive.set(false);
            self.fit.ivars().ctx.borrow_mut().take();
            self.fit.ivars().app.borrow_mut().take();
            self.fit.ivars().on_viewport.borrow_mut().take();
            // a v1 app's session closes with its component
            self.v1.take();
            // SAFETY: removes this view's observations, added by
            // `watch_window`.
            unsafe { NSNotificationCenter::defaultCenter().removeObserver(&self.fit) };
        }
    }

    /// The text fields under `view`, in view order.
    fn text_fields(view: &NSView, out: &mut Vec<Retained<NSView>>) {
        for sub in view.subviews().iter() {
            if sub.class().name().to_str().is_ok_and(|n| n.contains("TextField")) {
                out.push(sub.clone());
            }
            text_fields(&sub, out);
        }
    }

    /// Focuses text field `n` under `root` and inserts `text` into it.
    fn dev_type(root: &NSView, n: usize, text: &str) {
        let mut fields = Vec::new();
        text_fields(root, &mut fields);
        let (Some(field), Some(window)) = (fields.get(n), root.window()) else {
            log::warn!("dev type: no text field {n} ({} found)", fields.len());
            return;
        };
        window.makeFirstResponder(Some(field));
        let Some(editor) = window.firstResponder() else { return };
        let s = nsh_platform_macos::objc2_foundation::NSString::from_str(text);
        // SAFETY: the first responder of a focused field is its field editor (an NSTextView).
        let () = unsafe { msg_send![&*editor, insertText: &*s] };
        log::info!("dev type: {} fields, typed {text:?} into {n}", fields.len());
    }

    impl NativeComponent for MiniApp {
        fn view(&self) -> Retained<NSView> {
            Retained::into_super(self.fit.clone())
        }

        fn set_prop(&mut self, name: &str, value: Option<&JsonValue>) {
            if name == "design" {
                let size = value.and_then(|v| v.as_array()).and_then(|a| {
                    Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?))
                });
                self.set_design(size);
                return;
            }
            if name == "dev_type" {
                // dev hook (`rox-agent-menu`: `type <n> <text>`): the app's
                // n-th text field takes the focus and `text` is inserted the
                // way typing ends up (`insertText:` on the field editor)
                if let Some((n, text)) = value
                    .and_then(|v| v.as_str())
                    .and_then(|v| v.split_once('|'))
                    .and_then(|(_, rest)| rest.split_once(' '))
                {
                    dev_type(&self.fit, n.parse().unwrap_or(0), text);
                }
                return;
            }
            if name == "theme" {
                *self.fit.ivars().theme.borrow_mut() = value.and_then(|v| v.as_str()).unwrap_or_default().to_owned();
                self.fit.sync_appearance();
                return;
            }
            if name == "placement" {
                let p = value.and_then(|v| v.as_str()).filter(|p| *p == "inline").unwrap_or("full");
                *self.fit.ivars().placement.borrow_mut() = p.to_owned();
                if p == "inline" {
                    self.fit.arm();
                }
                return;
            }
            if name == "ready" {
                // `rox.ready()` of this card's open, not of one before it
                let n = value.and_then(|v| v.as_u64());
                if n.is_some() && n == self.v1.as_ref().map(|o| o.n())
                    && let Some(s) = self.fit.ivars().settle.borrow_mut().as_mut()
                {
                    s.ready();
                }
                return;
            }
            if name == "paused" {
                self.fit.ivars().held.set(value.and_then(|v| v.as_bool()).unwrap_or(false));
                self.fit.hold();
                return;
            }
            if name != "source" {
                return;
            }
            let Some(source) = value.and_then(|v| v.as_object()) else {
                return;
            };
            let module = source
                .get("module")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned();
            let js = source
                .get("js")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned();
            if js.is_empty()
                || self
                    .last
                    .as_ref()
                    .is_some_and(|(last_module, last_js)| last_module == &module && last_js == &js)
            {
                return;
            }
            self.last = Some((module.clone(), js.clone()));
            self.fit.sync_appearance();
            if crate::mini_v1::is_v1(&module) {
                return self.load_v1(&module, &js);
            }
            self.view
                .load(nsh_platform_macos::App::Js(super::mini_bundle(&module, js)));
        }
    }
}

/// Registers `rox/mini-app` on the app's component registry (call before
/// `run_app` so the first cards resolve it).
#[cfg(target_os = "macos")]
pub fn register_mini_app(components: &nsh_platform_macos::Components) {
    components.register(
        "rox/mini-app",
        std::rc::Rc::new(|ctx| {
            Box::new(imp::MiniApp::new(ctx)) as Box<dyn nsh_platform_macos::NativeComponent>
        }),
    );
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex, mpsc},
        time::{Duration, Instant},
    };

    use nsh_core::{
        ApproxTextEngine, Command, EventKind, EventPayload, MountBatch, MountItem, PayloadValue,
        ROOT_ID, desktop::DesktopCommand,
    };
    use nsh_engine::{
        App, Bundle, Engine, EngineConfig, FetchEvent, FetchTask, HttpHead, HttpRequest, MountSink,
        Network,
    };

    /// A representative generated widget: imports the same nsh face the
    /// server prompts for, `export default function R()`.
    const WIDGET: &str = r#"
import { signal, Text, View } from "nsh";

export default function R() {
  const n = signal(0);
  return (
    <View style="p-4 bg-white rounded">
      <Text>{`count ${n()}`}</Text>
      <Text onPress={() => n(n() + 1)}>bump</Text>
    </View>
  );
}
"#;

    const WIDGET_EXFIL: &str = r#"
import { signal, Text } from "nsh";
import { Clipboard } from "nsh/clipboard";

// a refused clipboard call rejects with its code (nsh: no `clipboard`
// capability, ERR_ACCESS_DENIED before the platform is asked)
const code = (e) => e.code;

export default function R() {
  const result = signal("waiting");
  Promise.all([
    fetch("http://127.0.0.1:8787/threads").then(() => "fetched", () => "refused"),
    Clipboard.hasText().catch(code),
    Clipboard.readText().catch(code),
  ]).then(([fetchOutcome, has, text]) => {
    result.set(`${fetchOutcome} ${has} [${text}]`);
  });
  return <Text>{result()}</Text>;
}
"#;

    struct Sink(mpsc::Sender<MountBatch>);

    impl MountSink for Sink {
        fn submit(&self, batch: MountBatch) {
            let _ = self.0.send(batch);
        }
    }

    struct CompletedTask;

    impl FetchTask for CompletedTask {
        fn send(&mut self, _: Vec<u8>) {}

        fn finish(&mut self) {}

        fn pause(&mut self, _: bool) {}

        fn cancel(self: Box<Self>) {}
    }

    struct RecordingNetwork {
        seen: Arc<Mutex<Vec<String>>>,
    }

    impl Network for RecordingNetwork {
        fn fetch(
            &self,
            request: HttpRequest,
            mut events: nsh_engine::FetchEvents,
        ) -> Box<dyn FetchTask> {
            let url = request.url;
            self.seen.lock().unwrap().push(url.clone());
            std::thread::spawn(move || {
                events(FetchEvent::Head(HttpHead {
                    status: 200,
                    url,
                    ..HttpHead::default()
                }));
                events(FetchEvent::End);
            });
            Box::new(CompletedTask)
        }
    }

    fn start_widget(
        bundle: Bundle,
    ) -> (Engine, mpsc::Receiver<MountBatch>, Arc<Mutex<Vec<String>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut config = EngineConfig::new(App::Js(bundle));
        config.network = Some(Arc::new(RecordingNetwork { seen: seen.clone() }));
        let (tx, rx) = mpsc::channel();
        let engine = Engine::start(config, || ApproxTextEngine, Sink(tx)).expect("engine starts");
        (engine, rx, seen)
    }

    fn run_until_text(
        engine: &Engine,
        rx: &mpsc::Receiver<MountBatch>,
        wanted: &str,
    ) -> Vec<Command> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let control = engine.control();
        let mut commands = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let batch = rx.recv_timeout(remaining).expect("widget result");
            let mut shown = false;
            for item in &batch.items {
                match item {
                    MountItem::Text { text, .. } if text.text == wanted => shown = true,
                    MountItem::Command { command, .. }
                        if matches!(
                            command,
                            Command::ReadClipboard { .. }
                                | Command::SetClipboard(_)
                                | Command::Desktop(_)
                        ) =>
                    {
                        if let Command::ReadClipboard { id, .. } = command {
                            control.dispatch_event(
                                ROOT_ID,
                                EventKind::Clipboard,
                                EventPayload::Map(vec![
                                    ("id", PayloadValue::Number(*id as f64)),
                                    ("has", PayloadValue::Bool(false)),
                                ]),
                            );
                        }
                        commands.push(command.clone());
                    }
                    _ => {}
                }
            }
            if shown {
                return commands;
            }
        }
    }

    #[test]
    fn every_mini_app_follows_roxs_theme() {
        use super::mini_appearance as look;
        // whatever the system is, Rox's Light and Dark win; System (or no
        // pref yet) leaves the window's, which is the system's
        assert_eq!(look("dark"), Some(true));
        assert_eq!(look("light"), Some(false));
        for pref in ["system", "", "sepia"] {
            assert_eq!(look(pref), None, "{pref:?}: the window's appearance");
        }
    }

    #[test]
    fn compiles_a_widget() {
        let js = super::compile_widget("R999", WIDGET).expect("compile");
        assert!(js.contains("mount(R)"), "wrapper mounted:\n{js}");
        assert!(js.contains("export default"), "default export kept:\n{js}");
        assert!(js.contains("from \"nsh\""), "nsh imports kept:\n{js}");
    }

    #[test]
    fn old_homes_move_where_nsh_serves_them() {
        let src = "import { View, Input, signal } from \"nsh\";\nimport { Canvas } from \"nsh/canvas\";\nimport { Image as Pic } from 'nsh/image';\nimport * as Ios from \"nsh/ios\";\nimport { x } from \"./local\";\nconst s = \"nsh/canvas\";\n";
        let out = super::current_homes(src);
        assert!(out.contains("import { View, signal } from \"nsh\";"), "{out}");
        assert!(out.contains("import { Input } from \"nsh/base\";"), "{out}");
        assert!(out.contains("import { Canvas } from \"nsh\";"), "{out}");
        assert!(out.contains("import { Image as Pic } from \"nsh\";"), "{out}");
        assert!(out.contains("import * as Ios from \"nsh/mobile\";"), "{out}");
        assert!(out.contains("import { x } from \"./local\";"), "{out}");
        assert!(out.contains("const s = \"nsh/canvas\";"), "a string is no import:\n{out}");
    }

    #[test]
    fn widget_policy_grants_nothing() {
        let access = super::mini_bundle("R1", String::new()).policy.access();
        assert!(!access.allows_http("GET", "http://127.0.0.1:8787/threads"));
        assert!(!access.allows_http("POST", "https://attacker.example/"));
        assert!(!access.allows_media("https://attacker.example/x.png"));
        assert!(!access.allows_websocket("ws://127.0.0.1:8787/"));
        assert!(!access.allows_webviews());
        assert!(!access.allows_clipboard());
        assert!(!access.allows_picker());
        assert!(!access.allows_status_bar());
        assert!(!access.allows_open_url("https://attacker.example/"));
        assert!(!access.allows_open_url("someapp://run"));
        assert!(!access.allows_host("anything"));
        assert!(!access.allows_process("sh"));
        assert!(!access.allows_socket("127.0.0.1", 8787));
        assert!(super::widget_policy().harden);

        // a v1 app never runs on this policy: it gets its own (mini_v1)
        for module in ["file:/x", "", "v1:dev.goldcoders.fixture/board"] {
            let policy = super::mini_bundle(module, String::new()).policy;
            assert_eq!(
                policy.declared,
                Some(nsh_core::access::Capabilities::default())
            );
            assert!(policy.harden);
        }
    }

    #[test]
    fn a_module_answer_gives_its_source_or_the_servers_error() {
        use serde_json::json;
        let ok = super::module_source(&json!({"source": "export default function R() {}", "title": "Forms"}));
        assert_eq!(ok.as_deref(), Ok("export default function R() {}"));
        // `{source: "", error}`: the plugin's words, not "empty module"
        let gone = super::module_source(&json!({"source": "", "error": "bir ships no app nope"}));
        assert_eq!(gone, Err("bir ships no app nope".to_owned()));
        assert_eq!(
            super::module_source(&json!({"error": " no such server "})),
            Err("no such server".to_owned())
        );
        assert_eq!(
            super::module_source(&json!({"source": "  "})),
            Err("the server sent an empty module".to_owned())
        );
        assert_eq!(super::module_source(&json!({})), Err("the server sent an empty module".to_owned()));
    }

    #[test]
    fn a_compile_error_shows_its_first_line() {
        assert_eq!(
            super::compile_error("\n  Expected `}` at forms.tsx:3:1\n  more\n"),
            "it failed to compile: Expected `}` at forms.tsx:3:1"
        );
        assert_eq!(super::compile_error(""), "it failed to compile: no details");
        let err = super::compile_widget("mcp:bir/forms", "export default function R() { return <View> }")
            .expect_err("broken JSX");
        let shown = super::compile_error(&err);
        assert!(shown.starts_with("it failed to compile: ") && !shown.contains('\n'), "{shown}");
    }

    #[test]
    fn mcp_apps_reach_only_the_app_server() {
        let policy = super::mini_bundle("mcp:bir/forms", String::new()).policy;
        let access = policy.access();
        assert!(access.allows_http("POST", "http://127.0.0.1:8787/mcp/call"));
        assert!(access.allows_http("POST", "http://127.0.0.1:8787/mcp/open"));
        assert!(access.allows_http("POST", "http://127.0.0.1:8787/attach/pick"));
        assert!(access.allows_http("GET", "http://127.0.0.1:8787/file/raw/a/b.pdf"));
        assert!(access.allows_media("http://127.0.0.1:8787/file/raw/x"));
        assert!(access.allows_media("http://127.0.0.1:8787/asset/generated/assets/x.png"));
        assert!(access.allows_media("asset:/generated/assets/x.png"));
        // Only those routes, with their methods.
        assert!(!access.allows_http("POST", "http://127.0.0.1:8787/mcp/servers"));
        assert!(!access.allows_http("POST", "http://127.0.0.1:8787/mcp/server/delete"));
        assert!(!access.allows_http("GET", "http://127.0.0.1:8787/data/linear/issues"));
        assert!(!access.allows_http("GET", "http://127.0.0.1:8787/state/x"));
        assert!(!access.allows_http("POST", "http://127.0.0.1:8787/state/x"));
        assert!(!access.allows_http("GET", "http://127.0.0.1:8787/threads"));
        assert!(!access.allows_http("GET", "http://127.0.0.1:8787/"));
        assert!(!access.allows_http("GET", "http://127.0.0.1:8787/mcp/call"));
        assert!(!access.allows_http("POST", "http://127.0.0.1:8787/mcp/call/x"));
        assert!(!access.allows_http("POST", "http://127.0.0.1:8787/file/raw/x"));
        assert!(!access.allows_http("GET", "http://127.0.0.1:8787/file/rawx"));
        assert!(!access.allows_http("GET", "http://127.0.0.1:8787/asset/x.png"));
        assert!(access.allows_http("POST", "http://127.0.0.1:8787/mcp/call?x=1"));
        assert!(!access.allows_http("GET", "https://attacker.example/"));
        assert!(!access.allows_http("GET", "http://127.0.0.1:8788/"));
        assert!(!access.allows_http("GET", "http://localhost:8787/"));
        assert!(!access.allows_http("GET", "https://127.0.0.1:8787/"));
        // NSH checks WebSockets as HTTP GETs of the same URL.
        assert!(!access.allows_websocket("ws://127.0.0.1:8787/"));
        assert!(!access.allows_websocket("ws://127.0.0.1:8787/mcp/call"));
        assert!(!access.allows_websocket("ws://127.0.0.1:8788/"));
        assert!(!access.allows_websocket("wss://attacker.example/"));
        assert!(!access.allows_socket("127.0.0.1", 8787));
        assert!(!access.allows_webviews());
        assert!(!access.allows_clipboard());
        assert!(!access.allows_picker());
        assert!(!access.allows_status_bar());
        assert!(!access.allows_open_url("https://attacker.example/"));
        assert!(!access.allows_host("anything"));
        assert!(!access.allows_process("sh"));
        assert!(policy.harden);
    }

    #[test]
    fn a_widget_cannot_fetch_the_app_server_or_read_the_clipboard() {
        let js = super::compile_widget("R998", WIDGET_EXFIL).expect("compile widget");

        let (engine, rx, seen) = start_widget(super::mini_bundle("R998", js.clone()));
        let commands = run_until_text(&engine, &rx, "refused ERR_ACCESS_DENIED [ERR_ACCESS_DENIED]");
        assert!(
            seen.lock().unwrap().is_empty(),
            "network requests: {seen:?}"
        );
        assert!(
            commands.is_empty(),
            "clipboard commands reached the platform: {commands:?}"
        );
        drop(engine);

        // an MCP app reaches the app server's app routes only, not the chats
        let (engine, rx, seen) = start_widget(super::mini_bundle("mcp:bir/forms", js.clone()));
        let commands = run_until_text(&engine, &rx, "refused ERR_ACCESS_DENIED [ERR_ACCESS_DENIED]");
        assert!(
            seen.lock().unwrap().is_empty(),
            "network requests: {seen:?}"
        );
        assert!(
            commands.is_empty(),
            "clipboard commands reached the platform: {commands:?}"
        );
        drop(engine);

        let (engine, rx, seen) = start_widget(Bundle::source(js));
        let commands = run_until_text(&engine, &rx, "fetched false []");
        assert_eq!(*seen.lock().unwrap(), ["http://127.0.0.1:8787/threads"]);
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::ReadClipboard { .. })),
            "default policy did not send a clipboard read command: {commands:?}"
        );
    }

    const WIDGET_WINDOW: &str = r#"
import { Text } from "nsh";
import { Window } from "nsh/desktop";
import os from "os";

export default function R() {
  Window.setTitle("spoofed");
  Window.minimize();
  let home: string;
  try {
    home = typeof os.homedir();
  } catch (e) {
    home = (e as Error).name;
  }
  return <Text>{`home ${home}`}</Text>;
}
"#;

    #[test]
    fn a_widget_cannot_control_the_window_or_read_the_home_directory() {
        let js = super::compile_widget("R997", WIDGET_WINDOW).expect("compile widget");

        // Widgets and MCP apps declare no `window` or `homeDir`: the window
        // commands never reach the platform and `os.homedir()` throws.
        for module in ["R997", "mcp:bir/forms"] {
            let (engine, rx, _) = start_widget(super::mini_bundle(module, js.clone()));
            let commands = run_until_text(&engine, &rx, "home TypeError");
            assert!(
                commands.is_empty(),
                "{module}: desktop commands reached the platform: {commands:?}"
            );
            drop(engine);
        }

        // An app without a declared policy keeps both.
        let (engine, rx, _) = start_widget(Bundle::source(js));
        let commands = run_until_text(&engine, &rx, "home string");
        assert!(
            commands.iter().any(|command| matches!(
                command,
                Command::Desktop(command)
                    if matches!(
                        command.as_ref(),
                        DesktopCommand::SetTitle(title) if title == "spoofed"
                    )
            )),
            "default policy did not send the title: {commands:?}"
        );
    }
}
