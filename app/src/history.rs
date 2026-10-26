//! Back and forward (⌘[ / ⌘]): where the user has been — the chat, the
//! details pane's tab, the file viewer's file, App Details, the plugins
//! modal's page stack, Settings' page, the New Bot sheet. A place is
//! recorded whenever it changes; stepping restores it, closing what the
//! place did not have open and opening what it did. The command palette
//! is not a place.

use std::cell::{Cell, RefCell};

use nsh_core::ui::prelude::*;

use crate::live::store;
use crate::store::{Attach, PluginNav};

/// Everything a step back restores.
#[derive(Clone, PartialEq, Default)]
struct Place {
    thread: String,
    /// The details pane's tab while it is open.
    pane: Option<String>,
    viewer: Option<Attach>,
    /// App Details' item: (id, kind).
    details: Option<(String, String)>,
    plugins: Option<Vec<PluginNav>>,
    settings: Option<String>,
    bot_modal: bool,
    /// Bot Details: bot, edit mode, page stack.
    bot_details: Option<(String, bool, Vec<String>)>,
}

/// How many places back the history reaches.
const LIMIT: usize = 100;

thread_local! {
    static PLACES: RefCell<Vec<Place>> = const { RefCell::new(Vec::new()) };
    /// The current place's index in `PLACES`.
    static AT: Cell<usize> = const { Cell::new(0) };
    /// The place a step is restoring: the changes on the way there (a
    /// chat or App Details lands a moment later) are not new places.
    static GOING: RefCell<Option<Place>> = const { RefCell::new(None) };
}

fn here() -> Place {
    let st = store();
    Place {
        thread: st.active_id.get(),
        pane: st.right_open.get().then(|| st.detail_tab.get()),
        viewer: st.viewing.get(),
        details: st.app_details.get().map(|a| (a.name.clone(), a.kind.clone())),
        plugins: st.plugins_open.get().then(|| st.plugin_nav.get()),
        settings: st.settings_open.get().then(|| st.settings_page.get()),
        bot_modal: st.bot_modal.get(),
        bot_details: st.bot_details.get().map(|d| (d.bot, d.edit, st.bd_nav.get())),
    }
}

/// Records each new place. Call once, under the store's owner.
pub(crate) fn track() {
    let place = computed(here);
    effect(move || {
        let p = place.get();
        if p.thread.is_empty() {
            return;
        }
        let going = GOING.with(|g| {
            let mut g = g.borrow_mut();
            if g.as_ref() == Some(&p) {
                *g = None;
                return true;
            }
            g.is_some()
        });
        if going {
            return;
        }
        PLACES.with(|places| {
            let mut places = places.borrow_mut();
            let at = AT.with(Cell::get);
            if places.get(at) == Some(&p) {
                return;
            }
            // a new place drops the forward ones
            let keep = if places.is_empty() { 0 } else { at + 1 };
            places.truncate(keep);
            places.push(p);
            if places.len() > LIMIT {
                places.remove(0);
            }
            AT.with(|a| a.set(places.len() - 1));
        });
    });
}

/// One step back (-1) or forward (1).
pub(crate) fn step(d: i64) {
    // over a full-screen app, Back / Forward step through the apps first
    if crate::mini::step(d) {
        return;
    }
    let target = PLACES.with(|places| {
        let places = places.borrow();
        let at = AT.with(Cell::get) as i64 + d;
        (at >= 0 && (at as usize) < places.len()).then(|| (at as usize, places[at as usize].clone()))
    });
    let Some((at, place)) = target else { return };
    AT.with(|a| a.set(at));
    GOING.with(|g| *g.borrow_mut() = Some(place.clone()));
    apply(&place);
    // a place that never quite lands (a chat gone, a fetch failed) stops
    // holding new places back
    spawn(async move {
        timer(std::time::Duration::from_millis(1500)).await;
        GOING.with(|g| {
            let mut g = g.borrow_mut();
            if g.as_ref() == Some(&place) {
                *g = None;
            }
        });
    });
}

fn apply(p: &Place) {
    let st = store();
    if st.palette.get_untracked() {
        st.close_palette();
    }
    if p.thread != st.active_id.get_untracked() {
        st.open_thread(p.thread.clone());
    }
    match &p.pane {
        Some(tab) => {
            st.detail_tab.set(tab.clone());
            st.right_open.set(true);
        }
        None => st.right_open.set(false),
    }
    match &p.viewer {
        Some(f) if st.viewing.get_untracked().as_ref() != Some(f) => st.open_file(f),
        Some(_) => {}
        None => st.viewing.set(None),
    }
    match &p.details {
        Some((id, kind)) => {
            let shown = st.app_details.get_untracked().map(|a| a.name.clone());
            if shown.as_deref() != Some(id.as_str()) {
                st.open_details_of(id, kind);
            }
        }
        None => st.app_details.set(None),
    }
    match &p.plugins {
        Some(nav) => {
            if st.plugin_nav.get_untracked() != *nav || !st.plugins_open.get_untracked() {
                st.close_plugins();
                st.open_plugins();
                // each page again, with its own fetches
                for page in nav.iter().skip(1) {
                    st.push_nav(&page.page, &page.name, &page.id);
                }
            }
        }
        None => {
            if st.plugins_open.get_untracked() {
                st.close_plugins();
            }
        }
    }
    match &p.settings {
        Some(page) => {
            st.settings_page.set(page.clone());
            st.settings_open.set(true);
        }
        None => st.settings_open.set(false),
    }
    st.bot_modal.set(p.bot_modal);
    match &p.bot_details {
        Some((bot, edit, nav)) => {
            let same = st.bot_details.get_untracked().is_some_and(|d| d.bot == *bot && d.edit == *edit);
            if !same {
                crate::botdetails::open(bot, *edit);
            }
            st.bd_nav.set(nav.clone());
        }
        None => st.bot_details.set(None),
    }
}
