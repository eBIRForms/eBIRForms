//! Read-only, fail-closed submission materialization from an active Final Copy.
//!
//! This boundary deliberately stops before encryption, queueing, persistence,
//! networking, or transport. It reconstructs a Submit-phase request only from
//! the active persisted Final Copy and resolves only the generated reviewed
//! rule-set registry.

use super::{
    CheckedSerializationArtifact, CheckedSerializationArtifactError, FormRuleEvaluator,
    TrustedEvaluationError,
};
use crate::db::{Database, FormFinalCopy, FormRuleStateError};
use bir_rules::{
    BehaviorProfile, EvaluationError, EvaluationRequest, FormRevisionKey, InputRevision,
    RawInputSnapshot, RegistryError, RuleSetRegistry, StaticRuleSetRegistry, ValidationContext,
    ValidationPhase,
    serialization::{
        ArtifactVariantId, SerializationArtifactIdentity, SerializationArtifactTarget,
        SerializationError,
    },
    serialization_contract::SerializationArtifactSpec,
};
use std::fmt;
use thiserror::Error;

/// Persisted filing authority that would be required to choose among otherwise
/// valid reviewed paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistedSubmissionAuthorityInput {
    SubmissionArtifactVariant,
}

/// Opaque result of a read-only submission preflight.
///
/// This value is intentionally neither `Clone` nor serializable. It carries no
/// boolean authority, exposes no payload bytes, and cannot be constructed from
/// a bare evaluation or from a caller-supplied rule-set registry.
pub struct SubmissionPreflightArtifact {
    finalization_id: i64,
    artifact: CheckedSerializationArtifact,
}

impl fmt::Debug for SubmissionPreflightArtifact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SubmissionPreflightArtifact")
            .field("finalization_id", &self.finalization_id)
            .field("rule_set", self.artifact.rule_set())
            .field("context", &self.artifact.context())
            .field("artifact", self.artifact.artifact())
            .field("plaintext_sha256", &self.artifact.plaintext_sha256())
            .finish_non_exhaustive()
    }
}

impl SubmissionPreflightArtifact {
    pub const fn finalization_id(&self) -> i64 {
        self.finalization_id
    }

    pub fn rule_set(&self) -> &FormRevisionKey {
        self.artifact.rule_set()
    }

    pub const fn plaintext_sha256(&self) -> bir_rules::Sha256Digest {
        self.artifact.plaintext_sha256()
    }
}

/// Re-evaluate and materialize the unique reviewed submission artifact for the
/// active persisted Final Copy of `form_draft_id`.
///
/// No caller can supply a registry, raw snapshot, context, rule-set identity,
/// validation phase, behavior profile, or artifact target to this boundary.
pub fn preflight_active_form_submission(
    database: &Database,
    form_draft_id: i64,
) -> Result<SubmissionPreflightArtifact, SubmissionPreflightError> {
    let final_copy = database
        .load_active_form_final_copy(form_draft_id)?
        .ok_or(SubmissionPreflightError::NoActiveFinalCopy { form_draft_id })?;
    let request = reconstruct_submit_request(&final_copy)?;
    require_submit_request(&request)?;

    let registry =
        StaticRuleSetRegistry::try_new(bir_rules::generated::reviewed_rule_set_entries())
            .map_err(SubmissionPreflightError::ReviewedRegistry)?;
    let evaluator = FormRuleEvaluator::new(&registry);
    let trusted = evaluator
        .evaluate_trusted(&request)
        .map_err(SubmissionPreflightError::TrustedEvaluation)?;
    let rule_set = registry
        .resolve(request.rule_set())
        .map_err(SubmissionPreflightError::ReviewedRegistry)?;
    let selected_artifact =
        select_submission_artifact(final_copy.id, rule_set.serialization_contract().artifacts)?;
    require_submission_artifact(&selected_artifact)?;
    let artifact = evaluator
        .materialize_checked(&trusted, &selected_artifact)
        .map_err(SubmissionPreflightError::CheckedMaterialization)?;

    if artifact.context() != request.context() {
        return Err(SubmissionPreflightError::CheckedBindingMismatch { field: "context" });
    }
    require_submission_artifact(artifact.artifact())?;

    Ok(SubmissionPreflightArtifact {
        finalization_id: final_copy.id,
        artifact,
    })
}

fn reconstruct_submit_request(
    final_copy: &FormFinalCopy,
) -> Result<EvaluationRequest, SubmissionPreflightError> {
    let raw_inputs: RawInputSnapshot = serde_json::from_str(&final_copy.raw_snapshot_json)
        .map_err(|source| SubmissionPreflightError::PersistedRawSnapshot {
            finalization_id: final_copy.id,
            source,
        })?;
    EvaluationRequest::try_new(
        final_copy.evaluation.rule_set().clone(),
        ValidationContext::new(ValidationPhase::Submit, BehaviorProfile::FilingSafe),
        InputRevision::new(final_copy.source_storage_revision),
        final_copy.context_values.values().to_vec(),
        raw_inputs.repeated_group_instances().to_vec(),
        raw_inputs.fields().to_vec(),
    )
    .map_err(SubmissionPreflightError::SubmitRequest)
}

fn require_submit_request(request: &EvaluationRequest) -> Result<(), SubmissionPreflightError> {
    if request.context().phase() != ValidationPhase::Submit {
        return Err(SubmissionPreflightError::RequestBindingMismatch { field: "phase" });
    }
    if request.context().profile() != BehaviorProfile::FilingSafe {
        return Err(SubmissionPreflightError::RequestBindingMismatch { field: "profile" });
    }
    Ok(())
}

fn select_submission_artifact(
    finalization_id: i64,
    artifacts: &'static [SerializationArtifactSpec],
) -> Result<SerializationArtifactIdentity, SubmissionPreflightError> {
    let matches = artifacts
        .iter()
        .filter(|artifact| artifact.target == SerializationArtifactTarget::SubmissionPayload)
        .collect::<Vec<_>>();
    let selected = match matches.as_slice() {
        [] => {
            return Err(
                SubmissionPreflightError::MissingReviewedSubmissionArtifact { finalization_id },
            );
        }
        [selected] => *selected,
        _ => {
            return Err(SubmissionPreflightError::MissingPersistedAuthorityInput {
                finalization_id,
                input: PersistedSubmissionAuthorityInput::SubmissionArtifactVariant,
            });
        }
    };
    let variant = ArtifactVariantId::parse(selected.variant_id).map_err(|source| {
        SubmissionPreflightError::InvalidReviewedSubmissionArtifact {
            finalization_id,
            source,
        }
    })?;
    Ok(SerializationArtifactIdentity::new(
        SerializationArtifactTarget::SubmissionPayload,
        variant,
    ))
}

fn require_submission_artifact(
    artifact: &SerializationArtifactIdentity,
) -> Result<(), SubmissionPreflightError> {
    if artifact.target() != SerializationArtifactTarget::SubmissionPayload {
        return Err(SubmissionPreflightError::ArtifactTargetMismatch {
            actual: artifact.target(),
        });
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum SubmissionPreflightError {
    #[error("form-rule persistence failed: {0}")]
    Persistence(#[from] FormRuleStateError),
    #[error("form draft {form_draft_id} has no active persisted Final Copy")]
    NoActiveFinalCopy { form_draft_id: i64 },
    #[error("persisted Final Copy {finalization_id} raw snapshot is invalid: {source}")]
    PersistedRawSnapshot {
        finalization_id: i64,
        #[source]
        source: serde_json::Error,
    },
    #[error("Submit-phase request reconstruction failed: {0}")]
    SubmitRequest(#[source] EvaluationError),
    #[error("reconstructed submission request has a non-authoritative {field}")]
    RequestBindingMismatch { field: &'static str },
    #[error("generated reviewed rule-set registry failed: {0}")]
    ReviewedRegistry(#[source] RegistryError),
    #[error("filing-safe Submit evaluation failed: {0}")]
    TrustedEvaluation(#[source] TrustedEvaluationError),
    #[error("Final Copy {finalization_id} has no reviewed submission serialization artifact")]
    MissingReviewedSubmissionArtifact { finalization_id: i64 },
    #[error(
        "Final Copy {finalization_id} is missing persisted authority input required for {input:?}"
    )]
    MissingPersistedAuthorityInput {
        finalization_id: i64,
        input: PersistedSubmissionAuthorityInput,
    },
    #[error("Final Copy {finalization_id} has an invalid reviewed submission artifact: {source}")]
    InvalidReviewedSubmissionArtifact {
        finalization_id: i64,
        #[source]
        source: SerializationError,
    },
    #[error("submission preflight cannot select artifact target {actual:?}")]
    ArtifactTargetMismatch { actual: SerializationArtifactTarget },
    #[error("checked submission materialization failed: {0}")]
    CheckedMaterialization(#[source] CheckedSerializationArtifactError),
    #[error("checked submission artifact does not match reconstructed {field}")]
    CheckedBindingMismatch { field: &'static str },
}

#[cfg(test)]
mod tests {
    use super::*;
    use bir_rules::{
        FormCode, FormRevision, OfficialPackageVersion, RuleSetId, Sha256Digest,
        serialization_contract::SerializationPlan,
        static_ir::{Branch, Profiled},
    };

    const EMPTY_PLAN: SerializationPlan = SerializationPlan { nodes: &[] };
    const SUBMISSION_ARTIFACT_A: SerializationArtifactSpec = SerializationArtifactSpec {
        artifact_id: "submission-a",
        target: SerializationArtifactTarget::SubmissionPayload,
        variant_id: "submission-a",
        branches: Profiled {
            official: Branch::Unresolved,
            filing_safe: Branch::Executable(EMPTY_PLAN),
        },
    };
    const SUBMISSION_ARTIFACT_B: SerializationArtifactSpec = SerializationArtifactSpec {
        artifact_id: "submission-b",
        target: SerializationArtifactTarget::SubmissionPayload,
        variant_id: "submission-b",
        branches: Profiled {
            official: Branch::Unresolved,
            filing_safe: Branch::Executable(EMPTY_PLAN),
        },
    };

    fn request(phase: ValidationPhase) -> EvaluationRequest {
        EvaluationRequest::try_new(
            FormRevisionKey::new(
                RuleSetId::parse("test-v1-p1").unwrap(),
                FormCode::parse("TEST").unwrap(),
                FormRevision::parse("v1").unwrap(),
                OfficialPackageVersion::parse("p1").unwrap(),
                Sha256Digest::from_bytes([9; 32]),
            ),
            ValidationContext::new(phase, BehaviorProfile::FilingSafe),
            InputRevision::new(1),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap()
    }

    #[test]
    fn non_submit_phase_is_rejected() {
        assert!(matches!(
            require_submit_request(&request(ValidationPhase::FinalCopy)),
            Err(SubmissionPreflightError::RequestBindingMismatch { field: "phase" })
        ));
    }

    #[test]
    fn non_submission_artifact_target_is_rejected() {
        let artifact = SerializationArtifactIdentity::new(
            SerializationArtifactTarget::EncryptedFinalCopy,
            ArtifactVariantId::parse("encrypted-copy").unwrap(),
        );
        assert!(matches!(
            require_submission_artifact(&artifact),
            Err(SubmissionPreflightError::ArtifactTargetMismatch {
                actual: SerializationArtifactTarget::EncryptedFinalCopy
            })
        ));
    }

    #[test]
    fn multiple_reviewed_submission_paths_require_persisted_authority() {
        static ARTIFACTS: [SerializationArtifactSpec; 2] =
            [SUBMISSION_ARTIFACT_A, SUBMISSION_ARTIFACT_B];

        assert!(matches!(
            select_submission_artifact(41, &ARTIFACTS),
            Err(SubmissionPreflightError::MissingPersistedAuthorityInput {
                finalization_id: 41,
                input: PersistedSubmissionAuthorityInput::SubmissionArtifactVariant,
            })
        ));
    }
}
