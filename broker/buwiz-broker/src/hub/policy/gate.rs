//! The shell gate: what `shell_run` may run on this Mac under the Mac's mode and rules, how
//! risky an allow rule is, and how a command is kept in Activity (clipped, secrets masked).
//!
//! Ported in part from opengrok-server crates/opengrok-policy (MIT): `matches`,
//! `MAX_JUDGED_BYTES` and the deny half of `decide` are `src/local_exec.rs`'s. The modes are
//! this broker's own: `only` runs a line when every simple command in it has an allow, and
//! `bypass` runs anything; a block applies in both (the reference's bypass skipped its denies).

use std::path::{Path, PathBuf};

use super::shell;

/// The Mac's shell mode (`shell_mode`; no row is `Bypass`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Only commands with an allow rule run; the rest are refused.
    Only,
    /// Every command runs; blocks still apply.
    Bypass,
}

impl Mode {
    pub fn from_stored(s: &str) -> Self {
        if s == "only" {
            Mode::Only
        } else {
            Mode::Bypass
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Mode::Only => "only",
            Mode::Bypass => "bypass",
        }
    }
}

/// The longest line the gate reads (opengrok's bound): a longer one is refused unread.
pub const MAX_JUDGED_BYTES: usize = 64 * 1024;

/// What the model is told when a block refuses a command: not which block, or it learns the
/// words to reword around (opengrok #224).
pub const BLOCKED: &str = "A block on this Mac refused this command, so it did not run. Do not reword it to get past the block; tell the user what you needed it for.";

/// The gate's verdict for one `shell_run` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Run,
    /// a block matched (`rule`, for Activity only)
    Blocked { rule: String },
    /// mode `only` and `part` has no allow
    NotAllowed { part: String },
    TooLong,
}

/// The raw-text prefix match (opengrok `local_exec::matches`): equal, or `pattern` plus a
/// space. Kept only as a second way for a block to match.
pub fn matches(pattern: &str, command: &str) -> bool {
    let pattern = pattern.trim();
    let command = command.trim();
    if pattern.is_empty() {
        return false;
    }
    command == pattern || command.starts_with(&format!("{pattern} "))
}

/// The block among `blocks` that refuses `command`, if any: every simple command in it, the
/// inside of each substitution, and each part as the allow side reads it.
pub fn blocked_by<'a>(blocks: &'a [String], command: &str) -> Option<&'a String> {
    if blocks.is_empty() {
        return None;
    }
    let line = shell::read(command);
    let mut runs = shell::programs(command, line.plain.as_deref());
    for part in &line.simple_commands {
        let plain = shell::read(part).plain;
        if let Some(words) = plain.as_deref() {
            runs.extend(shell::programs("", Some(words)));
        }
    }
    blocks.iter().find(|b| matches(b, command) || shell::denies(b, &runs))
}

/// THE GATE for one line under `mode`, `allow` and `block`.
pub fn decide(mode: Mode, allow: &[String], block: &[String], command: &str) -> Verdict {
    if command.len() > MAX_JUDGED_BYTES && (mode == Mode::Only || !block.is_empty()) {
        return Verdict::TooLong;
    }
    if let Some(rule) = blocked_by(block, command) {
        return Verdict::Blocked { rule: rule.trim().to_owned() };
    }
    if mode == Mode::Bypass {
        return Verdict::Run;
    }
    let parts = shell::read(command).simple_commands;
    if parts.is_empty() {
        return Verdict::NotAllowed { part: command.trim().to_owned() };
    }
    for part in parts {
        let covered = shell::read(&part).plain.is_some_and(|words| allow.iter().any(|a| shell::allows(a, &words)));
        if !covered {
            return Verdict::NotAllowed { part };
        }
    }
    Verdict::Run
}

/// How much an allow rule lets through (spec section 3), or why it can never be kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Risk {
    /// a fixed command that only reads
    Ro,
    /// build and test tools, whose project scripts run
    Pc,
    /// a runner, a writer, or anything not known to be narrower
    Any,
    /// refused for the Bot and for the person; the sentence says why
    Never(String),
}

impl Risk {
    pub fn as_str(&self) -> &'static str {
        match self {
            Risk::Ro => "ro",
            Risk::Pc => "pc",
            Risk::Any => "any",
            Risk::Never(_) => "never",
        }
    }
}

/// Programs that only read, whatever their arguments.
const READS: &[&str] = &["ls", "pwd", "whoami", "date", "uname", "which", "whence", "wc", "head", "tail", "cat", "du", "df", "stat", "file", "tree", "echo", "id", "hostname", "sw_vers"];
/// `git` subcommands that only read.
const GIT_READS: &[&str] = &["status", "log", "diff", "show", "rev-parse", "blame", "describe", "shortlog", "ls-files"];
/// Build and test commands (program, subcommand; `""` for the program alone).
const PROJECT: &[(&str, &str)] = &[
    ("cargo", "test"),
    ("cargo", "build"),
    ("cargo", "check"),
    ("cargo", "clippy"),
    ("cargo", "run"),
    ("cargo", "bench"),
    ("cargo", "doc"),
    ("npm", "test"),
    ("npm", "run"),
    ("npm", "ci"),
    ("npm", "start"),
    ("pnpm", "test"),
    ("pnpm", "run"),
    ("pnpm", "build"),
    ("yarn", "test"),
    ("yarn", "run"),
    ("yarn", "build"),
    ("make", ""),
    ("just", ""),
    ("swift", "build"),
    ("swift", "test"),
    ("go", "test"),
    ("go", "build"),
    ("go", "vet"),
    ("pytest", ""),
    ("xcodebuild", ""),
];

/// A rule's text as stored: trimmed, a trailing ` *` dropped (`*` is not a wildcard); and
/// whether one was.
pub fn normalize(pattern: &str) -> (String, bool) {
    let mut p = pattern.trim().to_owned();
    let mut stripped = false;
    while let Some(rest) = p.strip_suffix(" *").or_else(|| p.strip_suffix("\t*")) {
        p = rest.trim_end().to_owned();
        stripped = true;
    }
    if !p.contains(['\'', '"', '\\']) {
        p = p.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    (p, stripped)
}

/// The sentence for a trailing ` *` dropped from `pattern` (spec section 7).
pub fn star_note(pattern: &str) -> String {
    format!("`*` isn't a wildcard here. Rules match the start of a command, so this is saved as `{pattern}`.")
}

/// The risk of an allow of `pattern` (already [`normalize`]d), with `data` the broker's data
/// dir: anything naming a path under it can never be allowed.
pub fn risk(pattern: &str, data: &Path) -> Risk {
    let never = |s: String| Risk::Never(s);
    if pattern.trim().is_empty() {
        return never("Not saved: the rule is empty.".into());
    }
    if pattern.contains(['*', '?']) {
        return never("Not saved: `*` and `?` aren't wildcards here. Rules match the start of a command, so send the command's first words.".into());
    }
    let compound = || format!("Not saved: an allow rule must be one plain command. `{pattern}` can't be kept. Allow each command on its own.");
    if pattern.contains([';', '&', '|', '<', '>', '`', '\n', '\r']) || pattern.contains("$(") {
        return never(compound());
    }
    let first = pattern.split_whitespace().next().unwrap_or("");
    let program = shell::basename(first).to_ascii_lowercase();
    if program == "sudo" || program == "doas" {
        return never(format!("Not saved: {program} can't be always allowed."));
    }
    if shell::runs_another(first) {
        return never(format!("Not saved: `{program}` runs other commands, so it can't be always allowed."));
    }
    let Some(words) = shell::read(pattern).plain else { return never(compound()) };
    if words.iter().any(|w| names_dir(w, data)) {
        return never("Not saved: a rule can't name the broker's own files.".into());
    }
    let program = words[0].as_str();
    let sub = words.get(1).map(String::as_str).unwrap_or("");
    if READS.contains(&program) {
        return Risk::Ro;
    }
    if program == "git" && GIT_READS.contains(&sub) {
        return Risk::Ro;
    }
    // `claude --version`: a program asked only for its version or help
    if words.len() == 2 && ["--version", "-v", "-V", "--help", "-h"].contains(&sub) {
        return Risk::Ro;
    }
    if PROJECT.iter().any(|(p, s)| *p == program && (s.is_empty() || *s == sub)) {
        return Risk::Pc;
    }
    Risk::Any
}

/// `p` resolved as far as it exists (`/tmp` is `/private/tmp`), the rest joined as written.
pub fn real(p: &Path) -> PathBuf {
    let mut tail = Vec::new();
    let mut at = p.to_path_buf();
    loop {
        if let Ok(r) = at.canonicalize() {
            return tail.iter().rev().fold(r, |acc, t: &std::ffi::OsString| acc.join(t));
        }
        match (at.file_name().map(|n| n.to_owned()), at.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                at = parent.to_path_buf();
            }
            _ => return p.to_path_buf(),
        }
    }
}

/// Whether the word `w` is (or is under) `data`, as written or resolved; `~/` is `$HOME`.
fn names_dir(w: &str, data: &Path) -> bool {
    let home = std::env::var("HOME").unwrap_or_default();
    let expanded = match w.strip_prefix("~/") {
        Some(rest) if !home.is_empty() => format!("{home}/{rest}"),
        _ => w.to_owned(),
    };
    let dirs = [data.to_path_buf(), real(data)];
    let text = |d: &PathBuf| d.to_string_lossy().trim_end_matches('/').to_owned();
    if dirs.iter().any(|d| !text(d).is_empty() && expanded.contains(&text(d))) {
        return true;
    }
    if !expanded.starts_with('/') {
        return false;
    }
    let p = real(Path::new(&expanded));
    dirs.iter().any(|d| p.starts_with(d))
}

/// The block that makes the allow `pattern` inert, if one does.
pub fn inert_by<'a>(blocks: &'a [String], pattern: &str) -> Option<&'a String> {
    blocked_by(blocks, pattern)
}

const REDACTED: &str = "«redacted»";
const SECRET_KEY_FRAGMENTS: &[&str] = &["token", "secret", "password", "passwd", "authorization", "api_key", "apikey", "credential", "cookie"];

/// The app's coarse test for a key-shaped string (`app/src/cards/approval.rs`
/// `looks_like_a_secret`, ported).
pub fn looks_like_a_secret(text: &str) -> bool {
    let t = text.trim();
    if t.starts_with("Bearer ") || t.starts_with("sk-") || t.starts_with("xoxb-") {
        return true;
    }
    t.len() >= 40 && !t.contains(' ') && t.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+' | '/' | '='))
}

fn secret_name(name: &str) -> bool {
    let lower = name.trim_start_matches('-').to_ascii_lowercase();
    !lower.is_empty() && SECRET_KEY_FRAGMENTS.iter().any(|f| lower.contains(f))
}

/// A command as Activity keeps it: secret-shaped words and the values of secret-named
/// settings (`TOKEN=…`, `--password …`, `Bearer …`) masked (the rules of the app's
/// `redact_value`), then clipped to 120 characters.
pub fn mask_and_clip(command: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut hide_next = false;
    for word in command.split_whitespace() {
        let bare = word.trim_matches(|c| c == '\'' || c == '"');
        if hide_next && bare != "Bearer" {
            out.push(REDACTED.to_owned());
            hide_next = false;
            continue;
        }
        if let Some((name, _)) = bare.split_once('=') {
            if secret_name(name) {
                out.push(format!("{name}={REDACTED}"));
                continue;
            }
        }
        if looks_like_a_secret(bare) {
            out.push(REDACTED.to_owned());
            continue;
        }
        if bare == "Bearer" || (bare.starts_with('-') && secret_name(bare)) || bare.to_ascii_lowercase().trim_end_matches(':') == "authorization" {
            hide_next = true;
        }
        out.push(word.to_owned());
    }
    clip(&out.join(" "), 120)
}

/// `text` cut to `max` characters, the last one an ellipsis when cut.
pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}
