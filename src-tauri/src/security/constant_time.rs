//! Constant-time string comparison to prevent timing attacks.
//!
//! Both inputs are HMAC-SHA256 digested with a process-lifetime secret key
//! before comparison.  HMAC produces a fixed-length (32-byte) MAC regardless
//! of input length, and the secret key hides the input content from an
//! attacker who can observe only timing.  The final comparison is done with
//! `ct_eq` from the `subtle` crate so the last step is also timing-safe.
//!
//! Why not plain SHA-256?  SHA-256 is *not* constant-time with respect to
//! input length — longer messages require more compression rounds, leaking
//! token length through CPU-cycle timing.  HMAC with a secret key closes
//! that channel because even an adversary who knows the HMAC algorithm
//! cannot compute a reference MAC without knowing the key.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::sync::OnceLock;
use subtle::ConstantTimeEq;

/// Process-lifetime secret key used only for comparison MAC operations.
/// Generated from the OS CSPRNG once per process; never persisted or logged.
static COMPARE_KEY: OnceLock<[u8; 32]> = OnceLock::new();

fn compare_key() -> &'static [u8; 32] {
    COMPARE_KEY.get_or_init(|| {
        use rand::RngCore;
        let mut key = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut key);
        key
    })
}

/// Compare two strings in constant time using HMAC-SHA256 pre-hashing.
///
/// HMAC-SHA256 normalises both inputs to 32-byte MACs, eliminating both the
/// length side-channel (all outputs are the same size) and the content
/// side-channel (the secret key prevents an attacker from predicting the MAC
/// value for any candidate input without timing the comparison).
pub fn secure_compare(a: &str, b: &str) -> bool {
    let key = compare_key();

    let mut mac_a = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac_a.update(a.as_bytes());
    let result_a = mac_a.finalize().into_bytes();

    let mut mac_b = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac_b.update(b.as_bytes());
    let result_b = mac_b.finalize().into_bytes();

    result_a.ct_eq(&result_b).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_equal_strings() {
        assert!(secure_compare("hello", "hello"));
        assert!(secure_compare("", ""));
        assert!(secure_compare(
            "a-long-secret-token-12345",
            "a-long-secret-token-12345"
        ));
    }

    #[test]
    fn test_unequal_strings() {
        assert!(!secure_compare("hello", "world"));
        assert!(!secure_compare("hello", "hell"));
        assert!(!secure_compare("a", "b"));
    }

    #[test]
    fn test_different_lengths() {
        assert!(!secure_compare("short", "longer-string"));
        assert!(!secure_compare("", "notempty"));
        assert!(!secure_compare("notempty", ""));
    }
}
