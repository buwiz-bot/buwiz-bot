//! The broker's proof of identity (`POST /v1/host/prove`): before a saved
//! login leaves the app, the app sends a fresh nonce and the broker
//! answers with an HMAC of it keyed by the host token it wrote (0600).
//! Only the process that made the token can answer, so a stranger bound
//! to the broker's port gets nothing; the request carries no token, so an
//! unproven server learns none.

use sha2::{Digest as _, Sha256};

/// What the HMAC covers before the nonce: a proof made for this purpose
/// can't be replayed as any other MAC under the token.
pub const PROOF_CONTEXT: &str = "buwiz-broker-proof:";

/// The shortest and longest nonce a broker answers (base64url characters:
/// 16 to 96 bytes). The app sends 32 random bytes, 43 characters.
pub const NONCE_MIN: usize = 22;
pub const NONCE_MAX: usize = 128;

/// HMAC-SHA256 of `msg` under `key` (RFC 2104).
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let pad = |x: u8| k.iter().map(|b| b ^ x).collect::<Vec<u8>>();
    let inner = Sha256::new().chain_update(pad(0x36)).chain_update(msg).finalize();
    Sha256::new().chain_update(pad(0x5c)).chain_update(inner).finalize().into()
}

/// Whether `nonce` is one a broker answers: base64url without padding,
/// [`NONCE_MIN`] to [`NONCE_MAX`] characters.
pub fn valid_nonce(nonce: &str) -> bool {
    (NONCE_MIN..=NONCE_MAX).contains(&nonce.len())
        && nonce.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The proof for `nonce` under the host token `token`:
/// base64url(HMAC-SHA256(token, "buwiz-broker-proof:" + nonce)).
pub fn broker_proof(token: &str, nonce: &str) -> String {
    let msg = format!("{PROOF_CONTEXT}{nonce}");
    super::keys::b64(&hmac_sha256(token.as_bytes(), msg.as_bytes()))
}

/// Byte equality in time that depends on the length only; empty never
/// matches.
pub fn same(got: &[u8], want: &[u8]) -> bool {
    !want.is_empty() && got.len() == want.len() && got.iter().zip(want).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

/// Whether `proof` is the answer to `nonce` from the holder of `token`
/// (constant time).
pub fn proof_matches(token: &str, nonce: &str, proof: &str) -> bool {
    !token.is_empty() && valid_nonce(nonce) && same(proof.as_bytes(), broker_proof(token, nonce).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_sha256_matches_rfc_4231() {
        let hex = |b: [u8; 32]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        assert_eq!(hex(hmac_sha256(b"Jefe", b"what do ya want for nothing?")), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        assert_eq!(
            hex(hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn a_proof_matches_only_its_token_and_nonce() {
        let nonce = crate::keys::b64(&[7u8; 32]);
        assert_eq!(nonce.len(), 43);
        let proof = broker_proof("token-a", &nonce);
        assert_eq!(proof.len(), 43);
        assert_eq!(proof, crate::keys::b64(&hmac_sha256(b"token-a", format!("buwiz-broker-proof:{nonce}").as_bytes())));
        assert!(proof_matches("token-a", &nonce, &proof));
        assert!(!proof_matches("token-b", &nonce, &proof));
        assert!(!proof_matches("token-a", &crate::keys::b64(&[8u8; 32]), &proof));
        assert!(!proof_matches("", &nonce, &broker_proof("", &nonce)));
        assert!(!proof_matches("token-a", &nonce, ""));
    }

    #[test]
    fn nonces_are_bounded_base64url() {
        assert!(valid_nonce(&"a".repeat(NONCE_MIN)));
        assert!(valid_nonce(&"A-_9".repeat(32)));
        assert!(!valid_nonce(&"a".repeat(NONCE_MIN - 1)));
        assert!(!valid_nonce(&"a".repeat(NONCE_MAX + 1)));
        assert!(!valid_nonce(&format!("{}=", "a".repeat(30))));
        assert!(!valid_nonce(&format!("{}/", "a".repeat(30))));
        assert!(!valid_nonce(""));
    }
}
