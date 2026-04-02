//! JWT utilities for K2K authentication

use crate::models::K2KClaims;
use anyhow::{Context, Result};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};

/// Verify RS256 JWT signature using RSA public key
pub fn verify_k2k_jwt(jwt: &str, public_key_pem: &str) -> Result<K2KClaims> {
    let decoding_key = DecodingKey::from_rsa_pem(public_key_pem.as_bytes())
        .context("Failed to create decoding key from public key")?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.validate_exp = false; // We'll validate manually for better error messages
    validation.validate_nbf = false;
    validation.validate_aud = false;
    validation.set_required_spec_claims(&["iss", "exp", "iat"]);

    // Verify signature
    let token_data = decode::<K2KClaims>(jwt, &decoding_key, &validation)
        .context("Signature verification failed")?;

    let claims = token_data.claims;

    // Manual expiration validation with better error message
    let now = chrono::Utc::now().timestamp();
    if claims.exp < now {
        anyhow::bail!("JWT expired (exp: {}, now: {})", claims.exp, now);
    }

    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use rsa::pkcs1::EncodeRsaPublicKey;
    use rsa::pkcs8::EncodePrivateKey;
    use rsa::RsaPrivateKey;

    #[test]
    fn test_jwt_verify() {
        // Generate test keypair
        let mut rng = rand::thread_rng();
        let private_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let public_key = private_key.to_public_key();
        let public_pem = public_key
            .to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
            .unwrap();
        let private_pem = private_key
            .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
            .unwrap();

        let now = chrono::Utc::now().timestamp();
        let claims = K2KClaims {
            iss: "kb:test-store".to_string(),
            aud: "kb:test-store".to_string(),
            source_kb_id: "test-store".to_string(),
            iat: now,
            exp: now + 300,
            jti: "test-jti-001".to_string(),
            transfer_id: "test123".to_string(),
            client_id: "test-client".to_string(),
        };

        let encoding_key = EncodingKey::from_rsa_pem(private_pem.as_bytes()).unwrap();
        let jwt = encode(&Header::new(Algorithm::RS256), &claims, &encoding_key).unwrap();

        let result = verify_k2k_jwt(&jwt, &public_pem);
        assert!(result.is_ok(), "JWT verification failed: {:?}", result.err());
        let verified_claims = result.unwrap();
        assert_eq!(verified_claims.client_id, "test-client");
        assert_eq!(verified_claims.transfer_id, "test123");
    }
}
