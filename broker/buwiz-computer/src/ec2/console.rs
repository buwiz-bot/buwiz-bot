//! The instance's SSH host key, from its serial console as EC2's `GetConsoleOutput` gives it.
//!
//! At its first boot cloud-init prints the host keys it made (`-----BEGIN SSH HOST KEY KEYS-----`)
//! and their fingerprints. That output reaches the broker through the AWS API, signed and over TLS,
//! not through the instance's network: a machine that answers on its address can't forge it. It
//! is read before anything of ours runs on the instance, and pinned for good, so nothing that runs
//! there later can change what is trusted.
//!
//! One ed25519 key, or no key: two different ones anywhere in the output, or a fingerprint line
//! that doesn't match it, and nothing is pinned.

use base64::Engine;
use sha2::{Digest, Sha256};

const KEYS_BEGIN: &str = "-----BEGIN SSH HOST KEY KEYS-----";
const KEYS_END: &str = "-----END SSH HOST KEY KEYS-----";
const KIND: &str = "ssh-ed25519";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Read {
    /// `ssh-ed25519 <key>`
    Key(String),
    /// it hasn't printed its keys yet
    NotYet,
    /// what it printed doesn't hold together: nothing is trusted
    Doubtful(&'static str),
}

/// The pin in `console`.
pub fn host_key(console: &str) -> Read {
    let mut keys: Vec<String> = Vec::new();
    let mut inside = false;
    let mut blocks = 0;
    for line in console.lines() {
        if line.contains(KEYS_BEGIN) {
            inside = true;
            blocks += 1;
            continue;
        }
        if line.contains(KEYS_END) {
            inside = false;
            continue;
        }
        if inside {
            // a prefix (`ec2: `, a timestamp) may come first: the key starts at its type
            if let Some(at) = line.find(&format!("{KIND} ")) {
                let key = line[at + KIND.len() + 1..].split_whitespace().next().unwrap_or_default();
                if base64::engine::general_purpose::STANDARD.decode(key).is_err() || key.is_empty() {
                    return Read::Doubtful("a host key line isn't a key");
                }
                let pin = format!("{KIND} {key}");
                if !keys.contains(&pin) {
                    keys.push(pin);
                }
            }
        }
    }
    if blocks == 0 || inside {
        return Read::NotYet;
    }
    let pin = match keys.as_slice() {
        [] => return Read::Doubtful("it printed no ed25519 host key"),
        [one] => one.clone(),
        _ => return Read::Doubtful("it printed two different ed25519 host keys"),
    };
    // every ED25519 fingerprint it printed is this key's
    let mine = fingerprint(&pin);
    for line in console.lines().filter(|l| l.contains("(ED25519)")) {
        if let Some(said) = line.split_whitespace().find(|w| w.starts_with("SHA256:")) {
            if Some(said) != mine.as_deref() {
                return Read::Doubtful("a fingerprint it printed isn't its key's");
            }
        }
    }
    Read::Key(pin)
}

/// `SHA256:…` as `ssh-keygen -l` prints it.
fn fingerprint(pin: &str) -> Option<String> {
    let raw = base64::engine::general_purpose::STANDARD.decode(pin.split_once(' ')?.1).ok()?;
    Some(format!("SHA256:{}", base64::engine::general_purpose::STANDARD_NO_PAD.encode(Sha256::digest(raw))))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
pub mod tests {
    use super::*;

    pub const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAGvgQ/uwCuNoi8TtSUfRbtFwCChFjTrqFTN6bC3Iq7L";
    const OTHER: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFMUQzIoUweaWFmCD2hUs96kUjz4ovO6pk0e+3uh7tou";

    /// A first boot's console, as Ubuntu's cloud-init prints it on EC2.
    pub fn console(key: &str) -> String {
        format!(
            "[   12.345678] cloud-init[812]: Cloud-init v. 24.4 finished\n\
             <14>Nov  9 10:15:01 ec2: \n\
             <14>Nov  9 10:15:01 ec2: #############################################################\n\
             <14>Nov  9 10:15:01 ec2: -----BEGIN SSH HOST KEY FINGERPRINTS-----\n\
             <14>Nov  9 10:15:01 ec2: 256 SHA256:4fpCq8lqL+4iJXvL0X0b3tXJSkS1zG5n2bQyZb6e3s0 root@ip-10-73-1-20 (ECDSA)\n\
             <14>Nov  9 10:15:01 ec2: 256 {} root@ip-10-73-1-20 (ED25519)\n\
             <14>Nov  9 10:15:01 ec2: -----END SSH HOST KEY FINGERPRINTS-----\n\
             <14>Nov  9 10:15:01 ec2: #############################################################\n\
             -----BEGIN SSH HOST KEY KEYS-----\n\
             ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTY= root@ip-10-73-1-20\n\
             {key} root@ip-10-73-1-20\n\
             -----END SSH HOST KEY KEYS-----\n\
             [   13.0] cloud-init[812]: Cloud-init v. 24.4 finished at Mon, 09 Nov 2026 10:15:01 +0000.\n",
            fingerprint(key).unwrap()
        )
    }

    #[test]
    fn the_ed25519_key_of_a_first_boot_is_read() {
        assert_eq!(host_key(&console(KEY)), Read::Key(KEY.to_owned()));
        // printed again at a later boot, the same key is still one key
        assert_eq!(host_key(&format!("{}{}", console(KEY), console(KEY))), Read::Key(KEY.to_owned()));
    }

    #[test]
    fn nothing_yet_is_a_wait_and_not_a_key() {
        assert_eq!(host_key(""), Read::NotYet);
        assert_eq!(host_key("booting\n-----BEGIN SSH HOST KEY KEYS-----\n"), Read::NotYet);
    }

    #[test]
    fn what_doesnt_hold_together_is_never_pinned() {
        assert!(matches!(host_key(&format!("{}{}", console(KEY), console(OTHER))), Read::Doubtful(_)), "two keys");
        let lying = console(KEY).replace(&fingerprint(KEY).unwrap(), &fingerprint(OTHER).unwrap());
        assert!(matches!(host_key(&lying), Read::Doubtful(_)), "a fingerprint of another key");
        assert!(matches!(host_key("-----BEGIN SSH HOST KEY KEYS-----\necdsa-sha2-nistp256 AAAA\n-----END SSH HOST KEY KEYS-----\n"), Read::Doubtful(_)));
        assert!(matches!(host_key("-----BEGIN SSH HOST KEY KEYS-----\nssh-ed25519 not*base64\n-----END SSH HOST KEY KEYS-----\n"), Read::Doubtful(_)));
    }
}
