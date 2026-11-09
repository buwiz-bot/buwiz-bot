//! Where the broker's AWS credentials come from: the environment's `AWS_ACCESS_KEY_ID` and
//! `AWS_SECRET_ACCESS_KEY` (and `AWS_SESSION_TOKEN`), else a profile's static keys in the shared
//! credentials file (`~/.aws/credentials`, or `AWS_SHARED_CREDENTIALS_FILE`) or config file
//! (`~/.aws/config`, or `AWS_CONFIG_FILE`). The profile is the broker's own, `buwiz-broker` unless
//! `BUWIZ_EC2_PROFILE` says another: never `AWS_PROFILE`, which a person's shell may point at an
//! administrator. A profile that signs in another way (`aws login`, SSO, a role) is named in the
//! sentence, so a person knows what to change.
//!
//! The keys are read for each call, so a fixed file is used at once; they are never logged.

use std::path::{Path, PathBuf};

use super::sigv4::Credentials;
use crate::{Problem, Result, Secret};

pub const DEFAULT_PROFILE: &str = "buwiz-broker";

/// Where the broker's keys come from: read for each call, so a source can change under it. The
/// environment and the profile's files are one ([`EnvAndProfile`]); a fixed set of keys is
/// another. A person's own keys from a settings page or the Keychain would be a third, and the
/// provider wouldn't know the difference.
pub trait Keys: Send + Sync + std::fmt::Debug {
    fn read(&self) -> Result<Credentials>;
}

fn none(profile: &str, why: &str) -> Problem {
    Problem::Remote(format!("This Mac has no AWS keys for the broker: {why}. Put the access key of the IAM user made for it (buwiz-computer-broker) under [{profile}] in ~/.aws/credentials."))
}

/// The environment, then the profile's files.
#[derive(Clone, Debug)]
pub struct EnvAndProfile {
    pub profile: String,
}

impl Keys for EnvAndProfile {
    fn read(&self) -> Result<Credentials> {
        resolve(&|k: &str| std::env::var(k).ok(), std::env::var_os("HOME").map(PathBuf::from).as_deref(), &self.profile)
    }
}

/// These, always.
impl Keys for Credentials {
    fn read(&self) -> Result<Credentials> {
        Ok(self.clone())
    }
}

/// The chain, with `env` for the environment and `home` for `~`.
pub fn resolve(env: &dyn Fn(&str) -> Option<String>, home: Option<&Path>, profile: &str) -> Result<Credentials> {
    let set = |k: &str| env(k).filter(|v| !v.trim().is_empty());
    if let (Some(id), Some(secret)) = (set("AWS_ACCESS_KEY_ID"), set("AWS_SECRET_ACCESS_KEY")) {
        return Ok(Credentials { access_key: id.trim().to_owned(), secret: Secret::new(secret.trim()), token: set("AWS_SESSION_TOKEN").map(|t| Secret::new(t.trim())) });
    }
    let file = |var: &str, rest: &str| set(var).map(PathBuf::from).or_else(|| home.map(|h| h.join(rest)));
    let credentials = file("AWS_SHARED_CREDENTIALS_FILE", ".aws/credentials").and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    let config = file("AWS_CONFIG_FILE", ".aws/config").and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    let config_name = if profile == "default" { "default".to_owned() } else { format!("profile {profile}") };
    let sections = [section(&credentials, profile), section(&config, &config_name)];
    let get = |key: &str| sections.iter().flatten().find_map(|s| s.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())).filter(|v| !v.is_empty());
    if let (Some(id), Some(secret)) = (get("aws_access_key_id"), get("aws_secret_access_key")) {
        return Ok(Credentials { access_key: id, secret: Secret::new(secret), token: get("aws_session_token").map(Secret::new) });
    }
    if sections.iter().all(Option::is_none) {
        return Err(none(profile, &format!("there's no profile {profile}")));
    }
    let other = ["login_session", "sso_session", "sso_start_url", "role_arn", "credential_process"].into_iter().find(|k| get(k).is_some());
    Err(match other {
        Some(way) => none(profile, &format!("profile {profile} signs in with {way}, which the broker doesn't use")),
        None => none(profile, &format!("profile {profile} has no access key")),
    })
}

/// The `key = value` lines of `[name]` in an INI file, or `None` when it has no such section.
fn section(text: &str, name: &str) -> Option<Vec<(String, String)>> {
    let mut inside = false;
    let mut found = None::<Vec<(String, String)>>;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(head) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            inside = head.split_whitespace().collect::<Vec<_>>().join(" ") == name;
            if inside {
                found.get_or_insert_with(Vec::new);
            }
            continue;
        }
        if let (true, Some((k, v))) = (inside, line.split_once('=')) {
            found.get_or_insert_with(Vec::new).push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
        }
    }
    found
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn home(credentials: &str, config: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("buwiz-aws-{}", crate::random_hex(4).unwrap()));
        std::fs::create_dir_all(d.join(".aws")).unwrap();
        std::fs::write(d.join(".aws/credentials"), credentials).unwrap();
        std::fs::write(d.join(".aws/config"), config).unwrap();
        d
    }

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        move |k| map.get(k).cloned()
    }

    const FILES: &str = "[default]\naws_access_key_id = AKIDDEFAULT\naws_secret_access_key = d\n\n[buwiz-broker]\naws_access_key_id = AKIDBROKER\naws_secret_access_key = s3cret\n";

    #[test]
    fn the_environment_comes_first_then_the_brokers_profile() {
        let h = home(FILES, "");
        let from_env = resolve(&env(&[("AWS_ACCESS_KEY_ID", "AKIDENV"), ("AWS_SECRET_ACCESS_KEY", "e"), ("AWS_SESSION_TOKEN", "t")]), Some(&h), DEFAULT_PROFILE).unwrap();
        assert_eq!((from_env.access_key.as_str(), from_env.token.as_ref().map(Secret::expose)), ("AKIDENV", Some("t")));
        let from_file = resolve(&env(&[("AWS_PROFILE", "default")]), Some(&h), DEFAULT_PROFILE).unwrap();
        assert_eq!(from_file.access_key, "AKIDBROKER", "AWS_PROFILE isn't the broker's");
        assert_eq!(from_file.secret.expose(), "s3cret");
        assert_eq!(resolve(&env(&[]), Some(&h), "default").unwrap().access_key, "AKIDDEFAULT");
    }

    #[test]
    fn a_profile_in_the_config_file_counts_and_other_sign_ins_are_named() {
        let h = home("", "[profile buwiz-broker]\nregion = ap-southeast-1\naws_access_key_id=AKIDCFG\naws_secret_access_key=c\n[profile buwiz]\nlogin_session = arn:aws:iam::1:user/x\n");
        assert_eq!(resolve(&env(&[]), Some(&h), DEFAULT_PROFILE).unwrap().access_key, "AKIDCFG");
        let said = resolve(&env(&[]), Some(&h), "buwiz").unwrap_err().sentence();
        assert!(said.contains("login_session"), "{said}");
        let said = resolve(&env(&[]), Some(&h), "nobody").unwrap_err().sentence();
        assert!(said.contains("no profile nobody"), "{said}");
    }

    #[test]
    fn a_failure_never_says_a_secret() {
        let h = home("[buwiz-broker]\naws_access_key_id = AKIDHALF\n", "");
        let said = resolve(&env(&[]), Some(&h), DEFAULT_PROFILE).unwrap_err().sentence();
        assert!(said.contains("has no access key") && !said.contains("AKIDHALF"), "{said}");
    }
}
