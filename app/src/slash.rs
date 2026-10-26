//! Composer commands and `@` tokens. `/name args` runs a built-in, or a
//! command an MCP server announces in its `rox://manifest` (`GET
//! /mcp/manifest`): that server resolves it (`POST /mcp/resolve`) and Rox
//! opens what it answers — it never reaches the model. `@server:scope:value`
//! tokens complete as they're typed; a message of tokens alone opens them,
//! one with other text goes to the model with its tokens in it.
//!
//! The composer's popup (`/` commands, `@` servers, scopes and values, a
//! resolve's choices, and the artifacts the old @ picker listed) is one
//! list: ↑ ↓ move, Enter or Tab picks, ⌘1…⌘9 pick a shown row, Escape
//! closes (keys.rs).
//!
//! Tokens of a scope the server marks `attach` can be attached to a chat
//! or a bot (`POST /mcp/attach`): `@server:scope:value attach` sent from
//! the composer, or the full-screen app's Attach. The server posts the
//! token's card there and scopes its tools to it.

use nsh_core::prelude::*;

use crate::client;
use crate::live::store;
use crate::store::AtItem;

/// The popup shows this many rows; the rest scroll in with the selection.
pub(crate) const VISIBLE: usize = 5;

fn js(v: &serde_json::Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_owned()
}

/// A command, `@` scope or mini app a server announces; a scope whose
/// tokens can be attached to a chat or bot says `attach`.
#[derive(Clone, PartialEq, Default)]
pub struct Entry {
    pub name: String,
    pub title: String,
    pub args: String,
    pub description: String,
    pub attach: bool,
}

/// One server's manifest: its commands, `@` scopes and mini apps.
#[derive(Clone, PartialEq, Default)]
pub struct Manifest {
    pub id: String,
    pub title: String,
    pub commands: Vec<Entry>,
    pub scopes: Vec<Entry>,
    pub apps: Vec<Entry>,
}

impl Manifest {
    fn from_json(v: &serde_json::Value) -> Self {
        // an app's name is its `id`
        let entries = |k: &str| -> Vec<Entry> {
            v.get(k)
                .and_then(|x| x.as_array())
                .map(|l| {
                    l.iter()
                        .map(|e| Entry {
                            name: Some(js(e, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| js(e, "id")),
                            title: js(e, "title"),
                            args: js(e, "args"),
                            description: js(e, "description"),
                            attach: e.get("attach").and_then(|x| x.as_bool()).unwrap_or(false),
                        })
                        .filter(|e| !e.name.is_empty())
                        .collect()
                })
                .unwrap_or_default()
        };
        Self { id: js(v, "id"), title: js(v, "title"), commands: entries("commands"), scopes: entries("scopes"), apps: entries("apps") }
    }
}

/// What a popup row does when picked.
#[derive(Clone, PartialEq)]
pub enum Pick {
    /// A built-in command ([`BUILTINS`]).
    Builtin(&'static str),
    /// A server's command: (server, name).
    Command(String, String),
    /// `@<server>:` — its scopes next.
    Server(String),
    /// `@<server>:<scope>:` — its values next.
    Scope(String, String),
    /// A server's mini app, opened: (server, app, title).
    App(String, String, String),
    /// A completed token, `@<server>:<scope>:<value>`.
    Value(String, String, String),
    /// One of a resolve's choices: (server, name, the args with it, whether
    /// a slash command asked — its draft then follows the args).
    Choice(String, String, String, bool),
    /// An artifact or file (`/artifacts`).
    Artifact(AtItem),
}

/// A popup row: what it does, its name, a muted hint (a command's args)
/// and a description.
#[derive(Clone, PartialEq)]
pub struct PopRow {
    pub pick: Pick,
    pub label: String,
    pub hint: String,
    pub detail: String,
}

/// A built-in command; `run` gets what follows its name (`:demo`, ` x`, "").
struct Builtin {
    name: &'static str,
    args: &'static str,
    description: &'static str,
    run: fn(&str),
}

const BUILTINS: &[Builtin] = &[Builtin {
    name: "status",
    args: ":demo | :<state> | :off",
    description: "Play the agent status states",
    // `/status` (the demo) and `/status:<state>` (App.tsx:574)
    run: |rest| {
        let arg = rest.strip_prefix(':').map(str::trim).unwrap_or(if rest.trim().is_empty() { "demo" } else { rest.trim() });
        store().mock_status(arg)
    },
}];

/// `GET /mcp/manifest`, then `then`.
fn load_manifest(then: impl FnOnce() + 'static) {
    store().mcp_asked.set(Some(std::time::Instant::now()));
    Owner::detached().run(|| {
        spawn(async move {
            if let Some(v) = client::get("/mcp/manifest").await {
                let ms = v
                    .get("servers")
                    .and_then(|x| x.as_array())
                    .map(|l| l.iter().map(Manifest::from_json).collect())
                    .unwrap_or_default();
                store().mcp_manifest.set(ms);
            }
            store().mcp_loaded.set(true);
            then();
        })
    });
}

/// The manifest again when it's over a minute old (servers come and go);
/// the popup follows it in.
fn refresh() {
    if store().mcp_asked.get().is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(60)) {
        return;
    }
    load_manifest(|| {
        let t = store().draft.get_untracked();
        if t.starts_with('/') || t.contains('@') {
            crate::composer::draft_changed(&t);
        }
    });
}

/// At boot: the manifest, so a command sent first thing is known.
pub(crate) fn boot() {
    refresh();
}

/// `send`'s first look at the composer text: true when a command, a
/// message of tokens alone, or tokens and `attach` took it — it never
/// reaches the model.
pub(crate) fn dispatch(text: &str) -> bool {
    let st = store();
    if text.is_empty() {
        return false;
    }
    if let Some(cmd) = text.strip_prefix('/') {
        let end = cmd.find(|c: char| c == ':' || c.is_whitespace()).unwrap_or(cmd.len());
        let (name, rest) = cmd.split_at(end);
        if let Some(b) = BUILTINS.iter().find(|b| b.name == name) {
            st.draft.set(String::new());
            close();
            (b.run)(rest);
            return true;
        }
    }
    // `@… @… attach`: the tokens, attached to this chat
    let words: Vec<&str> = text.split_whitespace().collect();
    let attaching = words.len() > 1
        && words.iter().filter(|w| w.eq_ignore_ascii_case("attach")).count() == 1
        && words.iter().all(|w| w.starts_with('@') || w.eq_ignore_ascii_case("attach"));
    let maybe = text.starts_with('/') || attaching || words.iter().all(|w| w.starts_with('@'));
    if !maybe {
        return false;
    }
    // the servers' commands aren't known yet: sent again once they are
    if !st.mcp_loaded.get() {
        load_manifest(|| store().send());
        return true;
    }
    if let Some(cmd) = text.strip_prefix('/') {
        let end = cmd.find(char::is_whitespace).unwrap_or(cmd.len());
        let (name, args) = cmd.split_at(end);
        let Some(server) = command_server(name) else { return false };
        resolve(server, name.to_owned(), args.trim().to_owned(), Some(text.to_owned()));
        return true;
    }
    if attaching {
        let toks: Vec<&str> = words.into_iter().filter(|w| !w.eq_ignore_ascii_case("attach")).collect();
        if toks.iter().any(|w| token(w).is_none()) {
            return false;
        }
        if let Some(w) = toks.iter().find(|w| !attachable(w)) {
            let scope = token(w).map(|(s, sc, _)| format!("@{s}:{sc}")).unwrap_or_default();
            note(format!("{scope} can't be attached"));
            return true;
        }
        let thread = st.active_id.get_untracked();
        if thread.is_empty() {
            note("Open a chat to attach to".to_owned());
            return true;
        }
        let place = st
            .threads
            .with_untracked(|t| t.iter().find(|t| t.id == thread).map(|t| t.name.clone()))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "this chat".to_owned());
        let sent = text.to_owned();
        close();
        attach(toks.into_iter().map(str::to_owned).collect(), thread, place, Some(sent));
        return true;
    }
    // tokens alone open; with files or a reply the message has more to say
    if !st.attachments.get_untracked().is_empty() || st.reply_to.get_untracked().is_some() {
        return false;
    }
    let Some(tokens) = text.split_whitespace().map(token).collect::<Option<Vec<_>>>() else {
        return false;
    };
    let sent = text.to_owned();
    Owner::detached().run(|| {
        spawn(async move {
            let mut opened = true;
            for (server, scope, value) in tokens {
                opened &= resolve_one(server, scope, value, false).await;
            }
            if opened && store().draft.get_untracked().trim() == sent.trim() {
                store().draft.set(String::new());
            }
        })
    });
    true
}

/// The server that announces command `name`.
fn command_server(name: &str) -> Option<String> {
    store().mcp_manifest.with_untracked(|ms| {
        ms.iter().find(|m| m.commands.iter().any(|c| c.name == name)).map(|m| m.id.clone())
    })
}

/// `@server:scope:value` → its parts, whatever the servers announce.
fn token_parts(w: &str) -> Option<(&str, &str, &str)> {
    let mut it = w.strip_prefix('@')?.splitn(3, ':');
    let (server, scope, value) = (it.next()?, it.next()?, it.next()?);
    (!server.is_empty() && !scope.is_empty()).then_some((server, scope, value))
}

/// `@server:scope:value` with a scope its server announces.
pub(crate) fn token(w: &str) -> Option<(String, String, String)> {
    let (server, scope, value) = token_parts(w)?;
    store()
        .mcp_manifest
        .with_untracked(|ms| ms.iter().any(|m| m.id == server && m.scopes.iter().any(|s| s.name == scope)))
        .then(|| (server.to_owned(), scope.to_owned(), value.to_owned()))
}

/// Whether `tok`'s scope says its tokens can be attached.
pub(crate) fn attachable(tok: &str) -> bool {
    let Some((server, scope, _)) = token_parts(tok) else { return false };
    store().mcp_manifest.with_untracked(|ms| {
        ms.iter().any(|m| m.id == server && m.scopes.iter().any(|s| s.name == scope && s.attach))
    })
}

/// `POST /mcp/attach` for each token, into chat `thread` (a bot's: its
/// main chat), `place` its name for the toast. The server posts each
/// token's card there; the open chat shows it. `sent` is the composer's
/// draft, cleared once one attached unless the user typed on.
pub(crate) fn attach(tokens: Vec<String>, thread: String, place: String, sent: Option<String>) {
    Owner::detached().run(|| {
        spawn(async move {
            let mut done = Vec::new();
            for tok in tokens {
                let server = token_parts(&tok).map(|(s, _, _)| s.to_owned()).unwrap_or_default();
                let body = serde_json::json!({ "server": server, "token": tok, "thread": thread });
                match client::post_any("/mcp/attach", body).await {
                    Some(v) if v.get("ok").and_then(|x| x.as_bool()) == Some(true) => {
                        let title = v.get("attachment").map(|a| js(a, "title")).unwrap_or_default();
                        done.push(if title.is_empty() { tok } else { title });
                    }
                    Some(v) => {
                        let error = js(&v, "error");
                        note(if error.is_empty() { format!("Couldn't attach {tok}") } else { error });
                    }
                    None => note("The app server didn't answer".to_owned()),
                }
            }
            if done.is_empty() {
                return;
            }
            note(format!("Attached {} to {place}", done.join(", ")));
            let st = store();
            if sent.is_some_and(|s| st.draft.get_untracked().trim() == s.trim()) {
                st.draft.set(String::new());
            }
            // the card the server posted, and the Library's new item
            if st.active_id.get_untracked() == thread {
                st.refresh_turns();
                st.refresh_thread_apps();
            }
        })
    });
}

/// Attach to chat or bot… (the full-screen app's): the palette over the
/// bots and chats; the one picked gets `tok` (a bot's main chat).
pub(crate) fn attach_picker(tok: &str, title: &str) {
    use crate::store::{PaletteAct, PaletteItem};
    let st = store();
    let row = |title: String, subtitle: String, thread: String| PaletteItem {
        title: title.clone(),
        subtitle,
        icon: String::new(),
        checked: false,
        act: PaletteAct::Attach(tok.to_owned(), thread, title),
        bot: None,
        letter_of: String::new(),
        app_of: String::new(),
    };
    let mut items = Vec::new();
    for b in st.bots.get_untracked().into_iter().filter(|b| !b.hidden && !b.thread_id.is_empty()) {
        items.push(PaletteItem { bot: Some(b.clone()), ..row(b.name.clone(), "Bot".into(), b.thread_id.clone()) });
    }
    for t in untracked(|| st.chats()) {
        let name = if t.name.is_empty() { "New chat".to_owned() } else { t.name.clone() };
        items.push(PaletteItem { letter_of: name.clone(), ..row(name, "Chat".into(), t.id.clone()) });
    }
    let what = if title.is_empty() { tok.to_owned() } else { title.to_owned() };
    st.open_palette_pick(format!("Attach {what} to…"), items);
}

/// Show in Chat for an attached token: where it's attached
/// (`GET /mcp/attachments?token=`). One place goes to its card; more
/// open the palette over them; none, `fallback` (a chat and turn).
pub(crate) fn show_in_chats(tok: String, fallback: Option<(String, String)>) {
    use crate::store::{PaletteAct, PaletteItem};
    Owner::detached().run(|| {
        spawn(async move {
            let st = store();
            let url = format!("/mcp/attachments?token={}", client::urlenc(&tok));
            // (thread, turn, chat title, bot id, bot name)
            let mut places: Vec<(String, String, String, String, String)> = Vec::new();
            for a in client::get(&url)
                .await
                .and_then(|v| v.get("attachments").and_then(|x| x.as_array()).cloned())
                .unwrap_or_default()
            {
                let thread = js(&a, "thread");
                if !thread.is_empty() && !places.iter().any(|p| p.0 == thread) {
                    places.push((thread, js(&a, "turn"), js(&a, "threadTitle"), js(&a, "bot"), js(&a, "botName")));
                }
            }
            let go = |thread: &str, turn: &str| {
                st.go_to_key(thread, Some(turn.to_owned()).filter(|t| !t.is_empty()));
            };
            match places.len() {
                0 => match fallback {
                    Some((thread, turn)) => go(&thread, &turn),
                    None => note("It isn't attached anywhere".to_owned()),
                },
                1 => go(&places[0].0, &places[0].1),
                _ => {
                    let bots = st.bots.get_untracked();
                    let items = places
                        .into_iter()
                        .map(|(thread, turn, title, bot, bot_name)| {
                            let bot = bots.iter().find(|b| !bot.is_empty() && b.id == bot).cloned();
                            let title = if title.is_empty() { "Chat".to_owned() } else { title };
                            let link = if turn.is_empty() { format!("rox://chat/{thread}") } else { format!("rox://chat/{thread}?m={turn}") };
                            PaletteItem {
                                title: title.clone(),
                                subtitle: match (&bot, bot_name.is_empty()) {
                                    (Some(b), _) => format!("Bot · {}", b.name),
                                    (None, false) => format!("Bot · {bot_name}"),
                                    (None, true) => "Chat".to_owned(),
                                },
                                icon: String::new(),
                                checked: false,
                                act: PaletteAct::Link(link),
                                letter_of: if bot.is_none() { title } else { String::new() },
                                bot,
                                app_of: String::new(),
                            }
                        })
                        .collect();
                    st.open_palette_pick("Show in which chat?".to_owned(), items);
                }
            }
        })
    });
}

/// A token's chip text: its label when it was picked from a list.
pub(crate) fn token_label(tok: &str) -> String {
    store().token_labels.borrow().get(tok).cloned().unwrap_or_default()
}

/// Resolves a command (`sent` its draft, cleared once it opened unless
/// the user typed on) in the background.
fn resolve(server: String, name: String, args: String, sent: Option<String>) {
    Owner::detached().run(|| {
        spawn(async move {
            let slash = sent.is_some();
            if resolve_one(server, name, args, slash).await
                && sent.is_some_and(|s| store().draft.get_untracked().trim() == s.trim())
            {
                store().draft.set(String::new());
            }
        })
    });
}

/// `POST /mcp/resolve`, then: opens what it resolved to (true), offers its
/// choices in the popup, or shows its error by the composer.
async fn resolve_one(server: String, name: String, args: String, slash: bool) -> bool {
    let thread = store().active_id.get_untracked();
    let body = serde_json::json!({ "server": server, "name": name, "args": args, "complete": false, "thread": thread });
    let Some(v) = client::post("/mcp/resolve", body).await else {
        note("The app server didn't answer".to_owned());
        return false;
    };
    if v.get("ok").and_then(|x| x.as_bool()) == Some(true) {
        close();
        let (open, title, tok) = (js(&v, "open"), js(&v, "title"), js(&v, "token"));
        match crate::store::mini_link(&open) {
            Some(key) => store().open_mini_token(&key, &title, &tok),
            None if !open.is_empty() => store().open_link(&open),
            None => {}
        }
        return true;
    }
    let error = js(&v, "error");
    let rows: Vec<PopRow> = choices(&v)
        .into_iter()
        .map(|(value, label, detail)| PopRow {
            pick: Pick::Choice(server.clone(), name.clone(), format!("{args} {value}").trim().to_owned(), slash),
            label,
            hint: String::new(),
            detail,
        })
        .collect();
    if rows.is_empty() {
        note(if error.is_empty() { format!("Nothing found for {name}") } else { error });
    } else {
        show(rows, error);
    }
    false
}

/// A resolver's `choices`: (value, label, detail).
fn choices(v: &serde_json::Value) -> Vec<(String, String, String)> {
    v.get("choices")
        .and_then(|x| x.as_array())
        .map(|l| {
            l.iter()
                .map(|c| {
                    let value = js(c, "value");
                    let label = js(c, "label");
                    (value.clone(), if label.is_empty() { value } else { label }, js(c, "detail"))
                })
                .filter(|(v, _, _)| !v.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// What a command or token couldn't do, as a toast.
fn note(msg: String) {
    crate::toast::show(msg);
}

/// The popup's own rows, its header (a resolve's question) and the first
/// row selected.
fn show(rows: Vec<PopRow>, head: String) {
    let st = store();
    batch(|| {
        st.pop_rows.set(rows);
        st.pop_head.set(head);
        st.pop_sel.set(0);
    });
}

/// Closes the popup (the artifact list with it).
pub(crate) fn close() {
    let st = store();
    st.complete_seq.set(st.complete_seq.get() + 1);
    batch(|| {
        st.pop_rows.set(Vec::new());
        st.pop_head.set(String::new());
        st.at_items.set(Vec::new());
        st.at_start.set(-1);
    });
}

/// Every row the popup lists: its own, then the artifacts (tracked).
pub(crate) fn rows() -> Vec<PopRow> {
    let st = store();
    let mut rows = st.pop_rows.get();
    rows.extend(st.at_items.get().into_iter().map(|it| {
        let hint = if it.kind == "app" {
            format!(
                "{}{}{}",
                it.name,
                if it.v > 0 { format!(" · v{}", it.v) } else { String::new() },
                if it.scope == "thread" { " · this chat" } else { "" },
            )
        } else {
            it.file.clone()
        };
        PopRow {
            label: if it.title.is_empty() { it.name.clone() } else { it.title.clone() },
            hint,
            detail: String::new(),
            pick: Pick::Artifact(it),
        }
    }));
    rows
}

/// `/<q>` being typed (no space yet): the commands starting with it, the
/// built-ins first. False when `t` is something else.
pub(crate) fn slash_menu(t: &str) -> bool {
    let Some(q) = t.strip_prefix('/') else { return false };
    if q.contains(|c: char| c.is_whitespace() || c == ':') {
        return false;
    }
    refresh();
    let q = q.to_lowercase();
    let mut rows: Vec<PopRow> = BUILTINS
        .iter()
        .filter(|b| b.name.starts_with(&q))
        .map(|b| PopRow {
            pick: Pick::Builtin(b.name),
            label: format!("/{}", b.name),
            hint: b.args.to_owned(),
            detail: b.description.to_owned(),
        })
        .collect();
    store().mcp_manifest.with_untracked(|ms| {
        for m in ms {
            for c in m.commands.iter().filter(|c| c.name.to_lowercase().starts_with(&q)) {
                rows.push(PopRow {
                    pick: Pick::Command(m.id.clone(), c.name.clone()),
                    label: format!("/{}", c.name),
                    hint: c.args.clone(),
                    detail: [c.description.as_str(), m.title.as_str()]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · "),
                });
            }
        }
    });
    show(rows, String::new());
    true
}

/// `@<q>` (no `:` yet): the servers with scopes or apps that match.
pub(crate) fn server_menu(q: &str) {
    refresh();
    let q = q.to_lowercase();
    let rows = store().mcp_manifest.with_untracked(|ms| {
        ms.iter()
            .filter(|m| !m.scopes.is_empty() || !m.apps.is_empty())
            .filter(|m| m.id.starts_with(&q) || m.title.to_lowercase().contains(&q))
            .map(|m| PopRow {
                pick: Pick::Server(m.id.clone()),
                label: format!("@{}", m.id),
                hint: m.title.clone(),
                detail: if m.scopes.is_empty() {
                    format!("{} apps", m.apps.len())
                } else {
                    m.scopes.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")
                },
            })
            .collect()
    });
    show(rows, String::new());
}

/// `@<server>:…` being typed: that server's scopes (its apps when it has
/// none), or past `<scope>:` the values its resolver suggests for what's
/// typed (`complete: true`). False without a `:` (the server list then).
pub(crate) fn token_menu(tail: &str) -> bool {
    let Some((server, rest)) = tail.split_once(':') else { return false };
    refresh();
    let Some(m) = store().mcp_manifest.with_untracked(|ms| ms.iter().find(|m| m.id == server).cloned()) else {
        show(Vec::new(), String::new());
        return true;
    };
    let Some((scope, value)) = rest.split_once(':') else {
        let q = rest.to_lowercase();
        let rows = if m.scopes.is_empty() {
            m.apps
                .iter()
                .filter(|a| a.name.to_lowercase().starts_with(&q) || a.title.to_lowercase().contains(&q))
                .map(|a| PopRow {
                    pick: Pick::App(m.id.clone(), a.name.clone(), a.title.clone()),
                    label: if a.title.is_empty() { a.name.clone() } else { a.title.clone() },
                    hint: "Mini app".to_owned(),
                    detail: a.description.clone(),
                })
                .collect()
        } else {
            m.scopes
                .iter()
                .filter(|s| s.name.to_lowercase().starts_with(&q))
                .map(|s| PopRow {
                    pick: Pick::Scope(m.id.clone(), s.name.clone()),
                    label: format!("@{}:{}", m.id, s.name),
                    hint: String::new(),
                    detail: s.description.clone(),
                })
                .collect()
        };
        show(rows, String::new());
        return true;
    };
    if !m.scopes.iter().any(|s| s.name == scope) {
        show(Vec::new(), String::new());
        return true;
    }
    let (server, scope, value) = (m.id.clone(), scope.to_owned(), value.to_owned());
    let n = next_ask();
    Owner::detached().run(|| {
        spawn(async move {
            if let Some(found) = complete(n, &server, &scope, &value).await {
                show(value_rows(&server, &scope, found), String::new());
            }
        })
    });
    true
}

/// A new completion ask's number: an older one's answer is dropped.
fn next_ask() -> u64 {
    let st = store();
    let n = st.complete_seq.get() + 1;
    st.complete_seq.set(n);
    n
}

/// The resolver's suggestions for `@server:scope:value` as typed
/// (`complete: true`, ask `n`); None when a newer ask took over. The rows
/// shown stay until the answer replaces them.
async fn complete(n: u64, server: &str, scope: &str, value: &str) -> Option<Vec<(String, String, String)>> {
    let st = store();
    let thread = st.active_id.get_untracked();
    let body = serde_json::json!({ "server": server, "name": scope, "args": value, "complete": true, "thread": thread });
    let v = client::post("/mcp/resolve", body).await;
    (st.complete_seq.get() == n).then(|| v.map(|v| choices(&v)).unwrap_or_default())
}

/// A scope's suggestions as popup rows (at most 20).
fn value_rows(server: &str, scope: &str, found: Vec<(String, String, String)>) -> Vec<PopRow> {
    found
        .into_iter()
        .take(20)
        .map(|(value, label, detail)| PopRow {
            pick: Pick::Value(server.to_owned(), scope.to_owned(), value.clone()),
            hint: if label == value { String::new() } else { value },
            label,
            detail,
        })
        .collect()
}

/// A value was picked: the token stays open while the resolver has more
/// to add to it (a taxpayer's `<tin>/` takes a form next, a form its
/// period) and the popup lists that at once; else it ends with a space.
fn value_picked(server: String, scope: String, value: String, draft: String) {
    let n = next_ask();
    Owner::detached().run(|| {
        spawn(async move {
            let found = complete(n, &server, &scope, &value).await;
            let st = store();
            if st.draft.get_untracked() != draft {
                return;
            }
            let Some(found) = found else { return };
            let more = value.ends_with('/') || found.iter().any(|(v, _, _)| v != &value && v.starts_with(&value));
            if more && !found.is_empty() {
                show(value_rows(&server, &scope, found), String::new());
                return;
            }
            close();
            st.draft.set(format!("{draft} "));
        })
    });
}

/// The draft without its `@…` tail (from `at_start`).
fn before_tail() -> String {
    let st = store();
    let t = st.draft.get_untracked();
    let i = st.at_start.get_untracked();
    let at = if i >= 0 && (i as usize) < t.len() && t.as_bytes()[i as usize] == b'@' {
        Some(i as usize)
    } else {
        t.rfind('@')
    };
    at.map_or(t.clone(), |i| t[..i].to_owned())
}

/// The draft with its `@…` tail replaced by `@<with>`, and the popup
/// following it.
fn replace_tail(with: &str) {
    let next = format!("{}@{with}", before_tail());
    store().draft.set(next.clone());
    crate::composer::draft_changed(&next);
}

/// Picks row `row`.
pub(crate) fn pick(row: &PopRow) {
    let st = store();
    let command = |name: &str| {
        // typed in full already: Enter runs it
        if st.draft.get_untracked().trim() == format!("/{name}") {
            close();
            st.send();
            return;
        }
        st.draft.set(format!("/{name} "));
        close();
    };
    match &row.pick {
        Pick::Builtin(name) => command(name),
        Pick::Command(_, name) => command(name.as_str()),
        Pick::Server(id) => replace_tail(&format!("{id}:")),
        Pick::Scope(server, scope) => replace_tail(&format!("{server}:{scope}:")),
        Pick::Value(server, scope, value) => {
            let tok = format!("@{server}:{scope}:{value}");
            // a value extending a picked `…/` part (a taxpayer, then a
            // form) keeps that part's label in front: the chip still says
            // who it's for
            let label = {
                let labels = st.token_labels.borrow();
                let lead = labels
                    .iter()
                    .filter(|(k, _)| k.ends_with('/') && tok.starts_with(k.as_str()) && k.len() < tok.len())
                    .max_by_key(|(k, _)| k.len())
                    .map(|(_, l)| l.clone());
                match lead {
                    Some(lead) => format!("{lead} · {}", row.label),
                    None => row.label.clone(),
                }
            };
            st.token_labels.borrow_mut().insert(tok, label);
            let next = format!("{}@{server}:{scope}:{value}", before_tail());
            st.draft.set(next.clone());
            value_picked(server.clone(), scope.clone(), value.clone(), next);
        }
        Pick::App(server, app, title) => {
            st.draft.set(before_tail());
            close();
            st.open_mini_full(&format!("mcp:{server}/{app}"), title);
        }
        Pick::Choice(server, name, args, slash) => {
            let sent = slash.then(|| format!("/{name} {args}"));
            if let Some(s) = &sent {
                st.draft.set(s.clone());
            }
            close();
            resolve(server.clone(), name.clone(), args.clone(), sent);
        }
        Pick::Artifact(it) => crate::composer::at_pick(it),
    }
}

/// The first row the popup shows with row `sel` selected.
pub(crate) fn first_shown(sel: usize) -> usize {
    sel.saturating_sub(VISIBLE - 1)
}

/// ⌘1…⌘9: picks the `n`th row the popup shows.
pub(crate) fn pick_shown(n: usize) {
    let rows = untracked(rows);
    let sel = store().pop_sel.get_untracked().min(rows.len().saturating_sub(1));
    if n == 0 || n > VISIBLE {
        return;
    }
    if let Some(r) = rows.get(first_shown(sel) + n - 1) {
        pick(r);
    }
}

/// ↑ / ↓ in the popup.
pub(crate) fn step(d: i64) {
    let st = store();
    let n = untracked(rows).len() as i64;
    if n > 0 {
        st.pop_sel.update(|i| *i = ((*i as i64 + n + d) % n) as usize);
    }
}

/// Enter / Tab: picks the selected row.
pub(crate) fn enter() {
    let rows = untracked(rows);
    let sel = store().pop_sel.get_untracked().min(rows.len().saturating_sub(1));
    if let Some(r) = rows.get(sel) {
        pick(r);
    }
}
