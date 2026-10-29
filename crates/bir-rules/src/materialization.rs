//! Opaque, deterministic materialization of one reviewed serialization plan.
//!
//! This module exposes an accounting manifest only. It deliberately does not
//! construct artifact bytes or authorize any filing/container operation.

use crate::{
    BehaviorProfile, CalculationId, CanonicalValue, ContextFingerprint, ContextValueId,
    EvaluationError, FieldInstance, FormRevisionKey, InputRevision, InterpreterError, OutputId,
    RepeatedGroupInstance, Sha256Digest, ValidationContext, ValidationPhase,
    serialization::{
        SerializationArtifactIdentity, SerializationArtifactTarget, SerializationError,
    },
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{error::Error, fmt};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct ContractEmissionId {
    ordinal: u32,
    group_path: Vec<RepeatedGroupInstance>,
}

impl ContractEmissionId {
    pub(crate) fn new(ordinal: u32, group_path: Vec<RepeatedGroupInstance>) -> Self {
        Self {
            ordinal,
            group_path,
        }
    }

    pub const fn ordinal(&self) -> u32 {
        self.ordinal
    }

    pub fn group_path(&self) -> &[RepeatedGroupInstance] {
        &self.group_path
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum MaterializedBindingView {
    PseudoXmlField { key: String, occurrence: u32 },
    MetadataElement { exact_tag: String },
    ReviewedLiteral { exact_bytes: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum MaterializedValueSourceView {
    None,
    Field {
        field: FieldInstance,
    },
    Derived {
        calculation_id: CalculationId,
        output_id: OutputId,
        instance: Option<RepeatedGroupInstance>,
    },
    Context {
        context_value_id: ContextValueId,
    },
    Constant {
        value: CanonicalValue,
    },
    Default {
        value: CanonicalValue,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MaterializedOmissionView {
    Emitted,
    PresenceFalse,
    ContractOmitted,
    SemanticAbsent,
    SemanticBlank,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MaterializedRecordView {
    emission_id: ContractEmissionId,
    binding: MaterializedBindingView,
    value_source: MaterializedValueSourceView,
    omission: MaterializedOmissionView,
    semantic_value: Option<CanonicalValue>,
    semantic_body: Option<String>,
    encoded_body: Option<String>,
}

impl MaterializedRecordView {
    pub(crate) fn new(
        emission_id: ContractEmissionId,
        binding: MaterializedBindingView,
        value_source: MaterializedValueSourceView,
        omission: MaterializedOmissionView,
        semantic_value: Option<CanonicalValue>,
        semantic_body: Option<String>,
        encoded_body: Option<String>,
    ) -> Self {
        Self {
            emission_id,
            binding,
            value_source,
            omission,
            semantic_value,
            semantic_body,
            encoded_body,
        }
    }

    pub fn emission_id(&self) -> &ContractEmissionId {
        &self.emission_id
    }
    pub fn binding(&self) -> &MaterializedBindingView {
        &self.binding
    }
    pub fn value_source(&self) -> &MaterializedValueSourceView {
        &self.value_source
    }
    pub const fn omission(&self) -> MaterializedOmissionView {
        self.omission
    }
    pub fn semantic_value(&self) -> Option<&CanonicalValue> {
        self.semantic_value.as_ref()
    }
    pub fn semantic_body(&self) -> Option<&str> {
        self.semantic_body.as_deref()
    }
    pub fn encoded_body(&self) -> Option<&str> {
        self.encoded_body.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GroupAccountingView {
    emission_id: ContractEmissionId,
    group_id: String,
    instances: Vec<RepeatedGroupInstance>,
}

impl GroupAccountingView {
    pub(crate) fn new(
        emission_id: ContractEmissionId,
        group_id: String,
        instances: Vec<RepeatedGroupInstance>,
    ) -> Self {
        Self {
            emission_id,
            group_id,
            instances,
        }
    }
    pub fn emission_id(&self) -> &ContractEmissionId {
        &self.emission_id
    }
    pub fn group_id(&self) -> &str {
        &self.group_id
    }
    pub fn instances(&self) -> &[RepeatedGroupInstance] {
        &self.instances
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "entry", content = "value", rename_all = "kebab-case")]
pub enum MaterializationTraceEntryView {
    Record(MaterializedRecordView),
    GroupAccounting(GroupAccountingView),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SerializationMaterialization {
    rule_set: FormRevisionKey,
    context: ValidationContext,
    input_revision: InputRevision,
    context_fingerprint: ContextFingerprint,
    artifact_id: String,
    artifact: SerializationArtifactIdentity,
    contract_digest: Sha256Digest,
    raw_input_digest: Sha256Digest,
    evaluation_digest: Sha256Digest,
    record_manifest_digest: Sha256Digest,
    trace: Vec<MaterializationTraceEntryView>,
}

impl SerializationMaterialization {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        rule_set: FormRevisionKey,
        context: ValidationContext,
        input_revision: InputRevision,
        context_fingerprint: ContextFingerprint,
        artifact_id: String,
        artifact: SerializationArtifactIdentity,
        contract_digest: Sha256Digest,
        raw_input_digest: Sha256Digest,
        evaluation_digest: Sha256Digest,
        trace: Vec<MaterializationTraceEntryView>,
    ) -> Self {
        let record_manifest_digest =
            digest_serializable(b"bir-rules/serialization-record-manifest/v2\0", &trace);
        Self {
            rule_set,
            context,
            input_revision,
            context_fingerprint,
            artifact_id,
            artifact,
            contract_digest,
            raw_input_digest,
            evaluation_digest,
            record_manifest_digest,
            trace,
        }
    }

    pub fn rule_set(&self) -> &FormRevisionKey {
        &self.rule_set
    }
    pub fn artifact_id(&self) -> &str {
        &self.artifact_id
    }
    pub const fn context(&self) -> ValidationContext {
        self.context
    }
    pub const fn input_revision(&self) -> InputRevision {
        self.input_revision
    }
    pub const fn context_fingerprint(&self) -> ContextFingerprint {
        self.context_fingerprint
    }
    pub fn artifact(&self) -> &SerializationArtifactIdentity {
        &self.artifact
    }
    pub const fn contract_digest(&self) -> Sha256Digest {
        self.contract_digest
    }
    pub const fn raw_input_digest(&self) -> Sha256Digest {
        self.raw_input_digest
    }
    pub const fn evaluation_digest(&self) -> Sha256Digest {
        self.evaluation_digest
    }
    pub const fn record_manifest_digest(&self) -> Sha256Digest {
        self.record_manifest_digest
    }
    pub fn trace(&self) -> &[MaterializationTraceEntryView] {
        &self.trace
    }
    pub fn records(&self) -> impl Iterator<Item = &MaterializedRecordView> {
        self.trace.iter().filter_map(|entry| match entry {
            MaterializationTraceEntryView::Record(record) => Some(record),
            MaterializationTraceEntryView::GroupAccounting(_) => None,
        })
    }
    pub fn group_accounting(&self) -> impl Iterator<Item = &GroupAccountingView> {
        self.trace.iter().filter_map(|entry| match entry {
            MaterializationTraceEntryView::Record(_) => None,
            MaterializationTraceEntryView::GroupAccounting(group) => Some(group),
        })
    }
}

#[derive(Debug)]
pub enum MaterializationError {
    RuleSetMismatch,
    ArtifactSelection {
        matches: usize,
    },
    MissingContractDigest,
    InvalidContractDigest,
    UnsupportedContractVersion,
    PhaseMismatch {
        target: SerializationArtifactTarget,
        actual: ValidationPhase,
    },
    HistoricalImportUnsupported,
    BranchUnavailable {
        profile: BehaviorProfile,
    },
    InvalidContractStructure {
        ordinal: u32,
    },
    NestedDynamicGroup,
    GroupCardinality,
    ProjectionOverflow,
    InvalidProjectedKey,
    OccurrenceGap {
        key: String,
        expected: u32,
        actual: u32,
    },
    DuplicateEmissionId {
        id: ContractEmissionId,
    },
    BindingCollision,
    MissingValue,
    Evaluation(EvaluationError),
    Interpreter(InterpreterError),
    Formatting(SerializationError),
}

impl fmt::Display for MaterializationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuleSetMismatch => formatter.write_str("materialization rule-set mismatch"),
            Self::ArtifactSelection { matches } => {
                write!(
                    formatter,
                    "exact artifact selection matched {matches} contracts"
                )
            }
            Self::MissingContractDigest => {
                formatter.write_str("serialization contract digest is missing")
            }
            Self::InvalidContractDigest => {
                formatter.write_str("serialization contract digest is invalid")
            }
            Self::UnsupportedContractVersion => {
                formatter.write_str("serialization contract version is unsupported")
            }
            Self::PhaseMismatch { target, actual } => write!(
                formatter,
                "artifact target {target:?} is invalid for phase {actual:?}"
            ),
            Self::HistoricalImportUnsupported => {
                formatter.write_str("historical import requires a dedicated import context")
            }
            Self::BranchUnavailable { profile } => write!(
                formatter,
                "serialization branch is unavailable for {profile:?}"
            ),
            Self::InvalidContractStructure { ordinal } => write!(
                formatter,
                "serialization contract node {ordinal} has an invalid static structure"
            ),
            Self::NestedDynamicGroup => {
                formatter.write_str("nested dynamic groups are unsupported")
            }
            Self::GroupCardinality => {
                formatter.write_str("dynamic-group cardinality differs from the contract")
            }
            Self::ProjectionOverflow => {
                formatter.write_str("serialization index projection overflow")
            }
            Self::InvalidProjectedKey => {
                formatter.write_str("projected serialization key is invalid")
            }
            Self::OccurrenceGap {
                key,
                expected,
                actual,
            } => write!(
                formatter,
                "occurrence gap for key {key:?}: expected {expected}, got {actual}"
            ),
            Self::DuplicateEmissionId { id } => {
                write!(formatter, "duplicate contract emission ID {id:?}")
            }
            Self::BindingCollision => formatter.write_str("serialization binding collision"),
            Self::MissingValue => {
                formatter.write_str("declared serialization value does not resolve")
            }
            Self::Evaluation(error) => write!(formatter, "evaluation failed: {error}"),
            Self::Interpreter(error) => {
                write!(formatter, "materialization interpreter failed: {error}")
            }
            Self::Formatting(error) => {
                write!(formatter, "serialization formatting failed: {error}")
            }
        }
    }
}

impl Error for MaterializationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Evaluation(error) => Some(error),
            Self::Interpreter(error) => Some(error),
            Self::Formatting(error) => Some(error),
            _ => None,
        }
    }
}

pub(crate) fn digest_serializable(domain: &'static [u8], value: &impl Serialize) -> Sha256Digest {
    let encoded = serde_json::to_vec(value).expect("typed materialization values serialize");
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(encoded);
    Sha256Digest::from_bytes(hasher.finalize().into())
}
