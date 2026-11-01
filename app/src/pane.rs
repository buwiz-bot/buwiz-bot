//! The right detail pane (App.tsx:2465-3007): 320pt of bot profile or,
//! when the open thread has no bot, the app card. Variant A carries the
//! editable avatar (hover → darkened + pencil + the Bot|Generate|Upload
//! editor card), the name field, the model pill and the
//! Details|Library|Computer tabs; variant B the app's meta,
//! `/state/:app` kvs and the window/pin/publish buttons.

use std::str::FromStr;

use nsh_base::input;
use nsh_core::prelude::*;
use nsh_core::element::View;

use crate::avatar::{bot_avatar, BlobEdit, BlobGaze, BotAvatarProps};
use crate::live::store;
use crate::store::{AppMeta, Crop, Store, RoxApp};
use crate::theme;

/// The pane's top bar height — the TSX's `TOP`.
pub(crate) const TOP: f32 = 60.0;
/// The pane's width (`width: 320`).
const W: f32 = 320.0;
/// The 64pt profile avatar's crop box (`CROP = 84` in the TSX editor).
const CROP: f32 = 84.0;
/// The zoom slider's track (`TRACK = 236`).
const TRACK: f32 = 236.0;

/// The right column — `rightEff`'s panel (App.tsx:2467).
pub(crate) fn pane() -> impl IntoElement {
    let st = store();
    view()
        .w(px(W))
        .h_full()
        // `tc("#121212")` — #121212 on dark, white on light (App.tsx:2469);
        // CARD (#1f1f1f) only matched on light. 0.5pt hairline like Grok's.
        .bg(theme::PANE)
        .border_l(px(0.5))
        .border_l_color(theme::BORDER_F)
        .child(top_bar())
        .child(
            view().flex_1().child(
                dynamic(move || {
                    // A plain chat gets the same profile as an agent (the
                    // app's default avatar, its own title): Details,
                    // Library and Computer apply to both.
                    // an agents' conversation: its own Details (no name
                    // field, model nor tabs: nobody talks in it)
                    if st.active_agents().is_some() {
                        conversation_pane().into_element()
                    } else if st.active_bot().is_some() || !st.active_id.get().is_empty() {
                        bot_profile().into_element()
                    } else if st.detail_meta.get().is_some() {
                        app_detail().into_element()
                    } else {
                        empty_pane().into_element()
                    }
                }),
            ),
        )
}

// `avatarEditing`'s floating editor card — mounted at the root layers so
// its scrim covers the whole window (App.tsx:3163).

/// `pane top` — the window-drag strip's right side: the theme toggle and
/// the close button, both 36pt circles (App.tsx:2474).
fn top_bar() -> impl IntoElement {
    let st = store();
    view()
        .flex_row()
        .items_center()
        .justify_end()
        .gap_2()
        .px_3()
        .h(px(TOP))
        .window_drag(true)
        .child(icon_rail())
        .child(pane_circle_btn("theme", || {
            // App.tsx:2492 — the pref's own icon, not the resolved scheme.
            match store().theme_pref.get().as_str() {
                "light" => "sun",
                "dark" => "moon",
                _ => "systemTheme",
            }
        }, || {
            let st = store();
            st.set_theme(st.next_theme());
        }))
        .child(pane_circle_btn("close", || "close", || {
            store().right_open.set(false);
        }))
        .on_mouse_leave(|_: &MouseEvent| st.pane_btn_hover.set(String::new()))
}

/// `circleBtn()` — a 36pt border circle hovering to `#262626`.
fn pane_circle_btn(
    key: &'static str,
    name: impl Fn() -> &'static str + 'static,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    view()
        .size(px(36.0))
        .rounded(px(18.0))
        .items_center()
        .justify_center()
        .border_1()
        .border_color(theme::BORDER_F)
        .bg(move || {
            // `circleBtn().backgroundColor` = `tc("#1f1f1f")`, hover
            // `tc("#262626")` (App.tsx:2490).
            if store().pane_btn_hover.get() == key {
                theme::tc("#262626")
            } else {
                theme::CARD
            }
        })
        .child(dynamic(move || icon_view(name()).into_element()))
        .on_mouse_enter(move |_: &MouseEvent| store().pane_btn_hover.set(key.to_owned()))
        .on_mouse_leave(|_: &MouseEvent| store().pane_btn_hover.set(String::new()))
        .on_press(on_press)
        .label(match key {
            "theme" => "Theme",
            _ => "Close",
        })
}

/// `<Icon size={15} color="#f5f5f5e6" />` (App.tsx:2493) — the pane's
/// round buttons ink brighter than the default `DIM` glyphs.
fn icon_view(name: &'static str) -> impl IntoElement {
    crate::icons::icon_ink(name, 15.0, || theme::tc("#f5f5f5e6"))
}

// ————————————————————————— the pane's icon rail —————————————————————————
// NativeChat's row of small icon buttons over a Bot's screen
// (`opengrok/src/components/computer.rs` `box_chrome`), here in the
// pane's own top strip, left of the theme and close circles: Computer,
// Recipes, Plugins, Tools, Route traffic, this computer's network rule,
// Update, Reset and Bot settings. Every copy string below is transcribed
// from that file; what a state looks like is a plain function of the
// state, tested in this module's tests.

/// A rail glyph's ink.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RailInk {
    Muted,
    /// The tab this button opens is the one showing.
    Active,
    Accent,
    Danger,
    Success,
}

impl RailInk {
    fn color(self) -> AdaptiveColor {
        match self {
            RailInk::Muted => theme::MUTED,
            RailInk::Active => theme::TEXT,
            RailInk::Accent => theme::ACCENT,
            RailInk::Danger => theme::DOWN,
            RailInk::Success => theme::UP,
        }
    }
}

/// How one rail button draws: its glyph, its ink, whether a press does
/// anything (a disabled one is drawn at half opacity, as `box_chrome`'s
/// `icon_btn_enabled` does), and the sentence it says on hover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RailLook {
    pub icon: &'static str,
    pub ink: RailInk,
    pub enabled: bool,
    pub tip: String,
}

/// Said after a tooltip for a button the broker has no route for yet.
const NOT_YET: &str = " — not on this server yet";

/// What the reroute icon says on hover (`opengrok src/components/
/// computer.rs` `reroute_tip`): the host-wide switch, so it says it
/// applies to all of the person's computers.
fn reroute_tip(routing: bool) -> &'static str {
    if routing {
        "Routing traffic through this desktop for all your computers. New connections go out through it. Click to stop."
    } else {
        "Traffic goes out on its own. Click to route web traffic from all your computers through this desktop instead."
    }
}

/// What this computer's network rule says on hover (`opengrok
/// src/components/computer.rs` `network_rule_tip`).
fn network_rule_tip(current: crate::computer::Egress) -> String {
    format!(
        "Use your network from this computer: {}. Click to change. Applies to this computer only.",
        current.label()
    )
}

/// Route traffic through this desktop — the host switch Settings ›
/// Computer owns (`settings_computer`, `GET/PUT /ag-ui/host-settings`
/// `egressTunnelEnabled`). `answered` is whether the server said
/// anything at all: a broker without the route (404/501) or none at all
/// leaves the globe muted and dead (`opengrok src/components/computer.rs`
/// `route_traffic_icon`).
pub(crate) fn route_traffic_look(answered: bool, routing: bool) -> RailLook {
    if !answered {
        return RailLook {
            icon: "globe",
            ink: RailInk::Muted,
            enabled: false,
            tip: "This server has no tunnel yet.".to_owned(),
        };
    }
    if routing {
        RailLook { icon: "routeTraffic", ink: RailInk::Success, enabled: true, tip: reroute_tip(true).to_owned() }
    } else {
        RailLook { icon: "globe", ink: RailInk::Muted, enabled: true, tip: reroute_tip(false).to_owned() }
    }
}

/// This computer's network rule (`PUT|GET /coworkers/{id}/computer/
/// egress-policy`): tinted when the answer is not "ask", dead until the
/// server has one (`opengrok src/components/computer.rs`
/// `network_policy_icon`).
pub(crate) fn network_policy_look(current: Option<crate::computer::Egress>) -> RailLook {
    use crate::computer::Egress;
    let ink = match current {
        None | Some(Egress::Ask) => RailInk::Muted,
        Some(Egress::Always) => RailInk::Accent,
        Some(Egress::Never) => RailInk::Danger,
    };
    let tip = current
        .map(network_rule_tip)
        .unwrap_or_else(|| "This computer has no network rule from the server yet.".to_owned());
    RailLook { icon: "shieldBadge", ink, enabled: current.is_some(), tip }
}

/// Update this computer: the status is the tooltip, and the glyph is
/// tinted while a newer image waits (`opengrok src/components/
/// computer.rs` `box_chrome`). The enable rule is the Computer tab's own
/// (`computer::controls`): a box, nothing running on it, and something
/// to update.
pub(crate) fn update_look(present: bool, busy: bool, stale: bool, current: bool) -> RailLook {
    let tip = if stale {
        "Update available"
    } else if current {
        "Up to date"
    } else if present {
        "Update this computer"
    } else {
        "No computer yet"
    };
    RailLook {
        icon: "download",
        ink: if stale { RailInk::Success } else { RailInk::Muted },
        enabled: present && !busy && !(current && !stale),
        tip: tip.to_owned(),
    }
}

/// Reset this computer (`box_chrome`: a box that is not updating).
pub(crate) fn reset_look(present: bool, busy: bool) -> RailLook {
    RailLook {
        icon: "reset",
        ink: RailInk::Muted,
        enabled: present && !busy,
        tip: "Reset this computer".to_owned(),
    }
}

/// `box_chrome`'s hover fill: `#777777` at 20%, the same on either theme.
fn rail_hover() -> AdaptiveColor {
    theme::tone(theme::hsla(0.0, 0.0, 0.467, 0.2), theme::hsla(0.0, 0.0, 0.467, 0.2))
}

/// One rail button: a 28pt square with an 8pt radius and a 16pt glyph,
/// and its sentence in a tooltip under it after a moment (the rail's own
/// copy of `sidebar::rail_circle`'s tooltip, which hangs above its rail).
fn rail_btn(id: &'static str, look: RailLook, on: impl Fn() + 'static) -> impl IntoElement {
    let hovered = signal(false);
    let tip = signal(false);
    let epoch = std::rc::Rc::new(std::cell::Cell::new(0u64));
    let epoch2 = epoch.clone();
    let enabled = look.enabled;
    let ink = look.ink;
    let words = look.tip.clone();
    view()
        .test_id(id)
        .label(look.tip.clone())
        // a disabled button takes no presses and says it is disabled, to
        // screen readers and to gpui-agent's snapshot
        .disabled(!enabled)
        .size(px(28.0))
        .rounded(px(8.0))
        .items_center()
        .justify_center()
        .opacity(if enabled { 1.0 } else { 0.5 })
        .bg(move || if hovered.get() { rail_hover() } else { AdaptiveColor::TRANSPARENT })
        .child(crate::icons::icon_ink(look.icon, 16.0, move || ink.color()))
        .child(when(
            move || tip.get(),
            move || {
                // Its own width: the sentence on one line plus the padding,
                // up to the pane's own width less its padding.
                let one_line = measure_text("420 12px -apple-system, system-ui, sans-serif", &words).0.ceil() + 12.0;
                view()
                    .absolute()
                    .top(px(28.0 + 6.0))
                    .right(px(-6.0))
                    .z_index(40)
                    .pointer_events(PointerEvents::None)
                    .w(px(one_line.min(W - 32.0)))
                    .py(px(4.0))
                    .px(px(6.0))
                    .rounded(px(6.0))
                    .bg(theme::tone(theme::hsla(0.0, 0.0, 1.0, 1.0), theme::hsla(0.0, 0.0, 0.122, 1.0)))
                    .child(
                        text(words.clone())
                            .text_size(px(12.0))
                            .line_height(px(16.0))
                            .font_weight(FontWeight(420))
                            .text_color(theme::TEXT),
                    )
                    .into_element()
            },
        ))
        .on_mouse_enter(move |_: &MouseEvent| {
            hovered.set(true);
            let e = epoch.get().wrapping_add(1);
            epoch.set(e);
            let epoch = epoch.clone();
            spawn(async move {
                timer(std::time::Duration::from_millis(500)).await;
                if epoch.get() == e {
                    tip.set(true);
                }
            });
        })
        .on_mouse_leave(move |_: &MouseEvent| {
            epoch2.set(epoch2.get().wrapping_add(1));
            hovered.set(false);
            tip.set(false);
        })
        .on_press(move || {
            tip.set(false);
            if enabled {
                on()
            }
        })
}

/// Opens the pane on `tab`, or closes it when that tab is the one open.
fn show_tab(tab: &'static str) {
    let st = store();
    if st.detail_tab.get_untracked() == tab && st.right_open.get_untracked() {
        st.right_open.set(false);
    } else {
        st.detail_tab.set(tab.to_owned());
        st.right_open.set(true);
    }
}

/// The chat's own way back while the pane is closed (`hot::top_bar`): the
/// rail's button, opening the pane on `tab`. Its ids are the title bar's
/// own (`title-monitor`, `title-right-sidebar`) so they never collide
/// with the pane's.
pub(crate) fn open_pane_btn(
    id: &'static str,
    icon: &'static str,
    tip: &'static str,
    tab: &'static str,
) -> impl IntoElement {
    rail_btn(id, RailLook { icon, ink: RailInk::Muted, enabled: true, tip: tip.to_owned() }, move || {
        store().detail_tab.set(tab.to_owned());
        store().right_open.set(true);
    })
}

/// The rail, rebuilt as the tab, the host switch and this bot's computer
/// change.
fn icon_rail() -> impl IntoElement {
    view().flex_row().items_center().gap(px(2.0)).child(dynamic(|| rail_buttons().into_element()))
}

fn rail_buttons() -> impl IntoElement {
    use crate::computer::Action;
    let st = store();
    let computer_shown = st.detail_tab.get() == "computer";
    let details_shown = st.detail_tab.get() == "details";
    let bot = st.active_bot();
    // The host record is Settings › Computer's read; the rail asks for it
    // too, so the globe says the truth before Settings is ever opened.
    crate::settings_computer::ensure_host_settings();
    let (answered, routing) = crate::settings_computer::host_egress_tunnel();
    let computer = bot.as_ref().and_then(|b| crate::computer::rail_computer(&b.id));
    let present = computer.as_ref().is_some_and(crate::computer::Computer::present);
    let busy = computer.as_ref().is_some_and(crate::computer::Computer::updating);
    let stale = computer.as_ref().is_some_and(crate::computer::Computer::image_stale);
    let current = computer.as_ref().is_some_and(crate::computer::Computer::image_current);
    view()
        .flex_row()
        .items_center()
        .gap(px(2.0))
        .child(rail_btn(
            "header-monitor",
            RailLook {
                icon: "monitor",
                ink: if computer_shown { RailInk::Active } else { RailInk::Muted },
                enabled: true,
                tip: "Computer".to_owned(),
            },
            || show_tab("computer"),
        ))
        .child(rail_btn(
            "computer-recipes",
            RailLook {
                icon: "cake",
                ink: RailInk::Muted,
                enabled: false,
                tip: format!("Recipes: tasks taught on this screen{NOT_YET}"),
            },
            || {},
        ))
        .child(rail_btn(
            "computer-plugins",
            RailLook {
                icon: "plugins",
                ink: RailInk::Muted,
                enabled: false,
                tip: format!("Plugins for this Bot{NOT_YET}"),
            },
            || {},
        ))
        .child(rail_btn(
            "computer-tools",
            RailLook {
                icon: "wrench",
                ink: RailInk::Muted,
                enabled: bot.is_some(),
                tip: "Tools for this Bot".to_owned(),
            },
            // the Tools card lives in Details
            || {
                store().detail_tab.set("details".to_owned());
            },
        ))
        .child(rail_btn("route-traffic-this-computer", route_traffic_look(answered, routing), move || {
            crate::settings_computer::set_egress_tunnel(!routing);
        }))
        .child(rail_btn(
            "network-policy",
            network_policy_look(computer.as_ref().and_then(|c| c.egress)),
            // the Computer tab holds the choice itself
            || {
                store().detail_tab.set("computer".to_owned());
            },
        ))
        .child(rail_btn("computer-update", update_look(present, busy, stale, current), || {
            store().detail_tab.set("computer".to_owned());
            crate::computer::ask_confirm(Action::Update);
        }))
        .child(rail_btn("computer-reset", reset_look(present, busy), || {
            store().detail_tab.set("computer".to_owned());
            crate::computer::ask_confirm(Action::Reset);
        }))
        .child(rail_btn(
            "header-right-sidebar",
            RailLook {
                icon: "settings",
                ink: if details_shown { RailInk::Active } else { RailInk::Muted },
                enabled: true,
                tip: "Bot settings".to_owned(),
            },
            || show_tab("details"),
        ))
}

// ————————————————————————— variant A: the bot profile —————————————————————————

/// `bot profile` — the gaze-following 64pt avatar, the name field, the
/// model pill, and the tab pills (App.tsx:2505).
fn bot_profile() -> impl IntoElement {
    let st = store();
    // `panelGaze`/`panelEdit`/`panelEditK` — the pane avatar's own
    // follow/edit drivers (App.tsx:205-207).
    let gaze = BlobGaze::new();
    let edit = BlobEdit::new();
    let edit_k = crate::avatar::FadeValue::new();
    let gaze2 = gaze.clone();
    let gaze3 = gaze.clone();
    let edit2 = edit.clone();
    let edit3 = edit.clone();
    let edit_k2 = edit_k.clone();
    let edit_k3 = edit_k.clone();
    view()
        .flex_1()
        .child(
            // The profile block follows the pointer for the avatar's gaze
            // (`onHoverMove`/`panelGaze`, App.tsx:2513).
            view()
                .items_center()
                .pt_2()
                .on_mouse_move(move |e: &MouseEvent| {
                    gaze.follow(e.position.x.0 as f64, e.position.y.0 as f64, 160.0, 40.0, 32.0)
                })
                .on_mouse_leave(move |_: &MouseEvent| gaze3.rest())
                .child(
                    // The 64pt avatar: hover darkens (editK) and shows the
                    // pencil + the "Edit Avatar" tooltip (App.tsx:2519).
                    view()
                        .size(px(64.0))
                        .items_center()
                        .justify_center()
                        // Only an agent's avatar is editable.
                        .on_mouse_enter(move |_: &MouseEvent| {
                            if store().active_bot_untracked().is_none() {
                                return;
                            }
                            edit_k.timing(1.0, 220.0);
                            edit.to(1.0);
                            store().avatar_hover.set(true);
                        })
                        .on_mouse_leave(move |_: &MouseEvent| {
                            edit_k2.timing(0.0, 380.0);
                            edit3.to(0.0);
                            store().avatar_hover.set(false);
                        })
                        .on_press(|| {
                            if store().active_bot_untracked().is_some() {
                                store().open_avatar_editor()
                            }
                        })
                        .label("Edit Bot avatar")
                        .child(
                            dynamic(move || {
                                let mut p = match st.active_bot() {
                                    Some(b) => BotAvatarProps::bot(&b, 80.0),
                                    // A plain chat: the app's default mark.
                                    None => BotAvatarProps {
                                        shape: "blob".to_owned(),
                                        color: "#FFFFFF".to_owned(),
                                        size: 80.0,
                                        seed: Some("app".to_owned()),
                                        ..Default::default()
                                    },
                                };
                                p.box_ = Some(64.0);
                                p.gaze = Some(gaze2.clone());
                                p.edit = Some(edit2.clone());
                                p.morph_shape = true;
                                bot_avatar(p).into_element()
                            })
                            .into_element(),
                        )
                        // The hover pen at `editK` opacity — the avatar
                        // itself darkens via `edit`, no veil (App.tsx:2547).
                        .child(
                            view()
                                .absolute()
                                .size_full()
                                .items_center()
                                .justify_center()
                                .opacity(move || {
                                    if store().avatar_editing.get() {
                                        0.0
                                    } else {
                                        edit_k3.get()
                                    }
                                })
                                .child(
                                    text("✎").text_size(px(22.0)).text_color(white()),
                                ),
                        ),
                )
                // The name field — Enter commits `updateBot {name}`
                // (App.tsx:2560).
                .child(
                    view()
                        .pt_4()
                        .mx(px(40.0))
                        .child(
                            input()
                                // `value={botEditName()}` — the draft is
                                // the text, empty included; the store
                                // reseeds it only when the bot or its
                                // name actually changes.
                                .value(move || store().bot_edit_name.get())
                                .on_change(|v: &str| store().bot_edit_name.set(v.to_owned()))
                                .on_submit(|_: &str| {
                                    let st = store();
                                    let n = st.bot_edit_name.get_untracked().trim().to_owned();
                                    if n.is_empty() {
                                        return;
                                    }
                                    match st.active_bot_untracked() {
                                        Some(b) if n != b.name => {
                                            st.update_bot(&b.id, serde_json::json!({"name": n}))
                                        }
                                        Some(_) => {}
                                        // A plain chat's field names the chat.
                                        None => st.rename_chat(n),
                                    }
                                })
                                .placeholder("Bot name")
                                .placeholder_color(|| color(theme::GHOST))
                                .text_size(px(17.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_center()
                                .text_color(theme::TEXT)
                                .rounded(px(8.0))
                                .py(px(2.0))
                                .px_2(),
                        ),
                )
                // The agent's label (Grok Bot's "Add a label"): Enter saves
                // it; a plain chat reads "Chat", a routine's thread "Routine
                // of <bot>" (the routine answers as the bot).
                .child(dynamic(|| match (store().active_routine(), store().active_bot()) {
                    (Some(_), b) => text(format!("Routine of {}", b.map(|b| b.name).unwrap_or_else(|| crate::app_name().to_owned())))
                        .text_size(px(11.0))
                        .text_color(theme::MUTED)
                        .pt(px(4.0))
                        .into_element(),
                    (None, Some(b)) => {
                        // "Made by Piccolo" under an agent another one made
                        let made = crate::agents::made_by_line(&b, &store().bots.get());
                        let parent = b.helper.as_ref().map(|h| h.parent.clone()).unwrap_or_else(|| b.made_by.clone());
                        view()
                            .items_center()
                            .child(label_field(b.id, b.label))
                            .when_some(made, |v, m| v.child(made_by(m, parent)))
                            .into_element()
                    }
                    (None, None) => text("Chat")
                        .text_size(px(11.0))
                        .text_color(theme::MUTED)
                        .pt(px(4.0))
                        .into_element(),
                }))
                .child(model_pill().pt(px(10.0)).into_element())
                // The "Edit Avatar" tooltip — 8pt under the avatar, over
                // the name (App.tsx:2587).
                .child(when(
                    || store().avatar_hover.get() && !store().avatar_editing.get(),
                    || {
                        view()
                            .absolute()
                            .top(px(80.0))
                            .py(px(4.0))
                            .px(px(6.0))
                            .rounded(px(6.0))
                            // Opaque, as Grok Bot's: `CARD2` is a 6% wash on
                            // light and the name read through it.
                            .bg(theme::POPOVER)
                            .border_1()
                            .border_color(theme::BORDER_F)
                            .shadow([BoxShadow {
                                offset_x: px(0.0),
                                offset_y: px(2.0),
                                blur: px(8.0),
                                spread: px(-1.0),
                                color: hsla(0.0, 0.0, 0.0, 0.12).into(),
                                inset: false,
                            }])
                            .child(text("Edit Avatar").text_size(px(12.0)).text_color(theme::TEXT))
                            .into_element()
                    },
                ))
                .child(tab_pills()),
        )
        .child(
            scroll()
                .flex_1()
                .child(
                    view()
                        .px_4()
                        .pt(px(10.0))
                        .pb_6()
                        .child(dynamic(|| detail_body().into_element())),
                ),
        )
}

/// The agent's label under its name: an inline field, saved with Enter
/// (`/bot/update {label}`); empty, it shows "Add a label".
fn label_field(id: String, label: String) -> impl IntoElement {
    let draft = signal(label);
    input()
        .value(move || draft.get())
        .on_change(move |v: &str| draft.set(v.to_owned()))
        .on_submit(move |_: &str| {
            let st = store();
            let v = draft.get_untracked().trim().to_owned();
            st.bots.update(|bots| {
                if let Some(b) = bots.iter_mut().find(|b| b.id == id) {
                    b.label = v.clone();
                }
            });
            st.update_bot(&id, serde_json::json!({ "label": v }));
        })
        .placeholder("Add a label")
        .placeholder_color(|| color(theme::MUTED))
        .text_size(px(11.0))
        .text_center()
        .text_color(theme::MUTED)
        .mt(px(4.0))
        .w(px(200.0))
}

/// `ModelPill(choice)` — the shared pill (`models::model_pill`,
/// modelpicker.tsx:60): the bot's model + effort label, opening the
/// anchored popover.
fn model_pill() -> View {
    view().child(crate::models::model_pill())
}

/// The Details|Library|Computer pills (App.tsx:2605).
fn tab_pills() -> impl IntoElement {
    // Connect left the pane: connectors are the Plugins window's, and the
    // agent installs / lists / toggles them by tool (mcp_*, skill_*).
    let tabs = ["details", "library", "computer"];
    view()
        .flex_row()
        .gap(px(1.0))
        .pt(px(14.0))
        .pb(px(10.0))
        .children(tabs.into_iter().map(|tb| {
            view()
                .rounded(px(999.0))
                .px_2()
                .py_1()
                .bg(move || {
                    if store().detail_tab.get() == tb {
                        theme::SUBTLE
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .child(
                    text({
                        let mut c = tb.chars();
                        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
                    })
                    .text_size(px(13.0))
                    .text_color(move || {
                        if store().detail_tab.get() == tb {
                            theme::TEXT
                        } else {
                            theme::DIM
                        }
                    }),
                )
                .on_press(move || store().detail_tab.set(tb.to_owned()))
                .into_element()
        }))
}

/// The tab's body — Details, Library or Computer.
fn detail_body() -> impl IntoElement {
    match store().detail_tab.get().as_str() {
        "library" => library_tab().into_element(),
        "computer" => computer_tab().into_element(),
        _ => details_tab().into_element(),
    }
}

/// `paneLabel` — a section's 13pt dim title (App.tsx:1233).
fn pane_label(t: String) -> impl IntoElement {
    text(t)
        .text_size(px(13.0))
        .text_color(theme::DIM)
        .px_3()
        .pb(px(10.0))
}

/// `paneCard()` — the `#1f1f1f` 12pt card with a 0.5pt border.
fn pane_card() -> View {
    view()
        .bg(theme::CARD2)
        .rounded(px(12.0))
        .border_1()
        .border_color(theme::BORDER_F)
        .p_1()
}

/// Details — the Routines card (App.tsx:2627). Routines are managed
/// here, with the bot they belong to (not in Settings): each row runs,
/// pauses or resumes its routine; Suggested offers the templates.
fn details_tab() -> impl IntoElement {
    store().v1_refresh();
    crate::routines::load_routines();
    crate::routines::load_templates();
    view()
        // An agent's own context: what it is, what it remembers, what it
        // has used — each fills on demand (asking it in chat, or using a
        // plugin), so empty sections say how.
        .child(dynamic(|| match store().active_bot() {
            Some(b) => bot_context(b).into_element(),
            None => view().into_element(),
        }))
        // which tools it may use: one switch per tool, read from the broker
        // for this bot and read again when the active bot changes
        .child(dynamic(|| {
            let bot = store().active_bot().map(|b| b.id).unwrap_or_default();
            crate::ceiling::load(bot.clone());
            if bot.is_empty() { view().into_element() } else { tools_card().into_element() }
        }))
        // its agents: whom it takes messages from, its conversations with
        // other agents, the helpers it started
        .child(dynamic(|| match store().active_bot() {
            Some(b) if store().active_routine().is_none() => agents_sections(b).into_element(),
            _ => view().into_element(),
        }))
        // a routine's thread: that routine alone, nothing to open
        .child(dynamic(|| {
            let bot = store().active_bot().map(|b| b.name).unwrap_or_else(|| crate::app_name().to_owned());
            let listed = store().v1.listed.get();
            match routines_section(store().active_routine(), &listed, &bot, crate::store::checked_at) {
                RoutinesSection::This(r) => {
                    view().child(pane_label("This routine".into())).child(this_routine(*r)).into_element()
                }
                RoutinesSection::List => view()
                    .child(pane_label("Routines".into()))
                    .child(routines_list())
                    .child(crate::routines::suggested())
                    .into_element(),
            }
        }))
}

// ————————————————————————— tools for this bot —————————————————————————

/// Details — "Tools for this Bot" (NativeChat's Tools card, opengrok
/// `src/components/agent_settings.rs` `CeilingCard`): one row per tool the
/// broker lists for the active bot, grouped by kind, each with a switch.
/// Without rows the card is one quiet line: still reading, the broker has no
/// ceiling route yet, the broker is away, or nothing to switch.
fn tools_card() -> impl IntoElement {
    view().pb(px(14.0)).child(pane_label("Tools for this Bot".into())).child(dynamic(|| {
        let card = store().ceiling.get();
        let Some(read) = card.rows().filter(|r| !r.tools.is_empty()) else {
            let line = card.line().unwrap_or_default();
            return pane_card()
                .test_id("tools-card")
                .label(line.clone())
                .py(px(14.0))
                .px(px(13.0))
                .child(text(line).text_size(px(13.0)).text_color(theme::DIM))
                .into_element();
        };
        let mut v = pane_card().test_id("tools-card").label("Tools for this Bot");
        if let Some(note) = card.note.clone() {
            v = v.child(text(note).text_size(px(12.0)).text_color(theme::DIM).px(px(10.0)).pt(px(8.0)).pb(px(4.0)));
        }
        for kind in [crate::ceiling::CeilingKind::Builtin, crate::ceiling::CeilingKind::Plugin] {
            let rows = read.of_kind(kind);
            if rows.is_empty() {
                continue;
            }
            v = v
                .child(
                    text(kind.heading())
                        .text_size(px(11.0))
                        .text_color(theme::MUTED)
                        .px(px(10.0))
                        .pt(px(8.0))
                        .pb(px(2.0)),
                )
                .children(rows.into_iter().map(|r| tool_row(&card, r)));
        }
        v.into_element()
    }))
}

/// The switch's track: the `#1680f3`/`#1e86fc` blue Settings › Plugins uses
/// when on, a 15% ink when off (`plugins.rs` `tool_switch`).
fn switch_track(on: bool) -> AdaptiveColor {
    if on {
        let dark = Hsla::from_str("#1e86fc").unwrap_or(theme::ACCENT.dark);
        theme::tone(Hsla::from_str("#1680f3").unwrap_or(dark), dark)
    } else {
        theme::tone(theme::hsla(0.0, 0.0, 0.11, 0.15), theme::hsla(0.0, 0.0, 0.96, 0.15))
    }
}

/// One tool: its title and a line under it (why it cannot move, whose
/// connection it uses, or what it does), and the switch. The whole row is the
/// control, id `tool-switch-<name>`, so the driver clicks it by name. A row
/// the broker could not offer is dimmed and only goes off; nothing moves
/// while another switch is with the broker.
fn tool_row(card: &crate::ceiling::CeilingCard, r: crate::ceiling::CeilingRow) -> AnyElement {
    let on = card.shown_on(&r);
    let live = card.may_switch(&r);
    let title = r.title().to_owned();
    let subtitle = r.subtitle();
    let name = r.name.clone();
    let hov = signal(false);
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .px(px(10.0))
        .py(px(8.0))
        .rounded(px(8.0))
        .opacity(if r.is_available() { 1.0 } else { 0.5 })
        .bg(move || if hov.get() && live { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .test_id(&format!("tool-switch-{}", r.name))
        .label(format!("{title}: {}", if on { "on" } else { "off" }))
        .on_press(move || {
            if live {
                crate::ceiling::switch(name.clone(), !on);
            }
        })
        .child(
            view()
                .flex_1()
                .gap(px(2.0))
                .child(text(title.clone()).text_size(px(13.0)).text_color(theme::TEXT).number_of_lines(1))
                .when_some(subtitle, |v, s| v.child(text(s).text_size(px(11.0)).text_color(theme::MUTED).number_of_lines(2))),
        )
        .child(
            // the same 26×16 switch Settings › Plugins draws (`plugins.rs` `tool_switch`)
            view()
                .w(px(26.0))
                .h(px(16.0))
                .rounded(px(8.0))
                .justify_center()
                .bg(switch_track(on))
                .child(view().size(px(12.0)).rounded(px(6.0)).bg(white()).ml(if on { px(12.0) } else { px(2.0) })),
        )
        .into_element()
}

// ————————————————————————— agents' conversations —————————————————————————

/// "Made by Piccolo" under an agent's label: opens the parent's chat.
fn made_by(line: String, parent: String) -> impl IntoElement {
    let hov = signal(false);
    text(line.clone())
        .text_size(px(11.0))
        .text_color(move || if hov.get() { theme::TEXT } else { theme::MUTED })
        .pt(px(2.0))
        .label(line)
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .on_press(move || {
            let t = store().bot_thread(&parent);
            if !t.is_empty() {
                store().go_to(&t, None)
            }
        })
}

/// A section's quiet line ("No conversations yet …").
fn pane_note(t: String) -> AnyElement {
    pane_card().py(px(14.0)).px(px(13.0)).child(text(t).text_size(px(13.0)).text_color(theme::DIM)).into_element()
}

/// A bot's Details sections about other agents: Accept messages from,
/// Conversations, Helper and Past helpers.
fn agents_sections(b: crate::store::RoxBot) -> impl IntoElement {
    let st = store();
    let bots = st.bots.get();
    let threads = st.threads.get();
    let rows = crate::agents::conversations_of(&b.id, &threads, &bots);
    st.load_agents_last(
        rows.iter()
            .filter(|r| !r.archived)
            .map(|r| (r.thread.clone(), threads.iter().find(|t| t.id == r.thread).map(|t| t.count).unwrap_or(0)))
            .collect(),
    );
    let (live, past) = crate::agents::helpers_of(&b.id, &bots);
    let (open, archived): (Vec<_>, Vec<_>) = rows.into_iter().partition(|r| !r.archived);
    let show_archived = signal(false);
    let n_archived = archived.len();
    let name = b.name.clone();
    view()
        .pb(px(14.0))
        // a helper takes messages from its parent only: no setting
        .when(b.helper.is_none(), |v| v.child(pane_label("Accept messages from".into())).child(accepts_card(b.id.clone(), b.accepts.clone())))
        .child(view().pt(px(14.0)).child(pane_label("Conversations".into())))
        .child(if open.is_empty() && archived.is_empty() {
            pane_note(format!("No conversations yet. Ask {name} in chat to hand a task to another agent."))
        } else {
            pane_card()
                .children(open.into_iter().map(conversation_row))
                .when(n_archived > 0, move |v| {
                    let hov = signal(false);
                    v.child(
                        view()
                            .flex_row()
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(8.0))
                            .bg(move || if hov.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
                            .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
                            .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
                            .label(format!("Archived conversations ({n_archived})"))
                            .on_press(move || show_archived.update(|s| *s = !*s))
                            .child(
                                text(move || format!("Archived ({n_archived}) {}", if show_archived.get() { "▾" } else { "›" }))
                                    .text_size(px(12.0))
                                    .text_color(theme::MUTED),
                            ),
                    )
                    .child(when(show_archived, move || {
                        view().children(archived.clone().into_iter().map(conversation_row)).into_element()
                    }))
                })
                .into_element()
        })
        .when(!live.is_empty(), |v| {
            v.child(view().pt(px(14.0)).child(pane_label("Helper".into())))
                .child(pane_card().children(live.into_iter().map(helper_row)))
        })
        .when(!past.is_empty(), |v| {
            v.child(view().pt(px(14.0)).child(pane_label("Past helpers".into())))
                .child(pane_card().children(past.into_iter().map(helper_row)))
        })
}

/// One conversation in a bot's Details: subject, participants, its last
/// message; Open, Resume while paused, Archive or Unarchive.
fn conversation_row(r: crate::agents::ConversationRow) -> AnyElement {
    let last = {
        let thread = r.thread.clone();
        move || store().agents_last.with(|l| l.iter().find(|(t, ..)| *t == thread).map(|(.., m)| m.clone())).unwrap_or_default()
    };
    let (t1, c1, c2) = (r.thread.clone(), r.conversation.clone(), r.conversation.clone());
    let subject = r.subject.clone();
    let mut acts = view().flex_row().flex_wrap().gap_1().pt(px(2.0)).child(routine_act("Open", format!("Open {subject}"), false, move || store().go_to(&t1, None)));
    if r.paused {
        acts = acts.child(dynamic(move || {
            let c = c2.clone();
            if store().agents_paused(&c) {
                routine_act("Resume", format!("Resume {}", "the conversation"), false, move || store().agents_resume(c.clone()))
            } else {
                view().into_element()
            }
        }));
    }
    let archived = r.archived;
    acts = acts.child(routine_act(
        if archived { "Unarchive" } else { "Archive" },
        format!("{} {subject}", if archived { "Unarchive" } else { "Archive" }),
        false,
        move || store().agents_archive(c1.clone(), !archived),
    ));
    view()
        .gap(px(2.0))
        .px(px(10.0))
        .py(px(8.0))
        .rounded(px(8.0))
        .label(r.subject.clone())
        .child(text(r.subject).text_size(px(13.0)).text_color(theme::TEXT).number_of_lines(1))
        .child(
            text(if r.paused { format!("{} · paused", r.participants) } else { r.participants })
                .text_size(px(12.0))
                .text_color(theme::MUTED)
                .number_of_lines(1),
        )
        .when(!archived, |v| {
            v.child(dynamic(move || {
                let t = last();
                if t.is_empty() {
                    return view().into_element();
                }
                text(t).text_size(px(12.0)).text_color(theme::DIM).number_of_lines(2).into_element()
            }))
        })
        .child(acts)
        .into_element()
}

/// A temporary helper in its parent's Details: avatar, name, task; Open
/// goes to its chat.
fn helper_row(b: crate::store::RoxBot) -> AnyElement {
    let hov = signal(false);
    let thread = b.thread_id.clone();
    let task = b.helper.as_ref().map(|h| h.task.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| b.purpose.clone());
    view()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .px(px(10.0))
        .py(px(8.0))
        .rounded(px(8.0))
        .bg(move || if hov.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .label(format!("Open {}'s chat", b.name))
        .on_press(move || store().go_to(&thread, None))
        .child(crate::chat::peer_avatar(&b.id, &b.name, 26.0))
        .child(
            view()
                .flex_1()
                .child(text(b.name.clone()).text_size(px(13.0)).text_color(theme::TEXT).number_of_lines(1))
                .when(!task.is_empty(), |v| v.child(text(task).text_size(px(12.0)).text_color(theme::MUTED).number_of_lines(2))),
        )
        .child(text("Open").text_size(px(12.0)).text_color(move || if hov.get() { theme::TEXT } else { theme::ACCENT }))
        .into_element()
}

/// Accept messages from: All my agents / None (`some`, a list an agent
/// set, shows as such until one is picked).
fn accepts_card(bot: String, now: String) -> impl IntoElement {
    let choice = |word: &'static str, value: &'static str, on: bool, bot: String| {
        let hov = signal(false);
        view()
            .flex_row()
            .items_center()
            .px(px(10.0))
            .py(px(7.0))
            .rounded(px(8.0))
            .bg(move || if hov.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
            .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
            .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
            .label(format!("Accept messages from {}", word.to_lowercase()))
            .on_press(move || {
                if !on {
                    store().agents_accepts(bot.clone(), value)
                }
            })
            .child(text(word).text_size(px(13.0)).text_color(theme::TEXT).flex_1())
            .when(on, |v| v.child(text("✓").text_size(px(13.0)).text_color(theme::ACCENT)))
            .into_element()
    };
    pane_card()
        .child(choice("All my agents", "all", now == "all", bot.clone()))
        .child(choice("None", "none", now == "none", bot))
        .when(now == "some", |v| {
            v.child(text("Now: some agents only").text_size(px(12.0)).text_color(theme::MUTED).px(px(10.0)).pb(px(6.0)))
        })
}

/// An agents' conversation's Details: its participants (each with "Go to
/// <name> chat"), subject, when it started, how many messages; Resume
/// while paused, Archive or Unarchive.
fn conversation_pane() -> impl IntoElement {
    view()
        .flex_1()
        .child(scroll().flex_1().child(view().px_4().pt(px(4.0)).pb_6().child(dynamic(|| {
            let st = store();
            let Some(a) = st.active_agents() else { return view().into_element() };
            let names = st.agents_names(&a);
            let title = crate::agents::pill_text(&names);
            let id = st.active_id.get();
            let count = if st.turns_of.get() == id {
                st.turns.with(|t| t.iter().filter(|t| t.message.is_some()).count() as i64)
            } else {
                st.threads.with(|t| t.iter().find(|t| t.id == id).map(|t| t.count).unwrap_or(0))
            };
            let paused = st.agents_paused(&a.conversation);
            let (c1, c2, archived) = (a.conversation.clone(), a.conversation.clone(), a.archived);
            let fact = |k: &str, v: String| {
                view()
                    .flex_row()
                    .gap(px(10.0))
                    .px(px(10.0))
                    .py(px(6.0))
                    .child(text(k.to_owned()).text_size(px(12.0)).text_color(theme::MUTED).w(px(70.0)))
                    .child(text(v).text_size(px(13.0)).text_color(theme::TEXT).flex_1().number_of_lines(3))
                    .into_element()
            };
            let status = match (archived, paused) {
                (true, _) => "Archived",
                (false, true) => "Paused",
                _ => "Open",
            };
            view()
                .child(
                    view()
                        .items_center()
                        .gap(px(8.0))
                        .pb(px(18.0))
                        .child(view().flex_row().items_center().children(a.participants.iter().zip(&names).take(4).enumerate().map(|(k, (p, n))| {
                            view().when(k > 0, |v| v.ml(px(-10.0))).child(crate::chat::peer_avatar(p, n, 44.0)).into_element()
                        })))
                        .child(text(title).text_size(px(17.0)).font_weight(FontWeight::MEDIUM).text_color(theme::TEXT).text_center())
                        .child(text("Agents' conversation · read-only").text_size(px(11.0)).text_color(theme::MUTED)),
                )
                .child(pane_label("Participants".into()))
                .child(pane_card().children(a.participants.iter().zip(&names).map(|(p, n)| participant_row(p.clone(), n.clone()))))
                .child(view().pt(px(14.0)).child(pane_label("Conversation".into())))
                .child(
                    pane_card()
                        .child(fact("Subject", if a.subject.is_empty() { "—".to_owned() } else { a.subject.clone() }))
                        .child(fact("Created", if a.created_at > 0.0 { crate::store::checked_at(a.created_at) } else { "—".to_owned() }))
                        .child(fact("Messages", count.to_string()))
                        .child(fact("Status", status.to_owned()))
                        .child(
                            view()
                                .flex_row()
                                .gap_1()
                                .px(px(4.0))
                                .pb(px(4.0))
                                .when(paused && !archived, |v| v.child(routine_act("Resume", "Resume the conversation".to_owned(), false, move || store().agents_resume(c1.clone()))))
                                .child(routine_act(
                                    if archived { "Unarchive" } else { "Archive" },
                                    format!("{} the conversation", if archived { "Unarchive" } else { "Archive" }),
                                    false,
                                    move || store().agents_archive(c2.clone(), !archived),
                                )),
                        ),
                )
                .into_element()
        }))))
}

/// A participant in a conversation's Details: avatar, name, and "Go to
/// <name> chat" (an outside endpoint has no chat).
fn participant_row(id: String, name: String) -> AnyElement {
    let hov = signal(false);
    let chat = store().bot_thread(&id);
    let go = format!("Go to {name} chat");
    view()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .px(px(10.0))
        .py(px(8.0))
        .child(crate::chat::peer_avatar(&id, &name, 26.0))
        .child(text(name.clone()).text_size(px(13.0)).text_color(theme::TEXT).flex_1().number_of_lines(1))
        .child(if chat.is_empty() {
            text(if crate::agents::is_external(&id) { "Outside" } else { "" }).text_size(px(12.0)).text_color(theme::MUTED).into_element()
        } else {
            view()
                .px(px(10.0))
                .py(px(5.0))
                .rounded(px(8.0))
                .bg(move || if hov.get() { theme::CARD2 } else { theme::MAIN })
                .border_1()
                .border_color(theme::BORDER_F)
                .label(go.clone())
                .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
                .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
                .on_press(move || store().go_to(&chat, None))
                .child(text(go).text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(theme::TEXT))
                .into_element()
        })
        .into_element()
}

/// What the Details pane's routine section shows: in a routine's thread,
/// that routine (`This`); elsewhere the chat's routines (`List`).
#[derive(Debug, Clone, PartialEq)]
enum RoutinesSection {
    This(Box<ThisRoutine>),
    List,
}

/// The open routine thread's own routine, as its Details card shows it.
#[derive(Debug, Clone, PartialEq, Default)]
struct ThisRoutine {
    name: String,
    /// Its plugin, id and instance ("" before instances), for Run now and
    /// Pause.
    plugin: String,
    id: String,
    instance: String,
    /// The plugin that runs it ("" before the plugins are listed).
    by: String,
    /// "Checks every 5 min" ("" unknown).
    schedule: String,
    /// "Last checked 3:42 PM · nothing new", its error, or why it can't.
    status: String,
    failed: bool,
    /// Running on its schedule (Pause), or paused (Resume); false too
    /// before the plugins are listed. `pending`: waiting for the user's
    /// Enable (the agent made it).
    enabled: bool,
    pending: bool,
    /// The chat it reports to, and the action back to it.
    chat: String,
    go: String,
}

fn routines_section(
    open: Option<crate::store::RoutineOf>,
    listed: &[crate::mini_v1::connect::Listed],
    bot: &str,
    checked: impl Fn(f64) -> String,
) -> RoutinesSection {
    let Some(r) = open else { return RoutinesSection::List };
    // the instance's state as a routine's (the routine's own before instances)
    let found = listed.iter().find(|p| p.id == r.plugin).and_then(|p| {
        let x = p.routines.iter().find(|x| x.id == r.id)?;
        let i = x.instances.iter().find(|i| i.instance == r.instance);
        match (r.instance.is_empty(), i) {
            (true, _) => Some((p.title.clone(), x.clone(), false, "schedule".to_owned())),
            (false, Some(i)) => Some((p.title.clone(), i.as_routine(x), i.pending, i.trigger.clone())),
            (false, None) => None,
        }
    });
    let mut out = ThisRoutine { name: r.name, plugin: r.plugin, id: r.id, instance: r.instance, chat: r.thread, go: format!("Go to {bot} chat"), ..Default::default() };
    if let Some((by, x, pending, trigger)) = found {
        let (status, failed) = if pending { ("Waiting for your Enable".to_owned(), false) } else { x.last_check(checked) };
        let mut c = status.chars();
        out.status = c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default();
        out.failed = failed;
        out.by = by;
        out.schedule = if trigger == "webhook" { "Runs on its webhook".to_owned() } else { x.schedule() };
        out.enabled = x.enabled;
        out.pending = pending;
    }
    RoutinesSection::This(Box::new(out))
}

/// A small text action on a routine (Run now, Pause, Open, Delete).
/// A routine row's action: a pressable view, so the agent tree and
/// VoiceOver read it as a button named `aria`.
fn routine_act(label: &'static str, aria: String, danger: bool, on: impl Fn() + 'static) -> AnyElement {
    let hov = signal(false);
    view()
        .px_2()
        .py_1()
        .rounded(px(6.0))
        .bg(move || if hov.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .on_press(on)
        .label(aria)
        .child(text(label).text_size(px(12.0)).text_color(move || match (hov.get(), danger) {
            (true, true) => theme::DOWN,
            (true, false) => theme::TEXT,
            _ => theme::MUTED,
        }))
        .into_element()
}

/// The "This routine" card: name, plugin, schedule, last check (or its
/// Run now's state), Run now and Pause / Resume, and the way to the chat
/// it reports to (as the header's back pill).
fn this_routine(r: ThisRoutine) -> impl IntoElement {
    let hov = signal(false);
    let chat = r.chat.clone();
    let key = crate::routines::v1_key(&r.plugin, if r.instance.is_empty() { &r.id } else { &r.instance });
    let usual = (r.status.clone(), r.failed);
    let line = |t: String, c| text(t).text_size(px(12.0)).text_color(c).number_of_lines(3);
    let (p1, i1, n1) = (r.plugin.clone(), r.id.clone(), r.instance.clone());
    let (p2, i2, n2, c2) = (r.plugin.clone(), r.id.clone(), r.instance.clone(), r.chat.clone());
    let known = !r.by.is_empty();
    let (enabled, pending) = (r.enabled, r.pending);
    pane_card().child(
        view()
            .gap(px(2.0))
            .px(px(10.0))
            .py(px(8.0))
            .child(text(r.name.clone()).text_size(px(13.0)).text_color(theme::TEXT).number_of_lines(2))
            .when(!r.by.is_empty(), |v| v.child(line(r.by, theme::MUTED)))
            .when(!r.schedule.is_empty(), |v| v.child(line(r.schedule, theme::DIM)))
            .child(dynamic(move || {
                let (t, failed) = crate::routines::row_status(crate::routines::run_of(&key).as_ref(), usual.clone());
                if t.is_empty() {
                    return view().into_element();
                }
                line(t, if failed { theme::DOWN } else { theme::DIM }).into_element()
            }))
            .when(known, |v| {
                v.child(
                    view()
                        .flex_row()
                        .pt(px(4.0))
                        .gap_1()
                        .when(enabled, |v| {
                            v.child(routine_act("Run now", format!("Run {} now", r.name), false, move || store().v1_routine_run(&p1, &i1, &n1)))
                        })
                        .child(pause_or_resume(&r.name, enabled, pending, p2, i2, n2, c2)),
                )
            })
            .child(
                view()
                    .flex_row()
                    .pt(px(8.0))
                    .child(
                        view()
                            .px(px(10.0))
                            .py(px(5.0))
                            .rounded(px(8.0))
                            .bg(move || if hov.get() { theme::CARD2 } else { theme::MAIN })
                            .border_1()
                            .border_color(theme::BORDER_F)
                            .label(r.go.clone())
                            .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
                            .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
                            .on_press(move || store().go_to(&chat, None))
                            .child(text(r.go).text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(theme::TEXT)),
                    ),
            ),
    )
}

/// The chat's routines, or how to get one.
fn routines_list() -> impl IntoElement {
    dynamic(|| {
        let st = store();
        let routines = chat_routines(&st.active_id.get(), &st.v1.listed.get(), &st.routines.get(), crate::app_name());
        if routines.is_empty() {
            let name = store().active_bot().map(|b| b.name).unwrap_or_else(|| crate::app_name().to_owned());
            return pane_card()
                .py(px(14.0))
                .px(px(13.0))
                .child(
                    text(format!("No routines yet. Ask {name} in chat to run something on a schedule."))
                        .text_size(px(13.0))
                        .text_color(theme::DIM),
                )
                .into_element();
        }
        pane_card().children(routines.into_iter().map(routine_row)).into_element()
    })
}

/// A routine that reports to the open chat: its name, who runs it, how
/// its last run went, whether it runs on its schedule, and which kind.
#[derive(Debug, Clone, PartialEq)]
struct ChatRoutine {
    name: String,
    by: String,
    status: String,
    failed: bool,
    enabled: bool,
    kind: RoutineKind,
}

#[derive(Debug, Clone, PartialEq)]
enum RoutineKind {
    /// A v1 plugin routine's instance: its plugin, routine id, instance
    /// ("" before instances), own thread ("" not made yet), the chat it
    /// reports to, trigger, whether its webhook has a URL, whether it waits
    /// for the user's Enable, and its params as JSON ("" when the routine
    /// takes none).
    Plugin { plugin: String, id: String, instance: String, home: String, chat: String, trigger: String, hook: bool, pending: bool, params: String },
    /// The server's own (`/routines`): its turns post in this chat.
    Server { id: String },
}

impl ChatRoutine {
    /// The key of its Run now (`routines::run_of`).
    fn key(&self) -> String {
        match &self.kind {
            RoutineKind::Plugin { plugin, id, instance, .. } => crate::routines::v1_key(plugin, if instance.is_empty() { id } else { instance }),
            RoutineKind::Server { id } => crate::routines::v0_key(id),
        }
    }
}

/// The routines, v1 plugins' instances and the server's own, that report
/// to `chat`, paused and pending ones too (they resume or start here).
fn chat_routines(chat: &str, listed: &[crate::mini_v1::connect::Listed], server: &[crate::routines::Routine], app: &str) -> Vec<ChatRoutine> {
    let status = |enabled: bool, last_run: f64, last_error: &str| match (enabled, last_error.is_empty(), last_run > 0.0) {
        (false, _, _) => ("Paused".to_owned(), false),
        (true, false, _) => (format!("Last run failed: {last_error}"), true),
        (true, true, true) => (format!("Last ran {} ago", Store::ago(last_run)), false),
        (true, true, false) => ("Waiting for its first run".to_owned(), false),
    };
    let mut out = Vec::new();
    for p in listed {
        for r in &p.routines {
            for i in r.instances_or_self().into_iter().filter(|i| !chat.is_empty() && i.thread == chat) {
                let (status, failed) = if i.pending {
                    ("Waiting for your Enable".to_owned(), false)
                } else if i.available || !i.enabled {
                    status(i.enabled, i.last_run, &i.last_error)
                } else {
                    (i.reason.clone(), true)
                };
                let how = if i.trigger == "webhook" {
                    "on its webhook".to_owned()
                } else {
                    crate::routines::interval(if i.every_s > 0.0 { i.every_s } else { r.every_s })
                };
                out.push(ChatRoutine {
                    name: if i.summary.is_empty() { r.name.clone() } else { format!("{} · {}", r.name, i.summary) },
                    by: format!("{} · {how}", p.title),
                    status,
                    failed,
                    enabled: i.enabled,
                    kind: RoutineKind::Plugin {
                        plugin: p.id.clone(),
                        id: r.id.clone(),
                        instance: i.instance.clone(),
                        home: i.home.clone(),
                        chat: i.thread.clone(),
                        trigger: i.trigger.clone(),
                        hook: i.hook,
                        pending: i.pending,
                        params: if r.params_schema.is_null() { String::new() } else { i.params.to_string() },
                    },
                });
            }
        }
    }
    for r in server.iter().filter(|r| !chat.is_empty() && r.thread == chat) {
        let (status, failed) = status(r.enabled, r.last_run, &r.last_error);
        let by = if r.bot_name.is_empty() { app.to_owned() } else { r.bot_name.clone() };
        out.push(ChatRoutine { name: r.name.clone(), by, status, failed, enabled: r.enabled, kind: RoutineKind::Server { id: r.id.clone() } });
    }
    out
}

/// One routine in the Details card, with its actions: Run now (while it
/// runs on its schedule), Pause or Resume; a v1 plugin's Open (its own
/// thread, made once it is enabled; its plugin page only when the broker
/// names none), the server's own Delete.
fn routine_row(r: ChatRoutine) -> AnyElement {
    let key = r.key();
    let usual = (r.status.clone(), r.failed);
    let name = r.name.clone();
    let mut acts = view().flex_row().flex_wrap().gap_1().pt(px(2.0));
    // a plugin instance's params editor and webhook URL, under its actions
    let mut extra: Option<String> = None;
    match r.kind.clone() {
        RoutineKind::Plugin { plugin, id, instance, home, chat, trigger, hook, pending, params } => {
            let (p1, i1, n1) = (plugin.clone(), id.clone(), instance.clone());
            if r.enabled {
                acts = acts.child(routine_act("Run now", format!("Run {name} now"), false, move || store().v1_routine_run(&p1, &i1, &n1)));
            }
            acts = acts.child(pause_or_resume(&name, r.enabled, pending, plugin.clone(), id.clone(), instance.clone(), chat));
            if !params.is_empty() && !instance.is_empty() {
                let (n2, text) = (instance.clone(), params.clone());
                acts = acts.child(routine_act("Edit params", format!("Edit {name}'s params"), false, move || {
                    store().routine_edit.set(Some((n2.clone(), text.clone())))
                }));
            }
            if trigger == "webhook" && !instance.is_empty() {
                let (p3, i3, n3) = (plugin.clone(), id.clone(), instance.clone());
                acts = acts.child(routine_act(if hook { "New URL" } else { "Get URL" }, format!("Webhook URL of {name}"), false, move || {
                    store().v1_instance_hook(&p3, &i3, &n3)
                }));
            }
            let (p4, i4) = (plugin.clone(), id.clone());
            acts = acts.child(routine_act("Open", format!("Open the {name} thread"), false, move || {
                match Some(home.clone()).filter(|h| !h.is_empty()).or_else(|| store().routine_thread(&p4, &i4)) {
                    Some(thread) => store().go_to(&thread, None),
                    None => {
                        store().open_plugins();
                        store().push_nav("v1plugin", &p4, "");
                    }
                }
            }));
            if !instance.is_empty() {
                let n5 = instance.clone();
                acts = acts.child(routine_act("Delete", format!("Delete {name}"), true, move || {
                    store().v1_instance(&plugin, &id, &n5, crate::mini_v1::host_api::InstanceAction::Delete, serde_json::json!({}))
                }));
                extra = Some(instance);
            }
        }
        RoutineKind::Server { id } => {
            let (i1, i2, i3) = (id.clone(), id.clone(), id);
            let on = r.enabled;
            if on {
                acts = acts.child(routine_act("Run now", format!("Run {name} now"), false, move || crate::routines::run_v0(i1.clone())));
            }
            acts = acts
                .child(routine_act(if on { "Pause" } else { "Resume" }, format!("{} {name}", if on { "Pause" } else { "Resume" }), false, move || {
                    crate::routines::set_routine_enabled(i2.clone(), !on)
                }))
                .child(routine_act("Delete", format!("Delete {name}"), true, move || crate::routines::delete_routine(i3.clone())));
        }
    }
    view()
        .gap(px(2.0))
        .px(px(10.0))
        .py(px(8.0))
        .rounded(px(8.0))
        .label(r.name.clone())
        .child(text(r.name).text_size(px(13.0)).text_color(theme::TEXT).number_of_lines(1))
        .child(text(r.by).text_size(px(12.0)).text_color(theme::MUTED).number_of_lines(1))
        .child(dynamic(move || {
            let (t, failed) = crate::routines::row_status(crate::routines::run_of(&key).as_ref(), usual.clone());
            text(t).text_size(px(12.0)).text_color(if failed { theme::DOWN } else { theme::DIM }).number_of_lines(2).into_element()
        }))
        .child(acts)
        .when_some(extra, |v, instance| v.child(instance_extras(instance)))
        .into_element()
}

/// Pause or Resume (Enable for one waiting for the user's consent: the
/// card's Enable, from here). Resume re-enables it in the chat it reports
/// to (`chat`).
fn pause_or_resume(name: &str, enabled: bool, pending: bool, plugin: String, id: String, instance: String, chat: String) -> AnyElement {
    use crate::mini_v1::host_api::InstanceAction;
    if enabled {
        routine_act("Pause", format!("Pause {name}"), false, move || {
            store().v1_instance(&plugin, &id, &instance, InstanceAction::Disable, serde_json::json!({}))
        })
    } else if pending {
        routine_act("Enable", format!("Enable {name}"), false, move || {
            store().routine_consented.update(|l| l.push((instance.clone(), "enabled".to_owned())));
            store().v1_instance(&plugin, &id, &instance, InstanceAction::Enable, serde_json::json!({ "approve": true }))
        })
    } else {
        routine_act("Resume", format!("Resume {name}"), false, move || {
            store().v1_instance(&plugin, &id, &instance, InstanceAction::Enable, serde_json::json!({ "thread": chat }))
        })
    }
}

/// Under a plugin instance's row: its params editor while open (JSON,
/// Save checks it), and its webhook URL right after it was made (Copy).
fn instance_extras(instance: String) -> impl IntoElement {
    dynamic(move || {
        let st = store();
        let editing = st.routine_edit.get().filter(|(i, _)| *i == instance);
        let url = st.routine_hook.get().filter(|(i, _)| *i == instance).map(|(_, u)| u);
        let mut v = view().gap(px(6.0));
        if editing.is_some() {
            let inst = instance.clone();
            v = v.child(
                view()
                    .gap(px(4.0))
                    .pt(px(4.0))
                    .child(
                        input()
                            .auto_focus(true)
                            .value(move || store().routine_edit.get().map(|(_, t)| t).unwrap_or_default())
                            .placeholder("{\"taxpayers\": [...]}")
                            .placeholder_color(|| color(theme::GHOST))
                            .text_size(px(12.0))
                            .text_color(theme::TEXT)
                            .on_change(|t: &str| store().routine_edit.update(|e| if let Some((_, x)) = e { *x = t.to_owned() }))
                            .h(px(30.0))
                            .px_2()
                            .rounded(px(7.0))
                            .bg(theme::HOVER),
                    )
                    .child(
                        view()
                            .flex_row()
                            .gap_1()
                            .child(routine_act("Save", "Save params".to_owned(), false, move || save_params(&inst)))
                            .child(routine_act("Cancel", "Cancel editing params".to_owned(), false, || store().routine_edit.set(None))),
                    ),
            );
        }
        if let Some(url) = url {
            let copy = url.clone();
            v = v.child(
                view()
                    .gap(px(2.0))
                    .child(text("Webhook URL (shown once; New URL retires it):").text_size(px(12.0)).text_color(theme::DIM))
                    .child(text(url).text_size(px(12.0)).text_color(theme::TEXT).number_of_lines(3))
                    .child(view().flex_row().gap_1().child(routine_act("Copy", "Copy webhook URL".to_owned(), false, move || {
                        store().copy_text(&copy);
                        crate::toast::show("Copied the webhook URL");
                    })).child(routine_act("Hide", "Hide webhook URL".to_owned(), false, || store().routine_hook.set(None)))),
            );
        }
        v.into_element()
    })
}

/// Save on the params editor: the typed JSON, when it is an object, to
/// the broker (`update`), which checks it against the routine's schema.
fn save_params(instance: &str) {
    let st = store();
    let Some((_, text)) = st.routine_edit.get_untracked() else { return };
    let params = match crate::routines::parse_params(&text) {
        Ok(p) => p,
        Err(e) => return crate::toast::show(e),
    };
    let found = st.v1.listed.with_untracked(|l| {
        l.iter().find_map(|p| {
            p.routines.iter().find_map(|r| r.instances.iter().any(|i| i.instance == instance).then(|| (p.id.clone(), r.id.clone())))
        })
    });
    let Some((plugin, id)) = found else { return };
    st.routine_edit.set(None);
    st.v1_instance(&plugin, &id, instance, crate::mini_v1::host_api::InstanceAction::Update, serde_json::json!({ "params": params }));
}

/// The bot's context in Details: its description, and a row to its Bot
/// Details (instructions, memories, skills, integrations live there).
fn bot_context(b: crate::store::RoxBot) -> impl IntoElement {
    let id = b.id.clone();
    let hov = signal(false);
    view()
        .when(!b.description.trim().is_empty(), |v| {
            // In a full-width box so the text wraps at the pane's edge.
            v.child(
                view().w_full().px_3().pb(px(14.0)).child(
                    text(b.description.clone())
                        .text_size(px(13.0))
                        .line_height(px(19.0))
                        .text_color(theme::DIM),
                ),
            )
        })
        .child(
            view()
                .flex_row()
                .items_center()
                .mx_1()
                .mb(px(14.0))
                .px_2()
                .py(px(8.0))
                .rounded(px(8.0))
                .bg(move || if hov.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
                .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
                .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
                .child(text("Context and integrations").text_size(px(13.0)).text_color(theme::TEXT).flex_1())
                .child(text("›").text_size(px(15.0)).text_color(theme::MUTED))
                .on_press(move || crate::botdetails::open(&id, false)),
        )
}

/// Library — `libraryGroups()` + version trees (App.tsx:2637).
fn library_tab() -> impl IntoElement {
    view()
        .child(when(
            || store().thread_apps.get().is_empty(),
            || {
                view()
                    .child(pane_label("Library".into()))
                    .child(
                        pane_card()
                            .py(px(14.0))
                            .px(px(13.0))
                            .child(
                                text(move || {
                                    let name = store()
                                        .active_bot()
                                        .map(|b| b.name)
                                        .unwrap_or_default();
                                    // a plain chat has no bot to name
                                    if name.is_empty() {
                                        "Files made in this chat show up here.".to_owned()
                                    } else {
                                        format!("Files {name} makes show up here.")
                                    }
                                })
                                .text_size(px(13.0))
                                .text_color(theme::DIM),
                            ),
                    )
                    .into_element()
            },
        ))
        .child(when(|| !store().thread_apps.get().is_empty(), || lib_chips().into_element()))
        .child(each(
            || store().library_groups(),
            |g| g.0.clone(),
            |g, _| {
                // The group's rows come from a second `each` on the live
                // signal: the outer key is the label, so an agent switch
                // that lands in the same buckets ("Older" → "Older")
                // would keep the first agent's `items` forever.
                let label = g.get_untracked().0;
                view()
                    .pb(px(22.0))
                    .child(pane_label(label))
                    .child(
                        pane_card().child(each(
                            move || g.get().1,
                            |a| a.app.clone(),
                            |a, _| app_row(a).into_element(),
                        )),
                    )
                    .into_element()
            },
        ))
}

/// The kind chips over the Library: All, then each kind the chat has
/// made, with its count; the pick holds for the session (`lib_filter`).
fn lib_chips() -> impl IntoElement {
    view()
        .flex_row()
        .flex_wrap()
        .gap(px(6.0))
        .px_1()
        .pb(px(14.0))
        .child(dynamic(|| {
            let cur = store().lib_kind();
            // Full width, so the chips wrap inside the pane instead of
            // running past its edge.
            view()
                .w_full()
                .flex_row()
                .flex_wrap()
                .gap(px(6.0))
                .children(store().lib_chips().into_iter().map(|(kind, label, n)| {
                    lib_chip(kind, label, n, cur == kind).into_element()
                }))
                .into_element()
        }))
}

fn lib_chip(kind: &'static str, label: &'static str, n: usize, on: bool) -> impl IntoElement {
    let hov = signal(false);
    view()
        .flex_row()
        .items_center()
        .gap(px(5.0))
        .h(px(26.0))
        .px(px(10.0))
        .rounded(px(13.0))
        .bg(move || {
            if on {
                theme::ACTIVE
            } else if hov.get() {
                theme::HOVER
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .border_1()
        .border_color(if on { AdaptiveColor::TRANSPARENT } else { theme::BORDER_F })
        .child(
            text(label)
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if on { theme::TEXT } else { theme::DIM }),
        )
        .child(text(n.to_string()).text_size(px(12.0)).text_color(theme::MUTED))
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .on_press(move || store().lib_filter.set(kind.to_owned()))
        .label(format!("Show {label}"))
}

/// A library row — the icon tile, title/file, the version picker chip,
/// `ago` (App.tsx:2648).
fn app_row(a: ReadSignal<RoxApp>) -> impl IntoElement {
    let a0 = a.get();
    let n_chip = a0.app.clone();
    let a_ctx = a;
    let focus = a0.app.clone();
    let press = a;
    view()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(7.0))
                .py(px(6.0))
                .px_2()
                .rounded(px(8.0))
                // A card's ⋯ → Details lands here (`library_focus`).
                .bg(move || {
                    if store().library_focus.get() == focus {
                        theme::ACTIVE
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                // Each kind opens with its own handler: a mini app pops
                // out, an image opens in the lightbox, a file in the viewer.
                .on_press(move || store().open_library_item(&press.get_untracked()))
                .on_context_menu(move |e: &ContextMenuEvent| {
                    let at = (e.page_position.x.0, e.page_position.y.0);
                    crate::ctx::open(e, crate::ctx::app_items(&a_ctx.get(), at))
                })
                // The square tile: an app's icon, an image's thumbnail, a
                // file's set icon or its type tile.
                .child({
                    dynamic(move || crate::appdetails::artifact_tile(&a.get(), 28.0).into_element())
                })
                .child(
                    view()
                        .flex_1()
                        .child(
                            text({
                                move || a.get().title
                            })
                            .text_size(px(13.0))
                            .text_color(theme::TEXT)
                            .number_of_lines(1),
                        )
                        .child(
                            text({
                                move || subtitle(&a.get())
                            })
                            .text_size(px(12.0))
                            .text_color(theme::MUTED)
                            .number_of_lines(1),
                        ),
                )
                .child(when(
                    {
                        move || a.get().v > 0
                    },
                    move || {
                        let a2 = a;
                        crate::versions::version_chip(n_chip.clone(), move || {
                            let a = a2.get();
                            (a.v, a.versions)
                        })
                        .into_element()
                    },
                ))
                .child(
                    text(move || Store::ago(a.get().mtime))
                        .text_size(px(13.0))
                        .text_color(theme::MUTED),
                ),
        )
}

/// A library row's second line: the kind and what is known of it —
/// "Mini app · 360×480", "Image · 1024×1024", "PDF · 2 pages",
/// "Sheet · 3 sheets", "Document · 12 KB"; a connector's file its type
/// and where it lives — "Google Doc · Google Drive".
pub(crate) fn subtitle(a: &RoxApp) -> String {
    // an attached server app: what the server says of it (a TIN, a branch)
    if a.kind == "mcp-app" {
        return if a.details.subtitle.is_empty() { "Attached app".to_owned() } else { a.details.subtitle.clone() };
    }
    let m = &a.meta;
    if a.storage.connector() {
        let kind = if m.file_type.is_empty() { "File" } else { m.file_type.as_str() };
        return format!("{kind} · {}", a.storage.place_name());
    }
    let plural = |n: i64, one: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") };
    let size = || (m.size > 0.0).then(|| crate::files::fmt_size(m.size));
    let ext = || {
        let e = crate::files::ext_of(&a.filename, &m.ext);
        if e.is_empty() { "File".to_owned() } else { e.to_uppercase() }
    };
    let (label, detail) = match a.kind.as_str() {
        "app" => ("Mini app".to_owned(), (a.w > 0.0).then(|| format!("{}×{}", a.w, a.h))),
        "image" => ("Image".to_owned(), (a.w > 0.0).then(|| format!("{}×{}", a.w, a.h))),
        "pdf" => ("PDF".to_owned(), (m.pages > 0).then(|| plural(m.pages, "page")).or_else(size)),
        "spreadsheet" => (
            "Sheet".to_owned(),
            (!m.sheets.is_empty()).then(|| plural(m.sheets.len() as i64, "sheet")).or_else(size),
        ),
        "presentation" => ("Slides".to_owned(), (m.slides > 0).then(|| plural(m.slides, "slide")).or_else(size)),
        "document" => ("Document".to_owned(), (m.pages > 0).then(|| plural(m.pages, "page")).or_else(size)),
        "media" => (
            if a.attach.as_ref().is_some_and(|f| f.kind == "audio") { "Audio" } else { "Video" }.to_owned(),
            (m.duration > 0.0).then(|| clock(m.duration)).or_else(size),
        ),
        _ => (ext(), size()),
    };
    match detail {
        Some(d) => format!("{label} · {d}"),
        None => label,
    }
}

/// A media length as m:ss.
fn clock(secs: f64) -> String {
    let s = secs.round() as i64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Computer — the bot's computer: status, Start, Update, Reset and the
/// network policy, with its screen while it runs (`computer.rs`).
fn computer_tab() -> impl IntoElement {
    crate::computer::pane()
}

/// `detail() && !activeBot()` — the app detail: tile, title, pills,
/// kvs, actions (App.tsx:2845).
fn app_detail() -> impl IntoElement {
    view()
        .flex_1()
        .items_center()
        .px(px(14.0))
        .child(
            view()
                .size(px(64.0))
                .rounded(px(20.0))
                .items_center()
                .justify_center()
                .bg(move || {
                    store()
                        .detail_meta
                        .get()
                        .map(|m| hue(&m.name).dark)
                        .unwrap_or(theme::CARD2.dark)
                })
                .child(
                    text(move || {
                        store()
                            .detail_meta
                            .get()
                            .map(|m| {
                                m.title
                                    .chars()
                                    .next()
                                    .map(|c| c.to_uppercase().collect::<String>())
                                    .unwrap_or_else(|| "?".into())
                            })
                            .unwrap_or_else(|| "?".into())
                    })
                    .text_size(px(26.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(white()),
                ),
        )
        .child(
            text(move || {
                store().detail_meta.get().map(|m| m.title).unwrap_or_default()
            })
            .text_size(px(15.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::TEXT)
            .pt(px(10.0)),
        )
        .child(
            text(move || {
                store()
                    .detail_meta
                    .get()
                    .map(|m| format!("app · {}.tsx", m.name))
                    .unwrap_or_default()
            })
            .text_size(px(11.0))
            .text_color(theme::GHOST),
        )
        .child(detail_pills().mt_1().into_element())
        .child(
            scroll()
                .flex_1()
                .child(
                    view()
                        .pt_3()
                        .child(dynamic(|| detail_variant_body().into_element())),
                ),
        )
        .child(app_actions().pb_1().into_element())
}

/// The Details|State segmented pills (App.tsx:2872).
fn detail_pills() -> View {
    view()
        .flex_row()
        .bg(theme::HOVER)
        .rounded(px(999.0))
        .p(px(3.0))
        .children(["details", "state"].into_iter().map(|tb| {
            view()
                .rounded(px(999.0))
                .px_4()
                .py_1()
                .bg(move || {
                    if store().detail_tab.get() == tb {
                        theme::ACTIVE
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .child(
                    text(if tb == "details" { "Details" } else { "State" })
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || {
                            if store().detail_tab.get() == tb {
                                theme::TEXT
                            } else {
                                theme::MUTED
                            }
                        }),
                )
                .on_press(move || store().detail_tab.set(tb.to_owned()))
                .into_element()
        }))
}

/// Variant B's scroll body — meta rows or the state kvs.
fn detail_variant_body() -> impl IntoElement {
    if store().detail_tab.get() == "state" {
        view()
            .gap_2()
            .child(when(
                || store().detail.get().is_empty(),
                || {
                    text("no persisted state")
                        .text_size(px(12.0))
                        .text_color(theme::GHOST)
                        .into_element()
                },
            ))
            .child(each(
                || store().detail.get(),
                |kv| kv.0.clone(),
                |kv, _| state_row(kv.get()),
            ))
            .into_element()
    } else {
        view()
            .gap_2()
            .child(meta_row("module", |m| m.name.clone()))
            .child(meta_row("status", |m| {
                if m.published { "in Mini Apps".into() } else { "chat only".into() }
            }))
            .child(when(
                || !store().detail_meta.get().map(|m| m.kinds.is_empty()).unwrap_or(true),
                || meta_row("kinds", |m| m.kinds.join(" · ")).into_element(),
            ))
            .child(when(
                || !store().detail_meta.get().map(|m| m.tags.is_empty()).unwrap_or(true),
                || {
                    meta_row("tags", |m| {
                        m.tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" ")
                    })
                    .into_element()
                },
            ))
            .into_element()
    }
}

fn meta_row(label: &'static str, value: impl Fn(&AppMeta) -> String + 'static) -> impl IntoElement {
    view()
        .flex_row()
        .justify_between()
        .child(text(label).text_size(px(12.0)).text_color(theme::MUTED))
        .child(
            text(move || {
                store()
                    .detail_meta
                    .get()
                    .map(|m| value(&m))
                    .unwrap_or_default()
            })
            .text_size(px(12.0))
            .text_color(theme::TEXT),
        )
}

fn state_row(kv: crate::store::Kv) -> impl IntoElement {
    view()
        .flex_row()
        .justify_between()
        .gap_2()
        .child(
            text(kv.0)
                .text_size(px(12.0))
                .text_color(theme::MUTED),
        )
        .child(
            text(kv.1)
                .text_size(px(12.0))
                .font_family("Menlo")
                .text_color(theme::TEXT)
                .number_of_lines(2),
        )
}

/// The action buttons — `Open window`, `Save to Mini Apps`
/// (App.tsx:2963).
fn app_actions() -> View {
    view()
        .gap_2()
        .child(
            action_btn("Open window", false, || {
                let Some(m) = store().detail_meta.get_untracked() else { return };
                store().popout(&m.name);
            }),
        )
        .child(when(
            || !store().detail_meta.get().map(|m| m.published).unwrap_or(true),
            || {
                action_btn("Save to Mini Apps", true, || {
                    let Some(m) = store().detail_meta.get_untracked() else { return };
                    let st = store();
                    spawn(async move {
                        let _ = client_post("/publish", serde_json::json!({"name": m.name})).await;
                        st.refresh_threads();
                    });
                })
                .into_element()
            },
        ))
}

fn action_btn(label: &'static str, accent: bool, on_press: impl Fn() + 'static) -> impl IntoElement {
    view()
        .rounded(px(10.0))
        .bg(if accent {
            Hsla::from_str("#1884fc").unwrap_or(theme::ACCENT.dark)
        } else {
            theme::HOVER.dark
        })
        .py(px(9.0))
        .items_center()
        .child(
            text(label)
                .text_size(px(13.0))
                .font_weight(if accent { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                .text_color(if accent { white() } else { color(theme::TEXT) }),
        )
        .on_press(on_press)
}

/// The empty pane — no bot and no app yet (App.tsx:2835).
fn empty_pane() -> impl IntoElement {
    view()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_2()
        .px(px(14.0))
        .child(
            text("No app in this conversation yet")
                .text_size(px(13.0))
                .text_color(theme::GHOST),
        )
        .child(
            text("generated apps show their details here")
                .text_size(px(11.0))
                .text_color(theme::GHOST),
        )
}

// ————————————————————————— the avatar editor —————————————————————————

/// `avatarEditing`'s floating card under the pane avatar — 297 wide,
/// Bot | Generate | Upload tabs (App.tsx:3163). Rendered from `pane()`.
pub(crate) fn editor_overlay() -> impl IntoElement {
    dynamic(|| {
        let st = store();
        if !(st.avatar_editing.get() && st.right_open.get() && st.active_bot().is_some()) {
            return view().into_element();
        }
        view()
            .size_full()
            .absolute()
            // The outside-tap scrim — `onPress={closeAvatarEditor}`.
            .child(
                view()
                    .size_full()
                    .on_press(|| store().close_avatar_editor()),
            )
            .child(
                view()
                    .absolute()
                    .right(px(11.0))
                    .top(px(TOP + 8.0 + 64.0 + 6.0))
                    .w(px(297.0))
                    .rounded(px(16.0))
                    .bg(theme::EDITOR)
                    .border_1()
                    .border_color(theme::BORDER_F)
                    .child(editor_tabs())
                    .child(dynamic(|| editor_body().into_element())),
            )
            .into_element()
    })
}

/// The Bot|Generate|Upload pills + Reset (App.tsx:3181).
fn editor_tabs() -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .pt(px(9.0))
        .px_2()
        .gap(px(2.0))
        .children(["bot", "generate", "upload"].into_iter().map(|tb| {
            view()
                .rounded(px(999.0))
                .px_2()
                .py_1()
                .bg(move || {
                    if store().avatar_edit_tab.get() == tb {
                        theme::SUBTLE
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .child(
                    text({
                        let mut c = tb.chars();
                        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
                    })
                    .text_size(px(13.0))
                    .text_color(move || {
                        if store().avatar_edit_tab.get() == tb {
                            theme::TEXT
                        } else {
                            theme::DIM
                        }
                    }),
                )
                .on_press(move || {
                    let st = store();
                    st.avatar_edit_tab.set(tb.to_owned());
                    st.avatar_err.set(String::new());
                })
                .into_element()
        }))
        .child(view().flex_1())
        .child(when(
            || store().avatar_edit_tab.get() == "bot",
            || {
                view()
                    .h(px(22.0))
                    .px_2()
                    .rounded(px(8.0))
                    .justify_center()
                    .label("Reset avatar")
                    .child(text("Reset").text_size(px(13.0)).text_color(theme::DIM))
                    .on_press(|| {
                        let Some(b) = store().active_bot_untracked() else { return };
                        store().update_bot(
                            &b.id,
                            serde_json::json!({"shape": "blob", "color": "#FFFFFF", "image": "", "crop": serde_json::Value::Null}),
                        );
                    })
                    .into_element()
            },
        ))
}

/// The editor's body — the bot grid, the generate prompt, or the upload
/// zone (App.tsx:3214).
fn editor_body() -> impl IntoElement {
    match store().avatar_edit_tab.get().as_str() {
        "generate" => generate_tab().into_element(),
        "upload" => upload_tab().into_element(),
        _ => bot_tab().into_element(),
    }
}

/// `Bot` — 48pt shapes five per row, 32pt swatches six per row
/// (App.tsx:3215).
fn bot_tab() -> impl IntoElement {
    let st = store();
    view()
        .pt(px(26.0))
        .px_3()
        .pb(px(20.0))
        .gap(px(26.0))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .justify_center()
                .gap_2()
                .children(crate::mark::BOT_SHAPES.iter().map(|sh| {
                    let sh = *sh;
                    view()
                        .size(px(48.0))
                        .items_center()
                        .justify_center()
                        .label(format!("{sh} character shape"))
                        .on_press(move || {
                            let Some(b) = st.active_bot_untracked() else { return };
                            st.update_bot(
                                &b.id,
                                serde_json::json!({"shape": sh, "image": "", "crop": serde_json::Value::Null}),
                            );
                        })
                        .child(when(
                            move || {
                                store()
                                    .active_bot()
                                    .map(|b| b.image.is_empty() && b.shape == sh)
                                    .unwrap_or(false)
                            },
                            || {
                                view()
                                    .absolute()
                                    .size(px(46.0))
                                    .rounded(px(23.0))
                                    .border_1()
                                    .border_color(theme::BORDER_S)
                                    .into_element()
                            },
                        ))
                        // App.tsx:3248 — the adaptive swatch raw; the
                        // painter resolves `inkOf` per theme.
                        .child(crate::avatar::bot_blob(crate::avatar::BotBlobProps {
                            shape: sh.into(),
                            color: "#FFFFFF".to_owned(),
                            size: 44.0,
                            paused: true,
                            ..Default::default()
                        }))
                        .into_element()
                })),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .justify_center()
                .gap_2()
                .px_5()
                .children(crate::mark::BOT_COLORS.iter().map(|c| {
                    let c = *c;
                    view()
                        .size(px(32.0))
                        .rounded(px(16.0))
                        .items_center()
                        .justify_center()
                        .border_1()
                        .border_color(move || {
                            let same = store()
                                .active_bot()
                                .map(|b| {
                                    b.image.is_empty()
                                        && crate::mark::norm_color(&b.color) == crate::mark::norm_color(c)
                                })
                                .unwrap_or(false);
                            if same { theme::BORDER_S } else { AdaptiveColor::TRANSPARENT }
                        })
                        .label(format!("{c} character color"))
                        .on_press(move || {
                            let Some(b) = store().active_bot_untracked() else { return };
                            store().update_bot(
                                &b.id,
                                serde_json::json!({"color": c, "image": "", "crop": serde_json::Value::Null}),
                            );
                        })
                        .child(
                            view()
                                .size(px(24.0))
                                .rounded(px(12.0))
                                // `inkOf(c())` on a style binding — follows
                                // the env memo when the theme flips.
                                .bg(move || {
                                    Hsla::from_str(&crate::mark::ink_of(c, !dark()))
                                        .unwrap_or(theme::TEXT.dark)
                                }),
                        )
                        .into_element()
                })),
        )
}

/// `Generate` — the prompt, the pulsing placeholder, or the crop
/// (App.tsx:3261).
fn generate_tab() -> impl IntoElement {
    view()
        .pt(px(20.0))
        .px_3()
        .pb(px(13.0))
        .gap_3()
        .child(when(
            || store().avatar_draft.get().is_none(),
            || {
                view()
                    .gap_3()
                    .child(
                        input()
                            .value(|| store().avatar_gen_prompt.get())
                            .on_change(|v: &str| store().avatar_gen_prompt.set(v.to_owned()))
                            .placeholder("Describe your avatar…")
                            .placeholder_color(|| color(theme::GHOST))
                            .multiline(|| !store().avatar_gen_busy.get())
                            .readonly(|| store().avatar_gen_busy.get())
                            .h(move || px(if store().avatar_gen_busy.get() { 32.0 } else { 174.0 }))
                            .text_size(px(14.0))
                            .text_color(theme::TEXT)
                            .bg(theme::CARD2)
                            .py_2()
                            .px(px(10.0))
                            .rounded(px(10.0))
                            .border_1()
                            .border_color(theme::BORDER_S),
                    )
                    .child(when(
                        || store().avatar_gen_busy.get(),
                        || {
                            view()
                                .items_center()
                                .py_3()
                                .child(
                                    view()
                                        .size(px(84.0))
                                        .rounded(px(42.0))
                                        .bg(Hsla::from_str("#2a2a2a").unwrap_or(theme::CARD2.dark)),
                                )
                                .into_element()
                        },
                    ))
                    .child(
                        view()
                            .h(px(30.0))
                            .rounded(px(10.0))
                            .items_center()
                            .justify_center()
                            .bg(move || {
                                if store().avatar_gen_busy.get() {
                                    theme::HOVER.dark
                                } else if !store().avatar_gen_prompt.get().trim().is_empty() {
                                    theme::TEXT.dark
                                } else {
                                    theme::GHOST.dark
                                }
                            })
                            .child(
                                text(move || {
                                    if store().avatar_gen_busy.get() { "Generating…" } else { "Generate" }
                                })
                                .text_size(px(13.0))
                                .text_color(move || {
                                    if store().avatar_gen_busy.get() {
                                        theme::MUTED.dark
                                    } else {
                                        theme::CARD2.dark
                                    }
                                }),
                            )
                            .on_press(|| store().generate_avatar()),
                    )
                    .into_element()
            },
        ))
        .child(when(
            || store().avatar_draft.get().is_some(),
            || {
                let d = store().avatar_draft.get().unwrap_or_default();
                crop_editor(d, false).into_element()
            },
        ))
        .child(editor_err())
}

/// `Upload` — the dashed drop zone, then the crop with name·size
/// (App.tsx:3328).
fn upload_tab() -> impl IntoElement {
    view()
        .pt(px(20.0))
        .px_3()
        .pb(px(13.0))
        .gap_3()
        .child(when(
            || store().avatar_draft.get().is_none(),
            || {
                view()
                    .h(px(216.0))
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(theme::BORDER_S)
                    .items_center()
                    .justify_center()
                    .px_3()
                    .child(
                        text("Drag, drop, or paste an image")
                            .text_size(px(14.0))
                            .text_color(theme::DIM),
                    )
                    .child(text("or").text_size(px(14.0)).text_color(theme::MUTED).mt_2())
                    .child(
                        view()
                            .mt_2()
                            .h(px(30.0))
                            .px(px(13.0))
                            .rounded(px(10.0))
                            .bg(theme::SUBTLE)
                            .justify_center()
                            .child(text("Browse files").text_size(px(13.0)).text_color(theme::TEXT))
                            .on_press(|| store().pick_avatar("pick")),
                    )
                    .into_element()
            },
        ))
        .child(when(
            || store().avatar_draft.get().is_some(),
            || {
                let d = store().avatar_draft.get().unwrap_or_default();
                crop_editor(d, true).into_element()
            },
        ))
        .child(editor_err())
}

fn editor_err() -> impl IntoElement {
    when(
        || !store().avatar_err.get().is_empty(),
        || {
            text(|| store().avatar_err.get())
                .text_size(px(12.0))
                .text_color(Hsla::from_str("#ff7b7b").unwrap_or(theme::TEXT.dark))
                .into_element()
        },
    )
}

/// `cropEditor` — the round image, the zoom slider, Restart / Set avatar
/// (App.tsx:1744).
fn crop_editor(d: crate::store::AvatarDraft, show_name: bool) -> impl IntoElement {
    // `dragFrom` — the crop when the pan started (`onPanResponderGrant`).
    let drag_from = std::rc::Rc::new(std::cell::Cell::new(Crop { s: 1.0, x: 0.0, y: 0.0 }));
    let df1 = drag_from.clone();
    let df2 = drag_from.clone();
    let df3 = drag_from.clone();
    let df4 = drag_from.clone();
    let uri = d.uri.clone();
    view()
        .gap_3()
        .child(
            view()
                .items_center()
                .gap(px(6.0))
                .child(
                    view()
                        .on_pan_start(move |_: &PanEvent| {
                            df1.set(store().draft_crop.get_untracked());
                        })
                        .on_pan_end(move |e: &PanEvent| {
                            let f = df2.get();
                            store().crop_drag(f, e.translation.x.0 as f64, e.translation.y.0 as f64);
                        })
                        .child(dynamic(move || {
                            crate::avatar::cropped_image(
                                &uri,
                                CROP,
                                Some(store().draft_crop.get()),
                            )
                            .into_element()
                        })),
                )
                .child(when(
                    move || show_name,
                    {
                        let d = d.clone();
                        move || {
                            text(format!("{} · {}×{}", d.name, d.w as i64, d.h as i64))
                                .text_size(px(13.0))
                                .text_color(theme::DIM)
                                .px_2()
                                .mt(px(6.0))
                                .into_element()
                        }
                    },
                ))
                .child(
                    text("Drag to reposition")
                        .text_size(px(13.0))
                        .text_color(theme::MUTED),
                ),
        )
        .child(
            // The 1×–3× zoom slider: `−`/`+` step 0.25, the track pans
            // `s` (App.tsx:1766).
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    view()
                        .w(px(14.0))
                        .items_center()
                        .label("Zoom out")
                        .child(text("−").text_size(px(15.0)).text_color(theme::DIM))
                        .on_press(|| store().crop_zoom(-0.25)),
                )
                .child(
                    view()
                        .w(px(TRACK))
                        .h(px(20.0))
                        .justify_center()
                        .on_pan_start(move |_: &PanEvent| {
                            df3.set(store().draft_crop.get_untracked());
                        })
                        .on_pan_end(move |e: &PanEvent| {
                            let f = df4.get();
                            store().crop_scale(f, e.translation.x.0 as f64);
                        })
                        .child(
                            view()
                                .h(px(3.0))
                                .rounded(px(1.5))
                                .bg(theme::HOVER),
                        )
                        .child(
                            view()
                                .absolute()
                                .left(move || {
                                    px(((store().draft_crop.get().s as f32 - 1.0) / 2.0) * (TRACK - 14.0))
                                })
                                .size(px(14.0))
                                .rounded(px(7.0))
                                .bg(theme::TEXT),
                        ),
                )
                .child(
                    view()
                        .w(px(14.0))
                        .items_center()
                        .label("Zoom in")
                        .child(text("+").text_size(px(15.0)).text_color(theme::DIM))
                        .on_press(|| store().crop_zoom(0.25)),
                ),
        )
        .child(
            view()
                .flex_row()
                .gap_2()
                .child(
                    view()
                        .flex_1()
                        .h(px(30.0))
                        .rounded(px(10.0))
                        .bg(theme::HOVER)
                        .items_center()
                        .justify_center()
                        .child(text("Restart").text_size(px(13.0)).text_color(theme::TEXT))
                        .on_press(|| {
                            let st = store();
                            st.draft_crop.set(Crop { s: 1.0, x: 0.0, y: 0.0 });
                            st.avatar_draft.set(None);
                        }),
                )
                .child(
                    view()
                        .flex_1()
                        .h(px(30.0))
                        .rounded(px(10.0))
                        .bg(theme::TEXT)
                        .items_center()
                        .justify_center()
                        .child(text("Set avatar").text_size(px(13.0)).text_color(theme::CARD2))
                        .on_press(|| store().set_avatar()),
                ),
        )
}

// ————————————————————————— the app rename modal —————————————————————————

/// `appRename`'s modal — a full-window scrim with a centered 320pt card
/// naming a version branch: the input, Cancel / Rename (App.tsx:4190).
pub(crate) fn rename_overlay() -> impl IntoElement {
    dynamic(|| {
        let st = store();
        let Some(r) = st.app_rename.get() else {
            return view().into_element();
        };
        let (title, placeholder) =
            (format!("Name Branch · v{}", r.v.unwrap_or(0)), "branch name (e.g. slow-ball)".to_owned());
        view()
            .size_full()
            .absolute()
            .items_center()
            .justify_center()
            .bg(move || ac(if dark() { "#00000080" } else { "#00000040" }, theme::PANE.dark))
            .on_press(|| store().app_rename.set(None))
            .child(
                view()
                    .w(px(320.0))
                    .rounded(px(16.0))
                    // A dialog over the scrim: opaque in both themes.
                    .bg(theme::POPOVER)
                    .border_1()
                    .border_color(theme::BORDER_F)
                    .p(px(16.0))
                    .gap(px(12.0))
                    .on_press(|| {})
                    .child(
                        text(title)
                            .text_size(px(15.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT),
                    )
                    .child(
                        view()
                            .rounded(px(10.0))
                            .border_1()
                            .border_color(theme::BORDER_S)
                            .bg(theme::MAIN)
                            .px(px(10.0))
                            .child(
                                input()
                                    .value(|| store().app_rename_text.get())
                                    .on_change(|v: &str| store().app_rename_text.set(v.to_owned()))
                                    .on_submit(|_: &str| store().save_app_rename())
                                    .placeholder(placeholder)
                                    .placeholder_color(|| color(theme::GHOST))
                                    .auto_focus(|| true)
                                    .text_size(px(14.0))
                                    .text_color(theme::TEXT)
                                    .py(px(9.0)),
                            ),
                    )
                    .child(
                        view()
                            .flex_row()
                            .justify_end()
                            .gap_2()
                            .child(
                                view()
                                    .px(px(14.0))
                                    .py(px(8.0))
                                    .rounded(px(999.0))
                                    .child(
                                        text("Cancel")
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(theme::DIM),
                                    )
                                    .on_press(|| store().app_rename.set(None)),
                            )
                            .child(
                                view()
                                    .px(px(14.0))
                                    .py(px(8.0))
                                    .rounded(px(999.0))
                                    .bg(theme::TEXT)
                                    .child(
                                        text("Rename")
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme::CARD),
                                    )
                                    .on_press(|| store().save_app_rename()),
                            ),
                    ),
            )
            .into_element()
    })
}

/// A hex string → `AdaptiveColor` (both modes), `fallback` when it
/// doesn't parse.
fn ac(hex: &str, fallback: Hsla) -> AdaptiveColor {
    AdaptiveColor::from(Hsla::from_str(hex).unwrap_or(fallback))
}

/// `hue(name)` — the library tile's deterministic color (App.tsx:102).
/// Grok's `AVATAR_HUES` — the same table as the sidebar and plugins.
fn hue(s: &str) -> AdaptiveColor {
    let c = crate::plugins::hue(s);
    AdaptiveColor { light: c, dark: c }
}

/// `client::post` — the pane's own POST wrapper (the store's client is
/// `pub(crate)` already but the pane keeps its call sites short).
async fn client_post(route: &str, body: serde_json::Value) -> Option<serde_json::Value> {
    crate::client::post(route, body).await
}

#[cfg(test)]
mod tests {
    use super::{
        chat_routines, network_policy_look, reset_look, route_traffic_look, routines_section, update_look, ChatRoutine,
        RailInk, RoutineKind, RoutinesSection, ThisRoutine,
    };
    use crate::computer::Egress;
    use crate::mini_v1::connect::Listed;
    use crate::store::RoutineOf;

    #[test]
    fn the_rails_route_traffic_icon_says_which_of_its_three_states_it_is_in() {
        let none = route_traffic_look(false, false);
        assert_eq!((none.icon, none.ink, none.enabled), ("globe", RailInk::Muted, false));
        assert_eq!(none.tip, "This server has no tunnel yet.");
        // a server that answered, with the tunnel off: the globe, and the
        // offer to route traffic through this desktop
        let off = route_traffic_look(true, false);
        assert_eq!((off.icon, off.ink, off.enabled), ("globe", RailInk::Muted, true));
        assert!(off.tip.starts_with("Traffic goes out on its own.") && off.tip.ends_with("through this desktop instead."));
        let on = route_traffic_look(true, true);
        assert_eq!((on.icon, on.ink, on.enabled), ("routeTraffic", RailInk::Success, true));
        assert!(on.tip.starts_with("Routing traffic through this desktop for all your computers.") && on.tip.ends_with("Click to stop."));
        // a server with no tunnel route is never asked to change one
        assert!(!route_traffic_look(false, true).enabled);
    }

    #[test]
    fn the_rails_network_rule_is_tinted_only_when_the_answer_is_not_ask() {
        assert_eq!(network_policy_look(Some(Egress::Ask)).ink, RailInk::Muted);
        assert_eq!(network_policy_look(Some(Egress::Always)).ink, RailInk::Accent);
        assert_eq!(network_policy_look(Some(Egress::Never)).ink, RailInk::Danger);
        let always = network_policy_look(Some(Egress::Always));
        assert!(always.enabled);
        assert_eq!(always.tip, "Use your network from this computer: Always. Click to change. Applies to this computer only.");
        let nothing = network_policy_look(None);
        assert_eq!((nothing.ink, nothing.enabled), (RailInk::Muted, false));
        assert_eq!(nothing.tip, "This computer has no network rule from the server yet.");
    }

    #[test]
    fn the_rails_update_and_reset_say_the_computers_status_and_enable_with_it() {
        // (present, busy, stale, current) → the tooltip `box_chrome` shows
        let tip = |present, busy, stale, current| update_look(present, busy, stale, current).tip;
        assert_eq!(tip(true, false, true, false), "Update available");
        assert_eq!(tip(true, false, false, true), "Up to date");
        assert_eq!(tip(true, false, false, false), "Update this computer");
        assert_eq!(tip(false, false, false, false), "No computer yet");
        let stale = update_look(true, false, true, false);
        assert_eq!((stale.ink, stale.enabled), (RailInk::Success, true), "a newer image waits: tinted and pressable");
        assert!(!update_look(true, false, false, true).enabled, "nothing to update");
        assert!(!update_look(true, true, true, false).enabled, "an update is already running");
        assert!(!update_look(false, false, false, false).enabled, "no computer at all");
        assert_eq!(update_look(true, false, false, false).ink, RailInk::Muted);
        // reset wants a computer that is not updating
        assert!(reset_look(true, false).enabled);
        assert!(!reset_look(true, true).enabled);
        assert!(!reset_look(false, false).enabled);
        assert_eq!(reset_look(true, false).tip, "Reset this computer");
    }

    #[test]
    fn a_routine_thread_shows_its_own_routine_and_a_chat_its_routines() {
        let listed = vec![Listed::of(&serde_json::json!({"id": "app.buwiz.ebirforms", "title": "BIR", "routines": [
            {"id": "payments", "name": "BIR payments", "every_s": 300, "enabled": true, "available": true,
             "thread": "t2", "home": "t3", "last_run": 1793194720000u64, "last_items": 0}]}))];
        let at = |_: f64| "3:42 PM".to_owned();
        assert_eq!(routines_section(None, &listed, "Piccolo", at), RoutinesSection::List, "a chat lists its routines");
        let open = RoutineOf { plugin: "app.buwiz.ebirforms".into(), id: "payments".into(), instance: String::new(), name: "BIR payments".into(), thread: "t2".into() };
        assert_eq!(
            routines_section(Some(open.clone()), &listed, "Piccolo", at),
            RoutinesSection::This(Box::new(ThisRoutine {
                name: "BIR payments".into(),
                plugin: "app.buwiz.ebirforms".into(),
                id: "payments".into(),
                instance: String::new(),
                by: "BIR".into(),
                schedule: "Checks every 5 min".into(),
                status: "Last checked 3:42 PM · nothing new".into(),
                failed: false,
                enabled: true,
                pending: false,
                chat: "t2".into(),
                go: "Go to Piccolo chat".into(),
            }))
        );
        // a failure reads as one; before the plugins are listed, the name
        // and the way back still show
        let mut failing = listed.clone();
        failing[0].routines[0].last_error = "fx_query: offline".into();
        let RoutinesSection::This(r) = routines_section(Some(open.clone()), &failing, "Piccolo", at) else { panic!() };
        assert_eq!((r.status.as_str(), r.failed), ("Last check failed: fx_query: offline", true));
        let RoutinesSection::This(r) = routines_section(Some(open), &[], "Piccolo", at) else { panic!() };
        assert_eq!((r.by.as_str(), r.status.as_str(), r.go.as_str()), ("", "", "Go to Piccolo chat"));
        assert!(!r.enabled, "no Run now or Pause before the plugins are listed");
    }

    #[test]
    fn a_chat_lists_its_routines_paused_ones_too_and_no_other_chats() {
        let listed = vec![Listed::of(&serde_json::json!({"id": "app.buwiz.ebirforms", "title": "BIR", "routines": [
            {"id": "payments", "name": "BIR payments", "every_s": 300, "enabled": true, "available": true, "thread": "t2", "home": "t3"},
            {"id": "confirm", "name": "BIR confirmations", "every_s": 300, "enabled": false, "available": false,
             "reason": "you have not approved what it reads", "thread": "t2", "home": "t4"},
            {"id": "elsewhere", "name": "Other chat", "every_s": 300, "enabled": true, "available": true, "thread": "t9"},
            {"id": "never", "name": "Never enabled", "every_s": 300, "enabled": false, "available": false}]}))];
        let server = vec![
            crate::routines::Routine::of(&serde_json::json!({"id": "r1", "name": "Calendar sync", "thread": "t2", "enabled": true, "botName": "Piccolo", "last_error": "offline"})),
            crate::routines::Routine::of(&serde_json::json!({"id": "r2", "name": "Not here", "thread": "t5", "enabled": true})),
        ];
        let got = chat_routines("t2", &listed, &server, "Buwiz Bot");
        assert_eq!(got.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["BIR payments", "BIR confirmations", "Calendar sync"]);
        assert_eq!((got[0].status.as_str(), got[0].enabled), ("Waiting for its first run", true));
        assert_eq!(got[0].by, "BIR · every 5 min");
        assert_eq!(
            got[0].kind,
            RoutineKind::Plugin {
                plugin: "app.buwiz.ebirforms".into(),
                id: "payments".into(),
                instance: String::new(),
                home: "t3".into(),
                chat: "t2".into(),
                trigger: "schedule".into(),
                hook: false,
                pending: false,
                params: String::new(),
            }
        );
        // paused: it says so (not why it couldn't run), and resumes in t2
        assert_eq!((got[1].status.as_str(), got[1].failed, got[1].enabled), ("Paused", false, false));
        assert_eq!(
            got[2],
            ChatRoutine {
                name: "Calendar sync".into(),
                by: "Piccolo".into(),
                status: "Last run failed: offline".into(),
                failed: true,
                enabled: true,
                kind: RoutineKind::Server { id: "r1".into() },
            }
        );
        assert_eq!(got[0].key(), "v1:app.buwiz.ebirforms/payments");
        assert_eq!(got[2].key(), "v0:r1");
        assert!(chat_routines("", &listed, &server, "Buwiz Bot").is_empty(), "no chat open, none");
    }

    #[test]
    fn a_routines_instances_list_in_their_own_chats_with_params_and_trigger() {
        let listed = vec![Listed::of(&serde_json::json!({"id": "app.buwiz.ebirforms", "title": "BIR", "routines": [
            {"id": "confirm", "name": "BIR confirmations", "every_s": 300, "enabled": true, "available": true, "thread": "t2",
             "params": {"type": "object", "properties": {"taxpayers": {"type": "array", "items": {"type": "string"}}}},
             "writes": ["bir_confirm_filing"],
             "instances": [
                {"instance": "ri_a", "summary": "Taxpayer A", "params": {"taxpayers": ["tp_a"]}, "trigger": "schedule", "every_s": 600,
                 "enabled": true, "available": true, "thread": "t2", "home": "t7", "last_run": 0},
                {"instance": "ri_b", "summary": "Taxpayer B", "params": {"taxpayers": ["tp_b"]}, "trigger": "webhook", "every_s": 300,
                 "enabled": false, "pending": true, "available": true, "thread": "t2", "hook": false},
                {"instance": "ri_c", "summary": "Other bot", "params": {}, "trigger": "schedule", "enabled": true, "available": true, "thread": "t9"}]}]}))];
        let got = chat_routines("t2", &listed, &[], "Buwiz Bot");
        assert_eq!(got.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["BIR confirmations · Taxpayer A", "BIR confirmations · Taxpayer B"]);
        assert_eq!(got[0].by, "BIR · every 10 min", "the instance's own interval");
        assert_eq!(got[1].by, "BIR · on its webhook");
        assert_eq!((got[1].status.as_str(), got[1].enabled), ("Waiting for your Enable", false));
        let RoutineKind::Plugin { instance, params, pending, trigger, .. } = &got[1].kind else { panic!() };
        assert_eq!((instance.as_str(), params.as_str(), *pending, trigger.as_str()), ("ri_b", r#"{"taxpayers":["tp_b"]}"#, true, "webhook"));
        assert_eq!(got[0].key(), "v1:app.buwiz.ebirforms/ri_a", "a run is the instance's");
        // a routine thread's card is its instance's
        let open = RoutineOf { plugin: "app.buwiz.ebirforms".into(), id: "confirm".into(), instance: "ri_b".into(), name: "BIR confirmations · Taxpayer B".into(), thread: "t2".into() };
        let RoutinesSection::This(r) = routines_section(Some(open), &listed, "Piccolo", |_| "3:42 PM".to_owned()) else { panic!() };
        assert_eq!((r.instance.as_str(), r.pending, r.enabled, r.status.as_str(), r.schedule.as_str()), ("ri_b", true, false, "Waiting for your Enable", "Runs on its webhook"));
    }
}
