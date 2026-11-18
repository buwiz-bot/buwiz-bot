//! The Mini App Protocol v1 pieces Buwiz Bot and the broker share: JCS (RFC 8785)
//! bytes, ed25519 keys and signatures over them, the id grammars, and the
//! composer's tokens and tickets, and the broker's proof of identity
//! (`proof`), and the environment variables' Buwiz names, with the refusal of
//! any variable of an earlier name (`env`), and the broker's data folder on the owner's Mac and its
//! one move from `Grok Shell` to `Buwiz Broker` (`broker_dir`).
//!
//! buwiz-gws (buwiz-app/google-workspace) keeps a copy of `jcs` and part of
//! `keys` in its `src/v1`, so its manifest digest matches: change them
//! together.

pub mod broker_dir;
pub mod env;
pub mod homes;
pub mod switches;
pub mod ids;
pub mod jcs;
pub mod keys;
pub mod proof;
pub mod setup;
pub mod text;
pub mod tokens;
pub mod ui;

#[cfg(test)]
#[path = "../../test-support/pins.rs"]
mod pins;
