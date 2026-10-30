//! A thread's model choice and where its calls go (model.mjs `choiceFor`,
//! `labelOf`, `routed`, `withEffort`).

use serde_json::Value;

use super::anthropic::Endpoint;
use super::Settings;
use crate::hub::ctx::Db;
use crate::hub::types::ModelChoice;

/// What `choiceFor` reads of the bot whose thread it is (Node's
/// `threadBot`: the acting bot, the thread's own, or the one a routine
/// thread reports to; the store finds it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BotModel {
    pub name: String,
    pub model: String,
    pub effort: String,
    pub fast: bool,
}

/// `metaGet(k)` on app.db (the `/prefs` rows); `None` when unset.
pub fn meta_get(db: &Db, key: &str) -> Option<String> {
    db.with(|c| {
        use rusqlite::OptionalExtension;
        c.query_row("SELECT v FROM meta WHERE k=?", [key], |r| r.get::<_, Option<String>>(0)).optional().map(Option::flatten)
    })
    .ok()
    .flatten()
}

/// `choiceFor(threadId)`: the bot's own model, else the user's (with the
/// bot's effort and fast), else only the bot's effort and fast; a bot's
/// name rides along for the label. `meta` is `metaGet`.
pub fn choice_for(bot: Option<&BotModel>, meta: impl Fn(&str) -> Option<String>) -> Option<ModelChoice> {
    let m = meta("model").filter(|m| !m.is_empty());
    let base = match (bot, m) {
        (Some(b), _) if !b.model.is_empty() => Some(ModelChoice { model: b.model.clone(), effort: b.effort.clone(), fast: b.fast, name: None }),
        (b, Some(m)) => Some(ModelChoice {
            model: m,
            effort: b.map(|b| b.effort.clone()).filter(|e| !e.is_empty()).or_else(|| meta("effort")).unwrap_or_default(),
            fast: match b {
                Some(b) => b.fast,
                None => meta("fast").as_deref() == Some("1"),
            },
            name: None,
        }),
        (Some(b), None) if !b.effort.is_empty() || b.fast => Some(ModelChoice { model: String::new(), effort: b.effort.clone(), fast: b.fast, name: None }),
        _ => None,
    };
    match bot.filter(|b| !b.name.is_empty()) {
        Some(b) => Some(ModelChoice { name: Some(b.name.clone()), ..base.unwrap_or_default() }),
        None => base,
    }
}

/// `choiceFor` on app.db's meta rows.
pub fn choice_for_db(db: &Db, bot: Option<&BotModel>) -> Option<ModelChoice> {
    choice_for(bot, |k| meta_get(db, k))
}

// BUG: /retry runs outside the thread's model choice (no modelCtx.run in
// routes/chat.mjs), so a bot's retry goes to the default model and route
// (MODEL-01). The route's turn takes this choice, not `choice_for`'s.
pub fn retry_choice() -> Option<ModelChoice> {
    None
}

/// `labelOf(c)`: the bot's name, else the model (with `--fast`), else the
/// default model.
pub fn label_of(c: Option<&ModelChoice>, settings: &Settings) -> String {
    match c {
        Some(ModelChoice { name: Some(n), .. }) if !n.is_empty() => n.clone(),
        Some(c) if !c.model.is_empty() => format!("{}{}", c.model, if c.fast { "--fast" } else { "" }),
        _ => settings.model.clone(),
    }
}

/// `withEffort(p, c)`: a chosen effort as `output_config.effort` and
/// `reasoning_effort` (opencodex reads one, OpenAI-style providers the other).
pub fn with_effort(mut body: Value, c: Option<&ModelChoice>) -> Value {
    if let (Some(c), Some(b)) = (c.filter(|c| !c.effort.is_empty()), body.as_object_mut()) {
        b.insert("output_config".to_owned(), serde_json::json!({ "effort": c.effort }));
        b.insert("reasoning_effort".to_owned(), Value::String(c.effort.clone()));
    }
    body
}

/// `routed(params)`: the default goes to `ANTHROPIC_BASE_URL` (else
/// opencodex) with the bot's effort; a chosen model to opencodex, with
/// `--fast` when asked.
pub fn routed(settings: &Settings, c: Option<&ModelChoice>, mut body: Value) -> (Endpoint, Value) {
    match c.filter(|c| !c.model.is_empty()) {
        None => (settings.direct(), with_effort(body, c)),
        Some(chosen) => {
            if let Some(b) = body.as_object_mut() {
                b.insert("model".to_owned(), Value::String(format!("{}{}", chosen.model, if chosen.fast { "--fast" } else { "" })));
            }
            (settings.opencodex_endpoint(), with_effort(body, c))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    fn meta(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k| m.get(k).cloned()
    }

    fn bot(name: &str, model: &str, effort: &str, fast: bool) -> BotModel {
        BotModel { name: name.into(), model: model.into(), effort: effort.into(), fast }
    }

    fn c(model: &str, effort: &str, fast: bool, name: Option<&str>) -> Option<ModelChoice> {
        Some(ModelChoice { model: model.into(), effort: effort.into(), fast, name: name.map(Into::into) })
    }

    #[test]
    fn model_choice_for_takes_the_bot_then_the_user_then_the_bots_effort() {
        assert_eq!(choice_for(None, meta(&[])), None);
        assert_eq!(choice_for(None, meta(&[("model", "gpt-5.5"), ("effort", "low"), ("fast", "1")])), c("gpt-5.5", "low", true, None));
        assert_eq!(choice_for(None, meta(&[("model", ""), ("effort", "low")])), None);
        let ada = bot("Ada", "claude-x", "high", false);
        assert_eq!(choice_for(Some(&ada), meta(&[("model", "gpt-5.5")])), c("claude-x", "high", false, Some("Ada")));
        // a bot on the default model keeps its effort and fast with the user's model
        let bo = bot("Bo", "", "max", false);
        assert_eq!(choice_for(Some(&bo), meta(&[("model", "gpt-5.5"), ("effort", "low"), ("fast", "1")])), c("gpt-5.5", "max", false, Some("Bo")));
        assert_eq!(choice_for(Some(&bot("Bo", "", "", false)), meta(&[("model", "gpt-5.5"), ("effort", "low")])), c("gpt-5.5", "low", false, Some("Bo")));
        assert_eq!(choice_for(Some(&bo), meta(&[])), c("", "max", false, Some("Bo")));
        assert_eq!(choice_for(Some(&bot("Cy", "", "", false)), meta(&[])), c("", "", false, Some("Cy")));
        assert_eq!(choice_for(Some(&bot("", "", "", true)), meta(&[])), c("", "", true, None));
        assert_eq!(retry_choice(), None);
    }

    #[test]
    fn model_labels_credit_the_bot_then_the_model() {
        let s = Settings::from(|_| None);
        assert_eq!(label_of(None, &s), "claude-haiku-5-5");
        assert_eq!(label_of(c("gpt-5.5", "", true, None).as_ref(), &s), "gpt-5.5--fast");
        assert_eq!(label_of(c("gpt-5.5", "", false, Some("Ada")).as_ref(), &s), "Ada");
        assert_eq!(label_of(c("", "max", false, None).as_ref(), &s), "claude-haiku-5-5");
    }

    #[test]
    fn model_routes_the_default_direct_and_a_choice_to_opencodex() {
        let s = Settings::from(|k| match k {
            "OPENCODEX_URL" => Some("http://oc".into()),
            "ANTHROPIC_BASE_URL" => Some("http://direct".into()),
            "ANTHROPIC_API_KEY" => Some("sk".into()),
            _ => None,
        });
        let body = json!({"model": "claude-haiku-5-5", "max_tokens": 8, "messages": []});
        let (ep, b) = routed(&s, None, body.clone());
        assert_eq!((ep.base.as_str(), ep.key.as_str()), ("http://direct", "sk"));
        assert_eq!(crate::js::stringify(&b), r#"{"model":"claude-haiku-5-5","max_tokens":8,"messages":[]}"#);
        let (ep, b) = routed(&s, c("", "max", false, Some("Bo")).as_ref(), body.clone());
        assert_eq!(ep.base, "http://direct");
        assert_eq!(crate::js::stringify(&b), r#"{"model":"claude-haiku-5-5","max_tokens":8,"messages":[],"output_config":{"effort":"max"},"reasoning_effort":"max"}"#);
        let (ep, b) = routed(&s, c("gpt-5.5", "low", true, None).as_ref(), body);
        assert_eq!((ep.base.as_str(), ep.key.as_str()), ("http://oc", "opencodex"));
        assert_eq!(crate::js::stringify(&b), r#"{"model":"gpt-5.5--fast","max_tokens":8,"messages":[],"output_config":{"effort":"low"},"reasoning_effort":"low"}"#);
    }
}
