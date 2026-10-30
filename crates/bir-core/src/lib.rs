#![allow(clippy::collapsible_if)]
//! # bir-core
//!
//! Core library for the BIR eBIRForms replacement.
//!
//! This crate provides all business logic for Philippine tax return filing:
//! - BIR pseudo-XML format parsing and generation
//! - Taxpayer profile management with encrypted storage
//! - Form schema engine with validation
//! - The submission queue (leases, retries) over an injected transport
//! - Reference data (RDO, ATC codes, regions, etc.)
//!
//! This crate is UI-agnostic and opens no network connection. IAF
//! encryption and the BIR upload live server-side, never here.

// Links SQLCipher's LibTomCrypt build, which rusqlite's `sqlcipher` feature
// uses on Android.
#[cfg(target_os = "android")]
use sqlcipher_tomcrypt as _;

pub mod background_cron;
pub mod bir_xml;
pub mod calendar_events;
pub mod calendar_ics;
pub mod calendar_rules;
pub mod db;
pub mod deadline_overrides;
pub mod export;
pub mod filing_queue;
pub mod form_rules;
pub mod forms;
pub mod import;
pub mod integration;
pub mod job_display;
#[cfg(target_os = "macos")]
// Unit tests run on a mock store, so the native store is unused there.
#[cfg_attr(test, allow(dead_code))]
mod keychain;

/// `BIR_NO_KEYCHAIN=1`: this process never reads or writes the OS keychain,
/// for unattended runs where a keychain prompt would stall them. The
/// database key then comes from `BIR_DB_KEY` / `BIR_DB_KEY_FILE`.
pub fn keychain_disabled() -> bool {
    std::env::var_os("BIR_NO_KEYCHAIN").is_some_and(|value| !value.is_empty() && value != "0")
}
pub mod legacy_migration;
pub mod log_files;
pub mod logging;
pub mod naming;
pub mod official_holidays;
pub mod official_import;
pub mod official_xml;
pub mod penalties;
pub mod platform;
pub mod profile;
pub mod receipt;
pub mod reference;
pub mod tax_profile;
pub mod time_utils;
pub mod validation;

// Re-export core types
pub use bir_rules::{
    BehaviorProfile, CompiledRuleSet, ContextFingerprint, ContextValueSnapshot, EvaluationResult,
    FieldValueSource, FormRevisionKey, InputRevision, RuleAssessment, RuleFieldRef, RuleSeverity,
    RuleViolation, ValidationContext, ValidationPhase, ValidationReport, WorkflowAction,
    WorkflowNotification, WorkflowNotificationChannel, WorkflowStateId, WorkflowTransitionError,
    WorkflowTransitionId, WorkflowTransitionResult,
};
pub use bir_xml::{
    BirXmlParseError, generate_bir_xml, parse_bir_xml_checked, parse_bir_xml_with_codec_checked,
};
pub use export::{
    export_database_zip, export_existing_database_zip, export_profile_backup_as_admin,
    export_profile_data,
};
pub use filing_queue::{QueueAuthSource, QueueAuthorization, QueueCompletion, parse_1601c_period};
pub use form_rules::{
    CheckedFinalCopyPayload, CheckedFinalCopyPayloadError, ShadowEvaluationOutcome,
    TrustedEvaluation, WorkflowDispatchError,
};
pub use forms::{
    ATC_TABLE_2551Q, FilingStatus, Form2551QDraft, FormDraftSummary, FormFilingProgress,
    FormSuggestion, FormSuggestionSource, FormsSetReconcileResult, QuarterState, Schedule1Row,
    find_atc, find_form, reconcile_forms_set_for_year,
};
pub use import::{extract_database_zip, import_profile_data};
pub use integration::{
    FormDraftOutput, FormMapper, IncomeCategory, IncomeSource, Mapper2551Q, MapperError,
    PayloadValidationError, SyncError, SyncResponse, SyncResult, UniversalTaxPayload,
    applicable_forms_for_profile, deadline_applies_to_profile, form_suggestions_for_profile_year,
    import_payload_directory, import_payload_file, process_sync, process_sync_json,
    profile_deadline_overrides_for_year, recurring_obligation_decisions_for_profile_and_year,
    resolve_profile_obligations_for_year,
    resolve_profile_obligations_for_year_with_global_overrides, validate_form_applicability,
    validate_payload,
};
pub use job_display::{FilingJobLabel, format_filing_period_label, format_submit_job_name};
pub use naming::{Tin, iaf_filename, savefile_name};
pub use official_import::{OfficialSavefile, parse_period_code};
pub use profile::{
    ComplianceSourceMode, CorDocumentRef, CorRegistrationFacts, ManualObligationOverride,
    ManualObligationOverrideAction, ProfileDeadlineOverride, RegisteredTaxType,
    ResolvedTaxProfileForYear, TaxClassification, TaxProfileResolutionIssue,
    TaxProfileResolutionIssueKind, TaxProfileVersion, TaxProfileVersionSource,
    TaxProfileVersionStatus, TaxpayerProfile, VatRegistrationTextClassification,
    classify_vat_registration_text,
};
pub use receipt::{BirReceiptConfirmation, split_bir_filename};
pub use time_utils::format_next_run;
pub use validation::{ValidationError, validate_ph_phone, validate_profile, validate_zip};
