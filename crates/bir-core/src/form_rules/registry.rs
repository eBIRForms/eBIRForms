use super::{
    CheckedSerializationArtifact, CheckedSerializationArtifactError, EvaluationStamp,
    ShadowEvaluationOutcome,
};
use bir_rules::{
    BehaviorProfile, ContextFingerprint, ContextValueSnapshot, EvaluationError, EvaluationRequest,
    EvaluationResult, FormRevisionKey, InputRevision, RawInputSnapshot, RegistryError,
    RuleSetRegistry, ValidationContext, WorkflowAction, WorkflowStateId, WorkflowTransitionError,
    WorkflowTransitionResult, serialization::SerializationArtifactIdentity,
};
use std::{error::Error, fmt};

/// Inert dispatcher over a caller-supplied compiled rule-set registry.
///
/// Resolution always uses the request's complete `FormRevisionKey`. There is
/// intentionally no form-code lookup or default/current snapshot fallback.
pub struct FormRuleEvaluator<'registry> {
    registry: &'registry dyn RuleSetRegistry,
}

impl<'registry> FormRuleEvaluator<'registry> {
    pub const fn new(registry: &'registry dyn RuleSetRegistry) -> Self {
        Self { registry }
    }

    /// Observe compiled behavior without authorizing any UI or queue action.
    ///
    /// Registry and evaluation failures are returned as data so shadow
    /// integration can record them. This method performs no persistence,
    /// capability mutation, or fallback to handwritten behavior.
    pub fn evaluate_shadow(&self, request: &EvaluationRequest) -> ShadowEvaluationOutcome {
        let stamp = EvaluationStamp::from_request(request);
        let rule_set = match self.registry.resolve(request.rule_set()) {
            Ok(rule_set) => rule_set,
            Err(error) => {
                return ShadowEvaluationOutcome::RegistryFailure { stamp, error };
            }
        };

        match rule_set.evaluate(request) {
            Ok(result) => ShadowEvaluationOutcome::Evaluated { stamp, result },
            Err(error) => ShadowEvaluationOutcome::EvaluationFailure { stamp, error },
        }
    }

    /// Evaluate an explicit official-compatibility workflow transition for
    /// diagnostics only. The exact request/result pair remains mandatory.
    pub fn transition_shadow(
        &self,
        request: &EvaluationRequest,
        result: &EvaluationResult,
        current_state: &WorkflowStateId,
        action: WorkflowAction,
    ) -> Result<WorkflowTransitionResult, WorkflowDispatchError> {
        let rule_set = self
            .registry
            .resolve(request.rule_set())
            .map_err(WorkflowDispatchError::Registry)?;
        rule_set
            .transition_workflow(request, result, current_state, action)
            .map_err(WorkflowDispatchError::Transition)
    }

    /// Evaluate at a trusted boundary.
    ///
    /// Missing exact identity, digest mismatch, unresolved context, and all
    /// evaluator failures remain errors. Callers must not create a final copy
    /// or queue entry unless this returns `Ok`.
    pub fn evaluate_trusted(
        &self,
        request: &EvaluationRequest,
    ) -> Result<TrustedEvaluation, TrustedEvaluationError> {
        if request.context().profile() != BehaviorProfile::FilingSafe {
            return Err(TrustedEvaluationError::ProfileNotAuthorized {
                profile: request.context().profile(),
            });
        }
        let rule_set = self
            .registry
            .resolve(request.rule_set())
            .map_err(TrustedEvaluationError::Registry)?;
        let result = rule_set
            .evaluate(request)
            .map_err(TrustedEvaluationError::Evaluation)?;
        TrustedEvaluation::try_new(request, result)
    }

    /// Materialize and independently parse one exact reviewed plaintext
    /// artifact from a previously trusted request.
    ///
    /// This remains distinct from Final Copy authorization: the returned
    /// artifact cannot be converted into a queue, encrypted container, or
    /// submission payload by this API.
    pub fn materialize_checked(
        &self,
        trusted: &TrustedEvaluation,
        artifact: &SerializationArtifactIdentity,
    ) -> Result<CheckedSerializationArtifact, CheckedSerializationArtifactError> {
        let rule_set = self
            .registry
            .resolve(trusted.rule_set())
            .map_err(CheckedSerializationArtifactError::registry)?;
        CheckedSerializationArtifact::try_new(rule_set, trusted, artifact)
    }

    /// Evaluate a filing-safe workflow transition from an opaque trusted
    /// evaluation. Official compatibility cannot enter this boundary.
    pub fn transition_trusted(
        &self,
        trusted: &TrustedEvaluation,
        current_state: &WorkflowStateId,
        action: WorkflowAction,
    ) -> Result<WorkflowTransitionResult, WorkflowDispatchError> {
        let rule_set = self
            .registry
            .resolve(trusted.rule_set())
            .map_err(WorkflowDispatchError::Registry)?;
        rule_set
            .transition_workflow(trusted.request(), trusted.result(), current_state, action)
            .map_err(WorkflowDispatchError::Transition)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowDispatchError {
    Registry(RegistryError),
    Transition(WorkflowTransitionError),
}

impl fmt::Display for WorkflowDispatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registry(error) => write!(formatter, "rule-set resolution failed: {error}"),
            Self::Transition(error) => write!(formatter, "workflow transition failed: {error}"),
        }
    }
}

impl Error for WorkflowDispatchError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Registry(error) => Some(error),
            Self::Transition(error) => Some(error),
        }
    }
}

/// Opaque proof that one exact request passed the trusted evaluator boundary.
///
/// This type cannot be constructed by application callers from a bare
/// `EvaluationResult`. It owns the evaluated request snapshot so persistence
/// can bind the exact raw/context inputs that produced the result.
#[derive(Clone, PartialEq, Eq)]
pub struct TrustedEvaluation {
    request: EvaluationRequest,
    result: EvaluationResult,
}

impl fmt::Debug for TrustedEvaluation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TrustedEvaluation")
            .field("rule_set", self.rule_set())
            .field("context", &self.context())
            .field("input_revision", &self.input_revision())
            .field("context_fingerprint", &self.context_fingerprint())
            .finish_non_exhaustive()
    }
}

impl TrustedEvaluation {
    fn try_new(
        request: &EvaluationRequest,
        result: EvaluationResult,
    ) -> Result<Self, TrustedEvaluationError> {
        if request.context().profile() != BehaviorProfile::FilingSafe {
            return Err(TrustedEvaluationError::ProfileNotAuthorized {
                profile: request.context().profile(),
            });
        }
        if result.rule_set() != request.rule_set() {
            return Err(TrustedEvaluationError::ResultMetadataMismatch { field: "rule_set" });
        }
        if result.context() != request.context() {
            return Err(TrustedEvaluationError::ResultMetadataMismatch { field: "context" });
        }
        if result.input_revision() != request.input_revision() {
            return Err(TrustedEvaluationError::ResultMetadataMismatch {
                field: "input_revision",
            });
        }
        if result.context_fingerprint() != request.context_fingerprint() {
            return Err(TrustedEvaluationError::ResultMetadataMismatch {
                field: "context_fingerprint",
            });
        }
        if !result.is_valid() {
            return Err(TrustedEvaluationError::BlockingIssues { result });
        }
        Ok(Self {
            request: request.clone(),
            result,
        })
    }

    /// Test-only bridge for bir-core persistence tests.
    ///
    /// It enforces the same filing-safe, metadata, and blocking checks as the
    /// production evaluator and is not present in normal builds.
    #[cfg(test)]
    pub(crate) fn try_from_parts_for_test(
        request: EvaluationRequest,
        result: EvaluationResult,
    ) -> Result<Self, TrustedEvaluationError> {
        Self::try_new(&request, result)
    }

    pub fn request(&self) -> &EvaluationRequest {
        &self.request
    }

    pub fn result(&self) -> &EvaluationResult {
        &self.result
    }

    pub fn rule_set(&self) -> &FormRevisionKey {
        self.request.rule_set()
    }

    pub const fn context(&self) -> ValidationContext {
        self.request.context()
    }

    pub const fn input_revision(&self) -> InputRevision {
        self.request.input_revision()
    }

    pub const fn context_fingerprint(&self) -> ContextFingerprint {
        self.request.context_fingerprint()
    }

    pub fn context_values(&self) -> &ContextValueSnapshot {
        self.request.context_values()
    }

    pub fn raw_inputs(&self) -> &RawInputSnapshot {
        self.request.raw_inputs()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustedEvaluationError {
    ProfileNotAuthorized { profile: BehaviorProfile },
    Registry(RegistryError),
    Evaluation(EvaluationError),
    ResultMetadataMismatch { field: &'static str },
    BlockingIssues { result: EvaluationResult },
}

impl fmt::Display for TrustedEvaluationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProfileNotAuthorized { profile } => write!(
                formatter,
                "behavior profile {profile:?} cannot authorize a trusted filing action"
            ),
            Self::Registry(error) => write!(formatter, "rule-set resolution failed: {error}"),
            Self::Evaluation(error) => write!(formatter, "rule-set evaluation failed: {error}"),
            Self::ResultMetadataMismatch { field } => {
                write!(formatter, "trusted result does not match request {field}")
            }
            Self::BlockingIssues { result } => write!(
                formatter,
                "trusted evaluation produced {} blocking issue(s)",
                result
                    .report()
                    .violations()
                    .iter()
                    .filter(|issue| issue.severity() == bir_rules::RuleSeverity::Blocking)
                    .count()
            ),
        }
    }
}

impl Error for TrustedEvaluationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ProfileNotAuthorized { .. } | Self::ResultMetadataMismatch { .. } => None,
            Self::Registry(error) => Some(error),
            Self::Evaluation(error) => Some(error),
            Self::BlockingIssues { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bir_rules::{
        BehaviorProfile, EvaluationExpectation, EvaluationOutput, EvaluationRequest,
        EvaluationResult, FormCode, FormRevision, FormRevisionKey, InputRevision,
        OfficialPackageVersion, RegistryError, RuleSetId, RuleSetRegistryEntry, Sha256Digest,
        StaticRuleSetRegistry, ValidationContext, ValidationPhase,
        serialization::{
            ArtifactVariantId, SerializationArtifactIdentity, SerializationArtifactTarget,
        },
    };

    static EMPTY_ENTRIES: [RuleSetRegistryEntry; 0] = [];

    fn identity() -> FormRevisionKey {
        FormRevisionKey::new(
            RuleSetId::parse("test-v1-p1").unwrap(),
            FormCode::parse("TEST").unwrap(),
            FormRevision::parse("v1").unwrap(),
            OfficialPackageVersion::parse("p1").unwrap(),
            Sha256Digest::from_bytes([4; 32]),
        )
    }

    fn request_with_profile(profile: BehaviorProfile) -> EvaluationRequest {
        EvaluationRequest::try_new(
            identity(),
            ValidationContext::new(ValidationPhase::Validate, profile),
            InputRevision::new(17),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap()
    }

    fn request() -> EvaluationRequest {
        request_with_profile(BehaviorProfile::FilingSafe)
    }

    fn trusted() -> TrustedEvaluation {
        let request = request();
        let expectation = EvaluationExpectation::try_new(Vec::new(), Vec::new()).unwrap();
        let result = EvaluationResult::try_new(
            &request,
            &expectation,
            EvaluationOutput::new(Vec::new(), Vec::new(), Vec::new(), Vec::new()),
        )
        .unwrap();
        TrustedEvaluation::try_new(&request, result).unwrap()
    }

    #[test]
    fn shadow_records_missing_exact_identity_and_preserves_request_stamp() {
        let registry = StaticRuleSetRegistry::try_new(&EMPTY_ENTRIES).unwrap();
        let evaluator = FormRuleEvaluator::new(&registry);
        let request = request();

        match evaluator.evaluate_shadow(&request) {
            ShadowEvaluationOutcome::RegistryFailure { stamp, error } => {
                assert_eq!(stamp.rule_set(), request.rule_set());
                assert_eq!(stamp.input_revision(), InputRevision::new(17));
                assert_eq!(
                    stamp.context_fingerprint(),
                    request.context_values().fingerprint()
                );
                assert!(matches!(
                    error,
                    RegistryError::NotFound { requested } if requested == identity()
                ));
            }
            outcome => panic!("unexpected shadow outcome: {outcome:?}"),
        }
    }

    #[test]
    fn trusted_evaluation_fails_closed_when_exact_identity_is_unregistered() {
        let registry = StaticRuleSetRegistry::try_new(&EMPTY_ENTRIES).unwrap();
        let evaluator = FormRuleEvaluator::new(&registry);

        assert!(matches!(
            evaluator.evaluate_trusted(&request()),
            Err(TrustedEvaluationError::Registry(
                RegistryError::NotFound { .. }
            ))
        ));
    }

    #[test]
    fn workflow_dispatch_fails_before_transition_when_identity_is_unregistered() {
        let registry = StaticRuleSetRegistry::try_new(&EMPTY_ENTRIES).unwrap();
        let evaluator = FormRuleEvaluator::new(&registry);
        let trusted = trusted();
        let state = WorkflowStateId::parse("edit").unwrap();

        assert!(matches!(
            evaluator.transition_shadow(
                trusted.request(),
                trusted.result(),
                &state,
                WorkflowAction::Validate,
            ),
            Err(WorkflowDispatchError::Registry(
                RegistryError::NotFound { .. }
            ))
        ));
        assert!(matches!(
            evaluator.transition_trusted(&trusted, &state, WorkflowAction::Validate),
            Err(WorkflowDispatchError::Registry(
                RegistryError::NotFound { .. }
            ))
        ));
    }

    #[test]
    fn official_compatibility_cannot_authorize_trusted_evaluation() {
        let registry = StaticRuleSetRegistry::try_new(&EMPTY_ENTRIES).unwrap();
        let evaluator = FormRuleEvaluator::new(&registry);

        assert!(matches!(
            evaluator.evaluate_trusted(&request_with_profile(
                BehaviorProfile::OfficialCompatibility
            )),
            Err(TrustedEvaluationError::ProfileNotAuthorized {
                profile: BehaviorProfile::OfficialCompatibility
            })
        ));
    }

    #[test]
    fn checked_materialization_fails_before_artifact_use_when_identity_is_unregistered() {
        let registry = StaticRuleSetRegistry::try_new(&EMPTY_ENTRIES).unwrap();
        let evaluator = FormRuleEvaluator::new(&registry);
        let artifact = SerializationArtifactIdentity::new(
            SerializationArtifactTarget::EditableSave,
            ArtifactVariantId::parse("save").unwrap(),
        );

        assert!(matches!(
            evaluator.materialize_checked(&trusted(), &artifact),
            Err(CheckedSerializationArtifactError::Registry(
                RegistryError::NotFound { .. }
            ))
        ));
    }
}
