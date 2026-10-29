//! Revision-pinned, UI-agnostic BIR validation rule contracts.
//!
//! The canonical extracted evidence remains in the repository-level `rules/`
//! directory. This crate packages only reviewed compiled snapshots and strict
//! runtime contracts. It deliberately has no GPUI, database, XML transport,
//! networking, floating-point amount, or runtime JSON-corpus dependency.

#![forbid(unsafe_code)]
// These contracts are fail-closed: an error carries the offending rule, field
// instance and revision key so a caller can report exactly why a filing path
// stayed closed. Boxing those payloads would put the diagnostic surface behind
// an indirection for a stack-size win this crate does not need.
#![allow(clippy::result_large_err)]
// Rule construction and evaluation take the full pinned context (revision key,
// phase, field instance, provenance) as explicit arguments rather than a
// loosely-typed bag, which is the point of these contracts.
#![allow(clippy::too_many_arguments)]

mod context;
mod evaluation;
pub mod generated;
mod identity;
mod issue;
mod materialization;
mod provider;
mod registry;
pub mod serialization;
pub mod serialization_contract;
pub mod static_ir;
mod value;
mod workflow;

pub use context::{
    BehaviorProfile, ContextFingerprint, InputRevision, ValidationContext, ValidationPhase,
};
pub use evaluation::{
    DerivedOutputExpectation, DerivedValue, EvaluationError, EvaluationExpectation,
    EvaluationOutput, EvaluationRequest, EvaluationResult, FieldValueAssignment,
    FieldValueAssignmentExpectation,
};
pub use identity::{
    CalculationId, ContextValueId, FieldId, FormCode, FormRevision, FormRevisionKey, IdentityError,
    OfficialPackageVersion, OutputId, RepeatedGroupId, RuleId, RuleSetId, Sha256Digest,
    StableInstanceId, WorkflowStateId, WorkflowTransitionId, XmlKey,
};
pub use issue::{
    IssueOrder, ReportError, RuleAssessment, RuleExecution, RuleExpectation, RuleFieldRef,
    RuleFieldRefError, RuleSeverity, RuleViolation, SerializedOccurrence, ValidationReport,
};
pub use materialization::{
    ContractEmissionId, GroupAccountingView, MaterializationError, MaterializationTraceEntryView,
    MaterializedBindingView, MaterializedOmissionView, MaterializedRecordView,
    MaterializedValueSourceView, SerializationMaterialization,
};
pub use provider::CompiledRuleSet;
pub use registry::{RegistryError, RuleSetRegistry, RuleSetRegistryEntry, StaticRuleSetRegistry};
pub use serialization_contract::StaticSerializationContract;
pub use static_ir::{
    InterpreterError, SerializationInspectionError, SerializationInspector, StaticSpecError,
    WorkflowTransitionError,
};
pub use value::{
    CanonicalDate, CanonicalFieldValue, CanonicalValue, ContextSnapshotError, ContextValue,
    ContextValueSnapshot, DateError, DecimalError, ExactDecimal, FieldInstance, FieldValueSource,
    InputSnapshotError, RawFieldValue, RawInputSnapshot, RawValue, RepeatedGroupInstance,
};
pub use workflow::{
    WorkflowAction, WorkflowNotification, WorkflowNotificationChannel, WorkflowTransitionResult,
};
