use crate::{
    CalculationId, CanonicalFieldValue, CanonicalValue, ContextFingerprint, ContextSnapshotError,
    ContextValue, ContextValueSnapshot, FieldInstance, FieldValueSource, FormRevisionKey,
    InputRevision, InputSnapshotError, OutputId, RawFieldValue, RawInputSnapshot, RawValue,
    RepeatedGroupInstance, ReportError, RuleExecution, RuleExpectation, RuleId, RuleViolation,
    ValidationContext, ValidationPhase, ValidationReport,
};
use serde::{Deserialize, Deserializer, Serialize, de};
use std::{error::Error, fmt};

/// Complete immutable input to one rule-set evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvaluationRequest {
    rule_set: FormRevisionKey,
    context: ValidationContext,
    input_revision: InputRevision,
    #[serde(skip_serializing_if = "Option::is_none")]
    event_field: Option<FieldInstance>,
    context_fingerprint: ContextFingerprint,
    context_values: ContextValueSnapshot,
    raw_inputs: RawInputSnapshot,
}

impl EvaluationRequest {
    /// Capture and validate raw fields and materialized repeated-group
    /// instances. Inputs are sorted by stable identity; duplicates and
    /// transient/undeclared row references fail closed. The context
    /// fingerprint is computed from the validated context values.
    pub fn try_new(
        rule_set: FormRevisionKey,
        context: ValidationContext,
        input_revision: InputRevision,
        context_values: Vec<ContextValue>,
        repeated_group_instances: Vec<RepeatedGroupInstance>,
        raw_fields: Vec<RawFieldValue>,
    ) -> Result<Self, EvaluationError> {
        Self::try_new_inner(
            rule_set,
            context,
            input_revision,
            None,
            context_values,
            repeated_group_instances,
            raw_fields,
        )
    }

    /// Capture one exact input/blur/change dispatch.
    ///
    /// The event field is a complete [`FieldInstance`], including its stable
    /// repeated-group identity, and must occur in the captured raw snapshot.
    pub fn try_new_for_field_event(
        rule_set: FormRevisionKey,
        context: ValidationContext,
        input_revision: InputRevision,
        event_field: FieldInstance,
        context_values: Vec<ContextValue>,
        repeated_group_instances: Vec<RepeatedGroupInstance>,
        raw_fields: Vec<RawFieldValue>,
    ) -> Result<Self, EvaluationError> {
        Self::try_new_inner(
            rule_set,
            context,
            input_revision,
            Some(event_field),
            context_values,
            repeated_group_instances,
            raw_fields,
        )
    }

    fn try_new_inner(
        rule_set: FormRevisionKey,
        context: ValidationContext,
        input_revision: InputRevision,
        event_field: Option<FieldInstance>,
        context_values: Vec<ContextValue>,
        repeated_group_instances: Vec<RepeatedGroupInstance>,
        raw_fields: Vec<RawFieldValue>,
    ) -> Result<Self, EvaluationError> {
        let context_values = ContextValueSnapshot::try_new(context_values)
            .map_err(EvaluationError::InvalidContextSnapshot)?;
        let raw_inputs = RawInputSnapshot::try_new(repeated_group_instances, raw_fields)
            .map_err(EvaluationError::InvalidInputSnapshot)?;
        match (context.phase().is_field_event(), event_field.as_ref()) {
            (true, None) => {
                return Err(EvaluationError::MissingEventField {
                    phase: context.phase(),
                });
            }
            (false, Some(field)) => {
                return Err(EvaluationError::UnexpectedEventField {
                    phase: context.phase(),
                    field: field.clone(),
                });
            }
            (true, Some(field)) if raw_inputs.raw_value(field).is_none() => {
                return Err(EvaluationError::EventFieldMissingFromSnapshot {
                    field: field.clone(),
                });
            }
            _ => {}
        }
        let context_fingerprint = context_values.fingerprint();
        Ok(Self {
            rule_set,
            context,
            input_revision,
            event_field,
            context_fingerprint,
            context_values,
            raw_inputs,
        })
    }

    pub fn capture(
        rule_set: FormRevisionKey,
        context: ValidationContext,
        input_revision: InputRevision,
        context_values: Vec<ContextValue>,
        source: &dyn FieldValueSource,
    ) -> Result<Self, EvaluationError> {
        Self::try_new(
            rule_set,
            context,
            input_revision,
            context_values,
            source.repeated_group_instances().to_vec(),
            source.raw_fields().to_vec(),
        )
    }

    pub fn capture_for_field_event(
        rule_set: FormRevisionKey,
        context: ValidationContext,
        input_revision: InputRevision,
        event_field: FieldInstance,
        context_values: Vec<ContextValue>,
        source: &dyn FieldValueSource,
    ) -> Result<Self, EvaluationError> {
        Self::try_new_for_field_event(
            rule_set,
            context,
            input_revision,
            event_field,
            context_values,
            source.repeated_group_instances().to_vec(),
            source.raw_fields().to_vec(),
        )
    }

    pub fn rule_set(&self) -> &FormRevisionKey {
        &self.rule_set
    }

    pub const fn context(&self) -> ValidationContext {
        self.context
    }

    pub const fn input_revision(&self) -> InputRevision {
        self.input_revision
    }

    pub fn event_field(&self) -> Option<&FieldInstance> {
        self.event_field.as_ref()
    }

    pub const fn context_fingerprint(&self) -> ContextFingerprint {
        self.context_fingerprint
    }

    pub fn context_values(&self) -> &ContextValueSnapshot {
        &self.context_values
    }

    pub fn raw_inputs(&self) -> &RawInputSnapshot {
        &self.raw_inputs
    }
}

impl<'de> Deserialize<'de> for EvaluationRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            rule_set: FormRevisionKey,
            context: ValidationContext,
            input_revision: InputRevision,
            #[serde(default)]
            event_field: Option<FieldInstance>,
            context_fingerprint: ContextFingerprint,
            context_values: ContextValueSnapshot,
            raw_inputs: RawInputSnapshot,
        }

        let wire = Wire::deserialize(deserializer)?;
        // RawInputSnapshot has already passed its validating deserializer. Use
        // its canonical slices to keep this constructor as the only request
        // assembly path.
        let supplied_fingerprint = wire.context_fingerprint;
        let request = Self::try_new_inner(
            wire.rule_set,
            wire.context,
            wire.input_revision,
            wire.event_field,
            wire.context_values.values().to_vec(),
            wire.raw_inputs.repeated_group_instances().to_vec(),
            wire.raw_inputs.fields().to_vec(),
        )
        .map_err(de::Error::custom)?;
        let computed_fingerprint = request.context_fingerprint();
        if supplied_fingerprint != computed_fingerprint {
            return Err(de::Error::custom(
                EvaluationError::ContextFingerprintMismatch {
                    supplied: supplied_fingerprint,
                    computed: computed_fingerprint,
                },
            ));
        }
        Ok(request)
    }
}

/// Expected executable inventory for one phase/profile branch.
///
/// Rule order is reviewed display/first-error order. Calculation order is the
/// compiler's topological order. Executions of the same group-scoped rule or
/// output are ordered by stable repeated-group identity. Both lists are exact
/// and duplicate-free.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvaluationExpectation {
    rules: Vec<RuleExpectation>,
    outputs: Vec<DerivedOutputExpectation>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    field_value_assignments: Vec<FieldValueAssignmentExpectation>,
}

impl<'de> Deserialize<'de> for EvaluationExpectation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            rules: Vec<RuleExpectation>,
            outputs: Vec<DerivedOutputExpectation>,
            #[serde(default)]
            field_value_assignments: Vec<FieldValueAssignmentExpectation>,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::try_new_with_field_value_assignments(
            wire.rules,
            wire.outputs,
            wire.field_value_assignments,
        )
        .map_err(de::Error::custom)
    }
}

impl EvaluationExpectation {
    pub fn try_new(
        rules: Vec<RuleExpectation>,
        outputs: Vec<DerivedOutputExpectation>,
    ) -> Result<Self, EvaluationError> {
        Self::try_new_with_field_value_assignments(rules, outputs, Vec::new())
    }

    pub fn try_new_with_field_value_assignments(
        rules: Vec<RuleExpectation>,
        outputs: Vec<DerivedOutputExpectation>,
        field_value_assignments: Vec<FieldValueAssignmentExpectation>,
    ) -> Result<Self, EvaluationError> {
        for (index, rule) in rules.iter().enumerate() {
            if rules[..index]
                .iter()
                .any(|prior| prior.execution() == rule.execution())
            {
                return Err(EvaluationError::DuplicateExpectedRule {
                    execution: rule.execution().clone(),
                });
            }
        }
        for pair in rules.windows(2) {
            if pair[0].order() > pair[1].order()
                || (pair[0].order() == pair[1].order() && pair[0].rule_id() != pair[1].rule_id())
            {
                return Err(EvaluationError::ExpectedRuleOrderNotStrict {
                    previous: pair[0].order(),
                    current: pair[1].order(),
                });
            }
            if pair[0].order() == pair[1].order() && pair[0].execution() >= pair[1].execution() {
                return Err(EvaluationError::ExpectedRuleExecutionOrderNotStrict {
                    order: pair[0].order(),
                    previous: pair[0].execution().clone(),
                    current: pair[1].execution().clone(),
                });
            }
        }
        for (index, output) in outputs.iter().enumerate() {
            if outputs[..index].contains(output) {
                return Err(EvaluationError::DuplicateExpectedOutput {
                    calculation_id: output.calculation_id.clone(),
                    output_id: output.output_id.clone(),
                    instance: output.instance.clone(),
                });
            }
            if let Some(previous) = outputs[..index].iter().rev().find(|prior| {
                prior.calculation_id == output.calculation_id && prior.output_id == output.output_id
            }) && previous.instance.as_ref() >= output.instance.as_ref()
            {
                return Err(EvaluationError::ExpectedOutputInstanceOrderNotStrict {
                    calculation_id: output.calculation_id.clone(),
                    output_id: output.output_id.clone(),
                    previous: previous.instance.clone(),
                    current: output.instance.clone(),
                });
            }
        }
        let mut previous_key = None;
        for (index, assignment) in field_value_assignments.iter().enumerate() {
            let rule_position = rules
                .iter()
                .position(|rule| rule.execution() == assignment.execution())
                .ok_or_else(|| EvaluationError::AssignmentForUnknownRule {
                    execution: assignment.execution().clone(),
                })?;
            if field_value_assignments[..index].iter().any(|prior| {
                prior.execution() == assignment.execution()
                    && prior.effect_index() == assignment.effect_index()
            }) {
                return Err(EvaluationError::DuplicateAssignmentEffect {
                    execution: assignment.execution().clone(),
                    effect_index: assignment.effect_index(),
                });
            }
            let key = (rule_position, assignment.effect_index());
            if previous_key.is_some_and(|previous| previous >= key) {
                return Err(EvaluationError::AssignmentsOutOfOrder {
                    execution: assignment.execution().clone(),
                    effect_index: assignment.effect_index(),
                });
            }
            previous_key = Some(key);
        }
        Ok(Self {
            rules,
            outputs,
            field_value_assignments,
        })
    }

    pub fn rules(&self) -> &[RuleExpectation] {
        &self.rules
    }

    pub fn outputs(&self) -> &[DerivedOutputExpectation] {
        &self.outputs
    }

    pub fn field_value_assignments(&self) -> &[FieldValueAssignmentExpectation] {
        &self.field_value_assignments
    }
}

/// Exact identity of one required named output in reviewed
/// topological/emission order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DerivedOutputExpectation {
    calculation_id: CalculationId,
    output_id: OutputId,
    instance: Option<RepeatedGroupInstance>,
}

impl DerivedOutputExpectation {
    pub const fn new(
        calculation_id: CalculationId,
        output_id: OutputId,
        instance: Option<RepeatedGroupInstance>,
    ) -> Self {
        Self {
            calculation_id,
            output_id,
            instance,
        }
    }

    pub const fn singleton(calculation_id: CalculationId, output_id: OutputId) -> Self {
        Self::new(calculation_id, output_id, None)
    }

    pub fn calculation_id(&self) -> &CalculationId {
        &self.calculation_id
    }

    pub fn output_id(&self) -> &OutputId {
        &self.output_id
    }

    pub fn instance(&self) -> Option<&RepeatedGroupInstance> {
        self.instance.as_ref()
    }
}

/// One derived calculation output, including its exact execution instance, in
/// reviewed topological order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DerivedValue {
    calculation_id: CalculationId,
    output_id: OutputId,
    instance: Option<RepeatedGroupInstance>,
    value: CanonicalValue,
}

impl DerivedValue {
    pub const fn new(
        calculation_id: CalculationId,
        output_id: OutputId,
        instance: Option<RepeatedGroupInstance>,
        value: CanonicalValue,
    ) -> Self {
        Self {
            calculation_id,
            output_id,
            instance,
            value,
        }
    }

    pub const fn singleton(
        calculation_id: CalculationId,
        output_id: OutputId,
        value: CanonicalValue,
    ) -> Self {
        Self::new(calculation_id, output_id, None, value)
    }

    pub fn calculation_id(&self) -> &CalculationId {
        &self.calculation_id
    }

    pub fn output_id(&self) -> &OutputId {
        &self.output_id
    }

    pub fn instance(&self) -> Option<&RepeatedGroupInstance> {
        self.instance.as_ref()
    }

    pub fn value(&self) -> &CanonicalValue {
        &self.value
    }

    pub fn expectation(&self) -> DerivedOutputExpectation {
        DerivedOutputExpectation::new(
            self.calculation_id.clone(),
            self.output_id.clone(),
            self.instance.clone(),
        )
    }
}

/// One requested raw-buffer replacement emitted by a matched field-event rule.
///
/// The request snapshot and each canonical record's retained raw value remain
/// immutable. An exact field-event program may additionally apply the
/// assignment to its canonical working-field overlay so later scheduled steps
/// observe the official DOM mutation; batched evaluations keep assignments as
/// output instructions only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldValueAssignment {
    execution: RuleExecution,
    effect_index: u32,
    field: FieldInstance,
    value: RawValue,
}

/// Exact potential raw assignment compiled into one selected rule execution.
///
/// The result carries this inventory even when the rule predicate does not
/// match, so deserialization can reject forged effect indexes, targets, or
/// replacement values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldValueAssignmentExpectation {
    execution: RuleExecution,
    effect_index: u32,
    field: FieldInstance,
    value: RawValue,
}

impl FieldValueAssignmentExpectation {
    pub const fn new(
        execution: RuleExecution,
        effect_index: u32,
        field: FieldInstance,
        value: RawValue,
    ) -> Self {
        Self {
            execution,
            effect_index,
            field,
            value,
        }
    }

    pub fn execution(&self) -> &RuleExecution {
        &self.execution
    }

    pub const fn effect_index(&self) -> u32 {
        self.effect_index
    }

    pub fn field(&self) -> &FieldInstance {
        &self.field
    }

    pub fn value(&self) -> &RawValue {
        &self.value
    }
}

impl FieldValueAssignment {
    pub const fn new(
        execution: RuleExecution,
        effect_index: u32,
        field: FieldInstance,
        value: RawValue,
    ) -> Self {
        Self {
            execution,
            effect_index,
            field,
            value,
        }
    }

    pub fn execution(&self) -> &RuleExecution {
        &self.execution
    }

    pub const fn effect_index(&self) -> u32 {
        self.effect_index
    }

    pub fn field(&self) -> &FieldInstance {
        &self.field
    }

    pub fn value(&self) -> &RawValue {
        &self.value
    }

    pub fn expectation(&self) -> FieldValueAssignmentExpectation {
        FieldValueAssignmentExpectation::new(
            self.execution.clone(),
            self.effect_index,
            self.field.clone(),
            self.value.clone(),
        )
    }
}

/// Raw output produced by crate-owned generated evaluator code.
///
/// The sealed [`crate::CompiledRuleSet`] wrapper converts this into an
/// [`EvaluationResult`] only after validating every completeness and ordering
/// invariant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluationOutput {
    canonical_inputs: Vec<CanonicalFieldValue>,
    derived_outputs: Vec<DerivedValue>,
    evaluated_rules: Vec<RuleExecution>,
    violations: Vec<RuleViolation>,
    field_value_assignments: Vec<FieldValueAssignment>,
}

impl EvaluationOutput {
    pub fn new(
        canonical_inputs: Vec<CanonicalFieldValue>,
        derived_outputs: Vec<DerivedValue>,
        evaluated_rules: Vec<RuleExecution>,
        violations: Vec<RuleViolation>,
    ) -> Self {
        Self {
            canonical_inputs,
            derived_outputs,
            evaluated_rules,
            violations,
            field_value_assignments: Vec::new(),
        }
    }

    pub fn new_with_field_value_assignments(
        canonical_inputs: Vec<CanonicalFieldValue>,
        derived_outputs: Vec<DerivedValue>,
        evaluated_rules: Vec<RuleExecution>,
        violations: Vec<RuleViolation>,
        field_value_assignments: Vec<FieldValueAssignment>,
    ) -> Self {
        Self {
            canonical_inputs,
            derived_outputs,
            evaluated_rules,
            violations,
            field_value_assignments,
        }
    }

    pub fn with_field_value_assignments(
        mut self,
        field_value_assignments: Vec<FieldValueAssignment>,
    ) -> Self {
        self.field_value_assignments = field_value_assignments;
        self
    }
}

/// Validated, reproducible result returned to GPUI and trusted filing gates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvaluationResult {
    report: ValidationReport,
    canonical_inputs: Vec<CanonicalFieldValue>,
    expected_outputs: Vec<DerivedOutputExpectation>,
    derived_outputs: Vec<DerivedValue>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    expected_field_value_assignments: Vec<FieldValueAssignmentExpectation>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    field_value_assignments: Vec<FieldValueAssignment>,
}

impl EvaluationResult {
    pub fn try_new(
        request: &EvaluationRequest,
        expectation: &EvaluationExpectation,
        output: EvaluationOutput,
    ) -> Result<Self, EvaluationError> {
        validate_canonical_inputs(request.raw_inputs(), &output.canonical_inputs)?;
        validate_calculation_coverage(expectation.outputs(), &output.derived_outputs)?;

        let report = match request.event_field() {
            Some(event_field) => ValidationReport::try_new_for_field_event(
                request.rule_set.clone(),
                request.context,
                request.input_revision,
                event_field.clone(),
                request.context_fingerprint,
                expectation.rules.clone(),
                output.evaluated_rules,
                output.violations,
            ),
            None => ValidationReport::try_new(
                request.rule_set.clone(),
                request.context,
                request.input_revision,
                request.context_fingerprint,
                expectation.rules.clone(),
                output.evaluated_rules,
                output.violations,
            ),
        }
        .map_err(EvaluationError::InvalidReport)?;
        validate_field_value_assignments(
            request.context().phase(),
            report.expected_rules(),
            expectation.field_value_assignments(),
            request
                .raw_inputs()
                .fields()
                .iter()
                .map(RawFieldValue::field),
            &output.field_value_assignments,
        )?;

        Ok(Self {
            report,
            canonical_inputs: output.canonical_inputs,
            expected_outputs: expectation.outputs.clone(),
            derived_outputs: output.derived_outputs,
            expected_field_value_assignments: expectation.field_value_assignments.clone(),
            field_value_assignments: output.field_value_assignments,
        })
    }

    pub fn report(&self) -> &ValidationReport {
        &self.report
    }

    pub fn rule_set(&self) -> &FormRevisionKey {
        self.report.rule_set()
    }

    pub const fn context(&self) -> ValidationContext {
        self.report.context()
    }

    pub const fn input_revision(&self) -> InputRevision {
        self.report.input_revision()
    }

    pub fn event_field(&self) -> Option<&FieldInstance> {
        self.report.event_field()
    }

    pub const fn context_fingerprint(&self) -> ContextFingerprint {
        self.report.context_fingerprint()
    }

    pub fn canonical_inputs(&self) -> &[CanonicalFieldValue] {
        &self.canonical_inputs
    }

    pub fn expected_outputs(&self) -> &[DerivedOutputExpectation] {
        &self.expected_outputs
    }

    pub fn derived_outputs(&self) -> &[DerivedValue] {
        &self.derived_outputs
    }

    pub fn field_value_assignments(&self) -> &[FieldValueAssignment] {
        &self.field_value_assignments
    }

    pub fn expected_field_value_assignments(&self) -> &[FieldValueAssignmentExpectation] {
        &self.expected_field_value_assignments
    }

    pub fn is_valid(&self) -> bool {
        self.report.is_valid()
    }

    fn validate_stored(&self) -> Result<(), EvaluationError> {
        for pair in self.canonical_inputs.windows(2) {
            if pair[0].field() >= pair[1].field() {
                return Err(EvaluationError::CanonicalInputsOutOfOrder {
                    previous: pair[0].field().clone(),
                    current: pair[1].field().clone(),
                });
            }
        }
        if let Some(event_field) = self.event_field()
            && !self
                .canonical_inputs
                .iter()
                .any(|input| input.field() == event_field)
        {
            return Err(EvaluationError::EventFieldMissingFromSnapshot {
                field: event_field.clone(),
            });
        }
        EvaluationExpectation::try_new_with_field_value_assignments(
            self.report.expected_rules().to_vec(),
            self.expected_outputs.clone(),
            self.expected_field_value_assignments.clone(),
        )?;
        validate_calculation_coverage(&self.expected_outputs, &self.derived_outputs)?;
        validate_field_value_assignments(
            self.context().phase(),
            self.report.expected_rules(),
            &self.expected_field_value_assignments,
            self.canonical_inputs.iter().map(CanonicalFieldValue::field),
            &self.field_value_assignments,
        )
    }
}

impl<'de> Deserialize<'de> for EvaluationResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            report: ValidationReport,
            canonical_inputs: Vec<CanonicalFieldValue>,
            expected_outputs: Vec<DerivedOutputExpectation>,
            derived_outputs: Vec<DerivedValue>,
            #[serde(default)]
            expected_field_value_assignments: Vec<FieldValueAssignmentExpectation>,
            #[serde(default)]
            field_value_assignments: Vec<FieldValueAssignment>,
        }

        let wire = Wire::deserialize(deserializer)?;
        let result = Self {
            report: wire.report,
            canonical_inputs: wire.canonical_inputs,
            expected_outputs: wire.expected_outputs,
            derived_outputs: wire.derived_outputs,
            expected_field_value_assignments: wire.expected_field_value_assignments,
            field_value_assignments: wire.field_value_assignments,
        };
        result.validate_stored().map_err(de::Error::custom)?;
        Ok(result)
    }
}

fn validate_field_value_assignments<'a>(
    phase: ValidationPhase,
    expected_rules: &[RuleExpectation],
    expected_assignments: &[FieldValueAssignmentExpectation],
    available_fields: impl IntoIterator<Item = &'a FieldInstance>,
    assignments: &[FieldValueAssignment],
) -> Result<(), EvaluationError> {
    if assignments.is_empty() && expected_assignments.is_empty() {
        return Ok(());
    }
    if !phase.is_field_event() {
        return Err(EvaluationError::AssignmentsRequireEventPhase { phase });
    }

    let available_fields: Vec<_> = available_fields.into_iter().collect();
    for expected in expected_assignments {
        if !available_fields.contains(&expected.field()) {
            return Err(EvaluationError::AssignmentTargetMissing {
                field: expected.field().clone(),
            });
        }
    }
    let mut previous_position = None;
    let mut seen_effects = Vec::new();
    for assignment in assignments {
        expected_rules
            .iter()
            .position(|expected| expected.execution() == assignment.execution())
            .ok_or_else(|| EvaluationError::AssignmentForUnknownRule {
                execution: assignment.execution().clone(),
            })?;
        if seen_effects.contains(&(assignment.execution().clone(), assignment.effect_index())) {
            return Err(EvaluationError::DuplicateAssignmentEffect {
                execution: assignment.execution().clone(),
                effect_index: assignment.effect_index(),
            });
        }
        let expected_position = expected_assignments
            .iter()
            .position(|expected| expected == &assignment.expectation())
            .ok_or_else(|| EvaluationError::AssignmentDoesNotMatchEffect {
                execution: assignment.execution().clone(),
                effect_index: assignment.effect_index(),
            })?;
        if previous_position.is_some_and(|previous| previous >= expected_position) {
            return Err(EvaluationError::AssignmentsOutOfOrder {
                execution: assignment.execution().clone(),
                effect_index: assignment.effect_index(),
            });
        }
        if !available_fields.contains(&assignment.field()) {
            return Err(EvaluationError::AssignmentTargetMissing {
                field: assignment.field().clone(),
            });
        }
        seen_effects.push((assignment.execution().clone(), assignment.effect_index()));
        previous_position = Some(expected_position);
    }
    Ok(())
}

fn validate_canonical_inputs(
    raw_inputs: &RawInputSnapshot,
    canonical_inputs: &[CanonicalFieldValue],
) -> Result<(), EvaluationError> {
    let expected_fields: Vec<_> = raw_inputs
        .fields()
        .iter()
        .map(|item| item.field().clone())
        .collect();
    let actual_fields: Vec<_> = canonical_inputs
        .iter()
        .map(|item| item.field().clone())
        .collect();
    if actual_fields != expected_fields {
        return Err(EvaluationError::CanonicalInputCoverage {
            expected: expected_fields,
            actual: actual_fields,
        });
    }

    for (raw, canonical) in raw_inputs.fields().iter().zip(canonical_inputs) {
        if raw.value() != canonical.raw() {
            return Err(EvaluationError::CanonicalRawValueMismatch {
                field: raw.field().clone(),
            });
        }
    }
    Ok(())
}

fn validate_calculation_coverage(
    expected: &[DerivedOutputExpectation],
    derived: &[DerivedValue],
) -> Result<(), EvaluationError> {
    let evaluated: Vec<_> = derived.iter().map(DerivedValue::expectation).collect();
    if evaluated != expected {
        return Err(EvaluationError::CalculationCoverage {
            expected: expected.to_vec(),
            evaluated,
        });
    }
    Ok(())
}

/// Fail-closed evaluator error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvaluationError {
    InvalidContextSnapshot(ContextSnapshotError),
    InvalidInputSnapshot(InputSnapshotError),
    ContextFingerprintMismatch {
        supplied: ContextFingerprint,
        computed: ContextFingerprint,
    },
    MissingEventField {
        phase: ValidationPhase,
    },
    UnexpectedEventField {
        phase: ValidationPhase,
        field: FieldInstance,
    },
    EventFieldMissingFromSnapshot {
        field: FieldInstance,
    },
    RuleSetMismatch {
        expected: FormRevisionKey,
        requested: FormRevisionKey,
    },
    UnsupportedContext {
        context: ValidationContext,
    },
    DuplicateExpectedRule {
        execution: RuleExecution,
    },
    ExpectedRuleOrderNotStrict {
        previous: u32,
        current: u32,
    },
    ExpectedRuleExecutionOrderNotStrict {
        order: u32,
        previous: RuleExecution,
        current: RuleExecution,
    },
    DuplicateExpectedOutput {
        calculation_id: CalculationId,
        output_id: OutputId,
        instance: Option<RepeatedGroupInstance>,
    },
    ExpectedOutputInstanceOrderNotStrict {
        calculation_id: CalculationId,
        output_id: OutputId,
        previous: Option<RepeatedGroupInstance>,
        current: Option<RepeatedGroupInstance>,
    },
    MissingInput {
        field: FieldInstance,
    },
    InvalidRawValue {
        field: FieldInstance,
        reason: String,
    },
    UnresolvedRule {
        rule_id: RuleId,
    },
    CalculationFailed {
        calculation_id: CalculationId,
        reason: String,
    },
    CanonicalInputCoverage {
        expected: Vec<FieldInstance>,
        actual: Vec<FieldInstance>,
    },
    CanonicalRawValueMismatch {
        field: FieldInstance,
    },
    CanonicalInputsOutOfOrder {
        previous: FieldInstance,
        current: FieldInstance,
    },
    CalculationCoverage {
        expected: Vec<DerivedOutputExpectation>,
        evaluated: Vec<DerivedOutputExpectation>,
    },
    AssignmentsRequireEventPhase {
        phase: ValidationPhase,
    },
    AssignmentForUnknownRule {
        execution: RuleExecution,
    },
    DuplicateAssignmentEffect {
        execution: RuleExecution,
        effect_index: u32,
    },
    AssignmentDoesNotMatchEffect {
        execution: RuleExecution,
        effect_index: u32,
    },
    AssignmentsOutOfOrder {
        execution: RuleExecution,
        effect_index: u32,
    },
    AssignmentTargetMissing {
        field: FieldInstance,
    },
    InvalidReport(ReportError),
    Interpreter(crate::static_ir::InterpreterError),
    InternalInvariant {
        message: String,
    },
}

impl fmt::Display for EvaluationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidContextSnapshot(error) => {
                write!(formatter, "invalid context snapshot: {error}")
            }
            Self::InvalidInputSnapshot(error) => {
                write!(formatter, "invalid input snapshot: {error}")
            }
            Self::ContextFingerprintMismatch { supplied, computed } => write!(
                formatter,
                "serialized context fingerprint {} does not match computed fingerprint {}",
                supplied.digest(),
                computed.digest()
            ),
            Self::MissingEventField { phase } => write!(
                formatter,
                "field-event phase {phase:?} requires an exact event field"
            ),
            Self::UnexpectedEventField { phase, field } => write!(
                formatter,
                "non-event phase {phase:?} must not carry event field {}",
                field.field_id()
            ),
            Self::EventFieldMissingFromSnapshot { field } => write!(
                formatter,
                "event field {} does not occur in the raw input snapshot",
                field.field_id()
            ),
            Self::RuleSetMismatch {
                expected,
                requested,
            } => write!(
                formatter,
                "evaluation requested rule set {}, but evaluator is {}",
                requested.rule_set_id(),
                expected.rule_set_id()
            ),
            Self::UnsupportedContext { context } => {
                write!(
                    formatter,
                    "phase/profile branch is not executable: {context:?}"
                )
            }
            Self::DuplicateExpectedRule { execution } => {
                write!(
                    formatter,
                    "duplicate expected rule execution {}",
                    format_rule_execution(execution)
                )
            }
            Self::ExpectedRuleOrderNotStrict { previous, current } => write!(
                formatter,
                "expected rule order must be strictly increasing, got {current} after {previous}"
            ),
            Self::ExpectedRuleExecutionOrderNotStrict {
                order,
                previous,
                current,
            } => write!(
                formatter,
                "rule executions at order {order} are not in stable identity order: {} follows {}",
                format_rule_execution(current),
                format_rule_execution(previous)
            ),
            Self::DuplicateExpectedOutput {
                calculation_id,
                output_id,
                instance,
            } => match instance {
                Some(instance) => write!(
                    formatter,
                    "duplicate expected calculation output {calculation_id}:{output_id}@{}:{}",
                    instance.group_id(),
                    instance.instance_id()
                ),
                None => write!(
                    formatter,
                    "duplicate expected calculation output {calculation_id}:{output_id}@singleton"
                ),
            },
            Self::ExpectedOutputInstanceOrderNotStrict {
                calculation_id,
                output_id,
                previous,
                current,
            } => write!(
                formatter,
                "calculation output {calculation_id}:{output_id} instances are not in stable identity order: {} follows {}",
                format_optional_instance(current.as_ref()),
                format_optional_instance(previous.as_ref())
            ),
            Self::MissingInput { field } => {
                write!(formatter, "required input {} is missing", field.field_id())
            }
            Self::InvalidRawValue { field, reason } => {
                write!(
                    formatter,
                    "invalid raw value for {}: {reason}",
                    field.field_id()
                )
            }
            Self::UnresolvedRule { rule_id } => {
                write!(formatter, "unresolved rule {rule_id} cannot execute")
            }
            Self::CalculationFailed {
                calculation_id,
                reason,
            } => write!(formatter, "calculation {calculation_id} failed: {reason}"),
            Self::CanonicalInputCoverage { expected, actual } => write!(
                formatter,
                "canonical input coverage mismatch: expected {} field occurrences, got {}",
                expected.len(),
                actual.len()
            ),
            Self::CanonicalRawValueMismatch { field } => write!(
                formatter,
                "canonical record for {} does not echo the exact raw value",
                field.field_id()
            ),
            Self::CanonicalInputsOutOfOrder { previous, current } => write!(
                formatter,
                "canonical inputs are not in stable order: {} follows {}",
                current.field_id(),
                previous.field_id()
            ),
            Self::CalculationCoverage {
                expected,
                evaluated,
            } => write!(
                formatter,
                "calculation coverage mismatch: expected {} outputs, got {}",
                expected.len(),
                evaluated.len()
            ),
            Self::AssignmentsRequireEventPhase { phase } => write!(
                formatter,
                "raw field-value assignments are not permitted in non-event phase {phase:?}"
            ),
            Self::AssignmentForUnknownRule { execution } => write!(
                formatter,
                "raw field-value assignment refers to unknown rule execution {}",
                format_rule_execution(execution)
            ),
            Self::DuplicateAssignmentEffect {
                execution,
                effect_index,
            } => write!(
                formatter,
                "raw field-value assignment repeats effect {effect_index} for {}",
                format_rule_execution(execution)
            ),
            Self::AssignmentDoesNotMatchEffect {
                execution,
                effect_index,
            } => write!(
                formatter,
                "raw field-value assignment does not match compiled effect {effect_index} for {}",
                format_rule_execution(execution)
            ),
            Self::AssignmentsOutOfOrder {
                execution,
                effect_index,
            } => write!(
                formatter,
                "raw field-value assignment effect {effect_index} for {} is out of order",
                format_rule_execution(execution)
            ),
            Self::AssignmentTargetMissing { field } => write!(
                formatter,
                "raw field-value assignment target {} is missing from the request",
                field.field_id()
            ),
            Self::InvalidReport(error) => write!(formatter, "invalid validation report: {error}"),
            Self::Interpreter(error) => write!(formatter, "static rule execution failed: {error}"),
            Self::InternalInvariant { message } => {
                write!(formatter, "compiled rule-set invariant failed: {message}")
            }
        }
    }
}

fn format_rule_execution(execution: &RuleExecution) -> String {
    match execution.instance() {
        Some(instance) => format!(
            "{}@{}:{}",
            execution.rule_id(),
            instance.group_id(),
            instance.instance_id()
        ),
        None => format!("{}@singleton", execution.rule_id()),
    }
}

fn format_optional_instance(instance: Option<&RepeatedGroupInstance>) -> String {
    match instance {
        Some(instance) => format!("{}:{}", instance.group_id(), instance.instance_id()),
        None => "singleton".to_owned(),
    }
}

impl Error for EvaluationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidContextSnapshot(error) => Some(error),
            Self::InvalidInputSnapshot(error) => Some(error),
            Self::InvalidReport(error) => Some(error),
            Self::Interpreter(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BehaviorProfile, ContextValueId, ExactDecimal, FieldId, FormCode, FormRevision,
        OfficialPackageVersion, OutputId, RawValue, RepeatedGroupId, RuleAssessment, RuleSetId,
        RuleSeverity, Sha256Digest, StableInstanceId, ValidationPhase,
    };

    fn identity() -> FormRevisionKey {
        FormRevisionKey::new(
            RuleSetId::parse("test-v1-p1").unwrap(),
            FormCode::parse("TEST").unwrap(),
            FormRevision::parse("v1").unwrap(),
            OfficialPackageVersion::parse("p1").unwrap(),
            Sha256Digest::from_bytes([7; 32]),
        )
    }

    fn request() -> EvaluationRequest {
        EvaluationRequest::try_new(
            identity(),
            ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe),
            InputRevision::new(12),
            Vec::new(),
            Vec::new(),
            vec![RawFieldValue::new(
                FieldInstance::singleton(FieldId::parse("amount").unwrap()),
                RawValue::Text("1.2300".into()),
            )],
        )
        .unwrap()
    }

    fn row(instance_id: &str) -> RepeatedGroupInstance {
        RepeatedGroupInstance::new(
            RepeatedGroupId::parse("schedule").unwrap(),
            StableInstanceId::parse(instance_id).unwrap(),
        )
    }

    #[test]
    fn request_rejects_duplicate_context_value_ids() {
        let id = ContextValueId::parse("vat-rate").unwrap();
        let result = EvaluationRequest::try_new(
            identity(),
            ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe),
            InputRevision::new(12),
            vec![
                ContextValue::new(id.clone(), CanonicalValue::Decimal("0.12".parse().unwrap())),
                ContextValue::new(id, CanonicalValue::Decimal("0.10".parse().unwrap())),
            ],
            Vec::new(),
            Vec::new(),
        );

        assert!(matches!(
            result,
            Err(EvaluationError::InvalidContextSnapshot(
                ContextSnapshotError::DuplicateId { .. }
            ))
        ));
    }

    fn expectation() -> EvaluationExpectation {
        EvaluationExpectation::try_new(
            vec![RuleExpectation::singleton(
                RuleId::parse("amount-positive").unwrap(),
                10,
            )],
            vec![DerivedOutputExpectation::singleton(
                CalculationId::parse("tax-due").unwrap(),
                OutputId::parse("total").unwrap(),
            )],
        )
        .unwrap()
    }

    fn output(request: &EvaluationRequest) -> EvaluationOutput {
        let raw = &request.raw_inputs().fields()[0];
        EvaluationOutput::new(
            vec![CanonicalFieldValue::new(
                raw.field().clone(),
                raw.value().clone(),
                CanonicalValue::Decimal("1.2300".parse::<ExactDecimal>().unwrap()),
            )],
            vec![DerivedValue::singleton(
                CalculationId::parse("tax-due").unwrap(),
                OutputId::parse("total").unwrap(),
                CanonicalValue::Decimal("0.12".parse().unwrap()),
            )],
            vec![RuleExecution::singleton(
                RuleId::parse("amount-positive").unwrap(),
            )],
            Vec::new(),
        )
    }

    #[test]
    fn result_binds_raw_canonical_calculation_and_revision_metadata() {
        let request = request();
        let result = EvaluationResult::try_new(&request, &expectation(), output(&request)).unwrap();

        assert_eq!(result.input_revision(), InputRevision::new(12));
        assert_eq!(
            result.context_fingerprint(),
            request.context_values().fingerprint()
        );
        assert_eq!(result.canonical_inputs().len(), 1);
        assert_eq!(result.derived_outputs().len(), 1);
        assert!(result.report().is_complete());
        assert!(result.is_valid());
    }

    #[test]
    fn request_deserialization_rejects_a_caller_asserted_context_fingerprint() {
        let request = request();
        let mut wire = serde_json::to_value(&request).unwrap();
        let round_trip: EvaluationRequest = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(round_trip, request);

        wire["context_fingerprint"] =
            serde_json::Value::String(Sha256Digest::from_bytes([9; 32]).to_hex());

        let error = serde_json::from_value::<EvaluationRequest>(wire).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("does not match computed fingerprint")
        );
    }

    #[test]
    fn field_event_requests_require_an_exact_snapshotted_field_and_preserve_it_on_wire() {
        let context = ValidationContext::new(ValidationPhase::Blur, BehaviorProfile::FilingSafe);
        let amount = FieldInstance::singleton(FieldId::parse("amount").unwrap());
        let raw_fields = vec![RawFieldValue::new(
            amount.clone(),
            RawValue::Text("1.2300".into()),
        )];

        assert!(matches!(
            EvaluationRequest::try_new(
                identity(),
                context,
                InputRevision::new(12),
                Vec::new(),
                Vec::new(),
                raw_fields.clone(),
            ),
            Err(EvaluationError::MissingEventField {
                phase: ValidationPhase::Blur
            })
        ));
        assert!(matches!(
            EvaluationRequest::try_new_for_field_event(
                identity(),
                context,
                InputRevision::new(12),
                FieldInstance::singleton(FieldId::parse("other").unwrap()),
                Vec::new(),
                Vec::new(),
                raw_fields.clone(),
            ),
            Err(EvaluationError::EventFieldMissingFromSnapshot { .. })
        ));
        assert!(matches!(
            EvaluationRequest::try_new_for_field_event(
                identity(),
                ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe),
                InputRevision::new(12),
                amount.clone(),
                Vec::new(),
                Vec::new(),
                raw_fields.clone(),
            ),
            Err(EvaluationError::UnexpectedEventField {
                phase: ValidationPhase::Validate,
                ..
            })
        ));

        let event_request = EvaluationRequest::try_new_for_field_event(
            identity(),
            context,
            InputRevision::new(12),
            amount.clone(),
            Vec::new(),
            Vec::new(),
            raw_fields,
        )
        .unwrap();
        assert_eq!(event_request.event_field(), Some(&amount));
        let mut wire = serde_json::to_value(&event_request).unwrap();
        assert_eq!(wire["event_field"]["field_id"], "amount");
        assert_eq!(
            serde_json::from_value::<EvaluationRequest>(wire.clone()).unwrap(),
            event_request
        );
        wire.as_object_mut().unwrap().remove("event_field");
        assert!(
            serde_json::from_value::<EvaluationRequest>(wire)
                .unwrap_err()
                .to_string()
                .contains("requires an exact event field")
        );

        assert!(
            serde_json::to_value(request())
                .unwrap()
                .get("event_field")
                .is_none()
        );
    }

    #[test]
    fn serialized_request_and_result_reject_unknown_properties() {
        let request = request();
        let mut request_wire = serde_json::to_value(&request).unwrap();
        request_wire["unexpected"] = serde_json::Value::Bool(true);
        assert!(
            serde_json::from_value::<EvaluationRequest>(request_wire)
                .unwrap_err()
                .to_string()
                .contains("unknown field")
        );

        let result = EvaluationResult::try_new(&request, &expectation(), output(&request)).unwrap();
        let mut result_wire = serde_json::to_value(result).unwrap();
        result_wire["unexpected"] = serde_json::Value::Bool(true);
        assert!(
            serde_json::from_value::<EvaluationResult>(result_wire)
                .unwrap_err()
                .to_string()
                .contains("unknown field")
        );
    }

    #[test]
    fn result_rejects_omitted_calculation_or_canonical_input() {
        let request = request();
        let mut missing_calculation = output(&request);
        missing_calculation.derived_outputs.clear();
        assert!(matches!(
            EvaluationResult::try_new(&request, &expectation(), missing_calculation),
            Err(EvaluationError::CalculationCoverage { .. })
        ));

        let mut wrong_output = output(&request);
        wrong_output.derived_outputs[0].output_id = OutputId::parse("different-output").unwrap();
        assert!(matches!(
            EvaluationResult::try_new(&request, &expectation(), wrong_output),
            Err(EvaluationError::CalculationCoverage { .. })
        ));

        let mut missing_input = output(&request);
        missing_input.canonical_inputs.clear();
        assert!(matches!(
            EvaluationResult::try_new(&request, &expectation(), missing_input),
            Err(EvaluationError::CanonicalInputCoverage { .. })
        ));
    }

    #[test]
    fn result_rejects_wrong_issue_order_instead_of_sorting_it() {
        let request = request();
        let expectation = EvaluationExpectation::try_new(
            vec![
                RuleExpectation::singleton(RuleId::parse("first").unwrap(), 10),
                RuleExpectation::singleton(RuleId::parse("second").unwrap(), 20),
            ],
            vec![DerivedOutputExpectation::singleton(
                CalculationId::parse("tax-due").unwrap(),
                OutputId::parse("total").unwrap(),
            )],
        )
        .unwrap();
        let mut output = output(&request);
        output.evaluated_rules = vec![
            RuleExecution::singleton(RuleId::parse("first").unwrap()),
            RuleExecution::singleton(RuleId::parse("second").unwrap()),
        ];
        output.violations = vec![
            RuleViolation::singleton(
                RuleId::parse("second").unwrap(),
                ValidationPhase::Validate,
                crate::IssueOrder::new(20, 0),
                Vec::new(),
                None,
                "second".into(),
                RuleAssessment::VerifiedCorrect,
                RuleSeverity::Blocking,
                BehaviorProfile::FilingSafe,
            ),
            RuleViolation::singleton(
                RuleId::parse("first").unwrap(),
                ValidationPhase::Validate,
                crate::IssueOrder::new(10, 0),
                Vec::new(),
                None,
                "first".into(),
                RuleAssessment::VerifiedCorrect,
                RuleSeverity::Blocking,
                BehaviorProfile::FilingSafe,
            ),
        ];

        assert!(matches!(
            EvaluationResult::try_new(&request, &expectation, output),
            Err(EvaluationError::InvalidReport(
                ReportError::IssuesOutOfOrder { .. }
            ))
        ));
    }

    #[test]
    fn group_calculation_coverage_preserves_two_stable_row_instances() {
        let request = request();
        let row_a = row("row-a");
        let row_b = row("row-b");
        let expectation = EvaluationExpectation::try_new(
            vec![RuleExpectation::singleton(
                RuleId::parse("amount-positive").unwrap(),
                10,
            )],
            vec![
                DerivedOutputExpectation::new(
                    CalculationId::parse("tax-due").unwrap(),
                    OutputId::parse("total").unwrap(),
                    Some(row_a.clone()),
                ),
                DerivedOutputExpectation::new(
                    CalculationId::parse("tax-due").unwrap(),
                    OutputId::parse("total").unwrap(),
                    Some(row_b.clone()),
                ),
            ],
        )
        .unwrap();
        let mut output = output(&request);
        output.derived_outputs = vec![
            DerivedValue::new(
                CalculationId::parse("tax-due").unwrap(),
                OutputId::parse("total").unwrap(),
                Some(row_a.clone()),
                CanonicalValue::Decimal("0.12".parse().unwrap()),
            ),
            DerivedValue::new(
                CalculationId::parse("tax-due").unwrap(),
                OutputId::parse("total").unwrap(),
                Some(row_b.clone()),
                CanonicalValue::Decimal("0.24".parse().unwrap()),
            ),
        ];

        let result = EvaluationResult::try_new(&request, &expectation, output).unwrap();

        assert_eq!(result.expected_outputs()[0].instance(), Some(&row_a));
        assert_eq!(result.derived_outputs()[1].instance(), Some(&row_b));
        let wire = serde_json::to_value(&result).unwrap();
        assert_eq!(
            wire["derived_outputs"][0]["instance"]["instance_id"],
            "row-a"
        );
        assert_eq!(
            serde_json::from_value::<EvaluationResult>(wire.clone()).unwrap(),
            result
        );

        let mut reversed_wire = wire;
        reversed_wire["expected_outputs"]
            .as_array_mut()
            .unwrap()
            .swap(0, 1);
        reversed_wire["derived_outputs"]
            .as_array_mut()
            .unwrap()
            .swap(0, 1);
        assert!(
            serde_json::from_value::<EvaluationResult>(reversed_wire)
                .unwrap_err()
                .to_string()
                .contains("not in stable identity order")
        );
    }

    #[test]
    fn group_calculation_coverage_rejects_duplicate_missing_and_wrong_instances() {
        let request = request();
        let row_a = row("row-a");
        let row_b = row("row-b");
        let row_c = row("row-c");
        let expectation = EvaluationExpectation::try_new(
            Vec::new(),
            vec![
                DerivedOutputExpectation::new(
                    CalculationId::parse("tax-due").unwrap(),
                    OutputId::parse("total").unwrap(),
                    Some(row_a.clone()),
                ),
                DerivedOutputExpectation::new(
                    CalculationId::parse("tax-due").unwrap(),
                    OutputId::parse("total").unwrap(),
                    Some(row_b.clone()),
                ),
            ],
        )
        .unwrap();
        let value_for = |instance: RepeatedGroupInstance| {
            DerivedValue::new(
                CalculationId::parse("tax-due").unwrap(),
                OutputId::parse("total").unwrap(),
                Some(instance),
                CanonicalValue::Integer(1),
            )
        };
        let output_with = |derived_outputs| {
            let mut output = output(&request);
            output.derived_outputs = derived_outputs;
            output.evaluated_rules.clear();
            output
        };

        for derived_outputs in [
            vec![value_for(row_a.clone()), value_for(row_a.clone())],
            vec![value_for(row_a.clone())],
            vec![value_for(row_a.clone()), value_for(row_c)],
        ] {
            assert!(matches!(
                EvaluationResult::try_new(&request, &expectation, output_with(derived_outputs)),
                Err(EvaluationError::CalculationCoverage { .. })
            ));
        }

        let duplicate_expected = EvaluationExpectation::try_new(
            Vec::new(),
            vec![
                DerivedOutputExpectation::new(
                    CalculationId::parse("tax-due").unwrap(),
                    OutputId::parse("total").unwrap(),
                    Some(row_b.clone()),
                ),
                DerivedOutputExpectation::new(
                    CalculationId::parse("tax-due").unwrap(),
                    OutputId::parse("total").unwrap(),
                    Some(row_b),
                ),
            ],
        );
        assert!(matches!(
            duplicate_expected,
            Err(EvaluationError::DuplicateExpectedOutput { .. })
        ));
    }

    #[test]
    fn group_expectation_rejects_unstable_instance_order_and_serializes_singleton_explicitly() {
        let row_a = row("row-a");
        let row_b = row("row-b");
        let reversed = EvaluationExpectation::try_new(
            Vec::new(),
            vec![
                DerivedOutputExpectation::new(
                    CalculationId::parse("tax-due").unwrap(),
                    OutputId::parse("total").unwrap(),
                    Some(row_b),
                ),
                DerivedOutputExpectation::new(
                    CalculationId::parse("tax-due").unwrap(),
                    OutputId::parse("total").unwrap(),
                    Some(row_a),
                ),
            ],
        );
        assert!(matches!(
            reversed,
            Err(EvaluationError::ExpectedOutputInstanceOrderNotStrict { .. })
        ));

        let expected = DerivedOutputExpectation::singleton(
            CalculationId::parse("tax-due").unwrap(),
            OutputId::parse("total").unwrap(),
        );
        let value = DerivedValue::singleton(
            CalculationId::parse("tax-due").unwrap(),
            OutputId::parse("total").unwrap(),
            CanonicalValue::Integer(1),
        );
        assert_eq!(
            serde_json::to_value(expected).unwrap()["instance"],
            serde_json::Value::Null
        );
        assert_eq!(
            serde_json::to_value(value).unwrap()["instance"],
            serde_json::Value::Null
        );
    }
}
