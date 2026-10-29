use super::TrustedEvaluation;
use crate::bir_xml::{BirXmlParseError, parse_bir_xml_encoded_occurrences_checked};
use bir_rules::{
    BehaviorProfile, CalculationId, CanonicalValue, ContextFingerprint, EvaluationResult, FieldId,
    FieldInstance, FormRevisionKey, InputRevision, OutputId, RawInputSnapshot,
    SerializedOccurrence, Sha256Digest, ValidationContext, ValidationPhase, XmlKey,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};
use thiserror::Error;

const FINAL_COPY_PROOF_VERSION: &str = "checked-final-copy-payload-v2";
const FINAL_COPY_SERIALIZER_VERSION: &str = "bir-pseudo-xml-encoded-occurrences-v2";

/// One exact encoded BIR field covered by a checked Final Copy payload.
///
/// Construction is crate-private so only reviewed form adapters can declare
/// the correspondence between their serializer and the emitted XML.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FinalCopyFieldCoverage {
    serialized_key: XmlKey,
    occurrence: SerializedOccurrence,
    source: FinalCopyValueSource,
    semantic_value: CanonicalValue,
    encoded_value: String,
}

impl FinalCopyFieldCoverage {
    pub(crate) fn canonical_field(
        serialized_key: XmlKey,
        occurrence: SerializedOccurrence,
        field: FieldInstance,
        semantic_value: CanonicalValue,
        encoded_value: String,
    ) -> Self {
        Self {
            serialized_key,
            occurrence,
            source: FinalCopyValueSource::CanonicalField { field },
            semantic_value,
            encoded_value,
        }
    }

    pub(crate) fn derived_output(
        serialized_key: XmlKey,
        occurrence: SerializedOccurrence,
        calculation_id: CalculationId,
        output_id: OutputId,
        semantic_value: CanonicalValue,
        encoded_value: String,
    ) -> Self {
        Self {
            serialized_key,
            occurrence,
            source: FinalCopyValueSource::DerivedOutput {
                calculation_id,
                output_id,
            },
            semantic_value,
            encoded_value,
        }
    }

    pub(crate) fn reviewed_constant(
        serialized_key: XmlKey,
        occurrence: SerializedOccurrence,
        id: FieldId,
        value: CanonicalValue,
        encoded_value: String,
    ) -> Self {
        Self {
            serialized_key,
            occurrence,
            source: FinalCopyValueSource::ReviewedConstant {
                id,
                value: value.clone(),
            },
            semantic_value: value,
            encoded_value,
        }
    }

    pub(crate) fn reviewed_default(
        serialized_key: XmlKey,
        occurrence: SerializedOccurrence,
        id: FieldId,
        value: CanonicalValue,
        encoded_value: String,
    ) -> Self {
        Self {
            serialized_key,
            occurrence,
            source: FinalCopyValueSource::ReviewedDefault {
                id,
                value: value.clone(),
            },
            semantic_value: value,
            encoded_value,
        }
    }
}

/// Closed authority for every value emitted into checked Final Copy XML.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum FinalCopyValueSource {
    CanonicalField {
        field: FieldInstance,
    },
    DerivedOutput {
        calculation_id: CalculationId,
        output_id: OutputId,
    },
    ReviewedConstant {
        id: FieldId,
        value: CanonicalValue,
    },
    ReviewedDefault {
        id: FieldId,
        value: CanonicalValue,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum FinalCopyValueSourceKey {
    CanonicalField(FieldInstance),
    DerivedOutput(CalculationId, OutputId),
    ReviewedValue(FieldId),
}

impl FinalCopyValueSource {
    fn key(&self) -> FinalCopyValueSourceKey {
        match self {
            Self::CanonicalField { field } => {
                FinalCopyValueSourceKey::CanonicalField(field.clone())
            }
            Self::DerivedOutput {
                calculation_id,
                output_id,
            } => FinalCopyValueSourceKey::DerivedOutput(calculation_id.clone(), output_id.clone()),
            Self::ReviewedConstant { id, .. } | Self::ReviewedDefault { id, .. } => {
                FinalCopyValueSourceKey::ReviewedValue(id.clone())
            }
        }
    }

    fn description(&self) -> String {
        match self {
            Self::CanonicalField { field } => {
                format!("canonical field {}", field.field_id())
            }
            Self::DerivedOutput {
                calculation_id,
                output_id,
            } => format!("derived output {calculation_id}:{output_id}"),
            Self::ReviewedConstant { id, .. } => format!("reviewed constant {id}"),
            Self::ReviewedDefault { id, .. } => format!("reviewed default {id}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FinalCopyProofWire {
    proof_version: String,
    serializer_version: String,
    rule_set: FormRevisionKey,
    context: ValidationContext,
    input_revision: InputRevision,
    context_fingerprint: ContextFingerprint,
    request_input_sha256: Sha256Digest,
    coverage_manifest_json: String,
    coverage_manifest_sha256: Sha256Digest,
    xml_byte_len: u64,
    xml_sha256: Sha256Digest,
}

/// Opaque proof that exact BIR XML bytes cover a reviewed serialized manifest.
///
/// Application callers can inspect and pass this value but cannot construct it
/// from an arbitrary `String`. A future reviewed form adapter must use the
/// crate-private constructor with the `TrustedEvaluation` that produced the
/// Final Copy.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedFinalCopyPayload {
    wire: FinalCopyProofWire,
    proof_json: String,
    proof_sha256: Sha256Digest,
    xml_payload: String,
}

impl fmt::Debug for CheckedFinalCopyPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CheckedFinalCopyPayload")
            .field("proof_version", &self.wire.proof_version)
            .field("serializer_version", &self.wire.serializer_version)
            .field("rule_set", &self.wire.rule_set)
            .field("context", &self.wire.context)
            .field("input_revision", &self.wire.input_revision)
            .field("context_fingerprint", &self.wire.context_fingerprint)
            .field(
                "coverage_manifest_sha256",
                &self.wire.coverage_manifest_sha256,
            )
            .field("xml_byte_len", &self.wire.xml_byte_len)
            .field("xml_sha256", &self.wire.xml_sha256)
            .field("proof_sha256", &self.proof_sha256)
            .finish_non_exhaustive()
    }
}

impl CheckedFinalCopyPayload {
    /// Production construction stays closed until a generated, reviewed
    /// serialization contract owns value projection, formatting, encoding,
    /// ordering, and complete artifact coverage.
    ///
    /// Keeping this boundary explicitly fallible prevents a future form
    /// adapter from promoting its own semantic/encoded manifest to trusted
    /// evidence while the generated registry still lacks that contract.
    #[allow(dead_code)]
    pub(crate) fn try_new(
        _trusted_evaluation: &TrustedEvaluation,
        _coverage: Vec<FinalCopyFieldCoverage>,
        _xml_payload: String,
    ) -> Result<Self, CheckedFinalCopyPayloadError> {
        Err(CheckedFinalCopyPayloadError::MissingSerializationContract)
    }

    #[cfg(test)]
    fn try_from_unreviewed_coverage_for_test(
        trusted_evaluation: &TrustedEvaluation,
        coverage: Vec<FinalCopyFieldCoverage>,
        xml_payload: String,
    ) -> Result<Self, CheckedFinalCopyPayloadError> {
        validate_coverage_shape(&coverage)?;
        validate_xml_coverage(&coverage, &xml_payload)?;

        let coverage_manifest_json =
            serde_json::to_string(&coverage).map_err(CheckedFinalCopyPayloadError::serialize)?;
        let request_input_sha256 = digest_serialized(trusted_evaluation.raw_inputs())?;
        let wire = FinalCopyProofWire {
            proof_version: FINAL_COPY_PROOF_VERSION.to_string(),
            serializer_version: FINAL_COPY_SERIALIZER_VERSION.to_string(),
            rule_set: trusted_evaluation.rule_set().clone(),
            context: trusted_evaluation.context(),
            input_revision: trusted_evaluation.input_revision(),
            context_fingerprint: trusted_evaluation.context_fingerprint(),
            request_input_sha256,
            coverage_manifest_sha256: sha256_digest(coverage_manifest_json.as_bytes()),
            coverage_manifest_json,
            xml_byte_len: u64::try_from(xml_payload.len()).map_err(|_| {
                CheckedFinalCopyPayloadError::InvalidProof(
                    "XML byte length cannot be represented by the proof format".to_string(),
                )
            })?,
            xml_sha256: sha256_digest(xml_payload.as_bytes()),
        };
        let proof_json =
            serde_json::to_string(&wire).map_err(CheckedFinalCopyPayloadError::serialize)?;
        let proof_sha256 = sha256_digest(proof_json.as_bytes());
        let payload = Self {
            wire,
            proof_json,
            proof_sha256,
            xml_payload,
        };
        payload.validate_against_trusted(trusted_evaluation)?;
        Ok(payload)
    }

    /// Test-only bridge that still exercises every production proof check.
    #[cfg(test)]
    pub(crate) fn try_from_coverage_for_test(
        trusted_evaluation: &TrustedEvaluation,
        coverage: Vec<FinalCopyFieldCoverage>,
        xml_payload: &str,
    ) -> Result<Self, CheckedFinalCopyPayloadError> {
        Self::try_from_unreviewed_coverage_for_test(
            trusted_evaluation,
            coverage,
            xml_payload.to_string(),
        )
    }

    pub fn proof_version(&self) -> &str {
        &self.wire.proof_version
    }

    pub fn serializer_version(&self) -> &str {
        &self.wire.serializer_version
    }

    pub fn rule_set(&self) -> &FormRevisionKey {
        &self.wire.rule_set
    }

    pub const fn context(&self) -> ValidationContext {
        self.wire.context
    }

    pub const fn input_revision(&self) -> InputRevision {
        self.wire.input_revision
    }

    pub const fn context_fingerprint(&self) -> ContextFingerprint {
        self.wire.context_fingerprint
    }

    pub fn coverage_manifest_json(&self) -> &str {
        &self.wire.coverage_manifest_json
    }

    pub const fn coverage_manifest_sha256(&self) -> Sha256Digest {
        self.wire.coverage_manifest_sha256
    }

    pub fn xml_payload(&self) -> &str {
        &self.xml_payload
    }

    pub const fn xml_sha256(&self) -> Sha256Digest {
        self.wire.xml_sha256
    }

    pub fn proof_json(&self) -> &str {
        &self.proof_json
    }

    pub const fn proof_sha256(&self) -> Sha256Digest {
        self.proof_sha256
    }

    pub(crate) fn validate_against_trusted(
        &self,
        trusted_evaluation: &TrustedEvaluation,
    ) -> Result<(), CheckedFinalCopyPayloadError> {
        self.validate_integrity()?;
        if trusted_evaluation.context().profile() != BehaviorProfile::FilingSafe {
            return Err(CheckedFinalCopyPayloadError::WrongProfile {
                actual: trusted_evaluation.context().profile(),
            });
        }
        if trusted_evaluation.context().phase() != ValidationPhase::FinalCopy {
            return Err(CheckedFinalCopyPayloadError::WrongPhase {
                actual: trusted_evaluation.context().phase(),
            });
        }
        if self.rule_set() != trusted_evaluation.rule_set() {
            return Err(CheckedFinalCopyPayloadError::BindingMismatch { field: "rule_set" });
        }
        if self.context() != trusted_evaluation.context() {
            return Err(CheckedFinalCopyPayloadError::BindingMismatch { field: "context" });
        }
        if self.input_revision() != trusted_evaluation.input_revision() {
            return Err(CheckedFinalCopyPayloadError::BindingMismatch {
                field: "input_revision",
            });
        }
        if self.context_fingerprint() != trusted_evaluation.context_fingerprint() {
            return Err(CheckedFinalCopyPayloadError::BindingMismatch {
                field: "context_fingerprint",
            });
        }
        self.validate_input_snapshot(trusted_evaluation.raw_inputs())?;
        self.validate_semantic_coverage(trusted_evaluation.result())
    }

    pub(crate) fn validate_against_evaluation(
        &self,
        evaluation: &EvaluationResult,
    ) -> Result<(), CheckedFinalCopyPayloadError> {
        self.validate_integrity()?;
        if self.rule_set() != evaluation.rule_set() {
            return Err(CheckedFinalCopyPayloadError::BindingMismatch { field: "rule_set" });
        }
        if self.context() != evaluation.context() {
            return Err(CheckedFinalCopyPayloadError::BindingMismatch { field: "context" });
        }
        if self.input_revision() != evaluation.input_revision() {
            return Err(CheckedFinalCopyPayloadError::BindingMismatch {
                field: "input_revision",
            });
        }
        if self.context_fingerprint() != evaluation.context_fingerprint() {
            return Err(CheckedFinalCopyPayloadError::BindingMismatch {
                field: "context_fingerprint",
            });
        }
        self.validate_semantic_coverage(evaluation)
    }

    pub(crate) fn validate_input_snapshot(
        &self,
        raw_inputs: &RawInputSnapshot,
    ) -> Result<(), CheckedFinalCopyPayloadError> {
        if digest_serialized(raw_inputs)? != self.wire.request_input_sha256 {
            return Err(CheckedFinalCopyPayloadError::RequestInputMismatch);
        }
        Ok(())
    }

    fn validate_semantic_coverage(
        &self,
        evaluation: &EvaluationResult,
    ) -> Result<(), CheckedFinalCopyPayloadError> {
        let coverage: Vec<FinalCopyFieldCoverage> =
            serde_json::from_str(&self.wire.coverage_manifest_json)
                .map_err(CheckedFinalCopyPayloadError::deserialize)?;
        for field in &coverage {
            let value_source = field.source.description();
            let expected = match &field.source {
                FinalCopyValueSource::CanonicalField {
                    field: source_field,
                } => {
                    let matches = evaluation
                        .canonical_inputs()
                        .iter()
                        .filter(|candidate| candidate.field() == source_field)
                        .collect::<Vec<_>>();
                    if matches.len() != 1 {
                        return Err(CheckedFinalCopyPayloadError::ValueSourceMultiplicity {
                            value_source,
                            actual: matches.len(),
                        });
                    }
                    matches[0].canonical()
                }
                FinalCopyValueSource::DerivedOutput {
                    calculation_id,
                    output_id,
                } => {
                    let matches = evaluation
                        .derived_outputs()
                        .iter()
                        .filter(|candidate| {
                            candidate.calculation_id() == calculation_id
                                && candidate.output_id() == output_id
                        })
                        .collect::<Vec<_>>();
                    if matches.len() != 1 {
                        return Err(CheckedFinalCopyPayloadError::ValueSourceMultiplicity {
                            value_source,
                            actual: matches.len(),
                        });
                    }
                    matches[0].value()
                }
                FinalCopyValueSource::ReviewedConstant { value, .. }
                | FinalCopyValueSource::ReviewedDefault { value, .. } => value,
            };
            if expected != &field.semantic_value {
                return Err(CheckedFinalCopyPayloadError::SemanticValueMismatch { value_source });
            }
        }
        Ok(())
    }

    pub(crate) fn from_stored_parts(
        proof_json: String,
        proof_sha256: Sha256Digest,
        xml_payload: String,
    ) -> Result<Self, CheckedFinalCopyPayloadError> {
        if sha256_digest(proof_json.as_bytes()) != proof_sha256 {
            return Err(CheckedFinalCopyPayloadError::ProofDigestMismatch);
        }
        let wire: FinalCopyProofWire =
            serde_json::from_str(&proof_json).map_err(CheckedFinalCopyPayloadError::deserialize)?;
        let canonical_proof_json =
            serde_json::to_string(&wire).map_err(CheckedFinalCopyPayloadError::serialize)?;
        if canonical_proof_json != proof_json {
            return Err(CheckedFinalCopyPayloadError::InvalidProof(
                "proof JSON is not in its canonical serialization".to_string(),
            ));
        }
        let payload = Self {
            wire,
            proof_json,
            proof_sha256,
            xml_payload,
        };
        payload.validate_integrity()?;
        Ok(payload)
    }

    fn validate_integrity(&self) -> Result<(), CheckedFinalCopyPayloadError> {
        if self.wire.proof_version != FINAL_COPY_PROOF_VERSION {
            return Err(CheckedFinalCopyPayloadError::UnsupportedProofVersion {
                actual: self.wire.proof_version.clone(),
            });
        }
        if self.wire.serializer_version != FINAL_COPY_SERIALIZER_VERSION {
            return Err(CheckedFinalCopyPayloadError::UnsupportedSerializerVersion {
                actual: self.wire.serializer_version.clone(),
            });
        }
        if self.wire.context.profile() != BehaviorProfile::FilingSafe {
            return Err(CheckedFinalCopyPayloadError::WrongProfile {
                actual: self.wire.context.profile(),
            });
        }
        if self.wire.context.phase() != ValidationPhase::FinalCopy {
            return Err(CheckedFinalCopyPayloadError::WrongPhase {
                actual: self.wire.context.phase(),
            });
        }
        if sha256_digest(self.proof_json.as_bytes()) != self.proof_sha256 {
            return Err(CheckedFinalCopyPayloadError::ProofDigestMismatch);
        }
        if sha256_digest(self.wire.coverage_manifest_json.as_bytes())
            != self.wire.coverage_manifest_sha256
        {
            return Err(CheckedFinalCopyPayloadError::ManifestDigestMismatch);
        }
        if sha256_digest(self.xml_payload.as_bytes()) != self.wire.xml_sha256 {
            return Err(CheckedFinalCopyPayloadError::XmlDigestMismatch);
        }
        if u64::try_from(self.xml_payload.len()).ok() != Some(self.wire.xml_byte_len) {
            return Err(CheckedFinalCopyPayloadError::XmlLengthMismatch);
        }

        let coverage: Vec<FinalCopyFieldCoverage> =
            serde_json::from_str(&self.wire.coverage_manifest_json)
                .map_err(CheckedFinalCopyPayloadError::deserialize)?;
        let canonical_manifest =
            serde_json::to_string(&coverage).map_err(CheckedFinalCopyPayloadError::serialize)?;
        if canonical_manifest != self.wire.coverage_manifest_json {
            return Err(CheckedFinalCopyPayloadError::InvalidManifest(
                "coverage manifest is not in its canonical serialization".to_string(),
            ));
        }
        validate_coverage_shape(&coverage)?;
        validate_xml_coverage(&coverage, &self.xml_payload)
    }
}

fn validate_coverage_shape(
    coverage: &[FinalCopyFieldCoverage],
) -> Result<(), CheckedFinalCopyPayloadError> {
    if coverage.is_empty() {
        return Err(CheckedFinalCopyPayloadError::InvalidManifest(
            "coverage manifest is empty".to_string(),
        ));
    }
    let mut occurrences = BTreeSet::new();
    let mut next_occurrence_by_key = BTreeMap::new();
    let mut sources = BTreeSet::new();
    for field in coverage {
        let key = field.serialized_key.as_str();
        if !occurrences.insert((key, field.occurrence)) {
            return Err(CheckedFinalCopyPayloadError::DuplicateManifestKey {
                key: key.to_string(),
            });
        }
        if !sources.insert(field.source.key()) {
            return Err(CheckedFinalCopyPayloadError::DuplicateValueSource {
                value_source: field.source.description(),
            });
        }
        let expected = next_occurrence_by_key.entry(key).or_insert(1_u16);
        if field.occurrence.get() != *expected {
            return Err(CheckedFinalCopyPayloadError::OccurrenceMismatch {
                key: key.to_string(),
                expected: *expected,
                actual: field.occurrence.get(),
            });
        }
        *expected = expected.checked_add(1).ok_or_else(|| {
            CheckedFinalCopyPayloadError::InvalidManifest(format!(
                "serialized key {key:?} has more occurrences than the proof format permits"
            ))
        })?;
    }
    Ok(())
}

fn validate_xml_coverage(
    coverage: &[FinalCopyFieldCoverage],
    xml_payload: &str,
) -> Result<(), CheckedFinalCopyPayloadError> {
    if xml_payload.trim().is_empty() {
        return Err(CheckedFinalCopyPayloadError::EmptyPayload);
    }
    let parsed = parse_bir_xml_encoded_occurrences_checked(xml_payload)?;
    for field in coverage {
        let key = field.serialized_key.as_str();
        let occurrence = usize::from(field.occurrence.get());
        let Some(actual) = parsed
            .iter()
            .find(|candidate| candidate.field_id == key && candidate.occurrence == occurrence)
        else {
            return Err(CheckedFinalCopyPayloadError::MissingXmlKey {
                key: format!("{key}#{occurrence}"),
            });
        };
        if actual.encoded_value != field.encoded_value {
            return Err(CheckedFinalCopyPayloadError::EncodedValueMismatch {
                key: format!("{key}#{occurrence}"),
            });
        }
    }
    for actual in &parsed {
        if !coverage.iter().any(|field| {
            field.serialized_key.as_str() == actual.field_id
                && usize::from(field.occurrence.get()) == actual.occurrence
        }) {
            return Err(CheckedFinalCopyPayloadError::ExtraXmlKey {
                key: format!("{}#{}", actual.field_id, actual.occurrence),
            });
        }
    }
    for (position, (expected, actual)) in coverage.iter().zip(&parsed).enumerate() {
        if expected.serialized_key.as_str() != actual.field_id
            || usize::from(expected.occurrence.get()) != actual.occurrence
        {
            return Err(CheckedFinalCopyPayloadError::XmlOrderMismatch {
                position,
                expected: format!(
                    "{}#{}",
                    expected.serialized_key.as_str(),
                    expected.occurrence.get()
                ),
                actual: format!("{}#{}", actual.field_id, actual.occurrence),
            });
        }
    }
    Ok(())
}

fn digest_serialized<T: Serialize + ?Sized>(
    value: &T,
) -> Result<Sha256Digest, CheckedFinalCopyPayloadError> {
    let encoded = serde_json::to_vec(value).map_err(CheckedFinalCopyPayloadError::serialize)?;
    Ok(sha256_digest(&encoded))
}

fn sha256_digest(value: &[u8]) -> Sha256Digest {
    Sha256Digest::from_bytes(Sha256::digest(value).into())
}

#[derive(Debug, Error)]
pub enum CheckedFinalCopyPayloadError {
    #[error("checked Final Copy XML payload is empty")]
    EmptyPayload,
    #[error("checked Final Copy requires FilingSafe, got {actual:?}")]
    WrongProfile { actual: BehaviorProfile },
    #[error("checked Final Copy requires FinalCopy phase, got {actual:?}")]
    WrongPhase { actual: ValidationPhase },
    #[error("checked Final Copy binding does not match trusted {field}")]
    BindingMismatch { field: &'static str },
    #[error("checked Final Copy request input snapshot does not match")]
    RequestInputMismatch,
    #[error("checked Final Copy construction requires a generated reviewed serialization contract")]
    MissingSerializationContract,
    #[error("checked Final Copy XML is invalid: {0}")]
    InvalidXml(#[from] BirXmlParseError),
    #[error("checked Final Copy manifest is invalid: {0}")]
    InvalidManifest(String),
    #[error("checked Final Copy manifest contains duplicate serialized key/occurrence {key:?}")]
    DuplicateManifestKey { key: String },
    #[error("checked Final Copy manifest contains duplicate value source {value_source}")]
    DuplicateValueSource { value_source: String },
    #[error(
        "checked Final Copy value source {value_source} resolved {actual} times; exactly one is required"
    )]
    ValueSourceMultiplicity { value_source: String, actual: usize },
    #[error("checked Final Copy semantic value differs from {value_source}")]
    SemanticValueMismatch { value_source: String },
    #[error("checked Final Copy manifest occurrence for {key:?} is {actual}, expected {expected}")]
    OccurrenceMismatch {
        key: String,
        expected: u16,
        actual: u16,
    },
    #[error("checked Final Copy XML is missing serialized key {key:?}")]
    MissingXmlKey { key: String },
    #[error("checked Final Copy XML contains extra serialized key {key:?}")]
    ExtraXmlKey { key: String },
    #[error(
        "checked Final Copy XML occurrence order differs at position {position}: expected {expected:?}, got {actual:?}"
    )]
    XmlOrderMismatch {
        position: usize,
        expected: String,
        actual: String,
    },
    #[error("checked Final Copy encoded value differs for serialized key {key:?}")]
    EncodedValueMismatch { key: String },
    #[error("unsupported checked Final Copy proof version {actual:?}")]
    UnsupportedProofVersion { actual: String },
    #[error("unsupported checked Final Copy serializer version {actual:?}")]
    UnsupportedSerializerVersion { actual: String },
    #[error("checked Final Copy proof is invalid: {0}")]
    InvalidProof(String),
    #[error("checked Final Copy proof digest does not match")]
    ProofDigestMismatch,
    #[error("checked Final Copy coverage manifest digest does not match")]
    ManifestDigestMismatch,
    #[error("checked Final Copy XML digest does not match")]
    XmlDigestMismatch,
    #[error("checked Final Copy XML byte length does not match")]
    XmlLengthMismatch,
    #[error("checked Final Copy proof serialization failed: {0}")]
    Serialization(String),
}

impl CheckedFinalCopyPayloadError {
    fn serialize(error: serde_json::Error) -> Self {
        Self::Serialization(error.to_string())
    }

    fn deserialize(error: serde_json::Error) -> Self {
        Self::Serialization(error.to_string())
    }
}
