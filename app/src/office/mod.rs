//! Office files in native views. Each format is a [`session::Document`] on
//! a worker thread, drawn from BetterOffice's display lists ([`draw`]).

pub(crate) mod draw;
pub(crate) mod session;
