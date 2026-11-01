//! The hub's library: what the routes and the tools share. Track A's are
//! `botctx` (a bot's own context), `attachments` (`@` tokens) and `groups`;
//! track C's (5.7b) `appmeta` (a mini app's details), `artifacts` (the
//! Library's records) and `drive` (Drive files as artifacts).

pub mod attachments;
pub mod botctx;
pub mod groups;
pub mod appmeta;
pub mod artifacts;
pub mod drive;
