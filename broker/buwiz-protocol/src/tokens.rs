//! The composer's grammars for v1 plugins (spec 3) and what a send does
//! with them (9.2, 9.3).
//!
//! - Typed tokens `@short:scope:value` as the composer parses them, and
//!   canonical tokens `@short:scope:tkt_…`, the only form Rox stores and
//!   sends to the model.
//! - Slash commands `/[short:]name args` and their namespacing (3.1):
//!   built-ins win, a name one enabled plugin declares is reachable
//!   unqualified, anything else only as `/short:name`.
//! - The send rewrite (Q5): each typed token is resolved by its plugin
//!   (`kind: "scope"`, `complete: false`) and replaced by its canonical
//!   token, so the stored message and the model's context carry the
//!   ticket, never the typed identifier. A token that does not resolve
//!   stays as typed, with a warning.
//!
//! Everything here is pure: the resolve is a closure, so the tests run it
//! against a fake plugin.

use serde_json::Value;

use crate::ids as perms;

/// A typed token in a text: its byte range and parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Typed<'a> {
    pub start: usize,
    pub end: usize,
    pub short: &'a str,
    pub scope: &'a str,
    pub value: &'a str,
}

/// `\w` of an ECMAScript regex without the `u` flag.
fn word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The run of `s` from its start that `[a-z][<rest>]{0,max-1}` takes, when
/// a `:` follows it: its length.
fn part_then_colon(s: &str, min: usize, max: usize, rest: fn(u8) -> bool) -> Option<usize> {
    let b = s.as_bytes();
    if !b.first().is_some_and(u8::is_ascii_lowercase) {
        return None;
    }
    let n = 1 + b[1..].iter().take_while(|c| rest(**c)).count();
    (n >= min && n <= max && b.get(n) == Some(&b':')).then_some(n)
}

/// The typed tokens of `text` (spec 3: `(?<![\w@.])@(short):(scope):(\S*)`),
/// in order.
pub fn typed_tokens(text: &str) -> Vec<Typed<'_>> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find('@').map(|i| from + i) {
        from = at + 1;
        if text[..at].chars().next_back().is_some_and(|c| word_char(c) || c == '@' || c == '.') {
            continue;
        }
        let rest = &text[at + 1..];
        let Some(s) = part_then_colon(rest, 2, 16, |c| c.is_ascii_lowercase() || c.is_ascii_digit()) else { continue };
        let after_short = &rest[s + 1..];
        let Some(c) = part_then_colon(after_short, 1, 32, |c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        else {
            continue;
        };
        let value_at = at + 1 + s + 1 + c + 1;
        let value_len = text[value_at..].find(char::is_whitespace).unwrap_or(text.len() - value_at);
        let end = value_at + value_len;
        out.push(Typed {
            start: at,
            end,
            short: &rest[..s],
            scope: &after_short[..c],
            value: &text[value_at..end],
        });
        from = end.max(at + 1);
    }
    out
}

/// A canonical token's parts (`^@short:scope:tkt_…$`): `(short, scope, ticket)`.
pub fn canonical(token: &str) -> Option<(&str, &str, &str)> {
    let t = typed_tokens(token);
    match t.as_slice() {
        [one] if one.start == 0 && one.end == token.len() && perms::ticket_ok(one.value) => {
            Some((one.short, one.scope, one.value))
        }
        _ => None,
    }
}

/// A slash command (spec 3): the optional short qualifier, the command
/// and the argument string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slash<'a> {
    pub short: Option<&'a str>,
    pub name: &'a str,
    pub args: &'a str,
}

/// `^/(?:(short):)?(name)(?:\s+(.*))?$` over the whole text.
pub fn slash(text: &str) -> Option<Slash<'_>> {
    let rest = text.strip_prefix('/')?;
    let short = part_then_colon(rest, 2, 16, |c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let (short, rest) = match short {
        Some(n) => (Some(&rest[..n]), &rest[n + 1..]),
        None => (None, rest),
    };
    let n = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let name = &rest[..n];
    if !perms::scope_ok(name) {
        return None;
    }
    let args = rest[n..].trim_start();
    // `(.*)` stops at a line break: a command is one line
    if args.contains(['\n', '\r']) {
        return None;
    }
    Some(Slash { short, name, args })
}

/// A v1 plugin's commands, as the composer sees them.
#[derive(Debug, Clone, PartialEq)]
pub struct Commands {
    pub plugin: String,
    pub short: String,
    pub title: String,
    pub enabled: bool,
    pub commands: Vec<String>,
}

/// Where a slash command goes (3.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// A built-in (`/status`): not a plugin's.
    Builtin,
    /// The plugin `plugin` (short `short`) resolves it.
    Plugin { plugin: String, short: String },
    /// More than one enabled plugin declares it: only `/short:name`
    /// reaches it; these are the shorts.
    Ambiguous(Vec<String>),
    /// No enabled v1 plugin declares it (a v0 server's, or nothing).
    Unknown,
}

/// The built-in commands (3.1 rule 1).
pub const BUILTINS: &[&str] = &["status"];

/// Where `/[short:]name` goes among `plugins` (3.1).
pub fn route(cmd: &Slash, plugins: &[Commands]) -> Route {
    let declares = |p: &&Commands| p.enabled && p.commands.iter().any(|c| c == cmd.name);
    if let Some(short) = cmd.short {
        return match plugins.iter().filter(declares).find(|p| p.short == short) {
            Some(p) => Route::Plugin { plugin: p.plugin.clone(), short: p.short.clone() },
            None => Route::Unknown,
        };
    }
    if BUILTINS.contains(&cmd.name) {
        return Route::Builtin;
    }
    let found: Vec<&Commands> = plugins.iter().filter(declares).collect();
    match found.as_slice() {
        [] => Route::Unknown,
        [p] => Route::Plugin { plugin: p.plugin.clone(), short: p.short.clone() },
        many => Route::Ambiguous(many.iter().map(|p| p.short.clone()).collect()),
    }
}

/// The composer's rows for `/<q>`: every candidate, each in the form that
/// reaches it (`/name` when unique, else `/short:name`), with its plugin.
pub fn command_rows(q: &str, plugins: &[Commands]) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for p in plugins.iter().filter(|p| p.enabled) {
        for c in p.commands.iter().filter(|c| c.starts_with(q) || format!("{}:{c}", p.short).starts_with(q)) {
            let unique = route(&Slash { short: None, name: c, args: "" }, plugins) == Route::Plugin {
                plugin: p.plugin.clone(),
                short: p.short.clone(),
            };
            let label = if unique { format!("/{c}") } else { format!("/{}:{c}", p.short) };
            out.push((label, p.plugin.clone(), c.clone()));
        }
    }
    out
}

/// One choice of a resolve (9.2), as the popup shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub label: String,
    pub detail: String,
    pub value: String,
}

fn cap(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// A resolve's `choices`: at most 20, labels at most 80 characters,
/// details at most 160 (INV-04), whatever the plugin sent.
pub fn choices(result: &Value) -> Vec<Choice> {
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
    result
        .get("choices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|c| c.get("value").and_then(Value::as_str).is_some_and(|v| !v.is_empty()))
        .take(20)
        .map(|c| Choice { label: cap(&text(c, "label"), 80), detail: cap(&text(c, "detail"), 160), value: text(c, "value") })
        .collect()
}

/// What the send rewrite did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rewrite {
    /// The message as stored and sent: typed tokens that resolved replaced
    /// by their canonical tokens.
    pub text: String,
    /// The canonical tokens it carries, with their titles.
    pub tokens: Vec<(String, String)>,
    /// Typed tokens of v1 plugins that did not resolve (left as typed),
    /// each with why.
    pub unresolved: Vec<(String, String)>,
}

/// Rewrites `text` for sending (9.3, Q5). `plugin_of(short)` names the
/// enabled v1 plugin of a short (other shorts are a v0 server's and left
/// alone); `resolve(plugin, scope, value)` asks it for a ticket and
/// title. A token whose value is a ticket already is kept as it is.
pub fn rewrite(
    text: &str,
    plugin_of: &dyn Fn(&str) -> Option<String>,
    resolve: &mut dyn FnMut(&str, &str, &str) -> Result<(String, String), String>,
) -> Rewrite {
    let mut out = Rewrite::default();
    let mut at = 0;
    for t in typed_tokens(text) {
        let Some(plugin) = plugin_of(t.short) else { continue };
        let typed = &text[t.start..t.end];
        out.text.push_str(&text[at..t.start]);
        at = t.end;
        if perms::ticket_ok(t.value) {
            out.text.push_str(typed);
            out.tokens.push((typed.to_owned(), String::new()));
            continue;
        }
        match resolve(&plugin, t.scope, t.value) {
            Ok((ticket, title)) if perms::ticket_ok(&ticket) => {
                let token = format!("@{}:{}:{ticket}", t.short, t.scope);
                out.text.push_str(&token);
                out.tokens.push((token, title));
            }
            Ok(_) => {
                out.text.push_str(typed);
                out.unresolved.push((typed.to_owned(), "the plugin sent no ticket".to_owned()));
            }
            Err(why) => {
                out.text.push_str(typed);
                out.unresolved.push((typed.to_owned(), why));
            }
        }
    }
    out.text.push_str(&text[at..]);
    out
}

/// The longest text a resolve's `prompt` answer may put in the composer
/// (9.2, protocol 1.1), in characters.
pub const PROMPT_MAX: usize = 4000;

/// What a resolve answered (9.2), for the composer.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    Choices(Vec<Choice>),
    /// Open `app` of the plugin with `props` and `ticket`, titled `title`;
    /// `placement` as the plugin asked (else the app's declared one).
    Open { app: String, props: serde_json::Map<String, Value>, ticket: Option<String>, title: String, placement: Option<String> },
    Token { ticket: String, title: String },
    /// A starter flow (protocol 1.1): text for the composer, which the
    /// person edits and sends; Rox never sends it. A command's only.
    Prompt(String),
    Error(String),
}

/// A resolve's answer, read strictly: a result outside 9.2 is an error.
pub fn answer(v: &Value) -> Answer {
    let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
    match v.get("result").and_then(Value::as_str) {
        Some("choices") => Answer::Choices(choices(v)),
        Some("open") if perms::app_id_ok(&text("app")) => Answer::Open {
            app: text("app"),
            props: v.get("props").and_then(Value::as_object).cloned().unwrap_or_default(),
            ticket: Some(text("ticket")).filter(|t| perms::ticket_ok(t)),
            title: cap(&text("title"), 60),
            placement: Some(text("placement")).filter(|p| p == "full" || p == "inline"),
        },
        Some("token") if perms::ticket_ok(&text("ticket")) => {
            Answer::Token { ticket: text("ticket"), title: cap(&text("title"), 60) }
        }
        Some("prompt") if prompt_ok(&text("text")) => Answer::Prompt(text("text")),
        Some("error") => Answer::Error(cap(&text("message"), 200)),
        _ => Answer::Error("The plugin's answer could not be read.".to_owned()),
    }
}

/// A `prompt`'s text as the broker passes it: something other than spaces,
/// tabs and line breaks, at most [`PROMPT_MAX`] characters.
fn prompt_ok(t: &str) -> bool {
    t.chars().any(|c| !matches!(c, ' ' | '\t' | '\r' | '\n')) && t.chars().count() <= PROMPT_MAX
}

/// The draft after a starter flow's `prompt` answered: its text, or, when
/// the person typed something meanwhile, their draft with the text after
/// a blank line. Nothing they typed is lost, and nothing is sent.
pub fn prompt_draft(draft: &str, text: &str) -> String {
    let kept = draft.trim_end();
    if kept.trim().is_empty() { text.to_owned() } else { format!("{kept}\n\n{text}") }
}

/// Whether `text` is only tokens and/or a single slash command (9.3): such
/// a message is not sent to the model; each part is resolved instead.
pub fn only_tokens_or_command(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    if slash(text).is_some() {
        return true;
    }
    let mut rest = String::new();
    let mut at = 0;
    let toks = typed_tokens(text);
    if toks.is_empty() {
        return false;
    }
    for t in toks {
        rest.push_str(&text[at..t.start]);
        at = t.end;
    }
    rest.push_str(&text[at..]);
    rest.trim().is_empty()
}

/// The canonical tokens in `text` (stored messages), each with its range.
pub fn canonical_tokens(text: &str) -> Vec<(usize, usize, String, String, String)> {
    typed_tokens(text)
        .into_iter()
        .filter(|t| perms::ticket_ok(t.value))
        .map(|t| (t.start, t.end, t.short.to_owned(), t.scope.to_owned(), t.value.to_owned()))
        .collect()
}

/// `text` for a title (a chat's name): each canonical token as
/// `title_of(token)`, else `@short:scope`; never its ticket, not even a
/// cut one (a title the server shortened ends `…tkt_BK…`: `title_of` is
/// asked with the cut token then, to match by prefix). The full tokens
/// whose titles are not known yet come back too.
pub fn plain(text: &str, title_of: &dyn Fn(&str) -> Option<String>) -> (String, Vec<String>) {
    let mut out = String::with_capacity(text.len());
    let mut missing = Vec::new();
    let mut at = 0;
    for t in typed_tokens(text) {
        let full = perms::ticket_ok(t.value);
        if !full && !t.value.starts_with("tkt_") {
            continue;
        }
        out.push_str(&text[at..t.start]);
        at = t.end;
        let token = text[t.start..t.end].trim_end_matches(['…', '.']);
        match title_of(token).filter(|t| !t.is_empty()) {
            Some(title) => out.push_str(&title),
            None => {
                if full {
                    missing.push(token.to_owned());
                }
                out.push_str(&format!("@{}:{}", t.short, t.scope));
            }
        }
    }
    out.push_str(&text[at..]);
    (out, missing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const T1: &str = "tkt_0123456789ABCDEFGHJKMNPQRS";
    const T2: &str = "tkt_ZZZZZZZZZZZZZZZZZZZZZZZZZZ";

    #[test]
    fn inv_01_typed_tokens_parse_with_the_specs_grammar() {
        let t = typed_tokens("see @bir:taxprofile:123456789 and @bir:branchcode: now");
        assert_eq!(t.len(), 2);
        assert_eq!((t[0].short, t[0].scope, t[0].value), ("bir", "taxprofile", "123456789"));
        assert_eq!((t[1].short, t[1].scope, t[1].value), ("bir", "branchcode", ""));
        assert_eq!(&"see @bir:taxprofile:123456789 and"[t[0].start..t[0].end], "@bir:taxprofile:123456789");
        // the lookbehind: not after a word character, `@` or `.`
        for text in ["a@bir:x:1", "@@bir:x:1", ".@bir:x:1", "mail_@bir:x:1"] {
            assert_eq!(typed_tokens(text).iter().filter(|t| t.start > 0).count(), 0, "{text}");
        }
        assert_eq!(typed_tokens("(@bir:x:1)").len(), 1);
        // shorts are 2 to 16 lower-case letters or digits; scopes allow `-`
        for bad in ["@b:x:1", "@Bir:x:1", "@bir-x:y:1", "@abcdefghijklmnopq:x:1", "@bir:X:1", "@bir:1x:1", "@bir:x"] {
            assert!(typed_tokens(bad).is_empty(), "{bad}");
        }
        assert_eq!(typed_tokens("@bir:tax-form:1601C:2026")[0].value, "1601C:2026");
    }

    #[test]
    fn inv_01_canonical_tokens_carry_a_ticket() {
        assert_eq!(canonical(&format!("@bir:taxprofile:{T1}")), Some(("bir", "taxprofile", T1)));
        assert_eq!(canonical("@bir:taxprofile:123456789"), None);
        assert_eq!(canonical(&format!("x @bir:taxprofile:{T1}")), None);
        assert_eq!(canonical(&format!("@bir:taxprofile:{T1}x")), None);
        assert_eq!(canonical_tokens(&format!("a @bir:a:{T1} b @bir:b:123")).len(), 1);
    }

    #[test]
    fn slash_commands_parse_with_their_qualifier() {
        assert_eq!(slash("/taxform 123"), Some(Slash { short: None, name: "taxform", args: "123" }));
        assert_eq!(slash("/bir:taxform  123 x"), Some(Slash { short: Some("bir"), name: "taxform", args: "123 x" }));
        assert_eq!(slash("/status"), Some(Slash { short: None, name: "status", args: "" }));
        for bad in ["taxform", "/", "/Tax", "/bir:", "/a-b:x", "/x\ny\nz"] {
            assert_eq!(slash(bad), None, "{bad:?}");
        }
    }

    fn cmds(plugin: &str, short: &str, enabled: bool, names: &[&str]) -> Commands {
        Commands {
            plugin: plugin.into(),
            short: short.into(),
            title: short.to_uppercase(),
            enabled,
            commands: names.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn inv_03_a_unique_name_is_unqualified_and_an_ambiguous_one_needs_its_short() {
        let ps = vec![
            cmds("app.buwiz.ebirforms", "bir", true, &["taxform", "taxprofile", "report"]),
            cmds("app.buwiz.acc", "acc", true, &["report", "ledger"]),
            cmds("app.buwiz.off", "off", false, &["ledger", "status"]),
        ];
        let go = |t: &str| route(&slash(t).unwrap(), &ps);
        let bir = Route::Plugin { plugin: "app.buwiz.ebirforms".into(), short: "bir".into() };
        let acc = Route::Plugin { plugin: "app.buwiz.acc".into(), short: "acc".into() };
        assert_eq!(go("/taxform 123"), bir);
        // a disabled plugin's command is no candidate
        assert_eq!(go("/ledger"), acc);
        assert_eq!(go("/report"), Route::Ambiguous(vec!["bir".into(), "acc".into()]));
        assert_eq!(go("/bir:report"), bir);
        assert_eq!(go("/acc:report"), acc);
        // built-ins win the unqualified name
        assert_eq!(go("/status"), Route::Builtin);
        assert_eq!(go("/off:ledger"), Route::Unknown);
        assert_eq!(go("/nope"), Route::Unknown);
        // the composer lists every candidate in the form that reaches it
        let rows: Vec<String> = command_rows("re", &ps).into_iter().map(|r| r.0).collect();
        assert_eq!(rows, ["/bir:report", "/acc:report"]);
        let rows: Vec<String> = command_rows("tax", &ps).into_iter().map(|r| r.0).collect();
        assert_eq!(rows, ["/taxform", "/taxprofile"]);
    }

    #[test]
    fn inv_04_choices_are_capped() {
        let many: Vec<Value> = (0..30)
            .map(|i| json!({"label": "L".repeat(100), "detail": "D".repeat(200), "value": format!("v{i}")}))
            .collect();
        let c = choices(&json!({"result": "choices", "choices": many}));
        assert_eq!(c.len(), 20);
        assert!(c.iter().all(|c| c.label.chars().count() == 80 && c.detail.chars().count() == 160));
        assert!(choices(&json!({"choices": [{"label": "no value"}]})).is_empty());
    }

    #[test]
    fn inv_02_typed_tins_resolve_to_tickets_and_the_message_keeps_only_tickets() {
        let plugin_of = |s: &str| (s == "bir").then(|| "app.buwiz.ebirforms".to_owned());
        let mut asked = Vec::new();
        let mut resolve = |p: &str, scope: &str, value: &str| {
            asked.push((p.to_owned(), scope.to_owned(), value.to_owned()));
            match value {
                "987654321" => Ok((T1.to_owned(), "Dela Cruz Jane".to_owned())),
                "000" => Err("No taxpayer 000".to_owned()),
                _ => Ok((T2.to_owned(), "Branch 1".to_owned())),
            }
        };
        let text = format!("File for @bir:taxprofile:987654321 at @bir:branchcode:00001 also @bir:taxprofile:000 and @gws:mail:x @bir:taxprofile:{T1}");
        let r = rewrite(&text, &plugin_of, &mut resolve);
        assert_eq!(
            r.text,
            format!("File for @bir:taxprofile:{T1} at @bir:branchcode:{T2} also @bir:taxprofile:000 and @gws:mail:x @bir:taxprofile:{T1}")
        );
        assert!(!r.text.contains("987654321") && !r.text.contains("00001"), "no TIN is stored");
        assert_eq!(r.tokens[0], (format!("@bir:taxprofile:{T1}"), "Dela Cruz Jane".to_owned()));
        assert_eq!(r.unresolved, [("@bir:taxprofile:000".to_owned(), "No taxpayer 000".to_owned())]);
        // a ticket already canonical, and another server's token, are not asked about
        assert_eq!(asked.len(), 3);
        assert!(asked.iter().all(|a| a.0 == "app.buwiz.ebirforms"));
    }

    #[test]
    fn chat_titles_show_ticket_titles_and_never_the_ticket() {
        let name = format!("Check deadlines for @bir:taxprofile:{T1} and @bir:branchcode:{T2}");
        let known = |t: &str| t.ends_with(T1).then(|| "Dela Cruz Jane".to_owned());
        let (title, missing) = plain(&name, &known);
        assert_eq!(title, "Check deadlines for Dela Cruz Jane and @bir:branchcode");
        assert_eq!(missing, [format!("@bir:branchcode:{T2}")]);
        assert!(!title.contains("tkt_"));
        assert_eq!(plain("no tokens @bir:taxprofile:123", &known).0, "no tokens @bir:taxprofile:123");
        // a title the server cut inside the ticket: matched by its prefix,
        // else the scope, never the cut ticket
        let by_prefix = |t: &str| format!("@bir:taxprofile:{T1}").starts_with(t).then(|| "Dela Cruz Jane".to_owned());
        assert_eq!(plain("Check deadlines for @bir:taxprofile:tkt_0123…", &by_prefix).0, "Check deadlines for Dela Cruz Jane");
        let (cut, missing) = plain("Check @bir:taxprofile:tkt_99", &known);
        assert_eq!((cut.as_str(), missing.len()), ("Check @bir:taxprofile", 0));
    }

    #[test]
    fn a_message_of_tokens_or_one_command_is_not_for_the_model() {
        assert!(only_tokens_or_command("@bir:taxprofile:123"));
        assert!(only_tokens_or_command(" @bir:taxprofile:123  @bir:branchcode:1 "));
        assert!(only_tokens_or_command("/taxform 123 2026"));
        assert!(!only_tokens_or_command("file @bir:taxprofile:123"));
        assert!(!only_tokens_or_command("hello"));
        assert!(!only_tokens_or_command(""));
    }

    #[test]
    fn resolve_answers_are_read_strictly() {
        assert_eq!(answer(&json!({"result": "token", "ticket": T1, "title": "X"})), Answer::Token { ticket: T1.into(), title: "X".into() });
        assert!(matches!(answer(&json!({"result": "token", "ticket": "123"})), Answer::Error(_)));
        assert!(matches!(answer(&json!({"ok": true, "open": "buwiz://app/bir/forms"})), Answer::Error(_)), "v0's shape");
        match answer(&json!({"result": "open", "app": "forms", "title": "T".repeat(90), "placement": "inline"})) {
            Answer::Open { app, title, placement, .. } => {
                assert_eq!((app.as_str(), title.len(), placement.as_deref()), ("forms", 60, Some("inline")))
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn inv_10_a_prompt_is_text_for_the_composer_held_to_the_brokers_rules() {
        let text = "Draft the 1601C for September:\n- payee: ";
        assert_eq!(answer(&json!({"result": "prompt", "text": text})), Answer::Prompt(text.into()));
        assert_eq!(answer(&json!({"result": "prompt", "text": "é".repeat(PROMPT_MAX)})), Answer::Prompt("é".repeat(PROMPT_MAX)));
        for bad in [json!({"result": "prompt"}), json!({"result": "prompt", "text": " \n\t"}), json!({"result": "prompt", "text": "x".repeat(PROMPT_MAX + 1)})] {
            assert!(matches!(answer(&bad), Answer::Error(_)), "{bad}");
        }
        // an empty draft takes the text; one the person typed keeps theirs first
        assert_eq!(prompt_draft("", text), text);
        assert_eq!(prompt_draft("  \n", text), text);
        assert_eq!(prompt_draft("note to self  \n", "Draft it"), "note to self\n\nDraft it");
    }
}
