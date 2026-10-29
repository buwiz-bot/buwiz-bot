//! v1 app cards in a chat (spec 6.5, 9.6, Q9).
//!
//! A tool result's `_meta["rox/open"]` reaches Rox as a card on the reply
//! (the broker adds it to the turn's `links`, provider `rox-app`, with the
//! checked open under `card`). A full app's card has the title and
//! **Open**; an inline app is one card whose header's chevron expands it.
//! Nothing opens or mounts until it is pressed (INV-08, 9.7). An inline
//! app is sized from its manifest's `inline`, clamped to 240–960 × 96–800
//! and to the chat column, under its header with Open (full screen). Its height is the first frame's: the app's
//! `rox.fit` moves it to its content's within `inline.min_height` ..
//! `inline.max_height` ([`fit_height`]). A user's own command, token or pin mounts an
//! inline app at once (Q9); a tool result or link waits for the chevron.
//!
//! An inline app may hand Rox its card's chrome (7.12 to 7.14, protocol
//! 1.1): footer buttons Rox draws in its own style (`rox.actions`,
//! [`parse_actions`]), a short status in the header (`rox.meta`,
//! [`parse_meta`]), and the one line the card settles to (`rox.settle`,
//! [`parse_settle`]).

use serde_json::{Map, Value};

use super::{RoxError, V1Key, perms};

/// Inline width and height ranges (5.5, Appendix A).
pub const WIDTH: (f32, f32) = (240.0, 960.0);
pub const HEIGHT: (f32, f32) = (96.0, 800.0);
/// The heights `rox.fit` moves an inline card within when the manifest
/// gives no `inline.min_height` / `max_height` (6.5).
pub const FIT: (f32, f32) = (120.0, 640.0);

/// How an open is placed (6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Full,
    Inline,
}

impl Placement {
    pub fn parse(s: &str) -> Option<Placement> {
        match s {
            "full" => Some(Placement::Full),
            "inline" => Some(Placement::Inline),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Placement::Full => "full",
            Placement::Inline => "inline",
        }
    }
}

/// An answer card's size (9.8): one main card and up to two small ones
/// per answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Size {
    #[default]
    Main,
    Small,
}

/// A tool result's open, as its card shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenCard {
    pub plugin: String,
    pub app: String,
    pub title: String,
    pub placement: Placement,
    pub ticket: Option<String>,
    pub props: Map<String, Value>,
    pub size: Size,
}

impl OpenCard {
    /// The card a turn's link carries (`provider: "rox-app"`, its checked
    /// open under `card`); `None` for any other link, or a card that
    /// breaks the grammars (dropped, as the broker drops a bad one).
    pub fn from_link(link: &Value) -> Option<OpenCard> {
        if link.get("provider").and_then(Value::as_str) != Some("rox-app") {
            return None;
        }
        let c = link.get("card")?;
        let text = |k: &str| c.get(k).and_then(Value::as_str).map(str::to_owned);
        let (plugin, app) = (text("plugin")?, text("app")?);
        if !perms::plugin_id_ok(&plugin) || !perms::app_id_ok(&app) {
            return None;
        }
        let ticket = match c.get("ticket") {
            None | Some(Value::Null) => None,
            Some(Value::String(t)) if perms::ticket_ok(t) => Some(t.clone()),
            Some(_) => return None,
        };
        Some(OpenCard {
            title: text("title").filter(|t| !t.is_empty()).unwrap_or_else(|| app.clone()).chars().take(60).collect(),
            placement: text("placement").and_then(|p| Placement::parse(&p)).unwrap_or(Placement::Full),
            props: c.get("props").and_then(Value::as_object).cloned().unwrap_or_default(),
            size: if text("size").as_deref() == Some("small") { Size::Small } else { Size::Main },
            plugin,
            app,
            ticket,
        })
    }

    /// When its snapshot was taken (`props.as_of`, unix ms), if it has one
    /// (9.8).
    pub fn as_of(&self) -> Option<i64> {
        self.props.get("snapshot")?;
        self.props.get("as_of").and_then(Value::as_i64)
    }

    /// The key Refresh opens (9.8): the same open without its `snapshot`
    /// and `as_of`, so the card reads live through its own tools. History
    /// keeps the snapshot (the turn is not changed).
    pub fn live_key(&self) -> V1Key {
        let mut k = self.key();
        k.props.remove("snapshot");
        k.props.remove("as_of");
        k
    }

    /// The module key it opens.
    pub fn key(&self) -> V1Key {
        V1Key { plugin: self.plugin.clone(), app: self.app.clone(), ticket: self.ticket.clone(), props: self.props.clone() }
    }
}

/// Where an answer's card goes (9.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// The answer's one main card.
    Main,
    /// One of its two small cards.
    Small,
    /// Past those: a one-line link.
    Link,
}

/// The slots of an answer's cards, in order: the first main card, the
/// first two small ones; every other card is a link.
pub fn arrange(cards: &[OpenCard]) -> Vec<Slot> {
    let (mut main, mut small) = (false, 0);
    cards
        .iter()
        .map(|c| match c.size {
            Size::Main if !main => {
                main = true;
                Slot::Main
            }
            Size::Small if small < 2 => {
                small += 1;
                Slot::Small
            }
            _ => Slot::Link,
        })
        .collect()
}

/// How many of the person's messages after an answer collapse its cards
/// to a one-line chip (9.8).
pub const AGE: usize = 5;

/// Whether an answer's cards show as chips: `user_after` of the person's
/// messages came since.
pub fn aged(user_after: usize) -> bool {
    user_after >= AGE
}

/// "3:42 PM" of a local `hour` (0..24) and `minute`.
pub fn clock(hour: u32, minute: u32) -> String {
    let h12 = match hour % 12 {
        0 => 12,
        h => h,
    };
    format!("{h12}:{minute:02} {}", if hour < 12 { "AM" } else { "PM" })
}

/// The card's "as of" text for snapshot time `ms`: the local clock time,
/// with the date when it is not today.
pub fn as_of_label(ms: i64) -> String {
    let (then, today) = (local(ms / 1000), local(super::session::now_ms() / 1000));
    let time = clock(then.2, then.3);
    if (then.0, then.1) == (today.0, today.1) {
        format!("as of {time}")
    } else {
        const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        format!("as of {} {}, {time}", MONTHS[then.4 as usize % 12], then.5)
    }
}

/// Local (year, day of year, hour, minute, month 0..12, day of month) of
/// unix time `secs`.
fn local(secs: i64) -> (i32, i32, u32, u32, i32, i32) {
    #[repr(C)]
    struct Tm {
        sec: i32,
        min: i32,
        hour: i32,
        mday: i32,
        mon: i32,
        year: i32,
        wday: i32,
        yday: i32,
        isdst: i32,
        gmtoff: i64,
        zone: *const i8,
    }
    unsafe extern "C" {
        fn localtime_r(time: *const i64, out: *mut Tm) -> *mut Tm;
    }
    // SAFETY: `localtime_r` writes only the struct it is given.
    let tm = unsafe {
        let mut tm: Tm = std::mem::zeroed();
        localtime_r(&secs, &mut tm);
        tm
    };
    (tm.year, tm.yday, tm.hour as u32, tm.min as u32, tm.mon, tm.mday)
}

/// Who asked for an open: what decides whether an inline app mounts
/// before a click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opener {
    /// A command or token the person sent.
    User,
    /// A pin (library or dock).
    Pin,
    /// A tool result's card.
    ToolResult,
    /// A `rox://app/` link.
    Link,
}

/// Whether an inline open from `opener` mounts without a click (6.5, Q9).
pub fn mounts_at_once(opener: Opener) -> bool {
    matches!(opener, Opener::User | Opener::Pin)
}

/// The placement an app declares first (5.5: `placement[0]`, full when
/// absent).
pub fn declared(app: &Value) -> Placement {
    app.get("placement")
        .and_then(Value::as_array)
        .and_then(|p| p.first())
        .and_then(Value::as_str)
        .and_then(Placement::parse)
        .unwrap_or(Placement::Full)
}

/// An inline app's card size: its manifest's `inline.width`/`height`
/// clamped to the spec's ranges and then to `column` (the chat column's
/// width; the column wins over the minimum).
pub fn inline_size(app: &Value, column: f32) -> (f32, f32) {
    let inline = app.get("inline");
    let dim = |k: &str, (lo, hi): (f32, f32)| {
        inline.and_then(|i| i.get(k)).and_then(Value::as_f64).map_or(lo, |v| (v as f32).clamp(lo, hi))
    };
    let (w, h) = (dim("width", WIDTH), dim("height", HEIGHT));
    (if column > 0.0 { w.min(column) } else { w }, h)
}

/// The heights an inline app's `rox.fit` may set: its manifest's
/// `inline.min_height` and `max_height` ([`FIT`] when absent), each in the
/// spec's range; a maximum under the minimum is the minimum.
pub fn fit_range(app: &Value) -> (f32, f32) {
    let inline = app.get("inline");
    let dim = |k: &str, default: f32| {
        inline.and_then(|i| i.get(k)).and_then(Value::as_f64).map_or(default, |v| v as f32).clamp(HEIGHT.0, HEIGHT.1)
    };
    let lo = dim("min_height", FIT.0);
    (lo, dim("max_height", FIT.1).max(lo))
}

/// The card height `rox.fit(height)` sets: `height` in [`fit_range`].
pub fn fit_height(app: &Value, height: f32) -> f32 {
    let (lo, hi) = fit_range(app);
    height.clamp(lo, hi)
}

/// The most footer buttons an inline card has (`rox.actions`, 7.12).
pub const MAX_ACTIONS: usize = 3;
/// The longest action id; ids are `[A-Za-z0-9_.:-]`.
pub const MAX_ACTION_ID: usize = 64;
/// The longest button label, in characters (after trimming).
pub const MAX_LABEL: usize = 24;
/// The longest status `rox.meta` shows in the header.
pub const MAX_META: usize = 32;
/// The longest line `rox.settle` leaves.
pub const MAX_SETTLE: usize = 120;

/// One footer button of an inline card (`rox.actions`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardAction {
    pub id: String,
    pub label: String,
    pub primary: bool,
    pub disabled: bool,
}

/// The status in an inline card's header (`rox.meta`): plain dim text,
/// or a pill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardMeta {
    pub text: String,
    pub pill: bool,
}

fn invalid(m: impl Into<String>) -> RoxError {
    RoxError::new("invalid_args", m)
}

/// `v` as one line of text, trimmed: 1 to `max` characters, no line
/// breaks or other control characters.
fn one_line(v: Option<&Value>, what: &str, max: usize) -> Result<String, RoxError> {
    let t = v.and_then(Value::as_str).ok_or_else(|| invalid(format!("{what} is a string")))?.trim();
    if t.is_empty() {
        return Err(invalid(format!("{what} is empty")));
    }
    if t.chars().any(char::is_control) {
        return Err(invalid(format!("{what} is one line")));
    }
    if t.chars().count() > max {
        return Err(invalid(format!("{what} is at most {max} characters")));
    }
    Ok(t.to_owned())
}

/// Field `k` of `o` as a boolean, false when absent or null.
fn flag(o: &Map<String, Value>, k: &str) -> Result<bool, RoxError> {
    match o.get(k) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(invalid(format!("{k} is a boolean"))),
    }
}

/// `rox.actions(list)` checked (7.12): a list of at most [`MAX_ACTIONS`]
/// `{id, label, primary?, disabled?}` and no other key; ids unique, of
/// `[A-Za-z0-9_.:-]`, at most [`MAX_ACTION_ID`]; labels one line, at most
/// [`MAX_LABEL`] characters; at most one primary. `[]` clears the footer.
pub fn parse_actions(v: Option<&Value>) -> Result<Vec<CardAction>, RoxError> {
    let list = v.and_then(Value::as_array).ok_or_else(|| invalid("rox.actions takes a list"))?;
    if list.len() > MAX_ACTIONS {
        return Err(invalid(format!("a card has at most {MAX_ACTIONS} actions")));
    }
    let mut out: Vec<CardAction> = Vec::new();
    for a in list {
        let o = a.as_object().ok_or_else(|| invalid("an action is {id, label, primary?, disabled?}"))?;
        if let Some(k) = o.keys().find(|k| !["id", "label", "primary", "disabled"].contains(&k.as_str())) {
            return Err(invalid(format!("an action does not take {k}")));
        }
        let id = o.get("id").and_then(Value::as_str).unwrap_or_default();
        let id_ok = |c: char| c.is_ascii_alphanumeric() || "_.:-".contains(c);
        if id.is_empty() || id.len() > MAX_ACTION_ID || !id.chars().all(id_ok) {
            return Err(invalid(format!("an action's id is 1 to {MAX_ACTION_ID} of A-Z a-z 0-9 _ . : -")));
        }
        if out.iter().any(|x| x.id == id) {
            return Err(invalid(format!("two actions are {id}")));
        }
        let action = CardAction {
            id: id.to_owned(),
            label: one_line(o.get("label"), "a label", MAX_LABEL)?,
            primary: flag(o, "primary")?,
            disabled: flag(o, "disabled")?,
        };
        if action.primary && out.iter().any(|x| x.primary) {
            return Err(invalid("a card has one primary action"));
        }
        out.push(action);
    }
    Ok(out)
}

/// The footer's order: the primary first, then the rest as the app listed
/// them (the row is right-aligned, as Rox's own cards).
pub fn footer_order(actions: &[CardAction]) -> Vec<&CardAction> {
    actions.iter().filter(|a| a.primary).chain(actions.iter().filter(|a| !a.primary)).collect()
}

/// `rox.meta({text, pill?})` checked (7.13): `text` one line of at most
/// [`MAX_META`] characters, `pill` a boolean; `null` clears it.
pub fn parse_meta(v: Option<&Value>) -> Result<Option<CardMeta>, RoxError> {
    let o = match v {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Object(o)) => o,
        Some(_) => return Err(invalid("rox.meta takes {text, pill?} or null")),
    };
    if let Some(k) = o.keys().find(|k| !["text", "pill"].contains(&k.as_str())) {
        return Err(invalid(format!("rox.meta does not take {k}")));
    }
    Ok(Some(CardMeta { text: one_line(o.get("text"), "the status", MAX_META)?, pill: flag(o, "pill")? }))
}

/// `rox.settle(text)` checked (7.14): one line of at most [`MAX_SETTLE`]
/// characters, trimmed.
pub fn parse_settle(v: Option<&Value>) -> Result<String, RoxError> {
    one_line(v, "the outcome", MAX_SETTLE)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const T: &str = "tkt_0123456789ABCDEFGHJKMNPQRS";

    fn link(card: Value) -> Value {
        json!({
            "provider": "rox-app", "id": "rox://app/dev.goldcoders.bir/forms", "name": "BIR Forms",
            "link": "rox://app/dev.goldcoders.bir/forms", "action": "open", "kind": "app", "type": "Mini app", "card": card,
        })
    }

    #[test]
    fn inv_08_a_tool_result_card_offers_open_or_expand_and_never_opens_by_itself() {
        let full = OpenCard::from_link(&link(json!({
            "plugin": "dev.goldcoders.bir", "app": "forms", "props": {"form": "1601C"}, "ticket": T, "title": "BIR Forms", "placement": "full",
        })))
        .unwrap();
        assert_eq!(full.placement, Placement::Full);
        assert_eq!(full.key().to_key(), format!("v1:dev.goldcoders.bir/forms?ticket={T}&props=%7B%22form%22%3A%221601C%22%7D"));
        let inline = OpenCard::from_link(&link(json!({"plugin": "dev.goldcoders.bir", "app": "calendar", "title": "Calendar", "placement": "inline"}))).unwrap();
        assert_eq!(inline.placement, Placement::Inline);
        // tool results and links wait for the press; the person's own
        // commands and pins do not
        assert!(!mounts_at_once(Opener::ToolResult));
        assert!(!mounts_at_once(Opener::Link));
        assert!(mounts_at_once(Opener::User));
        assert!(mounts_at_once(Opener::Pin));
        // other links are no cards; bad cards are dropped
        assert_eq!(OpenCard::from_link(&json!({"provider": "gdrive", "card": {"plugin": "dev.goldcoders.bir", "app": "forms"}})), None);
        assert_eq!(OpenCard::from_link(&link(json!({"plugin": "bir", "app": "forms"}))), None);
        assert_eq!(OpenCard::from_link(&link(json!({"plugin": "dev.goldcoders.bir", "app": "forms", "ticket": "123"}))), None);
    }

    fn card(size: Size) -> OpenCard {
        OpenCard { plugin: "dev.goldcoders.bir".into(), app: "deadlines".into(), title: "T".into(), placement: Placement::Inline, ticket: None, props: Map::new(), size }
    }

    #[test]
    fn card_02_an_answer_has_one_main_card_two_small_ones_and_links_after() {
        use Size::*;
        let cards: Vec<OpenCard> = [Small, Main, Small, Main, Small, Small].into_iter().map(card).collect();
        assert_eq!(arrange(&cards), [Slot::Small, Slot::Main, Slot::Small, Slot::Link, Slot::Link, Slot::Link]);
        assert!(arrange(&[]).is_empty());
        let size = |s: Value| OpenCard::from_link(&json!({"provider": "rox-app", "card": {"plugin": "dev.goldcoders.bir", "app": "deadlines", "size": s}})).unwrap().size;
        assert_eq!(size(json!("small")), Small);
        assert_eq!(size(json!("main")), Main);
        assert_eq!(size(Value::Null), Main);
    }

    #[test]
    fn card_03_a_snapshot_shows_as_of_and_refresh_reads_live() {
        let snap = OpenCard::from_link(&json!({"provider": "rox-app", "card": {
            "plugin": "dev.goldcoders.bir", "app": "deadlines", "ticket": T,
            "props": {"ticket": T, "snapshot": {"due": 2}, "as_of": 1_793_200_000_000_i64},
        }}))
        .unwrap();
        assert_eq!(snap.as_of(), Some(1_793_200_000_000));
        let live = snap.live_key();
        assert!(!live.props.contains_key("snapshot") && !live.props.contains_key("as_of"));
        assert_eq!(live.props["ticket"], T);
        assert_eq!(live.ticket.as_deref(), Some(T));
        assert!(snap.key().props.contains_key("snapshot"), "history keeps the snapshot");
        // no snapshot, no "as of"
        let plain = OpenCard { props: [("as_of".to_owned(), json!(5))].into_iter().collect(), ..card(Size::Main) };
        assert_eq!(plain.as_of(), None);
        assert_eq!(clock(15, 42), "3:42 PM");
        assert_eq!(clock(0, 5), "12:05 AM");
        assert_eq!(clock(12, 0), "12:00 PM");
        assert!(as_of_label(crate::mini_v1::session::now_ms()).starts_with("as of "));
        assert!(as_of_label(0).contains(','), "another day shows its date");
    }

    #[test]
    fn card_04_cards_age_into_chips_after_five_of_the_persons_messages() {
        assert!(!aged(0));
        assert!(!aged(4));
        assert!(aged(5));
        assert!(aged(9));
    }

    #[test]
    fn inline_cards_take_the_declared_size_within_the_range_and_the_column() {
        let app = |w: f64, h: f64| json!({"placement": ["inline"], "inline": {"width": w, "height": h}});
        assert_eq!(inline_size(&app(480.0, 320.0), 700.0), (480.0, 320.0));
        assert_eq!(inline_size(&app(100.0, 20.0), 700.0), (240.0, 96.0));
        assert_eq!(inline_size(&app(2000.0, 2000.0), 2000.0), (960.0, 800.0));
        assert_eq!(inline_size(&app(960.0, 300.0), 560.0), (560.0, 300.0));
        assert_eq!(inline_size(&json!({}), 0.0), (240.0, 96.0));
        assert_eq!(declared(&app(1.0, 1.0)), Placement::Inline);
        assert_eq!(declared(&json!({"placement": ["full", "inline"]})), Placement::Full);
        assert_eq!(declared(&json!({})), Placement::Full);
    }

    #[test]
    fn fit_moves_an_inline_card_within_its_declared_heights() {
        let app = |i: Value| json!({"placement": ["inline"], "inline": i});
        // defaults 120 .. 640
        let plain = app(json!({"width": 560, "height": 230}));
        assert_eq!(fit_range(&plain), (120.0, 640.0));
        assert_eq!(fit_height(&plain, 284.0), 284.0);
        assert_eq!(fit_height(&plain, 40.0), 120.0);
        assert_eq!(fit_height(&plain, 2000.0), 640.0);
        // the manifest's, within the spec's 96 .. 800
        let own = app(json!({"width": 560, "height": 230, "min_height": 150, "max_height": 400}));
        assert_eq!(fit_height(&own, 100.0), 150.0);
        assert_eq!(fit_height(&own, 500.0), 400.0);
        assert_eq!(fit_range(&app(json!({"min_height": 10, "max_height": 5000}))), (96.0, 800.0));
        // a maximum under the minimum is the minimum
        assert_eq!(fit_range(&app(json!({"min_height": 300, "max_height": 200}))), (300.0, 300.0));
        assert_eq!(fit_range(&Value::Null), FIT);
    }

    fn act(id: &str, label: &str, primary: bool, disabled: bool) -> CardAction {
        CardAction { id: id.into(), label: label.into(), primary, disabled }
    }

    #[test]
    fn brg_19_actions_are_at_most_three_one_line_labels_with_one_primary() {
        let ok = parse_actions(Some(&json!([
            {"id": "later", "label": "  Later  "},
            {"id": "pay.now", "label": "Mark paid", "primary": true},
            {"id": "x:1", "label": "Off", "disabled": true, "primary": null},
        ])))
        .unwrap();
        assert_eq!(ok, [act("later", "Later", false, false), act("pay.now", "Mark paid", true, false), act("x:1", "Off", false, true)]);
        // the primary first, the rest in the app's order
        let ids: Vec<&str> = footer_order(&ok).iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["pay.now", "later", "x:1"]);
        assert!(parse_actions(Some(&json!([]))).unwrap().is_empty(), "an empty list clears the footer");
        assert_eq!(parse_actions(Some(&json!([{"id": "a", "label": "x".repeat(MAX_LABEL)}]))).unwrap()[0].label.len(), MAX_LABEL);
        let refused = |v: Value| parse_actions(Some(&v)).unwrap_err();
        let four: Vec<Value> = (0..4).map(|i| json!({"id": format!("a{i}"), "label": "A"})).collect();
        for (v, why) in [
            (json!(four), "a card has at most 3 actions"),
            (json!([{"id": "a", "label": "A"}, {"id": "a", "label": "B"}]), "two actions are a"),
            (json!([{"id": "a", "label": "A", "primary": true}, {"id": "b", "label": "B", "primary": true}]), "a card has one primary action"),
            (json!([{"id": "a", "label": "   "}]), "a label is empty"),
            (json!([{"id": "a", "label": "Pay\nnow"}]), "a label is one line"),
            (json!([{"id": "a", "label": "x".repeat(MAX_LABEL + 1)}]), "a label is at most 24 characters"),
            (json!([{"id": "a", "label": 5}]), "a label is a string"),
            (json!([{"id": "a b", "label": "A"}]), "an action's id is 1 to 64 of A-Z a-z 0-9 _ . : -"),
            (json!([{"id": "x".repeat(MAX_ACTION_ID + 1), "label": "A"}]), "an action's id is 1 to 64 of A-Z a-z 0-9 _ . : -"),
            (json!([{"label": "A"}]), "an action's id is 1 to 64 of A-Z a-z 0-9 _ . : -"),
            (json!([{"id": "a", "label": "A", "color": "red"}]), "an action does not take color"),
            (json!([{"id": "a", "label": "A", "primary": "yes"}]), "primary is a boolean"),
            (json!(["a"]), "an action is {id, label, primary?, disabled?}"),
            (json!({"id": "a"}), "rox.actions takes a list"),
        ] {
            let e = refused(v);
            assert_eq!((e.code.as_str(), e.message.as_str()), ("invalid_args", why));
        }
        assert_eq!(parse_actions(None).unwrap_err().code, "invalid_args");
    }

    #[test]
    fn brg_19_meta_is_a_short_status_and_settle_one_outcome_line() {
        assert_eq!(parse_meta(Some(&json!({"text": " Due in 3 days ", "pill": true}))).unwrap(), Some(CardMeta { text: "Due in 3 days".into(), pill: true }));
        assert_eq!(parse_meta(Some(&json!({"text": "Paid"}))).unwrap(), Some(CardMeta { text: "Paid".into(), pill: false }));
        assert_eq!(parse_meta(Some(&Value::Null)).unwrap(), None, "null clears it");
        assert_eq!(parse_meta(None).unwrap(), None);
        for (v, why) in [
            (json!({"text": "x".repeat(MAX_META + 1)}), "the status is at most 32 characters"),
            (json!({"text": ""}), "the status is empty"),
            (json!({"text": "a\tb"}), "the status is one line"),
            (json!({"text": "Paid", "tone": "green"}), "rox.meta does not take tone"),
            (json!({"text": "Paid", "pill": 1}), "pill is a boolean"),
            (json!("Paid"), "rox.meta takes {text, pill?} or null"),
        ] {
            assert_eq!(parse_meta(Some(&v)).unwrap_err().message, why);
        }
        assert_eq!(parse_settle(Some(&json!("  Marked paid · ₱12,000  "))).unwrap(), "Marked paid · ₱12,000");
        assert_eq!(parse_settle(Some(&json!("é".repeat(MAX_SETTLE)))).unwrap().chars().count(), MAX_SETTLE);
        for (v, why) in [
            (json!("x".repeat(MAX_SETTLE + 1)), "the outcome is at most 120 characters"),
            (json!("Paid\nThanks"), "the outcome is one line"),
            (json!(" "), "the outcome is empty"),
            (json!(5), "the outcome is a string"),
        ] {
            assert_eq!(parse_settle(Some(&v)).unwrap_err().message, why);
        }
        assert_eq!(parse_settle(None).unwrap_err().code, "invalid_args");
    }
}
