//! Dummy-data sample payloads for the queueable forms.
//!
//! `tests/sample-payloads/` holds what our serializers produce for a fake
//! taxpayer: the plaintext BIR pseudo-XML handed to the submission
//! transport. No real taxpayer data. (The IAF encryption of these payloads,
//! and its byte-exact tests, live server-side.)
//!
//! `<stem>.official.xml` is what the official eBIRForms page submits when a
//! filer enters `<stem>.steps.json`: `tools/official-xml/runtime.js` runs the
//! official HTA with its own scripts, so the official handlers compute and
//! format every derived amount, then its `saveXMLsubmit()` loop writes the
//! plaintext. Our plaintext must equal it byte for byte, which checks our
//! calculations, value formatting and serialization together. Regenerating our
//! samples never changes it.
//!
//! Regenerate after an intentional serializer change with
//! `UPDATE_SAMPLE_PAYLOADS=1 cargo test -p bir-core --test sample_payloads_test`.

use bir_core::official_xml;
use bir_core::profile::TaxpayerProfile;
use std::path::PathBuf;

/// One module per form: its dummy draft and its sample test.
mod sample_payloads;

pub(crate) fn dummy_profile(default_form_type: &str, taxpayer_type: &str) -> TaxpayerProfile {
    // 123-456-788 passes the official check digit; every value is fictitious.
    serde_json::from_value(serde_json::json!({
        "id": null,
        "full_name": "Sample Dummy Taxpayer",
        "tin": {
            "segment1": "123",
            "segment2": "456",
            "segment3": "788",
            "branch": "00000"
        },
        "rdo_code": "039",
        "line_of_business": "Sample Consulting Services",
        "registered_address": "123 Sample Street, Barangay Example, Quezon City",
        "zip_code": "1100",
        "phone": "09170000000",
        "email": "sample.taxpayer@example.com",
        "default_form_type": default_form_type,
        "taxpayer_type": taxpayer_type,
        "business_start_date": "2020-01-15",
        "tax_elections": [{
            "taxable_year": 2025,
            "election": "GraduatedUnspecified",
            "elected_at": "2025-01-15T00:00:00",
            "source_form": "sample"
        }]
    }))
    .expect("dummy profile must deserialize")
}

pub(crate) fn samples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/sample-payloads")
}

/// `_official_encrypt_sha256` is the SHA-256 of `<stem>.plain.xml` encrypted
/// by the official eBIRForms `Encrypt.exe`. The encryption is server-side,
/// so it is not checked here; the hash stays beside each sample so the
/// server's byte-exact tests and these share one table.
pub(crate) fn check_sample(
    stem: &str,
    form_id: &str,
    plaintext: &str,
    _official_encrypt_sha256: &str,
) {
    let layout = official_xml::layout(form_id).expect("official layout");
    check_sample_with_layout(stem, layout, plaintext, _official_encrypt_sha256);
}

/// [`check_sample`] for a form whose page adds rows at run time: the sample
/// follows the form's own extension of the generated layout.
pub(crate) fn check_sample_with_layout(
    stem: &str,
    layout: &official_xml::OfficialLayout,
    plaintext: &str,
    _official_encrypt_sha256: &str,
) {
    let fields = official_xml::read(layout, plaintext).expect("sample follows the official layout");
    assert!(!fields.is_empty());

    let dir = samples_dir();
    let plain_path = dir.join(format!("{stem}.plain.xml"));
    let updating = std::env::var_os("UPDATE_SAMPLE_PAYLOADS").is_some();
    if updating {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&plain_path, plaintext).unwrap();
    }

    let official_plain_path = dir.join(format!("{stem}.official.xml"));
    let official_plain = std::fs::read_to_string(&official_plain_path)
        .unwrap_or_else(|e| panic!("missing {}: {e}", official_plain_path.display()));
    assert_eq!(
        plaintext, official_plain,
        "{stem}: our plaintext differs from the official upload (saveEncryptedProfile) output"
    );

    if updating {
        return;
    }
    let committed = std::fs::read_to_string(&plain_path)
        .unwrap_or_else(|e| panic!("missing {}: {e}", plain_path.display()));
    assert_eq!(
        committed, plaintext,
        "{stem}: serializer output changed; rerun with UPDATE_SAMPLE_PAYLOADS=1 if intended"
    );
}
