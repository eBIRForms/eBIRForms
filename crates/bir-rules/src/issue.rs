use crate::{
    BehaviorProfile, ContextFingerprint, FieldInstance, FormRevisionKey, InputRevision,
    RepeatedGroupInstance, RuleId, ValidationContext, ValidationPhase, XmlKey,
};
use serde::{Deserialize, Deserializer, Serialize, de};
use std::{error::Error, fmt, num::NonZeroU16};

/// Review classification preserved from the extracted rule corpus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuleAssessment {
    VerifiedCorrect,
    OfficialBugCompatible,
    IncorrectOfficialBehavior,
    Ambiguous,
    Unverified,
    Obsolete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuleSeverity {
    Advisory,
    Blocking,
}

/// One-based occurrence of an official serialized key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SerializedOccurrence(NonZeroU16);

impl SerializedOccurrence {
    pub fn new(value: u16) -> Option<Self> {
        NonZeroU16::new(value).map(Self)
    }

    pub const fn get(self) -> u16 {
        self.0.get()
    }
}

/// Connects a stable semantic field occurrence to official serialization.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct RuleFieldRef {
    field: FieldInstance,
    xml_key: Option<XmlKey>,
    serialized_occurrence: Option<SerializedOccurrence>,
}

impl RuleFieldRef {
    pub fn try_new(
        field: FieldInstance,
        xml_key: Option<XmlKey>,
        serialized_occurrence: Option<SerializedOccurrence>,
    ) -> Result<Self, RuleFieldRefError> {
        if serialized_occurrence.is_some() && xml_key.is_none() {
            return Err(RuleFieldRefError::OccurrenceWithoutXmlKey);
        }
        Ok(Self {
            field,
            xml_key,
            serialized_occurrence,
        })
    }

    pub fn semantic(field: FieldInstance) -> Self {
        Self {
            field,
            xml_key: None,
            serialized_occurrence: None,
        }
    }

    pub fn field(&self) -> &FieldInstance {
        &self.field
    }

    pub fn xml_key(&self) -> Option<&XmlKey> {
        self.xml_key.as_ref()
    }

    pub const fn serialized_occurrence(&self) -> Option<SerializedOccurrence> {
        self.serialized_occurrence
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleFieldRefError {
    OccurrenceWithoutXmlKey,
}

impl fmt::Display for RuleFieldRefError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OccurrenceWithoutXmlKey => {
                formatter.write_str("serialized occurrence requires an exact XML key")
            }
        }
    }
}

impl Error for RuleFieldRefError {}

impl<'de> Deserialize<'de> for RuleFieldRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            field: FieldInstance,
            xml_key: Option<XmlKey>,
            serialized_occurrence: Option<SerializedOccurrence>,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.field, wire.xml_key, wire.serialized_occurrence)
            .map_err(de::Error::custom)
    }
}

/// Total issue position within a validation phase.
///
/// `rule_order` is the reviewed compiled order. `occurrence` orders multiple
/// issues emitted by the same rule (for example repeated schedule rows).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssueOrder {
    rule_order: u32,
    occurrence: u32,
}

impl IssueOrder {
    pub const fn new(rule_order: u32, occurrence: u32) -> Self {
        Self {
            rule_order,
            occurrence,
        }
    }

    pub const fn rule_order(self) -> u32 {
        self.rule_order
    }

    pub const fn occurrence(self) -> u32 {
        self.occurrence
    }
}

/// Exact identity of one execution of a compiled validation rule.
///
/// Singleton rules carry an explicit `None`; group-scoped rules carry the
/// persisted repeated-group instance they evaluated. The instance is part of
/// coverage identity, so executing a rule for the wrong row cannot satisfy the
/// expected inventory.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleExecution {
    rule_id: RuleId,
    instance: Option<RepeatedGroupInstance>,
}

impl RuleExecution {
    pub const fn new(rule_id: RuleId, instance: Option<RepeatedGroupInstance>) -> Self {
        Self { rule_id, instance }
    }

    pub const fn singleton(rule_id: RuleId) -> Self {
        Self::new(rule_id, None)
    }

    pub fn rule_id(&self) -> &RuleId {
        &self.rule_id
    }

    pub fn instance(&self) -> Option<&RepeatedGroupInstance> {
        self.instance.as_ref()
    }
}

/// One ordered issue from a compiled validation rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleViolation {
    execution: RuleExecution,
    phase: ValidationPhase,
    order: IssueOrder,
    fields: Vec<RuleFieldRef>,
    /// Exact official message, retained even if filing-safe presentation
    /// selects a separately reviewed message.
    official_message: Option<String>,
    message: String,
    assessment: RuleAssessment,
    severity: RuleSeverity,
    profile: BehaviorProfile,
}

impl RuleViolation {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        rule_id: RuleId,
        instance: Option<RepeatedGroupInstance>,
        phase: ValidationPhase,
        order: IssueOrder,
        fields: Vec<RuleFieldRef>,
        official_message: Option<String>,
        message: String,
        assessment: RuleAssessment,
        severity: RuleSeverity,
        profile: BehaviorProfile,
    ) -> Self {
        Self {
            execution: RuleExecution::new(rule_id, instance),
            phase,
            order,
            fields,
            official_message,
            message,
            assessment,
            severity,
            profile,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn singleton(
        rule_id: RuleId,
        phase: ValidationPhase,
        order: IssueOrder,
        fields: Vec<RuleFieldRef>,
        official_message: Option<String>,
        message: String,
        assessment: RuleAssessment,
        severity: RuleSeverity,
        profile: BehaviorProfile,
    ) -> Self {
        Self::new(
            rule_id,
            None,
            phase,
            order,
            fields,
            official_message,
            message,
            assessment,
            severity,
            profile,
        )
    }

    pub fn execution(&self) -> &RuleExecution {
        &self.execution
    }

    pub fn rule_id(&self) -> &RuleId {
        self.execution.rule_id()
    }

    pub fn instance(&self) -> Option<&RepeatedGroupInstance> {
        self.execution.instance()
    }

    pub const fn phase(&self) -> ValidationPhase {
        self.phase
    }

    pub const fn order(&self) -> IssueOrder {
        self.order
    }

    pub fn fields(&self) -> &[RuleFieldRef] {
        &self.fields
    }

    pub fn official_message(&self) -> Option<&str> {
        self.official_message.as_deref()
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub const fn assessment(&self) -> RuleAssessment {
        self.assessment
    }

    pub const fn severity(&self) -> RuleSeverity {
        self.severity
    }

    pub const fn profile(&self) -> BehaviorProfile {
        self.profile
    }
}

/// Reviewed position of one applicable executable rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleExpectation {
    execution: RuleExecution,
    order: u32,
}

impl RuleExpectation {
    pub const fn new(rule_id: RuleId, instance: Option<RepeatedGroupInstance>, order: u32) -> Self {
        Self {
            execution: RuleExecution::new(rule_id, instance),
            order,
        }
    }

    pub const fn singleton(rule_id: RuleId, order: u32) -> Self {
        Self::new(rule_id, None, order)
    }

    pub fn execution(&self) -> &RuleExecution {
        &self.execution
    }

    pub fn rule_id(&self) -> &RuleId {
        self.execution.rule_id()
    }

    pub fn instance(&self) -> Option<&RepeatedGroupInstance> {
        self.execution.instance()
    }

    pub const fn order(&self) -> u32 {
        self.order
    }
}

/// Why a validation report was rejected as incomplete or nondeterministic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportError {
    MissingEventField {
        phase: ValidationPhase,
    },
    UnexpectedEventField {
        phase: ValidationPhase,
        field: FieldInstance,
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
    IncompleteRuleCoverage {
        expected: Vec<RuleExecution>,
        evaluated: Vec<RuleExecution>,
    },
    IssueForUnknownRule {
        execution: RuleExecution,
    },
    IssueContextMismatch {
        execution: RuleExecution,
    },
    IssueRuleOrderMismatch {
        execution: RuleExecution,
        expected: u32,
        actual: u32,
    },
    IssuesOutOfOrder {
        previous: IssueOrder,
        current: IssueOrder,
    },
    EmptyIssueMessage {
        execution: RuleExecution,
    },
}

impl fmt::Display for ReportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingEventField { phase } => write!(
                formatter,
                "field-event validation report for {phase:?} is missing its event field"
            ),
            Self::UnexpectedEventField { phase, field } => write!(
                formatter,
                "non-event validation report for {phase:?} carries event field {}",
                field.field_id()
            ),
            Self::DuplicateExpectedRule { execution } => {
                write!(
                    formatter,
                    "rule expectation contains duplicate {}",
                    format_execution(execution)
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
                format_execution(current),
                format_execution(previous)
            ),
            Self::IncompleteRuleCoverage {
                expected,
                evaluated,
            } => write!(
                formatter,
                "rule coverage is incomplete: expected {} rules, evaluated {}",
                expected.len(),
                evaluated.len()
            ),
            Self::IssueForUnknownRule { execution } => {
                write!(
                    formatter,
                    "issue refers to non-applicable rule execution {}",
                    format_execution(execution)
                )
            }
            Self::IssueContextMismatch { execution } => write!(
                formatter,
                "issue for rule execution {} does not match the report phase/profile",
                format_execution(execution)
            ),
            Self::IssueRuleOrderMismatch {
                execution,
                expected,
                actual,
            } => write!(
                formatter,
                "issue for rule execution {} has order {actual}, expected {expected}",
                format_execution(execution)
            ),
            Self::IssuesOutOfOrder { previous, current } => write!(
                formatter,
                "issues are not in strict deterministic order: {current:?} follows {previous:?}"
            ),
            Self::EmptyIssueMessage { execution } => {
                write!(
                    formatter,
                    "issue for rule execution {} has an empty selected message",
                    format_execution(execution)
                )
            }
        }
    }
}

fn format_execution(execution: &RuleExecution) -> String {
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

impl Error for ReportError {}

/// Complete, deterministically ordered validation report.
///
/// Both inventories are retained in serialized evidence even though a valid
/// instance requires them to match exactly. That makes omission visible to
/// audit consumers instead of equating "no issue was emitted" with "the rule
/// was evaluated." Coverage compares exact rule-plus-instance identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValidationReport {
    rule_set: FormRevisionKey,
    context: ValidationContext,
    input_revision: InputRevision,
    #[serde(skip_serializing_if = "Option::is_none")]
    event_field: Option<FieldInstance>,
    context_fingerprint: ContextFingerprint,
    expected_rules: Vec<RuleExpectation>,
    evaluated_rules: Vec<RuleExecution>,
    violations: Vec<RuleViolation>,
}

impl ValidationReport {
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        rule_set: FormRevisionKey,
        context: ValidationContext,
        input_revision: InputRevision,
        context_fingerprint: ContextFingerprint,
        expected_rules: Vec<RuleExpectation>,
        evaluated_rules: Vec<RuleExecution>,
        violations: Vec<RuleViolation>,
    ) -> Result<Self, ReportError> {
        Self::try_new_inner(
            rule_set,
            context,
            input_revision,
            None,
            context_fingerprint,
            expected_rules,
            evaluated_rules,
            violations,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn try_new_for_field_event(
        rule_set: FormRevisionKey,
        context: ValidationContext,
        input_revision: InputRevision,
        event_field: FieldInstance,
        context_fingerprint: ContextFingerprint,
        expected_rules: Vec<RuleExpectation>,
        evaluated_rules: Vec<RuleExecution>,
        violations: Vec<RuleViolation>,
    ) -> Result<Self, ReportError> {
        Self::try_new_inner(
            rule_set,
            context,
            input_revision,
            Some(event_field),
            context_fingerprint,
            expected_rules,
            evaluated_rules,
            violations,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn try_new_inner(
        rule_set: FormRevisionKey,
        context: ValidationContext,
        input_revision: InputRevision,
        event_field: Option<FieldInstance>,
        context_fingerprint: ContextFingerprint,
        expected_rules: Vec<RuleExpectation>,
        evaluated_rules: Vec<RuleExecution>,
        violations: Vec<RuleViolation>,
    ) -> Result<Self, ReportError> {
        match (context.phase().is_field_event(), event_field.as_ref()) {
            (true, None) => {
                return Err(ReportError::MissingEventField {
                    phase: context.phase(),
                });
            }
            (false, Some(field)) => {
                return Err(ReportError::UnexpectedEventField {
                    phase: context.phase(),
                    field: field.clone(),
                });
            }
            _ => {}
        }
        for (index, expected) in expected_rules.iter().enumerate() {
            if expected_rules[..index]
                .iter()
                .any(|prior| prior.execution == expected.execution)
            {
                return Err(ReportError::DuplicateExpectedRule {
                    execution: expected.execution.clone(),
                });
            }
        }
        for pair in expected_rules.windows(2) {
            if pair[0].order > pair[1].order
                || (pair[0].order == pair[1].order && pair[0].rule_id() != pair[1].rule_id())
            {
                return Err(ReportError::ExpectedRuleOrderNotStrict {
                    previous: pair[0].order,
                    current: pair[1].order,
                });
            }
            if pair[0].order == pair[1].order && pair[0].execution >= pair[1].execution {
                return Err(ReportError::ExpectedRuleExecutionOrderNotStrict {
                    order: pair[0].order,
                    previous: pair[0].execution.clone(),
                    current: pair[1].execution.clone(),
                });
            }
        }

        let expected_executions: Vec<_> = expected_rules
            .iter()
            .map(|item| item.execution.clone())
            .collect();
        if evaluated_rules != expected_executions {
            return Err(ReportError::IncompleteRuleCoverage {
                expected: expected_executions,
                evaluated: evaluated_rules,
            });
        }

        let mut previous_order = None;
        for violation in &violations {
            if violation.message.is_empty() {
                return Err(ReportError::EmptyIssueMessage {
                    execution: violation.execution.clone(),
                });
            }
            if violation.phase != context.phase() || violation.profile != context.profile() {
                return Err(ReportError::IssueContextMismatch {
                    execution: violation.execution.clone(),
                });
            }
            let expected = expected_rules
                .iter()
                .find(|item| item.execution == violation.execution)
                .ok_or_else(|| ReportError::IssueForUnknownRule {
                    execution: violation.execution.clone(),
                })?;
            if violation.order.rule_order != expected.order {
                return Err(ReportError::IssueRuleOrderMismatch {
                    execution: violation.execution.clone(),
                    expected: expected.order,
                    actual: violation.order.rule_order,
                });
            }
            if previous_order.is_some_and(|previous| previous >= violation.order) {
                return Err(ReportError::IssuesOutOfOrder {
                    previous: previous_order.expect("checked as some"),
                    current: violation.order,
                });
            }
            previous_order = Some(violation.order);
        }

        Ok(Self {
            rule_set,
            context,
            input_revision,
            event_field,
            context_fingerprint,
            expected_rules,
            evaluated_rules: expected_executions,
            violations,
        })
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

    pub fn expected_rules(&self) -> &[RuleExpectation] {
        &self.expected_rules
    }

    pub fn evaluated_rules(&self) -> &[RuleExecution] {
        &self.evaluated_rules
    }

    pub fn violations(&self) -> &[RuleViolation] {
        &self.violations
    }

    pub fn is_complete(&self) -> bool {
        self.expected_rules
            .iter()
            .map(RuleExpectation::execution)
            .eq(self.evaluated_rules.iter())
    }

    pub fn is_valid(&self) -> bool {
        self.is_complete()
            && !self
                .violations
                .iter()
                .any(|violation| violation.severity == RuleSeverity::Blocking)
    }

    pub fn first_blocking(&self) -> Option<&RuleViolation> {
        self.violations
            .iter()
            .find(|violation| violation.severity == RuleSeverity::Blocking)
    }
}

impl<'de> Deserialize<'de> for ValidationReport {
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
            expected_rules: Vec<RuleExpectation>,
            evaluated_rules: Vec<RuleExecution>,
            violations: Vec<RuleViolation>,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::try_new_inner(
            wire.rule_set,
            wire.context,
            wire.input_revision,
            wire.event_field,
            wire.context_fingerprint,
            wire.expected_rules,
            wire.evaluated_rules,
            wire.violations,
        )
        .map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        FormCode, FormRevision, OfficialPackageVersion, RepeatedGroupId, RuleSetId, Sha256Digest,
        StableInstanceId,
    };

    fn rule_set() -> FormRevisionKey {
        FormRevisionKey::new(
            RuleSetId::parse("test-v1-p1").unwrap(),
            FormCode::parse("TEST").unwrap(),
            FormRevision::parse("v1").unwrap(),
            OfficialPackageVersion::parse("p1").unwrap(),
            Sha256Digest::from_bytes([0; 32]),
        )
    }

    fn context() -> ValidationContext {
        ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe)
    }

    fn row(instance_id: &str) -> RepeatedGroupInstance {
        RepeatedGroupInstance::new(
            RepeatedGroupId::parse("schedule").unwrap(),
            StableInstanceId::parse(instance_id).unwrap(),
        )
    }

    fn violation(rule_id: &str, order: u32, occurrence: u32) -> RuleViolation {
        RuleViolation::singleton(
            RuleId::parse(rule_id).unwrap(),
            ValidationPhase::Validate,
            IssueOrder::new(order, occurrence),
            Vec::new(),
            None,
            rule_id.to_owned(),
            RuleAssessment::VerifiedCorrect,
            RuleSeverity::Blocking,
            BehaviorProfile::FilingSafe,
        )
    }

    #[test]
    fn report_rejects_missing_rule_execution() {
        let result = ValidationReport::try_new(
            rule_set(),
            context(),
            InputRevision::new(2),
            Sha256Digest::from_bytes([1; 32]).into(),
            vec![
                RuleExpectation::singleton(RuleId::parse("first").unwrap(), 10),
                RuleExpectation::singleton(RuleId::parse("second").unwrap(), 20),
            ],
            vec![RuleExecution::singleton(RuleId::parse("first").unwrap())],
            Vec::new(),
        );

        assert!(matches!(
            result,
            Err(ReportError::IncompleteRuleCoverage { .. })
        ));
    }

    #[test]
    fn report_rejects_implicitly_reordered_issues() {
        let expected = vec![
            RuleExpectation::singleton(RuleId::parse("first").unwrap(), 10),
            RuleExpectation::singleton(RuleId::parse("second").unwrap(), 20),
        ];
        let evaluated = expected
            .iter()
            .map(|item| item.execution().clone())
            .collect();
        let result = ValidationReport::try_new(
            rule_set(),
            context(),
            InputRevision::new(2),
            Sha256Digest::from_bytes([1; 32]).into(),
            expected,
            evaluated,
            vec![violation("second", 20, 0), violation("first", 10, 0)],
        );

        assert!(matches!(result, Err(ReportError::IssuesOutOfOrder { .. })));
    }

    #[test]
    fn complete_advisory_report_remains_non_blocking() {
        let expected = vec![RuleExpectation::singleton(
            RuleId::parse("advisory").unwrap(),
            1,
        )];
        let evaluated = vec![RuleExecution::singleton(RuleId::parse("advisory").unwrap())];
        let mut issue = violation("advisory", 1, 0);
        issue.severity = RuleSeverity::Advisory;
        let report = ValidationReport::try_new(
            rule_set(),
            context(),
            InputRevision::new(1),
            Sha256Digest::from_bytes([2; 32]).into(),
            expected,
            evaluated,
            vec![issue],
        )
        .unwrap();

        assert!(report.is_complete());
        assert!(report.is_valid());
        assert!(report.first_blocking().is_none());
    }

    #[test]
    fn group_rule_coverage_preserves_two_stable_row_instances() {
        let rule_id = RuleId::parse("row-positive").unwrap();
        let row_a = row("row-a");
        let row_b = row("row-b");
        let expected = vec![
            RuleExpectation::new(rule_id.clone(), Some(row_a.clone()), 10),
            RuleExpectation::new(rule_id.clone(), Some(row_b.clone()), 10),
        ];
        let evaluated = expected
            .iter()
            .map(|item| item.execution().clone())
            .collect();
        let violations = vec![
            RuleViolation::new(
                rule_id.clone(),
                Some(row_a.clone()),
                ValidationPhase::Validate,
                IssueOrder::new(10, 0),
                Vec::new(),
                None,
                "row a".into(),
                RuleAssessment::VerifiedCorrect,
                RuleSeverity::Advisory,
                BehaviorProfile::FilingSafe,
            ),
            RuleViolation::new(
                rule_id,
                Some(row_b.clone()),
                ValidationPhase::Validate,
                IssueOrder::new(10, 1),
                Vec::new(),
                None,
                "row b".into(),
                RuleAssessment::VerifiedCorrect,
                RuleSeverity::Advisory,
                BehaviorProfile::FilingSafe,
            ),
        ];

        let report = ValidationReport::try_new(
            rule_set(),
            context(),
            InputRevision::new(3),
            Sha256Digest::from_bytes([3; 32]).into(),
            expected,
            evaluated,
            violations,
        )
        .unwrap();

        assert_eq!(report.expected_rules()[0].instance(), Some(&row_a));
        assert_eq!(report.evaluated_rules()[1].instance(), Some(&row_b));
        assert_eq!(report.violations()[1].instance(), Some(&row_b));
        let wire = serde_json::to_value(&report).unwrap();
        assert_eq!(
            wire["expected_rules"][0]["execution"]["instance"]["instance_id"],
            "row-a"
        );
        assert_eq!(
            wire["evaluated_rules"][1]["instance"]["instance_id"],
            "row-b"
        );
        assert_eq!(
            wire["violations"][1]["execution"]["instance"]["instance_id"],
            "row-b"
        );
        assert_eq!(
            serde_json::from_value::<ValidationReport>(wire).unwrap(),
            report
        );
    }

    #[test]
    fn group_rule_coverage_rejects_duplicate_missing_and_wrong_instances() {
        let rule_id = RuleId::parse("row-positive").unwrap();
        let row_a = row("row-a");
        let row_b = row("row-b");
        let expected = || {
            vec![
                RuleExpectation::new(rule_id.clone(), Some(row_a.clone()), 10),
                RuleExpectation::new(rule_id.clone(), Some(row_b.clone()), 10),
            ]
        };

        let duplicate_expected = ValidationReport::try_new(
            rule_set(),
            context(),
            InputRevision::new(3),
            Sha256Digest::from_bytes([3; 32]).into(),
            vec![
                RuleExpectation::new(rule_id.clone(), Some(row_a.clone()), 10),
                RuleExpectation::new(rule_id.clone(), Some(row_a.clone()), 10),
            ],
            Vec::new(),
            Vec::new(),
        );
        assert!(matches!(
            duplicate_expected,
            Err(ReportError::DuplicateExpectedRule { execution })
                if execution.instance() == Some(&row_a)
        ));

        let missing = ValidationReport::try_new(
            rule_set(),
            context(),
            InputRevision::new(3),
            Sha256Digest::from_bytes([3; 32]).into(),
            expected(),
            vec![RuleExecution::new(rule_id.clone(), Some(row_a.clone()))],
            Vec::new(),
        );
        assert!(matches!(
            missing,
            Err(ReportError::IncompleteRuleCoverage { .. })
        ));

        let duplicate_execution = ValidationReport::try_new(
            rule_set(),
            context(),
            InputRevision::new(3),
            Sha256Digest::from_bytes([3; 32]).into(),
            expected(),
            vec![
                RuleExecution::new(rule_id.clone(), Some(row_a.clone())),
                RuleExecution::new(rule_id.clone(), Some(row_a.clone())),
            ],
            Vec::new(),
        );
        assert!(matches!(
            duplicate_execution,
            Err(ReportError::IncompleteRuleCoverage { .. })
        ));

        let wrong = ValidationReport::try_new(
            rule_set(),
            context(),
            InputRevision::new(3),
            Sha256Digest::from_bytes([3; 32]).into(),
            expected(),
            vec![
                RuleExecution::new(rule_id.clone(), Some(row_a.clone())),
                RuleExecution::new(rule_id.clone(), Some(row("row-c"))),
            ],
            Vec::new(),
        );
        assert!(matches!(
            wrong,
            Err(ReportError::IncompleteRuleCoverage { .. })
        ));

        let wrong_violation_instance = ValidationReport::try_new(
            rule_set(),
            context(),
            InputRevision::new(3),
            Sha256Digest::from_bytes([3; 32]).into(),
            expected(),
            vec![
                RuleExecution::new(rule_id.clone(), Some(row("row-a"))),
                RuleExecution::new(rule_id.clone(), Some(row_b.clone())),
            ],
            vec![RuleViolation::new(
                rule_id.clone(),
                Some(row("row-c")),
                ValidationPhase::Validate,
                IssueOrder::new(10, 0),
                Vec::new(),
                None,
                "wrong row".into(),
                RuleAssessment::VerifiedCorrect,
                RuleSeverity::Blocking,
                BehaviorProfile::FilingSafe,
            )],
        );
        assert!(matches!(
            wrong_violation_instance,
            Err(ReportError::IssueForUnknownRule { execution })
                if execution.instance() == Some(&row("row-c"))
        ));
    }

    #[test]
    fn group_rule_inventory_requires_stable_instance_order_and_explicit_singleton_serde() {
        let rule_id = RuleId::parse("row-positive").unwrap();
        let row_a = row("row-a");
        let row_b = row("row-b");
        let reversed = ValidationReport::try_new(
            rule_set(),
            context(),
            InputRevision::new(3),
            Sha256Digest::from_bytes([3; 32]).into(),
            vec![
                RuleExpectation::new(rule_id.clone(), Some(row_b.clone()), 10),
                RuleExpectation::new(rule_id, Some(row_a), 10),
            ],
            Vec::new(),
            Vec::new(),
        );
        assert!(matches!(
            reversed,
            Err(ReportError::ExpectedRuleExecutionOrderNotStrict { .. })
        ));

        let singleton = RuleExpectation::singleton(RuleId::parse("singleton").unwrap(), 20);
        let wire = serde_json::to_value(&singleton).unwrap();
        assert_eq!(wire["execution"]["instance"], serde_json::Value::Null);
        assert_eq!(
            serde_json::to_value(singleton.execution()).unwrap()["instance"],
            serde_json::Value::Null
        );
        assert_eq!(
            serde_json::to_value(violation("singleton", 20, 0)).unwrap()["execution"]["instance"],
            serde_json::Value::Null
        );
        assert_eq!(
            serde_json::from_value::<RuleExpectation>(wire).unwrap(),
            singleton
        );
    }
}
