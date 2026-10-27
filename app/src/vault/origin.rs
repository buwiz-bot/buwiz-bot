//! The site a saved login belongs to. Ported from nativechat
//! `src/site_login/origin.rs`, without the `url` crate.
//!
//! Invariant: a login is offered only on the exact site it was saved for.
//! The site ([`site_origin`]) is the host, lowercased; on a loopback or IP
//! host, the port too. Two different hosts never compare equal
//! (`alice.pythonanywhere.com` and `mallory.pythonanywhere.com`,
//! `login.bank.com` and `evil.bank.com`), and neither do two ports of one
//! loopback or IP host (`localhost:3000` and `localhost:8080`): a suffix
//! list cannot know every shared host, and a port on this Mac can be
//! anyone's server.
//!
//! Rows saved before this rule hold a registrable domain (eTLD+1 from the
//! hand lists below, ports dropped): `google.com` for a login typed on
//! `accounts.google.com`. Such a row ([`SiteMatch::Legacy`]) is still
//! offered on its domain's hosts, with both sites shown, and is bound to
//! the exact site of its first use (`Vault::pin_site`).

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

/// Host / eTLD+1 from a domain, live host, or URL: what rows were saved
/// under before exact sites, so only for matching those. None when empty.
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
    } else if s.parse::<std::net::Ipv6Addr>().is_ok() {
        // a bare IPv6 address: its colons are no port
        s
    } else {
        s.split(':').next().unwrap_or(s)
    };
    let host = host.trim();
    (!host.is_empty()).then(|| host.to_owned())
}

/// The site of a domain, live host or URL: its host, lowercased and
/// without a trailing dot; `host:port` on a loopback or IP host that names
/// a port (`[::1]:8443` for IPv6). None when empty.
pub(crate) fn site_origin(raw: &str) -> Option<String> {
    let host = host_of(raw)?.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    let ip = host.parse::<IpAddr>().ok();
    let loopback = host == "localhost" || host.ends_with(".localhost");
    if ip.is_none() && !loopback {
        return Some(host);
    }
    Some(match port_of(raw) {
        Some(port) if matches!(ip, Some(IpAddr::V6(_))) => format!("[{host}]:{port}"),
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

/// The port `raw` names after its host, if any.
fn port_of(raw: &str) -> Option<String> {
    let mut s = raw.trim();
    if let Some(i) = s.find("://") {
        s = &s[i + 3..];
    }
    let s = s.split(['/', '?', '#']).next().unwrap_or(s);
    let s = s.rsplit('@').next().unwrap_or(s);
    let rest = match s.strip_prefix('[') {
        Some(r) => r.split_once(']').map(|(_, r)| r)?,
        // a bare IPv6 address has more than one colon and no port
        None if s.matches(':').count() > 1 => return None,
        None => &s[s.find(':').unwrap_or(s.len())..],
    };
    let port = rest.strip_prefix(':')?;
    (!port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())).then(|| port.to_owned())
}

/// Same site: exact host, and port on loopback and IP hosts.
pub(crate) fn origins_match(left: &str, right: &str) -> bool {
    match (site_origin(left), site_origin(right)) {
        (Some(a), Some(b)) => a == b,
        _ => left.trim().eq_ignore_ascii_case(right.trim()),
    }
}

/// How a saved row's site stands to the site a form asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SiteMatch {
    /// The same site.
    Exact,
    /// A row saved before exact sites (`exact` false) whose registrable
    /// domain is the form's: offered with both sites shown.
    Legacy,
    No,
}

/// How a row saved for `saved` (`exact`: under the exact-site rule)
/// matches a form for `requested`.
pub(crate) fn site_match(saved: &str, exact: bool, requested: &str) -> SiteMatch {
    if origins_match(saved, requested) {
        return SiteMatch::Exact;
    }
    match (registrable_origin(saved), registrable_origin(requested)) {
        (Some(a), Some(b)) if !exact && a == b => SiteMatch::Legacy,
        _ => SiteMatch::No,
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
    fn a_site_is_the_exact_host_and_the_port_on_loopback_and_ip_hosts() {
        assert_eq!(site_origin("https://Accounts.Google.com./signin").as_deref(), Some("accounts.google.com"));
        assert_eq!(site_origin("https://user@www.example.com:8080/x?y").as_deref(), Some("www.example.com"));
        assert_eq!(site_origin("http://localhost:3000/login").as_deref(), Some("localhost:3000"));
        assert_eq!(site_origin("app.localhost:5173").as_deref(), Some("app.localhost:5173"));
        assert_eq!(site_origin("127.0.0.1:8443").as_deref(), Some("127.0.0.1:8443"));
        assert_eq!(site_origin("https://10.0.0.7:9443/").as_deref(), Some("10.0.0.7:9443"));
        assert_eq!(site_origin("http://[::1]:8080/x").as_deref(), Some("[::1]:8080"));
        assert_eq!(site_origin("::1").as_deref(), Some("::1"));
        assert_eq!(site_origin("localhost").as_deref(), Some("localhost"));
        assert_eq!(site_origin("").as_deref(), None);
    }

    #[test]
    fn colliding_hosts_and_ports_are_distinct_sites() {
        for (a, b) in [
            ("alice.pythonanywhere.com", "mallory.pythonanywhere.com"),
            ("alice.hf.space", "mallory.hf.space"),
            ("https://alice.hf.space/login", "https://mallory.hf.space/login"),
            ("alice.amazonaws.com", "mallory.amazonaws.com"),
            ("localhost:3000", "localhost:8080"),
            ("http://localhost:3000", "localhost"),
            ("127.0.0.1:8443", "127.0.0.1:9000"),
            ("[::1]:8443", "[::1]:9000"),
            ("login.bank.com", "evil.bank.com"),
            ("alice.github.io", "bob.github.io"),
            ("bank.com.cn", "evil.com.cn"),
            ("x.vercel.app", "y.vercel.app"),
            ("example.s3.amazonaws.com", "other.s3.amazonaws.com"),
            ("accounts.google.com", "google.com"),
        ] {
            assert!(!origins_match(a, b), "{a} must not match {b}");
            assert_eq!(site_match(a, true, b), SiteMatch::No, "{a} saved exactly must not be offered on {b}");
        }
    }

    #[test]
    fn the_same_site_still_matches() {
        for (a, b) in [
            ("alice.github.io", "https://alice.github.io/login"),
            ("login.bank.com", "https://LOGIN.bank.com./signin?x=1"),
            ("alice.pythonanywhere.com", "alice.pythonanywhere.com"),
            ("localhost:3000", "http://localhost:3000/login"),
            ("127.0.0.1:8443", "https://127.0.0.1:8443/"),
            ("google.com", "https://ada@google.com/"),
        ] {
            assert!(origins_match(a, b), "{a} must match {b}");
            assert_eq!(site_match(a, true, b), SiteMatch::Exact);
            assert_eq!(site_match(a, false, b), SiteMatch::Exact);
        }
    }

    #[test]
    fn a_row_saved_before_exact_sites_is_offered_on_its_domain_as_legacy() {
        // what the old rule stored for `accounts.google.com` and `localhost:3000`
        assert_eq!(site_match("google.com", false, "accounts.google.com"), SiteMatch::Legacy);
        assert_eq!(site_match("localhost", false, "http://localhost:3000"), SiteMatch::Legacy);
        assert_eq!(site_match("127.0.0.1", false, "127.0.0.1:9000"), SiteMatch::Legacy);
        // never across domains, and never once the row is exact
        assert_eq!(site_match("google.com", false, "google.co.uk"), SiteMatch::No);
        assert_eq!(site_match("alice.github.io", false, "bob.github.io"), SiteMatch::No);
        assert_eq!(site_match("google.com", true, "accounts.google.com"), SiteMatch::No);
        assert_eq!(site_match("localhost:3000", true, "localhost:8080"), SiteMatch::No);
    }
}
