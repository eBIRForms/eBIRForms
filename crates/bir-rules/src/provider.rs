use crate::{
    EvaluationError, EvaluationRequest, EvaluationResult, FormRevisionKey, MaterializationError,
    SerializationInspectionError, SerializationInspector, SerializationMaterialization,
    StaticSerializationContract, WorkflowAction, WorkflowStateId, WorkflowTransitionError,
    WorkflowTransitionResult, serialization::SerializationArtifactIdentity,
};

pub(crate) mod sealed {
    use crate::{
        EvaluationError, EvaluationExpectation, EvaluationOutput, EvaluationRequest,
        EvaluationResult, MaterializationError, SerializationInspectionError,
        SerializationInspector, SerializationMaterialization, WorkflowAction, WorkflowStateId,
        WorkflowTransitionError, WorkflowTransitionResult,
        serialization::SerializationArtifactIdentity,
    };

    pub trait Sealed {
        fn expected_evaluation(
            &self,
            request: &EvaluationRequest,
        ) -> Result<EvaluationExpectation, EvaluationError>;

        fn evaluate_compiled(
            &self,
            request: &EvaluationRequest,
        ) -> Result<EvaluationOutput, EvaluationError>;

        fn materialize_compiled(
            &self,
            _request: &EvaluationRequest,
            _artifact: &SerializationArtifactIdentity,
        ) -> Result<SerializationMaterialization, MaterializationError> {
            Err(MaterializationError::ArtifactSelection { matches: 0 })
        }

        fn inspect_serialization_compiled(
            &self,
            _request: &EvaluationRequest,
            _result: &EvaluationResult,
        ) -> Result<SerializationInspector, SerializationInspectionError> {
            Err(SerializationInspectionError::Unavailable)
        }

        fn transition_workflow_compiled(
            &self,
            _request: &EvaluationRequest,
            _result: &EvaluationResult,
            _current_state: &WorkflowStateId,
            _action: WorkflowAction,
        ) -> Result<WorkflowTransitionResult, WorkflowTransitionError> {
            Err(WorkflowTransitionError::Unavailable {
                state: crate::static_ir::BranchState::Unresolved,
            })
        }
    }
}

/// UI-agnostic interface implemented only by reviewed code in this crate.
///
/// The private supertrait is intentional: downstream crates may select and
/// call packaged snapshots, but cannot label arbitrary handwritten or
/// dynamically loaded corpus logic as a `CompiledRuleSet`.
pub trait CompiledRuleSet: sealed::Sealed + Send + Sync {
    fn identity(&self) -> &FormRevisionKey;

    /// Exact reviewed serialization inventory bound to this rule-set identity.
    ///
    /// The returned contract is descriptive only. Selecting or materializing
    /// an artifact remains a separate trusted operation and has no fallback.
    fn serialization_contract(&self) -> &'static StaticSerializationContract;

    fn evaluate(&self, request: &EvaluationRequest) -> Result<EvaluationResult, EvaluationError> {
        if request.rule_set() != self.identity() {
            return Err(EvaluationError::RuleSetMismatch {
                expected: self.identity().clone(),
                requested: request.rule_set().clone(),
            });
        }
        let expectation = sealed::Sealed::expected_evaluation(self, request)?;
        let output = sealed::Sealed::evaluate_compiled(self, request)?;
        EvaluationResult::try_new(request, &expectation, output)
    }

    fn materialize_serialization(
        &self,
        request: &EvaluationRequest,
        artifact: &SerializationArtifactIdentity,
    ) -> Result<SerializationMaterialization, MaterializationError> {
        if request.rule_set() != self.identity() {
            return Err(MaterializationError::RuleSetMismatch);
        }
        sealed::Sealed::materialize_compiled(self, request, artifact)
    }

    /// Constructs one opaque inspector over the exact request and already
    /// validated result. Only the sealed compiled provider can bind the
    /// inspector to its static rule-set specification.
    #[doc(hidden)]
    fn serialization_inspector(
        &self,
        request: &EvaluationRequest,
        result: &EvaluationResult,
    ) -> Result<SerializationInspector, SerializationInspectionError> {
        if request.rule_set() != self.identity() || result.rule_set() != self.identity() {
            return Err(SerializationInspectionError::BindingMismatch { field: "rule_set" });
        }
        sealed::Sealed::inspect_serialization_compiled(self, request, result)
    }

    /// Selects one explicit workflow transition against the exact evaluated
    /// request. A valid report alone never mutates workflow state.
    fn transition_workflow(
        &self,
        request: &EvaluationRequest,
        result: &EvaluationResult,
        current_state: &WorkflowStateId,
        action: WorkflowAction,
    ) -> Result<WorkflowTransitionResult, WorkflowTransitionError> {
        if request.rule_set() != self.identity() || result.rule_set() != self.identity() {
            return Err(WorkflowTransitionError::BindingMismatch { field: "rule_set" });
        }
        sealed::Sealed::transition_workflow_compiled(self, request, result, current_state, action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BehaviorProfile, CanonicalFieldValue, CanonicalValue, EvaluationExpectation,
        EvaluationOutput, FieldId, FormCode, FormRevision, InputRevision, OfficialPackageVersion,
        RawFieldValue, RawValue, RuleSetId, Sha256Digest, ValidationContext, ValidationPhase,
    };

    struct EchoRuleSet {
        identity: FormRevisionKey,
    }

    impl sealed::Sealed for EchoRuleSet {
        fn expected_evaluation(
            &self,
            _request: &EvaluationRequest,
        ) -> Result<EvaluationExpectation, EvaluationError> {
            EvaluationExpectation::try_new(Vec::new(), Vec::new())
        }

        fn evaluate_compiled(
            &self,
            request: &EvaluationRequest,
        ) -> Result<EvaluationOutput, EvaluationError> {
            Ok(EvaluationOutput::new(
                request
                    .raw_inputs()
                    .fields()
                    .iter()
                    .map(|raw| {
                        CanonicalFieldValue::new(
                            raw.field().clone(),
                            raw.value().clone(),
                            match raw.value() {
                                RawValue::Absent => CanonicalValue::Absent,
                                RawValue::Text(value) if value.is_empty() => CanonicalValue::Blank,
                                RawValue::Text(value) => CanonicalValue::Text(value.clone()),
                            },
                        )
                    })
                    .collect(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ))
        }
    }

    impl CompiledRuleSet for EchoRuleSet {
        fn identity(&self) -> &FormRevisionKey {
            &self.identity
        }

        fn serialization_contract(&self) -> &'static StaticSerializationContract {
            &StaticSerializationContract::EMPTY_V1
        }
    }

    fn identity(id: &str, digest_byte: u8) -> FormRevisionKey {
        FormRevisionKey::new(
            RuleSetId::parse(id).unwrap(),
            FormCode::parse("TEST").unwrap(),
            FormRevision::parse("v1").unwrap(),
            OfficialPackageVersion::parse("p1").unwrap(),
            Sha256Digest::from_bytes([digest_byte; 32]),
        )
    }

    fn request(identity: FormRevisionKey) -> EvaluationRequest {
        EvaluationRequest::try_new(
            identity,
            ValidationContext::new(ValidationPhase::DraftPreview, BehaviorProfile::FilingSafe),
            InputRevision::new(4),
            Vec::new(),
            Vec::new(),
            vec![RawFieldValue::new(
                crate::FieldInstance::singleton(FieldId::parse("name").unwrap()),
                RawValue::Text("Taxpayer".into()),
            )],
        )
        .unwrap()
    }

    #[test]
    fn sealed_wrapper_accepts_exact_identity_and_validates_output() {
        let rules = EchoRuleSet {
            identity: identity("test-v1-p1", 1),
        };
        let result = rules.evaluate(&request(rules.identity.clone())).unwrap();
        assert_eq!(result.canonical_inputs().len(), 1);
        assert!(result.report().is_complete());
    }

    #[test]
    fn sealed_wrapper_rejects_same_form_with_different_digest() {
        let rules = EchoRuleSet {
            identity: identity("test-v1-p1", 1),
        };
        let result = rules.evaluate(&request(identity("test-v1-p1", 2)));
        assert!(matches!(
            result,
            Err(EvaluationError::RuleSetMismatch { .. })
        ));
    }
}
