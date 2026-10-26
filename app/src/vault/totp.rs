//! Authenticator codes: an `otpauth://totp/…` seed and RFC 6238 over
//! HMAC-SHA1, small enough to keep here rather than pull a crate in.
//! Only SHA1 seeds mint (what nearly every site hands out); a SHA256/512
//! seed is refused with a sentence.
//!
//! A code is minted at the moment it is sent: when the current step has
//! less than [`MIN_TTL_SECONDS`] left, the send waits for the next one so
//! the digits are still good when they arrive.

use std::time::{SystemTime, UNIX_EPOCH};

/// A step with fewer seconds than this left is minted on the next step.
pub(crate) const MIN_TTL_SECONDS: u64 = 8;

/// A parsed seed. Its `Debug` hides the key.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Totp {
    key: Vec<u8>,
    digits: u32,
    period: u64,
}

impl std::fmt::Debug for Totp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Totp")
            .field("key", &"<redacted>")
            .field("digits", &self.digits)
            .field("period", &self.period)
            .finish()
    }
}

impl Totp {
    /// An `otpauth://totp/...` URI, or a bare base32 secret.
    pub(crate) fn parse(uri: &str) -> Result<Self, String> {
        let uri = uri.trim();
        let (secret, query) = if let Some(rest) = uri.strip_prefix("otpauth://") {
            if !rest.to_ascii_lowercase().starts_with("totp/") {
                return Err("not an authenticator seed: only time-based codes are supported".into());
            }
            let query = rest.split_once('?').map(|(_, q)| q).unwrap_or("");
            let secret = param(query, "secret").ok_or("not an authenticator seed: no secret")?;
            (secret, query)
        } else {
            (uri.to_owned(), "")
        };
        if let Some(alg) = param(query, "algorithm")
            && !alg.eq_ignore_ascii_case("SHA1")
        {
            return Err(format!("{alg} authenticator codes are not supported yet"));
        }
        let key = base32(&secret).ok_or("not an authenticator seed: the secret is not base32")?;
        if key.is_empty() {
            return Err("not an authenticator seed: the secret is empty".into());
        }
        let digits = param(query, "digits").and_then(|d| d.parse().ok()).filter(|d| (6..=9).contains(d)).unwrap_or(6);
        let period = param(query, "period").and_then(|p| p.parse().ok()).filter(|p| *p > 0).unwrap_or(30);
        Ok(Self { key, digits, period })
    }

    /// The code at `unix_seconds`, zero-padded.
    pub(crate) fn at(&self, unix_seconds: u64) -> String {
        let counter = (unix_seconds / self.period).to_be_bytes();
        let mac = hmac_sha1(&self.key, &counter);
        let off = (mac[19] & 0x0f) as usize;
        let bin = u32::from_be_bytes([mac[off] & 0x7f, mac[off + 1], mac[off + 2], mac[off + 3]]);
        let code = bin % 10u32.pow(self.digits);
        format!("{code:0width$}", width = self.digits as usize)
    }

    /// The seconds left in the current step.
    pub(crate) fn ttl(&self) -> u64 {
        self.period - now_seconds() % self.period
    }

    /// The current code and the seconds it has left.
    pub(crate) fn current(&self) -> (String, u64) {
        (self.at(now_seconds()), self.ttl())
    }

    /// The seconds to wait before minting: none on a fresh step, the rest
    /// of the step when it is nearly over.
    pub(crate) fn wait_before_minting(&self) -> u64 {
        let ttl = self.ttl();
        if ttl < MIN_TTL_SECONDS { ttl } else { 0 }
    }
}

pub(crate) fn now_seconds() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A query parameter, percent-decoded.
fn param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        k.eq_ignore_ascii_case(name).then(|| crate::client::urldec(v))
    })
}

/// RFC 4648 base32, case-insensitive, padding and spaces ignored.
fn base32(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut buf, mut bits) = (0u64, 0u32);
    for c in s.chars().filter(|c| !matches!(c, '=' | ' ' | '-')) {
        let v = match c.to_ascii_uppercase() {
            c @ 'A'..='Z' => c as u64 - 'A' as u64,
            c @ '2'..='7' => c as u64 - '2' as u64 + 26,
            _ => return None,
        };
        buf = (buf << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..20].copy_from_slice(&sha1(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    inner.extend_from_slice(msg);
    let mut outer: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    outer.extend_from_slice(&sha1(&inner));
    sha1(&outer)
}

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (i, x) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rfc_6238_sha1_vectors_mint() {
        // the 20-byte seed "12345678901234567890"
        let rfc = Totp::parse("otpauth://totp/x?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&digits=8").expect("seed");
        assert_eq!(rfc.at(59), "94287082");
        assert_eq!(rfc.at(1111111109), "07081804");
        assert_eq!(rfc.at(20000000000), "65353130");
    }

    #[test]
    fn a_short_seed_with_an_issuer_parses_and_other_algorithms_are_refused() {
        let short = Totp::parse("otpauth://totp/GitHub:ada%40example.com?secret=JBSWY3DPEHPK3PXP&issuer=GitHub").expect("seed");
        assert_eq!(short.at(0).len(), 6);
        assert!(!format!("{short:?}").contains("JBSWY3DP"));
        assert!(Totp::parse("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&algorithm=SHA256").is_err());
        assert!(Totp::parse("otpauth://hotp/x?secret=JBSWY3DPEHPK3PXP").is_err());
        assert!(Totp::parse("not base32!").is_err());
    }
}
