//! File naming conventions for BIR files.

use serde::{Deserialize, Serialize};

/// Structured Philippine TIN (Tax Identification Number).
/// Format: XXX-XXX-XXX-XXXXX (3-3-3-5 digits)
///
/// The latest eBIRForms uses a 14-digit format where the branch code
/// is 5 digits. Older 12-digit TINs (3-digit branch) are still accepted
/// for backward compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tin {
    pub segment1: String,
    pub segment2: String,
    pub segment3: String,
    pub branch: String,
}

impl Tin {
    /// Full TIN as a single string (e.g., "00023456400000")
    pub fn full(&self) -> String {
        format!(
            "{}{}{}{}",
            self.segment1, self.segment2, self.segment3, self.branch
        )
    }

    /// Formatted TIN with dashes (e.g., "000-234-564-00000")
    pub fn formatted(&self) -> String {
        format!(
            "{}-{}-{}-{}",
            self.segment1, self.segment2, self.segment3, self.branch
        )
    }

    /// Parse a compact or dashed TIN string (12–14 digits, including branch).
    pub fn parse_digits(raw: &str) -> Option<Self> {
        let digits: String = raw.chars().filter(|ch| ch.is_ascii_digit()).collect();
        if !(12..=14).contains(&digits.len()) {
            return None;
        }
        Some(Self {
            segment1: digits[0..3].to_string(),
            segment2: digits[3..6].to_string(),
            segment3: digits[6..9].to_string(),
            branch: digits[9..].to_string(),
        })
    }

    /// Dashed display for a stored TIN string.
    ///
    /// 12–14 digit values (with or without dashes) become `XXX-XXX-XXX-XXXXX`.
    /// Unrecognized shapes are returned trimmed, not invented.
    pub fn dashed_display(raw: &str) -> String {
        Self::parse_digits(raw)
            .map(|tin| tin.formatted())
            .unwrap_or_else(|| raw.trim().to_string())
    }
}

/// Generate savefile name: {TIN}-{FormType}-{Period}.xml
pub fn savefile_name(tin: &Tin, form_type: &str, period: &str) -> String {
    format!("{}-{}-{}.xml", tin.full(), form_type, period)
}

/// Generate IAF filename: {TIN}-{FormType}-{Period}#{email}#.xml
pub fn iaf_filename(tin: &Tin, form_type: &str, period: &str, email: &str) -> String {
    format!("{}-{}-{}#{}#.xml", tin.full(), form_type, period, email)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tin_full_legacy_12_digit() {
        let tin = Tin {
            segment1: "000".into(),
            segment2: "234".into(),
            segment3: "564".into(),
            branch: "000".into(),
        };
        assert_eq!(tin.full(), "000234564000");
        assert_eq!(tin.formatted(), "000-234-564-000");
    }

    #[test]
    fn test_tin_full_new_14_digit() {
        let tin = Tin {
            segment1: "000".into(),
            segment2: "234".into(),
            segment3: "564".into(),
            branch: "00000".into(),
        };
        assert_eq!(tin.full(), "00023456400000");
        assert_eq!(tin.formatted(), "000-234-564-00000");
        assert_eq!(tin.full().len(), 14);
    }

    #[test]
    fn dashed_display_formats_compact_and_dashed_tins() {
        assert_eq!(Tin::dashed_display("00000000000000"), "000-000-000-00000");
        assert_eq!(
            Tin::dashed_display("000-000-000-00000"),
            "000-000-000-00000"
        );
        assert_eq!(Tin::dashed_display("123456789000"), "123-456-789-000");
        assert_eq!(Tin::dashed_display("not-a-tin"), "not-a-tin");
    }

    #[test]
    fn test_iaf_filename() {
        let tin = Tin {
            segment1: "000".into(),
            segment2: "234".into(),
            segment3: "564".into(),
            branch: "00000".into(),
        };
        let name = iaf_filename(&tin, "2551Qv2018", "122026Q1", "test@mail.com");
        assert_eq!(
            name,
            "00023456400000-2551Qv2018-122026Q1#test@mail.com#.xml"
        );
    }
}
