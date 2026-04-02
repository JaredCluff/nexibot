//! AES-256-GCM encryption for session transcripts with Argon2id key derivation.
//!
//! Each encrypted line is stored as base64-encoded `salt (16) || nonce (12) || ciphertext`.
//! The salt is used with Argon2id to derive a 32-byte key from the passphrase.
//! The nonce is randomly generated per encryption call.

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};
use zeroize::Zeroizing;

/// Salt length in bytes for Argon2id key derivation.
const SALT_LEN: usize = 16;
/// Nonce length in bytes for AES-256-GCM.
const NONCE_LEN: usize = 12;
/// Derived key length in bytes (AES-256).
const KEY_LEN: usize = 32;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Configuration for session transcript encryption.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionEncryptionConfig {
    /// Whether encryption is enabled.
    pub enabled: bool,
    /// Optional keyring key where the passphrase is stored.
    pub passphrase_keyring_key: Option<String>,
}

// ---------------------------------------------------------------------------
// Encryptor
// ---------------------------------------------------------------------------

/// Encrypts and decrypts session transcript lines using AES-256-GCM.
///
/// Key derivation uses Argon2id with a random salt per encryption.
/// Each encrypted line includes its own salt and nonce, so lines can be
/// decrypted independently.
pub struct SessionEncryptor {
    /// Raw passphrase bytes, memory-zeroed on drop via `Zeroizing`.
    passphrase: Zeroizing<Vec<u8>>,
    /// Whether encryption is active.
    enabled: bool,
    /// Whether encryption was requested but had to be disabled due to a keyring
    /// or configuration error. The UI should check this flag and warn the user
    /// that their session transcripts are not being encrypted.
    degraded: bool,
}

impl SessionEncryptor {
    /// Create a new encryptor from a passphrase.
    ///
    /// If `enabled` is `false`, `encrypt_line` and `decrypt_line` are
    /// pass-throughs.
    pub fn new(passphrase: &str, enabled: bool) -> Self {
        debug!("[SESSION_ENCRYPTION] Initialised (enabled: {})", enabled,);
        Self {
            passphrase: Zeroizing::new(passphrase.as_bytes().to_vec()),
            enabled,
            degraded: false,
        }
    }

    /// Create a disabled (no-op) encryptor.
    pub fn disabled() -> Self {
        Self {
            passphrase: Zeroizing::new(Vec::new()),
            enabled: false,
            degraded: false,
        }
    }

    /// Create a degraded encryptor: encryption was requested but could not be
    /// initialized (e.g. keyring failure). Behaves like `disabled()` but the
    /// `is_degraded()` flag is set so the UI can warn the user.
    pub fn disabled_degraded() -> Self {
        Self {
            passphrase: Zeroizing::new(Vec::new()),
            enabled: false,
            degraded: true,
        }
    }

    /// Whether encryption is currently enabled.
    #[allow(dead_code)]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Whether encryption was requested but had to be disabled due to an error.
    /// The UI should check this flag and display a security warning to the user.
    #[allow(dead_code)]
    pub fn is_degraded(&self) -> bool {
        self.degraded
    }

    /// Build an Argon2id instance with explicit OWASP-recommended parameters
    /// for interactive logins: m=19 MiB, t=2 iterations, p=1 parallelism.
    fn argon2() -> anyhow::Result<Argon2<'static>> {
        let params = Params::new(
            19 * 1024, // m_cost: 19 MiB in KiB
            2,         // t_cost: 2 iterations
            1,         // p_cost: 1 parallelism
            None,      // output_len: default
        )
        .map_err(|e| anyhow::anyhow!("Invalid Argon2 params: {}", e))?;
        Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
    }

    /// Build an Argon2id instance with the legacy default parameters that were
    /// used before the OWASP-recommended parameters were adopted. Needed to
    /// decrypt vault entries that were encrypted with the old defaults.
    fn argon2_legacy() -> Argon2<'static> {
        Argon2::default()
    }

    /// Derive a 32-byte key from passphrase + salt using Argon2id.
    fn derive_key(passphrase: &[u8], salt: &[u8]) -> anyhow::Result<[u8; KEY_LEN]> {
        let mut key = [0u8; KEY_LEN];
        Self::argon2()?
            .hash_password_into(passphrase, salt, &mut key)
            .map_err(|e| anyhow::anyhow!("Argon2id key derivation failed: {}", e))?;
        Ok(key)
    }

    /// Derive a 32-byte key using the legacy Argon2 default parameters.
    fn derive_key_legacy(passphrase: &[u8], salt: &[u8]) -> anyhow::Result<[u8; KEY_LEN]> {
        let mut key = [0u8; KEY_LEN];
        Self::argon2_legacy()
            .hash_password_into(passphrase, salt, &mut key)
            .map_err(|e| anyhow::anyhow!("Argon2id legacy key derivation failed: {}", e))?;
        Ok(key)
    }

    /// Encrypt a plaintext line and return a base64-encoded ciphertext string.
    ///
    /// Format: base64(salt[16] || nonce[12] || aes256gcm_ciphertext)
    pub fn encrypt_line(&self, plaintext: &str) -> anyhow::Result<String> {
        if !self.enabled {
            return Ok(plaintext.to_string());
        }
        if self.passphrase.is_empty() {
            warn!("[SESSION_ENCRYPTION] Encryption enabled but passphrase is empty");
            return Err(anyhow::anyhow!("Encryption passphrase is empty"));
        }

        // Generate random salt and nonce
        let mut salt = [0u8; SALT_LEN];
        OsRng.fill_bytes(&mut salt);
        let mut nonce_bytes = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce_bytes);

        // Derive key from passphrase + salt
        let key = Self::derive_key(&self.passphrase, &salt)?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|e| anyhow::anyhow!("Failed to create AES-256-GCM cipher: {}", e))?;
        let nonce = Nonce::from_slice(&nonce_bytes);

        // Encrypt
        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| anyhow::anyhow!("AES-256-GCM encryption failed: {}", e))?;

        // Concatenate: salt || nonce || ciphertext
        let mut output = Vec::with_capacity(SALT_LEN + NONCE_LEN + ciphertext.len());
        output.extend_from_slice(&salt);
        output.extend_from_slice(&nonce_bytes);
        output.extend_from_slice(&ciphertext);

        Ok(BASE64.encode(&output))
    }

    /// Decrypt a base64-encoded ciphertext line back to plaintext.
    ///
    /// Expects format: base64(salt[16] || nonce[12] || aes256gcm_ciphertext)
    pub fn decrypt_line(&self, ciphertext: &str) -> anyhow::Result<String> {
        if !self.enabled {
            return Ok(ciphertext.to_string());
        }
        if self.passphrase.is_empty() {
            warn!("[SESSION_ENCRYPTION] Decryption enabled but passphrase is empty");
            return Err(anyhow::anyhow!("Encryption passphrase is empty"));
        }

        // Stage 1: base64 decode
        let data = BASE64
            .decode(ciphertext)
            .map_err(|e| anyhow::anyhow!("Decryption failed at base64 decode stage: {}", e))?;

        let min_len = SALT_LEN + NONCE_LEN + 16; // 16 = GCM auth tag
        if data.len() < min_len {
            return Err(anyhow::anyhow!(
                "Decryption failed at nonce extraction stage: ciphertext too short ({} bytes, minimum {})",
                data.len(),
                min_len
            ));
        }

        // Stage 2: split salt || nonce || ciphertext
        let salt = &data[..SALT_LEN];
        let nonce_bytes = &data[SALT_LEN..SALT_LEN + NONCE_LEN];
        let encrypted = &data[SALT_LEN + NONCE_LEN..];

        // Derive key from passphrase + salt
        let key = Self::derive_key(&self.passphrase, salt)?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|e| anyhow::anyhow!("Failed to create AES-256-GCM cipher: {}", e))?;
        let nonce = Nonce::from_slice(nonce_bytes);

        // Stage 3: AES-256-GCM authentication tag verification + decrypt
        let plaintext_bytes = cipher.decrypt(nonce, encrypted).map_err(|_| {
            anyhow::anyhow!(
                "Decryption failed at GCM authentication tag verification stage: \
                 wrong passphrase or data has been tampered with"
            )
        })?;

        String::from_utf8(plaintext_bytes)
            .map_err(|e| anyhow::anyhow!("Decrypted data is not valid UTF-8: {}", e))
    }

    /// Attempt decryption using the legacy Argon2 default parameters.
    ///
    /// This is used as a fallback when `decrypt_line` fails, to handle vault
    /// entries that were encrypted before the Argon2 parameters were changed
    /// to OWASP-recommended values.
    pub fn decrypt_line_legacy(&self, ciphertext: &str) -> anyhow::Result<String> {
        if !self.enabled {
            return Ok(ciphertext.to_string());
        }
        if self.passphrase.is_empty() {
            return Err(anyhow::anyhow!("Encryption passphrase is empty"));
        }

        let data = BASE64
            .decode(ciphertext)
            .map_err(|e| anyhow::anyhow!("Legacy decryption failed at base64 decode: {}", e))?;

        let min_len = SALT_LEN + NONCE_LEN + 16;
        if data.len() < min_len {
            return Err(anyhow::anyhow!(
                "Legacy decryption: ciphertext too short ({} bytes, minimum {})",
                data.len(),
                min_len
            ));
        }

        let salt = &data[..SALT_LEN];
        let nonce_bytes = &data[SALT_LEN..SALT_LEN + NONCE_LEN];
        let encrypted = &data[SALT_LEN + NONCE_LEN..];

        let key = Self::derive_key_legacy(&self.passphrase, salt)?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|e| anyhow::anyhow!("Failed to create AES-256-GCM cipher: {}", e))?;
        let nonce = Nonce::from_slice(nonce_bytes);

        let plaintext_bytes = cipher.decrypt(nonce, encrypted).map_err(|_| {
            anyhow::anyhow!("Legacy decryption failed at GCM authentication tag verification")
        })?;

        String::from_utf8(plaintext_bytes)
            .map_err(|e| anyhow::anyhow!("Decrypted data is not valid UTF-8: {}", e))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip_encrypt_decrypt() {
        let enc = SessionEncryptor::new("super-secret-passphrase", true);
        let original = "This is a secret session transcript line.";
        let ciphertext = enc.encrypt_line(original).unwrap();
        assert_ne!(ciphertext, original);
        let decrypted = enc.decrypt_line(&ciphertext).unwrap();
        assert_eq!(decrypted, original);
    }

    #[test]
    fn test_different_passphrases_produce_different_ciphertext() {
        let enc_a = SessionEncryptor::new("passphrase_a", true);
        let enc_b = SessionEncryptor::new("passphrase_b", true);
        let original = "Hello, world!";
        let ct_a = enc_a.encrypt_line(original).unwrap();
        let ct_b = enc_b.encrypt_line(original).unwrap();
        assert_ne!(ct_a, ct_b);
    }

    #[test]
    fn test_wrong_passphrase_fails_decrypt() {
        let enc_a = SessionEncryptor::new("correct_passphrase", true);
        let enc_b = SessionEncryptor::new("wrong_passphrase", true);
        let original = "Secret data";
        let ct = enc_a.encrypt_line(original).unwrap();
        assert!(enc_b.decrypt_line(&ct).is_err());
    }

    #[test]
    fn test_disabled_is_passthrough() {
        let enc = SessionEncryptor::new("passphrase", false);
        let original = "plain text";
        let result = enc.encrypt_line(original).unwrap();
        assert_eq!(result, original);
        let decrypted = enc.decrypt_line(original).unwrap();
        assert_eq!(decrypted, original);
    }

    #[test]
    fn test_disabled_constructor() {
        let enc = SessionEncryptor::disabled();
        assert!(!enc.is_enabled());
        assert_eq!(enc.encrypt_line("hello").unwrap(), "hello");
    }

    #[test]
    fn test_empty_string_roundtrip() {
        let enc = SessionEncryptor::new("key", true);
        let ct = enc.encrypt_line("").unwrap();
        let pt = enc.decrypt_line(&ct).unwrap();
        assert_eq!(pt, "");
    }

    #[test]
    fn test_unicode_roundtrip() {
        let enc = SessionEncryptor::new("key", true);
        let original = "Hello, world -- some unicode symbols: \u{1F600}\u{1F4A9}\u{2603}";
        let ct = enc.encrypt_line(original).unwrap();
        let pt = enc.decrypt_line(&ct).unwrap();
        assert_eq!(pt, original);
    }

    #[test]
    fn test_same_plaintext_different_ciphertexts() {
        // Due to random salt + nonce, encrypting the same plaintext twice produces different results
        let enc = SessionEncryptor::new("key", true);
        let original = "Same plaintext";
        let ct1 = enc.encrypt_line(original).unwrap();
        let ct2 = enc.encrypt_line(original).unwrap();
        assert_ne!(ct1, ct2);
        // But both decrypt to the same plaintext
        assert_eq!(enc.decrypt_line(&ct1).unwrap(), original);
        assert_eq!(enc.decrypt_line(&ct2).unwrap(), original);
    }

    #[test]
    fn test_invalid_base64_returns_error() {
        let enc = SessionEncryptor::new("key", true);
        let result = enc.decrypt_line("not-valid-base64!!!");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("base64 decode stage"), "Expected base64 stage error, got: {}", err);
    }

    #[test]
    fn test_truncated_ciphertext_returns_error() {
        let enc = SessionEncryptor::new("key", true);
        // Valid base64 but too short for salt + nonce + auth tag
        let result = enc.decrypt_line(&BASE64.encode([0u8; 10]));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("nonce extraction stage"), "Expected nonce stage error, got: {}", err);
    }

    #[test]
    fn test_tampered_ciphertext_returns_error() {
        let enc = SessionEncryptor::new("key", true);
        let ct = enc.encrypt_line("secret").unwrap();
        let mut data = BASE64.decode(&ct).unwrap();
        // Flip a byte in the ciphertext portion
        if let Some(last) = data.last_mut() {
            *last ^= 0xFF;
        }
        let tampered = BASE64.encode(&data);
        let result = enc.decrypt_line(&tampered);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("GCM authentication tag verification stage"),
            "Expected GCM auth tag stage error, got: {}",
            err
        );
    }

    #[test]
    fn test_config_defaults() {
        let config = SessionEncryptionConfig::default();
        assert!(!config.enabled);
        assert!(config.passphrase_keyring_key.is_none());
    }

    #[test]
    fn test_wrong_passphrase_error_mentions_gcm_stage() {
        let enc_a = SessionEncryptor::new("correct_passphrase", true);
        let enc_b = SessionEncryptor::new("wrong_passphrase", true);
        let ct = enc_a.encrypt_line("sensitive data").unwrap();
        let err = enc_b.decrypt_line(&ct).unwrap_err().to_string();
        assert!(
            err.contains("GCM authentication tag verification stage"),
            "Expected GCM stage error for wrong passphrase, got: {}",
            err
        );
    }
}
