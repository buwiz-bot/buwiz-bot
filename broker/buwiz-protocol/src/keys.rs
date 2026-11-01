//! Keys and digests of the mini app protocol (spec 4, design §5): ed25519
//! over JCS bytes, base64url without padding, sha256 as lowercase hex.
//!
//! A [`SessionKey`] is an open's own key pair. Its private half is made
//! here, signs here and is dropped (zeroed) with the open: it never
//! crosses into the app's JS, a log or the broker. It has no `Debug` and
//! no way to export the seed.

use base64::Engine as _;
use ed25519_dalek::Signer as _;
use sha2::Digest as _;

/// base64url without padding.
pub fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Decodes base64url without padding (padding or another alphabet is
/// refused: one encoding, one form).
pub fn unb64(text: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .map_err(|e| format!("bad base64url: {e}"))
}

/// Standard base64 with padding: file bytes for the app's JS decoder.
pub fn b64_std(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// `n` bytes from the OS CSPRNG.
pub fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b).expect("the OS random source");
    b
}

/// A random 128-bit id (`open_id`, nonce), base64url.
pub fn random_id() -> String {
    b64(&random::<16>())
}

/// Lowercase hex of SHA-256 over `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// `perms_sha256`: SHA-256 of JCS(permissions); absent permissions are
/// `{}` (as the broker computes it).
pub fn perms_sha256(permissions: Option<&serde_json::Value>) -> Result<String, String> {
    let empty = serde_json::json!({});
    let bytes = super::jcs::signed_bytes(permissions.unwrap_or(&empty))?;
    Ok(sha256_hex(&bytes))
}

/// An open's session key pair.
pub struct SessionKey(ed25519_dalek::SigningKey);

impl SessionKey {
    /// A fresh key pair from the OS CSPRNG.
    pub fn generate() -> SessionKey {
        let mut seed = random::<32>();
        let key = ed25519_dalek::SigningKey::from_bytes(&seed);
        seed.fill(0);
        SessionKey(key)
    }

    /// The key pair of a stored seed (a plugin's key, spec 4.3): base64url
    /// of exactly 32 bytes, in its one canonical form.
    pub fn from_seed(seed: &str) -> Result<SessionKey, String> {
        let bytes = unb64(seed)?;
        let mut arr: [u8; 32] = bytes.as_slice().try_into().map_err(|_| "a key seed is 32 bytes".to_owned())?;
        if b64(&arr) != seed {
            arr.fill(0);
            return Err("a key seed is 32 bytes of base64url".to_owned());
        }
        let key = ed25519_dalek::SigningKey::from_bytes(&arr);
        arr.fill(0);
        Ok(SessionKey(key))
    }

    /// The public half, base64url (`ks_pub`).
    pub fn public(&self) -> String {
        b64(self.0.verifying_key().as_bytes())
    }

    /// The signature over JCS(`v`), base64url.
    pub fn sign(&self, v: &serde_json::Value) -> Result<String, String> {
        let bytes = super::jcs::signed_bytes(v)?;
        Ok(self.sign_bytes(&bytes))
    }

    /// The signature over `bytes`, base64url.
    pub fn sign_bytes(&self, bytes: &[u8]) -> String {
        b64(&self.0.sign(bytes).to_bytes())
    }
}

/// Whether `sig` (base64url) is `public`'s (base64url) signature over
/// JCS(`v`).
pub fn verify(public: &str, v: &serde_json::Value, sig: &str) -> Result<(), String> {
    let public: [u8; 32] = unb64(public)?.try_into().map_err(|_| "a public key is 32 bytes".to_owned())?;
    let key = ed25519_dalek::VerifyingKey::from_bytes(&public).map_err(|e| e.to_string())?;
    let sig: [u8; 64] = unb64(sig)?.try_into().map_err(|_| "a signature is 64 bytes".to_owned())?;
    let bytes = super::jcs::signed_bytes(v)?;
    key.verify_strict(&bytes, &ed25519_dalek::Signature::from_bytes(&sig))
        .map_err(|_| "the signature does not verify".to_owned())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn base64url_has_no_padding_and_refuses_other_forms() {
        assert_eq!(super::b64(&[0xfb, 0xff]), "-_8");
        assert_eq!(super::unb64("-_8").unwrap(), [0xfb, 0xff]);
        assert!(super::unb64("-_8=").is_err());
        assert!(super::unb64("+/8").is_err());
        assert_eq!(super::random_id().len(), 22);
        assert_ne!(super::random_id(), super::random_id());
    }

    #[test]
    fn a_session_key_signs_what_its_public_half_verifies() {
        let k = super::SessionKey::generate();
        let v = json!({"b": 1, "a": "x"});
        let sig = k.sign(&v).unwrap();
        assert_eq!(super::unb64(&sig).unwrap().len(), 64);
        super::verify(&k.public(), &v, &sig).unwrap();
        // key order does not matter (JCS); content does
        super::verify(&k.public(), &json!({"a": "x", "b": 1}), &sig).unwrap();
        assert!(super::verify(&k.public(), &json!({"a": "x", "b": 2}), &sig).is_err());
        assert!(super::SessionKey::from_seed(&super::b64(&[7; 32])).is_ok());
        assert!(super::SessionKey::from_seed(&super::b64(&[7; 31])).is_err());
        // a non-canonical last character decodes to the same bytes: refused
        let mut odd = super::b64(&[7; 32]);
        odd.pop();
        odd.push('d');
        assert!(super::SessionKey::from_seed(&odd).is_err());
        let other = super::SessionKey::generate();
        assert!(super::verify(&other.public(), &v, &sig).is_err());
        assert!(k.sign(&json!({"a": 0.5})).is_err());
    }

    #[test]
    fn the_permissions_digest_is_sha256_of_jcs() {
        // sha256("{}")
        assert_eq!(
            super::perms_sha256(None).unwrap(),
            "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
        );
        let p = json!({"tools": ["fx_echo"], "copy": true});
        let jcs = r#"{"copy":true,"tools":["fx_echo"]}"#;
        assert_eq!(super::perms_sha256(Some(&p)).unwrap(), super::sha256_hex(jcs.as_bytes()));
    }
}
