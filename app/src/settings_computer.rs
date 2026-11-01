//! Settings › Computer: what an agent may run on this Mac, and whether Bots
//! route their web traffic out through it. "Local commands" is this
//! machine's consent mode — Never allow, Ask every time, Always allow
//! (`bypass` on the wire) — with the rules a command card's "Always allow"
//! / "Never" kept, listed and removable, and new ones typed in; a rule the
//! gate can never match (`inert`) is drawn dimmed with the server's reason.
//! "Egress tunnel" is the host switch, with the note that says when the
//! tunnel is not available.
//!
//! Each half is read when the page mounts. A broker without the route
//! answers `501 not_ported` (and no broker at all answers nothing): the
//! controls show disabled under a one-line note, never an error.
//! Transcribed from nativechat's Computer page (`opengrok/src/components/
//! app_settings.rs`: `local_rules_block`, `local_rule_row`,
//! `settings_route_traffic_row`, `not_in_effect_line`) and its state
//! methods (`opengrok/src/state.rs`). Mounted by `settings.rs`.

use nsh_base::input;
use nsh_core::prelude::*;
use serde_json::{Value, json};

use crate::cards::transport;
use crate::theme;

/// The `mode` of `GET/PUT /local-exec/policy` (opengrok-server
/// `local_exec.rs` `VALID_MODES`), with nativechat's labels for them
/// (`opengrok/src/opengrok/client.rs` `LocalExecMode::label`).
const MODES: [(&str, &str, &str); 3] = [
    ("never", "Never allow", "Commands an agent asks to run on this Mac are refused."),
    ("ask", "Ask every time", "Each command waits for your answer on its card; the rules below settle the ones they match."),
    ("bypass", "Always allow", "Commands run on this Mac without asking. The rules below are not read."),
];

/// The two rule lists, in page order: the wire `kind` and its heading
/// (nativechat `rule_list_title`).
const KINDS: [(&str, &str); 2] = [("allow", "Always allowed"), ("deny", "Never allowed")];

/// Under the lists while they are empty (nativechat `NO_LOCAL_RULES`).
const NO_RULES: &str = "No commands are always allowed or never allowed yet. Always allow or Never on a command's card keeps one here.";

/// Over the lists (nativechat `LOCAL_RULES_NOTE`).
const RULES_NOTE: &str = "Always allow and Never on a command's card keep the command here. They apply only while this computer is set to Ask every time.";

/// A rule's `pattern` (`inert` row's `reason`) is drawn dimmed under this
/// line (nativechat `not_in_effect_line`).
pub(crate) fn not_in_effect_line(reason: &str) -> String {
    let reason = reason.trim();
    if reason.is_empty() {
        "Not in effect.".to_owned()
    } else if reason.ends_with(['.', '!', '?']) {
        format!("Not in effect: {reason}")
    } else {
        format!("Not in effect: {reason}.")
    }
}

/// The body of `GET /local-exec/policy` (opengrok-server `local_exec.rs`
/// `policy_listing`): `{machineId, mode, allow, deny, inert: [{pattern,
/// reason}]}`. `raw` is the record as it came, every field, so nothing
/// the server adds later is lost between a read and a write.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Policy {
    pub(crate) machine_id: String,
    pub(crate) mode: String,
    pub(crate) allow: Vec<String>,
    pub(crate) deny: Vec<String>,
    /// `pattern` → `reason`, for the allow rows the gate never reads.
    pub(crate) inert: Vec<(String, String)>,
    pub(crate) raw: Value,
}

impl Policy {
    pub(crate) fn parse(raw: &Value) -> Policy {
        let field = |k: &str| raw.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
        let list = |k: &str| -> Vec<String> {
            raw.get(k)
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default()
        };
        let inert = raw
            .get("inert")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|r| {
                        let pattern = r.get("pattern").and_then(Value::as_str)?;
                        let reason = r.get("reason").and_then(Value::as_str).unwrap_or_default();
                        Some((pattern.to_owned(), reason.to_owned()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Policy { machine_id: field("machineId"), mode: field("mode"), allow: list("allow"), deny: list("deny"), inert, raw: raw.clone() }
    }

    /// The rows of `kind`, each with its reason when it is inert.
    fn rows(&self, kind: &str) -> Vec<(String, Option<String>)> {
        let list = if kind == "allow" { &self.allow } else { &self.deny };
        list.iter()
            .map(|p| {
                let inert =
                    (kind == "allow").then(|| self.inert.iter().find(|(q, _)| q == p).map(|(_, r)| r.clone())).flatten();
                (p.clone(), inert)
            })
            .collect()
    }

    fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty()
    }
}

/// The host record of `GET/PUT /ag-ui/host-settings` (opengrok-server
/// `agui/routes/host.rs` `host_settings_reply`): `egressTunnelEnabled` is
/// the host's intent, `egressTunnelAvailable` whether a tunnel can carry
/// traffic now. `raw` keeps the whole record.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HostSettings {
    pub(crate) egress_tunnel_enabled: bool,
    pub(crate) egress_tunnel_available: bool,
    pub(crate) raw: Value,
}

impl HostSettings {
    pub(crate) fn parse(raw: &Value) -> HostSettings {
        let flag = |k: &str| raw.get(k).and_then(Value::as_bool).unwrap_or(false);
        HostSettings {
            egress_tunnel_enabled: flag("egressTunnelEnabled"),
            egress_tunnel_available: flag("egressTunnelAvailable"),
            raw: raw.clone(),
        }
    }
}

/// How far a read got. The page's controls are live only under `Live`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Reach {
    Loading,
    Live,
    /// 501 `not_ported` (or 404): the broker has no such route yet.
    NotPorted,
    /// No server answered.
    Offline,
    /// Another status: its sentence.
    Failed(String),
}

impl Reach {
    fn live(&self) -> bool {
        matches!(self, Reach::Live)
    }

    /// The one line under a half that is not live, or none.
    fn note(&self, what: &str) -> Option<String> {
        match self {
            Reach::Loading | Reach::Live => None,
            Reach::NotPorted => Some(format!("{what} isn't on this broker yet.")),
            Reach::Offline => Some("Not connected to an agent server yet.".to_owned()),
            Reach::Failed(why) => Some(why.clone()),
        }
    }
}

/// A reply read the page's way: 2xx its body; a missing route (501
/// `not_ported`, the hub's answer for an unported route, or 404) and no
/// server are each a kind of "not here"; anything else its sentence.
pub(crate) fn reach(reply: Option<(u16, Value)>) -> Result<Value, Reach> {
    match reply {
        None => Err(Reach::Offline),
        Some((200..=299, body)) => Ok(body),
        Some((404 | 501, _)) => Err(Reach::NotPorted),
        Some((status, body)) => Err(Reach::Failed(
            transport::refusal_sentence(&body).unwrap_or_else(|| format!("The server answered {status}; nothing was changed.")),
        )),
    }
}

/// The page's state, anchored (like the store) so a hot patch or a
/// remount of Settings keeps what was read.
#[derive(Clone, Copy)]
struct Ui {
    policy: Signal<Option<Policy>>,
    policy_reach: Signal<Reach>,
    host: Signal<Option<HostSettings>>,
    host_reach: Signal<Reach>,
    /// The text in the Add field of each list: allow, deny.
    draft_allow: Signal<String>,
    draft_deny: Signal<String>,
    /// A write in flight: the controls wait for it.
    busy: Signal<bool>,
    /// Whether the host record has been asked for at all: the right pane's
    /// icon rail can be the first surface that wants it, before this page
    /// is ever opened.
    host_asked: Signal<bool>,
    /// What the last refused write said.
    note: Signal<String>,
}

fn ui() -> Ui {
    let at = ra_jit_live::anchor("rox.settings.computer", || {
        let ui = Owner::detached().run(|| Ui {
            policy: signal(None),
            policy_reach: signal(Reach::Loading),
            host: signal(None),
            host_reach: signal(Reach::Loading),
            draft_allow: signal(String::new()),
            draft_deny: signal(String::new()),
            busy: signal(false),
            host_asked: signal(false),
            note: signal(String::new()),
        });
        Box::leak(Box::new(ui)) as *const Ui as usize
    });
    // SAFETY: the anchor holds a leaked `Ui` for the life of the process.
    unsafe { *(at as *const Ui) }
}

fn draft(kind: &str) -> Signal<String> {
    if kind == "allow" { ui().draft_allow } else { ui().draft_deny }
}

/// Reads both halves again.
fn reload() {
    let u = ui();
    Owner::detached().run(|| {
        spawn(async move { apply_policy(transport::read_policy().await) });
    });
    load_host(u);
}

/// `GET /ag-ui/host-settings` into the page's signals.
fn load_host(u: Ui) {
    u.host_asked.set(true);
    Owner::detached().run(|| {
        spawn(async move {
            match reach(transport::read_host_settings().await) {
                Ok(body) => {
                    u.host.set(Some(HostSettings::parse(&body)));
                    u.host_reach.set(Reach::Live);
                }
                Err(r) => {
                    u.host.set(None);
                    u.host_reach.set(r);
                }
            }
        });
    });
}

/// Whether the server answered about the host record, and the tunnel's
/// intent — for the right pane's icon rail, which draws the same switch
/// (`pane::route_traffic_look`) and reads the signals this page owns.
pub(crate) fn host_egress_tunnel() -> (bool, bool) {
    let u = ui();
    (u.host_reach.get().live(), u.host.get().is_some_and(|h| h.egress_tunnel_enabled))
}

/// Reads the host record if nothing has asked for it yet.
pub(crate) fn ensure_host_settings() {
    let u = ui();
    if !u.host_asked.get_untracked() {
        load_host(u);
    }
}

/// The host switch's write, as the rail presses it: one transport for
/// both surfaces, and the record is read again after it.
pub(crate) fn set_egress_tunnel(enabled: bool) {
    set_egress(enabled);
}

/// The listing just read decides what the page shows: live controls over
/// it, or nothing and the reason, whether the read followed a write or not.
fn apply_policy(reply: Option<(u16, Value)>) {
    let u = ui();
    match reach(reply) {
        Ok(body) => {
            u.policy.set(Some(Policy::parse(&body)));
            u.policy_reach.set(Reach::Live);
        }
        Err(r) => {
            u.policy.set(None);
            u.policy_reach.set(r);
        }
    }
}

/// A write, then the policy read again: the server's listing is the truth
/// (an add it refused is not in it).
fn write_policy(req: impl Future<Output = Option<(u16, Value)>> + 'static) {
    let u = ui();
    if u.busy.get_untracked() {
        return;
    }
    u.busy.set(true);
    u.note.set(String::new());
    Owner::detached().run(|| {
        spawn(async move {
            if let Err(Reach::Failed(why)) = reach(req.await) {
                u.note.set(why);
            }
            apply_policy(transport::read_policy().await);
            u.busy.set(false);
        });
    });
}

fn set_mode(mode: &'static str) {
    if ui().policy.with_untracked(|p| p.as_ref().is_some_and(|p| p.mode == mode)) {
        return;
    }
    write_policy(transport::write_mode(mode));
}

fn add_rule(kind: &'static str) {
    let pattern = draft(kind).get_untracked().trim().to_owned();
    if pattern.is_empty() {
        return;
    }
    draft(kind).set(String::new());
    write_policy(async move { transport::call("POST", "/local-exec/policy/rule", Some(&transport::rule_body(kind, &pattern))).await });
}

fn remove_rule(kind: &'static str, pattern: String) {
    write_policy(async move { transport::remove_rule(kind, &pattern).await });
}

/// The switch: the record comes back whole, `egressTunnelAvailable` on it.
fn set_egress(enabled: bool) {
    let u = ui();
    if u.busy.get_untracked() {
        return;
    }
    u.busy.set(true);
    u.note.set(String::new());
    Owner::detached().run(|| {
        spawn(async move {
            match reach(transport::write_host_settings(json!({ "egressTunnelEnabled": enabled })).await) {
                Ok(body) => u.host.set(Some(HostSettings::parse(&body))),
                Err(Reach::Failed(why)) => u.note.set(why),
                Err(r) => {
                    u.host.set(None);
                    u.host_reach.set(r);
                }
            }
            u.busy.set(false);
            // the server's own answer again, so nothing local is left
            // standing for a write it did not take
            load_host(u);
        });
    });
}

/// The Computer page body, for Settings to mount beside its other pages.
pub(crate) fn settings_page() -> impl IntoElement {
    reload();
    let u = ui();
    view()
        .flex_1()
        .h_full()
        .min_w(px(0.0))
        .test_id("settings-computer")
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .px_4()
                .pt_4()
                .pb_2()
                .child(text("Computer").text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT).flex_1())
                .child(small_btn("settings-computer-refresh", "Refresh", reload)),
        )
        .child(
            text("What an agent may run on this Mac, and whether Bots that share it send their web traffic out through it.")
                .text_size(px(12.0))
                .text_color(theme::MUTED)
                .px_4()
                .pb_2(),
        )
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.0))
                .child(
                    view()
                        .px_4()
                        .pb_4()
                        .gap_3()
                        .child(dynamic(move || egress_block(u).into_element()))
                        .child(dynamic(move || local_block(u).into_element()))
                        .child(dynamic(move || {
                            let n = u.note.get();
                            if n.is_empty() {
                                return view().into_element();
                            }
                            text(n).text_size(px(12.0)).text_color(theme::DOWN).test_id("settings-computer-note").into_element()
                        })),
                ),
        )
}

/// A section's frame: a hairline box with its title.
fn section(id: &str, title: &'static str) -> View {
    view()
        .w_full()
        .gap_2()
        .px_3()
        .py_3()
        .rounded(px(12.0))
        .border_1()
        .border_color(theme::BORDER_S)
        .bg(theme::CARD)
        .test_id(id)
        .child(text(title).text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT))
}

fn quiet_note(id: &str, line: String) -> impl IntoElement {
    text(line).text_size(px(12.0)).text_color(theme::FAINT).test_id(id)
}

// ---------------------------------------------------------------------------
// Local commands
// ---------------------------------------------------------------------------

fn local_block(u: Ui) -> impl IntoElement {
    let reach = u.policy_reach.get();
    let live = reach.live();
    let policy = u.policy.get();
    let mode = policy.as_ref().map(|p| p.mode.clone()).unwrap_or_default();
    let mut block = section("settings-local-commands", "Local commands");
    block = block.child(mode_picker(&mode, live));
    if let Some((_, _, what)) = MODES.iter().find(|(m, ..)| *m == mode) {
        block = block.child(text(*what).text_size(px(12.0)).text_color(theme::MUTED));
    }
    if let Some(line) = reach.note("Local command policy") {
        return block.child(quiet_note("settings-local-commands-note", line));
    }
    let Some(policy) = policy else {
        return block;
    };
    block = block.child(
        text(if policy.is_empty() { NO_RULES } else { RULES_NOTE }).text_size(px(12.0)).text_color(theme::MUTED).pt_1(),
    );
    for (kind, title) in KINDS {
        block = block.child(rule_list(u, kind, title, policy.rows(kind)));
    }
    block
}

/// Never / Ask / Bypass as three chips; the current one filled. Disabled
/// (not live): dimmed, and a press does nothing.
fn mode_picker(mode: &str, live: bool) -> impl IntoElement {
    view().flex_row().items_center().gap(px(6.0)).test_id("settings-local-mode").children(MODES.iter().map(|(wire, label, _)| {
        let on = *wire == mode;
        let id = format!("settings-local-mode-{wire}");
        let mut chip = text(*label)
            .text_size(px(12.0))
            .text_color(if on { theme::TEXT } else { theme::MUTED })
            .px_2()
            .py(px(5.0))
            .rounded(px(7.0))
            .border_1()
            .border_color(if on { theme::ACCENT } else { theme::BORDER_S })
            .bg(if on { theme::ACCENT_TINT } else { theme::HOVER })
            .opacity(if live { 1.0f32 } else { 0.45 })
            .label(*label)
            .test_id(&id);
        if live {
            chip = chip.on_press(move || set_mode(wire));
        }
        chip.into_element()
    }))
}

/// One list: its heading, its rows, and the field + Add under them.
fn rule_list(u: Ui, kind: &'static str, title: &'static str, rows: Vec<(String, Option<String>)>) -> impl IntoElement {
    let id = format!("settings-local-rules-{kind}");
    let mut list = view()
        .w_full()
        .gap(px(6.0))
        .pt_1()
        .test_id(&id)
        .child(text(title).text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::MUTED));
    if rows.is_empty() {
        list = list.child(text("None yet").text_size(px(12.0)).text_color(theme::GHOST));
    }
    for (n, (pattern, inert)) in rows.into_iter().enumerate() {
        list = list.child(rule_row(u, kind, n, pattern, inert));
    }
    let d = draft(kind);
    let field_id = format!("settings-local-rule-input-{kind}");
    let add_id = format!("settings-local-rule-add-{kind}");
    list.child(
        view()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                input()
                    .value(move || d.get())
                    .placeholder(if kind == "allow" { "Command to always allow, e.g. git status" } else { "Command to never allow, e.g. rm -rf" })
                    .placeholder_color(|| color(theme::GHOST))
                    .text_size(px(12.0))
                    .text_color(theme::TEXT)
                    .on_change(move |v: &str| d.set(v.to_owned()))
                    .on_submit(move |_: &str| add_rule(kind))
                    .flex_1()
                    .h(px(28.0))
                    .px_2()
                    .rounded(px(7.0))
                    .bg(theme::HOVER)
                    .test_id(&field_id),
            )
            .child(small_btn_id(add_id, "Add", move || add_rule(kind))),
    )
}

/// A rule: its pattern (dimmed, with the reason, when inert) and Remove.
fn rule_row(u: Ui, kind: &'static str, n: usize, pattern: String, inert: Option<String>) -> impl IntoElement {
    let dim = inert.is_some();
    let remove_id = format!("settings-local-rule-remove-{kind}-{n}");
    let row_id = format!("settings-local-rule-{kind}-{n}");
    let p = pattern.clone();
    view()
        .flex_row()
        .items_start()
        .gap_2()
        .px_2()
        .py(px(5.0))
        .rounded(px(7.0))
        .bg(theme::SUBTLE)
        .test_id(&row_id)
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(2.0))
                .child(
                    text(pattern)
                        .font_family("Menlo")
                        .text_size(px(12.0))
                        .text_color(if dim { theme::FAINT } else { theme::TEXT })
                        .number_of_lines(2),
                )
                .when(dim, |v| {
                    v.child(
                        text(not_in_effect_line(inert.as_deref().unwrap_or_default()))
                            .text_size(px(11.0))
                            .text_color(theme::FAINT),
                    )
                }),
        )
        .child(small_btn_id(remove_id, if u.busy.get() { "…" } else { "Remove" }, move || {
            remove_rule(kind, p.clone())
        }))
}

// ---------------------------------------------------------------------------
// Egress tunnel
// ---------------------------------------------------------------------------

fn egress_block(u: Ui) -> impl IntoElement {
    let reach = u.host_reach.get();
    let host = u.host.get();
    let enabled = host.as_ref().is_some_and(|h| h.egress_tunnel_enabled);
    let available = host.as_ref().is_some_and(|h| h.egress_tunnel_available);
    let live = reach.live();
    let what = if enabled {
        "New connections from Bots that share this computer go out through this desktop."
    } else {
        "Route web traffic from Bots that share this computer out through this desktop instead of the cloud. Applies to new connections."
    };
    let mut block = section("settings-egress-tunnel", "Egress tunnel").child(
        view()
            .flex_row()
            .items_center()
            .gap_3()
            .test_id("route-traffic-this-computer")
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap(px(2.0))
                    .child(text("Route traffic through this computer").text_size(px(13.0)).text_color(theme::TEXT))
                    .child(text(what).text_size(px(12.0)).text_color(theme::MUTED)),
            )
            .child(switch("egress-tunnel-enabled", enabled, live && !u.busy.get(), move || set_egress(!enabled))),
    );
    if let Some(line) = reach.note("The egress tunnel setting") {
        return block.child(quiet_note("settings-egress-tunnel-note", line));
    }
    if live && !available {
        block = block.child(quiet_note(
            "settings-egress-tunnel-availability",
            if enabled {
                "Not available right now: no Bot sharing this computer has a tunnel ready, so traffic still goes out through the cloud.".to_owned()
            } else {
                "Not available right now: turning it on takes effect once a Bot sharing this computer has a tunnel ready.".to_owned()
            },
        ));
    }
    block
}

/// An on/off pill: the knob slides right and the track takes the accent
/// when on. Disabled: dimmed, and a press does nothing.
fn switch(id: &str, on: bool, live: bool, flip: impl Fn() + 'static) -> impl IntoElement {
    let mut track = view()
        .w(px(34.0))
        .h(px(20.0))
        .rounded(px(10.0))
        .p(px(2.0))
        .items_end()
        .bg(if on { theme::ACCENT } else { theme::GHOST })
        .opacity(if live { 1.0f32 } else { 0.45 })
        .test_id(id)
        .label(if on { "On" } else { "Off" })
        .child(view().w(px(16.0)).h(px(16.0)).rounded(px(8.0)).bg(theme::ON_FILL).when(!on, |v| v.absolute().left(px(2.0)).top(px(2.0))));
    if live {
        track = track.on_press(flip);
    }
    track
}

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

fn small_btn(id: &'static str, label: &'static str, on: impl Fn() + 'static) -> impl IntoElement {
    small_btn_id(id.to_owned(), label, on)
}

/// A small text button, like the other settings pages' ("Refresh").
fn small_btn_id(id: String, label: &'static str, on: impl Fn() + 'static) -> impl IntoElement {
    let hov = signal(false);
    text(label)
        .text_size(px(12.0))
        .text_color(move || if hov.get() { theme::TEXT } else { theme::MUTED })
        .px_2()
        .py_1()
        .rounded(px(6.0))
        .bg(theme::HOVER)
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .on_press(on)
        .label(label)
        .test_id(&id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_listing_parses_with_inert_reasons() {
        let body = json!({
            "machineId": "m1", "mode": "ask",
            "allow": ["git status", "sudo *"], "deny": ["rm -rf *"],
            "inert": [{"pattern": "sudo *", "reason": "sudo is never allowed by a standing rule"}],
            "future": {"x": 1}
        });
        let p = Policy::parse(&body);
        assert_eq!(p.mode, "ask");
        assert_eq!(p.rows("allow"), vec![
            ("git status".to_owned(), None),
            ("sudo *".to_owned(), Some("sudo is never allowed by a standing rule".to_owned())),
        ]);
        assert_eq!(p.rows("deny"), vec![("rm -rf *".to_owned(), None)]);
        assert_eq!(p.raw["future"]["x"], 1);
    }

    #[test]
    fn host_settings_keep_the_record() {
        let body = json!({"egressTunnelEnabled": true, "egressTunnelAvailable": false, "other": "kept"});
        let h = HostSettings::parse(&body);
        assert!(h.egress_tunnel_enabled && !h.egress_tunnel_available);
        assert_eq!(h.raw, body);
    }

    #[test]
    fn not_ported_and_no_server_are_quiet() {
        assert_eq!(reach(Some((501, json!({"ok": false, "code": "not_ported"})))), Err(Reach::NotPorted));
        assert_eq!(reach(Some((404, Value::Null))), Err(Reach::NotPorted));
        assert_eq!(reach(None), Err(Reach::Offline));
        assert_eq!(reach(Some((200, json!({"mode": "ask"})))), Ok(json!({"mode": "ask"})));
        assert_eq!(
            reach(Some((422, json!({"error": "kind must be allow|deny and pattern non-empty"})))),
            Err(Reach::Failed("kind must be allow|deny and pattern non-empty".to_owned()))
        );
        assert_eq!(Reach::NotPorted.note("Local command policy").as_deref(), Some("Local command policy isn't on this broker yet."));
    }

    #[test]
    fn not_in_effect_line_punctuates() {
        assert_eq!(not_in_effect_line(""), "Not in effect.");
        assert_eq!(not_in_effect_line("sudo is refused"), "Not in effect: sudo is refused.");
        assert_eq!(not_in_effect_line("Refused!"), "Not in effect: Refused!");
    }
}
