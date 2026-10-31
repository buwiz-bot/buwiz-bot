# buwiz-app

Native Rust screens for first-party Buwiz Bot mini apps.

A mini app's screen is either TSX, which a plugin serves and Buwiz Bot
compiles and runs in a sandbox, or a native screen: an nsh Rust component
compiled into Buwiz Bot. Native screens are for first-party plugins only
(the broker and the host both refuse others). TSX stays the way for
third-party plugins.

Both kinds of screen open the same way. Each open gets its own view, session
and session key, the permissions its manifest grants, signed calls, the
bridge's limits (sizes, 8 calls in flight, 20 a second, 30 s each), and an
inline card's fit, ready, footer buttons, status and settle. The host runs
the same checks for a native screen's calls as for a TSX app's. This crate
only gives them a Rust shape.

This crate lives in the Buwiz Bot repository for now. It will move to the
SDK repository, `buwiz-app/buwiz-sdk`, as `crates/app` (beside
`crates/plugin`, `buwiz-plugin`, and mirroring `packages/app`), so it
depends on nothing of Buwiz Bot's, only on nsh.

## A plugin's screens crate

A first-party plugin keeps its native screens in a crate of its own, in the
plugin's repository. The crate depends on this one and on `nsh-core`, and it
exports its screens:

```rust
use buwiz_app::{ABI, Action, Bridge, ScreenDef};
use nsh_core::prelude::*;

pub fn screens() -> &'static [ScreenDef] {
    &[ScreenDef { id: "books.ledger", abi: ABI, build: ledger }]
}

fn ledger(bridge: Bridge) -> Root {
    buwiz_app::root(bridge, |b| {
        let total = signal(String::from("…"));
        let pressed = signal(String::new());
        b.on_action(move |id| pressed.set(id.to_owned()));
        let b2 = b.clone();
        b.spawn(async move {
            let _ = b2.actions(vec![Action::new("export", "Export").primary()]).await;
            match b2.call("books_total", serde_json::json!({})).await {
                Ok(v) => total.set(v.to_string()),
                Err(e) => total.set(e.message),
            }
            b2.ready();
        });
        text(move || total.get())
    })
}
```

The manifest names the screen in place of a TSX entry:

```json
{ "id": "ledger", "title": "Ledger", "native": { "screen": "books.ledger", "abi": 1 },
  "placement": ["full", "inline"], "permissions": { "tools": ["books_total"] } }
```

Buwiz Bot lists the screens crates it compiles in, one per plugin id, and
pins each by git rev. A Buwiz Bot without a screen that matches the
manifest's `(screen, abi)` shows "this screen needs a newer Buwiz Bot".

Build the root with `buwiz_app::root`. It puts the screen under an
error boundary, so a panic in its view, a binding, a listener or an effect
reaches the mini app's error banner, as a TSX app's uncaught error does.
Run the screen's tasks with `Bridge::spawn`, which does the same for a
panic in a task: nsh's own `spawn` sends a task's panic to no boundary, so
it is only logged. A root made another way still runs, but its panics are
only logged too.

The plugin test runner checks a manifest with native screens when you run
it with `--first-party` (`server/sdk/mini-app/docs/testing.md`).

## The API

The `buwiz` global, in Rust:

| `buwiz` (TSX) | `Bridge` |
|---|---|
| `buwiz.props`, `buwiz.app` | `props() -> Value`, `app() -> Value` |
| `buwiz.viewport()` | `viewport() -> ReadSignal<Viewport>`, `viewport_now() -> Viewport` |
| `buwiz.fit(height)` | `fit(f32) -> Result<(), BuwizError>` |
| `buwiz.ready()` | `ready()` |
| `buwiz.actions(list)` | `actions(Vec<Action>) -> impl Future<Output = Result<(), BuwizError>>` |
| `buwiz.onAction(f)` | `on_action(impl Fn(&str)) -> Spawned<Option<()>>` |
| `buwiz.meta(meta)` | `meta(Option<Meta>) -> impl Future<…>` |
| `buwiz.settle(text)` | `settle(&str) -> impl Future<…>` |
| `buwiz.call(tool, args)` | `call(&str, Value) -> impl Future<Output = Result<Value, BuwizError>>` |
| `buwiz.open({app, props, ticket?})` | `open(app, props)`, `open_ticket(app, ticket, props)` |
| `buwiz.open({url})` | `open_url(url)` |
| `buwiz.ticket.title(ticket)` | `ticket_title(&str) -> impl Future<Output = Result<Option<String>, BuwizError>>` |
| `host.on("buwiz.session")` | `on_session(impl Fn(SessionEvent)) -> Spawned<Option<()>>` |
| an `async` function | `spawn(impl Future<Output = T>) -> Spawned<Option<T>>` |
| `buwiz.storage` | `storage() -> Option<Storage>`: `get`, `set`, `remove`, `keys` |
| `buwiz.attach(scope, value)` | `attach(scope, value) -> Option<impl Future<Output = Result<Attached, BuwizError>>>` |
| `buwiz.pickFile(options)` | `pick_file(PickOptions) -> Option<impl Future<Output = Result<Vec<PickedFile>, BuwizError>>>`; `PickedFile::read()` |
| `buwiz.copy(text)` | `copy(&str) -> Option<impl Future<…>>` |
| `buwiz.preview({tool, args, title})` | `preview(tool, args, title) -> Option<impl Future<…>>` |

A capability the app's permissions may leave out returns `None` when they
do, as the TSX global leaves it out. A call is sent when it is made, not
when its future is first polled. A rejection is a
`BuwizError { code, retryable, message }`, with the TSX bridge's codes.

`viewport`, `on_action`, `on_session` and `spawn` make signals or tasks. Call them
inside a component's view: they last as long as the component does. The
rest may be called from anywhere the bridge is.

The host implements `Host`. Its methods and their data are the TSX
channel's (`buwiz.call` takes `{tool, args}` and so on), so one set of
handlers serves both kinds of screen.

`ABI` is 1. A screen runs only when the manifest's `native.abi` is the
screen's, and the screen's is one the host runs.
