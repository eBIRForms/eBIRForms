//! The relay trust list: everything the app trusts when it sends.
//!
//! A compact JWS (`EdDSA`, `typ: buwiz-trust+jwt`) signed **offline** with
//! a key the server never holds, over
//! `{"hosts", "sealing_keys", "receipt_keys", "iat", "exp"}`:
//!
//! - `hosts`: the only `host:port` pairs the relay pipe may open;
//! - `sealing_keys`: the Sender's HPKE keys. A return is sealed only to one
//!   of these, so a server that serves its own key gets nothing sealed;
//! - `receipt_keys`: the only keys whose receipts make a return Sent.
//!
//! The server only serves the list, it cannot sign one. No host is named
//! in this source; the list is trusted only through the pinned key.
//! Without a pinned key or a valid list there is no send.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::VerifyingKey;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::device::hex;
use crate::receipt::{self, ReceiptError, verify_compact};
use crate::seal::SealingKey;

pub const TYP: &str = "buwiz-trust+jwt";

/// The Ed25519 public keys (base64url, 32 bytes) that sign the trust list.
/// **A release must fill this** with the key made by
/// `buwiz-trust-sign keygen`; while it is empty a release build cannot
/// send. Tests and lab set [`crate::SubmitConfig::trust_keys`] in code.
pub const TRUST_KEYS: &[&str] = &[];

/// Comma-separated base64url keys that replace [`TRUST_KEYS`]. Honoured
/// only in debug builds or with the `lab` feature, never in a release.
pub const KEYS_ENV: &str = "BUWIZ_TRUST_KEYS";

/// The pinned keys: [`TRUST_KEYS`], or [`KEYS_ENV`] in a debug or `lab`
/// build when it is set.
pub fn pinned_keys() -> Vec<String> {
    if cfg!(any(debug_assertions, feature = "lab"))
        && let Ok(raw) = std::env::var(KEYS_ENV)
        && !raw.trim().is_empty()
    {
        return raw
            .split(',')
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(str::to_owned)
            .collect();
    }
    TRUST_KEYS.iter().map(|key| (*key).to_owned()).collect()
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum TrustError {
    #[error("this app has no trust key pinned, so it cannot send")]
    NoPinnedKey,
    #[error("a pinned trust key is not an Ed25519 key")]
    BadPinnedKey,
    #[error("the server's trust list is not signed by a pinned key")]
    UnknownKey,
    #[error("the server's trust list has a bad signature")]
    Signature,
    #[error("the server's trust list is malformed")]
    Malformed,
    #[error("the server's trust list has expired")]
    Expired,
    #[error("the server's sealing key is not in the signed trust list")]
    UnlistedSealingKey,
}

/// A verified list.
#[derive(Clone, Debug)]
pub struct TrustList {
    hosts: Vec<(String, u16)>,
    sealing_keys: Vec<SealingKey>,
    /// As a JWKS document, for [`receipt::verify`].
    receipt_jwks: Value,
}

impl TrustList {
    pub fn permits(&self, host: &str, port: u16) -> bool {
        self.hosts.iter().any(|(allowed, allowed_port)| {
            allowed.eq_ignore_ascii_case(host) && *allowed_port == port
        })
    }

    /// The listed sealing key the server names as current. The server may
    /// pick among the listed keys (to rotate) but never add one: a kid not
    /// on the list, or a listed kid with other key bytes, is refused.
    pub fn sealing_key(&self, offered: &SealingKey) -> Result<SealingKey, TrustError> {
        self.sealing_keys
            .iter()
            .find(|listed| listed.kid == offered.kid && listed.public_key == offered.public_key)
            .cloned()
            .ok_or(TrustError::UnlistedSealingKey)
    }

    /// Checks a receipt against the listed receipt keys only.
    pub fn verify_receipt(
        &self,
        jws: &str,
        xml_sha256: &str,
        jti: Option<&str>,
    ) -> Result<receipt::Receipt, ReceiptError> {
        receipt::verify(&self.receipt_jwks, jws, xml_sha256, jti)
    }
}

#[derive(Deserialize)]
struct Claims {
    hosts: Vec<String>,
    sealing_keys: Vec<SealingKey>,
    receipt_keys: Vec<Value>,
    exp: i64,
}

fn key_id(key: &VerifyingKey) -> String {
    hex(&Sha256::digest(key.as_bytes())[..8])
}

/// Verifies `jws` against `pinned` (base64url keys) at time `now`.
pub fn verify(pinned: &[String], jws: &str, now: i64) -> Result<TrustList, TrustError> {
    if pinned.is_empty() {
        return Err(TrustError::NoPinnedKey);
    }
    let keys = pinned
        .iter()
        .map(|raw| {
            let bytes: [u8; 32] = URL_SAFE_NO_PAD
                .decode(raw.trim_end_matches('='))
                .ok()
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or(TrustError::BadPinnedKey)?;
            let key = VerifyingKey::from_bytes(&bytes).map_err(|_| TrustError::BadPinnedKey)?;
            Ok((key_id(&key), key))
        })
        .collect::<Result<Vec<_>, TrustError>>()?;
    let payload = verify_compact(&keys, TYP, jws.trim()).map_err(|error| match error {
        ReceiptError::UnknownKey => TrustError::UnknownKey,
        ReceiptError::Signature => TrustError::Signature,
        _ => TrustError::Malformed,
    })?;
    let claims: Claims = serde_json::from_slice(&payload).map_err(|_| TrustError::Malformed)?;
    if claims.exp <= now {
        return Err(TrustError::Expired);
    }
    let hosts = claims
        .hosts
        .iter()
        .map(|entry| {
            let (host, port) = entry.rsplit_once(':').ok_or(TrustError::Malformed)?;
            let port: u16 = port.parse().map_err(|_| TrustError::Malformed)?;
            if host.is_empty() || port == 0 {
                return Err(TrustError::Malformed);
            }
            Ok((host.to_ascii_lowercase(), port))
        })
        .collect::<Result<_, _>>()?;
    let receipt_jwks = serde_json::json!({ "keys": claims.receipt_keys });
    if claims.sealing_keys.is_empty() || receipt::keys(&receipt_jwks).is_empty() {
        return Err(TrustError::Malformed);
    }
    Ok(TrustList {
        hosts,
        sealing_keys: claims.sealing_keys,
        receipt_jwks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::json;

    fn public(key: &SigningKey) -> String {
        URL_SAFE_NO_PAD.encode(key.verifying_key().as_bytes())
    }

    fn sign(key: &SigningKey, typ: &str, claims: &Value) -> String {
        let header = json!({ "alg": "EdDSA", "kid": key_id(&key.verifying_key()), "typ": typ });
        let input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap()),
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap())
        );
        format!(
            "{input}.{}",
            URL_SAFE_NO_PAD.encode(key.sign(input.as_bytes()).to_bytes())
        )
    }

    fn jwk(key: &SigningKey) -> Value {
        json!({ "kty": "OKP", "crv": "Ed25519", "kid": key_id(&key.verifying_key()), "x": public(key) })
    }

    fn sealing(kid: &str, byte: u8) -> Value {
        json!({ "kid": kid, "kem": crate::seal::KEM, "public_key": URL_SAFE_NO_PAD.encode([byte; 32]) })
    }

    fn list(exp: i64) -> Value {
        json!({
            "hosts": ["lab.example:22", "10.0.0.5:443"],
            "sealing_keys": [sealing("s1", 1)],
            "receipt_keys": [jwk(&SigningKey::from_bytes(&[9; 32]))],
            "iat": 1, "exp": exp,
        })
    }

    #[test]
    fn a_valid_list_permits_its_hosts_and_nothing_else() {
        let key = SigningKey::from_bytes(&[1; 32]);
        let verified = verify(&[public(&key)], &sign(&key, TYP, &list(100)), 50).unwrap();
        assert!(verified.permits("lab.example", 22));
        assert!(
            verified.permits("LAB.example", 22),
            "hosts compare without case"
        );
        assert!(verified.permits("10.0.0.5", 443));
        for (host, port) in [
            ("lab.example", 23),
            ("lab.example", 443),
            ("10.0.0.6", 443),
            ("127.0.0.1", 22),
            ("", 0),
        ] {
            assert!(!verified.permits(host, port), "{host}:{port}");
        }
    }

    #[test]
    fn anything_but_a_live_list_from_a_pinned_key_is_refused() {
        let key = SigningKey::from_bytes(&[1; 32]);
        let other = SigningKey::from_bytes(&[2; 32]);
        let good = sign(&key, TYP, &list(100));
        assert_eq!(
            verify(&[public(&other)], &good, 50).unwrap_err(),
            TrustError::UnknownKey
        );
        assert_eq!(verify(&[], &good, 50).unwrap_err(), TrustError::NoPinnedKey);
        assert_eq!(
            verify(&["not-a-key".into()], &good, 50).unwrap_err(),
            TrustError::BadPinnedKey
        );
        assert_eq!(
            verify(&[public(&key)], &good, 100).unwrap_err(),
            TrustError::Expired
        );
        let receipt_typed = sign(&key, "buwiz-receipt+jwt", &list(100));
        assert_eq!(
            verify(&[public(&key)], &receipt_typed, 50).unwrap_err(),
            TrustError::Malformed
        );
        let mut no_keys = list(100);
        no_keys["sealing_keys"] = json!([]);
        assert_eq!(
            verify(&[public(&key)], &sign(&key, TYP, &no_keys), 50).unwrap_err(),
            TrustError::Malformed
        );

        // The same kid, someone else's signature over a wider list.
        let mut wider = list(100);
        wider["hosts"] = json!(["evil.example:443"]);
        let wider = sign(&key, TYP, &wider);
        let parts: Vec<&str> = wider.split('.').collect();
        let forged_signature = other.sign(format!("{}.{}", parts[0], parts[1]).as_bytes());
        let forged = format!(
            "{}.{}.{}",
            parts[0],
            parts[1],
            URL_SAFE_NO_PAD.encode(forged_signature.to_bytes())
        );
        assert_eq!(
            verify(&[public(&key)], &forged, 50).unwrap_err(),
            TrustError::Signature
        );
        assert_eq!(
            verify(&[public(&key)], "a.b", 50).unwrap_err(),
            TrustError::Malformed
        );
    }

    #[test]
    fn only_a_listed_sealing_key_is_used() {
        let key = SigningKey::from_bytes(&[1; 32]);
        let verified = verify(&[public(&key)], &sign(&key, TYP, &list(100)), 50).unwrap();
        let listed: SealingKey = serde_json::from_value(sealing("s1", 1)).unwrap();
        assert_eq!(
            verified.sealing_key(&listed).unwrap().public_key,
            listed.public_key
        );
        let unlisted: SealingKey = serde_json::from_value(sealing("s2", 2)).unwrap();
        assert_eq!(
            verified.sealing_key(&unlisted).unwrap_err(),
            TrustError::UnlistedSealingKey
        );
        // A listed kid with another key behind it.
        let swapped: SealingKey = serde_json::from_value(sealing("s1", 3)).unwrap();
        assert_eq!(
            verified.sealing_key(&swapped).unwrap_err(),
            TrustError::UnlistedSealingKey
        );
    }

    #[test]
    fn only_listed_receipt_keys_verify_receipts() {
        let key = SigningKey::from_bytes(&[1; 32]);
        let verified = verify(&[public(&key)], &sign(&key, TYP, &list(100)), 50).unwrap();
        let sha = "aa".repeat(32);
        let claims = json!({
            "sub": "s", "tin": "t", "form_type": "f", "period": "p", "filename": "n",
            "xml_sha256": sha, "payload_sha256": "b", "remote_path": "/r", "mode": "lab",
            "status": "sent", "iat": 1,
        });
        let listed = sign(&SigningKey::from_bytes(&[9; 32]), receipt::TYP, &claims);
        assert!(verified.verify_receipt(&listed, &sha, None).is_ok());
        let forged = sign(&SigningKey::from_bytes(&[10; 32]), receipt::TYP, &claims);
        assert_eq!(
            verified.verify_receipt(&forged, &sha, None).unwrap_err(),
            ReceiptError::UnknownKey
        );
    }

    #[test]
    fn the_release_key_list_is_the_pin_and_the_override_needs_a_debug_or_lab_build() {
        // SAFETY: only this test touches the variable.
        unsafe { std::env::remove_var(KEYS_ENV) };
        assert_eq!(
            pinned_keys(),
            TRUST_KEYS
                .iter()
                .map(|key| key.to_string())
                .collect::<Vec<_>>()
        );
        unsafe { std::env::set_var(KEYS_ENV, " a , b ") };
        let expected = if cfg!(any(debug_assertions, feature = "lab")) {
            vec!["a".to_owned(), "b".to_owned()]
        } else {
            TRUST_KEYS.iter().map(|key| key.to_string()).collect()
        };
        assert_eq!(pinned_keys(), expected);
        unsafe { std::env::remove_var(KEYS_ENV) };
    }
}
