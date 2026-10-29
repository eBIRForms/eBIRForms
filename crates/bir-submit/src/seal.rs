//! Sealing the plaintext XML for the Sender: HPKE (RFC 9180) base mode,
//! DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, ChaCha20-Poly1305, with `info`
//! binding the ticket: `buwiz-submission-v1:{jti}`. The Gatekeeper stores
//! the envelope and cannot open it.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hpke::aead::ChaCha20Poly1305;
use hpke::kdf::HkdfSha256;
use hpke::kem::X25519HkdfSha256;
use hpke::{Deserializable, OpModeS, Serializable};
use serde::{Deserialize, Serialize};

pub const KEM: &str = "DHKEM(X25519, HKDF-SHA256)";

/// `GET /.well-known/buwiz-hpke.json`, one entry.
#[derive(Clone, Debug, Deserialize)]
pub struct SealingKey {
    pub kid: String,
    pub kem: String,
    pub public_key: String,
}

/// What goes to the server: `{"kid","enc","ct"}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub kid: String,
    pub enc: String,
    pub ct: String,
}

pub fn info(jti: &str) -> Vec<u8> {
    format!("buwiz-submission-v1:{jti}").into_bytes()
}

#[derive(Debug, thiserror::Error)]
pub enum SealError {
    #[error("the server's sealing key is not X25519")]
    BadKey,
    #[error("sealing failed")]
    Failed,
}

pub fn seal(key: &SealingKey, jti: &str, plaintext: &[u8]) -> Result<Envelope, SealError> {
    if key.kem != KEM {
        return Err(SealError::BadKey);
    }
    let public = URL_SAFE_NO_PAD
        .decode(&key.public_key)
        .map_err(|_| SealError::BadKey)?;
    let public = <X25519HkdfSha256 as hpke::Kem>::PublicKey::from_bytes(&public)
        .map_err(|_| SealError::BadKey)?;
    let (encapped, ct) = hpke::single_shot_seal::<ChaCha20Poly1305, HkdfSha256, X25519HkdfSha256>(
        &OpModeS::Base,
        &public,
        &info(jti),
        plaintext,
        b"",
    )
    .map_err(|_| SealError::Failed)?;
    Ok(Envelope {
        kid: key.kid.clone(),
        enc: URL_SAFE_NO_PAD.encode(encapped.to_bytes()),
        ct: URL_SAFE_NO_PAD.encode(ct),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hpke::{Kem as _, OpModeR};

    type Kem = X25519HkdfSha256;

    fn open(
        private: &<Kem as hpke::Kem>::PrivateKey,
        envelope: &Envelope,
        jti: &str,
    ) -> Option<Vec<u8>> {
        let enc = URL_SAFE_NO_PAD.decode(&envelope.enc).ok()?;
        let ct = URL_SAFE_NO_PAD.decode(&envelope.ct).ok()?;
        let encapped = <Kem as hpke::Kem>::EncappedKey::from_bytes(&enc).ok()?;
        hpke::single_shot_open::<ChaCha20Poly1305, HkdfSha256, Kem>(
            &OpModeR::Base,
            private,
            &encapped,
            &info(jti),
            &ct,
            b"",
        )
        .ok()
    }

    #[test]
    fn a_sealed_return_opens_with_the_private_key_and_its_ticket_only() {
        let (private, public) = Kem::derive_keypair(b"test key material, 32 bytes min!");
        let key = SealingKey {
            kid: "k1".into(),
            kem: KEM.into(),
            public_key: URL_SAFE_NO_PAD.encode(public.to_bytes()),
        };
        let xml = b"<?xml version='1.0'?><div>frm1701q:txtYear=2026frm1701q:txtYear=</div>";
        let envelope = seal(&key, "jti-1", xml).unwrap();
        assert_eq!(envelope.kid, "k1");
        assert_eq!(
            open(&private, &envelope, "jti-1").as_deref(),
            Some(xml.as_slice())
        );
        assert_eq!(
            open(&private, &envelope, "jti-2"),
            None,
            "bound to its ticket"
        );

        let mut ct = URL_SAFE_NO_PAD.decode(&envelope.ct).unwrap();
        ct[3] ^= 0x80;
        let tampered = Envelope {
            ct: URL_SAFE_NO_PAD.encode(ct),
            ..envelope.clone()
        };
        assert_eq!(
            open(&private, &tampered, "jti-1"),
            None,
            "tampering is caught"
        );

        let (other, _) = Kem::derive_keypair(b"another key material, 32 bytes!!");
        assert_eq!(open(&other, &envelope, "jti-1"), None);
        assert_ne!(
            seal(&key, "jti-1", xml).unwrap().enc,
            envelope.enc,
            "a fresh ephemeral key each time"
        );
    }

    #[test]
    fn other_key_types_are_refused() {
        let key = SealingKey {
            kid: "k".into(),
            kem: "P-256".into(),
            public_key: String::new(),
        };
        assert!(matches!(seal(&key, "j", b"x"), Err(SealError::BadKey)));
    }
}
