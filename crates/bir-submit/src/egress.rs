//! Where the relay pipe may connect: a signed egress list.
//!
//! The server asks the app to open TCP connections for a send. The app
//! opens only the `host:port` pairs on a list signed **offline** with an
//! egress key (a compact JWS, `EdDSA`, `typ: buwiz-egress+jwt`, payload
//! `{"hosts": [...], "iat", "exp"}`). The server only serves that list, it
//! cannot sign one, so even a compromised server cannot use a taxpayer's
//! computer as a general proxy. No host is named in this source: the list
//! comes from the server and is trusted only through the pinned key.
//!
//! Without a pinned key or a valid list there is no send.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::VerifyingKey;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::device::hex;
use crate::receipt::{ReceiptError, verify_compact};

pub const TYP: &str = "buwiz-egress+jwt";

/// The Ed25519 public keys (base64url, 32 bytes) that sign the egress
/// list. **A release must fill this** with the key made by
/// `buwiz-egress-sign keygen`; while it is empty, sending needs the
/// [`KEYS_ENV`] override (tests and lab only).
pub const EGRESS_KEYS: &[&str] = &[];

/// Comma-separated base64url keys that replace [`EGRESS_KEYS`]. For tests
/// and lab servers, never a real release.
pub const KEYS_ENV: &str = "BUWIZ_EGRESS_KEYS";

/// The pinned keys: [`KEYS_ENV`] when set, else [`EGRESS_KEYS`].
pub fn pinned_keys() -> Vec<String> {
    match std::env::var(KEYS_ENV) {
        Ok(raw) if !raw.trim().is_empty() => raw
            .split(',')
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => EGRESS_KEYS.iter().map(|key| (*key).to_owned()).collect(),
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum EgressError {
    #[error("this app has no egress key pinned, so it cannot send")]
    NoPinnedKey,
    #[error("a pinned egress key is not an Ed25519 key")]
    BadPinnedKey,
    #[error("the server's egress list is not signed by a pinned key")]
    UnknownKey,
    #[error("the server's egress list has a bad signature")]
    Signature,
    #[error("the server's egress list is malformed")]
    Malformed,
    #[error("the server's egress list has expired")]
    Expired,
}

/// The destinations a verified list permits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressList {
    hosts: Vec<(String, u16)>,
}

impl EgressList {
    pub fn permits(&self, host: &str, port: u16) -> bool {
        self.hosts.iter().any(|(allowed, allowed_port)| {
            allowed.eq_ignore_ascii_case(host) && *allowed_port == port
        })
    }
}

#[derive(Deserialize)]
struct Claims {
    hosts: Vec<String>,
    exp: i64,
}

fn key_id(key: &VerifyingKey) -> String {
    hex(&Sha256::digest(key.as_bytes())[..8])
}

/// Verifies `jws` against `pinned` (base64url keys) at time `now`.
pub fn verify(pinned: &[String], jws: &str, now: i64) -> Result<EgressList, EgressError> {
    if pinned.is_empty() {
        return Err(EgressError::NoPinnedKey);
    }
    let keys = pinned
        .iter()
        .map(|raw| {
            let bytes: [u8; 32] = URL_SAFE_NO_PAD
                .decode(raw.trim_end_matches('='))
                .ok()
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or(EgressError::BadPinnedKey)?;
            let key = VerifyingKey::from_bytes(&bytes).map_err(|_| EgressError::BadPinnedKey)?;
            Ok((key_id(&key), key))
        })
        .collect::<Result<Vec<_>, EgressError>>()?;
    let payload = verify_compact(&keys, TYP, jws.trim()).map_err(|error| match error {
        ReceiptError::UnknownKey => EgressError::UnknownKey,
        ReceiptError::Signature => EgressError::Signature,
        _ => EgressError::Malformed,
    })?;
    let claims: Claims = serde_json::from_slice(&payload).map_err(|_| EgressError::Malformed)?;
    if claims.exp <= now {
        return Err(EgressError::Expired);
    }
    let hosts = claims
        .hosts
        .iter()
        .map(|entry| {
            let (host, port) = entry.rsplit_once(':').ok_or(EgressError::Malformed)?;
            let port: u16 = port.parse().map_err(|_| EgressError::Malformed)?;
            if host.is_empty() || port == 0 {
                return Err(EgressError::Malformed);
            }
            Ok((host.to_ascii_lowercase(), port))
        })
        .collect::<Result<_, _>>()?;
    Ok(EgressList { hosts })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::{Value, json};

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

    fn list(exp: i64) -> Value {
        json!({ "hosts": ["lab.example:22", "10.0.0.5:443"], "iat": 1, "exp": exp })
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
            verify(&[public(&other)], &good, 50),
            Err(EgressError::UnknownKey)
        );
        assert_eq!(verify(&[], &good, 50), Err(EgressError::NoPinnedKey));
        assert_eq!(
            verify(&["not-a-key".into()], &good, 50),
            Err(EgressError::BadPinnedKey)
        );
        assert_eq!(
            verify(&[public(&key)], &good, 100),
            Err(EgressError::Expired)
        );
        let receipt_typed = sign(&key, "buwiz-receipt+jwt", &list(100));
        assert_eq!(
            verify(&[public(&key)], &receipt_typed, 50),
            Err(EgressError::Malformed)
        );

        // The same kid, someone else's signature over a wider list.
        let wider = sign(
            &key,
            TYP,
            &json!({ "hosts": ["evil.example:443"], "iat": 1, "exp": 100 }),
        );
        let parts: Vec<&str> = wider.split('.').collect();
        let forged_signature = other.sign(format!("{}.{}", parts[0], parts[1]).as_bytes());
        let forged = format!(
            "{}.{}.{}",
            parts[0],
            parts[1],
            URL_SAFE_NO_PAD.encode(forged_signature.to_bytes())
        );
        assert_eq!(
            verify(&[public(&key)], &forged, 50),
            Err(EgressError::Signature)
        );
        assert_eq!(
            verify(&[public(&key)], "a.b", 50),
            Err(EgressError::Malformed)
        );
    }

    #[test]
    fn the_release_key_list_is_the_only_pin_without_the_override() {
        // SAFETY: only this test touches the variable.
        unsafe { std::env::remove_var(KEYS_ENV) };
        assert_eq!(
            pinned_keys(),
            EGRESS_KEYS
                .iter()
                .map(|key| key.to_string())
                .collect::<Vec<_>>()
        );
        unsafe { std::env::set_var(KEYS_ENV, " a , b ") };
        assert_eq!(pinned_keys(), vec!["a".to_owned(), "b".to_owned()]);
        unsafe { std::env::remove_var(KEYS_ENV) };
    }
}
