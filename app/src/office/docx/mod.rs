//! The Word view: `.docx` (and legacy `.doc`, imported). The document lives
//! on the office worker in BetterOffice's editing engine ([`doc`],
//! [`layout`]); pages are drawn there from its display lists ([`draw`])
//! with the faces it measured with ([`fonts`]), and this side shows them
//! ([`view`]): what the view needs is kept in [`WordUi`].

#[cfg(test)]
mod bench;
pub(crate) mod chrome;
pub(crate) mod dialogs;
pub(crate) mod doc;
pub(crate) mod draw;
#[cfg(test)]
pub(crate) mod fixture;
pub(crate) mod fonts;
pub(crate) mod index;
pub(crate) mod layout;
pub(crate) mod project;
pub(crate) mod save;
#[cfg(test)]
mod save_tests;
#[cfg(test)]
mod tests;
pub(crate) mod view;

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use nsh_core::prelude::*;

use crate::live::store;
use doc::{Caret, Meta, Out};
use draw::Drawn;

/// A page drawn: for which layout and zoom.
#[derive(Clone)]
pub(crate) struct Shown {
    pub rev: u64,
    pub zoom: f32,
    pub drawn: Arc<Drawn>,
}

impl PartialEq for Shown {
    fn eq(&self, o: &Self) -> bool {
        Arc::ptr_eq(&self.drawn, &o.drawn) && self.zoom == o.zoom
    }
}

pub(crate) struct WordUi {
    pub meta: Signal<Arc<Meta>>,
    /// Each page's drawing, a signal per page: a page redrawn wakes only
    /// its own canvas.
    pub pages: RefCell<Vec<Signal<Option<Shown>>>>,
    pub caret: Signal<Caret>,
    pub zoom: Signal<f32>,
    /// The scroll, in points from the top of the first page.
    pub scroll: Signal<f32>,
    /// The page area's size, in points.
    pub area: Signal<(f32, f32)>,
    /// Pictures as `data:` URIs, by (part, relationship id).
    pub media: Signal<Arc<HashMap<(Option<String>, String), String>>>,
    /// The find bar: `Some(query)` while open.
    pub find: Signal<Option<String>>,
    /// The review list (tracked changes and comments) shows.
    pub review: Signal<bool>,
    pub note: Signal<String>,
    /// The document name's field has the keyboard.
    pub naming: Signal<bool>,
    /// A menu of the menu bar is open.
    pub menu_open: Signal<bool>,
    /// The Page Setup and Watermark dialogs show.
    pub setup_open: Signal<bool>,
    pub watermark_open: Signal<bool>,
    /// The pages last asked of the worker: (first, last, zoom).
    pub asked: std::cell::Cell<Option<(usize, usize, f32, u64)>>,
}

impl WordUi {
    pub(crate) fn new() -> WordUi {
        WordUi {
            meta: signal(Arc::new(Meta::default())),
            pages: RefCell::new(Vec::new()),
            caret: signal(Caret::default()),
            zoom: signal(1.0),
            scroll: signal(0.0),
            area: signal((0.0, 0.0)),
            media: signal(Arc::new(HashMap::new())),
            find: signal(None),
            review: signal(false),
            note: signal(String::new()),
            naming: signal(false),
            menu_open: signal(false),
            setup_open: signal(false),
            watermark_open: signal(false),
            asked: std::cell::Cell::new(None),
        }
    }

    /// A new file: nothing from the last one.
    pub(crate) fn reset(&self) {
        self.meta.set(Arc::new(Meta::default()));
        for p in self.pages.borrow().iter() {
            p.set(None);
        }
        self.caret.set(Caret::default());
        self.scroll.set(0.0);
        self.media.set(Arc::new(HashMap::new()));
        self.find.set(None);
        self.note.set(String::new());
        self.setup_open.set(false);
        self.watermark_open.set(false);
        self.asked.set(None);
    }

    /// The file changed under it: the pages are drawn again.
    pub(crate) fn reloaded(&self) {
        self.asked.set(None);
    }

    /// Page `i`'s signal (made on first use). It lives as long as the
    /// store, whoever asks first: made under a page's canvas binding it
    /// went with that binding's next run or with the view, and the next
    /// read or write (the worker's page, the next open's `reset`)
    /// panicked, leaving the document blank.
    pub(crate) fn page(&self, i: usize) -> Signal<Option<Shown>> {
        let mut pages = self.pages.borrow_mut();
        while pages.len() <= i {
            pages.push(Owner::detached().run(|| signal(None)));
        }
        pages[i]
    }
}

/// An answer from the worker.
pub(crate) fn on_out(u: &WordUi, o: Out) {
    match o {
        Out::Meta(m) => {
            let new = u.meta.with_untracked(|old| old.rev != m.rev || old.pages.len() != m.pages.len());
            u.meta.set(Arc::new(m));
            if new {
                view::ask_pages(u, true);
            }
        }
        Out::Page { index, rev, zoom, drawn } => {
            let s = u.page(index);
            if drawn.drawing.ops.is_empty() && drawn.w == 0.0 {
                // unchanged for this layout: the drawing stands
                if let Some(mut old) = s.get_untracked() {
                    old.rev = rev;
                    s.set(Some(old));
                }
                return;
            }
            s.set(Some(Shown { rev, zoom, drawn }));
        }
        Out::Caret(c) => {
            if let Some((page, y)) = c.reveal {
                view::reveal(u, page, y);
            }
            u.caret.set(c);
        }
        Out::Media(items) => {
            use base64::Engine as _;
            let mut m = (*u.media.get_untracked()).clone();
            for (key, bytes) in items {
                let mime = if bytes.starts_with(b"\x89PNG") {
                    "image/png"
                } else if bytes.starts_with(b"\xff\xd8") {
                    "image/jpeg"
                } else if bytes.starts_with(b"GIF") {
                    "image/gif"
                } else {
                    "application/octet-stream"
                };
                m.insert(key, format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(&*bytes)));
            }
            u.media.set(Arc::new(m));
        }
        Out::Copied(text) => crate::office::xlsx::edit::to_clipboard(&text, &html_escape(&text)),
        Out::Print(html) => {
            let title = store().viewing.get_untracked().map(|f| f.name).unwrap_or_default();
            crate::keys::print("html", html, &title);
        }
        Out::Note(n) => note(n),
    }
}

/// What the worker or a command couldn't do, as a toast (the web's
/// `sonner` toasts; there is no status bar).
pub(crate) fn note(n: impl Into<String>) {
    let n = n.into();
    if !n.is_empty() {
        crate::toast::show(n.clone());
    }
    store().office.word.note.set(n);
}

/// Plain text as an HTML fragment (the pasteboard's rich kind).
fn html_escape(s: &str) -> String {
    let e = s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    e.split('\n').map(|l| format!("<p>{l}</p>")).collect()
}
