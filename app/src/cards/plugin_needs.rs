//! The plugin-needs card (nativechat `render_plugin_needs`): a tagged
//! plugin needs something before the turn can run — installing, an
//! account, or which of several accounts to use. The bot never guesses
//! the account: the person picks one ("Remember for <bot>" keeps it as the
//! bot's pin), and the message goes again. Install and Add account open
//! Connect Apps on the plugin (`rox://plugin/<name>`).

use nsh_core::prelude::*;
use serde_json::Value;

use super::transport::{self, Sent};
use super::{Btn, CardState};
use crate::live::store;
use crate::theme;

/// The CUSTOM `name` the card arrives under.
pub(crate) const PLUGIN_NEEDS_CUSTOM: &str = "opengrok.pluginNeeds";

/// What each tagged plugin still needs, in the server's order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PluginNeedsSpec {
    pub needs: Vec<PluginNeed>,
    /// Answering it sends the message again: the turn that stopped for the
    /// card did not run.
    pub send_again: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PluginNeed {
    pub plugin: String,
    /// The service an account is for; none on an install.
    pub connector: Option<String>,
    pub kind: PluginNeedKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PluginNeedKind {
    Install,
    Account,
    /// `(id, label, kind)` of each account, never a secret.
    Choose(Vec<(String, String, String)>),
}

impl PluginNeedsSpec {
    /// The card an `opengrok.pluginNeeds` frame carries; a need this app
    /// does not know is left out, not guessed at.
    pub(crate) fn from_wire(event: &Value) -> Option<Self> {
        if event.get("name").and_then(Value::as_str) != Some(PLUGIN_NEEDS_CUSTOM) {
            return None;
        }
        let listed = event.get("value")?.get("needs")?.as_array()?;
        let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
        let needs: Vec<PluginNeed> = listed
            .iter()
            .filter_map(|need| {
                let kind = match need.get("need").and_then(Value::as_str)? {
                    "install" => PluginNeedKind::Install,
                    "account" => PluginNeedKind::Account,
                    "choose" => PluginNeedKind::Choose(
                        need.get("accounts")?
                            .as_array()?
                            .iter()
                            .filter_map(|a| Some((text(a, "id")?, text(a, "label")?, text(a, "kind").unwrap_or_default())))
                            .collect(),
                    ),
                    _ => return None,
                };
                Some(PluginNeed { plugin: text(need, "plugin")?, connector: text(need, "connector"), kind })
            })
            .collect();
        (!needs.is_empty()).then_some(Self { needs, send_again: true })
    }
}

/// `cloudflare` is Cloudflare.
fn titled(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// How an account was added, in Connect Apps' words.
fn how(kind: &str) -> &'static str {
    match kind {
        "mcp" => "Signed in",
        "token" => "Key",
        _ => "Account",
    }
}

/// Only the chat's last turn can be sent again, and not while a run goes:
/// an older one has messages after it.
fn answerable(turn: i64) -> bool {
    let st = store();
    let last = st.turns.with(|t| t.last().map(|t| t.i));
    let running = st.agent().is_some_and(|a| a.state != "waiting");
    let thread = st.active_id.get();
    last == Some(turn) && !running && st.sending.with(|s| crate::store::may_send(s, &thread))
}

pub(crate) fn card(spec: PluginNeedsSpec, key: String, turn: i64) -> impl IntoElement {
    let state = {
        let key = key.clone();
        computed(move || {
            (
                super::state_of(&key),
                store().card_remember.with(|r| r.contains(&key)),
                answerable(turn),
            )
        })
    };
    dynamic(move || {
        let (st, remember, open) = state.get();
        match st {
            Some(CardState::Answered(line)) => super::outcome_line(line).into_element(),
            Some(CardState::Superseded) | Some(CardState::Dismissed) => {
                super::outcome_line("Moved on to your next message.".to_owned()).into_element()
            }
            _ => open_card(&spec, &key, turn, remember, open, st).into_element(),
        }
    })
}

fn open_card(
    spec: &PluginNeedsSpec,
    key: &str,
    turn: i64,
    remember: bool,
    open: bool,
    st: Option<CardState>,
) -> impl IntoElement {
    let bot = super::bot_name();
    let mut body = super::shell(&format!("plugin-needs-{key}")).gap_3();
    for need in &spec.needs {
        body = body.child(need_row(need, key, turn, &bot, open));
    }
    if spec.needs.iter().any(|n| matches!(n.kind, PluginNeedKind::Choose(_))) {
        let key2 = key.to_owned();
        body = body.child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(crate::plugins::tool_switch(
                    move || remember,
                    move || {
                        let key = key2.clone();
                        store().card_remember.update(|r| {
                            if !r.remove(&key) {
                                r.insert(key);
                            }
                        });
                    },
                ))
                .child(text(format!("Remember for {bot}")).text_size(px(13.0)).text_color(theme::TEXT)),
        );
    }
    let again = spec.send_again && spec.needs.iter().any(|n| !matches!(n.kind, PluginNeedKind::Choose(_)));
    if again && open {
        let key = key.to_owned();
        body = body.child(
            view().flex_row().child(super::btn(Btn::Secondary, "Send again", move || {
                send_again(&key, turn, "Sent again.".to_owned())
            })),
        );
    }
    if let Some(CardState::Error(why)) = st {
        body = body.child(super::error_line(why));
    }
    body
}

fn need_row(need: &PluginNeed, key: &str, turn: i64, bot: &str, open: bool) -> impl IntoElement {
    let name = titled(&need.plugin);
    let plugin = need.plugin.clone();
    let open_plugins = move |label: &'static str| {
        let plugin = plugin.clone();
        view().flex_row().child(super::btn(Btn::Primary, label, move || {
            store().open_link(&format!("rox://plugin/{plugin}"))
        }))
    };
    match &need.kind {
        PluginNeedKind::Install => view()
            .gap(px(6.0))
            .child(super::title(format!("{name} isn't installed")))
            .child(super::note(format!(
                "Install it in Connect Apps, then send again. {bot} can use it as soon as it has an account."
            )))
            .child(open_plugins("Open in Connect Apps")),
        PluginNeedKind::Account => view()
            .gap(px(6.0))
            .child(super::title(format!("{name} needs an account")))
            .child(super::note(
                "Sign in or add a key in Connect Apps, then send again. It never goes through chat.".to_owned(),
            ))
            .child(open_plugins("Add account")),
        PluginNeedKind::Choose(accounts) => {
            let connector = need.connector.clone().unwrap_or_else(|| need.plugin.clone());
            let mut col = view().gap_2().child(super::title(format!("Which {name} account should {bot} use?")));
            for (id, label, kind) in accounts {
                let (key, connector, id, label2) = (key.to_owned(), connector.clone(), id.clone(), label.clone());
                col = col.child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(10.0))
                        .px(px(10.0))
                        .py(px(8.0))
                        .rounded(px(10.0))
                        .border(px(0.5))
                        .border_color(theme::BORDER_S)
                        .child(
                            view()
                                .flex_1()
                                .min_w(px(0.0))
                                .child(text(label.clone()).text_size(px(13.0)).text_color(theme::TEXT).number_of_lines(1))
                                .child(super::note(how(kind).to_owned())),
                        )
                        .when(open, |r| {
                            r.child(super::btn(Btn::Primary, "Use", move || {
                                use_account(&key, turn, &connector, &id, &label2)
                            }))
                        }),
                );
            }
            col
        }
    }
}

/// "Use": the account goes with the message sent again; with Remember on,
/// it is kept as the bot's pin first.
fn use_account(key: &str, turn: i64, connector: &str, account: &str, label: &str) {
    if !answerable(turn) || !super::answerable(key) {
        return;
    }
    let remember = store().card_remember.with_untracked(|r| r.contains(key));
    let line = format!("Sent again with {label}.");
    // This server takes no per-turn accounts: the choice is only logged.
    log::info!("rox cards: plugin account for the next turn: {connector} = {account}");
    if remember {
        let (key, connector, account, coworker) =
            (key.to_owned(), connector.to_owned(), account.to_owned(), super::agent_id());
        super::set_state(&key, CardState::Sending);
        Owner::detached().run(move || {
            spawn(async move {
                match transport::pin_account(&coworker, &connector, &account).await {
                    Sent::Done(_) => {}
                    Sent::Offline => transport::not_connected(),
                    Sent::Refused(why) => {
                        crate::toast::show(format!("The account was used but not remembered: {why}"))
                    }
                }
                send_again(&key, turn, line);
            });
        });
        return;
    }
    send_again(key, turn, line);
}

/// The person's message before the card, sent again; the card leaves
/// `line` behind.
fn send_again(key: &str, turn: i64, line: String) {
    let st = store();
    let text = st.turns.with_untracked(|t| {
        t.iter().rev().skip_while(|t| t.i != turn).find(|t| t.role == "user").map(|t| t.text.clone())
    });
    let Some(text) = text.filter(|t| !t.trim().is_empty()) else {
        super::set_state(key, CardState::Error("There is no message before it to send again.".to_owned()));
        return;
    };
    st.card_remember.update(|r| {
        r.remove(key);
    });
    super::set_state(key, CardState::Answered(line));
    st.send_text(text);
}
