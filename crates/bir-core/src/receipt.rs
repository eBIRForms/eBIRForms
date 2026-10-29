//! A BIR filing receipt as stored on the return: the filename BIR echoed
//! and when BIR says it received it. Reading BIR's confirmation email is not
//! part of this crate; a receipt is entered or attached by the user.

use chrono::{NaiveDate, NaiveTime};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BirReceiptConfirmation {
    pub filename: String,
    pub date_received: NaiveDate,
    pub time_received: NaiveTime,
    pub source_from: Option<String>,
    pub raw_text: String,
    pub raw_html: Option<String>,
}

pub fn split_bir_filename(filename: &str) -> Option<(String, String, String)> {
    let stem = filename.strip_suffix(".xml").unwrap_or(filename);
    let base = stem.split('#').next().unwrap_or(stem);
    let mut parts = base.splitn(3, '-');
    let tin = parts.next()?.to_string();
    let form = parts.next()?.to_string();
    let period = parts.next()?.to_string();
    if tin.is_empty() || form.is_empty() || period.is_empty() {
        return None;
    }
    Some((tin, form, period))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_bir_filename_strips_iaf_email_and_matches_savefile_stem() {
        let expected = Some((
            "00000000000000".to_string(),
            "1601Cv2018".to_string(),
            "092026".to_string(),
        ));
        assert_eq!(
            split_bir_filename("00000000000000-1601Cv2018-092026.xml"),
            expected,
            "plaintext savefile stem must parse TIN/form/period"
        );
        assert_eq!(
            split_bir_filename("00000000000000-1601Cv2018-092026#taxpayer@example.com#.xml"),
            expected,
            "IAF #email# suffix must not leak into period"
        );
    }

    #[test]
    fn split_bir_filename_strips_iaf_email_for_quarterly_receipt_alias() {
        assert_eq!(
            split_bir_filename("00023456400000-2551Qv2018-122026Q1#test@mail.com#.xml"),
            Some((
                "00023456400000".to_string(),
                "2551Qv2018".to_string(),
                "122026Q1".to_string()
            ))
        );
        assert_eq!(
            split_bir_filename("00023456400000-2551Qv2018-122026Q1.xml"),
            split_bir_filename("00023456400000-2551Qv2018-122026Q1#test@mail.com#.xml")
        );
    }

    #[test]
    fn split_bir_filename_keeps_reviewed_period_suffixes_after_stripping_email() {
        assert_eq!(
            split_bir_filename("00000000000000-0619F-042026WB#taxpayer@example.com#.xml"),
            Some((
                "00000000000000".to_string(),
                "0619F".to_string(),
                "042026WB".to_string()
            ))
        );
    }
}
