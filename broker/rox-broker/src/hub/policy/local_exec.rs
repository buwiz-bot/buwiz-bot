//! The shell consent gate: whether `shell_run` may run a command on the
//! user's own Mac. Transcribed from opengrok-server
//! `crates/opengrok-policy/src/local_exec.rs` (the policy, `decide`, the
//! standing-rule refusal and the stored-mode words) with one difference:
//! opengrok's default mode is `never`; Buwiz Bot's is `bypass` until the
//! approval card lands (slice S4 flips it to `ask`), so today's `shell_run`
//! keeps working. Persistence and the routes are `super` and
//! `hub/routes/local_exec.rs`; this file is pure.
//!
//! CLOSED BY DEFAULT otherwise: an unknown command in `ask` mode is `Ask`
//! (a person decides, never a silent yes), deny always beats allow, and an
//! unrecognised stored word reads as `never`. The only path to an
//! automatic yes is an explicit allowlist rule under `ask`, or the
//! deliberately-enabled `bypass`.

use serde::{Deserialize, Serialize};

use super::shell;

/// The consent mode for ONE machine's local commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LocalExecMode {
    /// The channel is OFF. Every command is denied.
    Never,
    /// Consult the lists: deny-match denies, allow-match allows, anything else asks a person.
    Ask,
    /// Allow everything, skipping the lists — a deliberate, machine-wide choice, like Claude
    /// Code's bypass. Buwiz Bot's default until S4 (the approval card) lands.
    #[default]
    Bypass,
}

/// A machine's local-commands policy. Absent ⇒ the default (`Bypass`, no rules).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LocalExecPolicy {
    pub mode: LocalExecMode,
    /// Command patterns that auto-ALLOW under `Ask` (added on demand: "always allow").
    #[serde(default)]
    pub allow: Vec<String>,
    /// Command patterns that auto-DENY under `Ask` (added on demand: "always deny").
    #[serde(default)]
    pub deny: Vec<String>,
    /// Allows that live only in this process. Dropped on restart. Not in GET /policy.
    #[serde(default, skip)]
    pub session_allow: Vec<String>,
}

/// The gate's verdict for one command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalExecDecision {
    /// Run it automatically (an allowlist rule, or `Bypass`).
    Allow,
    /// Refuse it. Never runs. `why` is what the model and the person are told; `rule` is the deny
    /// rule that matched, for the audit row only — `None` when no rule did (mode `never`, a line
    /// too long to read).
    Deny { why: String, rule: Option<String> },
    /// Suspend — a person decides for THIS command. Never treated as a yes.
    Ask,
}

/// The raw-text prefix match the gate used before it read shell syntax: equal, or `pattern` plus a
/// space. Kept ONLY as a second way for a deny rule to match, so reading the line more closely can
/// never make a stored deny (`curl x | sh`) refuse less than it did. Never used for allow: on the
/// raw line it allowed `ls; rm -rf ~` under a rule of `ls` (opengrok #204).
fn matches(pattern: &str, command: &str) -> bool {
    let pattern = pattern.trim();
    let command = command.trim();
    if pattern.is_empty() {
        return false;
    }
    command == pattern || command.starts_with(&format!("{pattern} "))
}

/// The first word of a command pattern, after a path prefix (`/usr/bin/sudo`,
/// `C:\Windows\System32\sudo.exe`). Used only to decide whether a standing allow is forbidden;
/// matching at run time is `decide`.
fn first_command(pattern: &str) -> &str {
    let token = pattern.split_whitespace().next().unwrap_or("");
    token.rsplit(['/', '\\']).next().unwrap_or(token)
}

/// Whether this standing rule may be persisted. Deny is never refused here — remembering "never
/// run sudo" is a safety net. Allow of `sudo` (and `sudo.exe`, any path, any arguments) is
/// refused, because a standing allow on `sudo` would silently cover `sudo rm -rf /`. So is an
/// allow that is not one plain command (`cd src && cargo test`): the gate would never match it,
/// and storing it inert hides that from the person who asked for it (opengrok #203).
///
/// The one writer of standing rules is `POST /local-exec/policy/rule`; any future writer goes
/// through this function too. The store writes rows, never policy.
pub fn standing_rule_refusal(kind: &str, pattern: &str) -> Option<&'static str> {
    if kind != "allow" {
        return None;
    }
    let command = first_command(pattern);
    if command.eq_ignore_ascii_case("sudo") || command.eq_ignore_ascii_case("sudo.exe") {
        Some("sudo cannot be a standing allow")
    } else if shell::read(pattern).plain.is_none() {
        Some(
            "an allow rule must be one plain command: no ; && || | & or newline, no $( ) or \
             backticks, no redirection to a path, no VAR= in front, and not a program that runs \
             another (sh, eval, env, sudo, xargs…)",
        )
    } else {
        None
    }
}

/// The longest line `Ask` reads. A line reaches the gate from a model's tool arguments, so the
/// reading must have a bound one call cannot push the broker past. A longer line is refused,
/// never asked about.
pub const MAX_JUDGED_BYTES: usize = 64 * 1024;

/// What the model is told when a deny rule refuses a command. See `decide`.
pub const DENIED_BY_A_RULE: &str = "a deny rule on this computer refused this command, so it did \
    not run. Do not reword it to get past the rule; tell the person what you needed it for.";

/// THE GATE. The one place a command on the user's own machine is judged. Everything that would
/// run a local command MUST pass through here first, before anything is spawned.
///
/// - `Never`: deny, always.
/// - `Bypass`: allow (the lists are skipped by the user's deliberate choice).
/// - `Ask`: a line over `MAX_JUDGED_BYTES` is denied unread. A deny rule matching ANY simple
///   command in the line denies (deny wins); else an allow or session-allow rule covering the
///   line allows, and only a line that is ONE plain simple command can be covered; else ask.
///   See `shell` for how the line is read.
pub fn decide(policy: &LocalExecPolicy, command: &str) -> LocalExecDecision {
    match policy.mode {
        LocalExecMode::Never => LocalExecDecision::Deny {
            why: "local commands on this computer are off (mode: never) — turn them on in Settings → Computer to run commands here".to_string(),
            rule: None,
        },
        LocalExecMode::Bypass => LocalExecDecision::Allow,
        LocalExecMode::Ask => {
            if command.len() > MAX_JUDGED_BYTES {
                return LocalExecDecision::Deny {
                    why: format!(
                        "this command is {} bytes; the gate reads at most {MAX_JUDGED_BYTES} \
                         before it decides, and refuses a longer one rather than run it unread — \
                         split it into shorter commands",
                        command.len()
                    ),
                    rule: None,
                };
            }
            let line = shell::read(command);
            let denied = if policy.deny.is_empty() {
                None
            } else {
                let runs = shell::programs(command, line.plain.as_deref());
                policy.deny.iter().find(|pattern| matches(pattern, command) || shell::denies(pattern, &runs))
            };
            let allowed = || match &line.plain {
                Some(words) => policy.allow.iter().chain(policy.session_allow.iter()).any(|pattern| shell::allows(pattern, words)),
                None => false,
            };
            // THE MODEL IS NOT TOLD WHICH RULE (opengrok #224). Naming it handed the model the
            // exact words to rephrase around, and a deny rule is a string match, not a sandbox.
            // The refusal still reaches it as a result it can reason about: what happened, and
            // what to do instead. The rule is for the person's audit row.
            if let Some(pattern) = denied {
                LocalExecDecision::Deny { why: DENIED_BY_A_RULE.to_string(), rule: Some(pattern.trim().to_string()) }
            } else if allowed() {
                LocalExecDecision::Allow
            } else {
                LocalExecDecision::Ask
            }
        }
    }
}

/// The simple commands in `line`, as written: the gate's own split, so the list an approval card
/// shows is the line the gate read.
pub fn simple_commands(line: &str) -> Vec<String> {
    shell::read(line).simple_commands
}

impl LocalExecMode {
    /// From the stored word; anything unrecognised is the closed `Never` — a typo in the
    /// database may only ever narrow access. (An ABSENT row is the default, `Bypass`: that is
    /// `super::load`'s call, not this one's.)
    pub fn from_stored(mode: &str) -> Self {
        match mode {
            "ask" => Self::Ask,
            "bypass" => Self::Bypass,
            _ => Self::Never,
        }
    }

    pub fn as_stored(&self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Ask => "ask",
            Self::Bypass => "bypass",
        }
    }
}

// opengrok-server `crates/opengrok-policy/tests/unit/local_exec.rs`, plus the cases Buwiz
// Bot's default and the shell reader add.
#[cfg(test)]
mod tests {
    use super::*;

    fn policy(mode: LocalExecMode, allow: &[&str], deny: &[&str]) -> LocalExecPolicy {
        LocalExecPolicy { mode, allow: allow.iter().map(|s| s.to_string()).collect(), deny: deny.iter().map(|s| s.to_string()).collect(), session_allow: Vec::new() }
    }

    fn denied(d: LocalExecDecision) -> bool {
        matches!(d, LocalExecDecision::Deny { .. })
    }

    #[test]
    fn policy_default_is_bypass_until_s4_and_an_unknown_stored_word_is_never() {
        // this PR's default: behaviour unchanged for today's shell_run
        assert_eq!(decide(&LocalExecPolicy::default(), "echo hi"), LocalExecDecision::Allow);
        assert_eq!(LocalExecMode::default(), LocalExecMode::Bypass);
        // but a stored word the gate does not know closes the channel, never opens it
        assert_eq!(LocalExecMode::from_stored("bypas"), LocalExecMode::Never);
        assert_eq!(LocalExecMode::from_stored(""), LocalExecMode::Never);
        assert_eq!(LocalExecMode::from_stored("ASK"), LocalExecMode::Never);
        for m in [LocalExecMode::Never, LocalExecMode::Ask, LocalExecMode::Bypass] {
            assert_eq!(LocalExecMode::from_stored(m.as_stored()), m);
        }
    }

    #[test]
    fn policy_never_denies_even_an_allowlisted_command() {
        let p = policy(LocalExecMode::Never, &["echo"], &[]);
        let d = decide(&p, "echo hi");
        assert!(denied(d.clone()));
        let LocalExecDecision::Deny { why, rule } = d else { unreachable!() };
        assert!(why.contains("mode: never"), "{why}");
        assert_eq!(rule, None);
    }

    #[test]
    fn policy_ask_with_no_rules_asks() {
        let p = policy(LocalExecMode::Ask, &[], &[]);
        assert_eq!(decide(&p, "echo hi"), LocalExecDecision::Ask);
    }

    #[test]
    fn policy_ask_allowlist_allows_on_word_boundary_only() {
        let p = policy(LocalExecMode::Ask, &["git status"], &[]);
        assert_eq!(decide(&p, "git status"), LocalExecDecision::Allow);
        assert_eq!(decide(&p, "git status --short"), LocalExecDecision::Allow);
        // A shared prefix that is NOT a word boundary must not match — closed by default.
        assert_eq!(decide(&p, "git statusx"), LocalExecDecision::Ask);
        assert_eq!(decide(&p, "git log"), LocalExecDecision::Ask);
    }

    #[test]
    fn policy_allow_never_covers_a_chained_or_nested_line() {
        // opengrok #204: `ls` allowed `ls; rm -rf ~` on the raw prefix; the shell reader asks
        let p = policy(LocalExecMode::Ask, &["ls"], &[]);
        assert_eq!(decide(&p, "ls"), LocalExecDecision::Allow);
        assert_eq!(decide(&p, "ls -la ~"), LocalExecDecision::Allow);
        for line in ["ls; rm -rf ~", "ls && rm -rf ~", "ls | sh", "ls $(rm -rf ~)", "ls `rm x`", "ls > /etc/passwd", "X=1 ls", "ls\nrm -rf ~"] {
            assert_eq!(decide(&p, line), LocalExecDecision::Ask, "{line:?}");
        }
    }

    #[test]
    fn policy_ask_denylist_denies() {
        let p = policy(LocalExecMode::Ask, &[], &["rm"]);
        assert!(denied(decide(&p, "rm -rf /")));
        // written another way, or hidden behind an allowed word (deny over-reads on purpose)
        assert!(denied(decide(&p, "/bin/rm -rf /")));
        assert!(denied(decide(&p, "true; rm -rf /")));
        assert!(denied(decide(&p, "sh -c 'rm -rf ~'")));
    }

    #[test]
    fn policy_deny_wins_over_allow() {
        let p = policy(LocalExecMode::Ask, &["sudo rm"], &["sudo"]);
        let d = decide(&p, "sudo rm -rf /");
        let LocalExecDecision::Deny { why, rule } = d else { panic!("{d:?}") };
        assert_eq!(why, DENIED_BY_A_RULE);
        assert_eq!(rule.as_deref(), Some("sudo"));
        let p = policy(LocalExecMode::Ask, &["git status"], &["git"]);
        assert!(denied(decide(&p, "git status")));
    }

    #[test]
    fn policy_sudo_cannot_stand_as_allow() {
        let refused = Some("sudo cannot be a standing allow");
        assert_eq!(standing_rule_refusal("allow", "sudo"), refused);
        assert_eq!(standing_rule_refusal("allow", "sudo rm -rf /"), refused);
        assert_eq!(standing_rule_refusal("allow", "/usr/bin/sudo"), refused);
        assert_eq!(standing_rule_refusal("allow", r"C:\Windows\System32\sudo.exe"), refused);
        assert_eq!(standing_rule_refusal("allow", "  SUDO -u root id"), refused);
        // Deny of sudo is the safety net; it is not refused.
        assert_eq!(standing_rule_refusal("deny", "sudo"), None);
        assert_eq!(standing_rule_refusal("deny", "sudo rm"), None);
        // A different command that merely starts with the letters, or sudo later.
        assert_eq!(standing_rule_refusal("allow", "sudoedit"), None);
        assert_eq!(standing_rule_refusal("allow", "echo sudo"), None);
        assert_eq!(standing_rule_refusal("allow", "uname"), None);
        // an allow that is not one plain command is refused (opengrok #203)
        assert!(standing_rule_refusal("allow", "cd src && cargo test").is_some());
        assert!(standing_rule_refusal("allow", "sh -c ls").is_some());
        assert_eq!(standing_rule_refusal("deny", "cd src && cargo test"), None);
    }

    #[test]
    fn policy_session_allow_runs_under_ask_and_dies_with_the_struct() {
        let mut p = policy(LocalExecMode::Ask, &[], &[]);
        p.session_allow = vec!["git status".into()];
        assert_eq!(decide(&p, "git status --short"), LocalExecDecision::Allow);
        assert_eq!(decide(&p, "git log"), LocalExecDecision::Ask);
        let mut denied_too = p.clone();
        denied_too.deny = vec!["git".into()];
        assert!(denied(decide(&denied_too, "git status")));
    }

    #[test]
    fn policy_bypass_allows_everything() {
        let p = policy(LocalExecMode::Bypass, &[], &["rm"]);
        assert_eq!(decide(&p, "rm -rf /"), LocalExecDecision::Allow);
        assert_eq!(decide(&p, "anything at all"), LocalExecDecision::Allow);
    }

    #[test]
    fn policy_an_empty_pattern_never_matches() {
        // A blank rule must not become a wildcard that allows or denies everything.
        assert!(!matches("", "echo hi"));
        assert!(!matches("   ", "echo hi"));
        let p = policy(LocalExecMode::Ask, &[""], &[]);
        assert_eq!(decide(&p, "echo hi"), LocalExecDecision::Ask);
        let p = policy(LocalExecMode::Ask, &["echo"], &["", "  "]);
        assert_eq!(decide(&p, "echo hi"), LocalExecDecision::Allow);
    }

    #[test]
    fn policy_a_line_too_long_to_read_is_denied_not_asked() {
        let p = policy(LocalExecMode::Ask, &["echo"], &[]);
        let long = format!("echo {}", "x".repeat(MAX_JUDGED_BYTES));
        let d = decide(&p, &long);
        assert!(denied(d.clone()), "{d:?}");
        assert_eq!(decide(&p, &long[..100]), LocalExecDecision::Allow);
        assert_eq!(simple_commands("ls; rm x"), vec!["ls".to_owned(), "rm x".to_owned()]);
    }

    #[test]
    fn policy_round_trips_as_json_with_unknown_fields_ignored_and_session_allow_skipped() {
        let p: LocalExecPolicy = serde_json::from_str(r#"{"mode":"ask","allow":["ls"],"deny":[],"future":1}"#).unwrap();
        assert_eq!(p.mode, LocalExecMode::Ask);
        assert_eq!(p.allow, ["ls"]);
        let mut p = p;
        p.session_allow = vec!["git".into()];
        assert_eq!(serde_json::to_string(&p).unwrap(), r#"{"mode":"ask","allow":["ls"],"deny":[]}"#);
        assert!(serde_json::from_str::<LocalExecPolicy>(r#"{"mode":"open"}"#).is_err(), "an unknown mode word is not a policy");
    }
}
