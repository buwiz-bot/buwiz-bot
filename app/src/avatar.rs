//! The agent avatar (`app/blob.tsx`): the TS version is an animated
//! blob; the Rust version is a rounded disc with the bot's emoji (or
//! initial), tinted per agent.

use nsh_core::ui::prelude::*;

use crate::store::BotMeta;
use crate::theme;

/// A bot's disc: emoji over a tinted circle.
pub(crate) fn bot_avatar(bot: BotMeta, size: f32) -> impl IntoElement {
    let face = if bot.emoji.is_empty() {
        bot.name.chars().next().unwrap_or('?').to_uppercase().to_string()
    } else {
        bot.emoji.clone()
    };
    view()
        .size(px(size))
        .rounded(px(size / 2.0))
        .bg(theme::ACCENT_TINT)
        .items_center()
        .justify_center()
        .child(text(face).text_size(px(size * 0.5)).text_color(theme::ACCENT))
}

/// A small colored dot (run status: running / ok / failed).
pub(crate) fn status_dot(kind: &'static str) -> impl IntoElement {
    let color = match kind {
        "running" => theme::RUNNING,
        "ok" => theme::UP,
        _ => theme::DOWN,
    };
    view().size(px(8.0)).rounded(px(4.0)).bg(color)
}
