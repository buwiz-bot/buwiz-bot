//! The Mini App Protocol v1 pieces Rox and the broker share: JCS (RFC 8785)
//! bytes, ed25519 keys and signatures over them, the id grammars, and the
//! composer's tokens and tickets.
//!
//! buwiz-gws (buwiz-app/google-workspace) keeps a copy of `jcs` and part of
//! `keys` in its `src/v1`, so its manifest digest matches: change them
//! together.

pub mod ids;
pub mod jcs;
pub mod keys;
pub mod tokens;
