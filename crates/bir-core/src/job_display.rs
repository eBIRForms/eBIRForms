//! Human-facing Background Tasks / job-queue titles.
//!
//! Machine identity stays on `Job.command` (`bir_poll_email {email}`). These
//! strings are display names only: form + period + dashed TIN + optional email.

use crate::forms::FormDraftSummary;
use crate::naming::Tin;

const MONTH_ABBR: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// One queued or submitted return, enough to build a distinguishable job title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilingJobLabel<'a> {
    pub form_code: &'a str,
    pub taxable_year: u16,
    pub month: Option<u8>,
    pub quarter: Option<u8>,
    pub tin: &'a str,
    pub email: Option<&'a str>,
}

impl<'a> FilingJobLabel<'a> {
    pub fn from_summary(summary: &'a FormDraftSummary, email: Option<&'a str>) -> Self {
        Self {
            form_code: &summary.form_code,
            taxable_year: summary.taxable_year,
            month: summary.month,
            quarter: summary.quarter,
            tin: &summary.tin,
            email,
        }
    }

    pub fn submit_name(&self) -> String {
        format_submit_job_name(
            self.form_code,
            self.taxable_year,
            self.month,
            self.quarter,
            self.tin,
            self.email,
        )
    }

    pub fn confirmation_poll_name(&self) -> String {
        format_confirmation_poll_job_name(
            self.form_code,
            self.taxable_year,
            self.month,
            self.quarter,
            self.tin,
            self.email,
        )
    }
}

/// Known email for a title parenthetical. Empty / whitespace is omitted.
pub fn known_job_email(email: Option<&str>) -> Option<&str> {
    email.map(str::trim).filter(|value| !value.is_empty())
}

/// Month abbreviation + year (`Sep 2026`), else quarter (`Q1 2026`), else year.
pub fn format_filing_period_label(year: u16, month: Option<u8>, quarter: Option<u8>) -> String {
    if let Some(month) = month
        && let Some(abbr) = month_abbr(month)
    {
        return format!("{abbr} {year}");
    }
    if let Some(quarter) = quarter
        && (1..=4).contains(&quarter)
    {
        return format!("Q{quarter} {year}");
    }
    year.to_string()
}

pub fn format_submit_job_name(
    form_code: &str,
    taxable_year: u16,
    month: Option<u8>,
    quarter: Option<u8>,
    tin: &str,
    email: Option<&str>,
) -> String {
    with_optional_email(
        format!(
            "Submit {form_code} {} for {}",
            format_filing_period_label(taxable_year, month, quarter),
            Tin::dashed_display(tin),
        ),
        email,
    )
}

pub fn format_confirmation_poll_job_name(
    form_code: &str,
    taxable_year: u16,
    month: Option<u8>,
    quarter: Option<u8>,
    tin: &str,
    email: Option<&str>,
) -> String {
    with_optional_email(
        format!(
            "Waiting for {form_code} {} confirmation for {}",
            format_filing_period_label(taxable_year, month, quarter),
            Tin::dashed_display(tin),
        ),
        email,
    )
}

fn month_abbr(month: u8) -> Option<&'static str> {
    MONTH_ABBR.get(usize::from(month.checked_sub(1)?)).copied()
}

fn with_optional_email(base: String, email: Option<&str>) -> String {
    match known_job_email(email) {
        Some(email) => format!("{base} ({email})"),
        None => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forms::FilingStatus;

    #[test]
    fn period_uses_english_month_abbr() {
        assert_eq!(format_filing_period_label(2026, Some(9), None), "Sep 2026");
        assert_eq!(format_filing_period_label(2026, Some(1), None), "Jan 2026");
        assert_eq!(format_filing_period_label(2026, Some(12), None), "Dec 2026");
    }

    #[test]
    fn period_uses_quarter_when_month_is_absent() {
        assert_eq!(format_filing_period_label(2026, None, Some(3)), "Q3 2026");
    }

    #[test]
    fn period_falls_back_to_year_when_month_and_quarter_are_invalid() {
        assert_eq!(format_filing_period_label(2026, Some(13), Some(0)), "2026");
        assert_eq!(format_filing_period_label(2026, None, None), "2026");
    }

    #[test]
    fn submit_title_includes_period_dashed_tin_and_email() {
        assert_eq!(
            format_submit_job_name(
                "1601C",
                2026,
                Some(9),
                None,
                "00000000000000",
                Some("taxpayer@example.com"),
            ),
            "Submit 1601C Sep 2026 for 000-000-000-00000 (taxpayer@example.com)"
        );
    }

    #[test]
    fn confirmation_title_includes_period_dashed_tin_and_email() {
        assert_eq!(
            format_confirmation_poll_job_name(
                "1601C",
                2026,
                Some(9),
                None,
                "00000000000000",
                Some("taxpayer@example.com"),
            ),
            "Waiting for 1601C Sep 2026 confirmation for 000-000-000-00000 (taxpayer@example.com)"
        );
    }

    #[test]
    fn email_parenthetical_is_omitted_when_unknown() {
        assert_eq!(
            format_submit_job_name("1601C", 2026, Some(9), None, "00000000000000", None),
            "Submit 1601C Sep 2026 for 000-000-000-00000"
        );
        assert_eq!(
            format_confirmation_poll_job_name(
                "1601C",
                2026,
                Some(9),
                None,
                "00000000000000",
                Some("   "),
            ),
            "Waiting for 1601C Sep 2026 confirmation for 000-000-000-00000"
        );
    }

    #[test]
    fn tin_dash_formatting_accepts_compact_or_dashed_input() {
        let compact = format_submit_job_name("1601C", 2026, Some(9), None, "00000000000000", None);
        let dashed =
            format_submit_job_name("1601C", 2026, Some(9), None, "000-000-000-00000", None);
        assert_eq!(compact, dashed);
        assert!(compact.contains("000-000-000-00000"));
        assert!(!compact.contains("00000000000000"));
    }

    #[test]
    fn quarterly_2551q_uses_quarter_label() {
        assert_eq!(
            format_submit_job_name(
                "2551Q",
                2026,
                None,
                Some(1),
                "123456789000",
                Some("guard@example.com"),
            ),
            "Submit 2551Q Q1 2026 for 123-456-789-000 (guard@example.com)"
        );
    }

    #[test]
    fn distinct_periods_produce_distinct_titles() {
        let august = format_submit_job_name("1601C", 2026, Some(8), None, "00000000000000", None);
        let september =
            format_submit_job_name("1601C", 2026, Some(9), None, "00000000000000", None);
        assert_ne!(august, september);
        assert!(august.contains("Aug 2026"));
        assert!(september.contains("Sep 2026"));
    }

    #[test]
    fn label_from_summary_matches_direct_formatter() {
        let summary = FormDraftSummary {
            id: 1,
            tin: "00000000000000".into(),
            form_code: "1601C".into(),
            taxable_year: 2026,
            quarter: None,
            month: Some(9),
            status: FilingStatus::Queued,
            updated_at: "2026-09-11T00:00:00Z".into(),
        };
        let label = FilingJobLabel::from_summary(&summary, Some("taxpayer@example.com"));
        assert_eq!(
            label.submit_name(),
            "Submit 1601C Sep 2026 for 000-000-000-00000 (taxpayer@example.com)"
        );
        assert_eq!(
            label.confirmation_poll_name(),
            "Waiting for 1601C Sep 2026 confirmation for 000-000-000-00000 (taxpayer@example.com)"
        );
    }
}
