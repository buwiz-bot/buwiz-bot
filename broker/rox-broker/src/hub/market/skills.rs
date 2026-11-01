//! Private skills: the six tools the model manages them with (tools.mjs
//! `toolSkillAdmin`), what the agent's system prompt carries of them
//! (`privateSkillsInject`), and the writes behind `POST /skills`,
//! `/skills/state` and `/skills/delete` (routes/market.mjs).

use std::sync::Arc;

use rusqlite::OptionalExtension;
use serde_json::{json, Value};

use super::{nn, now};
use crate::hub::ctx::HubCtx;
use crate::hub::store::Ceiling;
use crate::hub::types::{ToolResult, TurnCtx};
use crate::js;

pub const TOOLS: [&str; 6] = ["skill_create", "skill_update", "skill_delete", "skill_enable", "skill_disable", "skill_list"];

/// Registers the six skill tools.
pub fn register(ctx: &mut HubCtx) {
    for name in TOOLS {
        ctx.tools.register(name, move |ctx: Arc<HubCtx>, _: TurnCtx, input: Value| async move { tool(&ctx, name, &input) });
    }
}

/// `skillSlug(s)` (and POST /skills' id).
pub fn skill_slug(s: &str) -> String {
    let slug: String = super::dashes(s, "-").trim_matches('-').chars().take(48).collect();
    if slug.is_empty() {
        "skill".into()
    } else {
        slug
    }
}

/// A skills row, as the tools read it.
#[derive(Clone, Debug)]
struct Row {
    id: String,
    name: Value,
    description: Value,
    body: Value,
    enabled: Value,
}

fn cell(v: rusqlite::types::ValueRef<'_>) -> Value {
    use rusqlite::types::ValueRef::*;
    match v {
        Integer(i) => json!(i),
        Real(f) => json!(f),
        Text(t) => json!(String::from_utf8_lossy(t)),
        Null | Blob(_) => Value::Null,
    }
}

fn row_of(r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    Ok(Row {
        id: r.get::<_, Option<String>>("id")?.unwrap_or_default(),
        name: cell(r.get_ref("name")?),
        description: cell(r.get_ref("description")?),
        body: cell(r.get_ref("body")?),
        enabled: cell(r.get_ref("enabled")?),
    })
}

fn all(ctx: &HubCtx) -> Result<Vec<Row>, String> {
    ctx.db.with(|db| {
        let mut st = db.prepare("SELECT * FROM skills ORDER BY created_at")?;
        let r = st.query_map([], row_of)?;
        r.collect()
    })
}

fn by_id(ctx: &HubCtx, id: &str) -> Result<Option<Row>, String> {
    ctx.db.with(|db| db.query_row("SELECT * FROM skills WHERE id=?", [id], row_of).optional())
}

/// `${v}` in a template literal.
fn tpl(v: &Value) -> String {
    js::string_of(v)
}

/// `skillRow(input)`: by id, else by name or its slug.
fn skill_row(ctx: &HubCtx, input: &Value) -> Result<Option<Row>, String> {
    if let Some(id) = input.get("id").filter(|v| js::truthy(v)) {
        if let Some(r) = by_id(ctx, &js::string_of(id))? {
            return Ok(Some(r));
        }
    }
    if let Some(name) = input.get("name").filter(|v| js::truthy(v)) {
        let name = js::string_of(name);
        return ctx.db.with(|db| db.query_row("SELECT * FROM skills WHERE name=? OR id=?", [&name, &skill_slug(&name)], row_of).optional());
    }
    Ok(None)
}

fn text(content: String) -> ToolResult {
    ToolResult { content: Value::String(content), is_error: false }
}

/// `toolSkillAdmin(tool, input)`.
pub fn tool(ctx: &HubCtx, name: &str, input: &Value) -> ToolResult {
    match admin(ctx, name, input) {
        Ok(r) => r,
        // Node's execTool turns a throw into the tool's error text
        Err(e) => ToolResult { content: Value::String(e), is_error: true },
    }
}

fn admin(ctx: &HubCtx, tool: &str, input: &Value) -> Result<ToolResult, String> {
    let s = |k: &str| input.get(k).filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
    if tool == "skill_list" {
        let rows = all(ctx)?;
        if rows.is_empty() {
            return Ok(text("no private skills yet".into()));
        }
        let lines: Vec<String> = rows.iter().map(|r| format!("{} — {}{} · {}", r.id, tpl(&r.name), if js::truthy(&r.enabled) { "" } else { " (disabled)" }, tpl(&r.description))).collect();
        return Ok(text(lines.join("\n")));
    }
    if tool == "skill_create" {
        let name = skill_slug(&s("name"));
        let cur = by_id(ctx, &name)?;
        let shown = if s("name").is_empty() { name.clone() } else { s("name") };
        let body = if !s("instructions").is_empty() { s("instructions") } else { s("body") };
        let at = now(ctx);
        ctx.db.with(|db| {
            db.execute(
                "INSERT OR REPLACE INTO skills(id,name,description,body,enabled,created_at) VALUES(?,?,?,?,COALESCE((SELECT enabled FROM skills WHERE id=?),1),COALESCE((SELECT created_at FROM skills WHERE id=?),?))",
                rusqlite::params![name, js::slice_utf16(&shown, 80), js::slice_utf16(&s("description"), 400), body, name, name, at],
            )
        })?;
        return Ok(text(format!("{} private skill \"{name}\" — its instructions apply to every bot's next step", if cur.is_some() { "updated" } else { "created" })));
    }
    let Some(row) = skill_row(ctx, input)? else {
        let what = [s("id"), s("name")].into_iter().find(|x| !x.is_empty()).unwrap_or_else(|| "?".into());
        return Ok(ToolResult { content: json!(format!("no private skill matches {what} — skill_list shows the ids")), is_error: true });
    };
    if tool == "skill_update" {
        let given = |k: &str| input.get(k).map(js::string_of);
        let name = given("name").map_or(row.name.clone(), |n| json!(js::slice_utf16(&n, 80)));
        let description = given("description").map_or(row.description.clone(), |d| json!(js::slice_utf16(&d, 400)));
        let body = match (input.get("instructions"), input.get("body")) {
            (None, None) => row.body.clone(),
            (i, b) => json!(match nn(i) {
                Some(v) => js::string_of(v),
                None => b.map_or_else(|| "undefined".to_owned(), js::string_of),
            }),
        };
        let sql = |v: &Value| match v {
            Value::Null => rusqlite::types::Value::Null,
            Value::Number(n) => n.as_i64().map_or(rusqlite::types::Value::Real(n.as_f64().unwrap_or(0.0)), rusqlite::types::Value::Integer),
            other => rusqlite::types::Value::Text(js::string_of(other)),
        };
        ctx.db.with(|db| db.execute("UPDATE skills SET name=?, description=?, body=? WHERE id=?", rusqlite::params![sql(&name), sql(&description), sql(&body), row.id]))?;
        return Ok(text(format!("skill \"{}\" updated — applies from the next step", row.id)));
    }
    if tool == "skill_delete" {
        ctx.db.with(|db| db.execute("DELETE FROM skills WHERE id=?", [&row.id]))?;
        return Ok(text(format!("deleted private skill \"{}\"", row.id)));
    }
    let enabled = tool == "skill_enable";
    ctx.db.with(|db| db.execute("UPDATE skills SET enabled=? WHERE id=?", rusqlite::params![enabled as i64, row.id]))?;
    Ok(text(format!("skill \"{}\" {}", row.id, if enabled { "enabled — instructions apply" } else { "disabled — instructions withheld" })))
}

/// `privateSkillsInject()`: the enabled skills' bodies for the agent's
/// system prompt.
pub fn private_skills_inject(ctx: &HubCtx) -> String {
    private_skills_inject_under(ctx, None)
}

/// The same under a bot's tool ceiling: a skill switched off there is
/// left out of the prompt as a globally disabled one is (ceiling ∩
/// global, opengrok-policy's rule — the ceiling only narrows).
pub fn private_skills_inject_under(ctx: &HubCtx, ceiling: Option<&Ceiling>) -> String {
    let rows = ctx.db.with(|db| {
        let mut st = db.prepare("SELECT name, description, body, id FROM skills WHERE enabled=1 ORDER BY created_at")?;
        let r = st.query_map([], |r| Ok((cell(r.get_ref(0)?), cell(r.get_ref(1)?), cell(r.get_ref(2)?), r.get::<_, Option<String>>(3)?.unwrap_or_default())))?;
        r.collect::<rusqlite::Result<Vec<_>>>()
    });
    let Ok(rows) = rows else { return String::new() };
    let rows: Vec<(Value, Value, Value)> = rows.into_iter().filter(|(_, _, _, id)| ceiling.is_none_or(|c| c.allows(id))).map(|(n, d, b, _)| (n, d, b)).collect();
    if rows.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = rows.iter().map(|(n, d, b)| format!("### {}\n{}\n{}", tpl(n), tpl(d), js::slice_utf16(&if js::truthy(b) { js::string_of(b) } else { String::new() }, 8000))).collect();
    js::slice_utf16(&format!("\nPrivate skills (active — follow when applicable):\n{}", parts.join("\n\n")), 30000)
}

/// `POST /skills {id?, name, description, instructions}`: create or edit.
pub fn save(ctx: &HubCtx, b: &Value) -> Result<String, String> {
    let id = match b.get("id").filter(|v| js::truthy(v)) {
        Some(id) => js::string_of(id),
        None => skill_slug(&b.get("name").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default()),
    };
    let cur = by_id(ctx, &id)?;
    let first = |keys: &[&str]| keys.iter().find_map(|k| b.get(*k).filter(|v| js::truthy(v))).map(js::string_of).unwrap_or_default();
    if cur.is_none() && first(&["instructions", "body"]).trim_matches(js::is_space).is_empty() {
        return Err("skill needs instructions".into());
    }
    // `??`: only undefined and null fall through
    let pick = |k: &str, cur: Option<&Value>, d: &str| nn(b.get(k)).map(js::string_of).or_else(|| nn(cur).map(js::string_of)).unwrap_or_else(|| d.to_owned());
    let name = pick("name", cur.as_ref().map(|c| &c.name), &id);
    let description = pick("description", cur.as_ref().map(|c| &c.description), "");
    let body = nn(b.get("instructions")).or(nn(b.get("body"))).map(js::string_of).or_else(|| cur.as_ref().and_then(|c| nn(Some(&c.body))).map(js::string_of)).unwrap_or_default();
    let at = now(ctx);
    ctx.db.with(|db| {
        db.execute(
            "INSERT OR REPLACE INTO skills(id,name,description,body,enabled,created_at) VALUES(?,?,?,?,COALESCE((SELECT enabled FROM skills WHERE id=?),1),COALESCE((SELECT created_at FROM skills WHERE id=?),?))",
            rusqlite::params![id, js::slice_utf16(&name, 80), js::slice_utf16(&description, 400), body, id, id, at],
        )
    })?;
    Ok(id)
}

/// `POST /skills/state {id, enabled}`.
pub fn set_state(ctx: &HubCtx, b: &Value) -> Result<(), String> {
    let id = b.get("id").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
    ctx.db.with(|db| db.execute("UPDATE skills SET enabled=? WHERE id=?", rusqlite::params![b.get("enabled").is_some_and(js::truthy) as i64, id])).map(|_| ())
}

/// `POST /skills/delete {id}`.
pub fn delete(ctx: &HubCtx, b: &Value) -> Result<(), String> {
    let id = b.get("id").filter(|v| js::truthy(v)).map(js::string_of).unwrap_or_default();
    ctx.db.with(|db| db.execute("DELETE FROM skills WHERE id=?", [id])).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::traits::Clock;

    /// market.test.mjs MKT-13
    #[tokio::test]
    async fn mkt_13_the_six_skill_tools() {
        let dir = std::env::temp_dir().join(format!("rox-mkt-tools-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(dir.join("generated")).unwrap();
        let mut ctx = HubCtx::new(dir, Clock::manual());
        register(&mut ctx);
        super::super::schema(&ctx).unwrap();
        let ctx = Arc::new(ctx);
        let t = |name: &'static str, input: Value| {
            let ctx = ctx.clone();
            async move { ctx.tools.get(name).unwrap().call(ctx.clone(), TurnCtx::default(), input).await }
        };
        let ok = |s: &str| ToolResult { content: json!(s), is_error: false };
        let err = |s: &str| ToolResult { content: json!(s), is_error: true };
        assert_eq!(t("skill_list", json!({})).await, ok("no private skills yet"));
        assert_eq!(
            t("skill_create", json!({"name": "Tone Guide", "description": "How to write", "instructions": "Be brief."})).await,
            ok("created private skill \"tone-guide\" — its instructions apply to every bot's next step")
        );
        assert_eq!(t("skill_create", json!({"name": "tone guide", "instructions": "Be briefer."})).await, ok("updated private skill \"tone-guide\" — its instructions apply to every bot's next step"));
        assert_eq!(t("skill_create", json!({"body": "no name"})).await, ok("created private skill \"skill\" — its instructions apply to every bot's next step"));
        assert_eq!(t("skill_update", json!({"name": "Tone Guide", "description": "new"})).await, ok("skill \"tone-guide\" updated — applies from the next step"));
        assert_eq!(t("skill_update", json!({"id": "nope"})).await, err("no private skill matches nope — skill_list shows the ids"));
        assert_eq!(t("skill_update", json!({})).await, err("no private skill matches ? — skill_list shows the ids"));
        assert_eq!(t("skill_disable", json!({"id": "tone-guide"})).await, ok("skill \"tone-guide\" disabled — instructions withheld"));
        assert_eq!(t("skill_list", json!({})).await, ok("tone-guide — Tone Guide (disabled) · new\nskill — skill · "));
        assert_eq!(t("skill_enable", json!({"name": "tone-guide"})).await, ok("skill \"tone-guide\" enabled — instructions apply"));
        assert_eq!(private_skills_inject(&ctx), "\nPrivate skills (active — follow when applicable):\n### Tone Guide\nnew\nBe briefer.\n\n### skill\n\nno name");
        assert_eq!(t("skill_delete", json!({"name": "skill"})).await, ok("deleted private skill \"skill\""));
        assert_eq!(t("skill_delete", json!({"id": "tone-guide"})).await, ok("deleted private skill \"tone-guide\""));
        assert_eq!(private_skills_inject(&ctx), "");
        assert_eq!(all(&ctx).unwrap().len(), 0);
    }
}
