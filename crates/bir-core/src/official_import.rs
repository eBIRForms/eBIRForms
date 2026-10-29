//! What is left of importing an official eBIRForms savefile: the savefile
//! metadata, period-code parsing and form identity lookup. Decrypting a
//! savefile and submitting it needs the IAF key, which lives server-side.

use crate::forms::{FORM_CAPABILITY_REGISTRY, FormCapabilityRecord};

#[derive(Debug)]
pub struct OfficialSavefile {
    pub tin: String,
    pub form_type: String,
    pub period_code: String,
    pub email: String,
    pub year: u16,
    pub quarter: Option<u8>,
    pub month: Option<u8>,
}

pub(crate) fn find_capability_identity(identity: &str) -> Option<&'static FormCapabilityRecord> {
    FORM_CAPABILITY_REGISTRY.iter().find(|capability| {
        capability.code.eq_ignore_ascii_case(identity)
            || capability.form_id.eq_ignore_ascii_case(identity)
    })
}

// Helper to extract year/quarter/month from period code
pub fn parse_period_code(period_code: &str) -> (u16, Option<u8>, Option<u8>) {
    // 122026Q1 -> year 2026, month 12, quarter 1
    // 122026 -> year 2026, month 12
    if period_code.len() >= 6 {
        let month_str = &period_code[0..2];
        let year_str = &period_code[2..6];

        let month: Option<u8> = month_str.parse().ok();
        let year: u16 = year_str.parse().unwrap_or(
            chrono::Local::now()
                .format("%Y")
                .to_string()
                .parse()
                .unwrap_or(2024),
        );

        let mut quarter = None;
        if period_code.len() > 6 && period_code.contains('Q') {
            let q_part = period_code.split('Q').next_back().unwrap_or("");
            quarter = q_part.parse().ok();
        }

        (year, quarter, month)
    } else {
        (
            chrono::Local::now()
                .format("%Y")
                .to_string()
                .parse()
                .unwrap_or(2024),
            None,
            None,
        )
    }
}
