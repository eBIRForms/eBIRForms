use crate::naming::Tin;
use crate::profile::{ComplianceSourceMode, TaxProfileVersionStatus, TaxpayerProfile};
use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub field: &'static str,
    pub message: String,
}

impl ValidationError {
    pub fn new(field: &'static str, message: impl Into<String>) -> Self {
        Self {
            field,
            message: message.into(),
        }
    }
}

pub fn validate_required(field: &'static str, label: &str, value: &str) -> Option<ValidationError> {
    if value.trim().is_empty() {
        Some(ValidationError::new(field, format!("{label} is required")))
    } else {
        None
    }
}

/// Oldest taxable year the January-2018-and-later eBIRForms accept. The
/// official forms alert "Please file using the old version of the form."
pub const OFFICIAL_MIN_FORM_YEAR: u16 = 2018;

/// Exact alert the official forms show when the return-period year predates
/// this revision (`1601c-input-006`, `2551q-input-year-revision`).
pub const OFFICIAL_OLD_VERSION_MESSAGE: &str = "Please file using the old version of the form.";

/// Exact alert the official forms show when the RDO selection is blank or the
/// "000" placeholder (`1601c-save-016`, `1601c-validate-027`,
/// `2551q-validate-rdo`).
pub const OFFICIAL_INVALID_RDO_MESSAGE: &str = "Please enter a valid RDO Code on Item 7.";

/// Whether `code` is one the official RDO dropdown can hold. `getRdo()` builds
/// that dropdown from `xml/rdo.xml` behind a blank `'000'` placeholder, so the
/// official app can only submit a listed code (or the placeholder, which
/// 1601C rejects via `selectedIndex == 0`). Our field is free text, so this
/// membership test is the only place the dropdown's domain is enforced; the
/// placeholder is not in `rdo.json` and fails here too.
pub fn rdo_code_is_official_option(code: &str) -> bool {
    crate::reference::get_rdo(code.trim()).is_some()
}

/// Dev-mode escape hatch: set `EBIR_RELAXED_VALIDATION=1` in a debug build to
/// skip the official TIN check digit ([`official_tin_check_code`]), so test
/// TINs like `000-000-000-00000` validate. TIN format and every other rule
/// still apply. The flag is compiled out of release builds so a production
/// binary can never relax a gate.
pub fn relaxed_dev_mode() -> bool {
    cfg!(debug_assertions)
        && relaxed_dev_mode_enabled(std::env::var("EBIR_RELAXED_VALIDATION").ok().as_deref())
}

/// Exact alert the official app shows when the TIN check fails
/// (`getChkTinErrDesc` in `js/string-util.js`).
pub const OFFICIAL_INVALID_TIN_MESSAGE: &str = "You have entered an incorrect TIN";

/// Port of the official TIN check: `getTinChkCode` (`js/string-util.js`)
/// around `chkt.exe`, the Free Pascal tool `ValidateTinWChkDgt` runs on the
/// 9-digit TIN without its branch code. `0` passes; `1` is a check-digit
/// failure or one of the dummy TINs (rejected since 7.9.6.1); `2` is not
/// exactly nine digits. Blank input passes like the official helper.
///
/// Verified against `chkt.exe` (sha256 `c00bd413…b0ac`) run under emulation
/// on 3,000+ TINs, including the 900000005–905180008 special range.
pub fn official_tin_check_code(first_nine: &str) -> u8 {
    let tin = first_nine.trim();
    if tin.is_empty() {
        return 0;
    }
    if tin.len() != 9 || !tin.bytes().all(|b| b.is_ascii_digit()) {
        return 2;
    }
    if matches!(tin, "999999999" | "222222222" | "000000000") {
        return 1;
    }
    let digits: Vec<u32> = tin.bytes().map(|b| u32::from(b - b'0')).collect();
    let expected = official_tin_check_digit(&digits[..8]);
    let actual = digits[8];
    if expected == actual {
        return 0;
    }
    // chkt.exe retries a legacy 9xx block with the check digit lowered by
    // one (0 wraps to 9), so either digit is accepted there.
    let number: u32 = tin.parse().unwrap_or(0);
    if digits[0] == 9 && (900_000_005..=905_180_008).contains(&number) {
        return u8::from((actual + 9) % 10 != expected);
    }
    1
}

/// chkt.exe's check digit over the first eight TIN digits.
fn official_tin_check_digit(first_eight: &[u32]) -> u32 {
    fn digital_root(mut n: u32) -> u32 {
        while n >= 10 {
            let mut sum = 0;
            while n > 0 {
                sum += n % 10;
                n /= 10;
            }
            n = sum;
        }
        n
    }
    let total: u32 = first_eight
        .iter()
        .enumerate()
        .map(|(index, digit)| {
            let offset = 8 - index as u32;
            let weight = 256 >> index;
            digital_root(((digit + offset) % 10) * weight)
        })
        .sum();
    (10 - total % 10) % 10
}

fn relaxed_dev_mode_enabled(value: Option<&str>) -> bool {
    value.is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Maps a tax-relief specification to the option code the official form
/// serializes: "1" (Special Rate), "2" (International Tax Treaty), and — on
/// 1601C only — "3" (Both). Accepts the codes verbatim plus the labels users
/// typed when the field was free-text.
pub fn official_tax_relief_code(spec: &str, allow_both: bool) -> Option<String> {
    let normalized = spec.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "1" | "special rate" | "special" => Some("1".to_string()),
        "2" | "international tax treaty" | "tax treaty" | "international treaty" => {
            Some("2".to_string())
        }
        "3" | "both" if allow_both => Some("3".to_string()),
        _ => None,
    }
}

/// Official field-length limits used by the eBIRForms `maxlength` attributes.
/// `maxlength` counts the raw input, so this counts raw characters without
/// trimming. Reaching BIR with an over-long value truncates or corrupts the
/// wire file, so we reject it instead.
pub fn fits_official_maxlength(value: &str, max_chars: usize) -> bool {
    value.chars().count() <= max_chars
}

pub fn validate_zip(zip: &str) -> bool {
    zip.len() == 4 && zip.chars().all(|c| c.is_ascii_digit())
}

pub fn validate_email(email: &str) -> bool {
    static EMAIL_RE: OnceLock<Regex> = OnceLock::new();
    EMAIL_RE
        .get_or_init(|| Regex::new(r"^[^@\s]+@[^@\s]+\.[^@\s]+$").expect("valid email regex"))
        .is_match(email.trim())
}

pub fn validate_ph_phone(phone: &str) -> bool {
    let compact: String = phone
        .trim()
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '+')
        .collect();

    static MOBILE_RE: OnceLock<Regex> = OnceLock::new();
    static LANDLINE_RE: OnceLock<Regex> = OnceLock::new();

    let mobile = MOBILE_RE.get_or_init(|| {
        Regex::new(r"^(09\d{9}|\+639\d{9}|639\d{9})$").expect("valid mobile regex")
    });
    let landline = LANDLINE_RE.get_or_init(|| {
        Regex::new(r"^((02|\+632|632)?\d{8}|0[3-8]\d{1,2}\d{7}|\+63[3-8]\d{1,2}\d{7}|63[3-8]\d{1,2}\d{7})$")
            .expect("valid landline regex")
    });

    mobile.is_match(&compact) || landline.is_match(&compact)
}

/// Validate TIN allowing both legacy 12-digit and new 14-digit formats.
/// Use this for loading/importing existing data.
pub fn validate_tin(tin: &Tin) -> bool {
    let full = tin.full();
    (full.len() == 12 || full.len() == 13 || full.len() == 14)
        && full.chars().all(|c| c.is_ascii_digit())
}

/// Validate TIN strictly as the new 14-digit format (3-3-3-5).
/// Use this for new filings on the latest eBIRForms.
pub fn validate_tin_14(tin: &Tin) -> bool {
    let full = tin.full();
    full.len() == 14
        && full.chars().all(|c| c.is_ascii_digit())
        && tin.segment1.len() == 3
        && tin.segment2.len() == 3
        && tin.segment3.len() == 3
        && tin.branch.len() == 5
}

pub fn validate_profile(profile: &TaxpayerProfile) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    if !validate_tin(&profile.tin) {
        errors.push(ValidationError::new(
            "tin",
            "TIN must have 12 to 14 digits including branch code",
        ));
    } else if !relaxed_dev_mode()
        && official_tin_check_code(&format!(
            "{}{}{}",
            profile.tin.segment1, profile.tin.segment2, profile.tin.segment3
        )) != 0
    {
        errors.push(ValidationError::new("tin", OFFICIAL_INVALID_TIN_MESSAGE));
    }

    for err in [
        validate_required("rdo_code", "RDO", &profile.rdo_code),
        validate_required(
            "line_of_business",
            "Line of business",
            &profile.line_of_business,
        ),
        validate_required("full_name", "Taxpayer name", &profile.full_name),
        validate_required(
            "registered_address",
            "Registered address",
            &profile.registered_address,
        ),
        validate_required("zip_code", "ZIP code", &profile.zip_code),
        validate_required("phone", "Phone number", &profile.phone),
        validate_required("email", "Email", &profile.email),
    ]
    .into_iter()
    .flatten()
    {
        errors.push(err);
    }

    if !profile.zip_code.trim().is_empty() && !validate_zip(profile.zip_code.trim()) {
        errors.push(ValidationError::new(
            "zip_code",
            "ZIP code must be 4 digits",
        ));
    }

    if !profile.phone.trim().is_empty() && !validate_ph_phone(&profile.phone) {
        errors.push(ValidationError::new(
            "phone",
            "Phone must be a valid Philippine mobile or landline number",
        ));
    }

    if !profile.email.trim().is_empty() && !validate_email(&profile.email) {
        errors.push(ValidationError::new("email", "Email address is invalid"));
    }

    let mut confirmed_versions: Vec<_> = profile
        .profile_versions
        .iter()
        .filter(|version| version.status == TaxProfileVersionStatus::Confirmed)
        .collect();

    if profile.compliance_source_mode == ComplianceSourceMode::CorVersioned
        && confirmed_versions.is_empty()
    {
        errors.push(ValidationError::new(
            "profile_versions",
            "COR-managed compliance requires at least one confirmed profile version",
        ));
    }

    for version in &confirmed_versions {
        if version.effective_from.is_none() {
            errors.push(ValidationError::new(
                "profile_versions",
                format!(
                    "Confirmed profile version '{}' must have an effective start date",
                    version.label
                ),
            ));
        }
        if let (Some(effective_from), Some(effective_until)) =
            (version.effective_from, version.effective_until)
            && effective_until < effective_from
        {
            errors.push(ValidationError::new(
                "profile_versions",
                format!(
                    "Confirmed profile version '{}' has an effective end date before its start date",
                    version.label
                ),
            ));
        }
    }

    for version in &profile.profile_versions {
        for override_rule in &version.obligation_overrides {
            if override_rule.form_code.trim().is_empty() {
                errors.push(ValidationError::new(
                    "profile_versions",
                    format!(
                        "Profile version '{}' has an obligation override without a form code",
                        version.label
                    ),
                ));
            }
            if override_rule.reason.trim().is_empty()
                || override_rule
                    .source_reference
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .is_empty()
            {
                errors.push(ValidationError::new(
                    "profile_versions",
                    format!(
                        "Profile obligation override '{}' on '{}' requires a reason and source",
                        override_rule.form_code, version.label
                    ),
                ));
            }
        }

        for override_rule in &version.deadline_overrides {
            if override_rule.title.trim().is_empty()
                || override_rule.source_reference.trim().is_empty()
                || override_rule.affected_form_codes.is_empty()
            {
                errors.push(ValidationError::new(
                    "profile_versions",
                    format!(
                        "Profile deadline override on '{}' requires a title, source, and form code",
                        version.label
                    ),
                ));
            }
        }
    }

    confirmed_versions.sort_by(|a, b| {
        a.effective_from
            .cmp(&b.effective_from)
            .then(a.id.cmp(&b.id))
    });
    for pair in confirmed_versions.windows(2) {
        let previous = pair[0];
        let next = pair[1];
        let overlaps = match (previous.effective_until, next.effective_from) {
            (Some(previous_end), Some(next_start)) => previous_end >= next_start,
            _ => true,
        };

        if overlaps {
            errors.push(ValidationError::new(
                "profile_versions",
                format!(
                    "Confirmed profile versions '{}' and '{}' overlap",
                    previous.label, next.label
                ),
            ));
        }
    }

    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rdo_options_match_the_official_dropdown() {
        assert!(rdo_code_is_official_option("001"));
        assert!(rdo_code_is_official_option(" 018 "));
        // The blank placeholder and codes absent from rdo.xml are not options.
        assert!(!rdo_code_is_official_option("000"));
        assert!(!rdo_code_is_official_option(""));
        assert!(!rdo_code_is_official_option("999"));
    }

    #[test]
    fn tin_check_matches_the_official_chkt_exe() {
        // Expected codes are chkt.exe's own exit codes, captured by running
        // the shipped binary under emulation (not derived from this port).
        for (tin, expected) in [
            ("000234564", 0),
            ("123456788", 0),
            ("123456789", 1),
            ("111111114", 0),
            ("111111115", 1),
            ("900000004", 0),
            ("900000005", 0),
            ("900000006", 1),
            ("905180008", 1),
            ("905180009", 0),
            ("905180000", 0),
            ("905180010", 1),
            ("901234560", 1),
            ("901234569", 1),
            ("999999995", 0),
            ("899999999", 1),
            ("159880005", 0),
            ("428804832", 0),
            ("674465222", 0),
            ("686956816", 0),
            ("868654684", 0),
            ("137756848", 0),
            ("299687453", 0),
            ("806689686", 0),
            ("834141843", 0),
            ("747000872", 0),
            ("564539832", 0),
            ("768488845", 0),
            ("735260086", 0),
            ("980512832", 0),
            ("833838458", 0),
            ("824757551", 0),
            ("527104586", 1),
            ("806407983", 1),
            ("830080574", 1),
            ("629771961", 1),
            ("473622146", 0),
            ("257627256", 1),
            // 900000005–905180008 also accepts the check digit plus one.
            ("900013294", 0),
            ("903220207", 0),
            ("900423236", 0),
            ("900580588", 0),
            ("901505975", 0),
            ("904282195", 0),
        ] {
            assert_eq!(official_tin_check_code(tin), expected, "TIN {tin}");
        }
    }

    #[test]
    fn tin_check_rejects_dummies_and_malformed_input_like_get_tin_chk_code() {
        // chkt.exe accepts 000000000; `getTinChkCode` overrides the dummies.
        for dummy in ["000000000", "222222222", "999999999"] {
            assert_eq!(official_tin_check_code(dummy), 1, "{dummy}");
        }
        for malformed in ["12345678", "1234567890", "12345678a", "123-45678"] {
            assert_eq!(official_tin_check_code(malformed), 2, "{malformed}");
        }
        assert_eq!(official_tin_check_code(""), 0);
        assert_eq!(official_tin_check_code(" 000234564 "), 0);
    }

    #[test]
    fn relaxed_dev_mode_parses_only_truthy_values() {
        for v in ["1", "true", "yes", "on", "TRUE", " 1 "] {
            assert!(relaxed_dev_mode_enabled(Some(v)), "{v}");
        }
        for v in ["0", "false", "no", "off", "", "enabled"] {
            assert!(!relaxed_dev_mode_enabled(Some(v)), "{v}");
        }
        assert!(!relaxed_dev_mode_enabled(None));
    }

    #[test]
    fn tax_relief_spec_maps_labels_and_codes_to_official_options() {
        assert_eq!(official_tax_relief_code("1", true).as_deref(), Some("1"));
        assert_eq!(
            official_tax_relief_code("Special Rate", false).as_deref(),
            Some("1")
        );
        assert_eq!(
            official_tax_relief_code("International Tax Treaty", true).as_deref(),
            Some("2")
        );
        assert_eq!(official_tax_relief_code("both", true).as_deref(), Some("3"));
        // 2551Q's dropdown has no "Both" option.
        assert_eq!(official_tax_relief_code("3", false), None);
        assert_eq!(official_tax_relief_code("Special Law 123", true), None);
        assert_eq!(official_tax_relief_code("", true), None);
    }

    #[test]
    fn validates_zip_and_phone_patterns() {
        assert!(validate_zip("2200"));
        assert!(!validate_zip("22000"));
        assert!(validate_ph_phone("09170000100"));
        assert!(validate_ph_phone("+639170000100"));
        assert!(validate_ph_phone("02 8123 4567"));
        assert!(validate_ph_phone("(032) 123 4567"));
        assert!(!validate_ph_phone("123"));
    }
}
