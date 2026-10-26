//! The site a saved login belongs to: the registrable origin (eTLD+1) of a
//! domain, a live host or a URL. Ported from nativechat
//! `src/site_login/origin.rs`, without the `url` crate.
//!
//! The suffix lists are hand lists, so they cannot be complete; what matters
//! is that two people's sites under a shared suffix (`alice.github.io`,
//! `bob.github.io`) never compare equal, or one is offered the other's login.

use std::net::IpAddr;

/// Multi-part ICANN suffixes, so `login.google.co.uk` → `google.co.uk`.
const MULTI_PART_TLDS: &[&str] = &[
    "ac.uk", "co.uk", "gov.uk", "org.uk", "me.uk", "net.uk", "sch.uk", "com.au", "net.au",
    "org.au", "edu.au", "gov.au", "id.au", "co.jp", "ne.jp", "or.jp", "ac.jp", "go.jp", "com.br",
    "net.br", "org.br", "com.mx", "co.nz", "net.nz", "org.nz", "govt.nz", "ac.nz", "co.za",
    "org.za", "co.in", "net.in", "org.in", "com.sg", "com.hk", "com.cn", "net.cn", "org.cn",
    "gov.cn", "edu.cn", "co.kr", "or.kr", "ne.kr", "com.tr", "org.tr", "net.tr", "co.il", "org.il",
    "com.tw", "org.tw", "com.my", "com.ph", "co.th", "in.th", "com.ar", "net.ar", "org.ar",
    "co.id", "web.id", "com.pl", "net.pl", "org.pl", "com.ua", "com.vn", "com.pk", "co.ke",
    "com.ng", "com.eg", "com.sa", "co.ao", "com.pe", "com.co", "com.uy", "com.ec", "com.do",
    "com.gt", "com.ve", "com.bo", "com.py",
];

/// Suffixes under which anyone can register a name: the registrable unit
/// under these is three labels (four under a three-label one).
const PRIVATE_SUFFIXES: &[&str] = &[
    "github.io",
    "gitlab.io",
    "vercel.app",
    "herokuapp.com",
    "pages.dev",
    "workers.dev",
    "netlify.app",
    "blogspot.com",
    "wordpress.com",
    "web.app",
    "firebaseapp.com",
    "azurewebsites.net",
    "cloudfront.net",
    "s3.amazonaws.com",
    "elasticbeanstalk.com",
    "onrender.com",
    "fly.dev",
    "surge.sh",
    "glitch.me",
    "repl.co",
    "ngrok.io",
    "ngrok-free.app",
    "trycloudflare.com",
    "githubusercontent.com",
    "appspot.com",
];

/// Host / eTLD+1 from a domain, live host, or URL. None when empty.
pub(crate) fn registrable_origin(raw: &str) -> Option<String> {
    let host = host_of(raw)?;
    if host.parse::<IpAddr>().is_ok() {
        return Some(host);
    }
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    if host == "localhost" || host.ends_with(".localhost") {
        return Some("localhost".into());
    }
    let labels: Vec<&str> = host.split('.').filter(|l| !l.is_empty()).collect();
    if labels.len() < 2 {
        return Some(host);
    }
    let n = labels.len();
    let last_two = format!("{}.{}", labels[n - 2], labels[n - 1]);
    if n >= 3 {
        let last_three = format!("{}.{last_two}", labels[n - 3]);
        // a private suffix can itself be three labels (`s3.amazonaws.com`)
        if n >= 4 && PRIVATE_SUFFIXES.contains(&last_three.as_str()) {
            return Some(format!("{}.{last_three}", labels[n - 4]));
        }
        if PRIVATE_SUFFIXES.contains(&last_two.as_str()) || MULTI_PART_TLDS.contains(&last_two.as_str()) {
            return Some(last_three);
        }
    }
    Some(last_two)
}

/// The host of `raw`: past a scheme and user info, before a path, query,
/// fragment or port. An IPv6 literal keeps its address without brackets.
fn host_of(raw: &str) -> Option<String> {
    let mut s = raw.trim();
    if let Some(i) = s.find("://") {
        s = &s[i + 3..];
    }
    let s = s.split(['/', '?', '#']).next().unwrap_or(s);
    let s = s.rsplit('@').next().unwrap_or(s);
    let host = if let Some(rest) = s.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else {
        s.split(':').next().unwrap_or(s)
    };
    let host = host.trim();
    (!host.is_empty()).then(|| host.to_owned())
}

/// eTLD+1 equality, so `facebook.com` matches `https://www.facebook.com/login`.
pub(crate) fn origins_match(left: &str, right: &str) -> bool {
    match (registrable_origin(left), registrable_origin(right)) {
        (Some(a), Some(b)) => a == b,
        _ => left.trim().eq_ignore_ascii_case(right.trim()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etld_plus_one() {
        assert_eq!(registrable_origin("https://accounts.google.com/signin").as_deref(), Some("google.com"));
        assert_eq!(registrable_origin("login.google.co.uk").as_deref(), Some("google.co.uk"));
        assert_eq!(registrable_origin("127.0.0.1:8443").as_deref(), Some("127.0.0.1"));
        assert_eq!(registrable_origin("http://localhost:3000").as_deref(), Some("localhost"));
        assert_eq!(registrable_origin("https://user@www.example.com:8080/x?y").as_deref(), Some("example.com"));
        assert_eq!(registrable_origin("").as_deref(), None);
    }

    #[test]
    fn distinct_sites_under_a_shared_suffix_do_not_match() {
        for (a, b) in [
            ("bank.com.cn", "evil.com.cn"),
            ("alice.github.io", "bob.github.io"),
            ("x.vercel.app", "y.vercel.app"),
            ("example.s3.amazonaws.com", "other.s3.amazonaws.com"),
        ] {
            assert!(!origins_match(a, b), "{a} must not match {b}");
        }
        assert!(origins_match("alice.github.io", "www.alice.github.io"));
        assert!(origins_match("google.co.uk", "accounts.google.co.uk"));
    }
}
