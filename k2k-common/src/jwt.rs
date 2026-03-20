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
    validation.set_audience(&[] as &[&str]);
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
    use base64::Engine;
    use rsa::pkcs1::{EncodeRsaPrivateKey, EncodeRsaPublicKey};
    use rsa::RsaPrivateKey;
    use sha2::{Digest, Sha256};

    #[test]
    fn test_jwt_verify() {
        // Generate test keypair
        let mut rng = rand::thread_rng();
        let private_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let public_key = private_key.to_public_key();
        let public_pem = public_key.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF).unwrap();

        // Create a valid JWT
        let now = chrono::Utc::now().timestamp();
        let header = serde_json::json!({"alg": "RS256", "typ": "JWT"});
        let claims = serde_json::json!({
            "iss": "kb:test-store",
            "client_id": "test-client",
            "iat": now,
            "exp": now + 300,
            "transfer_id": "test123"
        });

        let header_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_string(&header).unwrap());
        let claims_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_string(&claims).unwrap());

        let message = format!("{}.{}", header_b64, claims_b64);

        // Hash the message
        let mut hasher = Sha256::new();
        hasher.update(message.as_bytes());
        let hash = hasher.finalize();

        // Sign with RSA private key
        use rsa::pkcs1v15::Pkcs1v15Sign;
        let signature = private_key
            .sign(Pkcs1v15Sign::new::<Sha256>(), &hash)
            .unwrap();
        let signature_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature);

        let jwt = format!("{}.{}", message, signature_b64);

        // Verify JWT
        let result = verify_k2k_jwt(&jwt, &public_pem);
        assert!(result.is_ok());
        let verified_claims = result.unwrap();
        assert_eq!(verified_claims.client_id, "test-client");
    }
}
