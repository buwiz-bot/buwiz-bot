//! What the model may read of a mini app's screen (O1).
//!
//! A screen is full of a person's data, and labels and data are alike in
//! the tree: both are `text` nodes. So a node's own words go out only by
//! where it stands, and everything else stays here:
//!
//! - A button's label: its accessibility name, or its one text. Rows are
//!   buttons too, and hold what they list, so a button with more texts, a
//!   name its text doesn't repeat, or a control inside it goes out unnamed.
//! - A field's name: its accessibility name.
//! - Other controls (a link, an option) stay nodes, unnamed: their words
//!   are often the data.
//! - Text anywhere else, a value of any kind, a checked or selected state
//!   and a role this module doesn't know say nothing.
//!
//! A name that went through still has to read like a label ([`plain`]).
//! `grouped` nodes are named from their children's text, so they count as
//! rows. Field values never go out, in any [`Reading`].

use buwiz_agent::{AgentId, Bounds, UiNode};
use nsh_platform_macos::ui_tree::UiNode as Seen;

/// How much of a screen goes out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reading {
    /// Button labels and field names only.
    Structure,
    /// The page's text too, for fake data in a test Bot. A release build
    /// has no such reading: the dev-agent feature isn't in it.
    #[cfg(feature = "dev-agent")]
    FullText,
}

impl Reading {
    /// This process's.
    pub fn configured() -> Reading {
        #[cfg(feature = "dev-agent")]
        if buwiz_protocol::env::var("BUWIZ_HUB_FULL_TEXT").as_deref() == Some("1") {
            return Reading::FullText;
        }
        Reading::Structure
    }

    fn full_text(self) -> bool {
        match self {
            Reading::Structure => false,
            #[cfg(feature = "dev-agent")]
            Reading::FullText => true,
        }
    }
}

const FIELDS: [&str; 8] = ["textfield", "search", "combobox", "checkbox", "switch", "radio", "slider", "spinbutton"];
const OTHER_CONTROLS: [&str; 5] = ["link", "tab", "option", "menuitem", "treeitem"];
const TEXT: [&str; 4] = ["text", "heading", "status", "alert"];

/// The longest a label may be, and the most digits it may hold: a TIN or a
/// phone number has more.
const LABEL_MAX: usize = 48;
const DIGITS_MAX: usize = 6;

fn is_control(role: &str) -> bool {
    role == "button" || FIELDS.contains(&role) || OTHER_CONTROLS.contains(&role)
}

/// Whether `s` reads like a label rather than a value.
fn plain(s: &str) -> bool {
    !s.is_empty()
        && s.chars().count() <= LABEL_MAX
        && s.chars().filter(char::is_ascii_digit).count() <= DIGITS_MAX
        && !s.contains('@')
        && !s.chars().any(char::is_control)
}

fn words(n: &Seen) -> &str {
    n.value.as_deref().filter(|v| !v.is_empty()).unwrap_or(&n.name).trim()
}

fn grouped(n: &Seen) -> bool {
    n.states.iter().any(|s| s == "grouped")
}

/// The texts in `n`, and whether a control is.
fn inside<'a>(n: &'a Seen, texts: &mut Vec<&'a str>) -> bool {
    n.children.iter().fold(false, |control, c| {
        if TEXT.contains(&c.role.as_str()) {
            texts.push(words(c));
        }
        inside(c, texts) || is_control(&c.role) || control
    })
}

/// A button's label, or `None` for a button that may be a row.
fn button_label(n: &Seen) -> Option<&str> {
    let mut texts = Vec::new();
    if inside(n, &mut texts) || grouped(n) {
        return None;
    }
    let name = n.name.trim();
    let label = match texts[..] {
        [] => name,
        [text] if name.is_empty() || name == text => text,
        _ => return None,
    };
    plain(label).then_some(label)
}

fn field_name(n: &Seen) -> Option<&str> {
    Some(n.name.trim()).filter(|name| !grouped(n) && plain(name))
}

struct Kept<'a> {
    role: &'a str,
    name: Option<&'a str>,
    node: &'a Seen,
}

fn walk<'a>(nodes: &'a [Seen], reading: Reading, out: &mut Vec<Kept<'a>>) {
    for n in nodes.iter().filter(|n| n.visible) {
        let role = n.role.as_str();
        if role == "button" {
            out.push(Kept { role, name: button_label(n), node: n });
        } else if FIELDS.contains(&role) {
            out.push(Kept { role, name: field_name(n), node: n });
        } else if OTHER_CONTROLS.contains(&role) {
            out.push(Kept { role, name: None, node: n });
        } else if reading.full_text() && TEXT.contains(&role) {
            out.push(Kept { role, name: Some(words(n)), node: n });
        }
        walk(&n.children, reading, out);
    }
}

/// `s` as a label of an [`AgentId`]: lowercase letters, digits and `-`,
/// from a letter, or `None` when nothing of it is left.
fn slug(s: &str) -> Option<String> {
    let mapped: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let parts: Vec<&str> = mapped.split('-').filter(|p| !p.is_empty()).collect();
    let joined = parts.join("-");
    match joined.chars().next() {
        None => None,
        Some(c) if c.is_ascii_digit() => Some(format!("n{joined}")),
        Some(_) => Some(joined),
    }
}

/// Makes ids that hold from run to run, as nsh's `n232` don't: a named
/// node's come from its name, an unnamed one's from its place among its
/// kind, and a repeat takes the next number.
#[derive(Default)]
struct Ids {
    used: std::collections::HashMap<String, usize>,
}

impl Ids {
    fn make(&mut self, kind: &str, name: Option<&str>) -> Option<AgentId> {
        let base = match name.and_then(slug) {
            Some(s) => format!("{kind}.{s}"),
            None => format!("{kind}.unnamed"),
        };
        let seen = self.used.entry(base.clone()).or_default();
        *seen += 1;
        let id = if *seen == 1 && name.is_some() { base } else { format!("{base}-{seen}") };
        let id = id.chars().take(128).collect::<String>();
        AgentId::parse(&id).ok()
    }
}

fn node(id: AgentId, role: &str, name: &str) -> UiNode {
    UiNode {
        id,
        role: role.to_owned(),
        name: name.to_owned(),
        value: None,
        checked: None,
        enabled: true,
        focused: false,
        visible: true,
        bounds: Bounds::default(),
        states: Vec::new(),
        children: Vec::new(),
    }
}

/// What the model gets of `nodes`, flat, in screen order. A node that can't
/// be given an id stays out.
pub fn readable(nodes: &[Seen], reading: Reading) -> Vec<UiNode> {
    let mut kept = Vec::new();
    walk(nodes, reading, &mut kept);
    let mut ids = Ids::default();
    kept.into_iter()
        .filter_map(|k| {
            // a page's text can't name its id: that would be data in a field no reading covers
            let by_name = if TEXT.contains(&k.role) { None } else { k.name };
            let id = ids.make(if FIELDS.contains(&k.role) { "field" } else { k.role }, by_name)?;
            let mut out = node(id, k.role, k.name.unwrap_or_default());
            out.enabled = k.node.enabled;
            out.focused = k.node.focused;
            Some(out)
        })
        .collect()
}

/// A screen as one node: `name` and `state` (`top`, `covered`, `card`) are
/// the Hub's, and `nodes` what [`readable`] left of it.
pub fn screen(id: AgentId, name: &str, state: &str, nodes: Vec<UiNode>) -> UiNode {
    let mut page = node(id, buwiz_agent::tree::role::PAGE, name);
    page.states.push(state.to_owned());
    page.children = nodes;
    page
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn seen(id: &str, role: &str, name: &str, value: Option<&str>, children: Vec<Seen>) -> Seen {
        let mut n = Seen::new(id, role, name);
        n.value = value.map(str::to_owned);
        n.children = children;
        n
    }

    pub(crate) fn text(id: &str, s: &str) -> Seen {
        seen(id, "text", "", Some(s), vec![])
    }

    pub(crate) fn button(id: &str, s: &str) -> Seen {
        seen(id, "button", "", None, vec![text(&format!("{id}t"), s)])
    }

    pub(crate) const NAME: &str = "Maria Fakerson";
    pub(crate) const TIN: &str = "000-111-222-00000";

    /// A profile as eBIRForms draws it, with its data in each place the
    /// live screens put it.
    pub(crate) fn profile() -> Vec<Seen> {
        vec![
            text("n1", NAME),
            text("n2", &format!("{TIN} · Head office · Individual")),
            button("n3", "New return"),
            button("n4", "Copy TIN"),
            seen("n5", "textfield", "TIN", Some(TIN), vec![]),
            seen("n6", "group", "Branch code 00000, the head office", None, vec![seen("n7", "textfield", "", Some("00000"), vec![])]),
            seen("n8", "radiogroup", "", None, vec![seen("n9", "radio", "Individual", Some("Individual"), vec![text("n10", "Individual")])]),
            seen(
                "n11",
                "button",
                &format!("Open {NAME}'s tax profile"),
                None,
                vec![text("n12", NAME), text("n13", &format!("{TIN} · Individual · 001")), text("n14", "›")],
            ),
            seen("n15", "listbox", "RDO", None, vec![seen("n16", "option", "", None, vec![text("n17", NAME)])]),
            seen("n18", "mystery", NAME, Some(TIN), vec![]),
        ]
    }

    fn out(nodes: &[Seen]) -> Vec<(String, String, String)> {
        readable(nodes, Reading::Structure).into_iter().map(|n| (n.id.to_string(), n.role, n.name)).collect()
    }

    fn triple(id: &str, role: &str, name: &str) -> (String, String, String) {
        (id.to_owned(), role.to_owned(), name.to_owned())
    }

    #[test]
    fn a_screen_keeps_button_labels_and_field_names_and_nothing_else() {
        assert_eq!(
            out(&profile()),
            vec![
                triple("button.new-return", "button", "New return"),
                triple("button.copy-tin", "button", "Copy TIN"),
                triple("field.tin", "textfield", "TIN"),
                triple("field.unnamed-1", "textfield", ""),
                triple("field.individual", "radio", "Individual"),
                triple("button.unnamed-1", "button", ""),
                triple("option.unnamed-1", "option", ""),
            ]
        );
    }

    #[test]
    fn neither_a_name_nor_a_tin_on_screen_goes_out_by_default() {
        let wire = serde_json::to_string(&readable(&profile(), Reading::Structure)).unwrap();
        assert!(!wire.contains(NAME) && !wire.contains("Fakerson") && !wire.contains("fakerson"));
        assert!(!wire.contains(TIN) && !wire.contains("000-111") && !wire.contains("000111"));
    }

    #[test]
    fn no_value_or_state_of_a_field_goes_out() {
        let mut radio = seen("r", "radio", "Corporation", Some("Corporation"), vec![]);
        radio.states.push("selected".into());
        radio.checked = Some(true);
        let got = &readable(&[radio], Reading::Structure)[0];
        assert_eq!((got.value.as_ref(), got.checked, got.states.len()), (None, None, 0));
    }

    #[test]
    fn a_button_that_may_be_a_row_goes_out_unnamed() {
        let name_differs = seen("a", "button", "Open Juan", None, vec![text("b", "Juan")]);
        let two_texts = seen("c", "button", "", None, vec![text("d", "Sales"), text("e", "3 branches")]);
        let holds_a_control = seen("f", "button", "", None, vec![text("g", "Sales"), button("h", "Edit")]);
        let mut grouped = seen("i", "button", "Juan Dela Cruz", None, vec![]);
        grouped.states.push("grouped".into());
        let got = out(&[name_differs, two_texts, holds_a_control, grouped]);
        assert_eq!(got.iter().map(|g| g.2.as_str()).collect::<Vec<_>>(), ["", "", "", "Edit", ""]);
    }

    #[test]
    fn a_button_named_as_its_text_or_by_name_alone_keeps_it() {
        let same = seen("a", "button", "Save", None, vec![text("b", "Save")]);
        let icon = seen("c", "button", "Clear", None, vec![]);
        assert_eq!(out(&[same, icon]).iter().map(|g| g.2.as_str()).collect::<Vec<_>>(), ["Save", "Clear"]);
    }

    #[test]
    fn a_label_that_reads_like_data_goes_out_unnamed() {
        for s in ["000-111-222", "0917 000 0000", "maria@example.test", "A name far too long to be a label on any button at all"] {
            assert_eq!(out(&[button("a", s)])[0].2, "", "{s}");
            assert_eq!(out(&[seen("b", "textfield", s, None, vec![])])[0].2, "", "{s}");
        }
        assert_eq!(out(&[button("a", "Add 2026")])[0].2, "Add 2026");
    }

    #[test]
    fn what_doesnt_show_goes_nowhere() {
        let mut gone = button("a", "Delete");
        gone.visible = false;
        assert!(out(&[gone]).is_empty());
    }

    #[test]
    fn repeats_and_unnamed_nodes_get_ids_that_hold_for_a_like_screen() {
        let screen = || vec![button("n1", "Save"), button("n99", "Save"), button("n7", "1099"), seen("n3", "textfield", "", None, vec![])];
        let ids: Vec<String> = out(&screen()).into_iter().map(|t| t.0).collect();
        assert_eq!(ids, ["button.save", "button.save-2", "button.n1099", "field.unnamed-1"]);
        let renumbered = vec![button("n400", "Save"), button("n401", "Save"), button("n2", "1099"), seen("n8", "textfield", "", None, vec![])];
        assert_eq!(out(&renumbered).into_iter().map(|t| t.0).collect::<Vec<_>>(), ids);
    }

    #[cfg(feature = "dev-agent")]
    #[test]
    fn full_text_adds_the_pages_text_but_never_a_fields_value_and_names_no_id_by_it() {
        let got = readable(&profile(), Reading::FullText);
        let wire = serde_json::to_string(&got).unwrap();
        assert!(got.iter().any(|n| n.role == "text" && n.name == NAME));
        assert!(got.iter().all(|n| n.value.is_none()));
        assert!(got.iter().filter(|n| n.role == "text").all(|n| n.id.as_str().starts_with("text.unnamed-")));
        assert!(!wire.contains("fakerson"));
    }

    #[test]
    fn the_default_reading_is_structure() {
        assert_eq!(Reading::configured(), Reading::Structure);
    }
}
