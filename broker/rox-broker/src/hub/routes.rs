//! Every route of the Node broker (drafted from `server/tools/route-inventory.mjs`),
//! by who answers it with the hub on (`ROX_BROKER_HUB=1`, plan D30):
//!
//! - `Hub(track)`: the brain's, answered here and never forwarded; one no
//!   track has registered yet answers 501 `not_ported`.
//!   `/office/*` and `/file/render` for office files are the hub's since
//!   6.1 (`Hub(OfficeFiles)`), `/popout` (6.3) too,
//!   and `GET /` and `/file/render` for every other kind since phase 7
//!   (`Hub(Files)`): no route is Node's office role's any more, so the hub
//!   runs without Node (`ROX_BROKER_NO_NODE=1`). `/highlight` is deleted
//!   (6.2: no caller), and `/mermaid*` too (Rox draws every diagram natively).
//! - `Rust`: already answered by the front door (leaf, `/mcp/*`, the line);
//!   with that part switched off, the hub answers 501 instead of Node.
//!
//! A path in no entry gets the hub's copy of Node's last resort (404); an
//! OPTIONS preflight goes to Node, or with no Node the hub answers it as
//! Node does (204).
//! The first matching entry wins: specific paths before the prefixes.
//!
//! Deleted in 6.2 (no caller; drafts' route inventory has the evidence):
//! `POST /highlight`. It has no entry, so with the hub on it gets the
//! hub's 404; plain Node still answers it until phase 7 (D11/D12).

/// the agents' host routes, `ah_` webhooks and the bridge (5.6, track E)
pub mod agents;
pub mod bots;
pub mod chat;
pub mod groups;
pub mod market;
pub mod mcp_chat;

use hyper::Method;

/// the Rox-facing `/v1/host/*` routes, the `ri_` webhooks and the host
/// token (5.5h, track D)
pub mod host;
/// `GET /models` (5.3, track B)
pub mod models;
/// track L's leaves
pub mod leftovers;
/// v0 routines (5.5, track D)
pub mod routines;
/// the mini-app and Library routes (5.7b, track C)
pub mod apps;
pub mod artifacts;
/// pop-out windows and their table (6.3)
pub mod popout;

use super::types::Pattern::{self, Exact as X, Prefix as P, Segments as S};
use super::types::Verb::{self, Any, Get, Post};

/// The track that ports a hub route (the inventory's §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Track {
    /// 5.2, track A: chats, bots, groups, chat-side MCP config
    Chat,
    /// 5.3, track B
    Models,
    /// 5.5, track D: v0 and v1 routines
    Routines,
    /// 5.5h, track D: the Rox-facing plugin host routes and the host token
    Host,
    /// 5.6, track E: agent messaging and rox-bridge
    Agents,
    /// the skeleton's `/v1/hooks/` dispatcher (`ah_` → Agents, else Routines)
    Hooks,
    /// 5.7b, track C: artifacts, widgets and the library
    Artifacts,
    /// 5.8, track C: the plugin market and skills
    Market,
    /// track L: leaves no 5.x item names
    Leftovers,
    /// 6.1: office in Rust (`/office/*`, `/file/render` for office files)
    OfficeFiles,
    /// phase 7: `/file/render` for every other kind, and `GET /`
    Files,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    Hub(Track),
    Rust,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub verb: Verb,
    pub pattern: Pattern,
    pub owner: Owner,
}

const fn e(verb: Verb, pattern: Pattern, owner: Owner) -> Entry {
    Entry { verb, pattern, owner }
}

use Owner::{Hub, Rust};
use Track::*;

pub const ROUTES: &[Entry] = &[
    // the front door itself and the line
    e(Get, X("/health"), Rust),
    e(Get, X("/meta"), Rust),
    e(Any, P("/v1/line/"), Rust),
    // 5.2 chats
    e(Post, X("/chat"), Hub(Chat)),
    e(Get, P("/status"), Hub(Chat)),
    e(Get, X("/threads"), Hub(Chat)),
    e(Get, X("/turns"), Hub(Chat)),
    e(Post, X("/thread/new"), Hub(Chat)),
    e(Post, X("/thread/update"), Hub(Chat)),
    e(Post, X("/thread/delete"), Hub(Chat)),
    e(Post, X("/thread/select"), Hub(Chat)),
    e(Get, X("/thread/apps"), Hub(Chat)),
    e(Post, X("/turn/delete"), Hub(Chat)),
    e(Post, X("/react"), Hub(Chat)),
    e(Post, X("/retry"), Hub(Chat)),
    e(Post, X("/copy"), Hub(Chat)),
    e(Post, X("/share"), Hub(Chat)),
    e(Post, X("/share/thread"), Hub(Chat)),
    e(Post, X("/bot/new"), Hub(Chat)),
    e(Post, X("/bot/update"), Hub(Chat)),
    e(Post, X("/bot/memory"), Hub(Chat)),
    e(Post, X("/bot/integration"), Hub(Chat)),
    e(Post, X("/bot/delete"), Hub(Chat)),
    e(Post, P("/avatar/image/"), Hub(Chat)),
    e(Post, X("/avatar/gen"), Hub(Chat)),
    e(Post, X("/attach/pick"), Hub(Chat)),
    e(Get, X("/groups"), Hub(Chat)),
    e(Post, X("/group/new"), Hub(Chat)),
    e(Post, X("/group/update"), Hub(Chat)),
    e(Post, X("/group/delete"), Hub(Chat)),
    e(Post, X("/group/item"), Hub(Chat)),
    e(Post, X("/mcp/servers"), Hub(Chat)),
    e(Any, X("/mcp/attach"), Hub(Chat)),
    e(Get, X("/mcp/attachments"), Hub(Chat)),
    // 5.3 the model client
    e(Get, X("/models"), Hub(Models)),
    // 5.5 routines
    e(Get, X("/routines"), Hub(Routines)),
    e(Get, X("/routines/templates"), Hub(Routines)),
    e(Any, X("/routine/update"), Hub(Routines)),
    e(Any, X("/routine/delete"), Hub(Routines)),
    e(Any, X("/routine/run"), Hub(Routines)),
    e(Any, X("/routine/from-template"), Hub(Routines)),
    e(Post, X("/v1/host/routines/approve"), Hub(Routines)),
    e(Post, X("/v1/host/routines/enable"), Hub(Routines)),
    e(Post, X("/v1/host/routines/disable"), Hub(Routines)),
    e(Post, X("/v1/host/routines/run"), Hub(Routines)),
    e(Post, X("/v1/host/routines/update"), Hub(Routines)),
    e(Post, X("/v1/host/routines/delete"), Hub(Routines)),
    e(Post, X("/v1/host/routines/hook"), Hub(Routines)),
    // 5.6 agents
    e(Post, X("/v1/host/agents/archive"), Hub(Agents)),
    e(Post, X("/v1/host/agents/resume"), Hub(Agents)),
    e(Post, X("/v1/host/agents/accepts"), Hub(Agents)),
    e(Post, X("/v1/host/agents/endpoints/create"), Hub(Agents)),
    e(Post, X("/v1/host/agents/endpoints/list"), Hub(Agents)),
    e(Post, X("/v1/host/agents/endpoints/rotate"), Hub(Agents)),
    e(Post, X("/v1/host/agents/endpoints/delete"), Hub(Agents)),
    e(Any, X("/v1/bridge/agents"), Hub(Agents)),
    e(Post, X("/v1/bridge/send"), Hub(Agents)),
    e(Post, X("/v1/bridge/receive"), Hub(Agents)),
    e(Any, X("/v1/bridge/conversations"), Hub(Agents)),
    e(Any, P("/v1/bridge/"), Hub(Agents)),
    // the webhooks: ah_… (agents) and ri_… (routines)
    e(Any, P("/v1/hooks/"), Hub(Hooks)),
    // 5.5h the Rox-facing plugin host (finding 1)
    e(Get, X("/v1/host/plugins"), Hub(Host)),
    e(Get, X("/v1/host/events"), Hub(Host)),
    e(Post, X("/v1/host/plugins/inspect"), Hub(Host)),
    e(Post, X("/v1/host/plugins/install"), Hub(Host)),
    e(Post, S("/v1/host/plugins/*/enable"), Hub(Host)),
    e(Post, S("/v1/host/plugins/*/disable"), Hub(Host)),
    e(Post, S("/v1/host/plugins/*/uninstall"), Hub(Host)),
    e(Post, S("/v1/host/plugins/*/tools"), Hub(Host)),
    e(Post, X("/v1/host/sessions/open"), Hub(Host)),
    e(Post, X("/v1/host/sessions/close"), Hub(Host)),
    e(Post, X("/v1/host/call"), Hub(Host)),
    e(Post, X("/v1/host/tickets/title"), Hub(Host)),
    e(Post, X("/v1/host/resolve"), Hub(Host)),
    e(Post, X("/v1/host/apps/source"), Hub(Host)),
    // the rest of /v1/host/ (Node's 401, 405 and 404 answers)
    e(Any, P("/v1/host/"), Hub(Host)),
    // 5.7b artifacts and widgets
    e(Get, P("/module"), Hub(Artifacts)),
    e(Post, X("/app/meta"), Hub(Artifacts)),
    e(Get, X("/app/info"), Hub(Artifacts)),
    e(Get, X("/app/icon"), Hub(Artifacts)),
    e(Post, X("/app/icon/gen"), Hub(Artifacts)),
    e(Post, X("/app/version"), Hub(Artifacts)),
    e(Post, X("/app/version/label"), Hub(Artifacts)),
    e(Post, X("/app/delete"), Hub(Artifacts)),
    e(Get, X("/apps/meta"), Hub(Artifacts)),
    e(Get, X("/artifacts"), Hub(Artifacts)),
    e(Post, X("/artifact/meta"), Hub(Artifacts)),
    e(Post, X("/artifact/rename-remote"), Hub(Artifacts)),
    e(Get, X("/artifact/info"), Hub(Artifacts)),
    e(Post, X("/artifact/hide"), Hub(Artifacts)),
    e(Post, X("/artifact/delete"), Hub(Artifacts)),
    e(Get, X("/artifact/icon"), Hub(Artifacts)),
    e(Get, X("/widgets"), Hub(Artifacts)),
    e(Post, X("/publish"), Hub(Artifacts)),
    e(Post, X("/unpublish"), Hub(Artifacts)),
    e(Post, X("/reuse"), Hub(Artifacts)),
    e(Get, X("/catalog"), Hub(Artifacts)),
    e(Post, X("/widget-meta"), Hub(Artifacts)),
    e(Get, X("/sources"), Hub(Artifacts)),
    // 5.8 market and skills
    e(Get, P("/plugins/catalog"), Hub(Market)),
    e(Get, P("/plugins/detail"), Hub(Market)),
    e(Get, P("/plugins/icon/"), Hub(Market)),
    e(Get, P("/plugins/skill"), Hub(Market)),
    e(Post, X("/plugins/inspect"), Hub(Market)),
    e(Post, X("/plugins/install"), Hub(Market)),
    e(Post, X("/plugins/uninstall"), Hub(Market)),
    e(Post, X("/plugins/settings"), Hub(Market)),
    e(Post, X("/plugins/connect"), Hub(Market)),
    e(Post, X("/skills"), Hub(Market)),
    e(Post, X("/skills/state"), Hub(Market)),
    e(Post, X("/skills/delete"), Hub(Market)),
    // L leftovers
    e(Get, X("/data/linear/issues"), Hub(Leftovers)),
    e(Get, X("/data/linear/states"), Hub(Leftovers)),
    e(Post, X("/data/linear/assign"), Hub(Leftovers)),
    e(Get, X("/data/reminders"), Hub(Leftovers)),
    e(Post, X("/data/reminders/add"), Hub(Leftovers)),
    e(Post, X("/data/reminders/complete"), Hub(Leftovers)),
    e(Post, X("/file/write"), Hub(Leftovers)),
    e(Post, X("/file/open"), Hub(Leftovers)),
    e(Post, X("/file/save"), Hub(Leftovers)),
    // pop-out windows (6.3; Node's office role's until then, D36)
    e(Post, X("/popout"), Hub(Artifacts)),
    // office: in Rust since 6.1 (`classify` gives office files'
    // `/file/render` to it); the other kinds and `/` since phase 7
    e(Get, X("/"), Hub(Files)),
    e(Get, P("/office/"), Hub(OfficeFiles)),
    e(Get, X("/file/render"), Hub(Files)),
    // already Rust's: leaf (phase 2)
    e(Any, X("/prefs"), Rust),
    e(Any, X("/keymap"), Rust),
    e(Get, X("/data/timeclock"), Rust),
    e(Post, X("/data/timeclock/in"), Rust),
    e(Post, X("/data/timeclock/out"), Rust),
    e(Get, X("/skills"), Rust),
    e(Get, P("/file/raw/"), Rust),
    e(Get, X("/file/versions"), Rust),
    e(Get, P("/files/attachments/"), Rust),
    e(Get, P("/files/avatars/"), Rust),
    e(Get, P("/asset/generated/assets/"), Rust),
    e(Get, P("/state"), Rust),
    e(Post, P("/state"), Rust),
    // already Rust's: /mcp (phase 4)
    e(Get, X("/mcp/servers"), Rust),
    e(Get, X("/mcp/apps"), Rust),
    e(Get, X("/mcp/manifest"), Rust),
    e(Post, X("/mcp/tools/state"), Rust),
    e(Get, P("/mcp/tools"), Rust),
    e(Post, X("/mcp/server/state"), Rust),
    e(Post, X("/mcp/server/delete"), Rust),
    e(Post, X("/mcp/call"), Rust),
    e(Post, X("/mcp/resolve"), Rust),
    e(Post, X("/mcp/detach"), Rust),
    e(Post, X("/mcp/attachment/rename"), Rust),
    e(Post, X("/mcp/auth"), Rust),
    e(Post, X("/mcp/account"), Rust),
    e(Post, X("/mcp/open"), Rust),
    e(Post, X("/mcp/open/pass"), Rust),
    e(Get, P("/oauth/callback/"), Rust),
];

/// The entry a request falls under, or `None` (the hub's 404 with the hub
/// on; Node's, as today, with it off).
pub fn classify(method: &Method, url: &str) -> Option<&'static Entry> {
    if method == Method::OPTIONS {
        return None;
    }
    let path = url.split('?').next().unwrap_or("");
    if method == Method::GET && path == "/file/render" && crate::office::routes::renders(url) {
        return Some(&OFFICE_RENDER);
    }
    ROUTES.iter().find(|r| r.verb.allows(method) && r.pattern.matches(path).is_some())
}

/// `/file/render` of a docx, xlsx or pptx file: the hub's (6.1).
const OFFICE_RENDER: Entry = e(Get, X("/file/render"), Hub(OfficeFiles));

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(method: Method, url: &str) -> Option<Owner> {
        classify(&method, url).map(|r| r.owner)
    }

    /// A path the entry's pattern matches.
    fn sample(p: Pattern) -> String {
        match p {
            Pattern::Exact(p) => p.to_owned(),
            Pattern::Prefix(p) => format!("{p}{}", if p.ends_with('/') { "x" } else { "" }),
            Pattern::Segments(p) => p.replace('*', "x"),
        }
    }

    fn method(v: Verb) -> Method {
        if v == Post {
            Method::POST
        } else {
            Method::GET
        }
    }

    #[test]
    fn hub_routes_classify_by_method_and_path() {
        assert_eq!(owner(Method::GET, "/threads"), Some(Hub(Chat)));
        assert_eq!(owner(Method::GET, "/turns?thread=t5"), Some(Hub(Chat)));
        assert_eq!(owner(Method::GET, "/status?id=3"), Some(Hub(Chat)));
        assert_eq!(owner(Method::GET, "/skills"), Some(Rust));
        assert_eq!(owner(Method::POST, "/skills"), Some(Hub(Market)));
        assert_eq!(owner(Method::POST, "/mcp/servers"), Some(Hub(Chat)));
        assert_eq!(owner(Method::GET, "/mcp/servers"), Some(Rust));
        assert_eq!(owner(Method::POST, "/v1/host/plugins/acme/enable"), Some(Hub(Host)));
        assert_eq!(owner(Method::POST, "/v1/host/agents/resume"), Some(Hub(Agents)));
        assert_eq!(owner(Method::DELETE, "/v1/host/plugins"), Some(Hub(Host)));
        assert_eq!(owner(Method::POST, "/v1/hooks/ah_x/secret"), Some(Hub(Hooks)));
        assert_eq!(owner(Method::GET, "/v1/bridge/nope"), Some(Hub(Agents)));
        assert_eq!(owner(Method::GET, "/office/caps"), Some(Hub(OfficeFiles)));
        assert_eq!(owner(Method::GET, "/file/render?path=a.docx"), Some(Hub(OfficeFiles)));
        assert_eq!(owner(Method::GET, "/file/render?path=d%2FB.XLSX"), Some(Hub(OfficeFiles)));
        assert_eq!(owner(Method::GET, "/file/render?path=a.md"), Some(Hub(Files)));
        assert_eq!(owner(Method::GET, "/file/render?path=app:R3"), Some(Hub(Files)));
        assert_eq!(owner(Method::POST, "/popout"), Some(Hub(Artifacts)));
        assert_eq!(owner(Method::POST, "/highlight"), None);
        assert_eq!(owner(Method::GET, "/mermaid?d=abc"), None);
        assert_eq!(owner(Method::GET, "/"), Some(Hub(Files)));
        assert_eq!(owner(Method::GET, "/state/R3"), Some(Rust));
        assert_eq!(owner(Method::GET, "/oauth/callback/x"), Some(Rust));
        assert_eq!(owner(Method::OPTIONS, "/threads"), None);
        assert_eq!(owner(Method::GET, "/echo"), None);
        assert_eq!(owner(Method::GET, "/chat"), None);
    }

    #[test]
    fn hub_table_agrees_with_what_rust_already_serves() {
        for r in ROUTES {
            let (m, url) = (method(r.verb), sample(r.pattern));
            // the leaf matches /file/versions only with its query
            let url = if url == "/file/versions" { format!("{url}?path=x") } else { url };
            let served = crate::leaf::route(&m, &url).is_some() || crate::leaf::mcp::route(&m, &url) || ["/health", "/meta"].contains(&url.as_str()) || url.starts_with("/v1/line/");
            assert_eq!(served, r.owner == Rust, "{:?} {url}", r.verb);
            assert_eq!(classify(&m, &url), Some(r), "{url} reaches its own entry first");
        }
    }

    #[test]
    fn hub_table_counts() {
        let count = |f: &dyn Fn(Owner) -> bool| ROUTES.iter().filter(|r| f(r.owner)).count();
        assert_eq!(count(&|o| matches!(o, Hub(_))), 121);
        assert_eq!(count(&|o| o == Rust), 32);
    }
}
