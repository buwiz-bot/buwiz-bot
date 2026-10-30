//! What `MOCK=1` answers instead of a model (model.mjs). The mock module
//! the legacy generator falls back to is the pipeline's (`mockModule`,
//! 5.7a); `ask_model` takes it as an argument.

/// `chatReply`'s answer with `MOCK` set.
pub const CHAT_REPLY: &str = "_(mock reply — no model configured)_";

/// `classifyIntent` with `MOCK` set: always a widget.
pub const INTENT: &str = "ui";
