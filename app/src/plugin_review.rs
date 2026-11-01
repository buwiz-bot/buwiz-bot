//! A plain plugin's review before it installs (extension format 2.1 §2.2,
//! D2): Connect Apps' Add asks the broker to stage the plugin
//! (`POST /plugins/inspect {name}`), shows what it would add, and only
//! Install sends `POST /plugins/install {name, staging, approve: true,
//! values?}`. The broker refuses an install without both.
//!
//! The requests go through a `post` function (the app's `client::post_any`),
//! so the tests drive them with a fake broker.

use std::collections::BTreeMap;
use std::future::Future;

use serde_json::{Value, json};

/// One server as the review shows it: never a value, only names.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Server {
    pub name: String,
    /// "command" or "remote".
    pub transport: String,
    /// The command line, or the URL.
    pub target: String,
    pub env: Vec<String>,
    pub headers: Vec<String>,
    /// "oauth" or "none".
    pub auth: String,
    pub unavailable: String,
}

/// `POST /plugins/inspect`'s answer.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlainReview {
    pub name: String,
    pub staging: String,
    pub title: String,
    pub publisher: String,
    pub author: String,
    pub version: String,
    pub description: String,
    /// The plugin has setup values to ask for before it installs.
    pub setup: bool,
    pub unsigned: String,
    pub servers: Vec<Server>,
    /// Skill names.
    pub skills: Vec<String>,
    /// What it ships that Buwiz lists but never runs.
    pub commands: Vec<String>,
    pub agents: Vec<String>,
    pub hooks: bool,
    pub lsp: bool,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_owned()
}

fn strings(v: &Value, k: &str) -> Vec<String> {
    v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default()
}

/// A refusal's message (`{ok: false, error}`), or a stand-in.
fn refusal(v: Option<&Value>, fallback: &str) -> String {
    v.map(|v| s(v, "error")).filter(|e| !e.is_empty()).unwrap_or_else(|| fallback.to_owned())
}

impl PlainReview {
    pub fn of(answer: &Value) -> Result<PlainReview, String> {
        if answer.get("ok") != Some(&json!(true)) || s(answer, "staging").is_empty() {
            return Err(refusal(Some(answer), "Couldn't review the plugin."));
        }
        let r = answer.get("review").cloned().unwrap_or(Value::Null);
        let listed = r.get("listed").cloned().unwrap_or(Value::Null);
        let servers = r.get("servers").and_then(Value::as_array).cloned().unwrap_or_default();
        Ok(PlainReview {
            name: s(answer, "name"),
            staging: s(answer, "staging"),
            title: s(&r, "title"),
            publisher: s(&r, "publisher"),
            author: s(&r, "author"),
            version: s(&r, "version"),
            description: s(&r, "description"),
            setup: answer.get("setup") == Some(&json!(true)),
            unsigned: s(&r, "unsigned"),
            servers: servers
                .iter()
                .map(|v| Server {
                    name: s(v, "name"),
                    transport: s(v, "transport"),
                    target: if s(v, "url").is_empty() { s(v, "command") } else { s(v, "url") },
                    env: strings(v, "env"),
                    headers: strings(v, "headers"),
                    auth: s(v, "auth"),
                    unavailable: s(v, "unavailable"),
                })
                .collect(),
            skills: r.get("skills").and_then(Value::as_array).map(|a| a.iter().map(|k| s(k, "name")).filter(|n| !n.is_empty()).collect()).unwrap_or_default(),
            commands: strings(&listed, "commands"),
            agents: strings(&listed, "agents"),
            hooks: listed.get("hooks") == Some(&json!(true)),
            lsp: listed.get("lsp") == Some(&json!(true)),
        })
    }

    /// "By Demo Co", with the manifest's author when it's someone else.
    pub fn byline(&self) -> String {
        match (self.publisher.as_str(), self.author.as_str()) {
            ("", "") => String::new(),
            (p, "") => format!("By {p}"),
            ("", a) => format!("By {a}"),
            (p, a) if p == a => format!("By {p}"),
            (p, a) => format!("By {p} · made by {a}"),
        }
    }

    /// The card's lines, in Connect Apps' review look (an indented line
    /// sits under the one before it).
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        for sv in &self.servers {
            out.push(if sv.transport == "remote" {
                format!("Server {}: connects to {}", sv.name, sv.target)
            } else {
                format!("Server {}: runs {}", sv.name, sv.target)
            });
            if !sv.env.is_empty() {
                out.push(format!("  Environment: {}", sv.env.join(", ")));
            }
            if !sv.headers.is_empty() {
                out.push(format!("  Headers: {}", sv.headers.join(", ")));
            }
            out.push(if sv.auth == "oauth" { "  Signs in with OAuth".to_owned() } else { "  No sign-in".to_owned() });
            if !sv.unavailable.is_empty() {
                out.push(format!("  Unavailable: {}", sv.unavailable));
            }
        }
        if !self.skills.is_empty() {
            out.push(format!("Skills: {}", self.skills.join(", ")));
        }
        let mut listed = Vec::new();
        if !self.commands.is_empty() {
            listed.push(format!("commands {}", self.commands.join(", ")));
        }
        if !self.agents.is_empty() {
            listed.push(format!("agents {}", self.agents.join(", ")));
        }
        if self.hooks {
            listed.push("hooks".to_owned());
        }
        if self.lsp {
            listed.push("a language server config".to_owned());
        }
        if !listed.is_empty() {
            out.push(format!("Listed, not run: {}", listed.join("; ")));
        }
        if out.is_empty() {
            out.push("No servers or skills".to_owned());
        }
        out
    }

    /// As the mini-app review's details take it (`plugins::review_details`).
    pub fn details(&self) -> crate::mini_v1::connect::Review {
        crate::mini_v1::connect::Review {
            title: self.title.clone(),
            publisher: self.publisher.clone(),
            version: self.version.clone(),
            description: self.description.clone(),
            lines: self.lines(),
            warnings: vec![if self.unsigned.is_empty() { "Unsigned: Buwiz can't check who made it".to_owned() } else { self.unsigned.clone() }],
            ..Default::default()
        }
    }
}

pub fn inspect_body(name: &str) -> Value {
    json!({ "name": name })
}

/// Install what `review` staged: the approval is the person's Install
/// press; setup values (empty ones left out) go with it.
pub fn install_body(review: &PlainReview, values: &BTreeMap<String, String>) -> Value {
    let mut body = json!({ "name": review.name, "staging": review.staging, "approve": true });
    let values: serde_json::Map<String, Value> = values.iter().filter(|(_, v)| !v.trim().is_empty()).map(|(k, v)| (k.clone(), json!(v))).collect();
    if !values.is_empty() {
        body["values"] = Value::Object(values);
    }
    body
}

/// `POST /plugins/inspect`: the review (nothing is installed).
pub async fn inspect<P, F>(post: P, name: &str) -> Result<PlainReview, String>
where
    P: Fn(&'static str, Value) -> F,
    F: Future<Output = Option<Value>>,
{
    match post("/plugins/inspect", inspect_body(name)).await {
        Some(v) => PlainReview::of(&v),
        None => Err("Couldn't reach the broker.".to_owned()),
    }
}

/// `POST /plugins/install` for what `review` showed.
pub async fn install<P, F>(post: P, review: &PlainReview, values: &BTreeMap<String, String>) -> Result<(), String>
where
    P: Fn(&'static str, Value) -> F,
    F: Future<Output = Option<Value>>,
{
    let v = post("/plugins/install", install_body(review, values)).await;
    match &v {
        Some(v) if v.get("ok") == Some(&json!(true)) => Ok(()),
        Some(v) if s(v, "code") == "review_expired" => Err("This review expired. Add the plugin again to review it.".to_owned()),
        _ => Err(refusal(v.as_ref(), "Couldn't add the plugin.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Runs a future the fake broker answers at once.
    fn now<T>(f: impl Future<Output = T>) -> T {
        let mut f = std::pin::pin!(f);
        match f.as_mut().poll(&mut std::task::Context::from_waker(std::task::Waker::noop())) {
            std::task::Poll::Ready(v) => v,
            std::task::Poll::Pending => panic!("the fake broker answers at once"),
        }
    }

    /// Records every request and answers from `answers` by route.
    #[derive(Default)]
    struct Fake {
        sent: Mutex<Vec<(String, Value)>>,
        install: Mutex<Option<Value>>,
    }

    impl Fake {
        fn post(&self) -> impl Fn(&'static str, Value) -> std::future::Ready<Option<Value>> + '_ {
            move |route, body| {
                self.sent.lock().unwrap().push((route.to_owned(), body));
                std::future::ready(match route {
                    "/plugins/inspect" => Some(inspect_answer()),
                    _ => Some(self.install.lock().unwrap().clone().unwrap_or(json!({"ok": true, "name": "demo"}))),
                })
            }
        }
    }

    fn inspect_answer() -> Value {
        json!({
            "ok": true, "staging": "ab".repeat(16), "name": "demo", "expires": 1800000, "setup": true,
            "review": {
                "title": "Demo", "publisher": "Demo Co", "author": "Ada", "version": "1.2.0", "description": "A demo.",
                "signed": false, "unsigned": "Unsigned: Buwiz can't check who made it",
                "servers": [
                    {"name": "demo-cli", "transport": "command", "url": null, "command": "npx -y demo-mcp", "env": ["DEMO_API_KEY", "DEMO_REGION"], "headers": [], "auth": "none", "unavailable": null},
                    {"name": "demo-web", "transport": "remote", "url": "https://mcp.demo.invalid/mcp", "command": null, "env": [], "headers": [], "auth": "oauth", "unavailable": null},
                ],
                "skills": [{"name": "hello", "description": "Says hello."}],
                "listed": {"commands": ["run"], "agents": [], "hooks": true, "lsp": false},
            },
        })
    }

    #[test]
    fn plain_review_add_inspects_first_and_install_sends_the_staging_token() {
        let b = Fake::default();
        let r = now(inspect(b.post(), "demo")).unwrap();
        assert_eq!(*b.sent.lock().unwrap(), [("/plugins/inspect".to_owned(), json!({"name": "demo"}))]);
        assert_eq!((r.title.as_str(), r.byline(), r.setup), ("Demo", "By Demo Co · made by Ada".to_owned(), true));
        assert_eq!(
            r.lines(),
            [
                "Server demo-cli: runs npx -y demo-mcp",
                "  Environment: DEMO_API_KEY, DEMO_REGION",
                "  No sign-in",
                "Server demo-web: connects to https://mcp.demo.invalid/mcp",
                "  Signs in with OAuth",
                "Skills: hello",
                "Listed, not run: commands run; hooks",
            ]
        );
        assert_eq!(r.details().warnings, ["Unsigned: Buwiz can't check who made it"]);
        // Install: the staging token and the approval, with the setup values
        let vals = BTreeMap::from([("DEMO_API_KEY".to_owned(), "k-1".to_owned()), ("BLANK".to_owned(), " ".to_owned())]);
        assert_eq!(now(install(b.post(), &r, &vals)), Ok(()));
        let sent = b.sent.lock().unwrap().last().cloned().unwrap();
        assert_eq!(sent, ("/plugins/install".to_owned(), json!({"name": "demo", "staging": "ab".repeat(16), "approve": true, "values": {"DEMO_API_KEY": "k-1"}})));
        // no values: none sent
        assert_eq!(install_body(&r, &BTreeMap::new()), json!({"name": "demo", "staging": "ab".repeat(16), "approve": true}));
    }

    #[test]
    fn plain_review_refusals_come_back_as_messages() {
        let b = Fake::default();
        let r = now(inspect(b.post(), "demo")).unwrap();
        *b.install.lock().unwrap() = Some(json!({"ok": false, "code": "review_expired", "error": "this review expired; review the plugin again"}));
        assert_eq!(now(install(b.post(), &r, &BTreeMap::new())), Err("This review expired. Add the plugin again to review it.".to_owned()));
        *b.install.lock().unwrap() = Some(json!({"ok": false, "error": "Demo needs Demo API Key before it can be added"}));
        assert_eq!(now(install(b.post(), &r, &BTreeMap::new())), Err("Demo needs Demo API Key before it can be added".to_owned()));
        assert_eq!(PlainReview::of(&json!({"ok": false, "error": "unknown plugin x"})), Err("unknown plugin x".to_owned()));
        let unreachable = |_: &'static str, _: Value| std::future::ready(None);
        assert_eq!(now(inspect(unreachable, "demo")), Err("Couldn't reach the broker.".to_owned()));
    }
}
