//! Inert boundary between core form adapters and packaged compiled rule sets.
//!
//! This module does not select a current form revision, register a form, or
//! alter UI, draft, queue, export, or submission behavior. Generic callers
//! supply an exact [`bir_rules::FormRevisionKey`] through an
//! [`bir_rules::EvaluationRequest`]; the 2550Q live facade owns request
//! construction and accepts only an exact revision pin.
//!
//! This seam is deliberately inert: `payload::CheckedFinalCopyPayload::try_new`
//! always returns `Err`, and the 2550Q designation always returns `None`. The
//! constructors and accessors that a future reviewed path would call therefore
//! have no production caller yet. They are reviewed scaffolding, not leftovers —
//! deleting them to satisfy `dead_code` would erode the boundary the review
//! flow is built around. Error payloads likewise carry full provenance so a
//! caller can report exactly why a filing path stayed closed.
#![allow(
    dead_code,
    clippy::result_large_err,
    clippy::enum_variant_names,
    clippy::type_complexity
)]

mod checked_serialization;
mod form_2550q;
mod payload;
#[allow(dead_code)]
mod plaintext_artifact;
mod registry;
mod shadow;
mod submission_preflight;

pub use checked_serialization::{CheckedSerializationArtifact, CheckedSerializationArtifactError};
#[cfg(test)]
pub(crate) use form_2550q::Form2550QFieldValueSource;
pub use form_2550q::{
    Form2550QCaptureError, Form2550QCaptureGap, Form2550QGroupBinding,
    Form2550QLiveValidationFacade, Form2550QMemberBinding, Form2550QRawFieldKey,
    Form2550QRawFieldKeyError, Form2550QRepoDiagnostic, Form2550QRepoDiagnosticSetupOutcome,
    Form2550QRepoLiveValidationState, Form2550QRepoLiveValidationUnavailableReason,
    form_2550q_group_bindings, form_2550q_local_print_raw_field_keys,
    form_2550q_singleton_field_ids, prepare_form_2550q_live_row_identities,
};
// UI-safe validated contracts needed by the raw editor/live facade. These are
// re-exports only; no unchecked constructor or fallback identity is added.
pub use bir_rules::{
    ContextValue, FieldInstance, FormRevisionKey, InputRevision, RawValue, StableInstanceId,
    ValidationPhase, WorkflowAction, WorkflowStateId, WorkflowTransitionResult,
};
pub(crate) use form_2550q::{
    seed_raw_editor_state_from_reviewed_fields, validate_form_2550q_raw_bindings,
};
// Consumed only by the `db::form_rule_state` coverage tests.
#[cfg(test)]
pub(crate) use payload::FinalCopyFieldCoverage;
pub use payload::{CheckedFinalCopyPayload, CheckedFinalCopyPayloadError};
pub use registry::{
    FormRuleEvaluator, TrustedEvaluation, TrustedEvaluationError, WorkflowDispatchError,
};
pub use shadow::{
    EvaluationStamp, ShadowDifference, ShadowDifferenceKind, ShadowDifferenceReport,
    ShadowEvaluationOutcome,
};
pub use submission_preflight::{
    PersistedSubmissionAuthorityInput, SubmissionPreflightArtifact, SubmissionPreflightError,
    preflight_active_form_submission,
};
