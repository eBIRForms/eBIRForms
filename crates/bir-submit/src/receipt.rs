//! Receipts: the Sender's signed statement that a return was handed to BIR
//! (or, in dry-run mode, that it would have been). A compact JWS, `EdDSA`,
//! `typ: buwiz-receipt+jwt`, checked against the server's JWKS by `kid`.
//! The app also compares `xml_sha256` with the XML it sealed, and `jti`
//! with its ticket.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const TYP: &str = "buwiz-receipt+jwt";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub sub: String,
    pub tin: String,
    pub form_type: String,
    pub period: String,
    pub filename: String,
    pub xml_sha256: String,
    pub payload_sha256: String,
    pub remote_path: String,
    /// `live`, `lab` or `dry_run`.
    pub mode: String,
    /// `sent` or `dry_run`.
    pub status: String,
    pub iat: i64,
    #[serde(default)]
    pub jti: Option<String>,
}

impl Receipt {
    /// Nothing reached BIR: the server is in dry-run mode.
    pub fn is_test(&self) -> bool {
        self.status == "dry_run" || self.mode == "dry_run"
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReceiptError {
    #[error("the receipt is malformed")]
    Malformed,
    #[error("the receipt is signed by a key not in the signed trust list")]
    UnknownKey,
    #[error("the receipt's signature is wrong")]
    Signature,
    #[error("the receipt is for other XML than was sent")]
    XmlMismatch,
    #[error("the receipt is for another ticket")]
    TicketMismatch,
}

/// The Ed25519 keys in a JWKS document.
pub fn keys(jwks: &Value) -> Vec<(String, VerifyingKey)> {
    jwks["keys"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|jwk| jwk["kty"] == "OKP" && jwk["crv"] == "Ed25519")
        .filter_map(|jwk| {
            let x: [u8; 32] = URL_SAFE_NO_PAD
                .decode(jwk["x"].as_str()?)
                .ok()?
                .try_into()
                .ok()?;
            Some((
                jwk["kid"].as_str()?.to_owned(),
                VerifyingKey::from_bytes(&x).ok()?,
            ))
        })
        .collect()
}

/// The `kid` a receipt names, to tell a key rotation from a forgery.
pub fn key_id(jws: &str) -> Option<String> {
    let header = jws.split('.').next()?;
    let header: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header).ok()?).ok()?;
    header["kid"].as_str().map(str::to_owned)
}

/// Checks a compact EdDSA JWS of type `typ` against `keys` (matched by
/// `kid`) and answers its payload bytes. Receipts and the trust list both
/// use it.
pub(crate) fn verify_compact(
    keys: &[(String, VerifyingKey)],
    typ: &str,
    jws: &str,
) -> Result<Vec<u8>, ReceiptError> {
    let mut parts = jws.split('.');
    let (Some(header), Some(payload), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(ReceiptError::Malformed);
    };
    let decode = |part: &str| {
        URL_SAFE_NO_PAD
            .decode(part)
            .map_err(|_| ReceiptError::Malformed)
    };
    let header: Value =
        serde_json::from_slice(&decode(header)?).map_err(|_| ReceiptError::Malformed)?;
    if header["alg"] != "EdDSA" || header["typ"] != typ {
        return Err(ReceiptError::Malformed);
    }
    let kid = header["kid"].as_str().ok_or(ReceiptError::Malformed)?;
    let (_, key) = keys
        .iter()
        .find(|(id, _)| id == kid)
        .ok_or(ReceiptError::UnknownKey)?;
    let signature =
        Signature::from_slice(&decode(signature)?).map_err(|_| ReceiptError::Malformed)?;
    let input = &jws[..jws.rfind('.').expect("three parts")];
    key.verify(input.as_bytes(), &signature)
        .map_err(|_| ReceiptError::Signature)?;
    decode(payload)
}

/// Checks the signature against `jwks`, then that the receipt is for
/// `xml_sha256` (hex) and, when given, ticket `jti`.
pub fn verify(
    jwks: &Value,
    jws: &str,
    xml_sha256: &str,
    jti: Option<&str>,
) -> Result<Receipt, ReceiptError> {
    let payload = verify_compact(&keys(jwks), TYP, jws)?;
    let receipt: Receipt = serde_json::from_slice(&payload).map_err(|_| ReceiptError::Malformed)?;
    if !receipt.xml_sha256.eq_ignore_ascii_case(xml_sha256) {
        return Err(ReceiptError::XmlMismatch);
    }
    if let Some(jti) = jti
        && receipt.jti.as_deref() != Some(jti)
    {
        return Err(ReceiptError::TicketMismatch);
    }
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::json;

    fn kid(key: &SigningKey) -> String {
        use sha2::{Digest, Sha256};
        crate::device::hex(&Sha256::digest(key.verifying_key().as_bytes())[..8])
    }

    fn jwks(key: &SigningKey) -> Value {
        json!({ "keys": [{
            "kty": "OKP", "crv": "Ed25519", "alg": "EdDSA", "use": "sig", "kid": kid(key),
            "x": URL_SAFE_NO_PAD.encode(key.verifying_key().as_bytes()),
        }] })
    }

    fn sign(key: &SigningKey, typ: &str, claims: &Value) -> String {
        let header = json!({ "alg": "EdDSA", "kid": kid(key), "typ": typ });
        let input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap()),
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap())
        );
        let signature = key.sign(input.as_bytes());
        format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()))
    }

    fn claims(xml_sha256: &str) -> Value {
        json!({
            "iss": "http://localhost", "sub": "s1", "account_id": "a1", "tin": "000-123-453-00000",
            "form_type": "1701Qv2018", "period": "2026Q1",
            "filename": "00012345300000-1701Qv2018-2026Q1#juana@example.com#.xml",
            "xml_sha256": xml_sha256, "payload_sha256": "ab", "remote_path": "/1701Qv2018/x.xml",
            "mode": "dry_run", "status": "dry_run", "iat": 1, "jti": "j1",
        })
    }

    #[test]
    fn a_receipt_verifies_against_the_published_key_and_the_sent_xml() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let sha = "aa".repeat(32);
        let jws = sign(&key, TYP, &claims(&sha));
        let receipt = verify(&jwks(&key), &jws, &sha, Some("j1")).unwrap();
        assert!(receipt.is_test());
        assert_eq!(receipt.form_type, "1701Qv2018");
        assert_eq!(key_id(&jws), Some(kid(&key)));
    }

    #[test]
    fn forged_or_mismatched_receipts_are_refused() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let other = SigningKey::from_bytes(&[8; 32]);
        let sha = "aa".repeat(32);
        let jws = sign(&key, TYP, &claims(&sha));
        assert_eq!(
            verify(&jwks(&other), &jws, &sha, None),
            Err(ReceiptError::UnknownKey)
        );
        assert_eq!(
            verify(&jwks(&key), &jws, &"bb".repeat(32), None),
            Err(ReceiptError::XmlMismatch)
        );
        assert_eq!(
            verify(&jwks(&key), &jws, &sha, Some("j2")),
            Err(ReceiptError::TicketMismatch)
        );
        let ticket = sign(&key, "buwiz-ticket+jwt", &claims(&sha));
        assert_eq!(
            verify(&jwks(&key), &ticket, &sha, None),
            Err(ReceiptError::Malformed)
        );

        // Same kid, someone else's signature.
        let parts: Vec<&str> = jws.split('.').collect();
        let forged_sig = other.sign(format!("{}.{}", parts[0], parts[1]).as_bytes());
        let forged = format!(
            "{}.{}.{}",
            parts[0],
            parts[1],
            URL_SAFE_NO_PAD.encode(forged_sig.to_bytes())
        );
        assert_eq!(
            verify(&jwks(&key), &forged, &sha, None),
            Err(ReceiptError::Signature)
        );
        assert_eq!(
            verify(&jwks(&key), "a.b", &sha, None),
            Err(ReceiptError::Malformed)
        );
    }
}
