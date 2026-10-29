//! External integration API — models, mappers, validation, and service layer.
//!
//! This module provides a standardized interface for external systems (Odoo, QuickBooks,
//! Taxman, etc.) to push financial data into eBirForms without needing to understand
//! BIR-specific XML field names or form structures.
//!
//! # Architecture
//!
//! ```text
//! External System → JSON payload → service::process_sync_json()
//!                                      ↓
//!                               validation::validate_payload()
//!                                      ↓
//!                               db.get_profile(tin)
//!                                      ↓
//!                               validation::validate_form_applicability()
//!                                      ↓
//!                               mapper::resolve_mappers()
//!                                      ↓
//!                               mapper.map() → Form2551QDraft
//!                                      ↓
//!                               draft.recompute() + draft.validate()
//!                                      ↓
//!                               db.save_2551q_draft()
//!                                      ↓
//!                               SyncResponse { results, warnings }
//! ```

pub mod computation;
pub mod fraud;
pub mod mapper;
pub mod models;
pub mod service;
pub mod simulation;
pub mod transactions;
pub mod validation;
pub mod withholding;

pub use mapper::{FormDraftOutput, FormMapper, Mapper2551Q, MapperError};
pub use models::{IncomeCategory, IncomeSource, UniversalTaxPayload};
pub use service::{
    SyncError, SyncResponse, SyncResult, import_payload_directory, import_payload_file,
    process_sync, process_sync_json,
};
pub use validation::{
    PayloadValidationError, ProfileConsistencyIssue, ProfileConsistencyReport,
    ProfileConsistencySeverity, ResolvedProfileObligations, all_form_codes,
    applicable_form_decisions_for_profile_and_year, applicable_forms_for_profile,
    applicable_forms_for_profile_and_year, check_annual_itr_conflicts,
    conflicting_active_annual_itrs, conflicting_annual_itr_code_groups,
    deadline_applies_to_profile, evaluate_forms_for_year, form_suggestions_for_profile_year,
    primary_annual_itrs_for_profile_year, profile_deadline_overrides_for_year,
    recurring_obligation_decisions_for_profile_and_year,
    recurring_obligation_forms_for_profile_and_year, registered_form_codes_for_version,
    resolve_profile_obligations_for_year,
    resolve_profile_obligations_for_year_with_global_overrides, validate_form_applicability,
    validate_payload,
};
