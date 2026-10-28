//! Bot Details — Grok Bot's "View Details" modal: the bot's mark, name,
//! last update and description, then Context (instructions, memories,
//! skills) and Integrations, each a page of the same modal (‹ back, ×
//! close). Publish sits in the footer, off until publishing exists.
//!
//! Opened to view (`edit` false: everything reads only) or to edit: the
//! description and instructions become fields saved with Save, memories
//! can be added, rewritten and forgotten, and integrations and skills
//! added and removed.

use nsh_base::input;
use nsh_core::prelude::*;

use crate::live::store;
use crate::store::{BotIntegration, RoxBot};
use crate::theme;

/// The modal's open bot and mode (`store().bot_details`).
#[derive(Clone, PartialEq)]
pub struct BotDetails {
    pub bot: String,
    pub edit: bool,
}

const W: f32 = 588.0;
const H: f32 = 710.0;

/// Opens the active chat's bot's details, to view or to edit.
pub(crate) fn open_current(edit: bool) {
    let st = store();
    let Some(b) = st.active_bot_untracked() else { return };
    open(&b.id, edit);
}

pub(crate) fn open(bot: &str, edit: bool) {
    let st = store();
    if let Some(b) = st.bots.get_untracked().iter().find(|b| b.id == bot) {
        st.bd_desc.set(b.description.clone());
        st.bd_instr.set(b.instructions.clone());
    }
    st.bd_nav.set(vec!["main".to_owned()]);
    st.bot_details.set(Some(BotDetails { bot: bot.to_owned(), edit }));
    // the add page lists what is installed
    st.fetch_mcp();
    st.fetch_skills();
}

fn close() {
    store().bot_details.set(None);
}

fn push(page: String) {
    let st = store();
    if let Some(m) = page.strip_prefix("memory:") {
        let text = st
            .bot_details
            .get_untracked()
            .and_then(|d| bot_of(&d.bot))
            .and_then(|b| b.memories.into_iter().find(|x| x.id == m))
            .map(|x| x.text)
            .unwrap_or_default();
        st.bd_mem.set(text);
    }
    st.bd_nav.update(|n| n.push(page));
}

fn pop() {
    store().bd_nav.update(|n| {
        if n.len() > 1 {
            n.pop();
        }
    });
}

fn bot_of(id: &str) -> Option<RoxBot> {
    store().bots.get().into_iter().find(|b| b.id == id)
}

/// The modal — mount in the root stack.
pub(crate) fn layer() -> impl IntoElement {
    dynamic(|| {
        let st = store();
        let Some(d) = st.bot_details.get() else { return view().into_element() };
        let Some(b) = bot_of(&d.bot) else { return view().into_element() };
        let page = st.bd_nav.get().last().cloned().unwrap_or_else(|| "main".to_owned());
        let (title, body): (String, AnyElement) = match page.as_str() {
            "context" => ("Context".to_owned(), context_page(&b, d.edit).into_element()),
            "integrations" => ("Integrations".to_owned(), integrations_page(&b, d.edit).into_element()),
            "add" => ("Add to this bot".to_owned(), add_page(&b).into_element()),
            p if p.starts_with("memory:") => {
                let id = &p["memory:".len()..];
                let text = b.memories.iter().find(|m| m.id == id).map(|m| m.text.clone()).unwrap_or_default();
                let title = if id.is_empty() { "New memory".to_owned() } else { text };
                (title, memory_page(&b, id, d.edit).into_element())
            }
            _ => (String::new(), main_page(&b, d.edit).into_element()),
        };
        view()
            .size_full()
            .absolute()
            .items_center()
            .justify_center()
            .child(view().size_full().absolute().bg(hsla(0.0, 0.0, 0.0, 0.4)).on_press(close))
            .child(
                view()
                    .w(px(W))
                    .h(px(H))
                    .max_w(percentage(0.94))
                    .max_h(percentage(0.92))
                    .bg(theme::MENU_BG)
                    .rounded(px(18.0))
                    .border_1()
                    .border_color(theme::BORDER_S)
                    .overflow_hidden()
                    .child(header(&title, page != "main"))
                    .child(scroll().flex_1().child(view().px(px(24.0)).pb(px(24.0)).child(body)))
                    .child(footer(&b, d.edit, &page)),
            )
            .into_element()
    })
}

fn header(title: &str, sub: bool) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .h(px(58.0))
        .px(px(14.0))
        .when(sub, |v| v.border_b(px(0.5)).border_b_color(theme::BORDER_S))
        .child(
            view()
                .size(px(32.0))
                .items_center()
                .justify_center()
                .rounded(px(16.0))
                .when(sub, |v| {
                    v.label("Back")
                        .child(text("‹").text_size(px(22.0)).text_color(theme::DIM))
                        .on_press(pop)
                }),
        )
        .child(
            text(title.to_owned())
                .flex_1()
                .number_of_lines(1)
                .text_size(px(16.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::TEXT)
                .text_center(),
        )
        .child(
            view()
                .label("Close")
                .size(px(32.0))
                .items_center()
                .justify_center()
                .rounded(px(16.0))
                .child(text("✕").text_size(px(14.0)).text_color(theme::DIM))
                .on_press(close),
        )
}

/// Save (edit mode, on pages with fields) and Publish (off for now).
fn footer(b: &RoxBot, edit: bool, page: &str) -> impl IntoElement {
    let id = b.id.clone();
    let saves = edit && (page == "main" || page == "context");
    let memory = page.strip_prefix("memory:").filter(|_| edit).map(str::to_owned);
    view()
        .flex_row()
        .items_center()
        .justify_end()
        .gap_2()
        .h(px(64.0))
        .px(px(18.0))
        .border_t(px(0.5))
        .border_t_color(theme::BORDER_S)
        .when(edit, |v| {
            v.child(text("Editing").text_size(px(12.0)).text_color(theme::MUTED).flex_1())
        })
        .when_some(memory, |v, mid| {
            let id = id.clone();
            let id2 = id.clone();
            let mid2 = mid.clone();
            v.when(!mid.is_empty(), |v| {
                v.child(pill("Forget", false, true, move || {
                    store().forget_memory(&id2, &mid2);
                    pop();
                }))
            })
            .child(pill("Save", true, true, move || {
                let t = store().bd_mem.get_untracked().trim().to_owned();
                if !t.is_empty() {
                    store().save_memory(&id, &mid, &t);
                    pop();
                }
            }))
        })
        .when(saves, |v| {
            let id = id.clone();
            v.child(pill("Save", true, true, move || {
                let st = store();
                st.update_bot(
                    &id,
                    serde_json::json!({
                        "description": st.bd_desc.get_untracked().trim(),
                        "instructions": st.bd_instr.get_untracked().trim(),
                    }),
                );
            }))
        })
        // Publishing a bot (a shareable template) comes later.
        .child(pill("Publish", true, false, || {}))
}

/// A footer button: filled (primary) or plain; off ones are dimmed and
/// do nothing.
fn pill(label: &'static str, primary: bool, on: bool, act: impl Fn() + 'static) -> impl IntoElement {
    text(label)
        .label(if on { label } else { "Publish (coming later)" })
        .text_size(px(15.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if primary { theme::ON_FILL } else { theme::TEXT })
        .px(px(24.0))
        .py(px(9.0))
        .rounded(px(20.0))
        .bg(if primary { theme::TEXT } else { theme::HOVER })
        .opacity(if on { 1.0f32 } else { 0.35 })
        .when(on, |v| v.on_press(move || act()))
}

fn mark(b: &RoxBot, size: f32) -> impl IntoElement {
    view().self_center().child(crate::avatar::bot_avatar(crate::avatar::BotAvatarProps::bot(b, size)))
}

fn label(t: &'static str) -> impl IntoElement {
    text(t).text_size(px(13.0)).text_color(theme::MUTED).pt(px(22.0)).pb(px(10.0)).px_2()
}

fn card() -> View {
    view().rounded(px(14.0)).bg(theme::SUBTLE).overflow_hidden()
}

/// A tappable card row: title, a line under it, and a ›.
fn nav_row(title: &'static str, sub: String, page: &'static str) -> impl IntoElement {
    let hov = signal(false);
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .px(px(16.0))
        .py(px(12.0))
        .bg(move || if hov.get() { theme::HOVER } else { AdaptiveColor::TRANSPARENT })
        .on_mouse_enter(move |_: &MouseEvent| hov.set(true))
        .on_mouse_leave(move |_: &MouseEvent| hov.set(false))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(text(title).text_size(px(15.0)).text_color(theme::TEXT))
                .child(text(sub).number_of_lines(1).text_size(px(14.0)).text_color(theme::DIM)),
        )
        .child(text("›").text_size(px(18.0)).text_color(theme::MUTED))
        .on_press(move || push(page.to_owned()))
}

/// "Last updated Oct 7" (this year) or "Oct 7, 2025".
fn updated_label(ms: f64) -> String {
    if ms <= 0.0 {
        return String::new();
    }
    // days since 1970 → civil date (Howard Hinnant's algorithm)
    let civil = |days: i64| {
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        (yoe + era * 400 + i64::from(m <= 2), m, d)
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0);
    let (y, m, d) = civil((ms / 86_400_000.0).floor() as i64);
    let (ny, _, _) = civil((now / 86_400_000.0).floor() as i64);
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let mon = MONTHS[(m - 1).clamp(0, 11) as usize];
    if y == ny { format!("Last updated {mon} {d}") } else { format!("Last updated {mon} {d}, {y}") }
}

fn field(value: Signal<String>, placeholder: &'static str, h: f32) -> impl IntoElement {
    input()
        .value(move || value.get())
        .placeholder(placeholder)
        .placeholder_color(|| color(theme::GHOST))
        .multiline(true)
        .text_size(px(15.0))
        .line_height(px(22.0))
        .text_color(theme::TEXT)
        .h(px(h))
        .px(px(16.0))
        .py(px(12.0))
        .on_change(move |t: &str| value.set(t.to_owned()))
}

fn main_page(b: &RoxBot, edit: bool) -> impl IntoElement {
    let st = store();
    let apps: Vec<String> = b.integrations.iter().filter(|i| i.kind != "skill").map(|i| i.name.clone()).collect();
    view()
        .items_center()
        .pt(px(36.0))
        .child(mark(b, 92.0))
        .child(text(b.name.clone()).text_size(px(19.0)).font_weight(FontWeight::MEDIUM).text_color(theme::TEXT).pt(px(14.0)))
        .child(text(updated_label(b.updated_at)).text_size(px(14.0)).text_color(theme::DIM).pt(px(4.0)))
        .child(if edit {
            card().w_full().mt(px(18.0)).child(field(st.bd_desc, "What this bot is for", 96.0)).into_element()
        } else {
            text(b.description.clone())
                .number_of_lines(3)
                .text_size(px(15.0))
                .line_height(px(22.0))
                .text_color(theme::DIM)
                .text_center()
                .pt(px(12.0))
                .into_element()
        })
        .child(
            card()
                .w_full()
                .mt(px(24.0))
                .child(nav_row("Context", "Instructions, memories, and skills".to_owned(), "context"))
                .child(view().h(px(0.5)).mx(px(16.0)).bg(theme::BORDER_F))
                .child(nav_row(
                    "Integrations",
                    if apps.is_empty() { "None yet".to_owned() } else { apps.join(", ") },
                    "integrations",
                )),
        )
}

fn context_page(b: &RoxBot, edit: bool) -> impl IntoElement {
    let st = store();
    let id = b.id.clone();
    let skills: Vec<BotIntegration> = b.integrations.iter().filter(|i| i.kind == "skill").cloned().collect();
    let mut memories = card();
    if b.memories.is_empty() && !edit {
        memories = memories.child(text("No memories yet").text_size(px(15.0)).text_color(theme::MUTED).px(px(16.0)).py(px(14.0)));
    }
    for (n, m) in b.memories.iter().enumerate() {
        if n > 0 {
            memories = memories.child(view().h(px(0.5)).mx(px(16.0)).bg(theme::BORDER_F));
        }
        let mid = m.id.clone();
        memories = memories.child(
            text(m.text.clone())
                .number_of_lines(1)
                .text_size(px(15.0))
                .text_color(theme::TEXT)
                .px(px(16.0))
                .py(px(14.0))
                .on_press(move || push(format!("memory:{mid}"))),
        );
    }
    if edit {
        if !b.memories.is_empty() {
            memories = memories.child(view().h(px(0.5)).mx(px(16.0)).bg(theme::BORDER_F));
        }
        memories = memories.child(
            text("+ Add a memory")
                .text_size(px(15.0))
                .text_color(theme::ACCENT)
                .px(px(16.0))
                .py(px(14.0))
                .on_press(|| push("memory:".to_owned())),
        );
    }
    view()
        .pt(px(24.0))
        .child(mark(b, 60.0))
        .child(label("Instructions"))
        .child(if edit {
            card().child(field(st.bd_instr, "How this bot behaves", 180.0)).into_element()
        } else {
            card()
                .child(
                    text(if b.instructions.is_empty() { "None".to_owned() } else { b.instructions.clone() })
                        .text_size(px(15.0))
                        .line_height(px(22.0))
                        .text_color(theme::TEXT)
                        .px(px(16.0))
                        .py(px(14.0)),
                )
                .into_element()
        })
        .child(label("Memories"))
        .child(memories)
        .child(label("Skills"))
        .child(integration_list(&id, skills, edit, "No skills yet"))
}

fn memory_page(b: &RoxBot, id: &str, edit: bool) -> impl IntoElement {
    let st = store();
    let text_now = b.memories.iter().find(|m| m.id == id).map(|m| m.text.clone()).unwrap_or_default();
    view()
        .pt(px(18.0))
        .child(label("Content"))
        .child(if edit {
            card().child(field(st.bd_mem, "Something this bot should remember", 160.0)).into_element()
        } else {
            card()
                .child(text(text_now).text_size(px(15.0)).line_height(px(22.0)).text_color(theme::TEXT).px(px(16.0)).py(px(14.0)))
                .into_element()
        })
}

fn integrations_page(b: &RoxBot, edit: bool) -> impl IntoElement {
    let apps: Vec<BotIntegration> = b.integrations.iter().filter(|i| i.kind != "skill").cloned().collect();
    view()
        .pt(px(24.0))
        .child(mark(b, 60.0))
        .child(view().h(px(18.0)))
        .child(integration_list(&b.id, apps, edit, "No integrations yet"))
}

/// Integration (or skill) rows: logo, name, what it does; × to remove
/// and + Add when editing.
fn integration_list(bot: &str, items: Vec<BotIntegration>, edit: bool, empty: &'static str) -> impl IntoElement {
    let mut c = card();
    if items.is_empty() && !edit {
        c = c.child(text(empty).text_size(px(15.0)).text_color(theme::MUTED).px(px(16.0)).py(px(14.0)));
    }
    for (n, it) in items.iter().enumerate() {
        if n > 0 {
            c = c.child(view().h(px(0.5)).mx(px(16.0)).bg(theme::BORDER_F));
        }
        let (bot2, spec) = (bot.to_owned(), format!("{}:{}", it.kind, it.id));
        c = c.child(
            view()
                .flex_row()
                .items_center()
                .gap(px(14.0))
                .px(px(16.0))
                .py(px(12.0))
                .child(logo(it, 40.0))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(text(it.name.clone()).text_size(px(15.0)).text_color(theme::TEXT))
                        .when(!it.description.is_empty(), |v| {
                            v.child(
                                text(it.description.clone())
                                    .number_of_lines(3)
                                    .text_size(px(14.0))
                                    .line_height(px(20.0))
                                    .text_color(theme::DIM),
                            )
                        }),
                )
                .when(edit, |v| {
                    v.child(
                        text("×")
                            .label("Remove")
                            .text_size(px(18.0))
                            .text_color(theme::MUTED)
                            .px_2()
                            .on_press(move || store().bot_integration(&bot2, &spec, false)),
                    )
                }),
        );
    }
    if edit {
        if !items.is_empty() {
            c = c.child(view().h(px(0.5)).mx(px(16.0)).bg(theme::BORDER_F));
        }
        c = c.child(
            text("+ Add")
                .text_size(px(15.0))
                .text_color(theme::ACCENT)
                .px(px(16.0))
                .py(px(14.0))
                .on_press(|| push("add".to_owned())),
        );
    }
    c
}

/// Everything installed the bot doesn't have yet: connected apps and
/// private skills; a press adds it.
fn add_page(b: &RoxBot) -> impl IntoElement {
    let st = store();
    let has = |kind: &str, id: &str| b.integrations.iter().any(|i| i.kind == kind && i.id == id);
    let mut rows: Vec<BotIntegration> = st
        .mcp_servers
        .get()
        .into_iter()
        .filter(|m| !has("mcp", &m.id))
        .map(|m| BotIntegration { id: m.id.clone(), name: m.name.clone(), kind: "mcp".into(), ..Default::default() })
        .collect();
    rows.extend(st.private_skills.get().into_iter().filter(|s| !has("skill", &s.id)).map(|s| BotIntegration {
        id: s.id.clone(),
        name: s.name.clone(),
        kind: "skill".into(),
        description: s.description.clone(),
        icon: String::new(),
    }));
    let mut c = card();
    if rows.is_empty() {
        c = c.child(text("Everything installed is already added").text_size(px(15.0)).text_color(theme::MUTED).px(px(16.0)).py(px(14.0)));
    }
    for (n, it) in rows.into_iter().enumerate() {
        if n > 0 {
            c = c.child(view().h(px(0.5)).mx(px(16.0)).bg(theme::BORDER_F));
        }
        let (bot, spec) = (b.id.clone(), format!("{}:{}", it.kind, it.id));
        c = c.child(
            view()
                .flex_row()
                .items_center()
                .gap(px(14.0))
                .px(px(16.0))
                .py(px(12.0))
                .child(logo(&it, 32.0))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(text(it.name.clone()).text_size(px(15.0)).text_color(theme::TEXT))
                        .child(text(if it.kind == "skill" { "Skill" } else { "App" }).text_size(px(13.0)).text_color(theme::MUTED)),
                )
                .child(text("Add").text_size(px(14.0)).text_color(theme::ACCENT))
                .on_press(move || {
                    store().bot_integration(&bot, &spec, true);
                    pop();
                }),
        );
    }
    view().pt(px(18.0)).child(c)
}

/// An integration's logo on a white plate, else its first letter.
fn logo(it: &BotIntegration, size: f32) -> AnyElement {
    let plate = view()
        .size(px(size))
        .rounded(px((size * 0.26).round()))
        .overflow_hidden()
        .items_center()
        .justify_center();
    if it.icon.is_empty() {
        let letter = it.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
        return plate
            .bg(theme::HOVER)
            .child(text(letter).text_size(px(size * 0.45)).font_weight(FontWeight::SEMIBOLD).text_color(theme::DIM))
            .into_element();
    }
    plate
        .bg(white())
        .border(px(0.5))
        .border_color(theme::BORDER_S)
        .child(image(format!("{}{}", crate::client::backend(), it.icon)).w(px(size * 0.76)).h(px(size * 0.76)))
        .into_element()
}
