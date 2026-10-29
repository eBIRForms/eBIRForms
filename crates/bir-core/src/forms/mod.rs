//! BIR form data models, ATC tables, form registry, and typed form traits.

pub mod agent;
pub mod atc;
pub mod calendar_status;
pub mod event_trigger;
pub mod excise_places;
pub mod form_0605;
pub mod form_0605_official;
pub mod form_0619e;
pub mod form_0619e_official;
pub mod form_0619f;
pub mod form_0619f_official;
pub mod form_1600pt;
pub mod form_1600vt;
pub mod form_1600wp;
pub mod form_1601c;
pub mod form_1601eq;
pub mod form_1601fq;
pub mod form_1602q;
pub mod form_1603q;
pub mod form_1604c;
pub mod form_1604e;
pub mod form_1604f;
pub mod form_1606;
pub mod form_1700;
pub mod form_1701;
pub mod form_1701_official;
pub mod form_1701a;
pub mod form_1701ms;
pub mod form_1701q;
pub mod form_1701q_official;
pub mod form_1702ex;
pub mod form_1702mx;
pub mod form_1702mx_official;
pub mod form_1702q;
pub mod form_1702rt;
pub mod form_1702rt_official;
pub mod form_1706;
pub mod form_1707;
pub mod form_1707a;
pub mod form_1800;
pub mod form_1801;
pub mod form_2000;
pub mod form_2000ot;
pub mod form_2200a;
pub mod form_2200an;
pub mod form_2200c;
pub mod form_2200m;
pub mod form_2200p;
pub mod form_2200s;
pub mod form_2200t;
pub mod form_2307;
pub mod form_2550m;
pub mod form_2550q;
pub mod form_2551q;
pub mod form_2552;
pub mod form_2553;
pub mod forms_set;
pub mod kept_text;
pub mod official_inputs;
pub mod payment_cells;
pub mod penalty_items;
pub mod print_map;
pub mod queueable;
pub mod registry;
pub mod support_level;
pub mod taxform;

pub use atc::{ATC_TABLE_2551Q, AtcEntry, AtcRateResolution, find_atc, resolve_2551q_atc_rate};
pub use form_1601c::Form1601CDraft;
pub use form_1701q::Form1701QDraft;
pub use form_2551q::{Form2551QDraft, Schedule1Row};
pub use forms_set::{
    FormCardData, FormSetConflict, FormSetEntry, FormSetReviewStatus, FormSetSource,
    FormSuggestion, FormSuggestionSource, FormsSetReconcileResult, PerYearFormsSet,
    reconcile_forms_set_for_year,
};
#[allow(deprecated)]
// re-exporting deprecated forms_for_taxpayer / forms_for_profile for backward compat only
pub use registry::{
    FORM_REGISTRY, FilingFrequency, FormDefinition, find_form, forms_for_profile,
    forms_for_taxpayer,
};
pub use support_level::{
    FORM_CAPABILITY_REGISTRY, FormCapabilities, FormCapabilityRecord, FormSupportLevel,
    can_open_certification_draft, can_queue_for_submission, find_form_capability,
    find_form_capability_by_id, form_support_level, queue_authorized_form_type_id,
};

pub mod form_0605_xml;
pub mod form_0619e_xml;
pub mod form_0619f_xml;
pub mod form_1601c_xml;
pub mod form_1701_xml;
pub mod form_1701q_xml;
pub mod form_1702mx_xml;
pub mod form_1702rt_xml;
pub mod form_2550q_xml;
pub mod form_2551q_xml;

pub trait FormValidator {
    /// Returns a list of (field_id, error_message)
    fn validate(&self) -> Vec<(String, String)>;
}

use std::collections::BTreeMap;

/// Filing period for a tax form — replaces the ambiguous quarter/month column.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FilingPeriod {
    /// Monthly filing: month 1-12.
    Monthly(u8),
    /// Quarterly filing: quarter 1-4.
    Quarterly(u8),
    /// Annual filing.
    Annual,
    /// Open-ended (filed as needed, e.g., payment forms).
    OpenEnded(u32),
}

impl FilingPeriod {
    /// Convert to a database `period_key` string.
    pub fn to_period_key(&self) -> String {
        match self {
            Self::Monthly(m) => format!("M{m:02}"),
            Self::Quarterly(q) => format!("Q{q}"),
            Self::Annual => "A".to_string(),
            Self::OpenEnded(n) => format!("O{n}"),
        }
    }

    /// Parse from a database `period_key` string.
    pub fn from_period_key(key: &str) -> Option<Self> {
        if key == "A" {
            return Some(Self::Annual);
        }
        if key.is_empty() {
            return None;
        }
        let (prefix, rest) = key.split_at(1);
        let num: u32 = rest.parse().ok()?;
        match prefix {
            "M" if (1..=12).contains(&num) => Some(Self::Monthly(num as u8)),
            "Q" if (1..=4).contains(&num) => Some(Self::Quarterly(num as u8)),
            "O" if num > 0 => Some(Self::OpenEnded(num)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod period_tests {
    use super::FilingPeriod;

    #[test]
    fn period_key_round_trips_supported_periods() {
        for period in [
            FilingPeriod::Monthly(1),
            FilingPeriod::Monthly(12),
            FilingPeriod::Quarterly(1),
            FilingPeriod::Quarterly(4),
            FilingPeriod::Annual,
            FilingPeriod::OpenEnded(17),
        ] {
            let key = period.to_period_key();
            assert_eq!(FilingPeriod::from_period_key(&key), Some(period));
        }
    }

    #[test]
    fn period_key_parser_rejects_empty_and_out_of_range_values() {
        for key in ["", "M00", "M13", "Q0", "Q5", "O0", "X1", "QX"] {
            assert_eq!(FilingPeriod::from_period_key(key), None);
        }
    }
}

/// Unified trait for all typed BIR form implementations.
///
/// Generated forms implement this trait to provide a consistent interface
/// for form code identification, filing period, computation, validation,
/// and BIR XML export.
pub trait TypedBirForm: FormValidator {
    /// BIR form code (e.g., "2551Q", "0605").
    fn form_code(&self) -> &'static str;

    /// Versioned form type ID (e.g., "2551Qv2018", "0605v1999").
    fn form_type_id(&self) -> &'static str;

    /// Filing period for this form instance.
    fn filing_period(&self) -> FilingPeriod;

    /// Recompute all derived fields from user-entered data.
    fn recompute(&mut self);

    /// Convert to BIR field map for XML export and PDF rendering.
    fn to_bir_field_map(&self) -> BTreeMap<String, String>;

    /// Generate BIR pseudo-XML payload.
    fn to_bir_xml(&self) -> String {
        crate::bir_xml::generate_bir_xml(&self.to_bir_field_map())
    }
}

/// Status of a form draft or filed return.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum FilingStatus {
    #[default]
    Draft,
    Queued,
    #[serde(alias = "Filed")]
    Submitted,
    Confirmed,
    Paid,
}

/// Summary record returned by database list queries (no full JSON needed).
#[derive(Debug, Clone)]
pub struct FormDraftSummary {
    pub id: i64,
    pub tin: String,
    pub form_code: String,
    pub taxable_year: u16,
    pub quarter: Option<u8>,
    pub month: Option<u8>,
    pub status: FilingStatus,
    pub updated_at: String,
}

impl FormDraftSummary {
    pub fn quarter_state(&self) -> QuarterState {
        match self.status {
            FilingStatus::Draft => QuarterState::Draft,
            FilingStatus::Queued => QuarterState::Queued,
            FilingStatus::Submitted => QuarterState::Submitted,
            FilingStatus::Confirmed => QuarterState::Confirmed,
            FilingStatus::Paid => QuarterState::Paid,
        }
    }
}

/// Per-quarter filing state for a form card on the dashboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuarterState {
    NotStarted,
    Draft,
    Queued,
    Submitted,
    Confirmed,
    Paid,
}

/// Aggregated filing progress for a single form in a given year.
#[derive(Debug, Clone)]
pub struct FormFilingProgress {
    pub form_code: String,
    pub taxable_year: u16,
    /// For quarterly forms: index 0 = Q1, 1 = Q2, 2 = Q3, 3 = Q4
    pub quarters: [QuarterState; 4],
    /// For monthly forms: index 0 = Jan, 11 = Dec
    pub months: [QuarterState; 12],
    /// For annual forms
    pub annual_status: QuarterState,
    /// For open-ended forms: total filed this year
    pub open_ended_count: u32,
}

impl FormFilingProgress {
    pub fn new_empty(form_code: &str, year: u16) -> Self {
        Self {
            form_code: form_code.to_string(),
            taxable_year: year,
            quarters: [
                QuarterState::NotStarted,
                QuarterState::NotStarted,
                QuarterState::NotStarted,
                QuarterState::NotStarted,
            ],
            months: [
                QuarterState::NotStarted,
                QuarterState::NotStarted,
                QuarterState::NotStarted,
                QuarterState::NotStarted,
                QuarterState::NotStarted,
                QuarterState::NotStarted,
                QuarterState::NotStarted,
                QuarterState::NotStarted,
                QuarterState::NotStarted,
                QuarterState::NotStarted,
                QuarterState::NotStarted,
                QuarterState::NotStarted,
            ],
            annual_status: QuarterState::NotStarted,
            open_ended_count: 0,
        }
    }

    /// How many quarters have been filed (for quarterly forms).
    pub fn filed_count(&self) -> u32 {
        self.quarters
            .iter()
            .filter(|q| {
                **q == QuarterState::Queued
                    || **q == QuarterState::Submitted
                    || **q == QuarterState::Confirmed
                    || **q == QuarterState::Paid
            })
            .count() as u32
    }

    /// Next quarter that hasn't been started yet (1-based). Returns None if all 4 submitted/confirmed.
    pub fn next_unfiled_quarter(&self) -> Option<u8> {
        self.quarters
            .iter()
            .enumerate()
            .find(|(_, s)| **s == QuarterState::NotStarted)
            .map(|(i, _)| (i + 1) as u8)
    }

    /// Next quarter that is a draft in progress (1-based).
    pub fn draft_quarter(&self) -> Option<u8> {
        self.quarters
            .iter()
            .enumerate()
            .find(|(_, s)| **s == QuarterState::Draft || **s == QuarterState::Queued)
            .map(|(i, _)| (i + 1) as u8)
    }
}

pub enum FormDraft {
    Form2551Q(Box<Form2551QDraft>),
    Form1601C(Box<Form1601CDraft>),
    Form0605(Box<form_0605::Form0605Draft>),
    Form0619E(Box<form_0619e::Form0619EDraft>),
    Form0619F(Box<form_0619f::Form0619FDraft>),
    Form1701(Box<form_1701::Form1701Draft>),
    Form1701Q(Box<form_1701q::Form1701QDraft>),
    Form1702MX(Box<form_1702mx::Form1702MXDraft>),
    Form1702RT(Box<form_1702rt::Form1702RTDraft>),
    Form2550Q(Box<form_2550q::Form2550QDraft>),
}

impl FormDraft {
    pub fn form_code(&self) -> &'static str {
        match self {
            Self::Form2551Q(_) => "2551Q",
            Self::Form1601C(_) => "1601C",
            Self::Form0605(_) => "0605",
            Self::Form0619E(_) => "0619E",
            Self::Form0619F(_) => "0619F",
            Self::Form1701(_) => "1701",
            Self::Form1701Q(_) => "1701Q",
            Self::Form1702MX(_) => "1702MX",
            Self::Form1702RT(_) => "1702RT",
            Self::Form2550Q(_) => "2550Q",
        }
    }

    pub fn status(&self) -> &FilingStatus {
        match self {
            Self::Form2551Q(f) => &f.status,
            Self::Form1601C(f) => &f.status,
            Self::Form0605(f) => &f.lifecycle.status,
            Self::Form0619E(f) => &f.lifecycle.status,
            Self::Form0619F(f) => &f.lifecycle.status,
            Self::Form1701(f) => &f.lifecycle.status,
            Self::Form1701Q(f) => &f.lifecycle.status,
            Self::Form1702MX(f) => &f.lifecycle.status,
            Self::Form1702RT(f) => &f.lifecycle.status,
            Self::Form2550Q(f) => &f.lifecycle.status,
        }
    }

    pub fn validate(&self) -> Vec<(String, String)> {
        match self {
            Self::Form2551Q(f) => f.validate(),
            Self::Form1601C(f) => f.validate(),
            Self::Form0605(f) => f.validate(),
            Self::Form0619E(f) => f.validate(),
            Self::Form0619F(f) => f.validate(),
            Self::Form1701(f) => f.validate(),
            Self::Form1701Q(f) => f.validate(),
            Self::Form1702MX(f) => f.validate(),
            Self::Form1702RT(f) => f.validate(),
            Self::Form2550Q(f) => f.validate(),
        }
    }
}
