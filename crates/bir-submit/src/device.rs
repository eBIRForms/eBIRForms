//! The device key: an Ed25519 key made on first sign-in and kept in the
//! secret store. The app registers its public half, then signs every v2
//! request with it, DPoP-style, over the method, path, body hash, a
//! timestamp and a nonce. The server refuses a timestamp more than 60
//! seconds off and a nonce it has seen.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};

pub const HEADER_DEVICE: &str = "buwiz-device";
pub const HEADER_TIMESTAMP: &str = "buwiz-timestamp";
pub const HEADER_NONCE: &str = "buwiz-nonce";
pub const HEADER_SIGNATURE: &str = "buwiz-signature";

pub struct DeviceKey(SigningKey);

impl DeviceKey {
    pub fn generate() -> Self {
        Self(SigningKey::from_bytes(&rand::random::<[u8; 32]>()))
    }

    /// From the stored base64url seed.
    pub fn from_stored(raw: &str) -> Option<Self> {
        let seed: [u8; 32] = URL_SAFE_NO_PAD.decode(raw.trim()).ok()?.try_into().ok()?;
        Some(Self(SigningKey::from_bytes(&seed)))
    }

    pub fn to_stored(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.0.to_bytes())
    }

    pub fn public_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.0.verifying_key().as_bytes())
    }

    /// The signature headers for one request (without `buwiz-device`).
    pub fn sign(
        &self,
        method: &str,
        path_and_query: &str,
        body: &[u8],
        timestamp: i64,
        nonce: &str,
    ) -> Vec<(&'static str, String)> {
        let body_hash = hex(&Sha256::digest(body));
        let input = signing_input(method, path_and_query, &body_hash, timestamp, nonce);
        let signature = self.0.sign(input.as_bytes());
        vec![
            (HEADER_TIMESTAMP, timestamp.to_string()),
            (HEADER_NONCE, nonce.to_owned()),
            (
                HEADER_SIGNATURE,
                URL_SAFE_NO_PAD.encode(signature.to_bytes()),
            ),
        ]
    }
}

/// What the device signs; the server builds the same string.
pub fn signing_input(
    method: &str,
    path_and_query: &str,
    body_sha256_hex: &str,
    timestamp: i64,
    nonce: &str,
) -> String {
    format!("buwiz-request-v1\n{method}\n{path_and_query}\n{body_sha256_hex}\n{timestamp}\n{nonce}")
}

/// A fresh nonce: 16 random bytes, base64url (22 characters).
pub fn nonce() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 16]>())
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    #[test]
    fn a_signature_covers_the_request_and_round_trips_through_storage() {
        let key = DeviceKey::generate();
        let again = DeviceKey::from_stored(&key.to_stored()).unwrap();
        assert_eq!(again.public_key(), key.public_key());

        let headers = key.sign(
            "POST",
            "/api/tickets",
            b"{}",
            1_700_000_000,
            "n0nce-n0nce-n0nce",
        );
        let signature = URL_SAFE_NO_PAD.decode(&headers[2].1).unwrap();
        let signature = Signature::from_slice(&signature).unwrap();
        let public: [u8; 32] = URL_SAFE_NO_PAD
            .decode(key.public_key())
            .unwrap()
            .try_into()
            .unwrap();
        let public = VerifyingKey::from_bytes(&public).unwrap();
        let body_hash = hex(&Sha256::digest(b"{}"));
        let input = signing_input(
            "POST",
            "/api/tickets",
            &body_hash,
            1_700_000_000,
            "n0nce-n0nce-n0nce",
        );
        assert!(public.verify(input.as_bytes(), &signature).is_ok());
        let other = signing_input(
            "POST",
            "/api/tickets",
            &body_hash,
            1_700_000_001,
            "n0nce-n0nce-n0nce",
        );
        assert!(public.verify(other.as_bytes(), &signature).is_err());
        assert_eq!(nonce().len(), 22);
    }
}
