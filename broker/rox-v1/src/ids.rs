//! The spec's grammars for ids (section 3): plugin ids, app ids, scopes,
//! tools, short names, tickets and prop keys. Rox and the broker check
//! them the same way.

pub fn plugin_id_ok(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    s.len() <= 255 && (3..=8).contains(&parts.len()) && parts.iter().all(|p| word(p, 63, |c| c == '-'))
}

pub fn app_id_ok(s: &str) -> bool {
    word(s, 48, |c| c == '-')
}

pub fn scope_ok(s: &str) -> bool {
    word(s, 32, |c| c == '-')
}

pub fn tool_ok(s: &str) -> bool {
    word(s, 63, |c| c == '_')
}

pub fn short_ok(s: &str) -> bool {
    s.len() >= 2 && word(s, 16, |_| false)
}

/// `^[a-z][a-z0-9<extra>]{0,max-1}$`.
fn word(s: &str, max: usize, extra: fn(char) -> bool) -> bool {
    let mut cs = s.chars();
    s.len() <= max
        && cs.next().is_some_and(|c| c.is_ascii_lowercase())
        && cs.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || extra(c))
}

/// `^tkt_[0-9A-HJKMNP-TV-Z]{26}$` (Crockford base32).
pub fn ticket_ok(s: &str) -> bool {
    s.strip_prefix("tkt_").is_some_and(|t| {
        t.len() == 26 && t.chars().all(|c| c.is_ascii_digit() || (c.is_ascii_uppercase() && !"ILOU".contains(c)))
    })
}

/// `^[a-zA-Z][a-zA-Z0-9_]{0,31}$`.
pub fn prop_key_ok(s: &str) -> bool {
    let mut cs = s.chars();
    s.len() <= 32
        && cs.next().is_some_and(|c| c.is_ascii_alphabetic())
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
