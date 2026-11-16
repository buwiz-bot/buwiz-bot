//! The `reminders` source: the user's Apple Reminders (what `/data/reminders`
//! answered before widgets called sources). EventKit where the Mac lets it, the
//! osascript JXA otherwise; the commands go through `LocalTools`, so tests stub them.

use std::time::Duration;

use serde_json::{json, Value};

use crate::hub::ctx::HubCtx;
use crate::hub::routes::leftovers::{field, get, parse_error, trim, truthy};
use crate::hub::traits::LocalCommand;
use crate::js;

pub(crate) const HINT: &str = "grant permission in System Settings → Privacy & Security → Reminders / Automation";

pub(crate) const REMINDERS_LIST: &str = r#"
      const app = Application("Reminders");
      const out = [];
      outer: for (const l of app.lists()) {
        const ln = l.name();
        for (const r of l.reminders.whose({completed: false})()) {
          let due = null, flagged = false;
          try { const d = r.dueDate(); if (d) due = d.toISOString(); } catch (e) {}
          try { flagged = !!r.flagged(); } catch (e) {}
          out.push({ name: r.name(), list: ln, due, flagged });
          if (out.length >= 50) break outer;
        }
      }
      JSON.stringify(out);"#;

pub(crate) const REMINDERS_COMPLETE: &str = r#"
            const app = Application("Reminders");
            ObjC.import("Cocoa");
            const env = $.NSProcessInfo.processInfo.environment;
            const name = ObjC.unwrap(env.objectForKey("REM_NAME"));
            let done = false;
            outer: for (const l of app.lists()) {
              const hits = l.reminders.whose({completed: false, name})();
              if (hits.length) { hits[0].completed = true; done = true; break outer; }
            }
            JSON.stringify({ ok: done });"#;

pub(crate) const REMINDERS_ADD: &str = r#"
            const app = Application("Reminders");
            ObjC.import("Cocoa");
            const env = $.NSProcessInfo.processInfo.environment;
            const get = (k) => { const v = env.objectForKey(k); return v ? ObjC.unwrap(v) : null; };
            const name = get("REM_NAME"), listName = get("REM_LIST"), due = get("REM_DUE");
            let list = null;
            if (listName) { const hits = app.lists.whose({name: listName})(); if (hits.length) list = hits[0]; }
            if (!list) list = app.defaultList();
            const props = { name };
            if (due) props.dueDate = new Date(due);
            list.reminders.push(app.Reminder(props));
            JSON.stringify({ ok: true, name, list: list.name() });"#;

/// The hub's local tools. A unit test that reaches here without giving the hub its own
/// stub would run the real Reminders and Keychain commands on the machine, so it fails.
pub(super) fn local(ctx: &HubCtx) -> std::sync::Arc<dyn crate::hub::traits::LocalTools> {
    #[cfg(test)]
    return ctx.local.clone().expect("a test gives the hub stub local tools before it calls a source");
    #[cfg(not(test))]
    crate::helper::local::tools(ctx)
}

/// `jxa(script, env)` (state.mjs): `osascript -l JavaScript -e <script>`
/// with a 15 s timeout, its output parsed as JSON, or `{error, hint}`.
async fn jxa(ctx: &HubCtx, script: &str, env: Vec<(String, String)>) -> Value {
    let cmd = LocalCommand { program: "osascript".into(), args: vec!["-l".into(), "JavaScript".into(), "-e".into(), script.into()], env, stdin: None, timeout: Some(Duration::from_secs(15)) };
    let failed = |message: String| json!({ "error": message.split('\n').next().unwrap_or(""), "hint": HINT });
    match local(ctx).run(cmd).await {
        Err(code) => failed(format!("spawnSync osascript {code}")),
        Ok(out) if out.code.is_none() => failed("spawnSync osascript ETIMEDOUT".into()),
        Ok(out) if out.code != Some(0) => failed(format!("Command failed: osascript -l JavaScript -e {script}\n{}", String::from_utf8_lossy(&out.stderr))),
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            let text = trim(&text);
            let text = if text.is_empty() { "null" } else { text };
            serde_json::from_str(text).unwrap_or_else(|e| failed(parse_error(text, &e)))
        }
    }
}

/// `list`: `{reminders}`, or the script's `{error}` as is. EventKit where the
/// Mac lets it (ms, not the JXA's ~30 s of Apple Events); the osascript path
/// answers when EventKit says `None` (access denied, no permission text, an
/// old OS) and on other platforms and stubs.
pub async fn list(ctx: &HubCtx) -> Value {
    if let Some(res) = local(ctx).reminders_list().await {
        return res.unwrap_or_else(|e| json!({ "error": e }));
    }
    let out = jxa(ctx, REMINDERS_LIST, Vec::new()).await;
    if truthy(get(Some(&out), "error")) {
        return out;
    }
    json!({ "reminders": if js::truthy(&out) { out } else { json!([]) } })
}

/// `add {name, list?, due?}` and `complete {name}`. `Err` is the TypeError
/// Node throws when the body is `null`.
pub async fn change(ctx: &HubCtx, completing: bool, b: &Value) -> Result<Value, String> {
    let mut env = Vec::new();
    for (key, name) in [("REM_NAME", "name"), ("REM_LIST", "list"), ("REM_DUE", "due")] {
        let v = field(b, name)?;
        env.push((key.to_owned(), if truthy(v) { js::string_of(v.expect("truthy")) } else { String::new() }));
    }
    if env[0].1.is_empty() {
        return Ok(json!({ "error": "name?" }));
    }
    let ek = if completing {
        local(ctx).reminders_complete(env[0].1.clone()).await
    } else {
        local(ctx).reminders_add(env[0].1.clone(), env[1].1.clone(), env[2].1.clone()).await
    };
    if let Some(res) = ek {
        return Ok(res.unwrap_or_else(|e| json!({ "error": e })));
    }
    let script = if completing { REMINDERS_COMPLETE } else { REMINDERS_ADD };
    Ok(jxa(ctx, script, env).await)
}
