//! PKCE (Proof Key for Code Exchange) utilities for OAuth 2.0
//!
//! Implements RFC 7636: generates a random verifier and derives the
//! S256 challenge sent to the authorization endpoint.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sha2::{Digest, Sha256};

/// Generate a random PKCE code verifier (128 characters, base64url).
pub fn generate_verifier() -> String {
    let bytes: Vec<u8> = (0..96).map(|_| rand::random::<u8>()).collect();
    URL_SAFE_NO_PAD.encode(&bytes)
}

/// Derive the PKCE S256 code challenge from a verifier.
pub fn challenge_from_verifier(verifier: &str) -> String {
    let hash = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verifier_length() {
        let v = generate_verifier();
        assert_eq!(v.len(), 128);
    }

    #[test]
    fn test_challenge_is_base64url() {
        let v = generate_verifier();
        let c = challenge_from_verifier(&v);
        assert!(!c.is_empty());
        assert!(!c.contains('+'));
        assert!(!c.contains('/'));
        assert!(!c.contains('='));
    }

    #[test]
    fn test_challenge_is_deterministic() {
        let v = generate_verifier();
        let c1 = challenge_from_verifier(&v);
        let c2 = challenge_from_verifier(&v);
        assert_eq!(c1, c2);
    }
}
