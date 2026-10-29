//! Checked plaintext artifact construction from a sealed rules contract.
//!
//! This is deliberately one boundary short of Final Copy authorization.  It
//! proves that one exact, filing-safe request was re-evaluated, materialized by
//! its reviewed serialization contract, rendered byte-for-byte, and parsed
//! back against the complete ordered trace.  It does not encrypt, persist,
//! queue, submit, or construct [`super::CheckedFinalCopyPayload`].

use super::{
    TrustedEvaluation,
    plaintext_artifact::{
        ExpectedPlaintextRecord, PlaintextArtifactError, parse_and_validate_plaintext,
        render_expected_plaintext,
    },
};
use bir_rules::{
    BehaviorProfile, CanonicalValue, CompiledRuleSet, ContextFingerprint, EvaluationError,
    FormRevisionKey, InputRevision, MaterializationError, MaterializationTraceEntryView,
    MaterializedBindingView, MaterializedOmissionView, MaterializedRecordView,
    MaterializedValueSourceView, RepeatedGroupInstance, SerializationInspectionError,
    SerializationInspector, SerializationMaterialization, Sha256Digest,
    StaticSerializationContract, ValidationContext, XmlKey,
    serialization::{
        FormattedSemanticValue, SerializationArtifactIdentity, SerializationError,
        format_serialization_value,
    },
    serialization_contract::{
        DynamicGroupNode, MetadataElementNode, PseudoXmlFieldNode, SerializationKeyProjection,
        SerializationNode, SerializationOccurrenceProjection, SerializationPlan,
        SerializationPresence, SerializationValueProjection,
    },
    static_ir::Branch,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fmt};
use thiserror::Error;

const CHECKED_SERIALIZATION_PROOF_VERSION: &str = "checked-serialization-artifact-v3";
const RAW_INPUT_DIGEST_DOMAIN: &[u8] = b"bir-rules/serialization-raw-input/v1\0";
const EVALUATION_DIGEST_DOMAIN: &[u8] = b"bir-rules/serialization-evaluation/v2\0";
const RECORD_MANIFEST_DIGEST_DOMAIN: &[u8] = b"bir-rules/serialization-record-manifest/v2\0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CheckedSerializationProofWire {
    proof_version: String,
    rule_set: FormRevisionKey,
    context: ValidationContext,
    input_revision: InputRevision,
    context_fingerprint: ContextFingerprint,
    artifact: SerializationArtifactIdentity,
    artifact_id: String,
    contract_digest: Sha256Digest,
    raw_input_digest: Sha256Digest,
    evaluation_digest: Sha256Digest,
    record_manifest_digest: Sha256Digest,
    plaintext_byte_len: u64,
    plaintext_sha256: Sha256Digest,
}

/// Opaque proof of one exact plaintext serialization materialization.
///
/// There is intentionally no public constructor or deserializer.  Application
/// code may inspect and pass this value, but only [`super::FormRuleEvaluator`]
/// can construct it after exact registry resolution and re-evaluation.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedSerializationArtifact {
    wire: CheckedSerializationProofWire,
    proof_json: String,
    proof_sha256: Sha256Digest,
    // Retained inside the opaque proof for a future sealed queue conversion.
    // No accessor exists while raw transport can still accept bare bytes.
    #[allow(dead_code)]
    plaintext: Vec<u8>,
}

impl fmt::Debug for CheckedSerializationArtifact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CheckedSerializationArtifact")
            .field("proof_version", &self.wire.proof_version)
            .field("rule_set", &self.wire.rule_set)
            .field("context", &self.wire.context)
            .field("input_revision", &self.wire.input_revision)
            .field("context_fingerprint", &self.wire.context_fingerprint)
            .field("artifact", &self.wire.artifact)
            .field("artifact_id", &self.wire.artifact_id)
            .field("contract_digest", &self.wire.contract_digest)
            .field("record_manifest_digest", &self.wire.record_manifest_digest)
            .field("plaintext_byte_len", &self.wire.plaintext_byte_len)
            .field("plaintext_sha256", &self.wire.plaintext_sha256)
            .field("proof_sha256", &self.proof_sha256)
            .finish_non_exhaustive()
    }
}

impl CheckedSerializationArtifact {
    pub(crate) fn try_new(
        rule_set: &dyn CompiledRuleSet,
        trusted: &TrustedEvaluation,
        selected_artifact: &SerializationArtifactIdentity,
    ) -> Result<Self, CheckedSerializationArtifactError> {
        if trusted.context().profile() != BehaviorProfile::FilingSafe {
            return Err(CheckedSerializationArtifactError::ProfileNotAuthorized {
                profile: trusted.context().profile(),
            });
        }
        if rule_set.identity() != trusted.rule_set() {
            return Err(CheckedSerializationArtifactError::BindingMismatch { field: "rule_set" });
        }

        // Re-evaluation prevents a previously trusted result from being paired
        // with a different provider implementation under the same call site.
        let reevaluated = rule_set
            .evaluate(trusted.request())
            .map_err(CheckedSerializationArtifactError::Evaluation)?;
        if &reevaluated != trusted.result() {
            return Err(CheckedSerializationArtifactError::ReevaluationMismatch);
        }
        let mut inspector = rule_set
            .serialization_inspector(trusted.request(), &reevaluated)
            .map_err(CheckedSerializationArtifactError::Inspection)?;

        // The sealed materializer evaluates again internally and owns exact
        // artifact/profile/phase selection, projections, formatting, and
        // codecs.  Core accepts only its opaque, generated trace.
        let materialization = rule_set
            .materialize_serialization(trusted.request(), selected_artifact)
            .map_err(CheckedSerializationArtifactError::Materialization)?;
        let materialization = CheckedMaterialization::from_provider(&materialization)?;
        Self::try_new_resolved(
            rule_set.serialization_contract(),
            trusted,
            selected_artifact,
            materialization,
            &mut inspector,
        )
    }

    fn try_new_resolved(
        contract: &'static StaticSerializationContract,
        trusted: &TrustedEvaluation,
        selected_artifact: &SerializationArtifactIdentity,
        materialization: CheckedMaterialization,
        inspector: &mut dyn CheckedSerializationInspector,
    ) -> Result<Self, CheckedSerializationArtifactError> {
        let selected_plan = validate_materialization_bindings(
            contract,
            trusted,
            selected_artifact,
            &materialization,
        )?;

        let expected_records =
            expected_plaintext_records(trusted, &materialization, selected_plan, inspector)?;
        let plaintext = render_expected_plaintext(&expected_records)
            .map_err(CheckedSerializationArtifactError::plaintext_render)?;
        let parsed = parse_and_validate_plaintext(&plaintext, &expected_records)
            .map_err(CheckedSerializationArtifactError::plaintext_parse)?;
        if parsed.len() != expected_records.len() {
            return Err(CheckedSerializationArtifactError::TraceInvariant {
                record_index: None,
                reason: "independent parser did not account for every trace entry",
            });
        }

        let plaintext_byte_len = u64::try_from(plaintext.len())
            .map_err(|_| CheckedSerializationArtifactError::PlaintextLengthOverflow)?;
        let wire = CheckedSerializationProofWire {
            proof_version: CHECKED_SERIALIZATION_PROOF_VERSION.to_string(),
            rule_set: trusted.rule_set().clone(),
            context: trusted.context(),
            input_revision: trusted.input_revision(),
            context_fingerprint: trusted.context_fingerprint(),
            artifact: selected_artifact.clone(),
            artifact_id: materialization.artifact_id().to_string(),
            contract_digest: materialization.contract_digest(),
            raw_input_digest: materialization.raw_input_digest(),
            evaluation_digest: materialization.evaluation_digest(),
            record_manifest_digest: materialization.record_manifest_digest(),
            plaintext_byte_len,
            plaintext_sha256: sha256_digest(&plaintext),
        };
        let proof_json = serde_json::to_string(&wire)
            .map_err(CheckedSerializationArtifactError::ProofSerialization)?;
        let proof_sha256 = sha256_digest(proof_json.as_bytes());
        Ok(Self {
            wire,
            proof_json,
            proof_sha256,
            plaintext,
        })
    }

    pub fn proof_version(&self) -> &str {
        &self.wire.proof_version
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

    pub fn artifact(&self) -> &SerializationArtifactIdentity {
        &self.wire.artifact
    }

    pub fn artifact_id(&self) -> &str {
        &self.wire.artifact_id
    }

    pub const fn contract_digest(&self) -> Sha256Digest {
        self.wire.contract_digest
    }

    pub const fn raw_input_digest(&self) -> Sha256Digest {
        self.wire.raw_input_digest
    }

    pub const fn evaluation_digest(&self) -> Sha256Digest {
        self.wire.evaluation_digest
    }

    pub const fn record_manifest_digest(&self) -> Sha256Digest {
        self.wire.record_manifest_digest
    }

    pub const fn plaintext_byte_len(&self) -> u64 {
        self.wire.plaintext_byte_len
    }

    pub const fn plaintext_sha256(&self) -> Sha256Digest {
        self.wire.plaintext_sha256
    }

    pub fn proof_json(&self) -> &str {
        &self.proof_json
    }

    pub const fn proof_sha256(&self) -> Sha256Digest {
        self.proof_sha256
    }
}

/// Private verifier input copied from the sealed provider result.
///
/// The record-manifest digest is recomputed over the provider's original
/// serializable trace before this copy is made. Keeping this mirror private
/// lets unit tests exercise the proof boundary without exposing a provider
/// constructor or a plaintext-bearing artifact constructor.
#[derive(Clone)]
struct CheckedMaterialization {
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
    recomputed_record_manifest_digest: Sha256Digest,
    trace: Vec<CheckedTraceEntry>,
}

impl CheckedMaterialization {
    fn from_provider(
        materialization: &SerializationMaterialization,
    ) -> Result<Self, CheckedSerializationArtifactError> {
        Ok(Self {
            rule_set: materialization.rule_set().clone(),
            context: materialization.context(),
            input_revision: materialization.input_revision(),
            context_fingerprint: materialization.context_fingerprint(),
            artifact_id: materialization.artifact_id().to_string(),
            artifact: materialization.artifact().clone(),
            contract_digest: materialization.contract_digest(),
            raw_input_digest: materialization.raw_input_digest(),
            evaluation_digest: materialization.evaluation_digest(),
            record_manifest_digest: materialization.record_manifest_digest(),
            recomputed_record_manifest_digest: digest_serializable(
                RECORD_MANIFEST_DIGEST_DOMAIN,
                materialization.trace(),
            )?,
            trace: materialization
                .trace()
                .iter()
                .map(CheckedTraceEntry::from_provider)
                .collect(),
        })
    }

    fn rule_set(&self) -> &FormRevisionKey {
        &self.rule_set
    }

    const fn context(&self) -> ValidationContext {
        self.context
    }

    const fn input_revision(&self) -> InputRevision {
        self.input_revision
    }

    const fn context_fingerprint(&self) -> ContextFingerprint {
        self.context_fingerprint
    }

    fn artifact_id(&self) -> &str {
        &self.artifact_id
    }

    fn artifact(&self) -> &SerializationArtifactIdentity {
        &self.artifact
    }

    const fn contract_digest(&self) -> Sha256Digest {
        self.contract_digest
    }

    const fn raw_input_digest(&self) -> Sha256Digest {
        self.raw_input_digest
    }

    const fn evaluation_digest(&self) -> Sha256Digest {
        self.evaluation_digest
    }

    const fn record_manifest_digest(&self) -> Sha256Digest {
        self.record_manifest_digest
    }

    const fn recomputed_record_manifest_digest(&self) -> Sha256Digest {
        self.recomputed_record_manifest_digest
    }

    fn trace(&self) -> &[CheckedTraceEntry] {
        &self.trace
    }
}

#[derive(Clone, Serialize)]
#[serde(tag = "entry", content = "value", rename_all = "kebab-case")]
enum CheckedTraceEntry {
    Record(CheckedMaterializedRecord),
    GroupAccounting(CheckedGroupAccounting),
}

impl CheckedTraceEntry {
    fn from_provider(entry: &MaterializationTraceEntryView) -> Self {
        match entry {
            MaterializationTraceEntryView::Record(record) => {
                Self::Record(CheckedMaterializedRecord::from_provider(record))
            }
            MaterializationTraceEntryView::GroupAccounting(accounting) => {
                Self::GroupAccounting(CheckedGroupAccounting {
                    emission_id: CheckedEmissionId::from_provider(accounting.emission_id()),
                    group_id: accounting.group_id().to_string(),
                    instances: accounting.instances().to_vec(),
                })
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct CheckedEmissionId {
    ordinal: u32,
    group_path: Vec<RepeatedGroupInstance>,
}

impl CheckedEmissionId {
    fn from_provider(emission_id: &bir_rules::ContractEmissionId) -> Self {
        Self {
            ordinal: emission_id.ordinal(),
            group_path: emission_id.group_path().to_vec(),
        }
    }

    const fn ordinal(&self) -> u32 {
        self.ordinal
    }

    fn group_path(&self) -> &[RepeatedGroupInstance] {
        &self.group_path
    }
}

#[derive(Clone, Serialize)]
struct CheckedMaterializedRecord {
    emission_id: CheckedEmissionId,
    binding: MaterializedBindingView,
    value_source: MaterializedValueSourceView,
    omission: MaterializedOmissionView,
    semantic_value: Option<CanonicalValue>,
    semantic_body: Option<String>,
    encoded_body: Option<String>,
}

impl CheckedMaterializedRecord {
    fn from_provider(record: &MaterializedRecordView) -> Self {
        Self {
            emission_id: CheckedEmissionId::from_provider(record.emission_id()),
            binding: record.binding().clone(),
            value_source: record.value_source().clone(),
            omission: record.omission(),
            semantic_value: record.semantic_value().cloned(),
            semantic_body: record.semantic_body().map(str::to_string),
            encoded_body: record.encoded_body().map(str::to_string),
        }
    }

    fn emission_id(&self) -> &CheckedEmissionId {
        &self.emission_id
    }

    fn binding(&self) -> &MaterializedBindingView {
        &self.binding
    }

    fn value_source(&self) -> &MaterializedValueSourceView {
        &self.value_source
    }

    const fn omission(&self) -> MaterializedOmissionView {
        self.omission
    }

    fn semantic_value(&self) -> Option<&CanonicalValue> {
        self.semantic_value.as_ref()
    }

    fn semantic_body(&self) -> Option<&str> {
        self.semantic_body.as_deref()
    }

    fn encoded_body(&self) -> Option<&str> {
        self.encoded_body.as_deref()
    }
}

#[derive(Clone, Serialize)]
struct CheckedGroupAccounting {
    emission_id: CheckedEmissionId,
    group_id: String,
    instances: Vec<RepeatedGroupInstance>,
}

impl CheckedGroupAccounting {
    fn emission_id(&self) -> &CheckedEmissionId {
        &self.emission_id
    }

    fn group_id(&self) -> &str {
        &self.group_id
    }

    fn instances(&self) -> &[RepeatedGroupInstance] {
        &self.instances
    }
}

trait CheckedSerializationInspector {
    fn evaluate_presence(
        &mut self,
        presence: SerializationPresence,
        current_group: Option<&RepeatedGroupInstance>,
    ) -> Result<bool, SerializationInspectionError>;

    fn resolve_value_source(
        &mut self,
        projection: SerializationValueProjection,
        current_group: Option<&RepeatedGroupInstance>,
    ) -> Result<MaterializedValueSourceView, SerializationInspectionError>;
}

impl CheckedSerializationInspector for SerializationInspector {
    fn evaluate_presence(
        &mut self,
        presence: SerializationPresence,
        current_group: Option<&RepeatedGroupInstance>,
    ) -> Result<bool, SerializationInspectionError> {
        SerializationInspector::evaluate_presence(self, presence, current_group)
    }

    fn resolve_value_source(
        &mut self,
        projection: SerializationValueProjection,
        current_group: Option<&RepeatedGroupInstance>,
    ) -> Result<MaterializedValueSourceView, SerializationInspectionError> {
        SerializationInspector::resolve_value_source(self, projection, current_group)
    }
}

fn validate_materialization_bindings(
    contract: &'static StaticSerializationContract,
    trusted: &TrustedEvaluation,
    selected_artifact: &SerializationArtifactIdentity,
    materialization: &CheckedMaterialization,
) -> Result<SerializationPlan, CheckedSerializationArtifactError> {
    if materialization.rule_set() != trusted.rule_set() {
        return Err(CheckedSerializationArtifactError::BindingMismatch { field: "rule_set" });
    }
    if materialization.context() != trusted.context() {
        return Err(CheckedSerializationArtifactError::BindingMismatch { field: "context" });
    }
    if materialization.input_revision() != trusted.input_revision() {
        return Err(CheckedSerializationArtifactError::BindingMismatch {
            field: "input_revision",
        });
    }
    if materialization.context_fingerprint() != trusted.context_fingerprint() {
        return Err(CheckedSerializationArtifactError::BindingMismatch {
            field: "context_fingerprint",
        });
    }
    if materialization.artifact() != selected_artifact {
        return Err(CheckedSerializationArtifactError::BindingMismatch { field: "artifact" });
    }

    let matching_artifacts = contract
        .artifacts
        .iter()
        .filter(|artifact| {
            artifact.target == selected_artifact.target()
                && artifact.variant_id == selected_artifact.variant().as_str()
        })
        .collect::<Vec<_>>();
    if matching_artifacts.len() != 1 {
        return Err(
            CheckedSerializationArtifactError::ContractArtifactSelection {
                matches: matching_artifacts.len(),
            },
        );
    }
    if matching_artifacts[0].artifact_id != materialization.artifact_id() {
        return Err(CheckedSerializationArtifactError::BindingMismatch {
            field: "artifact_id",
        });
    }
    let selected_plan = match trusted.context().profile() {
        BehaviorProfile::OfficialCompatibility => matching_artifacts[0].branches.official,
        BehaviorProfile::FilingSafe => matching_artifacts[0].branches.filing_safe,
    };
    let selected_plan = match selected_plan {
        Branch::Executable(plan) => plan,
        Branch::DocumentedOnly | Branch::Unresolved => {
            return Err(CheckedSerializationArtifactError::ContractBranchUnavailable);
        }
    };

    let expected_contract_digest = contract
        .canonical_sha256
        .ok_or(CheckedSerializationArtifactError::MissingContractDigest)
        .and_then(|digest| {
            Sha256Digest::parse(digest)
                .map_err(|_| CheckedSerializationArtifactError::InvalidContractDigest)
        })?;
    compare_digest(
        "contract_digest",
        expected_contract_digest,
        materialization.contract_digest(),
    )?;
    compare_digest(
        "raw_input_digest",
        digest_serializable(RAW_INPUT_DIGEST_DOMAIN, trusted.raw_inputs())?,
        materialization.raw_input_digest(),
    )?;
    compare_digest(
        "evaluation_digest",
        digest_serializable(EVALUATION_DIGEST_DOMAIN, trusted.result())?,
        materialization.evaluation_digest(),
    )?;
    compare_digest(
        "record_manifest_digest",
        materialization.recomputed_record_manifest_digest(),
        materialization.record_manifest_digest(),
    )?;
    Ok(selected_plan)
}

fn expected_plaintext_records(
    trusted: &TrustedEvaluation,
    materialization: &CheckedMaterialization,
    plan: SerializationPlan,
    inspector: &mut dyn CheckedSerializationInspector,
) -> Result<Vec<ExpectedPlaintextRecord>, CheckedSerializationArtifactError> {
    let mut verifier = ContractTraceVerifier {
        trusted,
        inspector,
        trace: materialization.trace(),
        cursor: 0,
        emission_ids: BTreeSet::new(),
        expected: Vec::with_capacity(materialization.trace().len()),
    };
    verifier.verify_nodes(plan.nodes, None)?;
    if verifier.cursor != verifier.trace.len() {
        return Err(trace_invariant(
            verifier.cursor,
            "materialization trace contains records absent from the selected contract",
        ));
    }
    Ok(verifier.expected)
}

struct ContractGroupOccurrence {
    group_id: &'static str,
    instance: RepeatedGroupInstance,
    index: u32,
}

#[derive(Clone, Copy)]
enum ContractBodyBoundary<'a> {
    PseudoXml(&'a str),
    Metadata(&'a str),
}

struct ContractTraceVerifier<'a> {
    trusted: &'a TrustedEvaluation,
    inspector: &'a mut dyn CheckedSerializationInspector,
    trace: &'a [CheckedTraceEntry],
    cursor: usize,
    emission_ids: BTreeSet<CheckedEmissionId>,
    expected: Vec<ExpectedPlaintextRecord>,
}

impl ContractTraceVerifier<'_> {
    fn verify_nodes(
        &mut self,
        nodes: &'static [SerializationNode],
        group: Option<&ContractGroupOccurrence>,
    ) -> Result<(), CheckedSerializationArtifactError> {
        for node in nodes {
            match *node {
                SerializationNode::PseudoXmlField(field) => {
                    self.verify_pseudo_xml(field, group)?;
                }
                SerializationNode::MetadataElement(element) => {
                    self.verify_metadata(element, group)?;
                }
                SerializationNode::ReviewedLiteral(literal) => {
                    let (record_index, record) = self.take_record()?;
                    self.verify_emission(record_index, &record, literal.ordinal, group)?;
                    match record.binding() {
                        MaterializedBindingView::ReviewedLiteral { exact_bytes }
                            if exact_bytes.as_slice() == literal.exact_bytes
                                && matches!(
                                    record.value_source(),
                                    MaterializedValueSourceView::None
                                )
                                && record.omission() == MaterializedOmissionView::Emitted
                                && record.semantic_value().is_none()
                                && record.semantic_body().is_none()
                                && record.encoded_body().is_none() =>
                        {
                            self.expected.push(ExpectedPlaintextRecord::ReviewedLiteral(
                                literal.exact_bytes.to_vec(),
                            ));
                        }
                        _ => {
                            return Err(trace_invariant(
                                record_index,
                                "reviewed literal differs from the selected contract",
                            ));
                        }
                    }
                }
                SerializationNode::DynamicGroup(dynamic) => {
                    if group.is_some() {
                        return Err(trace_invariant(
                            self.cursor,
                            "selected contract contains an unsupported nested dynamic group",
                        ));
                    }
                    self.verify_dynamic_group(dynamic)?;
                }
            }
        }
        Ok(())
    }

    fn verify_dynamic_group(
        &mut self,
        dynamic: DynamicGroupNode,
    ) -> Result<(), CheckedSerializationArtifactError> {
        let (record_index, entry) = self.take_trace_entry()?;
        let CheckedTraceEntry::GroupAccounting(accounting) = entry else {
            return Err(trace_invariant(
                record_index,
                "dynamic group is not preceded by its accounting trace",
            ));
        };
        if accounting.emission_id().ordinal() != dynamic.ordinal
            || !accounting.emission_id().group_path().is_empty()
            || accounting.group_id() != dynamic.group_id
        {
            return Err(trace_invariant(
                record_index,
                "dynamic-group accounting identity differs from the selected contract",
            ));
        }

        let instances = self
            .trusted
            .raw_inputs()
            .repeated_group_instances()
            .iter()
            .filter(|instance| instance.group_id().as_str() == dynamic.group_id)
            .cloned()
            .collect::<Vec<_>>();
        if instances.len() < dynamic.min_occurs
            || dynamic
                .max_occurs
                .is_some_and(|maximum| instances.len() > maximum)
            || accounting.instances() != instances
        {
            return Err(trace_invariant(
                record_index,
                "dynamic-group accounting differs from the trusted raw snapshot",
            ));
        }
        self.expected.push(ExpectedPlaintextRecord::Accounting);

        for (index, instance) in instances.into_iter().enumerate() {
            let index = u32::try_from(index)
                .map_err(|_| trace_invariant(record_index, "dynamic-group index exceeds u32"))?;
            let occurrence = ContractGroupOccurrence {
                group_id: dynamic.group_id,
                instance,
                index,
            };
            self.verify_nodes(dynamic.nodes, Some(&occurrence))?;
        }
        Ok(())
    }

    fn verify_pseudo_xml(
        &mut self,
        field: PseudoXmlFieldNode,
        group: Option<&ContractGroupOccurrence>,
    ) -> Result<(), CheckedSerializationArtifactError> {
        let (record_index, record) = self.take_record()?;
        self.verify_emission(record_index, &record, field.ordinal, group)?;
        let key = project_contract_key(record_index, field.key_projection, group)?;
        let occurrence =
            project_contract_occurrence(record_index, field.occurrence_projection, group)?;
        if !matches!(
            record.binding(),
            MaterializedBindingView::PseudoXmlField {
                key: actual_key,
                occurrence: actual_occurrence,
            } if actual_key == &key && *actual_occurrence == occurrence
        ) {
            return Err(trace_invariant(
                record_index,
                "pseudo-XML binding differs from the selected contract projection",
            ));
        }

        let encoded = self.verify_value_record(
            record_index,
            &record,
            field.value_projection,
            field.semantic_format,
            field.body_codec,
            field.presence,
            group,
            ContractBodyBoundary::PseudoXml(&key),
        )?;
        match encoded {
            Some(encoded_body) => {
                let occurrence = usize::try_from(occurrence).map_err(|_| {
                    CheckedSerializationArtifactError::OccurrenceOverflow { record_index }
                })?;
                self.expected.push(ExpectedPlaintextRecord::PseudoXmlField {
                    key,
                    occurrence,
                    encoded_body,
                });
            }
            None => self.expected.push(ExpectedPlaintextRecord::Omitted),
        }
        Ok(())
    }

    fn verify_metadata(
        &mut self,
        element: MetadataElementNode,
        group: Option<&ContractGroupOccurrence>,
    ) -> Result<(), CheckedSerializationArtifactError> {
        let (record_index, record) = self.take_record()?;
        self.verify_emission(record_index, &record, element.ordinal, group)?;
        if !matches!(
            record.binding(),
            MaterializedBindingView::MetadataElement { exact_tag }
                if exact_tag == element.exact_tag
        ) {
            return Err(trace_invariant(
                record_index,
                "metadata binding differs from the selected contract",
            ));
        }
        let encoded = self.verify_value_record(
            record_index,
            &record,
            element.value_projection,
            element.semantic_format,
            element.body_codec,
            element.presence,
            group,
            ContractBodyBoundary::Metadata(element.exact_tag),
        )?;
        match encoded {
            Some(encoded_body) => {
                self.expected
                    .push(ExpectedPlaintextRecord::MetadataElement {
                        exact_tag: element.exact_tag.to_string(),
                        encoded_body,
                    });
            }
            None => self.expected.push(ExpectedPlaintextRecord::Omitted),
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn verify_value_record(
        &mut self,
        record_index: usize,
        record: &CheckedMaterializedRecord,
        projection: SerializationValueProjection,
        format: bir_rules::serialization_contract::SerializationSemanticFormat,
        codec: bir_rules::serialization::BodyCodec,
        presence: SerializationPresence,
        group: Option<&ContractGroupOccurrence>,
        boundary: ContractBodyBoundary<'_>,
    ) -> Result<Option<Vec<u8>>, CheckedSerializationArtifactError> {
        let current_group = group.map(|group| &group.instance);
        let expected_source = self
            .inspector
            .resolve_value_source(projection, current_group)
            .map_err(CheckedSerializationArtifactError::Inspection)?;
        if record.value_source() != &expected_source {
            return Err(trace_invariant(
                record_index,
                "value source differs from independent compiled-rule-set resolution",
            ));
        }

        let independently_present = self
            .inspector
            .evaluate_presence(presence, current_group)
            .map_err(CheckedSerializationArtifactError::Inspection)?;
        match (presence, independently_present) {
            (SerializationPresence::Omitted, false) => {
                if record.omission() != MaterializedOmissionView::ContractOmitted
                    || !record_has_no_value_state(record)
                {
                    return Err(trace_invariant(
                        record_index,
                        "contract-omitted node has inconsistent trace state",
                    ));
                }
                return Ok(None);
            }
            (SerializationPresence::When(_), false) => {
                if record.omission() != MaterializedOmissionView::PresenceFalse
                    || !record_has_no_value_state(record)
                {
                    return Err(trace_invariant(
                        record_index,
                        "presence-false trace disagrees with independent predicate evaluation",
                    ));
                }
                return Ok(None);
            }
            (SerializationPresence::Always | SerializationPresence::When(_), true) => {
                if matches!(
                    record.omission(),
                    MaterializedOmissionView::PresenceFalse
                        | MaterializedOmissionView::ContractOmitted
                ) {
                    return Err(trace_invariant(
                        record_index,
                        "present trace disagrees with independent predicate evaluation",
                    ));
                }
            }
            _ => {
                return Err(trace_invariant(
                    record_index,
                    "contract presence mode has an impossible independent result",
                ));
            }
        }

        let value = resolve_trusted_contract_value(record_index, self.trusted, record)?;
        if record.semantic_value() != Some(&value) {
            return Err(CheckedSerializationArtifactError::SemanticValueMismatch { record_index });
        }
        match format_serialization_value(&value, format).map_err(|source| {
            CheckedSerializationArtifactError::ContractReformatting {
                record_index,
                source,
            }
        })? {
            FormattedSemanticValue::Omitted => {
                let expected_omission = match value {
                    CanonicalValue::Absent => MaterializedOmissionView::SemanticAbsent,
                    CanonicalValue::Blank => MaterializedOmissionView::SemanticBlank,
                    _ => {
                        return Err(trace_invariant(
                            record_index,
                            "present semantic value formatted as an omitted occurrence",
                        ));
                    }
                };
                if record.omission() != expected_omission
                    || record.semantic_body().is_some()
                    || record.encoded_body().is_some()
                {
                    return Err(trace_invariant(
                        record_index,
                        "semantic omission differs from independent contract formatting",
                    ));
                }
                Ok(None)
            }
            FormattedSemanticValue::Body(body) => {
                let independently_encoded = match boundary {
                    ContractBodyBoundary::PseudoXml(key) => {
                        let key = XmlKey::parse(key.to_string()).map_err(|_| {
                            trace_invariant(record_index, "contract projected an invalid XML key")
                        })?;
                        codec.encode(&body, &key)
                    }
                    ContractBodyBoundary::Metadata(tag) => codec.encode_metadata(&body, tag),
                }
                .map_err(|source| {
                    CheckedSerializationArtifactError::ContractReformatting {
                        record_index,
                        source,
                    }
                })?;
                if record.omission() != MaterializedOmissionView::Emitted
                    || record.semantic_body() != Some(body.as_str())
                    || record.encoded_body() != Some(independently_encoded.as_str())
                {
                    return Err(trace_invariant(
                        record_index,
                        "semantic or encoded body differs from independent contract formatting",
                    ));
                }
                Ok(Some(independently_encoded.into_bytes()))
            }
        }
    }

    fn verify_emission(
        &self,
        record_index: usize,
        record: &CheckedMaterializedRecord,
        ordinal: u32,
        group: Option<&ContractGroupOccurrence>,
    ) -> Result<(), CheckedSerializationArtifactError> {
        let expected_path = group
            .map(|group| std::slice::from_ref(&group.instance))
            .unwrap_or(&[]);
        if record.emission_id().ordinal() != ordinal
            || record.emission_id().group_path() != expected_path
        {
            return Err(trace_invariant(
                record_index,
                "emission identity differs from the selected contract traversal",
            ));
        }
        Ok(())
    }

    fn take_record(
        &mut self,
    ) -> Result<(usize, CheckedMaterializedRecord), CheckedSerializationArtifactError> {
        let (record_index, entry) = self.take_trace_entry()?;
        match entry {
            CheckedTraceEntry::Record(record) => Ok((record_index, record)),
            CheckedTraceEntry::GroupAccounting(_) => Err(trace_invariant(
                record_index,
                "selected value node encountered unexpected group accounting",
            )),
        }
    }

    fn take_trace_entry(
        &mut self,
    ) -> Result<(usize, CheckedTraceEntry), CheckedSerializationArtifactError> {
        let record_index = self.cursor;
        let entry = self.trace.get(record_index).cloned().ok_or_else(|| {
            trace_invariant(
                record_index,
                "materialization trace ended before the selected contract",
            )
        })?;
        self.cursor += 1;
        let emission_id = match &entry {
            CheckedTraceEntry::Record(record) => record.emission_id(),
            CheckedTraceEntry::GroupAccounting(group) => group.emission_id(),
        };
        if !self.emission_ids.insert(emission_id.clone()) {
            return Err(trace_invariant(
                record_index,
                "duplicate contract emission identity",
            ));
        }
        Ok((record_index, entry))
    }
}

fn resolve_trusted_contract_value(
    record_index: usize,
    trusted: &TrustedEvaluation,
    record: &CheckedMaterializedRecord,
) -> Result<CanonicalValue, CheckedSerializationArtifactError> {
    match record.value_source() {
        MaterializedValueSourceView::None => Err(trace_invariant(
            record_index,
            "value node has no semantic source",
        )),
        MaterializedValueSourceView::Field { field } => {
            let matches = trusted
                .result()
                .canonical_inputs()
                .iter()
                .filter(|candidate| candidate.field() == field)
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                return Err(CheckedSerializationArtifactError::ValueSourceMultiplicity {
                    record_index,
                    actual: matches.len(),
                });
            }
            Ok(matches[0].canonical().clone())
        }
        MaterializedValueSourceView::Derived {
            calculation_id,
            output_id,
            instance,
        } => {
            let matches = trusted
                .result()
                .derived_outputs()
                .iter()
                .filter(|candidate| {
                    candidate.calculation_id() == calculation_id
                        && candidate.output_id() == output_id
                        && candidate.instance() == instance.as_ref()
                })
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                return Err(CheckedSerializationArtifactError::ValueSourceMultiplicity {
                    record_index,
                    actual: matches.len(),
                });
            }
            Ok(matches[0].value().clone())
        }
        MaterializedValueSourceView::Context { context_value_id } => trusted
            .context_values()
            .get(context_value_id)
            .cloned()
            .ok_or(CheckedSerializationArtifactError::ValueSourceMultiplicity {
                record_index,
                actual: 0,
            }),
        MaterializedValueSourceView::Constant { value }
        | MaterializedValueSourceView::Default { value } => Ok(value.clone()),
    }
}

fn project_contract_key(
    record_index: usize,
    projection: SerializationKeyProjection,
    group: Option<&ContractGroupOccurrence>,
) -> Result<String, CheckedSerializationArtifactError> {
    match projection {
        SerializationKeyProjection::Exact(key) => Ok(key.to_string()),
        SerializationKeyProjection::GroupIndexed(indexed) => {
            let group = group
                .filter(|group| group.group_id == indexed.group_id)
                .ok_or_else(|| {
                    trace_invariant(
                        record_index,
                        "key projection references the wrong dynamic group",
                    )
                })?;
            let value = indexed
                .index_step
                .checked_mul(group.index)
                .and_then(|offset| indexed.index_base.checked_add(offset))
                .ok_or_else(|| trace_invariant(record_index, "key projection overflowed"))?;
            Ok(format!(
                "{}{:0width$}{}",
                indexed.prefix,
                value,
                indexed.suffix,
                width = indexed.padding as usize
            ))
        }
    }
}

fn project_contract_occurrence(
    record_index: usize,
    projection: SerializationOccurrenceProjection,
    group: Option<&ContractGroupOccurrence>,
) -> Result<u32, CheckedSerializationArtifactError> {
    match projection {
        SerializationOccurrenceProjection::Fixed(value) if value > 0 => Ok(value),
        SerializationOccurrenceProjection::Fixed(_) => Err(trace_invariant(
            record_index,
            "contract occurrence must be positive",
        )),
        SerializationOccurrenceProjection::GroupIndexed(indexed) => {
            let group = group
                .filter(|group| group.group_id == indexed.group_id)
                .ok_or_else(|| {
                    trace_invariant(
                        record_index,
                        "occurrence projection references the wrong dynamic group",
                    )
                })?;
            indexed
                .index_step
                .checked_mul(group.index)
                .and_then(|offset| indexed.index_base.checked_add(offset))
                .filter(|value| *value > 0)
                .ok_or_else(|| trace_invariant(record_index, "occurrence projection overflowed"))
        }
    }
}

fn record_has_no_value_state(record: &CheckedMaterializedRecord) -> bool {
    record.semantic_value().is_none()
        && record.semantic_body().is_none()
        && record.encoded_body().is_none()
}

fn trace_invariant(record_index: usize, reason: &'static str) -> CheckedSerializationArtifactError {
    CheckedSerializationArtifactError::TraceInvariant {
        record_index: Some(record_index),
        reason,
    }
}

fn compare_digest(
    field: &'static str,
    expected: Sha256Digest,
    actual: Sha256Digest,
) -> Result<(), CheckedSerializationArtifactError> {
    if expected == actual {
        Ok(())
    } else {
        Err(CheckedSerializationArtifactError::DigestMismatch { field })
    }
}

fn digest_serializable<T: Serialize + ?Sized>(
    domain: &'static [u8],
    value: &T,
) -> Result<Sha256Digest, CheckedSerializationArtifactError> {
    let encoded = serde_json::to_vec(value)
        .map_err(CheckedSerializationArtifactError::DigestSerialization)?;
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(encoded);
    Ok(Sha256Digest::from_bytes(hasher.finalize().into()))
}

fn sha256_digest(bytes: &[u8]) -> Sha256Digest {
    Sha256Digest::from_bytes(Sha256::digest(bytes).into())
}

/// Fail-closed construction failures for [`CheckedSerializationArtifact`].
#[derive(Debug, Error)]
pub enum CheckedSerializationArtifactError {
    #[error("behavior profile {profile:?} cannot authorize a checked serialization artifact")]
    ProfileNotAuthorized { profile: BehaviorProfile },
    #[error("exact rule-set registry resolution failed: {0}")]
    Registry(#[source] bir_rules::RegistryError),
    #[error("trusted request re-evaluation failed: {0}")]
    Evaluation(#[source] EvaluationError),
    #[error("trusted result differs from exact rule-set re-evaluation")]
    ReevaluationMismatch,
    #[error("independent serialization inspection failed: {0}")]
    Inspection(#[source] SerializationInspectionError),
    #[error("serialization materialization failed: {0}")]
    Materialization(#[source] MaterializationError),
    #[error("serialization materialization does not match trusted {field}")]
    BindingMismatch { field: &'static str },
    #[error("compiled rule set has no generated serialization contract digest")]
    MissingContractDigest,
    #[error("compiled rule set has an invalid serialization contract digest")]
    InvalidContractDigest,
    #[error("exact serialization artifact selection matched {matches} contract entries")]
    ContractArtifactSelection { matches: usize },
    #[error("selected serialization contract profile branch is not executable")]
    ContractBranchUnavailable,
    #[error("serialization materialization {field} is not independently reproducible")]
    DigestMismatch { field: &'static str },
    #[error("serialization trace invariant failed at record {record_index:?}: {reason}")]
    TraceInvariant {
        record_index: Option<usize>,
        reason: &'static str,
    },
    #[error("serialization value source at record {record_index} resolved {actual} trusted values")]
    ValueSourceMultiplicity { record_index: usize, actual: usize },
    #[error(
        "serialization semantic value at record {record_index} differs from trusted evaluation"
    )]
    SemanticValueMismatch { record_index: usize },
    #[error("serialization contract reformatting failed at record {record_index}: {source}")]
    ContractReformatting {
        record_index: usize,
        #[source]
        source: SerializationError,
    },
    #[error("serialization occurrence at record {record_index} does not fit this platform")]
    OccurrenceOverflow { record_index: usize },
    #[error("rendered plaintext length cannot be represented by the proof format")]
    PlaintextLengthOverflow,
    #[error("plaintext rendering failed: {message}")]
    PlaintextRender { message: String },
    #[error("independent plaintext parsing failed: {message}")]
    PlaintextParse { message: String },
    #[error("serialization digest input could not be encoded: {0}")]
    DigestSerialization(#[source] serde_json::Error),
    #[error("serialization proof could not be encoded: {0}")]
    ProofSerialization(#[source] serde_json::Error),
}

impl CheckedSerializationArtifactError {
    pub(crate) fn registry(error: bir_rules::RegistryError) -> Self {
        Self::Registry(error)
    }

    fn plaintext_render(error: PlaintextArtifactError) -> Self {
        Self::PlaintextRender {
            message: error.to_string(),
        }
    }

    fn plaintext_parse(error: PlaintextArtifactError) -> Self {
        Self::PlaintextParse {
            message: error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bir_rules::{
        CanonicalFieldValue, EvaluationExpectation, EvaluationOutput, EvaluationRequest,
        EvaluationResult, FieldId, FieldInstance, FormCode, FormRevision, OfficialPackageVersion,
        RawFieldValue, RawValue, RepeatedGroupId, RuleSetId, StableInstanceId, ValidationPhase,
        serialization::{
            AbsentValuePolicy, ArtifactVariantId, BlankValuePolicy, BodyCodec,
            SerializationArtifactTarget,
        },
        serialization_contract::{
            DynamicGroupNode, IndexedOccurrenceProjection, MetadataElementNode, PseudoXmlFieldNode,
            ReviewedLiteralNode, SerializationArtifactSpec, SerializationGroupInstanceOrder,
            SerializationPresentFormat, SerializationSemanticFormat,
        },
        static_ir::{FieldInstanceSelector, FieldRef, Predicate, Profiled},
    };

    const CONTRACT_DIGEST: &str =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const TEXT_FORMAT: SerializationSemanticFormat = SerializationSemanticFormat {
        absent: AbsentValuePolicy::Reject,
        blank: BlankValuePolicy::EmitEmptyBody,
        present: SerializationPresentFormat::Text,
    };

    const SUCCESS_NODES: &[SerializationNode] = &[
        SerializationNode::ReviewedLiteral(ReviewedLiteralNode {
            ordinal: 1,
            exact_bytes: b"HEAD\n",
        }),
        SerializationNode::PseudoXmlField(PseudoXmlFieldNode {
            ordinal: 2,
            key_projection: SerializationKeyProjection::Exact("Repeat"),
            occurrence_projection: SerializationOccurrenceProjection::Fixed(1),
            value_projection: SerializationValueProjection::Field(FieldRef {
                field_id: "first",
                instance: FieldInstanceSelector::Singleton,
            }),
            semantic_format: TEXT_FORMAT,
            body_codec: BodyCodec::Utf8PercentRfc3986Unreserved,
            presence: SerializationPresence::Always,
        }),
        SerializationNode::PseudoXmlField(PseudoXmlFieldNode {
            ordinal: 3,
            key_projection: SerializationKeyProjection::Exact("Repeat"),
            occurrence_projection: SerializationOccurrenceProjection::Fixed(2),
            value_projection: SerializationValueProjection::Field(FieldRef {
                field_id: "second",
                instance: FieldInstanceSelector::Singleton,
            }),
            semantic_format: TEXT_FORMAT,
            body_codec: BodyCodec::Utf8PercentRfc3986Unreserved,
            presence: SerializationPresence::Always,
        }),
        SerializationNode::MetadataElement(MetadataElementNode {
            ordinal: 4,
            exact_tag: "stamp",
            value_projection: SerializationValueProjection::Field(FieldRef {
                field_id: "stamp",
                instance: FieldInstanceSelector::Singleton,
            }),
            semantic_format: TEXT_FORMAT,
            body_codec: BodyCodec::RawLiteral,
            presence: SerializationPresence::Always,
        }),
        SerializationNode::PseudoXmlField(PseudoXmlFieldNode {
            ordinal: 5,
            key_projection: SerializationKeyProjection::Exact("Hidden"),
            occurrence_projection: SerializationOccurrenceProjection::Fixed(1),
            value_projection: SerializationValueProjection::Field(FieldRef {
                field_id: "optional",
                instance: FieldInstanceSelector::Singleton,
            }),
            semantic_format: TEXT_FORMAT,
            body_codec: BodyCodec::RawLiteral,
            presence: SerializationPresence::When(Predicate::Constant(false)),
        }),
        SerializationNode::DynamicGroup(DynamicGroupNode {
            ordinal: 6,
            group_id: "rows",
            instance_order: SerializationGroupInstanceOrder::StableInstanceIdAscending,
            min_occurs: 0,
            max_occurs: Some(2),
            nodes: &[
                SerializationNode::PseudoXmlField(PseudoXmlFieldNode {
                    ordinal: 7,
                    key_projection: SerializationKeyProjection::Exact("Line"),
                    occurrence_projection: SerializationOccurrenceProjection::GroupIndexed(
                        IndexedOccurrenceProjection {
                            group_id: "rows",
                            index_base: 1,
                            index_step: 1,
                        },
                    ),
                    value_projection: SerializationValueProjection::Field(FieldRef {
                        field_id: "line",
                        instance: FieldInstanceSelector::CurrentGroupInstance,
                    }),
                    semantic_format: TEXT_FORMAT,
                    body_codec: BodyCodec::Utf8PercentRfc3986Unreserved,
                    presence: SerializationPresence::Always,
                }),
                SerializationNode::ReviewedLiteral(ReviewedLiteralNode {
                    ordinal: 8,
                    exact_bytes: b";",
                }),
            ],
        }),
        SerializationNode::ReviewedLiteral(ReviewedLiteralNode {
            ordinal: 9,
            exact_bytes: b"\nTAIL",
        }),
    ];

    const OMITTED_REPEAT_NODES: &[SerializationNode] = &[
        SerializationNode::PseudoXmlField(PseudoXmlFieldNode {
            ordinal: 1,
            key_projection: SerializationKeyProjection::Exact("Repeat"),
            occurrence_projection: SerializationOccurrenceProjection::Fixed(1),
            value_projection: SerializationValueProjection::Field(FieldRef {
                field_id: "first",
                instance: FieldInstanceSelector::Singleton,
            }),
            semantic_format: TEXT_FORMAT,
            body_codec: BodyCodec::RawLiteral,
            presence: SerializationPresence::When(Predicate::Constant(false)),
        }),
        SerializationNode::PseudoXmlField(PseudoXmlFieldNode {
            ordinal: 2,
            key_projection: SerializationKeyProjection::Exact("Repeat"),
            occurrence_projection: SerializationOccurrenceProjection::Fixed(2),
            value_projection: SerializationValueProjection::Field(FieldRef {
                field_id: "second",
                instance: FieldInstanceSelector::Singleton,
            }),
            semantic_format: TEXT_FORMAT,
            body_codec: BodyCodec::RawLiteral,
            presence: SerializationPresence::Always,
        }),
    ];

    const ARTIFACTS: &[SerializationArtifactSpec] = &[
        SerializationArtifactSpec {
            artifact_id: "synthetic-save",
            target: SerializationArtifactTarget::EditableSave,
            variant_id: "success",
            branches: Profiled {
                official: Branch::Unresolved,
                filing_safe: Branch::Executable(SerializationPlan {
                    nodes: SUCCESS_NODES,
                }),
            },
        },
        SerializationArtifactSpec {
            artifact_id: "omitted-repeat",
            target: SerializationArtifactTarget::EditableSave,
            variant_id: "omitted-repeat",
            branches: Profiled {
                official: Branch::Unresolved,
                filing_safe: Branch::Executable(SerializationPlan {
                    nodes: OMITTED_REPEAT_NODES,
                }),
            },
        },
    ];

    static CONTRACT: StaticSerializationContract = StaticSerializationContract {
        contract_version: "1.0.0",
        canonical_sha256: Some(CONTRACT_DIGEST),
        artifacts: ARTIFACTS,
    };

    struct SyntheticInspector;

    impl CheckedSerializationInspector for SyntheticInspector {
        fn evaluate_presence(
            &mut self,
            presence: SerializationPresence,
            _current_group: Option<&RepeatedGroupInstance>,
        ) -> Result<bool, SerializationInspectionError> {
            match presence {
                SerializationPresence::Always => Ok(true),
                SerializationPresence::Omitted => Ok(false),
                SerializationPresence::When(Predicate::Constant(value)) => Ok(value),
                SerializationPresence::When(_) => Err(SerializationInspectionError::Unavailable),
            }
        }

        fn resolve_value_source(
            &mut self,
            projection: SerializationValueProjection,
            current_group: Option<&RepeatedGroupInstance>,
        ) -> Result<MaterializedValueSourceView, SerializationInspectionError> {
            let SerializationValueProjection::Field(field) = projection else {
                return Err(SerializationInspectionError::Unavailable);
            };
            let field_id = FieldId::parse(field.field_id)
                .map_err(|_| SerializationInspectionError::Unavailable)?;
            let field = match field.instance {
                FieldInstanceSelector::Singleton => FieldInstance::singleton(field_id),
                FieldInstanceSelector::CurrentGroupInstance => FieldInstance::try_new(
                    field_id,
                    vec![
                        current_group
                            .cloned()
                            .ok_or(SerializationInspectionError::Unavailable)?,
                    ],
                )
                .map_err(|_| SerializationInspectionError::Unavailable)?,
                FieldInstanceSelector::StableInstanceId(_) => {
                    return Err(SerializationInspectionError::Unavailable);
                }
            };
            Ok(MaterializedValueSourceView::Field { field })
        }
    }

    fn identity() -> FormRevisionKey {
        FormRevisionKey::new(
            RuleSetId::parse("checked-serialization-test-v1-p1").unwrap(),
            FormCode::parse("TEST").unwrap(),
            FormRevision::parse("v1").unwrap(),
            OfficialPackageVersion::parse("p1").unwrap(),
            Sha256Digest::from_bytes([0x44; 32]),
        )
    }

    fn artifact(variant: &str) -> SerializationArtifactIdentity {
        SerializationArtifactIdentity::new(
            SerializationArtifactTarget::EditableSave,
            ArtifactVariantId::parse(variant).unwrap(),
        )
    }

    fn row(instance_id: &str) -> RepeatedGroupInstance {
        RepeatedGroupInstance::new(
            RepeatedGroupId::parse("rows").unwrap(),
            StableInstanceId::parse(instance_id).unwrap(),
        )
    }

    fn field_instance(field_id: &str, group: Option<&RepeatedGroupInstance>) -> FieldInstance {
        let field_id = FieldId::parse(field_id).unwrap();
        match group {
            Some(group) => FieldInstance::try_new(field_id, vec![group.clone()]).unwrap(),
            None => FieldInstance::singleton(field_id),
        }
    }

    fn trusted() -> TrustedEvaluation {
        let row_a = row("row-a");
        let row_b = row("row-b");
        let request = EvaluationRequest::try_new(
            identity(),
            ValidationContext::new(ValidationPhase::Save, BehaviorProfile::FilingSafe),
            InputRevision::new(23),
            Vec::new(),
            vec![row_a.clone(), row_b.clone()],
            vec![
                RawFieldValue::new(
                    field_instance("first", None),
                    RawValue::Text("Alpha One".to_string()),
                ),
                RawFieldValue::new(
                    field_instance("second", None),
                    RawValue::Text("Beta/Two".to_string()),
                ),
                RawFieldValue::new(
                    field_instance("stamp", None),
                    RawValue::Text("2026-07-24".to_string()),
                ),
                RawFieldValue::new(
                    field_instance("optional", None),
                    RawValue::Text("hidden".to_string()),
                ),
                RawFieldValue::new(
                    field_instance("line", Some(&row_a)),
                    RawValue::Text("row A".to_string()),
                ),
                RawFieldValue::new(
                    field_instance("line", Some(&row_b)),
                    RawValue::Text("row+B".to_string()),
                ),
            ],
        )
        .unwrap();
        let canonical_inputs = request
            .raw_inputs()
            .fields()
            .iter()
            .map(|raw| {
                let canonical = match raw.value() {
                    RawValue::Text(value) => CanonicalValue::Text(value.clone()),
                    value => panic!("unexpected synthetic raw value: {value:?}"),
                };
                CanonicalFieldValue::new(raw.field().clone(), raw.value().clone(), canonical)
            })
            .collect();
        let expectation = EvaluationExpectation::try_new(Vec::new(), Vec::new()).unwrap();
        let result = EvaluationResult::try_new(
            &request,
            &expectation,
            EvaluationOutput::new(canonical_inputs, Vec::new(), Vec::new(), Vec::new()),
        )
        .unwrap();
        TrustedEvaluation::try_from_parts_for_test(request, result).unwrap()
    }

    fn emission(ordinal: u32, group_path: Vec<RepeatedGroupInstance>) -> CheckedEmissionId {
        CheckedEmissionId {
            ordinal,
            group_path,
        }
    }

    fn emitted_field(
        ordinal: u32,
        group: Option<&RepeatedGroupInstance>,
        key: &str,
        occurrence: u32,
        field_id: &str,
        semantic: &str,
        encoded: &str,
    ) -> CheckedTraceEntry {
        CheckedTraceEntry::Record(CheckedMaterializedRecord {
            emission_id: emission(ordinal, group.cloned().into_iter().collect()),
            binding: MaterializedBindingView::PseudoXmlField {
                key: key.to_string(),
                occurrence,
            },
            value_source: MaterializedValueSourceView::Field {
                field: field_instance(field_id, group),
            },
            omission: MaterializedOmissionView::Emitted,
            semantic_value: Some(CanonicalValue::Text(semantic.to_string())),
            semantic_body: Some(semantic.to_string()),
            encoded_body: Some(encoded.to_string()),
        })
    }

    fn reviewed_literal(
        ordinal: u32,
        group: Option<&RepeatedGroupInstance>,
        bytes: &[u8],
    ) -> CheckedTraceEntry {
        CheckedTraceEntry::Record(CheckedMaterializedRecord {
            emission_id: emission(ordinal, group.cloned().into_iter().collect()),
            binding: MaterializedBindingView::ReviewedLiteral {
                exact_bytes: bytes.to_vec(),
            },
            value_source: MaterializedValueSourceView::None,
            omission: MaterializedOmissionView::Emitted,
            semantic_value: None,
            semantic_body: None,
            encoded_body: None,
        })
    }

    fn base_materialization(
        artifact: SerializationArtifactIdentity,
        artifact_id: &str,
        trace: Vec<CheckedTraceEntry>,
    ) -> CheckedMaterialization {
        let trusted = trusted();
        let record_manifest_digest =
            digest_serializable(RECORD_MANIFEST_DIGEST_DOMAIN, &trace).unwrap();
        CheckedMaterialization {
            rule_set: trusted.rule_set().clone(),
            context: trusted.context(),
            input_revision: trusted.input_revision(),
            context_fingerprint: trusted.context_fingerprint(),
            artifact_id: artifact_id.to_string(),
            artifact,
            contract_digest: Sha256Digest::parse(CONTRACT_DIGEST).unwrap(),
            raw_input_digest: digest_serializable(RAW_INPUT_DIGEST_DOMAIN, trusted.raw_inputs())
                .unwrap(),
            evaluation_digest: digest_serializable(EVALUATION_DIGEST_DOMAIN, trusted.result())
                .unwrap(),
            record_manifest_digest,
            recomputed_record_manifest_digest: record_manifest_digest,
            trace,
        }
    }

    fn success_materialization() -> CheckedMaterialization {
        let row_a = row("row-a");
        let row_b = row("row-b");
        base_materialization(
            artifact("success"),
            "synthetic-save",
            vec![
                reviewed_literal(1, None, b"HEAD\n"),
                emitted_field(2, None, "Repeat", 1, "first", "Alpha One", "Alpha%20One"),
                emitted_field(3, None, "Repeat", 2, "second", "Beta/Two", "Beta%2FTwo"),
                CheckedTraceEntry::Record(CheckedMaterializedRecord {
                    emission_id: emission(4, Vec::new()),
                    binding: MaterializedBindingView::MetadataElement {
                        exact_tag: "stamp".to_string(),
                    },
                    value_source: MaterializedValueSourceView::Field {
                        field: field_instance("stamp", None),
                    },
                    omission: MaterializedOmissionView::Emitted,
                    semantic_value: Some(CanonicalValue::Text("2026-07-24".to_string())),
                    semantic_body: Some("2026-07-24".to_string()),
                    encoded_body: Some("2026-07-24".to_string()),
                }),
                CheckedTraceEntry::Record(CheckedMaterializedRecord {
                    emission_id: emission(5, Vec::new()),
                    binding: MaterializedBindingView::PseudoXmlField {
                        key: "Hidden".to_string(),
                        occurrence: 1,
                    },
                    value_source: MaterializedValueSourceView::Field {
                        field: field_instance("optional", None),
                    },
                    omission: MaterializedOmissionView::PresenceFalse,
                    semantic_value: None,
                    semantic_body: None,
                    encoded_body: None,
                }),
                CheckedTraceEntry::GroupAccounting(CheckedGroupAccounting {
                    emission_id: emission(6, Vec::new()),
                    group_id: "rows".to_string(),
                    instances: vec![row_a.clone(), row_b.clone()],
                }),
                emitted_field(7, Some(&row_a), "Line", 1, "line", "row A", "row%20A"),
                reviewed_literal(8, Some(&row_a), b";"),
                emitted_field(7, Some(&row_b), "Line", 2, "line", "row+B", "row%2BB"),
                reviewed_literal(8, Some(&row_b), b";"),
                reviewed_literal(9, None, b"\nTAIL"),
            ],
        )
    }

    fn refresh_record_manifest(materialization: &mut CheckedMaterialization) {
        let digest =
            digest_serializable(RECORD_MANIFEST_DIGEST_DOMAIN, materialization.trace()).unwrap();
        materialization.record_manifest_digest = digest;
        materialization.recomputed_record_manifest_digest = digest;
    }

    fn omitted_repeat_materialization() -> CheckedMaterialization {
        base_materialization(
            artifact("omitted-repeat"),
            "omitted-repeat",
            vec![
                CheckedTraceEntry::Record(CheckedMaterializedRecord {
                    emission_id: emission(1, Vec::new()),
                    binding: MaterializedBindingView::PseudoXmlField {
                        key: "Repeat".to_string(),
                        occurrence: 1,
                    },
                    value_source: MaterializedValueSourceView::Field {
                        field: field_instance("first", None),
                    },
                    omission: MaterializedOmissionView::PresenceFalse,
                    semantic_value: None,
                    semantic_body: None,
                    encoded_body: None,
                }),
                emitted_field(2, None, "Repeat", 2, "second", "Beta/Two", "Beta/Two"),
            ],
        )
    }

    fn build(
        selected_artifact: &SerializationArtifactIdentity,
        materialization: CheckedMaterialization,
    ) -> Result<CheckedSerializationArtifact, CheckedSerializationArtifactError> {
        CheckedSerializationArtifact::try_new_resolved(
            &CONTRACT,
            &trusted(),
            selected_artifact,
            materialization,
            &mut SyntheticInspector,
        )
    }

    #[test]
    fn proof_domains_are_distinct_and_deterministic() {
        let value = vec!["same", "typed", "value"];
        let raw = digest_serializable(RAW_INPUT_DIGEST_DOMAIN, &value).unwrap();
        let evaluation = digest_serializable(EVALUATION_DIGEST_DOMAIN, &value).unwrap();
        let manifest = digest_serializable(RECORD_MANIFEST_DIGEST_DOMAIN, &value).unwrap();

        assert_ne!(raw, evaluation);
        assert_ne!(raw, manifest);
        assert_ne!(evaluation, manifest);
        assert_eq!(
            raw,
            digest_serializable(RAW_INPUT_DIGEST_DOMAIN, &value).unwrap()
        );
    }

    #[test]
    fn resolved_materialization_traverses_contract_and_round_trips_plaintext() {
        let materialization = success_materialization();
        let expected_record_manifest_digest =
            digest_serializable(RECORD_MANIFEST_DIGEST_DOMAIN, materialization.trace()).unwrap();
        let checked = build(&artifact("success"), materialization).unwrap();
        let expected_plaintext = concat!(
            "HEAD\n",
            "<div>Repeat=Alpha%20OneRepeat=</div>",
            "<div>Repeat=Beta%2FTwoRepeat=</div>",
            "<stamp>2026-07-24</stamp>",
            "<div>Line=row%20ALine=</div>;",
            "<div>Line=row%2BBLine=</div>;",
            "\nTAIL"
        )
        .as_bytes();

        assert_eq!(checked.proof_version(), CHECKED_SERIALIZATION_PROOF_VERSION);
        assert_eq!(checked.rule_set(), &identity());
        assert_eq!(checked.context().profile(), BehaviorProfile::FilingSafe);
        assert_eq!(checked.input_revision(), InputRevision::new(23));
        assert_eq!(checked.artifact(), &artifact("success"));
        assert_eq!(checked.artifact_id(), "synthetic-save");
        assert_eq!(
            checked.record_manifest_digest(),
            expected_record_manifest_digest
        );
        assert_eq!(checked.plaintext, expected_plaintext);
        assert_eq!(
            checked.plaintext_byte_len(),
            u64::try_from(expected_plaintext.len()).unwrap()
        );
        assert_eq!(
            checked.plaintext_sha256(),
            sha256_digest(expected_plaintext)
        );
        assert_eq!(
            checked.proof_sha256(),
            sha256_digest(checked.proof_json().as_bytes())
        );

        let proof: serde_json::Value = serde_json::from_str(checked.proof_json()).unwrap();
        assert_eq!(proof["proof_version"], CHECKED_SERIALIZATION_PROOF_VERSION);
        assert!(proof.get("plaintext").is_none());
        assert!(!format!("{checked:?}").contains("Alpha One"));
    }

    #[test]
    fn resolved_materialization_rejects_bound_identity_and_trace_binding_changes() {
        let mut wrong_artifact_id = success_materialization();
        wrong_artifact_id.artifact_id = "other-artifact".to_string();
        assert!(matches!(
            build(&artifact("success"), wrong_artifact_id),
            Err(CheckedSerializationArtifactError::BindingMismatch {
                field: "artifact_id"
            })
        ));

        let mut wrong_occurrence = success_materialization();
        let CheckedTraceEntry::Record(record) = &mut wrong_occurrence.trace[2] else {
            panic!("synthetic trace record changed kind");
        };
        record.binding = MaterializedBindingView::PseudoXmlField {
            key: "Repeat".to_string(),
            occurrence: 3,
        };
        refresh_record_manifest(&mut wrong_occurrence);
        assert!(matches!(
            build(&artifact("success"), wrong_occurrence),
            Err(CheckedSerializationArtifactError::TraceInvariant {
                record_index: Some(2),
                reason: "pseudo-XML binding differs from the selected contract projection",
            })
        ));
    }

    #[test]
    fn resolved_materialization_rejects_each_bound_digest_change() {
        let mismatches = [
            ("contract_digest", 0_u8),
            ("raw_input_digest", 1_u8),
            ("evaluation_digest", 2_u8),
            ("record_manifest_digest", 3_u8),
        ];
        for (field, case) in mismatches {
            let mut materialization = success_materialization();
            let changed = Sha256Digest::from_bytes([case.wrapping_add(0xb0); 32]);
            match case {
                0 => materialization.contract_digest = changed,
                1 => materialization.raw_input_digest = changed,
                2 => materialization.evaluation_digest = changed,
                3 => materialization.record_manifest_digest = changed,
                _ => unreachable!(),
            }
            assert!(matches!(
                build(&artifact("success"), materialization),
                Err(CheckedSerializationArtifactError::DigestMismatch {
                    field: actual
                }) if actual == field
            ));
        }
    }

    #[test]
    fn omitted_occurrence_cannot_advance_the_plaintext_occurrence_sequence() {
        let error = build(
            &artifact("omitted-repeat"),
            omitted_repeat_materialization(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            CheckedSerializationArtifactError::PlaintextRender { ref message }
                if message.contains("declares occurrence 2")
                    && message.contains("expected 1")
        ));
    }
}
