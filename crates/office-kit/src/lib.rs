//! What Buwiz Bot's native office views share: the controls drawn as
//! BetterOffice's web editor draws them ([`ui`]), the spreadsheet's display
//! lists on the canvas ([`draw`]), the bundled office fonts and the faces
//! behind a family ([`fonts`]), the worker an open document lives on
//! ([`session`]), and what the views need of the app that hosts them
//! ([`host`]). Word (`buwiz-word`) and the spreadsheet
//! (`buwiz-spreadsheet`) are built on it.

pub mod clipboard;
pub mod draw;
pub mod fonts;
pub mod host;
pub mod prompts;
pub mod session;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
// The shared controls, wired into the Word view's chrome next (B2, B3).
#[allow(dead_code)]
pub mod ui;

use nsh_core::prelude::*;

/// Runs `f` on the UI's executor, owned by no view: it lives on when the
/// view that started it goes.
pub fn spawn_detached(f: impl std::future::Future<Output = ()> + 'static) {
    Owner::detached().run(|| {
        spawn(f);
    });
}
