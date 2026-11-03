//! Composer commands and `@` tokens. `/name args` runs a built-in, or a
//! command an MCP server announces in its `buwiz://manifest` (`GET
//! /mcp/manifest`): that server resolves it (`POST /mcp/resolve`) and Rox
//! opens what it answers — it never reaches the model. `@server:scope:value`
//! tokens complete as they're typed; a message of tokens alone opens them,
//! one with other text goes to the model with its tokens in it.
//!
//! The composer's popup (`/` commands, `@` servers, scopes and values, and
//! a resolve's choices) is one list. `@` lists mini-app data only: generated
//! apps and files open from the command palette, not from here. The list: ↑ ↓ move, Enter or Tab picks, ⌘1…⌘9 pick a shown row, Escape
//! closes (keys.rs). The rows remember the text they were computed for:
//! when the answer lags the typing (plugin commands resolve over the
//! broker), Enter sends what's typed rather than an older query's row.
//!
//! Tokens of a scope the server marks `attach` can be attached to a chat
//! or a bot (`POST /mcp/attach`): `@server:scope:value attach` sent from
//! the composer, or the full-screen app's Attach. The server posts the
//! token's card there and scopes its tools to it.

use nsh_core::prelude::*;

use crate::client;
use crate::live::store;

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
    /// A completed token, `@<server>:<scope>:<value>`.
    Value(String, String, String),
    /// One of a resolve's choices: (server, name, the args with it, whether
    /// a slash command asked — its draft then follows the args).
    Choice(String, String, String, bool),
    /// A v1 plugin's command, in the form that reaches it (`name` or
    /// `short:name`, 3.1).
    V1Command(String),
    /// One of a v1 resolve's choices: (plugin, kind, name, value).
    V1Choice(String, &'static str, String, String),
    /// The host's account command of a v1 plugin, `@<short>:<command>`.
    V1Account(String, String),
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
    // `@short:login` and the like alone: the host's account command (its
    // card in this chat), never the model's
    if st.attachments.get_untracked().is_empty() && st.reply_to.get_untracked().is_none() {
        if let Some((plugin, cmd)) = st.v1.listed.with_untracked(|l| crate::mini_v1::connect::account_command_for(text, l)) {
            close();
            st.v1_account_command(&plugin, &cmd, text);
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

/// `@server:scope:value` → its parts, whatever the servers announce, in
/// the grammar the app server parses tokens with (`attachments.mjs`,
/// `@([a-z0-9-]+):([A-Za-z0-9_-]+):(\S*)`): the server an id of `a-z`,
/// `0-9` and `-`; the scope ASCII letters, digits, `_` and `-`; the value
/// the rest of the word, `:` and `/` included (TINs, branch codes). An
/// empty value is a token too, as the server reads it: sent alone, its
/// resolver answers with choices.
pub(crate) fn parse_token(w: &str) -> Option<(&str, &str, &str)> {
    let (server, rest) = w.strip_prefix('@')?.split_once(':')?;
    let (scope, value) = rest.split_once(':')?;
    let server_ok = !server.is_empty()
        && server.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    let scope_ok = !scope.is_empty() && scope.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    (server_ok && scope_ok && !value.contains(char::is_whitespace)).then_some((server, scope, value))
}

/// `@server:scope:value` with a scope its server announces.
pub(crate) fn token(w: &str) -> Option<(String, String, String)> {
    let (server, scope, value) = parse_token(w)?;
    store()
        .mcp_manifest
        .with_untracked(|ms| ms.iter().any(|m| m.id == server && m.scopes.iter().any(|s| s.name == scope)))
        .then(|| (server.to_owned(), scope.to_owned(), value.to_owned()))
}

/// Whether `tok`'s scope says its tokens can be attached.
pub(crate) fn attachable(tok: &str) -> bool {
    let Some((server, scope, _)) = parse_token(tok) else { return false };
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
                let server = parse_token(&tok).map(|(s, _, _)| s.to_owned()).unwrap_or_default();
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
                            let link = if turn.is_empty() { format!("buwiz://chat/{thread}") } else { format!("buwiz://chat/{thread}?m={turn}") };
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
/// row selected, for the composer's text as it is now.
pub(crate) fn show_rows(rows: Vec<PopRow>) {
    show(rows, String::new());
}

/// `show_rows` for completions asked when the text was `query`: an
/// answer landing after more was typed is no pick for Enter.
pub(crate) fn show_rows_for(query: String, rows: Vec<PopRow>) {
    show_for(query, rows, String::new());
}

fn show(rows: Vec<PopRow>, head: String) {
    show_for(store().draft.get_untracked(), rows, head);
}

fn show_for(query: String, rows: Vec<PopRow>, head: String) {
    let st = store();
    *st.pop_query.borrow_mut() = query;
    batch(|| {
        st.pop_rows.set(rows);
        st.pop_head.set(head);
        st.pop_sel.set(0);
    });
}

/// Closes the popup.
pub(crate) fn close() {
    let st = store();
    st.complete_seq.set(st.complete_seq.get() + 1);
    batch(|| {
        st.pop_rows.set(Vec::new());
        st.pop_head.set(String::new());
        st.at_start.set(-1);
    });
}

/// Every row the popup lists (tracked).
pub(crate) fn rows() -> Vec<PopRow> {
    store().pop_rows.get()
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
    // v1 plugins' commands, each in the form that reaches it (3.1)
    let entries = store().v1.entries.get_untracked();
    for (label, plugin, name) in crate::mini_v1::tokens::command_rows(&q, &crate::mini_v1::ui::commands(&entries)) {
        let p = entries.iter().find(|p| p.id == plugin);
        let c = p.and_then(|p| p.commands.iter().find(|c| c.get("name").and_then(|n| n.as_str()) == Some(name.as_str())));
        rows.push(PopRow {
            pick: Pick::V1Command(label.trim_start_matches('/').to_owned()),
            hint: c.map(|c| js(c, "args")).unwrap_or_default(),
            detail: [c.map(|c| js(c, "description")).unwrap_or_default(), p.map(|p| p.title.clone()).unwrap_or_default()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · "),
            label,
        });
    }
    show(rows, String::new());
    true
}

/// `@<q>` (no `:` yet): the servers with `@` scopes that match.
pub(crate) fn server_menu(q: &str) {
    refresh();
    let plugins = store().v1.entries.get_untracked();
    let accounts: Vec<String> = store().v1.listed.with_untracked(|l| l.iter().filter(|p| !p.account_commands.is_empty()).map(|p| p.short.clone()).collect());
    let rows = store().mcp_manifest.with_untracked(|ms| server_rows(q, ms, &plugins, &accounts));
    show(rows, String::new());
}

/// The `@<q>` rows: the MCP servers and enabled v1 plugins with `@`
/// scopes whose name starts with `q` or whose title has it, a v1 plugin
/// replacing an MCP server of its short. A server announcing only apps
/// is no row: its apps are not data. A plugin of `accounts` (the shorts
/// whose package declares accounts) is a row for its account commands
/// even without scopes.
fn server_rows(q: &str, ms: &[Manifest], plugins: &[crate::mini_v1::load::PluginEntry], accounts: &[String]) -> Vec<PopRow> {
    let q = q.to_lowercase();
    let mut rows: Vec<PopRow> = ms
        .iter()
        .filter(|m| !m.scopes.is_empty())
        .filter(|m| m.id.starts_with(&q) || m.title.to_lowercase().contains(&q))
        .map(|m| PopRow {
            pick: Pick::Server(m.id.clone()),
            label: format!("@{}", m.id),
            hint: m.title.clone(),
            detail: m.scopes.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", "),
        })
        .collect();
    // v1 plugins with scopes, by their short
    for p in plugins.iter().filter(|p| p.enabled && (!p.scopes.is_empty() || accounts.contains(&p.short))) {
        if p.short.starts_with(&q) || p.title.to_lowercase().contains(&q) {
            rows.retain(|r| r.pick != Pick::Server(p.short.clone()));
            let mut names: Vec<String> = p.scopes.iter().map(|s| js(s, "name")).collect();
            if accounts.contains(&p.short) {
                names.extend(crate::mini_v1::tokens::ACCOUNT_COMMANDS.iter().map(|c| c.to_string()));
            }
            rows.push(PopRow { pick: Pick::Server(p.short.clone()), label: format!("@{}", p.short), hint: p.title.clone(), detail: names.join(", ") });
        }
    }
    rows
}

/// `@<server>:…` being typed: that server's scopes (its apps when it has
/// none), or past `<scope>:` the values its resolver suggests for what's
/// typed (`complete: true`). False without a `:` (the server list then).
pub(crate) fn token_menu(tail: &str) -> bool {
    let Some((server, rest)) = tail.split_once(':') else { return false };
    // a v1 plugin's scopes and the choices its resolve offers (9.3)
    if let Some(p) = store().v1.by_short(server) {
        match rest.split_once(':') {
            Some((scope, value)) => {
                store().v1_complete(server, scope, value);
            }
            None => {
                let mut rows = v1_scope_rows(&p, rest);
                let cmds = store().v1.listed.with_untracked(|l| l.iter().find(|x| x.id == p.id).map(|x| x.account_commands.clone())).unwrap_or_default();
                rows.extend(v1_account_rows(&p.short, rest, &cmds));
                show(rows, String::new())
            }
        }
        return true;
    }
    refresh();
    let Some(m) = store().mcp_manifest.with_untracked(|ms| ms.iter().find(|m| m.id == server).cloned()) else {
        show(Vec::new(), String::new());
        return true;
    };
    let Some((scope, value)) = rest.split_once(':') else {
        // `@` is mini-app data only: a server's scopes, never its apps
        // (those open from the command palette)
        let q = rest.to_lowercase();
        let rows = m
            .scopes
            .iter()
            .filter(|s| s.name.to_lowercase().starts_with(&q))
            .map(|s| PopRow {
                pick: Pick::Scope(m.id.clone(), s.name.clone()),
                label: format!("@{}:{}", m.id, s.name),
                hint: String::new(),
                detail: s.description.clone(),
            })
            .collect();
        show(rows, String::new());
        return true;
    };
    if !m.scopes.iter().any(|s| s.name == scope) {
        show(Vec::new(), String::new());
        return true;
    }
    let (server, scope, value) = (m.id.clone(), scope.to_owned(), value.to_owned());
    let n = next_ask();
    let query = store().draft.get_untracked();
    Owner::detached().run(|| {
        spawn(async move {
            if let Some(found) = complete(n, &server, &scope, &value).await {
                show_for(query, value_rows(&server, &scope, found), String::new());
            }
        })
    });
    true
}

/// `@<short>:<q>` rows: the v1 plugin's scopes starting with `q`.
fn v1_scope_rows(p: &crate::mini_v1::load::PluginEntry, q: &str) -> Vec<PopRow> {
    let q = q.to_lowercase();
    p.scopes
        .iter()
        .map(|sc| (js(sc, "name"), js(sc, "description")))
        .filter(|(n, _)| n.starts_with(&q))
        .map(|(n, d)| PopRow {
            pick: Pick::Scope(p.short.clone(), n.clone()),
            label: format!("@{}:{n}", p.short),
            hint: String::new(),
            detail: d,
        })
        .collect()
}

/// `@<short>:<q>` rows of the host's account commands `cmds` starting
/// with `q` (`@gws:login`, …).
fn v1_account_rows(short: &str, q: &str, cmds: &[String]) -> Vec<PopRow> {
    let q = q.to_lowercase();
    cmds.iter()
        .filter(|c| c.starts_with(&q))
        .map(|c| PopRow {
            pick: Pick::V1Account(short.to_owned(), c.clone()),
            label: format!("@{short}:{c}"),
            hint: String::new(),
            detail: crate::mini_v1::connect::account_command_detail(c).to_owned(),
        })
        .collect()
}

/// A new completion ask's number: an older one's answer is dropped.
pub(crate) fn next_ask() -> u64 {
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
                show_for(draft, value_rows(&server, &scope, found), String::new());
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
        Pick::V1Account(short, cmd) => {
            // typed in full already: Enter runs it
            if st.draft.get_untracked().trim() == format!("@{short}:{cmd}") {
                close();
                st.send();
                return;
            }
            replace_tail(&format!("{short}:{cmd}"));
            close();
        }
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
        Pick::Choice(server, name, args, slash) => {
            let sent = slash.then(|| format!("/{name} {args}"));
            if let Some(s) = &sent {
                st.draft.set(s.clone());
            }
            close();
            resolve(server.clone(), name.clone(), args.clone(), sent);
        }
        Pick::V1Command(form) => command(form.as_str()),
        Pick::V1Choice(plugin, kind, name, value) => st.v1_pick(plugin.clone(), kind, name.clone(), value.clone(), row.label.clone()),
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

/// What Enter does with the popup open.
#[derive(PartialEq)]
enum Enter<'a> {
    /// Picks this row.
    Pick(&'a PopRow),
    /// Sends what's typed.
    Send,
}

/// Enter with row `sel` selected in rows computed for the text `query`,
/// the composer holding `text` (D20): the row while the rows are for
/// that text, else what's typed. Rows for `/books` still showing when
/// `/books month-end` is typed would otherwise run plain `/books`.
fn on_enter<'a>(rows: &'a [PopRow], sel: usize, query: &str, text: &str) -> Enter<'a> {
    match rows.get(sel.min(rows.len().saturating_sub(1))) {
        Some(r) if query == text => Enter::Pick(r),
        _ => Enter::Send,
    }
}

/// The selected row while the rows are for the text as typed.
fn fresh_pick() -> Option<PopRow> {
    let st = store();
    let rows = untracked(rows);
    let query = st.pop_query.borrow().clone();
    match on_enter(&rows, st.pop_sel.get_untracked(), &query, &st.draft.get_untracked()) {
        Enter::Pick(r) => Some(r.clone()),
        Enter::Send => None,
    }
}

/// Enter: picks the selected row, or sends what's typed when the rows
/// are for an older text.
pub(crate) fn enter() {
    match fresh_pick() {
        Some(r) => pick(&r),
        None => {
            close();
            store().send();
        }
    }
}

/// Tab: picks the selected row while the rows are for the text as typed.
pub(crate) fn tab() {
    if let Some(r) = fresh_pick() {
        pick(&r);
    }
}

#[cfg(test)]
mod tests {
    use super::{Entry, Enter, Manifest, Pick, PopRow, on_enter, parse_token, server_rows, v1_account_rows, v1_scope_rows, value_rows};
    use crate::mini_v1::load::PluginEntry;
    use serde_json::json;

    fn entry(name: &str) -> Entry {
        Entry { name: name.to_owned(), ..Default::default() }
    }

    fn manifest(id: &str, scopes: &[&str], apps: &[&str]) -> Manifest {
        Manifest {
            id: id.to_owned(),
            title: id.to_uppercase(),
            scopes: scopes.iter().map(|n| entry(n)).collect(),
            apps: apps.iter().map(|n| entry(n)).collect(),
            ..Default::default()
        }
    }

    fn plugin(short: &str, title: &str, enabled: bool, scopes: &[&str]) -> PluginEntry {
        PluginEntry {
            id: format!("app.buwiz.{short}"),
            short: short.to_owned(),
            title: title.to_owned(),
            enabled,
            scopes: scopes.iter().map(|n| json!({ "name": n, "description": format!("{n} rows") })).collect(),
            apps: vec![json!({ "id": "main" })],
            ..Default::default()
        }
    }

    /// The `@` popup's picks are data only: servers, scopes, values and
    /// choices, never a generated app or file.
    fn data_only(rows: &[PopRow]) -> bool {
        rows.iter().all(|r| matches!(r.pick, Pick::Server(_) | Pick::Scope(..) | Pick::Value(..) | Pick::Choice(..) | Pick::V1Choice(..)))
    }

    #[test]
    fn at_alone_lists_the_mini_app_servers_with_scopes_only() {
        let ms = [manifest("bir", &["taxpayer"], &[]), manifest("gallery", &[], &["hello", "todo"])];
        let ps = [
            plugin("ebirforms", "eBIR Forms", true, &["taxprofile", "branchcode", "taxform"]),
            plugin("books", "Books", true, &["person", "account", "period"]),
            plugin("off", "Disabled", false, &["thing"]),
            plugin("viewer", "Viewer", true, &[]),
        ];
        let rows = server_rows("", &ms, &ps, &[]);
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        // an apps-only server, a disabled plugin and one without scopes are no rows
        assert_eq!(labels, ["@bir", "@ebirforms", "@books"]);
        assert!(rows.iter().all(|r| matches!(r.pick, Pick::Server(_))));
        assert_eq!(rows[1].detail, "taxprofile, branchcode, taxform");
        assert_eq!(rows[2].detail, "person, account, period");
    }

    #[test]
    fn at_query_filters_servers_and_a_v1_plugin_replaces_its_mcp_namesake() {
        let ms = [manifest("books", &["legacy"], &[]), manifest("gallery", &[], &["books"])];
        let ps = [plugin("books", "Books", true, &["person"]), plugin("ebirforms", "eBIR Forms", true, &["taxprofile"])];
        let rows = server_rows("bo", &ms, &ps, &[]);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].pick == Pick::Server("books".to_owned()));
        assert_eq!(rows[0].detail, "person");
        // a title match counts too
        let rows = server_rows("bir", &ms, &ps, &[]);
        assert_eq!(rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), ["@ebirforms"]);
        assert!(server_rows("zzz", &ms, &ps, &[]).is_empty());
    }

    #[test]
    fn a_plugin_with_accounts_offers_its_account_commands_under_its_short() {
        let ps = [plugin("gws", "Google Workspace", true, &[]), plugin("viewer", "Viewer", true, &[]), plugin("books", "Books", true, &["person"])];
        let rows = server_rows("", &[], &ps, &["gws".to_owned()]);
        assert_eq!(rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), ["@gws", "@books"], "no scopes but accounts: a row; neither: none");
        assert_eq!(rows[0].detail, "login, logout, switch, setup");
        let cmds: Vec<String> = ["login", "logout", "switch", "setup"].iter().map(|c| c.to_string()).collect();
        let all = v1_account_rows("gws", "", &cmds);
        assert_eq!(all.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), ["@gws:login", "@gws:logout", "@gws:switch", "@gws:setup"]);
        assert!(all[0].pick == Pick::V1Account("gws".to_owned(), "login".to_owned()));
        assert_eq!(v1_account_rows("gws", "lo", &cmds).len(), 2);
        assert!(v1_account_rows("gws", "", &[]).is_empty(), "a plugin without accounts offers none");
    }

    #[test]
    fn at_server_colon_lists_its_scopes_and_scope_colon_its_values() {
        let p = plugin("ebirforms", "eBIR Forms", true, &["taxprofile", "branchcode", "taxform"]);
        let rows = v1_scope_rows(&p, "");
        assert_eq!(rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), [
            "@ebirforms:taxprofile",
            "@ebirforms:branchcode",
            "@ebirforms:taxform"
        ]);
        assert!(rows[0].pick == Pick::Scope("ebirforms".to_owned(), "taxprofile".to_owned()));
        assert_eq!(v1_scope_rows(&p, "TAX").len(), 2);
        assert!(data_only(&rows));
        let found = vec![
            ("123456789/".to_owned(), "Acme Corp".to_owned(), "TIN 123-456-789".to_owned()),
            ("987654321/".to_owned(), "987654321/".to_owned(), String::new()),
        ];
        let rows = value_rows("ebirforms", "taxprofile", found);
        assert!(rows[0].pick == Pick::Value("ebirforms".to_owned(), "taxprofile".to_owned(), "123456789/".to_owned()));
        assert_eq!((rows[0].label.as_str(), rows[0].hint.as_str()), ("Acme Corp", "123456789/"));
        // a label that is the value has no hint
        assert_eq!(rows[1].hint, "");
        assert!(data_only(&rows));
        let ms = [manifest("bir", &["taxpayer"], &[])];
        assert!(data_only(&server_rows("", &ms, &[p], &[])));
    }

    #[test]
    fn tokens_parse_in_the_app_servers_grammar() {
        assert_eq!(parse_token("@bir:taxprofile:123456789"), Some(("bir", "taxprofile", "123456789")));
        assert_eq!(parse_token("@my-srv2:Tax_Profile-1:x"), Some(("my-srv2", "Tax_Profile-1", "x")));
        // the value keeps `:` and `/` (a branch code, a path)
        assert_eq!(parse_token("@bir:taxprofile:123/00001"), Some(("bir", "taxprofile", "123/00001")));
        assert_eq!(parse_token("@bir:form:1601C:2026:3"), Some(("bir", "form", "1601C:2026:3")));
        // empty value: the server's `\S*` reads it too (its resolver offers choices)
        assert_eq!(parse_token("@bir:taxprofile:"), Some(("bir", "taxprofile", "")));
    }

    #[test]
    fn tokens_outside_the_grammar_are_no_tokens() {
        for w in [
            "bir:taxprofile:1", // no `@`
            "@bir", // no scope
            "@bir:taxprofile", // no second `:`
            "@:taxprofile:1", // empty server
            "@bir::1", // empty scope
            "@BIR:taxprofile:1", // upper-case server
            "@b_r:taxprofile:1", // `_` in a server
            "@b.r:taxprofile:1", // `.` in a server
            "@bir:tax.profile:1", // `.` in a scope
            "@bir:tax/p:1", // `/` in a scope
            "@bir:taxé:1", // non-ASCII scope
            "@bir:taxprofile:1 2", // whitespace in the value
            "@bir:taxprofile:1\t",
            "",
            "@",
        ] {
            assert_eq!(parse_token(w), None, "{w:?}");
        }
    }

    fn command_row(name: &str) -> PopRow {
        PopRow { pick: Pick::V1Command(name.to_owned()), label: format!("/{name}"), hint: String::new(), detail: String::new() }
    }

    #[test]
    fn enter_sends_what_is_typed_over_rows_for_an_older_text() {
        // `/books` rows still showing, `/books month-end` typed since
        let rows = [command_row("books"), command_row("books-close")];
        assert!(on_enter(&rows, 0, "/books", "/books month-end") == Enter::Send);
        // the rows were for `/bo`: `/books` typed in full is sent as it is
        assert!(on_enter(&rows, 1, "/bo", "/books") == Enter::Send);
    }

    #[test]
    fn enter_picks_a_completion_computed_for_the_text_as_typed() {
        let rows = [command_row("books"), command_row("books-close")];
        assert!(on_enter(&rows, 0, "/books", "/books") == Enter::Pick(&rows[0]));
        assert!(on_enter(&rows, 1, "/bo", "/bo") == Enter::Pick(&rows[1]));
        // a selection past the end is the last row
        assert!(on_enter(&rows, 9, "/bo", "/bo") == Enter::Pick(&rows[1]));
        // no rows: what's typed
        assert!(on_enter(&[], 0, "/bo", "/bo") == Enter::Send);
    }
}
