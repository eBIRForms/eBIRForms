//! Per-year **Forms Set** — the user-owned, authoritative list of which BIR forms a
//! taxpayer files in a given taxable year.
//!
//! This replaces the rule-based temporal suggestion engine. A Forms Set is established
//! once per taxable year, either from exact form codes extracted from a reviewed
//! Certificate of Registration (COR), from the registered-tax-type fallback when the COR
//! has no exact form list, or by manual selection ([`FormSetSource::Manual`]). It is
//! persisted in the `per_year_forms` table and read by the dashboard and deadline
//! resolver. Different years may hold different sets.

use crate::forms::registry::{FilingFrequency, canonical_form_code, find_form};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const SYSTEM_DEACTIVATED_REASON: &str = "No longer suggested by the confirmed profile timeline";

/// How a [`FormSetEntry`] came to exist — for audit and revert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FormSetSource {
    /// Hand-picked by the user.
    Manual,
    /// Proposed from reviewed COR evidence, then confirmed.
    CorAi,
    /// Exact form code from confirmed, reviewed COR evidence.
    ReviewedCor,
    /// Generated from registered-tax-type fallback because no exact COR list exists.
    InferredTaxType,
    /// Seeded by the one-time migration backfill from existing profile versions.
    MigrationBackfill,
}

impl FormSetSource {
    /// Stable string for DB persistence.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::CorAi => "cor_ai",
            Self::ReviewedCor => "reviewed_cor",
            Self::InferredTaxType => "inferred_tax_type",
            Self::MigrationBackfill => "migration_backfill",
        }
    }

    /// Parse from the DB string; unknown values fall back to `Manual`.
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "cor_ai" => Self::CorAi,
            "reviewed_cor" => Self::ReviewedCor,
            "inferred_tax_type" => Self::InferredTaxType,
            "migration_backfill" => Self::MigrationBackfill,
            _ => Self::Manual,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FormSuggestionSource {
    MigrationBackfill,
    InferredTaxType,
    ReviewedCor,
    ManualOverride,
}

/// Whether a Forms Set entry is safe to use as a filing obligation.
///
/// `NeedsReview` is deliberately fail-closed: an entry may retain the candidate's
/// `active` value for audit/UI purposes, but it is not returned by filing APIs until
/// a user records a manual decision or reconciliation produces an unambiguous result.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FormSetReviewStatus {
    #[default]
    Resolved,
    NeedsReview,
}

impl FormSetReviewStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Resolved => "resolved",
            Self::NeedsReview => "needs_review",
        }
    }

    /// Unknown persisted values fail closed instead of silently becoming fileable.
    pub fn from_str_lossy(value: &str) -> Self {
        match value {
            "resolved" => Self::Resolved,
            _ => Self::NeedsReview,
        }
    }
}

impl FormSuggestionSource {
    fn priority(self) -> u8 {
        match self {
            Self::MigrationBackfill => 0,
            Self::InferredTaxType => 1,
            Self::ReviewedCor => 2,
            Self::ManualOverride => 3,
        }
    }

    fn form_set_source(self) -> FormSetSource {
        match self {
            Self::MigrationBackfill => FormSetSource::MigrationBackfill,
            Self::InferredTaxType => FormSetSource::InferredTaxType,
            Self::ReviewedCor => FormSetSource::ReviewedCor,
            Self::ManualOverride => FormSetSource::Manual,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormSuggestion {
    pub form_code: String,
    pub active: bool,
    pub source: FormSuggestionSource,
    pub reason: Option<String>,
    pub source_reference: Option<String>,
    pub effective_from: Option<NaiveDate>,
    pub effective_until: Option<NaiveDate>,
}

/// Complete evidence for an unresolved, equally authoritative suggestion conflict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormSetConflict {
    pub form_code: String,
    pub message: String,
    pub competing_suggestions: Vec<FormSuggestion>,
}

impl FormSuggestion {
    pub fn active(form_code: impl Into<String>, source: FormSuggestionSource) -> Self {
        Self {
            form_code: form_code.into(),
            active: true,
            source,
            reason: None,
            source_reference: None,
            effective_from: None,
            effective_until: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormsSetReconcileResult {
    pub forms_set: PerYearFormsSet,
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub deactivated: Vec<String>,
    pub preserved_manual: Vec<String>,
    pub conflicts: Vec<FormSetConflict>,
}

/// One form a taxpayer files in a given taxable year.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormSetEntry {
    /// Canonical BIR form code, e.g. `"2551Q"`. May be a non-registry code when `custom`.
    pub form_code: String,
    /// Filing cadence. Defaults from the registry but is overridable per entry.
    pub frequency: FilingFrequency,
    /// `false` = explicitly suppressed for the year (kept for audit rather than deleted).
    pub active: bool,
    /// Where this entry came from.
    pub source: FormSetSource,
    /// `true` = user-added form not in `FORM_REGISTRY` (skips registry validation; will
    /// have no calendar deadlines unless a matching rule exists).
    pub custom: bool,
    /// Optional human note (e.g. "per accountant", "registered for VAT mid-year").
    pub reason: Option<String>,
    /// Auditable document/profile reference carried from the accepted suggestion.
    #[serde(default)]
    pub source_reference: Option<String>,
    /// First day on which the supporting evidence applies.
    #[serde(default)]
    pub effective_from: Option<NaiveDate>,
    /// Last day on which the supporting evidence applies, when closed.
    #[serde(default)]
    pub effective_until: Option<NaiveDate>,
    /// Explicit resolution state. Filing APIs exclude `NeedsReview` entries.
    #[serde(default)]
    pub review_status: FormSetReviewStatus,
    /// Full competing evidence for an unresolved conflict. A manual resolution
    /// changes `review_status` but retains this payload as audit provenance.
    #[serde(default)]
    pub conflict: Option<FormSetConflict>,
}

impl FormSetEntry {
    /// Build an active entry for `form_code`, defaulting the frequency from the registry
    /// (falling back to [`FilingFrequency::OpenEnded`] and `custom = true` for codes not
    /// present in the registry).
    pub fn from_code(form_code: impl Into<String>, source: FormSetSource) -> Self {
        let form_code = form_code.into();
        match find_form(&form_code) {
            Some(def) => Self {
                form_code,
                frequency: def.frequency.clone(),
                active: true,
                source,
                custom: false,
                reason: None,
                source_reference: None,
                effective_from: None,
                effective_until: None,
                review_status: FormSetReviewStatus::Resolved,
                conflict: None,
            },
            None => Self {
                form_code,
                frequency: FilingFrequency::OpenEnded,
                active: true,
                source,
                custom: true,
                reason: None,
                source_reference: None,
                effective_from: None,
                effective_until: None,
                review_status: FormSetReviewStatus::Resolved,
                conflict: None,
            },
        }
    }

    /// True only when this entry is both included and safe for downstream filing.
    pub fn is_filing_active(&self) -> bool {
        self.active && self.review_status == FormSetReviewStatus::Resolved
    }

    pub fn needs_review(&self) -> bool {
        self.review_status == FormSetReviewStatus::NeedsReview
    }

    /// Whether this is an uncatalogued entry created directly in the Forms Set.
    ///
    /// Generated unknown codes retain an evidence reference or effective period
    /// when a later manual include/exclude decision changes their source to
    /// [`FormSetSource::Manual`]. They remain auditable generated obligations,
    /// not deletable user-created rows.
    pub fn is_user_created_custom(&self) -> bool {
        self.custom
            && self.source == FormSetSource::Manual
            && self.source_reference.is_none()
            && self.effective_from.is_none()
            && self.effective_until.is_none()
            && self.conflict.is_none()
    }

    /// Record a user-owned include/exclude decision while retaining generated
    /// conflict evidence for audit.
    pub fn apply_manual_decision(&mut self, active: bool, reason: Option<String>) {
        self.active = active;
        self.source = FormSetSource::Manual;
        self.reason = reason;
        self.review_status = FormSetReviewStatus::Resolved;
    }
}

/// Lightweight card representation of a form for UI / suggestion purposes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FormCardData {
    pub form_code: String,
    pub category: String,
    pub title: String,
    pub frequency: FilingFrequency,
}

/// The full Forms Set for one `(TIN, taxable_year)`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PerYearFormsSet {
    pub taxable_year: u16,
    pub entries: Vec<FormSetEntry>,
}

impl PerYearFormsSet {
    pub fn new(taxable_year: u16) -> Self {
        Self {
            taxable_year,
            entries: Vec::new(),
        }
    }

    /// Build a set from a list of form codes, all active, with the given source.
    pub fn from_codes<I, S>(taxable_year: u16, codes: I, source: FormSetSource) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            taxable_year,
            entries: codes
                .into_iter()
                .map(|c| FormSetEntry::from_code(c, source))
                .collect(),
        }
    }

    /// Whether any entry exists for this year (active or not).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The active form codes — the authoritative list the dashboard files against.
    pub fn active_form_codes(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| e.is_filing_active())
            .map(|e| e.form_code.clone())
            .collect()
    }

    /// Whether `form_code` is present and active.
    pub fn contains_active(&self, form_code: &str) -> bool {
        self.entries
            .iter()
            .any(|e| e.is_filing_active() && e.form_code == form_code)
    }

    /// Look up an entry by code.
    pub fn entry(&self, form_code: &str) -> Option<&FormSetEntry> {
        self.entries.iter().find(|e| e.form_code == form_code)
    }

    /// Unresolved entries that must be reviewed before they can drive filing.
    pub fn needs_review_entries(&self) -> impl Iterator<Item = &FormSetEntry> {
        self.entries.iter().filter(|entry| entry.needs_review())
    }

    /// Remove a custom obligation created by the user.
    ///
    /// Registry-unknown codes from reviewed COR or inferred profile evidence
    /// also carry `custom = true`, but remain generated audit records and must
    /// be included or excluded rather than erased.
    pub fn remove_manual_custom_entry(&mut self, form_code: &str) -> bool {
        let before = self.entries.len();
        self.entries
            .retain(|entry| !(entry.form_code == form_code && entry.is_user_created_custom()));
        self.entries.len() != before
    }

    /// Enforce "one annual ITR per year" on already-stored data.
    ///
    /// When two or more members of an annual-ITR group are active, keep the
    /// one the current tax facts imply (`keep_individual` / `keep_corporate`)
    /// active and deactivate the rest. When none of the active entries matches
    /// the implied primary — e.g. legacy data with no clear match — the first
    /// active member in canonical group order is kept so exactly one survives.
    /// Returns whether any entry changed. Idempotent.
    pub fn deactivate_redundant_annual_itrs(
        &mut self,
        keep_individual: Option<&str>,
        keep_corporate: Option<&str>,
    ) -> bool {
        const INDIVIDUAL: &[&str] = &["1700", "1701", "1701A", "1701MS"];
        const CORPORATE: &[&str] = &["1702", "1702RT", "1702EX", "1702MX"];

        let mut changed = false;
        for (group, preferred) in [(INDIVIDUAL, keep_individual), (CORPORATE, keep_corporate)] {
            let active: Vec<String> = self
                .entries
                .iter()
                .filter(|entry| {
                    entry.is_filing_active() && group.contains(&entry.form_code.as_str())
                })
                .map(|entry| entry.form_code.clone())
                .collect();
            if active.len() < 2 {
                continue;
            }
            // Choose the survivor: the implied primary if it is one of the
            // active entries, otherwise the first active member in group order.
            let keep = preferred
                .filter(|code| active.iter().any(|active_code| active_code == code))
                .map(str::to_string)
                .or_else(|| {
                    group
                        .iter()
                        .find(|code| active.iter().any(|active_code| active_code == *code))
                        .map(|code| code.to_string())
                });
            let Some(keep) = keep else {
                continue;
            };
            for entry in &mut self.entries {
                if entry.form_code != keep
                    && group.contains(&entry.form_code.as_str())
                    && entry.is_filing_active()
                {
                    entry.active = false;
                    entry.review_status = FormSetReviewStatus::Resolved;
                    entry.reason = Some(format!(
                        "Deactivated automatically: a taxpayer files only one annual ITR per year, and {keep} is active."
                    ));
                    changed = true;
                }
            }
        }
        changed
    }
}

/// Reconciles generated suggestions with the user-owned Forms Set.
///
/// Manual entries and prior user suppressions win. Generated entries that are
/// no longer suggested remain present but inactive for auditability.
pub fn reconcile_forms_set_for_year(
    taxable_year: u16,
    existing: Option<&PerYearFormsSet>,
    suggestions: &[FormSuggestion],
) -> FormsSetReconcileResult {
    let mut conflicts = Vec::new();
    let mut suggestions_by_code = BTreeMap::<String, Vec<FormSuggestion>>::new();

    let preserved_manual_codes = existing
        .into_iter()
        .flat_map(|set| set.entries.iter())
        .filter(|entry| {
            entry.source == FormSetSource::Manual
                || (entry.review_status == FormSetReviewStatus::Resolved
                    && !entry.active
                    && entry.reason.as_deref() != Some(SYSTEM_DEACTIVATED_REASON))
        })
        .map(|entry| canonical_form_code(&entry.form_code))
        .collect::<std::collections::BTreeSet<_>>();

    for suggestion in suggestions {
        let code = canonical_form_code(suggestion.form_code.trim());
        if code.is_empty() || preserved_manual_codes.contains(&code) {
            continue;
        }
        let mut candidate = suggestion.clone();
        candidate.form_code = code.clone();
        suggestions_by_code.entry(code).or_default().push(candidate);
    }

    let mut resolved_suggestions =
        BTreeMap::<String, Result<FormSuggestion, FormSetConflict>>::new();
    for (code, candidates) in suggestions_by_code {
        let highest_priority = candidates
            .iter()
            .map(|candidate| candidate.source.priority())
            .max()
            .unwrap_or_default();
        let mut authoritative = candidates
            .into_iter()
            .filter(|candidate| candidate.source.priority() == highest_priority)
            .collect::<Vec<_>>();
        sort_suggestions_for_audit(&mut authoritative);

        let has_active = authoritative.iter().any(|candidate| candidate.active);
        let has_inactive = authoritative.iter().any(|candidate| !candidate.active);
        if has_active && has_inactive {
            let conflict = FormSetConflict {
                form_code: code.clone(),
                message: format!(
                    "Conflicting equally authoritative suggestions for {code}; record a manual include/exclude decision"
                ),
                competing_suggestions: authoritative,
            };
            conflicts.push(conflict.clone());
            resolved_suggestions.insert(code, Err(conflict));
        } else if let Some(candidate) = authoritative.into_iter().next() {
            resolved_suggestions.insert(code, Ok(candidate));
        }
    }

    let mut entries_by_code = BTreeMap::new();
    let mut added = Vec::new();
    let mut updated = Vec::new();
    let mut deactivated = Vec::new();
    let mut preserved_manual = Vec::new();

    if let Some(existing) = existing {
        for existing_entry in &existing.entries {
            let code = canonical_form_code(&existing_entry.form_code);
            let is_manual = existing_entry.source == FormSetSource::Manual
                || (existing_entry.review_status == FormSetReviewStatus::Resolved
                    && !existing_entry.active
                    && existing_entry.reason.as_deref() != Some(SYSTEM_DEACTIVATED_REASON));
            if is_manual {
                let mut preserved = existing_entry.clone();
                preserved.form_code = code.clone();
                preserved.review_status = FormSetReviewStatus::Resolved;
                entries_by_code.insert(code.clone(), preserved);
                preserved_manual.push(code.clone());
                resolved_suggestions.remove(&code);
                continue;
            }

            if let Some(suggestion) = resolved_suggestions.remove(&code) {
                let replacement = match suggestion {
                    Ok(suggestion) => entry_from_suggestion(&suggestion),
                    Err(conflict) => entry_from_conflict(conflict),
                };
                if replacement != *existing_entry {
                    updated.push(code.clone());
                }
                entries_by_code.insert(code, replacement);
            } else {
                let mut inactive = existing_entry.clone();
                inactive.form_code = code.clone();
                if inactive.active || inactive.reason.as_deref() != Some(SYSTEM_DEACTIVATED_REASON)
                {
                    deactivated.push(code.clone());
                }
                inactive.active = false;
                inactive.reason = Some(SYSTEM_DEACTIVATED_REASON.to_string());
                inactive.review_status = FormSetReviewStatus::Resolved;
                inactive.conflict = None;
                entries_by_code.insert(code, inactive);
            }
        }
    }

    for (code, suggestion) in resolved_suggestions {
        added.push(code.clone());
        let entry = match suggestion {
            Ok(suggestion) => entry_from_suggestion(&suggestion),
            Err(conflict) => entry_from_conflict(conflict),
        };
        entries_by_code.insert(code, entry);
    }

    FormsSetReconcileResult {
        forms_set: PerYearFormsSet {
            taxable_year,
            entries: entries_by_code.into_values().collect(),
        },
        added,
        updated,
        deactivated,
        preserved_manual,
        conflicts,
    }
}

fn entry_from_suggestion(suggestion: &FormSuggestion) -> FormSetEntry {
    let mut entry = FormSetEntry::from_code(
        suggestion.form_code.clone(),
        suggestion.source.form_set_source(),
    );
    entry.active = suggestion.active;
    entry.reason = suggestion.reason.clone();
    entry.source_reference = suggestion.source_reference.clone();
    entry.effective_from = suggestion.effective_from;
    entry.effective_until = suggestion.effective_until;
    entry
}

fn entry_from_conflict(conflict: FormSetConflict) -> FormSetEntry {
    let source = conflict
        .competing_suggestions
        .first()
        .map(|suggestion| suggestion.source.form_set_source())
        .unwrap_or(FormSetSource::InferredTaxType);
    let mut entry = FormSetEntry::from_code(conflict.form_code.clone(), source);
    entry.active = false;
    entry.reason = Some(conflict.message.clone());
    entry.review_status = FormSetReviewStatus::NeedsReview;
    entry.conflict = Some(conflict);
    entry
}

fn sort_suggestions_for_audit(suggestions: &mut [FormSuggestion]) {
    suggestions.sort_by(|left, right| {
        left.active
            .cmp(&right.active)
            .then_with(|| left.source_reference.cmp(&right.source_reference))
            .then_with(|| left.effective_from.cmp(&right.effective_from))
            .then_with(|| left.effective_until.cmp(&right.effective_until))
            .then_with(|| left.reason.cmp(&right.reason))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_code_uses_registry_frequency() {
        let entry = FormSetEntry::from_code("2551Q", FormSetSource::Manual);
        assert_eq!(entry.frequency, FilingFrequency::Quarterly);
        assert!(entry.active);
        assert!(!entry.custom);
    }

    #[test]
    fn from_code_unknown_is_custom_openended() {
        let entry = FormSetEntry::from_code("ZZZZ", FormSetSource::Manual);
        assert!(entry.custom);
        assert_eq!(entry.frequency, FilingFrequency::OpenEnded);
    }

    #[test]
    fn manual_custom_entry_can_be_removed() {
        let mut set = PerYearFormsSet::from_codes(2026, ["ZZZZ"], FormSetSource::Manual);

        assert!(set.remove_manual_custom_entry("ZZZZ"));
        assert!(set.entries.is_empty());
    }

    #[test]
    fn deactivate_redundant_annual_itrs_keeps_the_primary_only() {
        let mut set = PerYearFormsSet::from_codes(
            2026,
            ["1701", "1701A", "1701MS", "2551Q"],
            FormSetSource::InferredTaxType,
        );

        // Self-employed default primary is 1701; the other ITRs deactivate,
        // non-ITR forms are untouched.
        assert!(set.deactivate_redundant_annual_itrs(Some("1701"), None));
        assert!(set.contains_active("1701"));
        assert!(!set.contains_active("1701A"));
        assert!(!set.contains_active("1701MS"));
        assert!(set.contains_active("2551Q"));

        // Idempotent: a second pass changes nothing.
        assert!(!set.deactivate_redundant_annual_itrs(Some("1701"), None));
    }

    #[test]
    fn deactivate_redundant_annual_itrs_falls_back_to_group_order() {
        let mut set =
            PerYearFormsSet::from_codes(2026, ["1701A", "1701MS"], FormSetSource::InferredTaxType);

        // The implied primary (1701) is not among the active entries, so the
        // first active member in canonical group order (1701A) survives.
        assert!(set.deactivate_redundant_annual_itrs(Some("1701"), None));
        assert!(set.contains_active("1701A"));
        assert!(!set.contains_active("1701MS"));
    }

    #[test]
    fn deactivate_redundant_annual_itrs_ignores_single_and_cross_group() {
        // One active individual ITR + one active corporate ITR: no conflict
        // within either group, nothing changes.
        let mut set =
            PerYearFormsSet::from_codes(2026, ["1701", "1702RT"], FormSetSource::InferredTaxType);
        assert!(!set.deactivate_redundant_annual_itrs(Some("1701"), Some("1702RT")));
        assert!(set.contains_active("1701"));
        assert!(set.contains_active("1702RT"));
    }

    #[test]
    fn unsupported_reviewed_cor_entry_cannot_be_erased() {
        let mut set = PerYearFormsSet::from_codes(2026, ["ZZZZ"], FormSetSource::ReviewedCor);

        assert!(!set.remove_manual_custom_entry("ZZZZ"));
        assert_eq!(set.entries.len(), 1);
        assert_eq!(set.entries[0].source, FormSetSource::ReviewedCor);
        assert!(set.entries[0].custom);
        assert_eq!(
            crate::forms::form_support_level("ZZZZ").action_label(),
            "Manual / external filing"
        );
    }

    #[test]
    fn manually_excluded_unsupported_suggestion_retains_its_evidence() {
        let mut set = PerYearFormsSet::from_codes(2026, ["ZZZZ"], FormSetSource::ReviewedCor);
        set.entries[0].source_reference = Some("reviewed-cor-document".to_string());
        set.entries[0].effective_from = NaiveDate::from_ymd_opt(2026, 1, 1);
        set.entries[0]
            .apply_manual_decision(false, Some("Not filed for this taxpayer".to_string()));

        assert!(!set.remove_manual_custom_entry("ZZZZ"));
        assert_eq!(set.entries.len(), 1);
        assert!(!set.entries[0].active);
        assert_eq!(set.entries[0].source, FormSetSource::Manual);
        assert_eq!(
            set.entries[0].source_reference.as_deref(),
            Some("reviewed-cor-document")
        );
        assert_eq!(
            set.entries[0].effective_from,
            NaiveDate::from_ymd_opt(2026, 1, 1)
        );
        assert!(!set.entries[0].is_user_created_custom());
    }

    #[test]
    fn active_codes_exclude_inactive() {
        let mut set =
            PerYearFormsSet::from_codes(2026, ["2551Q", "1701Q", "1701"], FormSetSource::CorAi);
        // Suppress one
        set.entries[1].active = false;
        let codes = set.active_form_codes();
        assert!(codes.contains(&"2551Q".to_string()));
        assert!(codes.contains(&"1701".to_string()));
        assert!(!codes.contains(&"1701Q".to_string()));
        assert!(set.contains_active("2551Q"));
        assert!(!set.contains_active("1701Q"));
    }

    #[test]
    fn reconcile_preserves_manual_entry_over_generated_suggestion() {
        let mut existing = PerYearFormsSet::from_codes(2026, ["2550Q"], FormSetSource::Manual);
        existing.entries[0].active = false;
        existing.entries[0].reason = Some("Accountant confirmed non-VAT".into());
        let suggestions = [FormSuggestion::active(
            "2550Q",
            FormSuggestionSource::InferredTaxType,
        )];

        let result = reconcile_forms_set_for_year(2026, Some(&existing), &suggestions);

        assert!(!result.forms_set.contains_active("2550Q"));
        assert_eq!(result.preserved_manual, vec!["2550Q"]);
    }

    #[test]
    fn reconcile_deactivates_removed_generated_entry_without_deleting_it() {
        let existing = PerYearFormsSet::from_codes(2026, ["2551Q"], FormSetSource::InferredTaxType);

        let result = reconcile_forms_set_for_year(2026, Some(&existing), &[]);

        assert_eq!(result.deactivated, vec!["2551Q"]);
        assert!(!result.forms_set.entries[0].active);
    }

    #[test]
    fn reconcile_prefers_reviewed_cor_over_inferred_tax_type() {
        let suggestions = [
            FormSuggestion::active("2551Q", FormSuggestionSource::InferredTaxType),
            FormSuggestion::active("2551Q", FormSuggestionSource::ReviewedCor),
        ];

        let result = reconcile_forms_set_for_year(2026, None, &suggestions);

        assert_eq!(
            result.forms_set.entries[0].source,
            FormSetSource::ReviewedCor
        );
    }

    #[test]
    fn reconcile_preserves_selected_suggestion_provenance() {
        let mut suggestion = FormSuggestion::active("2551Q", FormSuggestionSource::ReviewedCor);
        suggestion.source_reference = Some("cor-document:sha256:abc123".into());
        suggestion.effective_from = NaiveDate::from_ymd_opt(2026, 1, 1);
        suggestion.effective_until = NaiveDate::from_ymd_opt(2026, 12, 31);

        let result = reconcile_forms_set_for_year(2026, None, &[suggestion]);
        let entry = result.forms_set.entry("2551Q").unwrap();

        assert_eq!(
            (
                entry.source_reference.as_deref(),
                entry.effective_from,
                entry.effective_until,
            ),
            (
                Some("cor-document:sha256:abc123"),
                NaiveDate::from_ymd_opt(2026, 1, 1),
                NaiveDate::from_ymd_opt(2026, 12, 31),
            )
        );
    }

    #[test]
    fn reconcile_marks_equal_authority_include_exclude_conflict_needs_review() {
        let mut include = FormSuggestion::active("2551Q", FormSuggestionSource::ReviewedCor);
        include.source_reference = Some("cor-page-1".into());
        let mut exclude = include.clone();
        exclude.active = false;
        exclude.source_reference = Some("cor-page-2".into());

        let result = reconcile_forms_set_for_year(2026, None, &[include, exclude]);
        let entry = result.forms_set.entry("2551Q").unwrap();

        assert!(entry.needs_review());
        assert!(!result.forms_set.contains_active("2551Q"));
        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(
            entry
                .conflict
                .as_ref()
                .map(|conflict| conflict.competing_suggestions.len()),
            Some(2)
        );
    }

    #[test]
    fn manual_decision_resolves_and_survives_conflicting_refresh() {
        let mut include = FormSuggestion::active("2551Q", FormSuggestionSource::ReviewedCor);
        include.source_reference = Some("cor-page-1".into());
        include.effective_from = NaiveDate::from_ymd_opt(2026, 1, 1);
        let mut exclude = include.clone();
        exclude.active = false;
        exclude.source_reference = Some("cor-page-2".into());
        exclude.effective_from = NaiveDate::from_ymd_opt(2026, 7, 1);
        let suggestions = [include, exclude];
        let conflicted = reconcile_forms_set_for_year(2026, None, &suggestions);
        let mut existing = conflicted.forms_set;
        existing.entries[0]
            .apply_manual_decision(true, Some("Accountant reviewed the source conflict".into()));

        let refreshed = reconcile_forms_set_for_year(2026, Some(&existing), &suggestions);
        let entry = refreshed.forms_set.entry("2551Q").unwrap();

        assert!(entry.is_filing_active());
        assert_eq!(entry.source, FormSetSource::Manual);
        assert!(!entry.needs_review());
        assert_eq!(
            entry.conflict.as_ref().map(|conflict| {
                conflict
                    .competing_suggestions
                    .iter()
                    .map(|suggestion| {
                        (
                            suggestion.source_reference.as_deref(),
                            suggestion.effective_from,
                        )
                    })
                    .collect::<Vec<_>>()
            }),
            Some(vec![
                (Some("cor-page-2"), NaiveDate::from_ymd_opt(2026, 7, 1)),
                (Some("cor-page-1"), NaiveDate::from_ymd_opt(2026, 1, 1)),
            ])
        );
        assert!(refreshed.conflicts.is_empty());
        assert_eq!(refreshed.preserved_manual, vec!["2551Q"]);
    }

    #[test]
    fn manual_exclusion_resolves_conflict_and_retains_evidence() {
        let mut include = FormSuggestion::active("2551Q", FormSuggestionSource::ReviewedCor);
        include.source_reference = Some("cor-page-1".into());
        include.effective_from = NaiveDate::from_ymd_opt(2026, 1, 1);
        let mut exclude = include.clone();
        exclude.active = false;
        exclude.source_reference = Some("cor-page-2".into());
        exclude.effective_from = NaiveDate::from_ymd_opt(2026, 7, 1);
        let suggestions = [include, exclude];
        let mut existing = reconcile_forms_set_for_year(2026, None, &suggestions).forms_set;

        existing.entries[0]
            .apply_manual_decision(false, Some("Accountant excluded this form".into()));
        let refreshed = reconcile_forms_set_for_year(2026, Some(&existing), &suggestions);
        let entry = refreshed.forms_set.entry("2551Q").unwrap();

        assert_eq!(
            (
                entry.active,
                entry.needs_review(),
                entry.source,
                entry
                    .conflict
                    .as_ref()
                    .map(|conflict| conflict.competing_suggestions.len()),
                refreshed.preserved_manual,
            ),
            (
                false,
                false,
                FormSetSource::Manual,
                Some(2),
                vec!["2551Q".to_string()],
            )
        );
    }

    #[test]
    fn legacy_entry_json_defaults_to_resolved_review_state() {
        let json = r#"{
            "form_code":"2551Q",
            "frequency":"Quarterly",
            "active":true,
            "source":"ReviewedCor",
            "custom":false,
            "reason":null
        }"#;

        let entry: FormSetEntry = serde_json::from_str(json).unwrap();

        assert_eq!(entry.review_status, FormSetReviewStatus::Resolved);
        assert!(entry.source_reference.is_none());
        assert!(entry.effective_from.is_none());
        assert!(entry.effective_until.is_none());
        assert!(entry.conflict.is_none());
    }
}
