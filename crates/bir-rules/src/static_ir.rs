//! Crate-owned executable representation emitted by the offline rules compiler.
//!
//! The types in this module contain only Rust data and function pointers. They
//! never deserialize the repository `rules/` corpus at runtime. Construction of
//! the [`CompiledRuleSet`](crate::CompiledRuleSet) implementation remains
//! crate-private, so public IR data cannot be used by a downstream crate to
//! forge a reviewed provider.

use crate::materialization::{
    ContractEmissionId, GroupAccountingView, MaterializationError, MaterializationTraceEntryView,
    MaterializedBindingView, MaterializedOmissionView, MaterializedRecordView,
    MaterializedValueSourceView, SerializationMaterialization, digest_serializable,
};
use crate::serialization::{
    BodyEncodingBoundary, FormattedSemanticValue, SerializationArtifactIdentity,
    SerializationArtifactTarget, format_serialization_value,
};
use crate::serialization_contract::{
    DynamicGroupNode, SerializationGroupInstanceOrder, SerializationKeyProjection,
    SerializationNode, SerializationOccurrenceProjection, SerializationPlan, SerializationPresence,
    SerializationSemanticFormat, SerializationValueProjection,
};
use crate::{
    BehaviorProfile, CalculationId, CanonicalDate, CanonicalFieldValue, CanonicalValue,
    ContextValueId, DerivedOutputExpectation, DerivedValue, EvaluationError, EvaluationExpectation,
    EvaluationOutput, EvaluationRequest, EvaluationResult, ExactDecimal, FieldId, FieldInstance,
    FieldValueAssignment, FieldValueAssignmentExpectation, FormRevisionKey, IssueOrder, OutputId,
    RawValue, RepeatedGroupId, RepeatedGroupInstance, RuleAssessment, RuleExecution,
    RuleExpectation, RuleFieldRef, RuleId, RuleSeverity, RuleViolation, StableInstanceId,
    ValidationContext, ValidationPhase, WorkflowAction, WorkflowNotification,
    WorkflowNotificationChannel, WorkflowStateId, WorkflowTransitionId, WorkflowTransitionResult,
};
use num_bigint::{BigInt, Sign};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashSet},
    error::Error,
    fmt,
};

/// Closed value types accepted by executable v2 expression nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValueType {
    Null,
    String,
    Boolean,
    Integer,
    Decimal,
    Date,
}

/// Concrete runtime shape used in typed mismatch diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValueKind {
    Absent,
    Blank,
    String,
    Boolean,
    Integer,
    Decimal,
    Date,
}

impl ValueKind {
    fn of(value: &CanonicalValue) -> Self {
        match value {
            CanonicalValue::Absent => Self::Absent,
            CanonicalValue::Blank => Self::Blank,
            CanonicalValue::Text(_) => Self::String,
            CanonicalValue::Boolean(_) => Self::Boolean,
            CanonicalValue::Integer(_) => Self::Integer,
            CanonicalValue::Decimal(_) => Self::Decimal,
            CanonicalValue::Date(_) => Self::Date,
        }
    }
}

/// Branch states admitted by the v2 schemas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BranchState {
    Executable,
    DocumentedOnly,
    Unresolved,
}

/// One profile-specific branch. Non-executable variants carry no executable
/// payload and can never fall back to the other profile.
#[derive(Debug, Clone, Copy)]
pub enum Branch<T> {
    Executable(T),
    DocumentedOnly,
    Unresolved,
}

impl<T> Branch<T> {
    pub const fn state(&self) -> BranchState {
        match self {
            Self::Executable(_) => BranchState::Executable,
            Self::DocumentedOnly => BranchState::DocumentedOnly,
            Self::Unresolved => BranchState::Unresolved,
        }
    }
}

/// Independently reviewed official and filing-safe data.
#[derive(Debug, Clone, Copy)]
pub struct Profiled<T> {
    pub official: T,
    pub filing_safe: T,
}

impl<T: Copy> Profiled<T> {
    const fn select(&self, profile: BehaviorProfile) -> T {
        match profile {
            BehaviorProfile::OfficialCompatibility => self.official,
            BehaviorProfile::FilingSafe => self.filing_safe,
        }
    }
}

/// Exact literal payloads suitable for generated Rust constants.
#[derive(Debug, Clone, Copy)]
pub enum TypedValue {
    Null,
    String(&'static str),
    Boolean(bool),
    Integer(i128),
    Decimal(DecimalLiteral),
    Date(DateLiteral),
}

#[derive(Debug, Clone, Copy)]
pub struct DecimalLiteral {
    pub coefficient: i128,
    pub scale: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct DateLiteral {
    pub year: u16,
    pub month: u8,
    pub day: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrimSide {
    Both,
    Start,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LetterCase {
    Upper,
    Lower,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NewlineStyle {
    Lf,
    Crlf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DateFormat {
    YearMonthDay,
    MonthSlashDaySlashYear,
    MonthDashDayDashYear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecimalGrouping {
    None,
    Comma,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoundingMode {
    None,
    HalfUp,
    HalfEven,
    HalfCeiling,
    TowardZero,
    AwayFromZero,
    Floor,
    Ceiling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rounding {
    pub mode: RoundingMode,
    pub scale: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DecimalDivisionPolicy {
    pub scale: u32,
    pub rounding: RoundingMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExactRoundingError {
    Inexact,
    ScaleTooLarge { scale: u32, maximum: u32 },
    Overflow,
}

/// Every normalization node currently accepted by
/// `normalization.schema.json`, in pipeline order.
#[derive(Debug, Clone, Copy)]
pub enum NormalizationStep {
    Trim {
        side: TrimSide,
    },
    ChangeCase {
        case: LetterCase,
    },
    ReplaceLiteral {
        from: &'static str,
        to: &'static str,
    },
    StripCharacters {
        characters: &'static str,
    },
    DigitsOnly,
    NormalizeNewlines {
        style: NewlineStyle,
    },
    DateFormat {
        format: DateFormat,
    },
    DecimalFormat {
        grouping: DecimalGrouping,
        rounding: Rounding,
    },
    /// Exact finite behavior of the Offline eBIRForms `round(number, 2)`
    /// helper used by editable money controls. The helper's non-finite output
    /// strings remain evidence-blocked and therefore fail closed.
    OfflineEbirMoneyRoundV1,
    /// Exact finite behavior of the Offline eBIRForms
    /// `blockletterWithout2Decimal` helper: legacy `parseFloat`, followed by
    /// zero-decimal `toFixed`, with NaN mapped to an empty display buffer.
    OfflineEbirParseFloatFixedZeroV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StringEmptyPolicy {
    EmptyString,
    Null,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NumericEmptyPolicy {
    Null,
    Zero,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BooleanEmptyPolicy {
    Null,
    False,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DateEmptyPolicy {
    Null,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InvalidValuePolicy {
    Error,
    PreserveRaw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputGrouping {
    Forbidden,
    Comma,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OverflowPolicy {
    Error,
    Clamp,
}

#[derive(Debug, Clone, Copy)]
pub struct DecimalPolicy {
    pub precision: u32,
    pub scale: u32,
    pub division_scale: u32,
    pub rounding: Rounding,
    pub overflow: OverflowPolicy,
}

/// Every coercion node currently accepted by `coercion.schema.json`.
#[derive(Debug, Clone, Copy)]
pub enum Coercion {
    String {
        on_empty: StringEmptyPolicy,
    },
    Decimal {
        decimal: DecimalPolicy,
        grouping: InputGrouping,
        on_empty: NumericEmptyPolicy,
        on_invalid: InvalidValuePolicy,
    },
    Integer {
        on_empty: NumericEmptyPolicy,
        on_invalid: InvalidValuePolicy,
    },
    Boolean {
        true_values: &'static [&'static str],
        false_values: &'static [&'static str],
        on_empty: BooleanEmptyPolicy,
        on_invalid: InvalidValuePolicy,
    },
    Date {
        accepted_formats: &'static [DateFormat],
        on_empty: DateEmptyPolicy,
        on_invalid: InvalidValuePolicy,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct FieldEventNormalization {
    pub phase: ValidationPhase,
    pub normalization: &'static [NormalizationStep],
}

#[derive(Debug, Clone, Copy)]
pub struct FieldBehavior {
    pub normalization: &'static [NormalizationStep],
    /// Additional normalization applied only to the exact field occurrence
    /// named by an input/blur/change request, before calculations execute.
    pub event_normalization: &'static [FieldEventNormalization],
    pub coercion: Coercion,
}

#[derive(Debug, Clone, Copy)]
pub struct ContextValueSpec {
    pub context_value_id: &'static str,
    pub value_type: ValueType,
    pub required: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct FieldGroupSpec {
    pub group_id: &'static str,
    pub min_occurs: usize,
    pub max_occurs: Option<usize>,
    pub members: &'static [&'static str],
}

#[derive(Debug, Clone, Copy)]
pub struct FieldSpec {
    pub field_id: &'static str,
    pub value_type: ValueType,
    pub group_id: Option<&'static str>,
    pub calculation_id: Option<&'static str>,
    pub behavior: Profiled<Branch<FieldBehavior>>,
}

/// Closed field instance selectors accepted by `common.schema.json`.
#[derive(Debug, Clone, Copy)]
pub enum FieldInstanceSelector {
    Singleton,
    CurrentGroupInstance,
    StableInstanceId(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub struct FieldRef {
    pub field_id: &'static str,
    pub instance: FieldInstanceSelector,
}

/// Exact execution cardinality for a calculation or rule.
///
/// Source JSON must select one branch explicitly. There is no implicit
/// singleton default because doing so would silently collapse repeated-row
/// behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EvaluationScope {
    Singleton,
    EachGroup(&'static str),
}

/// Exact instance selector for a derived calculation output.
///
/// The target calculation's [`EvaluationScope`] supplies the repeated group
/// identity. A current-group selector is valid only while evaluating that same
/// group; a stable selector is resolved against the request's declared group
/// instances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DerivedInstanceSelector {
    Singleton,
    CurrentGroupInstance,
    StableInstanceId(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOperator {
    Negate,
    Absolute,
    Length,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Concat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NaryOperator {
    Sum,
    Minimum,
    Maximum,
    Concat,
    Coalesce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupAggregateOperator {
    Sum,
    Minimum,
    Maximum,
    Count,
    CountPresent,
}

/// Every expression node currently accepted by `expression.schema.json`.
///
/// Recursive children are references so generated modules can emit ordinary
/// `static` values without heap allocation.
#[derive(Debug, Clone, Copy)]
pub enum Expression {
    Literal(TypedValue),
    Field {
        result_type: ValueType,
        field: FieldRef,
    },
    Derived {
        result_type: ValueType,
        calculation_id: &'static str,
        output_id: &'static str,
        instance: DerivedInstanceSelector,
    },
    Context {
        result_type: ValueType,
        context_value_id: &'static str,
    },
    Unary {
        result_type: ValueType,
        operator: UnaryOperator,
        operand: &'static Expression,
    },
    Binary {
        result_type: ValueType,
        operator: BinaryOperator,
        division_policy: Option<DecimalDivisionPolicy>,
        left: &'static Expression,
        right: &'static Expression,
    },
    Nary {
        result_type: ValueType,
        operator: NaryOperator,
        operands: &'static [Expression],
    },
    Conditional {
        result_type: ValueType,
        condition: &'static Predicate,
        when_true: &'static Expression,
        when_false: &'static Expression,
    },
    Coerce {
        result_type: ValueType,
        input: &'static Expression,
        coercion: Coercion,
    },
    SplitComponent {
        result_type: ValueType,
        input: &'static Expression,
        delimiter: &'static str,
        index: u32,
    },
    JavaScriptParseIntRadix10 {
        result_type: ValueType,
        input: &'static Expression,
    },
    JavaScriptDateLocalDay {
        result_type: ValueType,
        year: &'static Expression,
        month_index: &'static Expression,
        day: &'static Expression,
    },
    CanonicalLocalDateDay {
        result_type: ValueType,
        input: &'static Expression,
    },
    GroupAggregate {
        result_type: ValueType,
        operator: GroupAggregateOperator,
        group_id: &'static str,
        value: &'static Expression,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompareOperator {
    Equal,
    NotEqual,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PresenceOperator {
    IsEmpty,
    IsPresent,
    IsNull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JavaScriptParseFloatOperator {
    IsNaN,
    StrictEqual,
    GreaterThan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JavaScriptNumberCompareOperator {
    LessThan,
    GreaterThan,
    StrictEqual,
}

/// Closed checksum algorithms whose complete behavior is packaged into the
/// static runtime. Adding a variant requires digest-pinned upstream evidence
/// and conformance fixtures; generated rule sets never supply executable code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChecksumAlgorithm {
    /// The nine-digit helper shipped with the Offline eBIRForms package.
    ///
    /// This includes the helper's strict legacy adjustment interval. Form
    /// wrappers may still add independently evidenced exceptions as ordinary
    /// predicates (for example, a literal bypass); those do not belong here.
    OfflineEbirTinV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupQuantifier {
    Any,
    All,
    None,
}

/// A compiler-supplied packaged matcher.
///
/// The schema's pattern string remains attached for provenance. The runtime
/// does not parse or compile a regex dynamically; code generation must provide
/// a deterministic matcher for the reviewed dialect.
#[derive(Debug, Clone, Copy)]
pub struct StaticPattern {
    pub source: &'static str,
    pub matcher: fn(value: &str, case_sensitive: bool) -> bool,
}

/// Every predicate node currently accepted by `predicate.schema.json`.
#[derive(Debug, Clone, Copy)]
pub enum Predicate {
    Constant(bool),
    Not(&'static Predicate),
    All(&'static [Predicate]),
    Any(&'static [Predicate]),
    Compare {
        operator: CompareOperator,
        left: &'static Expression,
        right: &'static Expression,
    },
    Presence {
        operator: PresenceOperator,
        value: &'static Expression,
    },
    CoercionFailed {
        field: FieldRef,
    },
    JavaScriptParseFloat {
        operator: JavaScriptParseFloatOperator,
        input: &'static Expression,
        operand: Option<DecimalLiteral>,
    },
    JavaScriptGlobalIsNaNLogicalOr {
        inputs: &'static [Expression],
    },
    JavaScriptNumberCompare {
        operator: JavaScriptNumberCompareOperator,
        input: &'static Expression,
        operand: &'static Expression,
    },
    Checksum {
        algorithm: ChecksumAlgorithm,
        input: &'static Expression,
    },
    Matches {
        value: &'static Expression,
        pattern: StaticPattern,
        case_sensitive: bool,
    },
    In {
        value: &'static Expression,
        candidates: &'static [TypedValue],
    },
    GroupQuantifier {
        quantifier: GroupQuantifier,
        group_id: &'static str,
        predicate: &'static Predicate,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EffectKind {
    EmitIssue,
    EmitNotification,
    SetRawFieldValue,
    SetDerived,
    NormalizeField,
    SetWorkflowState,
}

/// Raw buffer literals admitted by event-rule mutation effects.
#[derive(Debug, Clone, Copy)]
pub enum StaticRawValue {
    Absent,
    Text(&'static str),
}

impl StaticRawValue {
    fn to_raw_value(self) -> RawValue {
        match self {
            Self::Absent => RawValue::Absent,
            Self::Text(value) => RawValue::Text(value.to_owned()),
        }
    }
}

/// Every effect node currently accepted by `effect.schema.json`.
#[derive(Debug, Clone, Copy)]
pub enum Effect {
    EmitIssue {
        severity: RuleSeverity,
        message: &'static str,
        official_message: Option<&'static str>,
        assessment: RuleAssessment,
        fields: &'static [FieldRef],
    },
    EmitNotification {
        channel: WorkflowNotificationChannel,
        message: &'static str,
        official_message: Option<&'static str>,
    },
    SetRawFieldValue {
        field: FieldRef,
        value: StaticRawValue,
    },
    SetDerived {
        output_id: &'static str,
        value: &'static Expression,
    },
    NormalizeField {
        field: FieldRef,
        normalization: &'static [NormalizationStep],
    },
    SetWorkflowState {
        state_id: &'static str,
    },
}

impl Effect {
    pub const fn kind(&self) -> EffectKind {
        match self {
            Self::EmitIssue { .. } => EffectKind::EmitIssue,
            Self::EmitNotification { .. } => EffectKind::EmitNotification,
            Self::SetRawFieldValue { .. } => EffectKind::SetRawFieldValue,
            Self::SetDerived { .. } => EffectKind::SetDerived,
            Self::NormalizeField { .. } => EffectKind::NormalizeField,
            Self::SetWorkflowState { .. } => EffectKind::SetWorkflowState,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CalculationOutput {
    pub output_id: &'static str,
    pub value: &'static Expression,
    pub rounding: Option<&'static [Rounding]>,
    pub writeback: Option<CalculationWriteback>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CalculationWriteFormat {
    OfflineEbirFormatCurrencyV1,
}

#[derive(Debug, Clone, Copy)]
pub struct CalculationWriteback {
    pub field: FieldRef,
    pub format: CalculationWriteFormat,
}

#[derive(Debug, Clone, Copy)]
pub struct CalculationBranch {
    pub condition: &'static Predicate,
    pub outputs: &'static [CalculationOutput],
}

#[derive(Debug, Clone, Copy)]
pub struct CalculationSpec {
    pub calculation_id: &'static str,
    pub scope: EvaluationScope,
    pub depends_on: &'static [&'static str],
    pub phases: &'static [ValidationPhase],
    pub trigger_field_ids: &'static [&'static str],
    pub profiles: Profiled<Branch<CalculationBranch>>,
}

#[derive(Debug, Clone, Copy)]
pub struct RuleBranch {
    pub predicate: &'static Predicate,
    pub effects: &'static [Effect],
}

#[derive(Debug, Clone, Copy)]
pub struct RuleSpec {
    pub rule_id: &'static str,
    pub scope: EvaluationScope,
    pub order: u32,
    pub phases: &'static [ValidationPhase],
    pub trigger_field_ids: &'static [&'static str],
    pub profiles: Profiled<Branch<RuleBranch>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScheduledOutputWriteMode {
    Insert,
    Replace,
}

#[derive(Debug, Clone, Copy)]
pub enum FieldEventStep {
    Calculation {
        calculation_id: &'static str,
        output_ids: &'static [&'static str],
        write_mode: ScheduledOutputWriteMode,
    },
    Rule {
        rule_id: &'static str,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct FieldEventProgram {
    pub steps: &'static [FieldEventStep],
}

#[derive(Debug, Clone, Copy)]
pub struct FieldEventProgramSpec {
    pub phase: ValidationPhase,
    pub trigger_field_id: &'static str,
    pub profiles: Profiled<Branch<FieldEventProgram>>,
}

#[derive(Debug, Clone, Copy)]
pub struct WorkflowStateSpec {
    pub state_id: &'static str,
    pub terminal: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct WorkflowTransitionBranch {
    pub guard: &'static Predicate,
    pub effects: &'static [Effect],
}

#[derive(Debug, Clone, Copy)]
pub struct WorkflowTransitionSpec {
    pub transition_id: &'static str,
    pub from_state: &'static str,
    pub action: WorkflowAction,
    pub evaluation_phase: ValidationPhase,
    pub to_state: &'static str,
    pub profiles: Profiled<Branch<WorkflowTransitionBranch>>,
}

#[derive(Debug, Clone, Copy)]
pub struct StaticWorkflowSpec {
    pub initial_state: &'static str,
    pub states: &'static [WorkflowStateSpec],
    pub transitions: &'static [WorkflowTransitionSpec],
}

/// Explicit compatibility behavior for packages whose UI stopped at the first
/// blocking error. Rule predicates are still evaluated and reported in full.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EffectEvaluationMode {
    ApplyAll,
    StopEffectsAfterFirstBlockingIssue,
}

/// Static executable rule-set payload. Identity is held separately by the
/// crate-private provider so generated constants cannot weaken exact registry
/// selection.
#[derive(Debug)]
pub struct StaticRuleSetSpec {
    pub profile_status: Profiled<Branch<()>>,
    pub effect_mode: Profiled<Branch<EffectEvaluationMode>>,
    pub serialization: &'static crate::StaticSerializationContract,
    pub context_values: &'static [ContextValueSpec],
    pub field_groups: &'static [FieldGroupSpec],
    pub fields: &'static [FieldSpec],
    pub field_event_programs: &'static [FieldEventProgramSpec],
    pub evaluation_order: &'static [&'static str],
    pub calculations: &'static [CalculationSpec],
    pub rules: &'static [RuleSpec],
    pub workflow: Branch<StaticWorkflowSpec>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpecItemKind {
    RuleSet,
    EvaluationPolicy,
    ContextValue,
    FieldGroup,
    Field,
    FieldEventProgram,
    Calculation,
    Output,
    Rule,
    StableInstance,
    Pattern,
    WorkflowState,
    WorkflowTransition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaticSpecError {
    InvalidIdentifier {
        kind: SpecItemKind,
        value: &'static str,
    },
    DuplicateIdentifier {
        kind: SpecItemKind,
        value: &'static str,
    },
    EmptyRequiredList {
        kind: SpecItemKind,
        value: &'static str,
    },
    DuplicatePhase {
        kind: SpecItemKind,
        value: &'static str,
        phase: ValidationPhase,
    },
    InvalidEventBinding {
        kind: SpecItemKind,
        value: &'static str,
    },
    InvalidReference {
        kind: SpecItemKind,
        value: &'static str,
        target: &'static str,
    },
    InvalidGroupCardinality {
        group_id: &'static str,
        min_occurs: usize,
        max_occurs: usize,
    },
    DuplicateRuleOrder {
        phase: ValidationPhase,
        order: u32,
    },
    InvalidRuleOrder {
        rule_id: &'static str,
    },
    EvaluationOrderDuplicate {
        calculation_id: &'static str,
    },
    EvaluationOrderUnknown {
        calculation_id: &'static str,
    },
    EvaluationOrderMissing {
        calculation_id: &'static str,
    },
    DependencyOutOfOrder {
        calculation_id: &'static str,
        dependency_id: &'static str,
    },
    InvalidRoundingScale {
        scale: u32,
    },
    MissingDecimalDivisionPolicy,
    UnexpectedDecimalDivisionPolicy {
        operator: BinaryOperator,
    },
    InvalidDecimalDivisionScale {
        scale: u32,
    },
    InvalidDecimalPolicy {
        precision: u32,
        scale: u32,
        division_scale: u32,
    },
    TypeMismatch {
        operation: ExecutionOperation,
        expected: ValueType,
        actual: ValueType,
    },
    InvalidCoercionFailedPredicate {
        field_id: &'static str,
        profile: BehaviorProfile,
    },
    InvalidJavaScriptParseFloatPredicate {
        operator: JavaScriptParseFloatOperator,
        has_operand: bool,
    },
    AmbiguousBooleanCoercionValue {
        value: &'static str,
    },
    UnsupportedEffect {
        kind: SpecItemKind,
        value: &'static str,
        effect: EffectKind,
    },
    EmptyPattern,
}

impl fmt::Display for StaticSpecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentifier { kind, value } => {
                write!(formatter, "invalid static {kind:?} identifier {value:?}")
            }
            Self::DuplicateIdentifier { kind, value } => {
                write!(formatter, "duplicate static {kind:?} identifier {value}")
            }
            Self::EmptyRequiredList { kind, value } => {
                write!(
                    formatter,
                    "static {kind:?} {value} has an empty required list"
                )
            }
            Self::DuplicatePhase { kind, value, phase } => {
                write!(formatter, "static {kind:?} {value} repeats phase {phase:?}")
            }
            Self::InvalidEventBinding { kind, value } => write!(
                formatter,
                "static {kind:?} {value} has an invalid field-event phase/trigger binding"
            ),
            Self::InvalidReference {
                kind,
                value,
                target,
            } => write!(
                formatter,
                "static {kind:?} {value} contains invalid reference {target}"
            ),
            Self::InvalidGroupCardinality {
                group_id,
                min_occurs,
                max_occurs,
            } => write!(
                formatter,
                "group {group_id} minimum {min_occurs} exceeds maximum {max_occurs}"
            ),
            Self::DuplicateRuleOrder { phase, order } => {
                write!(
                    formatter,
                    "duplicate static rule order {order} in phase {phase:?}"
                )
            }
            Self::InvalidRuleOrder { rule_id } => {
                write!(formatter, "static rule {rule_id} has zero order")
            }
            Self::EvaluationOrderDuplicate { calculation_id } => {
                write!(
                    formatter,
                    "evaluation order repeats calculation {calculation_id}"
                )
            }
            Self::EvaluationOrderUnknown { calculation_id } => {
                write!(
                    formatter,
                    "evaluation order names unknown calculation {calculation_id}"
                )
            }
            Self::EvaluationOrderMissing { calculation_id } => {
                write!(
                    formatter,
                    "evaluation order omits calculation {calculation_id}"
                )
            }
            Self::DependencyOutOfOrder {
                calculation_id,
                dependency_id,
            } => write!(
                formatter,
                "calculation {calculation_id} dependency {dependency_id} is not earlier in evaluation order"
            ),
            Self::InvalidRoundingScale { scale } => {
                write!(formatter, "rounding scale {scale} exceeds 18")
            }
            Self::MissingDecimalDivisionPolicy => {
                formatter.write_str("decimal division is missing its expression policy")
            }
            Self::UnexpectedDecimalDivisionPolicy { operator } => {
                write!(
                    formatter,
                    "binary operator {operator:?} must not carry a decimal division policy"
                )
            }
            Self::InvalidDecimalDivisionScale { scale } => {
                write!(formatter, "decimal division scale {scale} exceeds 18")
            }
            Self::InvalidDecimalPolicy {
                precision,
                scale,
                division_scale,
            } => write!(
                formatter,
                "invalid decimal policy precision={precision}, scale={scale}, division_scale={division_scale}"
            ),
            Self::TypeMismatch {
                operation,
                expected,
                actual,
            } => write!(
                formatter,
                "static {operation:?} expected {expected:?}, got {actual:?}"
            ),
            Self::InvalidCoercionFailedPredicate { field_id, profile } => write!(
                formatter,
                "coercion-failed predicate field {field_id} is not a non-string preserve-raw coercion in profile {profile:?}"
            ),
            Self::InvalidJavaScriptParseFloatPredicate {
                operator,
                has_operand,
            } => write!(
                formatter,
                "JavaScript parseFloat predicate operator {operator:?} has_operand={has_operand}"
            ),
            Self::AmbiguousBooleanCoercionValue { value } => write!(
                formatter,
                "boolean coercion value {value:?} is present in both true_values and false_values"
            ),
            Self::UnsupportedEffect {
                kind,
                value,
                effect,
            } => write!(
                formatter,
                "static {kind:?} {value} uses unsupported effect {effect:?}"
            ),
            Self::EmptyPattern => formatter.write_str("static regex pattern must not be empty"),
        }
    }
}

impl Error for StaticSpecError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExecutionOperation {
    FieldLookup,
    ContextLookup,
    DerivedLookup,
    UnaryNegate,
    UnaryAbsolute,
    UnaryLength,
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Concat,
    Sum,
    Minimum,
    Maximum,
    Compare,
    Matches,
    Rounding,
    GroupAggregate,
    Coercion,
    SplitComponent,
    JavaScriptParseIntRadix10,
    JavaScriptParseFloat,
    JavaScriptGlobalIsNaNLogicalOr,
    JavaScriptNumberCompare,
    Checksum,
    JavaScriptDateLocalDay,
    CanonicalLocalDateDay,
    NormalizeField,
    SetRawFieldValue,
    CalculationWriteback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoercionFailure {
    Empty,
    InvalidSyntax,
    InvalidGrouping,
    UnknownBoolean,
    InvalidDate,
    PrecisionOverflow,
}

/// Typed fail-closed errors from the static interpreter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterpreterError {
    InvalidStaticSpec(StaticSpecError),
    BranchUnavailable {
        kind: SpecItemKind,
        id: &'static str,
        state: BranchState,
        context: ValidationContext,
    },
    MissingInput {
        field: FieldInstance,
    },
    UnexpectedInput {
        field: FieldInstance,
    },
    UnexpectedGroupInstance {
        instance: RepeatedGroupInstance,
    },
    MissingContextValue {
        id: ContextValueId,
    },
    UnexpectedContextValue {
        id: ContextValueId,
    },
    MissingDerivedValue {
        calculation_id: CalculationId,
        output_id: OutputId,
        instance: Option<RepeatedGroupInstance>,
    },
    MissingDerivedOutputTarget {
        output_id: OutputId,
    },
    AmbiguousDerivedOutput {
        output_id: OutputId,
    },
    MissingFieldEventProgram {
        phase: ValidationPhase,
        field: FieldInstance,
    },
    InvalidScheduledOutputWrite {
        calculation_id: CalculationId,
        output_id: OutputId,
        mode: ScheduledOutputWriteMode,
    },
    TypeMismatch {
        operation: ExecutionOperation,
        expected: ValueType,
        actual: ValueKind,
    },
    InvalidCoercion {
        target: ValueType,
        reason: CoercionFailure,
    },
    DivisionByZero {
        operation: ExecutionOperation,
    },
    NonTerminatingDecimalDivision,
    Overflow {
        operation: ExecutionOperation,
    },
    MissingCurrentGroup {
        field_id: FieldId,
    },
    MissingCurrentDerivedGroup {
        calculation_id: CalculationId,
    },
    DerivedScopeMismatch {
        calculation_id: CalculationId,
    },
    FieldScopeMismatch {
        field_id: FieldId,
    },
    GroupCardinality {
        group_id: RepeatedGroupId,
        minimum: usize,
        maximum: Option<usize>,
        actual: usize,
    },
    UnsupportedEffect {
        rule_id: RuleId,
        kind: EffectKind,
    },
}

impl fmt::Display for InterpreterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidStaticSpec(error) => write!(formatter, "invalid static IR: {error}"),
            Self::BranchUnavailable {
                kind,
                id,
                state,
                context,
            } => write!(
                formatter,
                "{kind:?} {id} branch is {state:?}, not executable for {context:?}"
            ),
            Self::MissingInput { field } => {
                write!(formatter, "missing input {}", field.field_id())
            }
            Self::UnexpectedInput { field } => {
                write!(formatter, "unexpected input {}", field.field_id())
            }
            Self::UnexpectedGroupInstance { instance } => write!(
                formatter,
                "unexpected repeated-group instance {}:{}",
                instance.group_id(),
                instance.instance_id()
            ),
            Self::MissingContextValue { id } => write!(formatter, "missing context value {id}"),
            Self::UnexpectedContextValue { id } => {
                write!(formatter, "unexpected context value {id}")
            }
            Self::MissingDerivedValue {
                calculation_id,
                output_id,
                instance,
            } => write!(
                formatter,
                "missing derived value {calculation_id}:{output_id} at {instance:?}"
            ),
            Self::MissingDerivedOutputTarget { output_id } => {
                write!(formatter, "missing derived output target {output_id}")
            }
            Self::AmbiguousDerivedOutput { output_id } => {
                write!(formatter, "derived output ID {output_id} is ambiguous")
            }
            Self::MissingFieldEventProgram { phase, field } => write!(
                formatter,
                "missing exact {phase:?} event program for {}",
                field.field_id()
            ),
            Self::InvalidScheduledOutputWrite {
                calculation_id,
                output_id,
                mode,
            } => write!(
                formatter,
                "scheduled {mode:?} is invalid for {calculation_id}:{output_id}"
            ),
            Self::TypeMismatch {
                operation,
                expected,
                actual,
            } => write!(
                formatter,
                "{operation:?} expected {expected:?}, got {actual:?}"
            ),
            Self::InvalidCoercion { target, reason } => {
                write!(formatter, "invalid {target:?} coercion: {reason:?}")
            }
            Self::DivisionByZero { operation } => {
                write!(formatter, "division by zero during {operation:?}")
            }
            Self::NonTerminatingDecimalDivision => {
                formatter.write_str("decimal division has no exact finite representation")
            }
            Self::Overflow { operation } => write!(formatter, "overflow during {operation:?}"),
            Self::MissingCurrentGroup { field_id } => {
                write!(
                    formatter,
                    "field {field_id} requires a current group instance"
                )
            }
            Self::MissingCurrentDerivedGroup { calculation_id } => write!(
                formatter,
                "calculation {calculation_id} requires a current group instance"
            ),
            Self::DerivedScopeMismatch { calculation_id } => write!(
                formatter,
                "calculation {calculation_id} was referenced with the wrong group scope"
            ),
            Self::FieldScopeMismatch { field_id } => {
                write!(
                    formatter,
                    "field {field_id} was referenced with the wrong group scope"
                )
            }
            Self::GroupCardinality {
                group_id,
                minimum,
                maximum,
                actual,
            } => write!(
                formatter,
                "group {group_id} has {actual} instances; expected {minimum}..={maximum:?}"
            ),
            Self::UnsupportedEffect { rule_id, kind } => write!(
                formatter,
                "rule {rule_id} uses {kind:?}, which has no safe executable semantics"
            ),
        }
    }
}

impl Error for InterpreterError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidStaticSpec(error) => Some(error),
            _ => None,
        }
    }
}

impl From<StaticSpecError> for InterpreterError {
    fn from(value: StaticSpecError) -> Self {
        Self::InvalidStaticSpec(value)
    }
}

/// Fail-closed error from independently inspecting a reviewed serialization
/// projection against one exact evaluated request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SerializationInspectionError {
    Unavailable,
    BindingMismatch { field: &'static str },
    CurrentGroupNotDeclared { instance: RepeatedGroupInstance },
    Interpreter(InterpreterError),
}

impl fmt::Display for SerializationInspectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => {
                formatter.write_str("compiled rule set has no serialization inspector")
            }
            Self::BindingMismatch { field } => {
                write!(
                    formatter,
                    "serialization inspector does not match evaluated {field}"
                )
            }
            Self::CurrentGroupNotDeclared { instance } => write!(
                formatter,
                "serialization inspector group {}:{} is absent from the exact request",
                instance.group_id(),
                instance.instance_id()
            ),
            Self::Interpreter(error) => {
                write!(formatter, "serialization inspection failed: {error}")
            }
        }
    }
}

impl Error for SerializationInspectionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Interpreter(error) => Some(error),
            _ => None,
        }
    }
}

impl From<InterpreterError> for SerializationInspectionError {
    fn from(value: InterpreterError) -> Self {
        Self::Interpreter(value)
    }
}

impl From<StaticSpecError> for SerializationInspectionError {
    fn from(value: StaticSpecError) -> Self {
        Self::Interpreter(value.into())
    }
}

/// Fail-closed error from selecting one explicit workflow transition against
/// an exact, already validated evaluation result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowTransitionError {
    Unavailable {
        state: BranchState,
    },
    BindingMismatch {
        field: &'static str,
    },
    InvalidActionPhase {
        action: WorkflowAction,
        expected: ValidationPhase,
        actual: ValidationPhase,
    },
    EvaluationNotValid,
    TransitionSelection {
        matches: usize,
    },
    BranchUnavailable {
        transition_id: &'static str,
        profile: BehaviorProfile,
        state: BranchState,
    },
    GuardRejected {
        transition_id: &'static str,
    },
    MissingStateEffect {
        transition_id: &'static str,
    },
    DuplicateStateEffect {
        transition_id: &'static str,
    },
    StateEffectMismatch {
        transition_id: &'static str,
        expected: &'static str,
        actual: &'static str,
    },
    UnsupportedEffect {
        transition_id: &'static str,
        kind: EffectKind,
    },
    Evaluation(EvaluationError),
    Interpreter(InterpreterError),
}

impl fmt::Display for WorkflowTransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable { state } => {
                write!(formatter, "workflow is {state:?}, not executable")
            }
            Self::BindingMismatch { field } => {
                write!(
                    formatter,
                    "workflow transition does not match evaluated {field}"
                )
            }
            Self::InvalidActionPhase {
                action,
                expected,
                actual,
            } => {
                write!(
                    formatter,
                    "workflow action {action:?} requires evaluated phase {expected:?}, found {actual:?}"
                )
            }
            Self::EvaluationNotValid => {
                formatter.write_str("workflow transition requires a valid evaluation")
            }
            Self::TransitionSelection { matches } => {
                write!(
                    formatter,
                    "workflow transition selection matched {matches} branches"
                )
            }
            Self::BranchUnavailable {
                transition_id,
                profile,
                state,
            } => write!(
                formatter,
                "workflow transition {transition_id} is {state:?} for {profile:?}"
            ),
            Self::GuardRejected { transition_id } => {
                write!(
                    formatter,
                    "workflow transition {transition_id} guard rejected"
                )
            }
            Self::MissingStateEffect { transition_id } => write!(
                formatter,
                "workflow transition {transition_id} has no state effect"
            ),
            Self::DuplicateStateEffect { transition_id } => write!(
                formatter,
                "workflow transition {transition_id} has more than one state effect"
            ),
            Self::StateEffectMismatch {
                transition_id,
                expected,
                actual,
            } => write!(
                formatter,
                "workflow transition {transition_id} targets {expected}, but its state effect targets {actual}"
            ),
            Self::UnsupportedEffect {
                transition_id,
                kind,
            } => write!(
                formatter,
                "workflow transition {transition_id} uses unsupported effect {kind:?}"
            ),
            Self::Evaluation(error) => {
                write!(formatter, "workflow request re-evaluation failed: {error}")
            }
            Self::Interpreter(error) => write!(formatter, "workflow interpreter failed: {error}"),
        }
    }
}

impl Error for WorkflowTransitionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Evaluation(error) => Some(error),
            Self::Interpreter(error) => Some(error),
            _ => None,
        }
    }
}

impl From<InterpreterError> for WorkflowTransitionError {
    fn from(value: InterpreterError) -> Self {
        Self::Interpreter(value)
    }
}

impl From<EvaluationError> for WorkflowTransitionError {
    fn from(value: EvaluationError) -> Self {
        Self::Evaluation(value)
    }
}

/// Opaque, request-bound view used to independently inspect generated
/// serialization predicates and sources without re-running the evaluator.
///
/// Construction remains behind [`CompiledRuleSet`](crate::CompiledRuleSet)'s
/// sealed provider boundary. The inspector owns canonical and derived
/// snapshots from an already-validated result and resolves every current-row
/// selector from the exact captured request.
#[doc(hidden)]
#[derive(Debug)]
pub struct SerializationInspector {
    spec: &'static StaticRuleSetSpec,
    request: EvaluationRequest,
    canonical_inputs: Vec<CanonicalFieldValue>,
    derived_outputs: Vec<DerivedValue>,
}

impl SerializationInspector {
    fn try_new(
        spec: &'static StaticRuleSetSpec,
        request: &EvaluationRequest,
        result: &EvaluationResult,
    ) -> Result<Self, SerializationInspectionError> {
        if request.rule_set() != result.rule_set() {
            return Err(SerializationInspectionError::BindingMismatch { field: "rule_set" });
        }
        if request.context() != result.context() {
            return Err(SerializationInspectionError::BindingMismatch { field: "context" });
        }
        if request.input_revision() != result.input_revision() {
            return Err(SerializationInspectionError::BindingMismatch {
                field: "input_revision",
            });
        }
        if request.context_fingerprint() != result.context_fingerprint() {
            return Err(SerializationInspectionError::BindingMismatch {
                field: "context_fingerprint",
            });
        }
        let raw_fields = request.raw_inputs().fields();
        if raw_fields.len() != result.canonical_inputs().len()
            || raw_fields
                .iter()
                .zip(result.canonical_inputs())
                .any(|(raw, canonical)| {
                    raw.field() != canonical.field() || raw.value() != canonical.raw()
                })
        {
            return Err(SerializationInspectionError::BindingMismatch {
                field: "canonical_inputs",
            });
        }
        validate_context_inputs(spec, request)?;
        validate_raw_input_shape(spec, request)?;
        let expected = inventory(spec, request)?.expectation;
        if expected.rules() != result.report().expected_rules() {
            return Err(SerializationInspectionError::BindingMismatch {
                field: "expected_rules",
            });
        }
        if expected.outputs() != result.expected_outputs() {
            return Err(SerializationInspectionError::BindingMismatch {
                field: "expected_outputs",
            });
        }

        Ok(Self {
            spec,
            request: request.clone(),
            canonical_inputs: result.canonical_inputs().to_vec(),
            derived_outputs: result.derived_outputs().to_vec(),
        })
    }

    /// Independently evaluates one selected contract presence branch for the
    /// exact singleton or repeated-row occurrence.
    pub fn evaluate_presence(
        &mut self,
        presence: SerializationPresence,
        current_group: Option<&RepeatedGroupInstance>,
    ) -> Result<bool, SerializationInspectionError> {
        self.validate_current_group(current_group)?;
        match presence {
            SerializationPresence::Always => Ok(true),
            SerializationPresence::Omitted => Ok(false),
            SerializationPresence::When(predicate) => {
                let mut environment = self.environment(current_group);
                evaluate_predicate(&predicate, &mut environment).map_err(Into::into)
            }
        }
    }

    /// Resolves one contract projection to its complete source identity,
    /// including the repeated-group ID and stable instance ID.
    pub fn resolve_value_source(
        &mut self,
        projection: SerializationValueProjection,
        current_group: Option<&RepeatedGroupInstance>,
    ) -> Result<MaterializedValueSourceView, SerializationInspectionError> {
        self.validate_current_group(current_group)?;
        match projection {
            SerializationValueProjection::Field(field) => {
                let instance = {
                    let environment = self.environment(current_group);
                    resolve_field_ref(field, &environment)?
                };
                if !self
                    .canonical_inputs
                    .iter()
                    .any(|candidate| candidate.field() == &instance)
                {
                    return Err(InterpreterError::MissingInput { field: instance }.into());
                }
                Ok(MaterializedValueSourceView::Field { field: instance })
            }
            SerializationValueProjection::Derived {
                calculation_id,
                output_id,
                instance,
            } => {
                let resolved_instance = {
                    let environment = self.environment(current_group);
                    resolve_derived_instance(calculation_id, output_id, instance, &environment)?
                };
                let calculation_id = parse_calculation_id(calculation_id)?;
                let output_id = parse_output_id(output_id)?;
                if !self.derived_outputs.iter().any(|candidate| {
                    candidate.calculation_id() == &calculation_id
                        && candidate.output_id() == &output_id
                        && candidate.instance() == resolved_instance.as_ref()
                }) {
                    return Err(InterpreterError::MissingDerivedValue {
                        calculation_id,
                        output_id,
                        instance: resolved_instance,
                    }
                    .into());
                }
                Ok(MaterializedValueSourceView::Derived {
                    calculation_id,
                    output_id,
                    instance: resolved_instance,
                })
            }
            SerializationValueProjection::Context { context_value_id } => {
                if !self
                    .spec
                    .context_values
                    .iter()
                    .any(|candidate| candidate.context_value_id == context_value_id)
                {
                    return Err(StaticSpecError::InvalidReference {
                        kind: SpecItemKind::RuleSet,
                        value: "serialization-inspector",
                        target: context_value_id,
                    }
                    .into());
                }
                Ok(MaterializedValueSourceView::Context {
                    context_value_id: parse_context_value_id(context_value_id)?,
                })
            }
            SerializationValueProjection::Constant(value) => {
                Ok(MaterializedValueSourceView::Constant {
                    value: literal_value(value)?,
                })
            }
            SerializationValueProjection::Default(value) => {
                Ok(MaterializedValueSourceView::Default {
                    value: literal_value(value)?,
                })
            }
        }
    }

    fn validate_current_group(
        &self,
        current_group: Option<&RepeatedGroupInstance>,
    ) -> Result<(), SerializationInspectionError> {
        if let Some(instance) = current_group
            && self
                .request
                .raw_inputs()
                .repeated_group_instances()
                .binary_search(instance)
                .is_err()
        {
            return Err(SerializationInspectionError::CurrentGroupNotDeclared {
                instance: instance.clone(),
            });
        }
        Ok(())
    }

    fn environment(&mut self, current_group: Option<&RepeatedGroupInstance>) -> Environment<'_> {
        Environment {
            spec: self.spec,
            request: &self.request,
            canonical_inputs: &mut self.canonical_inputs,
            derived_outputs: &mut self.derived_outputs,
            current_group: current_group.cloned(),
        }
    }
}

fn transition_workflow_static(
    spec: &'static StaticRuleSetSpec,
    request: &EvaluationRequest,
    result: &EvaluationResult,
    current_state: &WorkflowStateId,
    action: WorkflowAction,
) -> Result<WorkflowTransitionResult, WorkflowTransitionError> {
    validate_static_spec(spec)?;
    if request.rule_set() != result.rule_set() {
        return Err(WorkflowTransitionError::BindingMismatch { field: "rule_set" });
    }
    if request.context() != result.context() {
        return Err(WorkflowTransitionError::BindingMismatch { field: "context" });
    }
    if request.input_revision() != result.input_revision() {
        return Err(WorkflowTransitionError::BindingMismatch {
            field: "input_revision",
        });
    }
    if request.context_fingerprint() != result.context_fingerprint() {
        return Err(WorkflowTransitionError::BindingMismatch {
            field: "context_fingerprint",
        });
    }

    // Never trust a caller-supplied result merely because its public binding
    // metadata matches. Re-evaluate the complete request so repeated groups,
    // canonical inputs, derived values, rule coverage, and violations all bind
    // the workflow guard and transition to the exact compiled snapshot.
    let inventory = inventory(spec, request)?;
    let output = execute(spec, request)?;
    let reevaluated = EvaluationResult::try_new(request, &inventory.expectation, output)?;
    if &reevaluated != result {
        return Err(WorkflowTransitionError::BindingMismatch {
            field: "evaluation_result",
        });
    }
    if !result.is_valid() {
        return Err(WorkflowTransitionError::EvaluationNotValid);
    }

    let workflow = match spec.workflow {
        Branch::Executable(workflow) => workflow,
        Branch::DocumentedOnly => {
            return Err(WorkflowTransitionError::Unavailable {
                state: BranchState::DocumentedOnly,
            });
        }
        Branch::Unresolved => {
            return Err(WorkflowTransitionError::Unavailable {
                state: BranchState::Unresolved,
            });
        }
    };
    let matching = workflow
        .transitions
        .iter()
        .filter(|transition| {
            transition.from_state == current_state.as_str() && transition.action == action
        })
        .collect::<Vec<_>>();
    if matching.len() != 1 {
        return Err(WorkflowTransitionError::TransitionSelection {
            matches: matching.len(),
        });
    }
    let transition = matching[0];
    if transition.evaluation_phase != request.context().phase() {
        return Err(WorkflowTransitionError::InvalidActionPhase {
            action,
            expected: transition.evaluation_phase,
            actual: request.context().phase(),
        });
    }
    let branch = match transition.profiles.select(request.context().profile()) {
        Branch::Executable(branch) => branch,
        branch => {
            return Err(WorkflowTransitionError::BranchUnavailable {
                transition_id: transition.transition_id,
                profile: request.context().profile(),
                state: branch.state(),
            });
        }
    };

    let mut canonical_inputs = result.canonical_inputs().to_vec();
    let mut derived_outputs = result.derived_outputs().to_vec();
    let mut environment = Environment {
        spec,
        request,
        canonical_inputs: &mut canonical_inputs,
        derived_outputs: &mut derived_outputs,
        current_group: None,
    };
    if !evaluate_predicate(branch.guard, &mut environment)? {
        return Err(WorkflowTransitionError::GuardRejected {
            transition_id: transition.transition_id,
        });
    }
    drop(environment);

    let mut state_effect = None;
    let mut notifications = Vec::new();
    for effect in branch.effects {
        match effect {
            Effect::SetWorkflowState { state_id } => {
                if state_effect.replace(*state_id).is_some() {
                    return Err(WorkflowTransitionError::DuplicateStateEffect {
                        transition_id: transition.transition_id,
                    });
                }
            }
            Effect::EmitNotification {
                channel,
                message,
                official_message,
            } => notifications.push(WorkflowNotification::new(
                *channel,
                *message,
                official_message.map(str::to_owned),
            )),
            other => {
                return Err(WorkflowTransitionError::UnsupportedEffect {
                    transition_id: transition.transition_id,
                    kind: other.kind(),
                });
            }
        }
    }
    let Some(state_effect) = state_effect else {
        return Err(WorkflowTransitionError::MissingStateEffect {
            transition_id: transition.transition_id,
        });
    };
    if state_effect != transition.to_state {
        return Err(WorkflowTransitionError::StateEffectMismatch {
            transition_id: transition.transition_id,
            expected: transition.to_state,
            actual: state_effect,
        });
    }

    Ok(WorkflowTransitionResult::new(
        result.rule_set().clone(),
        result.context(),
        result.input_revision(),
        result.context_fingerprint(),
        parse_workflow_transition_id(transition.transition_id)?,
        current_state.clone(),
        action,
        parse_workflow_state_id(transition.to_state)?,
        notifications,
    ))
}

/// The only generic provider over static IR. Its constructor is crate-private,
/// preserving the sealed reviewed-provider boundary.
#[doc(hidden)]
pub struct StaticCompiledRuleSet {
    identity: FormRevisionKey,
    spec: &'static StaticRuleSetSpec,
}

impl StaticCompiledRuleSet {
    // Generated modules are currently empty, so this remains intentionally
    // unused in non-test crate builds until the first reviewed snapshot lands.
    #[allow(dead_code)]
    pub(crate) const fn new(identity: FormRevisionKey, spec: &'static StaticRuleSetSpec) -> Self {
        Self { identity, spec }
    }
}

impl crate::provider::sealed::Sealed for StaticCompiledRuleSet {
    fn expected_evaluation(
        &self,
        request: &EvaluationRequest,
    ) -> Result<EvaluationExpectation, EvaluationError> {
        inventory(self.spec, request)
            .map(|inventory| inventory.expectation)
            .map_err(EvaluationError::Interpreter)
    }

    fn evaluate_compiled(
        &self,
        request: &EvaluationRequest,
    ) -> Result<EvaluationOutput, EvaluationError> {
        execute(self.spec, request).map_err(EvaluationError::Interpreter)
    }

    fn materialize_compiled(
        &self,
        request: &EvaluationRequest,
        artifact: &SerializationArtifactIdentity,
    ) -> Result<SerializationMaterialization, MaterializationError> {
        materialize_static(self.spec, &self.identity, request, artifact)
    }

    fn inspect_serialization_compiled(
        &self,
        request: &EvaluationRequest,
        result: &EvaluationResult,
    ) -> Result<SerializationInspector, SerializationInspectionError> {
        SerializationInspector::try_new(self.spec, request, result)
    }

    fn transition_workflow_compiled(
        &self,
        request: &EvaluationRequest,
        result: &EvaluationResult,
        current_state: &WorkflowStateId,
        action: WorkflowAction,
    ) -> Result<WorkflowTransitionResult, WorkflowTransitionError> {
        transition_workflow_static(self.spec, request, result, current_state, action)
    }
}

impl crate::CompiledRuleSet for StaticCompiledRuleSet {
    fn identity(&self) -> &FormRevisionKey {
        &self.identity
    }

    fn serialization_contract(&self) -> &'static crate::StaticSerializationContract {
        self.spec.serialization
    }
}

fn materialize_static(
    spec: &'static StaticRuleSetSpec,
    identity: &FormRevisionKey,
    request: &EvaluationRequest,
    selected_identity: &SerializationArtifactIdentity,
) -> Result<SerializationMaterialization, MaterializationError> {
    if request.rule_set() != identity {
        return Err(MaterializationError::RuleSetMismatch);
    }
    let contract = spec.serialization;
    if contract.contract_version != "1.0.0" {
        return Err(MaterializationError::UnsupportedContractVersion);
    }
    let contract_digest = contract
        .canonical_sha256
        .ok_or(MaterializationError::MissingContractDigest)
        .and_then(|value| {
            crate::Sha256Digest::parse(value)
                .map_err(|_| MaterializationError::InvalidContractDigest)
        })?;
    validate_materialization_phase(selected_identity.target(), request.context().phase())?;

    let matching = contract
        .artifacts
        .iter()
        .filter(|artifact| {
            artifact.target == selected_identity.target()
                && artifact.variant_id == selected_identity.variant().as_str()
        })
        .collect::<Vec<_>>();
    if matching.len() != 1 {
        return Err(MaterializationError::ArtifactSelection {
            matches: matching.len(),
        });
    }
    let artifact = matching[0];
    let plan = match artifact.branches.select(request.context().profile()) {
        Branch::Executable(plan) => plan,
        Branch::DocumentedOnly | Branch::Unresolved => {
            return Err(MaterializationError::BranchUnavailable {
                profile: request.context().profile(),
            });
        }
    };
    validate_materialization_plan(spec, artifact.artifact_id, request.context(), plan)?;

    let expectation = inventory(spec, request)
        .map_err(MaterializationError::Interpreter)?
        .expectation;
    let output = execute(spec, request).map_err(MaterializationError::Interpreter)?;
    let evaluation = crate::EvaluationResult::try_new(request, &expectation, output)
        .map_err(MaterializationError::Evaluation)?;
    let mut canonical_inputs = evaluation.canonical_inputs().to_vec();
    let mut derived_outputs = evaluation.derived_outputs().to_vec();
    let mut environment = Environment {
        spec,
        request,
        canonical_inputs: &mut canonical_inputs,
        derived_outputs: &mut derived_outputs,
        current_group: None,
    };
    let mut builder = MaterializationBuilder {
        environment: &mut environment,
        trace: Vec::new(),
        emission_ids: BTreeSet::new(),
        bindings: BTreeSet::new(),
        next_occurrence: BTreeMap::new(),
    };
    builder.materialize_plan(plan)?;

    Ok(SerializationMaterialization::new(
        identity.clone(),
        request.context(),
        request.input_revision(),
        request.context_fingerprint(),
        artifact.artifact_id.to_owned(),
        selected_identity.clone(),
        contract_digest,
        digest_serializable(
            b"bir-rules/serialization-raw-input/v1\0",
            request.raw_inputs(),
        ),
        digest_serializable(b"bir-rules/serialization-evaluation/v2\0", &evaluation),
        builder.trace,
    ))
}

fn validate_materialization_plan(
    spec: &StaticRuleSetSpec,
    artifact_id: &'static str,
    context: ValidationContext,
    plan: SerializationPlan,
) -> Result<(), MaterializationError> {
    let scope = StaticValidationScope {
        spec,
        profile: context.profile(),
        phase: context.phase(),
        owner_kind: SpecItemKind::RuleSet,
        owner_id: artifact_id,
        calculation: None,
        current_group: None,
        trigger_field_ids: &[],
    };
    let mut expected_ordinal = 1_u32;
    validate_materialization_nodes_static(plan.nodes, scope, None, &mut expected_ordinal)
}

fn validate_materialization_nodes_static(
    nodes: &'static [SerializationNode],
    scope: StaticValidationScope<'_>,
    group: Option<(&'static str, Option<usize>)>,
    expected_ordinal: &mut u32,
) -> Result<(), MaterializationError> {
    for node in nodes {
        let ordinal = node.ordinal();
        if ordinal != *expected_ordinal {
            return Err(MaterializationError::InvalidContractStructure { ordinal });
        }
        *expected_ordinal = expected_ordinal
            .checked_add(1)
            .ok_or(MaterializationError::InvalidContractStructure { ordinal })?;

        match *node {
            SerializationNode::PseudoXmlField(field) => {
                let value_type = validate_materialization_value_projection(
                    ordinal,
                    field.value_projection,
                    scope,
                )?;
                validate_materialization_format(ordinal, value_type, field.semantic_format)?;
                validate_materialization_presence(field.presence, scope)?;

                let key_is_indexed =
                    validate_materialization_key_projection(ordinal, field.key_projection, group)?;
                let occurrence_is_indexed = validate_materialization_occurrence_projection(
                    ordinal,
                    field.occurrence_projection,
                    group,
                )?;
                if key_is_indexed && occurrence_is_indexed {
                    return Err(MaterializationError::InvalidContractStructure { ordinal });
                }
                if group.is_some_and(|(_, maximum)| maximum != Some(1))
                    && !key_is_indexed
                    && !occurrence_is_indexed
                {
                    return Err(MaterializationError::InvalidContractStructure { ordinal });
                }
            }
            SerializationNode::MetadataElement(element) => {
                if !valid_metadata_tag(element.exact_tag)
                    || group.is_some_and(|(_, maximum)| maximum != Some(1))
                {
                    return Err(MaterializationError::InvalidContractStructure { ordinal });
                }
                let value_type = validate_materialization_value_projection(
                    ordinal,
                    element.value_projection,
                    scope,
                )?;
                validate_materialization_format(ordinal, value_type, element.semantic_format)?;
                validate_materialization_presence(element.presence, scope)?;
            }
            SerializationNode::ReviewedLiteral(literal) => {
                if literal.exact_bytes.is_empty() {
                    return Err(MaterializationError::InvalidContractStructure { ordinal });
                }
            }
            SerializationNode::DynamicGroup(dynamic) => {
                if group.is_some() {
                    return Err(MaterializationError::NestedDynamicGroup);
                }
                parse_group_id(dynamic.group_id).map_err(MaterializationError::Interpreter)?;
                let Some(declared) = find_group(scope.spec, dynamic.group_id) else {
                    return Err(MaterializationError::InvalidContractStructure { ordinal });
                };
                if dynamic.min_occurs != declared.min_occurs
                    || dynamic.max_occurs != declared.max_occurs
                    || !matches!(
                        dynamic.instance_order,
                        SerializationGroupInstanceOrder::StableInstanceIdAscending
                    )
                {
                    return Err(MaterializationError::InvalidContractStructure { ordinal });
                }
                validate_materialization_nodes_static(
                    dynamic.nodes,
                    scope.with_current_group(dynamic.group_id),
                    Some((dynamic.group_id, dynamic.max_occurs)),
                    expected_ordinal,
                )?;
            }
        }
    }
    Ok(())
}

fn validate_materialization_value_projection(
    ordinal: u32,
    projection: SerializationValueProjection,
    scope: StaticValidationScope<'_>,
) -> Result<ValueType, MaterializationError> {
    match projection {
        SerializationValueProjection::Field(field) => validate_field_ref(field, scope)
            .map(|field| field.value_type)
            .map_err(MaterializationError::Interpreter),
        SerializationValueProjection::Derived {
            calculation_id,
            output_id,
            instance,
        } => resolve_derived_output(scope, calculation_id, output_id, instance)
            .map_err(MaterializationError::Interpreter),
        SerializationValueProjection::Context { context_value_id } => {
            parse_context_value_id(context_value_id).map_err(MaterializationError::Interpreter)?;
            scope
                .spec
                .context_values
                .iter()
                .find(|candidate| candidate.context_value_id == context_value_id)
                .map(|context| context.value_type)
                .ok_or(MaterializationError::InvalidContractStructure { ordinal })
        }
        SerializationValueProjection::Constant(value)
        | SerializationValueProjection::Default(value) => literal_value(value)
            .map(|_| typed_value_type(value))
            .map_err(MaterializationError::Interpreter),
    }
}

fn validate_materialization_format(
    ordinal: u32,
    value_type: ValueType,
    format: SerializationSemanticFormat,
) -> Result<(), MaterializationError> {
    let sample = match value_type {
        ValueType::String => CanonicalValue::Text("x".to_string()),
        ValueType::Boolean => CanonicalValue::Boolean(true),
        ValueType::Integer => CanonicalValue::Integer(1),
        ValueType::Decimal => CanonicalValue::Decimal(
            ExactDecimal::try_from_parts(1, 0)
                .map_err(|_| MaterializationError::InvalidContractStructure { ordinal })?,
        ),
        ValueType::Date => CanonicalValue::Date(
            CanonicalDate::try_new(2000, 1, 1)
                .map_err(|_| MaterializationError::InvalidContractStructure { ordinal })?,
        ),
        ValueType::Null => {
            return Err(MaterializationError::InvalidContractStructure { ordinal });
        }
    };
    format_serialization_value(&sample, format)
        .map(|_| ())
        .map_err(MaterializationError::Formatting)
}

fn validate_materialization_presence(
    presence: SerializationPresence,
    scope: StaticValidationScope<'_>,
) -> Result<(), MaterializationError> {
    match presence {
        SerializationPresence::Always | SerializationPresence::Omitted => Ok(()),
        SerializationPresence::When(predicate) => {
            validate_predicate(&predicate, scope).map_err(MaterializationError::Interpreter)
        }
    }
}

fn validate_materialization_key_projection(
    ordinal: u32,
    projection: SerializationKeyProjection,
    group: Option<(&'static str, Option<usize>)>,
) -> Result<bool, MaterializationError> {
    match projection {
        SerializationKeyProjection::Exact(key) => {
            crate::XmlKey::parse(key)
                .map_err(|_| MaterializationError::InvalidContractStructure { ordinal })?;
            Ok(false)
        }
        SerializationKeyProjection::GroupIndexed(indexed) => {
            let Some((group_id, Some(maximum))) = group else {
                return Err(MaterializationError::InvalidContractStructure { ordinal });
            };
            if indexed.group_id != group_id || indexed.index_step == 0 || indexed.padding > 32 {
                return Err(MaterializationError::InvalidContractStructure { ordinal });
            }
            let endpoint = materialization_projection_endpoint(
                indexed.index_base,
                indexed.index_step,
                maximum,
                ordinal,
            )?;
            for value in [indexed.index_base, endpoint] {
                let key = format!(
                    "{}{:0width$}{}",
                    indexed.prefix,
                    value,
                    indexed.suffix,
                    width = indexed.padding as usize
                );
                crate::XmlKey::parse(key)
                    .map_err(|_| MaterializationError::InvalidContractStructure { ordinal })?;
            }
            Ok(true)
        }
    }
}

fn validate_materialization_occurrence_projection(
    ordinal: u32,
    projection: SerializationOccurrenceProjection,
    group: Option<(&'static str, Option<usize>)>,
) -> Result<bool, MaterializationError> {
    match projection {
        SerializationOccurrenceProjection::Fixed(value) if value > 0 => Ok(false),
        SerializationOccurrenceProjection::Fixed(_) => {
            Err(MaterializationError::InvalidContractStructure { ordinal })
        }
        SerializationOccurrenceProjection::GroupIndexed(indexed) => {
            let Some((group_id, Some(maximum))) = group else {
                return Err(MaterializationError::InvalidContractStructure { ordinal });
            };
            if indexed.group_id != group_id || indexed.index_base == 0 || indexed.index_step == 0 {
                return Err(MaterializationError::InvalidContractStructure { ordinal });
            }
            materialization_projection_endpoint(
                indexed.index_base,
                indexed.index_step,
                maximum,
                ordinal,
            )?;
            Ok(true)
        }
    }
}

fn materialization_projection_endpoint(
    base: u32,
    step: u32,
    maximum: usize,
    ordinal: u32,
) -> Result<u32, MaterializationError> {
    if maximum == 0 {
        return Ok(base);
    }
    let last_index = u32::try_from(maximum - 1)
        .map_err(|_| MaterializationError::InvalidContractStructure { ordinal })?;
    step.checked_mul(last_index)
        .and_then(|offset| base.checked_add(offset))
        .ok_or(MaterializationError::InvalidContractStructure { ordinal })
}

fn valid_metadata_tag(tag: &str) -> bool {
    let mut characters = tag.chars();
    characters
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.' | ':')
        })
}

fn validate_materialization_phase(
    target: SerializationArtifactTarget,
    phase: ValidationPhase,
) -> Result<(), MaterializationError> {
    let valid = match target {
        SerializationArtifactTarget::EditableSave | SerializationArtifactTarget::FinalizedSave => {
            phase == ValidationPhase::Save
        }
        SerializationArtifactTarget::EncryptedFinalCopy => phase == ValidationPhase::FinalCopy,
        SerializationArtifactTarget::SubmissionPayload => phase == ValidationPhase::Submit,
        SerializationArtifactTarget::HistoricalImportCompatibility => {
            return Err(MaterializationError::HistoricalImportUnsupported);
        }
    };
    if valid {
        Ok(())
    } else {
        Err(MaterializationError::PhaseMismatch {
            target,
            actual: phase,
        })
    }
}

struct MaterializationBuilder<'a, 'request> {
    environment: &'a mut Environment<'request>,
    trace: Vec<MaterializationTraceEntryView>,
    emission_ids: BTreeSet<ContractEmissionId>,
    bindings: BTreeSet<String>,
    next_occurrence: BTreeMap<String, u32>,
}

impl MaterializationBuilder<'_, '_> {
    fn materialize_plan(&mut self, plan: SerializationPlan) -> Result<(), MaterializationError> {
        for node in plan.nodes {
            self.materialize_node(*node, Vec::new(), None)?;
        }
        Ok(())
    }

    fn materialize_node(
        &mut self,
        node: SerializationNode,
        group_path: Vec<RepeatedGroupInstance>,
        group_index: Option<(&'static str, u32)>,
    ) -> Result<(), MaterializationError> {
        match node {
            SerializationNode::DynamicGroup(group) => {
                if !group_path.is_empty() {
                    return Err(MaterializationError::NestedDynamicGroup);
                }
                self.materialize_group(group)
            }
            SerializationNode::ReviewedLiteral(literal) => {
                let id = self.claim_id(literal.ordinal, group_path)?;
                self.trace.push(MaterializationTraceEntryView::Record(
                    MaterializedRecordView::new(
                        id,
                        MaterializedBindingView::ReviewedLiteral {
                            exact_bytes: literal.exact_bytes.to_vec(),
                        },
                        MaterializedValueSourceView::None,
                        MaterializedOmissionView::Emitted,
                        None,
                        None,
                        None,
                    ),
                ));
                Ok(())
            }
            SerializationNode::PseudoXmlField(field) => {
                let id = self.claim_id(field.ordinal, group_path.clone())?;
                let key = self.project_key(field.key_projection, group_index)?;
                let occurrence =
                    self.project_occurrence(field.occurrence_projection, group_index)?;
                self.claim_binding(format!("pseudo:{key}:{occurrence}"))?;
                self.materialize_value_record(
                    id,
                    MaterializedBindingView::PseudoXmlField {
                        key: key.clone(),
                        occurrence,
                    },
                    field.value_projection,
                    field.semantic_format,
                    field.body_codec,
                    field.presence,
                    BodyEncodingBoundary::PseudoXmlKey(
                        &crate::XmlKey::parse(key.clone())
                            .map_err(|_| MaterializationError::InvalidProjectedKey)?,
                    ),
                    Some((key, occurrence)),
                )
            }
            SerializationNode::MetadataElement(element) => {
                let id = self.claim_id(element.ordinal, group_path)?;
                self.claim_binding(format!("metadata:{}", element.exact_tag))?;
                self.materialize_value_record(
                    id,
                    MaterializedBindingView::MetadataElement {
                        exact_tag: element.exact_tag.to_owned(),
                    },
                    element.value_projection,
                    element.semantic_format,
                    element.body_codec,
                    element.presence,
                    BodyEncodingBoundary::MetadataTag(element.exact_tag),
                    None,
                )
            }
        }
    }

    fn materialize_group(&mut self, group: DynamicGroupNode) -> Result<(), MaterializationError> {
        let instances = self
            .environment
            .request
            .raw_inputs()
            .repeated_group_instances()
            .iter()
            .filter(|instance| instance.group_id().as_str() == group.group_id)
            .cloned()
            .collect::<Vec<_>>();
        if instances.len() < group.min_occurs
            || group
                .max_occurs
                .is_some_and(|maximum| instances.len() > maximum)
        {
            return Err(MaterializationError::GroupCardinality);
        }
        let accounting_id = self.claim_id(group.ordinal, Vec::new())?;
        self.trace
            .push(MaterializationTraceEntryView::GroupAccounting(
                GroupAccountingView::new(
                    accounting_id,
                    group.group_id.to_owned(),
                    instances.clone(),
                ),
            ));
        for (index, instance) in instances.into_iter().enumerate() {
            let index =
                u32::try_from(index).map_err(|_| MaterializationError::ProjectionOverflow)?;
            self.environment.current_group = Some(instance.clone());
            for node in group.nodes {
                self.materialize_node(
                    *node,
                    vec![instance.clone()],
                    Some((group.group_id, index)),
                )?;
            }
        }
        self.environment.current_group = None;
        Ok(())
    }

    fn materialize_value_record(
        &mut self,
        id: ContractEmissionId,
        binding: MaterializedBindingView,
        projection: SerializationValueProjection,
        format: crate::serialization_contract::SerializationSemanticFormat,
        codec: crate::serialization::BodyCodec,
        presence: SerializationPresence,
        boundary: BodyEncodingBoundary<'_>,
        occurrence: Option<(String, u32)>,
    ) -> Result<(), MaterializationError> {
        let present = match presence {
            SerializationPresence::Always => true,
            SerializationPresence::When(predicate) => {
                evaluate_predicate(&predicate, self.environment)
                    .map_err(MaterializationError::Interpreter)?
            }
            SerializationPresence::Omitted => false,
        };
        if !present {
            let omission = if matches!(presence, SerializationPresence::Omitted) {
                MaterializedOmissionView::ContractOmitted
            } else {
                MaterializedOmissionView::PresenceFalse
            };
            self.trace.push(MaterializationTraceEntryView::Record(
                MaterializedRecordView::new(
                    id,
                    binding,
                    self.source_view(projection)?,
                    omission,
                    None,
                    None,
                    None,
                ),
            ));
            return Ok(());
        }

        let (source, value) = self.resolve_value(projection)?;
        let formatted =
            format_serialization_value(&value, format).map_err(MaterializationError::Formatting)?;
        let (omission, semantic_body, encoded_body) = match formatted {
            FormattedSemanticValue::Omitted => (
                match value {
                    CanonicalValue::Absent => MaterializedOmissionView::SemanticAbsent,
                    CanonicalValue::Blank => MaterializedOmissionView::SemanticBlank,
                    _ => return Err(MaterializationError::MissingValue),
                },
                None,
                None,
            ),
            FormattedSemanticValue::Body(body) => {
                let encoded = codec
                    .encode_for_boundary(&body, boundary)
                    .map_err(MaterializationError::Formatting)?;
                if let Some((key, actual)) = occurrence {
                    let expected = self.next_occurrence.entry(key.clone()).or_insert(1);
                    if actual != *expected {
                        return Err(MaterializationError::OccurrenceGap {
                            key,
                            expected: *expected,
                            actual,
                        });
                    }
                    *expected = expected
                        .checked_add(1)
                        .ok_or(MaterializationError::ProjectionOverflow)?;
                }
                (MaterializedOmissionView::Emitted, Some(body), Some(encoded))
            }
        };
        self.trace.push(MaterializationTraceEntryView::Record(
            MaterializedRecordView::new(
                id,
                binding,
                source,
                omission,
                Some(value),
                semantic_body,
                encoded_body,
            ),
        ));
        Ok(())
    }

    fn resolve_value(
        &self,
        projection: SerializationValueProjection,
    ) -> Result<(MaterializedValueSourceView, CanonicalValue), MaterializationError> {
        match projection {
            SerializationValueProjection::Field(field) => {
                let instance = resolve_field_ref(field, self.environment)
                    .map_err(MaterializationError::Interpreter)?;
                let value = self
                    .environment
                    .canonical_inputs
                    .iter()
                    .find(|value| value.field() == &instance)
                    .map(|value| value.canonical().clone())
                    .ok_or(MaterializationError::MissingValue)?;
                Ok((
                    MaterializedValueSourceView::Field { field: instance },
                    value,
                ))
            }
            SerializationValueProjection::Derived {
                calculation_id,
                output_id,
                instance,
            } => {
                let calculation = parse_calculation_id(calculation_id)
                    .map_err(MaterializationError::Interpreter)?;
                let output =
                    parse_output_id(output_id).map_err(MaterializationError::Interpreter)?;
                let resolved_instance =
                    resolve_derived_instance(calculation_id, output_id, instance, self.environment)
                        .map_err(MaterializationError::Interpreter)?;
                let value = derived_value(calculation_id, output_id, instance, self.environment)
                    .map_err(MaterializationError::Interpreter)?;
                Ok((
                    MaterializedValueSourceView::Derived {
                        calculation_id: calculation,
                        output_id: output,
                        instance: resolved_instance,
                    },
                    value,
                ))
            }
            SerializationValueProjection::Context { context_value_id } => {
                let id = parse_context_value_id(context_value_id)
                    .map_err(MaterializationError::Interpreter)?;
                let value = self
                    .environment
                    .request
                    .context_values()
                    .get(&id)
                    .cloned()
                    .ok_or(MaterializationError::MissingValue)?;
                Ok((
                    MaterializedValueSourceView::Context {
                        context_value_id: id,
                    },
                    value,
                ))
            }
            SerializationValueProjection::Constant(value) => {
                let value = literal_value(value).map_err(MaterializationError::Interpreter)?;
                Ok((
                    MaterializedValueSourceView::Constant {
                        value: value.clone(),
                    },
                    value,
                ))
            }
            SerializationValueProjection::Default(value) => {
                let value = literal_value(value).map_err(MaterializationError::Interpreter)?;
                Ok((
                    MaterializedValueSourceView::Default {
                        value: value.clone(),
                    },
                    value,
                ))
            }
        }
    }

    fn source_view(
        &self,
        projection: SerializationValueProjection,
    ) -> Result<MaterializedValueSourceView, MaterializationError> {
        match projection {
            SerializationValueProjection::Field(field) => Ok(MaterializedValueSourceView::Field {
                field: resolve_field_ref(field, self.environment)
                    .map_err(MaterializationError::Interpreter)?,
            }),
            SerializationValueProjection::Derived {
                calculation_id,
                output_id,
                instance,
            } => {
                let resolved_instance =
                    resolve_derived_instance(calculation_id, output_id, instance, self.environment)
                        .map_err(MaterializationError::Interpreter)?;
                Ok(MaterializedValueSourceView::Derived {
                    calculation_id: parse_calculation_id(calculation_id)
                        .map_err(MaterializationError::Interpreter)?,
                    output_id: parse_output_id(output_id)
                        .map_err(MaterializationError::Interpreter)?,
                    instance: resolved_instance,
                })
            }
            SerializationValueProjection::Context { context_value_id } => {
                Ok(MaterializedValueSourceView::Context {
                    context_value_id: parse_context_value_id(context_value_id)
                        .map_err(MaterializationError::Interpreter)?,
                })
            }
            SerializationValueProjection::Constant(value) => {
                Ok(MaterializedValueSourceView::Constant {
                    value: literal_value(value).map_err(MaterializationError::Interpreter)?,
                })
            }
            SerializationValueProjection::Default(value) => {
                Ok(MaterializedValueSourceView::Default {
                    value: literal_value(value).map_err(MaterializationError::Interpreter)?,
                })
            }
        }
    }

    fn project_key(
        &self,
        projection: SerializationKeyProjection,
        group: Option<(&'static str, u32)>,
    ) -> Result<String, MaterializationError> {
        match projection {
            SerializationKeyProjection::Exact(key) => Ok(key.to_owned()),
            SerializationKeyProjection::GroupIndexed(indexed) => {
                let (group_id, index) = group.ok_or(MaterializationError::ProjectionOverflow)?;
                if group_id != indexed.group_id {
                    return Err(MaterializationError::ProjectionOverflow);
                }
                let value = indexed
                    .index_step
                    .checked_mul(index)
                    .and_then(|offset| indexed.index_base.checked_add(offset))
                    .ok_or(MaterializationError::ProjectionOverflow)?;
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

    fn project_occurrence(
        &self,
        projection: SerializationOccurrenceProjection,
        group: Option<(&'static str, u32)>,
    ) -> Result<u32, MaterializationError> {
        match projection {
            SerializationOccurrenceProjection::Fixed(value) if value > 0 => Ok(value),
            SerializationOccurrenceProjection::Fixed(_) => {
                Err(MaterializationError::ProjectionOverflow)
            }
            SerializationOccurrenceProjection::GroupIndexed(indexed) => {
                let (group_id, index) = group.ok_or(MaterializationError::ProjectionOverflow)?;
                if group_id != indexed.group_id {
                    return Err(MaterializationError::ProjectionOverflow);
                }
                indexed
                    .index_step
                    .checked_mul(index)
                    .and_then(|offset| indexed.index_base.checked_add(offset))
                    .filter(|value| *value > 0)
                    .ok_or(MaterializationError::ProjectionOverflow)
            }
        }
    }

    fn claim_id(
        &mut self,
        ordinal: u32,
        group_path: Vec<RepeatedGroupInstance>,
    ) -> Result<ContractEmissionId, MaterializationError> {
        let id = ContractEmissionId::new(ordinal, group_path);
        if !self.emission_ids.insert(id.clone()) {
            return Err(MaterializationError::DuplicateEmissionId { id });
        }
        Ok(id)
    }

    fn claim_binding(&mut self, binding: String) -> Result<(), MaterializationError> {
        if !self.bindings.insert(binding) {
            Err(MaterializationError::BindingCollision)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone)]
struct SelectedCalculation {
    spec: &'static CalculationSpec,
    branch: CalculationBranch,
    instance: Option<RepeatedGroupInstance>,
    outputs: Vec<&'static CalculationOutput>,
    write_mode: ScheduledOutputWriteMode,
}

#[derive(Clone)]
struct SelectedRule {
    spec: &'static RuleSpec,
    branch: RuleBranch,
    instance: Option<RepeatedGroupInstance>,
}

#[derive(Clone)]
enum SelectedFieldEventStep {
    Calculation(SelectedCalculation),
    Rule(SelectedRule),
}

enum ExecutionPlan {
    Batched {
        calculations: Vec<SelectedCalculation>,
        rules: Vec<SelectedRule>,
    },
    FieldEvent {
        steps: Vec<SelectedFieldEventStep>,
    },
}

struct Inventory {
    expectation: EvaluationExpectation,
    plan: ExecutionPlan,
    effect_mode: EffectEvaluationMode,
}

fn inventory(
    spec: &'static StaticRuleSetSpec,
    request: &EvaluationRequest,
) -> Result<Inventory, InterpreterError> {
    let context = request.context();
    validate_static_spec(spec)?;
    select_branch(
        spec.profile_status.select(context.profile()),
        SpecItemKind::RuleSet,
        "rule-set",
        context,
    )?;

    let mut expected_outputs = Vec::new();
    let (plan, rules) = if let Some(event_field) = request.event_field() {
        let program = spec
            .field_event_programs
            .iter()
            .find(|candidate| {
                candidate.phase == context.phase()
                    && candidate.trigger_field_id == event_field.field_id().as_str()
            })
            .ok_or_else(|| InterpreterError::MissingFieldEventProgram {
                phase: context.phase(),
                field: event_field.clone(),
            })?;
        let branch = select_branch(
            program.profiles.select(context.profile()),
            SpecItemKind::FieldEventProgram,
            program.trigger_field_id,
            context,
        )?;
        let mut steps = Vec::new();
        let mut selected_rules = Vec::new();
        for step in branch.steps {
            match step {
                FieldEventStep::Calculation {
                    calculation_id,
                    output_ids,
                    write_mode,
                } => {
                    let calculation = spec
                        .calculations
                        .iter()
                        .find(|candidate| candidate.calculation_id == *calculation_id)
                        .expect("validated field-event calculation reference");
                    let calculation_branch = select_branch(
                        calculation.profiles.select(context.profile()),
                        SpecItemKind::Calculation,
                        calculation.calculation_id,
                        context,
                    )?;
                    let outputs = output_ids
                        .iter()
                        .map(|output_id| {
                            calculation_branch
                                .outputs
                                .iter()
                                .find(|output| output.output_id == *output_id)
                                .expect("validated scheduled output reference")
                        })
                        .collect::<Vec<_>>();
                    let parsed_calculation_id = parse_calculation_id(calculation.calculation_id)?;
                    for instance in execution_instances(calculation.scope, request) {
                        if *write_mode == ScheduledOutputWriteMode::Insert {
                            for output in &outputs {
                                expected_outputs.push(DerivedOutputExpectation::new(
                                    parsed_calculation_id.clone(),
                                    parse_output_id(output.output_id)?,
                                    instance.clone(),
                                ));
                            }
                        }
                        let selected = SelectedCalculation {
                            spec: calculation,
                            branch: calculation_branch,
                            instance,
                            outputs: outputs.clone(),
                            write_mode: *write_mode,
                        };
                        steps.push(SelectedFieldEventStep::Calculation(selected));
                    }
                }
                FieldEventStep::Rule { rule_id } => {
                    let rule = spec
                        .rules
                        .iter()
                        .find(|candidate| candidate.rule_id == *rule_id)
                        .expect("validated field-event rule reference");
                    let rule_branch = select_branch(
                        rule.profiles.select(context.profile()),
                        SpecItemKind::Rule,
                        rule.rule_id,
                        context,
                    )?;
                    for instance in execution_instances(rule.scope, request) {
                        let selected = SelectedRule {
                            spec: rule,
                            branch: rule_branch,
                            instance,
                        };
                        selected_rules.push(selected.clone());
                        steps.push(SelectedFieldEventStep::Rule(selected));
                    }
                }
            }
        }
        (ExecutionPlan::FieldEvent { steps }, selected_rules)
    } else {
        let mut calculations = Vec::new();
        for calculation_id in spec.evaluation_order {
            let calculation = spec
                .calculations
                .iter()
                .find(|candidate| candidate.calculation_id == *calculation_id)
                .expect("validated evaluation order reference");
            if !entry_applies(calculation.phases, calculation.trigger_field_ids, request) {
                continue;
            }
            let branch = select_branch(
                calculation.profiles.select(context.profile()),
                SpecItemKind::Calculation,
                calculation.calculation_id,
                context,
            )?;
            let parsed_calculation_id = parse_calculation_id(calculation.calculation_id)?;
            for instance in execution_instances(calculation.scope, request) {
                for output in branch.outputs {
                    expected_outputs.push(DerivedOutputExpectation::new(
                        parsed_calculation_id.clone(),
                        parse_output_id(output.output_id)?,
                        instance.clone(),
                    ));
                }
                calculations.push(SelectedCalculation {
                    spec: calculation,
                    branch,
                    instance,
                    outputs: branch.outputs.iter().collect(),
                    write_mode: ScheduledOutputWriteMode::Insert,
                });
            }
        }

        let mut applicable_rules: Vec<_> = spec
            .rules
            .iter()
            .filter(|rule| entry_applies(rule.phases, rule.trigger_field_ids, request))
            .collect();
        applicable_rules.sort_by_key(|rule| (rule.order, rule.rule_id));
        let mut selected_rules = Vec::new();
        for rule in applicable_rules {
            let branch = select_branch(
                rule.profiles.select(context.profile()),
                SpecItemKind::Rule,
                rule.rule_id,
                context,
            )?;
            for instance in execution_instances(rule.scope, request) {
                selected_rules.push(SelectedRule {
                    spec: rule,
                    branch,
                    instance,
                });
            }
        }
        selected_rules.sort_by(|left, right| {
            (left.spec.order, left.spec.rule_id, left.instance.as_ref()).cmp(&(
                right.spec.order,
                right.spec.rule_id,
                right.instance.as_ref(),
            ))
        });
        (
            ExecutionPlan::Batched {
                calculations,
                rules: selected_rules.clone(),
            },
            selected_rules,
        )
    };

    let expected_rules = rules
        .iter()
        .map(|rule| {
            Ok(RuleExpectation::new(
                parse_rule_id(rule.spec.rule_id)?,
                rule.instance.clone(),
                rule.spec.order,
            ))
        })
        .collect::<Result<Vec<_>, InterpreterError>>()?;
    let mut expected_field_value_assignments = Vec::new();
    for rule in &rules {
        let execution =
            RuleExecution::new(parse_rule_id(rule.spec.rule_id)?, rule.instance.clone());
        for (effect_index, effect) in rule.branch.effects.iter().enumerate() {
            let Effect::SetRawFieldValue { field, value } = effect else {
                continue;
            };
            let target =
                resolve_field_ref_for_instance(*field, spec, request, rule.instance.as_ref())?;
            if request.raw_inputs().raw_value(&target).is_none() {
                return Err(InterpreterError::MissingInput { field: target });
            }
            expected_field_value_assignments.push(FieldValueAssignmentExpectation::new(
                execution.clone(),
                u32::try_from(effect_index).map_err(|_| InterpreterError::Overflow {
                    operation: ExecutionOperation::SetRawFieldValue,
                })?,
                target,
                (*value).to_raw_value(),
            ));
        }
    }
    let expectation = EvaluationExpectation::try_new_with_field_value_assignments(
        expected_rules,
        expected_outputs,
        expected_field_value_assignments,
    )
    .map_err(|error| {
        InterpreterError::InvalidStaticSpec(StaticSpecError::InvalidReference {
            kind: SpecItemKind::RuleSet,
            value: "expectation",
            target: match error {
                EvaluationError::DuplicateExpectedRule { .. } => "duplicate-rule",
                EvaluationError::ExpectedRuleOrderNotStrict { .. } => "rule-order",
                EvaluationError::DuplicateExpectedOutput { .. } => "duplicate-output",
                EvaluationError::DuplicateAssignmentEffect { .. } => "duplicate-assignment-effect",
                _ => "invalid-expectation",
            },
        })
    })?;

    Ok(Inventory {
        expectation,
        plan,
        effect_mode: select_branch(
            spec.effect_mode.select(context.profile()),
            SpecItemKind::EvaluationPolicy,
            "effect-evaluation-mode",
            context,
        )?,
    })
}

fn entry_applies(
    phases: &[ValidationPhase],
    trigger_field_ids: &[&str],
    request: &EvaluationRequest,
) -> bool {
    if !phases.contains(&request.context().phase()) {
        return false;
    }
    request
        .event_field()
        .is_none_or(|field| trigger_field_ids.contains(&field.field_id().as_str()))
}

fn execution_instances(
    scope: EvaluationScope,
    request: &EvaluationRequest,
) -> Vec<Option<RepeatedGroupInstance>> {
    match scope {
        EvaluationScope::Singleton => vec![None],
        EvaluationScope::EachGroup(group_id) => {
            if let Some(event_field) = request.event_field() {
                event_field
                    .group_path()
                    .iter()
                    .find(|instance| instance.group_id().as_str() == group_id)
                    .cloned()
                    .map(Some)
                    .into_iter()
                    .collect()
            } else {
                request
                    .raw_inputs()
                    .repeated_group_instances()
                    .iter()
                    .filter(|instance| instance.group_id().as_str() == group_id)
                    .cloned()
                    .map(Some)
                    .collect()
            }
        }
    }
}

fn select_branch<T: Copy>(
    branch: Branch<T>,
    kind: SpecItemKind,
    id: &'static str,
    context: ValidationContext,
) -> Result<T, InterpreterError> {
    match branch {
        Branch::Executable(value) => Ok(value),
        Branch::DocumentedOnly => Err(InterpreterError::BranchUnavailable {
            kind,
            id,
            state: BranchState::DocumentedOnly,
            context,
        }),
        Branch::Unresolved => Err(InterpreterError::BranchUnavailable {
            kind,
            id,
            state: BranchState::Unresolved,
            context,
        }),
    }
}

fn execute(
    spec: &'static StaticRuleSetSpec,
    request: &EvaluationRequest,
) -> Result<EvaluationOutput, InterpreterError> {
    let inventory = inventory(spec, request)?;
    validate_context_inputs(spec, request)?;
    validate_raw_input_shape(spec, request)?;
    let mut canonical_inputs = canonicalize_inputs(spec, request)?;
    let mut derived_outputs = Vec::new();
    let mut environment = Environment {
        spec,
        request,
        canonical_inputs: &mut canonical_inputs,
        derived_outputs: &mut derived_outputs,
        current_group: None,
    };

    let mut rule_state = RuleExecutionState::default();
    match &inventory.plan {
        ExecutionPlan::Batched {
            calculations,
            rules,
        } => {
            for calculation in calculations {
                execute_selected_calculation(calculation, false, &mut environment)?;
            }
            for rule in rules {
                execute_selected_rule(
                    rule,
                    false,
                    inventory.effect_mode,
                    &mut environment,
                    &mut rule_state,
                )?;
            }
        }
        ExecutionPlan::FieldEvent { steps } => {
            for step in steps {
                match step {
                    SelectedFieldEventStep::Calculation(calculation) => {
                        execute_selected_calculation(calculation, true, &mut environment)?;
                    }
                    SelectedFieldEventStep::Rule(rule) => {
                        execute_selected_rule(
                            rule,
                            true,
                            inventory.effect_mode,
                            &mut environment,
                            &mut rule_state,
                        )?;
                    }
                }
            }
        }
    }
    environment.current_group = None;

    drop(environment);
    Ok(EvaluationOutput::new_with_field_value_assignments(
        canonical_inputs,
        derived_outputs,
        rule_state.evaluated_rules,
        rule_state.violations,
        rule_state.field_value_assignments,
    ))
}

#[derive(Default)]
struct RuleExecutionState {
    evaluated_rules: Vec<RuleExecution>,
    violations: Vec<RuleViolation>,
    field_value_assignments: Vec<FieldValueAssignment>,
    effects_stopped: bool,
    current_rule: Option<RuleId>,
    occurrence: u32,
}

fn execute_selected_calculation(
    calculation: &SelectedCalculation,
    apply_writeback: bool,
    environment: &mut Environment<'_>,
) -> Result<(), InterpreterError> {
    environment.current_group = calculation.instance.clone();
    let condition = evaluate_predicate(calculation.branch.condition, environment)?;
    let calculation_id = parse_calculation_id(calculation.spec.calculation_id)?;
    for output in &calculation.outputs {
        let mut value = if condition {
            evaluate_expression(output.value, environment)?
        } else {
            CanonicalValue::Absent
        };
        if let Some(rounding) = output.rounding {
            for step in rounding {
                value = apply_output_rounding(value, *step)?;
            }
        }
        let output_id = parse_output_id(output.output_id)?;
        let derived = DerivedValue::new(
            calculation_id.clone(),
            output_id.clone(),
            calculation.instance.clone(),
            value.clone(),
        );
        match calculation.write_mode {
            ScheduledOutputWriteMode::Insert => {
                if environment.derived_outputs.iter().any(|candidate| {
                    candidate.calculation_id() == &calculation_id
                        && candidate.output_id() == &output_id
                        && candidate.instance() == calculation.instance.as_ref()
                }) {
                    return Err(InterpreterError::InvalidScheduledOutputWrite {
                        calculation_id: calculation_id.clone(),
                        output_id,
                        mode: calculation.write_mode,
                    });
                }
                environment.derived_outputs.push(derived);
            }
            ScheduledOutputWriteMode::Replace => {
                let Some(slot) = environment.derived_outputs.iter_mut().find(|candidate| {
                    candidate.calculation_id() == &calculation_id
                        && candidate.output_id() == &output_id
                        && candidate.instance() == calculation.instance.as_ref()
                }) else {
                    return Err(InterpreterError::InvalidScheduledOutputWrite {
                        calculation_id: calculation_id.clone(),
                        output_id,
                        mode: calculation.write_mode,
                    });
                };
                *slot = derived;
            }
        }
        // A false calculation condition is the reviewed source no-write path.
        // It still materializes the absent derived slot, but it must not route
        // that sentinel through a terminal formatter. An evaluated output that
        // becomes absent is different: the formatter rejects it below so
        // blank/NaN-producing expression paths cannot masquerade as no-write.
        if apply_writeback
            && condition
            && let Some(writeback) = output.writeback
        {
            apply_calculation_writeback(writeback, &value, environment)?;
        }
    }
    Ok(())
}

fn execute_selected_rule(
    selected: &SelectedRule,
    mutate_working_fields: bool,
    effect_mode: EffectEvaluationMode,
    environment: &mut Environment<'_>,
    state: &mut RuleExecutionState,
) -> Result<(), InterpreterError> {
    let rule_id = parse_rule_id(selected.spec.rule_id)?;
    if state.current_rule.as_ref() != Some(&rule_id) {
        state.current_rule = Some(rule_id.clone());
        state.occurrence = 0;
    }
    environment.current_group = selected.instance.clone();
    let matched = evaluate_predicate(selected.branch.predicate, environment)?;
    state.evaluated_rules.push(RuleExecution::new(
        rule_id.clone(),
        selected.instance.clone(),
    ));
    if !matched || state.effects_stopped {
        return Ok(());
    }

    let mut current_rule_blocked = false;
    for (effect_index, effect) in selected.branch.effects.iter().enumerate() {
        if current_rule_blocked
            && matches!(
                effect,
                Effect::EmitIssue { .. } | Effect::EmitNotification { .. }
            )
        {
            continue;
        }
        let emitted_blocking = apply_effect(
            effect,
            selected.spec,
            &rule_id,
            state.occurrence,
            u32::try_from(effect_index).map_err(|_| InterpreterError::Overflow {
                operation: ExecutionOperation::SetRawFieldValue,
            })?,
            mutate_working_fields,
            environment,
            &mut state.violations,
            &mut state.field_value_assignments,
        )?;
        if matches!(effect, Effect::EmitIssue { .. }) {
            state.occurrence =
                state
                    .occurrence
                    .checked_add(1)
                    .ok_or(InterpreterError::Overflow {
                        operation: ExecutionOperation::NormalizeField,
                    })?;
        }
        if emitted_blocking
            && effect_mode == EffectEvaluationMode::StopEffectsAfterFirstBlockingIssue
        {
            state.effects_stopped = true;
            current_rule_blocked = true;
        }
    }
    Ok(())
}

struct Environment<'request> {
    spec: &'static StaticRuleSetSpec,
    request: &'request EvaluationRequest,
    canonical_inputs: &'request mut Vec<CanonicalFieldValue>,
    derived_outputs: &'request mut Vec<DerivedValue>,
    current_group: Option<RepeatedGroupInstance>,
}

fn validate_context_inputs(
    spec: &StaticRuleSetSpec,
    request: &EvaluationRequest,
) -> Result<(), InterpreterError> {
    for value in request.context_values().values() {
        let Some(expected) = spec
            .context_values
            .iter()
            .find(|candidate| candidate.context_value_id == value.id().as_str())
        else {
            return Err(InterpreterError::UnexpectedContextValue {
                id: value.id().clone(),
            });
        };
        ensure_type(
            expected.value_type,
            value.value(),
            ExecutionOperation::ContextLookup,
        )?;
    }
    for expected in spec.context_values {
        if expected.required
            && request
                .context_values()
                .get(&parse_context_value_id(expected.context_value_id)?)
                .is_none()
        {
            return Err(InterpreterError::MissingContextValue {
                id: parse_context_value_id(expected.context_value_id)?,
            });
        }
    }
    Ok(())
}

fn validate_raw_input_shape(
    spec: &StaticRuleSetSpec,
    request: &EvaluationRequest,
) -> Result<(), InterpreterError> {
    for instance in request.raw_inputs().repeated_group_instances() {
        let Some(group) = find_group(spec, instance.group_id().as_str()) else {
            return Err(InterpreterError::UnexpectedGroupInstance {
                instance: instance.clone(),
            });
        };
        let actual = request
            .raw_inputs()
            .repeated_group_instances()
            .iter()
            .filter(|candidate| candidate.group_id().as_str() == group.group_id)
            .count();
        if actual < group.min_occurs || group.max_occurs.is_some_and(|maximum| actual > maximum) {
            return Err(InterpreterError::GroupCardinality {
                group_id: parse_group_id(group.group_id)?,
                minimum: group.min_occurs,
                maximum: group.max_occurs,
                actual,
            });
        }
    }
    for group in spec.field_groups {
        let actual = request
            .raw_inputs()
            .repeated_group_instances()
            .iter()
            .filter(|candidate| candidate.group_id().as_str() == group.group_id)
            .count();
        if actual < group.min_occurs || group.max_occurs.is_some_and(|maximum| actual > maximum) {
            return Err(InterpreterError::GroupCardinality {
                group_id: parse_group_id(group.group_id)?,
                minimum: group.min_occurs,
                maximum: group.max_occurs,
                actual,
            });
        }
    }

    for raw in request.raw_inputs().fields() {
        let Some(field) = find_field(spec, raw.field().field_id().as_str()) else {
            return Err(InterpreterError::UnexpectedInput {
                field: raw.field().clone(),
            });
        };
        match field.group_id {
            None if raw.field().group_path().is_empty() => {}
            Some(group_id)
                if raw.field().group_path().len() == 1
                    && raw.field().group_path()[0].group_id().as_str() == group_id => {}
            _ => {
                return Err(InterpreterError::FieldScopeMismatch {
                    field_id: raw.field().field_id().clone(),
                });
            }
        }
    }

    for field in spec.fields {
        let field_id = parse_field_id(field.field_id)?;
        match field.group_id {
            None => {
                let instance = FieldInstance::singleton(field_id);
                if request.raw_inputs().raw_value(&instance).is_none() {
                    return Err(InterpreterError::MissingInput { field: instance });
                }
            }
            Some(group_id) => {
                for group_instance in request
                    .raw_inputs()
                    .repeated_group_instances()
                    .iter()
                    .filter(|candidate| candidate.group_id().as_str() == group_id)
                {
                    let instance =
                        FieldInstance::try_new(field_id.clone(), vec![group_instance.clone()])
                            .expect("one validated group path cannot duplicate");
                    if request.raw_inputs().raw_value(&instance).is_none() {
                        return Err(InterpreterError::MissingInput { field: instance });
                    }
                }
            }
        }
    }
    Ok(())
}

fn canonicalize_inputs(
    spec: &StaticRuleSetSpec,
    request: &EvaluationRequest,
) -> Result<Vec<CanonicalFieldValue>, InterpreterError> {
    request
        .raw_inputs()
        .fields()
        .iter()
        .map(|raw| {
            let field = find_field(spec, raw.field().field_id().as_str()).ok_or_else(|| {
                InterpreterError::UnexpectedInput {
                    field: raw.field().clone(),
                }
            })?;
            let behavior = select_branch(
                field.behavior.select(request.context().profile()),
                SpecItemKind::Field,
                field.field_id,
                request.context(),
            )?;
            let mut normalized = normalize_raw(raw.value(), behavior.normalization)?;
            if request.event_field() == Some(raw.field()) {
                let matching_events = behavior
                    .event_normalization
                    .iter()
                    .filter(|event| event.phase == request.context().phase())
                    .collect::<Vec<_>>();
                if !matching_events.is_empty() && matches!(normalized, OwnedNormalizedInput::Absent)
                {
                    // A missing raw control cannot dispatch a DOM field event.
                    // Reject it before an empty-value coercion can silently
                    // turn the impossible event into a usable value.
                    return Err(InterpreterError::InvalidCoercion {
                        target: field.value_type,
                        reason: CoercionFailure::Empty,
                    });
                }
                for event in matching_events {
                    normalized = normalize_owned(normalized, event.normalization)?;
                }
            }
            let canonical = coerce_normalized(normalized, behavior.coercion)?;
            Ok(CanonicalFieldValue::new(
                raw.field().clone(),
                raw.value().clone(),
                canonical,
            ))
        })
        .collect()
}

#[derive(Clone, Copy)]
enum NormalizedInput<'value> {
    Absent,
    Text(&'value str),
}

fn normalize_raw(
    raw: &RawValue,
    pipeline: &[NormalizationStep],
) -> Result<OwnedNormalizedInput, InterpreterError> {
    match raw {
        RawValue::Absent => Ok(OwnedNormalizedInput::Absent),
        RawValue::Text(value) => Ok(OwnedNormalizedInput::Text(normalize_text(value, pipeline)?)),
    }
}

enum OwnedNormalizedInput {
    Absent,
    Text(String),
}

impl OwnedNormalizedInput {
    fn as_borrowed(&self) -> NormalizedInput<'_> {
        match self {
            Self::Absent => NormalizedInput::Absent,
            Self::Text(value) => NormalizedInput::Text(value),
        }
    }
}

fn normalize_owned(
    normalized: OwnedNormalizedInput,
    pipeline: &[NormalizationStep],
) -> Result<OwnedNormalizedInput, InterpreterError> {
    match normalized {
        OwnedNormalizedInput::Absent => Ok(OwnedNormalizedInput::Absent),
        OwnedNormalizedInput::Text(value) => {
            normalize_text(&value, pipeline).map(OwnedNormalizedInput::Text)
        }
    }
}

fn normalize_text(
    source: &str,
    pipeline: &[NormalizationStep],
) -> Result<String, InterpreterError> {
    let mut value = source.to_owned();
    for step in pipeline {
        value = match step {
            NormalizationStep::Trim { side } => match side {
                TrimSide::Both => value.trim().to_owned(),
                TrimSide::Start => value.trim_start().to_owned(),
                TrimSide::End => value.trim_end().to_owned(),
            },
            NormalizationStep::ChangeCase { case } => match case {
                LetterCase::Upper => value.to_uppercase(),
                LetterCase::Lower => value.to_lowercase(),
            },
            NormalizationStep::ReplaceLiteral { from, to } => value.replace(from, to),
            NormalizationStep::StripCharacters { characters } => value
                .chars()
                .filter(|character| !characters.contains(*character))
                .collect(),
            NormalizationStep::DigitsOnly => value
                .chars()
                .filter(char::is_ascii_digit)
                .collect::<String>(),
            NormalizationStep::NormalizeNewlines { style } => {
                let lf = value.replace("\r\n", "\n").replace('\r', "\n");
                match style {
                    NewlineStyle::Lf => lf,
                    NewlineStyle::Crlf => lf.replace('\n', "\r\n"),
                }
            }
            NormalizationStep::DateFormat { format } => {
                if let Some(date) = parse_date_any(&value) {
                    format_date(date, *format)
                } else {
                    value
                }
            }
            NormalizationStep::DecimalFormat { grouping, rounding } => {
                match value.parse::<ExactDecimal>() {
                    Ok(decimal) => {
                        let rounded = round_decimal(decimal, *rounding)?;
                        format_decimal(rounded, *grouping)
                    }
                    Err(_) => value,
                }
            }
            NormalizationStep::OfflineEbirMoneyRoundV1 => offline_ebir_money_round_v1(&value)?,
            NormalizationStep::OfflineEbirParseFloatFixedZeroV1 => {
                offline_ebir_parse_float_fixed_zero_v1(&value)?
            }
        };
    }
    Ok(value)
}

fn coerce_normalized(
    normalized: OwnedNormalizedInput,
    coercion: Coercion,
) -> Result<CanonicalValue, InterpreterError> {
    let borrowed = normalized.as_borrowed();
    match coercion {
        Coercion::String { on_empty } => coerce_string(borrowed, on_empty),
        Coercion::Decimal {
            decimal,
            grouping,
            on_empty,
            on_invalid,
        } => coerce_decimal(borrowed, decimal, grouping, on_empty, on_invalid),
        Coercion::Integer {
            on_empty,
            on_invalid,
        } => coerce_integer(borrowed, on_empty, on_invalid),
        Coercion::Boolean {
            true_values,
            false_values,
            on_empty,
            on_invalid,
        } => coerce_boolean(borrowed, true_values, false_values, on_empty, on_invalid),
        Coercion::Date {
            accepted_formats,
            on_empty,
            on_invalid,
        } => coerce_date(borrowed, accepted_formats, on_empty, on_invalid),
    }
}

fn null_for(input: NormalizedInput<'_>) -> CanonicalValue {
    match input {
        NormalizedInput::Absent => CanonicalValue::Absent,
        NormalizedInput::Text(_) => CanonicalValue::Blank,
    }
}

fn is_empty_input(input: NormalizedInput<'_>) -> bool {
    matches!(input, NormalizedInput::Absent | NormalizedInput::Text(""))
}

fn coerce_string(
    input: NormalizedInput<'_>,
    on_empty: StringEmptyPolicy,
) -> Result<CanonicalValue, InterpreterError> {
    if is_empty_input(input) {
        return match on_empty {
            StringEmptyPolicy::EmptyString => Ok(CanonicalValue::Text(String::new())),
            StringEmptyPolicy::Null => Ok(null_for(input)),
            StringEmptyPolicy::Error => Err(InterpreterError::InvalidCoercion {
                target: ValueType::String,
                reason: CoercionFailure::Empty,
            }),
        };
    }
    match input {
        NormalizedInput::Text(value) => Ok(CanonicalValue::Text(value.to_owned())),
        NormalizedInput::Absent => unreachable!("handled as empty"),
    }
}

fn coerce_decimal(
    input: NormalizedInput<'_>,
    policy: DecimalPolicy,
    grouping: InputGrouping,
    on_empty: NumericEmptyPolicy,
    on_invalid: InvalidValuePolicy,
) -> Result<CanonicalValue, InterpreterError> {
    if is_empty_input(input) {
        return match on_empty {
            NumericEmptyPolicy::Null => Ok(null_for(input)),
            NumericEmptyPolicy::Zero => Ok(CanonicalValue::Decimal(
                ExactDecimal::try_from_parts(0, 0).expect("zero decimal"),
            )),
            NumericEmptyPolicy::Error => Err(InterpreterError::InvalidCoercion {
                target: ValueType::Decimal,
                reason: CoercionFailure::Empty,
            }),
        };
    }
    let NormalizedInput::Text(raw) = input else {
        unreachable!("handled as empty");
    };
    let parsed = parse_decimal_input(raw, grouping);
    let decimal = match parsed {
        Ok(value) => value,
        Err(_reason) if on_invalid == InvalidValuePolicy::PreserveRaw => {
            return Ok(CanonicalValue::Text(raw.to_owned()));
        }
        Err(reason) => {
            return Err(InterpreterError::InvalidCoercion {
                target: ValueType::Decimal,
                reason,
            });
        }
    };
    let rounded = round_decimal(decimal, policy.rounding)?;
    let constrained = constrain_decimal(rounded, policy)?;
    Ok(CanonicalValue::Decimal(constrained))
}

fn coerce_integer(
    input: NormalizedInput<'_>,
    on_empty: NumericEmptyPolicy,
    on_invalid: InvalidValuePolicy,
) -> Result<CanonicalValue, InterpreterError> {
    if is_empty_input(input) {
        return match on_empty {
            NumericEmptyPolicy::Null => Ok(null_for(input)),
            NumericEmptyPolicy::Zero => Ok(CanonicalValue::Integer(0)),
            NumericEmptyPolicy::Error => Err(InterpreterError::InvalidCoercion {
                target: ValueType::Integer,
                reason: CoercionFailure::Empty,
            }),
        };
    }
    let NormalizedInput::Text(raw) = input else {
        unreachable!("handled as empty");
    };
    match raw.parse::<i128>() {
        Ok(value) => Ok(CanonicalValue::Integer(value)),
        Err(_) if on_invalid == InvalidValuePolicy::PreserveRaw => {
            Ok(CanonicalValue::Text(raw.to_owned()))
        }
        Err(_) => Err(InterpreterError::InvalidCoercion {
            target: ValueType::Integer,
            reason: CoercionFailure::InvalidSyntax,
        }),
    }
}

fn coerce_boolean(
    input: NormalizedInput<'_>,
    true_values: &[&str],
    false_values: &[&str],
    on_empty: BooleanEmptyPolicy,
    on_invalid: InvalidValuePolicy,
) -> Result<CanonicalValue, InterpreterError> {
    if is_empty_input(input) {
        return match on_empty {
            BooleanEmptyPolicy::Null => Ok(null_for(input)),
            BooleanEmptyPolicy::False => Ok(CanonicalValue::Boolean(false)),
            BooleanEmptyPolicy::Error => Err(InterpreterError::InvalidCoercion {
                target: ValueType::Boolean,
                reason: CoercionFailure::Empty,
            }),
        };
    }
    let NormalizedInput::Text(raw) = input else {
        unreachable!("handled as empty");
    };
    if true_values.contains(&raw) {
        Ok(CanonicalValue::Boolean(true))
    } else if false_values.contains(&raw) {
        Ok(CanonicalValue::Boolean(false))
    } else if on_invalid == InvalidValuePolicy::PreserveRaw {
        Ok(CanonicalValue::Text(raw.to_owned()))
    } else {
        Err(InterpreterError::InvalidCoercion {
            target: ValueType::Boolean,
            reason: CoercionFailure::UnknownBoolean,
        })
    }
}

fn coerce_date(
    input: NormalizedInput<'_>,
    accepted_formats: &[DateFormat],
    on_empty: DateEmptyPolicy,
    on_invalid: InvalidValuePolicy,
) -> Result<CanonicalValue, InterpreterError> {
    if is_empty_input(input) {
        return match on_empty {
            DateEmptyPolicy::Null => Ok(null_for(input)),
            DateEmptyPolicy::Error => Err(InterpreterError::InvalidCoercion {
                target: ValueType::Date,
                reason: CoercionFailure::Empty,
            }),
        };
    }
    let NormalizedInput::Text(raw) = input else {
        unreachable!("handled as empty");
    };
    if let Some(date) = accepted_formats
        .iter()
        .find_map(|format| parse_date(raw, *format))
    {
        Ok(CanonicalValue::Date(date))
    } else if on_invalid == InvalidValuePolicy::PreserveRaw {
        Ok(CanonicalValue::Text(raw.to_owned()))
    } else {
        Err(InterpreterError::InvalidCoercion {
            target: ValueType::Date,
            reason: CoercionFailure::InvalidDate,
        })
    }
}

fn evaluate_expression(
    expression: &Expression,
    environment: &mut Environment<'_>,
) -> Result<CanonicalValue, InterpreterError> {
    let (value, expected, operation) = match expression {
        Expression::Literal(value) => return literal_value(*value),
        Expression::Field { result_type, field } => (
            field_value(*field, environment)?,
            *result_type,
            ExecutionOperation::FieldLookup,
        ),
        Expression::Derived {
            result_type,
            calculation_id,
            output_id,
            instance,
        } => (
            derived_value(calculation_id, output_id, *instance, environment)?,
            *result_type,
            ExecutionOperation::DerivedLookup,
        ),
        Expression::Context {
            result_type,
            context_value_id,
        } => (
            context_value(context_value_id, environment)?,
            *result_type,
            ExecutionOperation::ContextLookup,
        ),
        Expression::Unary {
            result_type,
            operator,
            operand,
        } => (
            evaluate_unary(*operator, evaluate_expression(operand, environment)?)?,
            *result_type,
            match operator {
                UnaryOperator::Negate => ExecutionOperation::UnaryNegate,
                UnaryOperator::Absolute => ExecutionOperation::UnaryAbsolute,
                UnaryOperator::Length => ExecutionOperation::UnaryLength,
            },
        ),
        Expression::Binary {
            result_type,
            operator,
            division_policy,
            left,
            right,
        } => {
            let left = evaluate_expression(left, environment)?;
            let right = evaluate_expression(right, environment)?;
            (
                evaluate_binary(*operator, *division_policy, left, right)?,
                *result_type,
                binary_operation(*operator),
            )
        }
        Expression::Nary {
            result_type,
            operator,
            operands,
        } => (
            evaluate_nary(*operator, operands, environment)?,
            *result_type,
            nary_operation(*operator),
        ),
        Expression::Conditional {
            result_type,
            condition,
            when_true,
            when_false,
        } => (
            if evaluate_predicate(condition, environment)? {
                evaluate_expression(when_true, environment)?
            } else {
                evaluate_expression(when_false, environment)?
            },
            *result_type,
            ExecutionOperation::Compare,
        ),
        Expression::Coerce {
            result_type,
            input,
            coercion,
        } => {
            let input = evaluate_expression(input, environment)?;
            (
                coerce_canonical(input, *coercion)?,
                *result_type,
                ExecutionOperation::Coercion,
            )
        }
        Expression::SplitComponent {
            result_type,
            input,
            delimiter,
            index,
        } => (
            evaluate_split_component(evaluate_expression(input, environment)?, delimiter, *index)?,
            *result_type,
            ExecutionOperation::SplitComponent,
        ),
        Expression::JavaScriptParseIntRadix10 { result_type, input } => (
            evaluate_javascript_parse_int_radix10(evaluate_expression(input, environment)?)?,
            *result_type,
            ExecutionOperation::JavaScriptParseIntRadix10,
        ),
        Expression::JavaScriptDateLocalDay {
            result_type,
            year,
            month_index,
            day,
        } => {
            let year = evaluate_expression(year, environment)?;
            let month_index = evaluate_expression(month_index, environment)?;
            let day = evaluate_expression(day, environment)?;
            (
                evaluate_javascript_date_local_day(year, month_index, day)?,
                *result_type,
                ExecutionOperation::JavaScriptDateLocalDay,
            )
        }
        Expression::CanonicalLocalDateDay { result_type, input } => (
            evaluate_canonical_local_date_day(evaluate_expression(input, environment)?)?,
            *result_type,
            ExecutionOperation::CanonicalLocalDateDay,
        ),
        Expression::GroupAggregate {
            result_type,
            operator,
            group_id,
            value,
        } => (
            evaluate_group_aggregate(*operator, group_id, value, environment)?,
            *result_type,
            ExecutionOperation::GroupAggregate,
        ),
    };
    ensure_type(expected, &value, operation)?;
    Ok(value)
}

fn literal_value(value: TypedValue) -> Result<CanonicalValue, InterpreterError> {
    match value {
        TypedValue::Null => Ok(CanonicalValue::Absent),
        TypedValue::String(value) => Ok(CanonicalValue::Text(value.to_owned())),
        TypedValue::Boolean(value) => Ok(CanonicalValue::Boolean(value)),
        TypedValue::Integer(value) => Ok(CanonicalValue::Integer(value)),
        TypedValue::Decimal(value) => ExactDecimal::try_from_parts(value.coefficient, value.scale)
            .map(CanonicalValue::Decimal)
            .map_err(|_| InterpreterError::Overflow {
                operation: ExecutionOperation::Coercion,
            }),
        TypedValue::Date(value) => CanonicalDate::try_new(value.year, value.month, value.day)
            .map(CanonicalValue::Date)
            .map_err(|_| InterpreterError::InvalidCoercion {
                target: ValueType::Date,
                reason: CoercionFailure::InvalidDate,
            }),
    }
}

fn evaluate_split_component(
    input: CanonicalValue,
    delimiter: &str,
    index: u32,
) -> Result<CanonicalValue, InterpreterError> {
    let input = match input {
        CanonicalValue::Absent | CanonicalValue::Blank => "",
        CanonicalValue::Text(value) => {
            return Ok(value
                .split(delimiter)
                .nth(index as usize)
                .map_or(CanonicalValue::Absent, |part| {
                    CanonicalValue::Text(part.to_owned())
                }));
        }
        actual => {
            return Err(InterpreterError::TypeMismatch {
                operation: ExecutionOperation::SplitComponent,
                expected: ValueType::String,
                actual: ValueKind::of(&actual),
            });
        }
    };
    Ok(input
        .split(delimiter)
        .nth(index as usize)
        .map_or(CanonicalValue::Absent, |part| {
            CanonicalValue::Text(part.to_owned())
        }))
}

fn is_ecmascript_string_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'
            ..='\u{000d}'
                | '\u{0020}'
                | '\u{00a0}'
                | '\u{1680}'
                | '\u{2000}'..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

fn evaluate_javascript_parse_int_radix10(
    input: CanonicalValue,
) -> Result<CanonicalValue, InterpreterError> {
    let input = match input {
        CanonicalValue::Absent | CanonicalValue::Blank => return Ok(CanonicalValue::Absent),
        CanonicalValue::Text(value) => value,
        actual => {
            return Err(InterpreterError::TypeMismatch {
                operation: ExecutionOperation::JavaScriptParseIntRadix10,
                expected: ValueType::String,
                actual: ValueKind::of(&actual),
            });
        }
    };
    let trimmed = input.trim_start_matches(is_ecmascript_string_whitespace);
    let (negative, digits) = match trimmed.as_bytes().first() {
        Some(b'+') => (false, &trimmed[1..]),
        Some(b'-') => (true, &trimmed[1..]),
        _ => (false, trimmed),
    };
    let digit_count = digits
        .as_bytes()
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digit_count == 0 {
        return Ok(CanonicalValue::Absent);
    }
    let mut number =
        digits[..digit_count]
            .parse::<f64>()
            .map_err(|_| InterpreterError::Overflow {
                operation: ExecutionOperation::JavaScriptParseIntRadix10,
            })?;
    if negative {
        number = -number;
    }
    // JavaScript parseInt returns an IEEE-754 Number. Every finite integer in
    // this range has an exact i128 representation. Infinity and finite values
    // outside the IR integer domain are deliberately mapped to the same absent
    // sentinel as NaN: neither is a usable integer, Date construction becomes
    // Invalid Date, and integer equality is false, matching every observation
    // available to the typed rule language without aborting evaluation.
    let i128_limit = 2_f64.powi(127);
    if !number.is_finite() || number >= i128_limit || number < -i128_limit {
        return Ok(CanonicalValue::Absent);
    }
    Ok(CanonicalValue::Integer(number as i128))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LegacyJavaScriptNumber {
    NaN,
    NegativeInfinity,
    Finite {
        coefficient: BigInt,
        decimal_exponent: BigInt,
    },
    PositiveInfinity,
}

impl LegacyJavaScriptNumber {
    fn zero() -> Self {
        Self::Finite {
            coefficient: BigInt::from(0_u8),
            decimal_exponent: BigInt::from(0_u8),
        }
    }

    fn compare(&self, other: &Self) -> Option<Ordering> {
        use LegacyJavaScriptNumber::{Finite, NaN, NegativeInfinity, PositiveInfinity};

        Some(match (self, other) {
            (NaN, _) | (_, NaN) => return None,
            (NegativeInfinity, NegativeInfinity) => Ordering::Equal,
            (NegativeInfinity, _) => Ordering::Less,
            (_, NegativeInfinity) => Ordering::Greater,
            (PositiveInfinity, PositiveInfinity) => Ordering::Equal,
            (PositiveInfinity, _) => Ordering::Greater,
            (_, PositiveInfinity) => Ordering::Less,
            (
                Finite {
                    coefficient: left,
                    decimal_exponent: left_exponent,
                },
                Finite {
                    coefficient: right,
                    decimal_exponent: right_exponent,
                },
            ) => compare_exact_javascript_finite(left, left_exponent, right, right_exponent),
        })
    }
}

fn compare_exact_javascript_finite(
    left: &BigInt,
    left_exponent: &BigInt,
    right: &BigInt,
    right_exponent: &BigInt,
) -> Ordering {
    let left_sign = left.sign();
    let right_sign = right.sign();
    match (left_sign, right_sign) {
        (Sign::Minus, Sign::Minus) => {
            compare_exact_javascript_magnitude(left, left_exponent, right, right_exponent).reverse()
        }
        (Sign::Minus, _) => Ordering::Less,
        (_, Sign::Minus) => Ordering::Greater,
        (Sign::NoSign, Sign::NoSign) => Ordering::Equal,
        (Sign::NoSign, Sign::Plus) => Ordering::Less,
        (Sign::Plus, Sign::NoSign) => Ordering::Greater,
        (Sign::Plus, Sign::Plus) => {
            compare_exact_javascript_magnitude(left, left_exponent, right, right_exponent)
        }
    }
}

fn compare_exact_javascript_magnitude(
    left: &BigInt,
    left_exponent: &BigInt,
    right: &BigInt,
    right_exponent: &BigInt,
) -> Ordering {
    let left_text = left.to_str_radix(10);
    let right_text = right.to_str_radix(10);
    let left_digits = left_text.strip_prefix('-').unwrap_or(&left_text);
    let right_digits = right_text.strip_prefix('-').unwrap_or(&right_text);
    let left_order = left_exponent.clone() + BigInt::from(left_digits.len());
    let right_order = right_exponent.clone() + BigInt::from(right_digits.len());
    match left_order.cmp(&right_order) {
        Ordering::Equal => {
            let width = left_digits.len().max(right_digits.len());
            for index in 0..width {
                let left_digit = left_digits.as_bytes().get(index).copied().unwrap_or(b'0');
                let right_digit = right_digits.as_bytes().get(index).copied().unwrap_or(b'0');
                match left_digit.cmp(&right_digit) {
                    Ordering::Equal => {}
                    ordering => return ordering,
                }
            }
            Ordering::Equal
        }
        ordering => ordering,
    }
}

fn legacy_javascript_number(input: &str) -> LegacyJavaScriptNumber {
    let input = input.trim_matches(is_ecmascript_string_whitespace);
    if input.is_empty() {
        return LegacyJavaScriptNumber::zero();
    }
    match input {
        "Infinity" | "+Infinity" => return LegacyJavaScriptNumber::PositiveInfinity,
        "-Infinity" => return LegacyJavaScriptNumber::NegativeInfinity,
        _ => {}
    }

    if let Some(hexadecimal) = input
        .strip_prefix("0x")
        .or_else(|| input.strip_prefix("0X"))
    {
        if hexadecimal.is_empty() || !hexadecimal.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return LegacyJavaScriptNumber::NaN;
        }
        return BigInt::parse_bytes(hexadecimal.as_bytes(), 16).map_or(
            LegacyJavaScriptNumber::NaN,
            |coefficient| LegacyJavaScriptNumber::Finite {
                coefficient,
                decimal_exponent: BigInt::from(0_u8),
            },
        );
    }

    let bytes = input.as_bytes();
    let mut cursor = 0;
    let negative = match bytes.first() {
        Some(b'+') => {
            cursor = 1;
            false
        }
        Some(b'-') => {
            cursor = 1;
            true
        }
        _ => false,
    };
    let integer_start = cursor;
    while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
        cursor += 1;
    }
    let integer_end = cursor;
    let mut fraction_start = cursor;
    let mut fraction_end = cursor;
    if bytes.get(cursor) == Some(&b'.') {
        cursor += 1;
        fraction_start = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        fraction_end = cursor;
    }
    if integer_start == integer_end && fraction_start == fraction_end {
        return LegacyJavaScriptNumber::NaN;
    }

    let mut exponent = BigInt::from(0_u8);
    if matches!(bytes.get(cursor), Some(b'e') | Some(b'E')) {
        cursor += 1;
        let exponent_negative = match bytes.get(cursor) {
            Some(b'+') => {
                cursor += 1;
                false
            }
            Some(b'-') => {
                cursor += 1;
                true
            }
            _ => false,
        };
        let exponent_start = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        if exponent_start == cursor {
            return LegacyJavaScriptNumber::NaN;
        }
        let Some(parsed_exponent) = BigInt::parse_bytes(&bytes[exponent_start..cursor], 10) else {
            return LegacyJavaScriptNumber::NaN;
        };
        exponent = if exponent_negative {
            -parsed_exponent
        } else {
            parsed_exponent
        };
    }
    if cursor != bytes.len() {
        return LegacyJavaScriptNumber::NaN;
    }

    let mut digits =
        String::with_capacity((integer_end - integer_start) + (fraction_end - fraction_start));
    digits.push_str(&input[integer_start..integer_end]);
    digits.push_str(&input[fraction_start..fraction_end]);
    let Some(mut coefficient) = BigInt::parse_bytes(digits.as_bytes(), 10) else {
        return LegacyJavaScriptNumber::NaN;
    };
    if negative {
        coefficient = -coefficient;
    }
    LegacyJavaScriptNumber::Finite {
        coefficient,
        decimal_exponent: exponent - BigInt::from(fraction_end - fraction_start),
    }
}

/// Reproduces the finite behavior of the hash-pinned Offline eBIRForms
/// `round(number, 2)` helper at its legacy JavaScript binary64 boundary.
///
/// The resource guard is deliberately separate from the official helper's
/// lexical-width test. It bounds hostile non-DOM inputs without changing any
/// reviewed form control limit. The official helper emits malformed strings
/// after non-finite arithmetic or exponential `Number#toString` output. Those
/// strings are deliberately rejected at the typed IR boundary.
fn offline_ebir_money_round_v1(source: &str) -> Result<String, InterpreterError> {
    if source.encode_utf16().count() > 4_096 {
        return Err(InterpreterError::Overflow {
            operation: ExecutionOperation::NormalizeField,
        });
    }

    let cleaned = source
        .chars()
        .filter(|character| !matches!(character, '$' | ','))
        .collect::<String>();
    let permitted_width = match cleaned.find('.') {
        Some(index) if index > 0 => cleaned[..index].encode_utf16().count() <= 12,
        _ => cleaned.encode_utf16().count() <= 12,
    };
    if !permitted_width {
        return Ok("0.00".to_owned());
    }

    offline_ebir_format_numeric_string(&cleaned, ExecutionOperation::NormalizeField)
}

fn offline_ebir_format_numeric_string(
    cleaned: &str,
    operation: ExecutionOperation,
) -> Result<String, InterpreterError> {
    let number = match legacy_javascript_number(cleaned) {
        LegacyJavaScriptNumber::NaN => 0.0,
        LegacyJavaScriptNumber::NegativeInfinity | LegacyJavaScriptNumber::PositiveInfinity => {
            return Err(InterpreterError::Overflow { operation });
        }
        LegacyJavaScriptNumber::Finite { .. } => {
            let input = cleaned.trim_matches(is_ecmascript_string_whitespace);
            if input.is_empty() {
                0.0
            } else if let Some(hexadecimal) = input
                .strip_prefix("0x")
                .or_else(|| input.strip_prefix("0X"))
            {
                u64::from_str_radix(hexadecimal, 16)
                    .expect("the legacy-number grammar and lexical-width gate bound valid hex")
                    as f64
            } else {
                input
                    .parse::<f64>()
                    .expect("the legacy-number grammar accepts a decimal Rust also parses")
            }
        }
    };
    if !number.is_finite() {
        return Err(InterpreterError::Overflow { operation });
    }

    // The source compares the original value with `Math.abs` using abstract
    // equality. Negative zero therefore remains unsigned, while any other
    // finite negative value retains its sign even when it rounds to zero.
    let negative = number.is_sign_negative() && number != 0.0;
    let scaled = number.abs() * 100.0;
    let rounded = (scaled + 0.50000000001).floor();
    if !rounded.is_finite() {
        return Err(InterpreterError::Overflow { operation });
    }

    let cents = rounded % 100.0;
    let whole = (rounded / 100.0).floor();
    if !cents.is_finite()
        || cents.fract() != 0.0
        || !(0.0..100.0).contains(&cents)
        || !whole.is_finite()
    {
        return Err(InterpreterError::Overflow { operation });
    }

    let whole = javascript_finite_number_to_string(whole);
    if whole.is_empty() || !whole.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(InterpreterError::Overflow { operation });
    }
    Ok(format_offline_ebir_money_cents(
        &whole,
        cents as u8,
        negative,
    ))
}

/// Reproduces the reviewed `formatCurrency` calculation-write path for values
/// representable by the reviewed finite, non-exponential numeric IR domain,
/// including source-order truncation, binary64 rounding, and signed-zero
/// behavior.
///
/// The official helper also maps blank/NaN tokens to zero and emits malformed
/// `In,fin,ity.NaN` strings for infinities. Those observations are
/// recorded-only: textual, blank, and non-finite calculation outputs fail
/// closed until their external-DOM or save/reopen reachability is independently
/// reviewed. Magnitudes whose binary64 representation reaches `1e21` also fail
/// closed: legacy `Number#toString` switches to exponent notation there, and
/// that path is outside the reviewed canonical output shape.
fn offline_ebir_format_currency_v1(value: &CanonicalValue) -> Result<String, InterpreterError> {
    let source = match value {
        CanonicalValue::Integer(value) => value.to_string(),
        CanonicalValue::Decimal(value) => value.to_string(),
        actual => {
            return Err(InterpreterError::TypeMismatch {
                operation: ExecutionOperation::CalculationWriteback,
                expected: ValueType::Decimal,
                actual: ValueKind::of(actual),
            });
        }
    };
    let binary64 = source
        .parse::<f64>()
        .map_err(|_| InterpreterError::Overflow {
            operation: ExecutionOperation::CalculationWriteback,
        })?;
    if !binary64.is_finite() || binary64.abs() >= 1e21 {
        return Err(InterpreterError::Overflow {
            operation: ExecutionOperation::CalculationWriteback,
        });
    }
    if source.encode_utf16().count() > 4_096 {
        return Err(InterpreterError::Overflow {
            operation: ExecutionOperation::CalculationWriteback,
        });
    }

    let mut cleaned = source
        .chars()
        .filter(|character| !matches!(character, '$' | ','))
        .collect::<String>();
    let comparison = legacy_javascript_number(&cleaned).compare(&LegacyJavaScriptNumber::zero());

    if let Some(dot) = cleaned.find('.') {
        if dot > 0 && comparison == Some(Ordering::Greater) {
            let integer = &cleaned[..dot];
            if integer.encode_utf16().count() > 15 {
                let fraction = cleaned[dot + 1..].split('.').next().unwrap_or_default();
                cleaned = format!("{}.{}", utf16_prefix(integer, 15), fraction);
            }
        } else if dot > 0 && comparison == Some(Ordering::Less) {
            let integer = &cleaned[..dot];
            if integer.encode_utf16().count() > 13 {
                let fraction = cleaned[dot + 1..].split('.').next().unwrap_or_default();
                cleaned = format!("{}.{}", utf16_prefix(integer, 15), fraction);
            }
        }
    } else if comparison == Some(Ordering::Greater) && cleaned.encode_utf16().count() > 15 {
        cleaned = utf16_prefix(&cleaned, 15);
    }

    offline_ebir_format_numeric_string(&cleaned, ExecutionOperation::CalculationWriteback)
}

fn utf16_prefix(value: &str, maximum_units: usize) -> String {
    value
        .chars()
        .scan(0_usize, |units, character| {
            let next = units.saturating_add(character.len_utf16());
            if next > maximum_units {
                None
            } else {
                *units = next;
                Some(character)
            }
        })
        .collect()
}

fn format_offline_ebir_money_cents(whole: &str, cents: u8, negative: bool) -> String {
    let mut grouped = String::with_capacity(whole.len() + whole.len() / 3);
    let first_group = whole.len() % 3;
    if first_group != 0 {
        grouped.push_str(&whole[..first_group]);
    }
    for chunk in whole.as_bytes()[first_group..].chunks(3) {
        if !grouped.is_empty() {
            grouped.push(',');
        }
        grouped.push_str(std::str::from_utf8(chunk).expect("decimal digits are UTF-8"));
    }
    if grouped.is_empty() {
        grouped.push('0');
    }

    format!("{}{grouped}.{cents:02}", if negative { "-" } else { "" })
}

/// Formats a finite non-negative IEEE-754 value using the decimal-placement
/// rules used by ECMAScript `Number#toString`. Rust's formatter supplies the
/// deterministic shortest round-tripping digits; this function applies the
/// legacy fixed-versus-exponential thresholds.
fn javascript_finite_number_to_string(value: f64) -> String {
    debug_assert!(value.is_finite() && value >= 0.0);
    if value == 0.0 {
        return "0".to_owned();
    }

    let rendered = value.to_string();
    let (mantissa, explicit_exponent) = rendered.split_once(['e', 'E']).map_or(
        (rendered.as_str(), 0_i32),
        |(mantissa, exponent)| {
            (
                mantissa,
                exponent
                    .parse::<i32>()
                    .expect("Rust formats a finite f64 with a bounded exponent"),
            )
        },
    );
    let fractional_digits = mantissa
        .split_once('.')
        .map_or(0_i32, |(_, fraction)| fraction.len() as i32);
    let mut digits = mantissa
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(char::from)
        .collect::<String>();
    let leading_zeroes = digits.bytes().take_while(|byte| *byte == b'0').count();
    digits.drain(..leading_zeroes);
    let trailing_zeroes = digits
        .bytes()
        .rev()
        .take_while(|byte| *byte == b'0')
        .count();
    digits.truncate(digits.len() - trailing_zeroes);
    let decimal_exponent = explicit_exponent - fractional_digits + trailing_zeroes as i32;
    let decimal_position = digits.len() as i32 + decimal_exponent;

    if decimal_position > 0 && decimal_position <= 21 {
        if decimal_position as usize >= digits.len() {
            digits.extend(std::iter::repeat_n(
                '0',
                decimal_position as usize - digits.len(),
            ));
            digits
        } else {
            digits.insert(decimal_position as usize, '.');
            digits
        }
    } else if decimal_position <= 0 && decimal_position > -6 {
        format!("0.{}{}", "0".repeat((-decimal_position) as usize), digits)
    } else {
        let exponent = decimal_position - 1;
        let mut characters = digits.chars();
        let first = characters
            .next()
            .expect("a finite nonzero number has significant digits");
        let remainder = characters.collect::<String>();
        let coefficient = if remainder.is_empty() {
            first.to_string()
        } else {
            format!("{first}.{remainder}")
        };
        format!(
            "{coefficient}e{}{exponent}",
            if exponent >= 0 { "+" } else { "" }
        )
    }
}

/// Reproduces the hash-pinned `blockletterWithout2Decimal` helper used by
/// whole-number text controls. The legacy helper parses a numeric prefix,
/// clears NaN, and renders a zero-decimal fixed string.
fn offline_ebir_parse_float_fixed_zero_v1(source: &str) -> Result<String, InterpreterError> {
    if source.encode_utf16().count() > 4_096 {
        return Err(InterpreterError::Overflow {
            operation: ExecutionOperation::NormalizeField,
        });
    }

    let number = javascript_parse_float(source);
    if number.is_nan() {
        return Ok(String::new());
    }
    if !number.is_finite() {
        return Err(InterpreterError::Overflow {
            operation: ExecutionOperation::NormalizeField,
        });
    }
    if number == 0.0 {
        return Ok("0".to_owned());
    }

    // Rust and legacy JavaScript both expose the shortest round-tripping
    // binary64 decimal. Rounding that decimal to an integer reproduces the
    // source-observed fixed-zero behavior, including full expansion of values
    // such as 1e99 rather than an exponent-form display.
    let rendered = number.abs().to_string();
    let (mantissa, explicit_exponent) = rendered.split_once(['e', 'E']).map_or(
        (rendered.as_str(), 0_i32),
        |(mantissa, exponent)| {
            (
                mantissa,
                exponent
                    .parse::<i32>()
                    .expect("Rust formats a finite f64 with a bounded exponent"),
            )
        },
    );
    let fractional_digits = mantissa
        .split_once('.')
        .map_or(0_i32, |(_, fraction)| fraction.len() as i32);
    let digits = mantissa
        .bytes()
        .filter(u8::is_ascii_digit)
        .collect::<Vec<_>>();
    let coefficient = BigInt::parse_bytes(&digits, 10)
        .expect("a finite nonzero f64 formatter emits decimal digits");
    let decimal_exponent = explicit_exponent - fractional_digits;
    let rounded = if decimal_exponent >= 0 {
        coefficient
            * BigInt::from(10_u8)
                .pow(u32::try_from(decimal_exponent).expect("finite f64 exponent is nonnegative"))
    } else {
        let denominator = BigInt::from(10_u8).pow(
            u32::try_from(-decimal_exponent).expect("finite f64 exponent magnitude is bounded"),
        );
        let quotient = &coefficient / &denominator;
        let remainder = &coefficient % &denominator;
        if remainder * BigInt::from(2_u8) >= denominator {
            quotient + BigInt::from(1_u8)
        } else {
            quotient
        }
    };
    Ok(format!(
        "{}{}",
        if number.is_sign_negative() { "-" } else { "" },
        rounded.to_str_radix(10)
    ))
}

fn javascript_string_or_null_truthy(
    value: &CanonicalValue,
    operation: ExecutionOperation,
) -> Result<bool, InterpreterError> {
    match value {
        CanonicalValue::Absent | CanonicalValue::Blank => Ok(false),
        CanonicalValue::Text(value) => Ok(!value.is_empty()),
        actual => Err(InterpreterError::TypeMismatch {
            operation,
            expected: ValueType::String,
            actual: ValueKind::of(actual),
        }),
    }
}

fn select_javascript_logical_or_value(
    values: &[CanonicalValue],
    operation: ExecutionOperation,
) -> Result<&CanonicalValue, InterpreterError> {
    let Some(final_value) = values.last() else {
        return Err(InterpreterError::InvalidStaticSpec(
            StaticSpecError::EmptyRequiredList {
                kind: SpecItemKind::Rule,
                value: "javascript-global-is-nan-logical-or",
            },
        ));
    };
    for value in values {
        if javascript_string_or_null_truthy(value, operation)? {
            return Ok(value);
        }
    }
    Ok(final_value)
}

fn legacy_javascript_number_from_string_or_null(
    value: CanonicalValue,
    operation: ExecutionOperation,
) -> Result<LegacyJavaScriptNumber, InterpreterError> {
    match value {
        CanonicalValue::Absent | CanonicalValue::Blank => Ok(LegacyJavaScriptNumber::zero()),
        CanonicalValue::Text(value) => Ok(legacy_javascript_number(&value)),
        actual => Err(InterpreterError::TypeMismatch {
            operation,
            expected: ValueType::String,
            actual: ValueKind::of(&actual),
        }),
    }
}

fn exact_javascript_number_operand(
    value: CanonicalValue,
) -> Result<Option<LegacyJavaScriptNumber>, InterpreterError> {
    let number = match value {
        CanonicalValue::Absent | CanonicalValue::Blank => return Ok(None),
        CanonicalValue::Integer(value) => LegacyJavaScriptNumber::Finite {
            coefficient: BigInt::from(value),
            decimal_exponent: BigInt::from(0_u8),
        },
        CanonicalValue::Decimal(value) => LegacyJavaScriptNumber::Finite {
            coefficient: BigInt::from(value.coefficient()),
            decimal_exponent: -BigInt::from(value.scale()),
        },
        actual => {
            return Err(InterpreterError::TypeMismatch {
                operation: ExecutionOperation::JavaScriptNumberCompare,
                expected: ValueType::Decimal,
                actual: ValueKind::of(&actual),
            });
        }
    };
    Ok(Some(number))
}

fn evaluate_javascript_number_compare(
    operator: JavaScriptNumberCompareOperator,
    input: CanonicalValue,
    operand: CanonicalValue,
) -> Result<bool, InterpreterError> {
    let input = legacy_javascript_number_from_string_or_null(
        input,
        ExecutionOperation::JavaScriptNumberCompare,
    )?;
    let Some(operand) = exact_javascript_number_operand(operand)? else {
        return Ok(false);
    };
    let Some(ordering) = input.compare(&operand) else {
        return Ok(false);
    };
    Ok(match operator {
        JavaScriptNumberCompareOperator::LessThan => ordering == Ordering::Less,
        JavaScriptNumberCompareOperator::GreaterThan => ordering == Ordering::Greater,
        JavaScriptNumberCompareOperator::StrictEqual => ordering == Ordering::Equal,
    })
}

fn evaluate_javascript_parse_float_predicate(
    operator: JavaScriptParseFloatOperator,
    input: CanonicalValue,
    operand: Option<DecimalLiteral>,
) -> Result<bool, InterpreterError> {
    let input = match input {
        CanonicalValue::Absent | CanonicalValue::Blank => "",
        CanonicalValue::Text(ref value) => value,
        ref actual => {
            return Err(InterpreterError::TypeMismatch {
                operation: ExecutionOperation::JavaScriptParseFloat,
                expected: ValueType::String,
                actual: ValueKind::of(actual),
            });
        }
    };
    let parsed = javascript_parse_float(input);
    match operator {
        JavaScriptParseFloatOperator::IsNaN => Ok(parsed.is_nan()),
        JavaScriptParseFloatOperator::StrictEqual | JavaScriptParseFloatOperator::GreaterThan => {
            let literal = operand.ok_or({
                InterpreterError::InvalidStaticSpec(
                    StaticSpecError::InvalidJavaScriptParseFloatPredicate {
                        operator,
                        has_operand: false,
                    },
                )
            })?;
            let exact =
                ExactDecimal::try_from_parts(literal.coefficient, literal.scale).map_err(|_| {
                    InterpreterError::Overflow {
                        operation: ExecutionOperation::JavaScriptParseFloat,
                    }
                })?;
            let expected =
                exact
                    .to_string()
                    .parse::<f64>()
                    .map_err(|_| InterpreterError::Overflow {
                        operation: ExecutionOperation::JavaScriptParseFloat,
                    })?;
            Ok(match operator {
                JavaScriptParseFloatOperator::StrictEqual => parsed == expected,
                JavaScriptParseFloatOperator::GreaterThan => parsed > expected,
                JavaScriptParseFloatOperator::IsNaN => unreachable!(),
            })
        }
    }
}

fn javascript_parse_float(input: &str) -> f64 {
    let trimmed = input.trim_start_matches(is_ecmascript_string_whitespace);
    let bytes = trimmed.as_bytes();
    let mut cursor = usize::from(matches!(bytes.first(), Some(b'+') | Some(b'-')));
    let negative = matches!(bytes.first(), Some(b'-'));
    if bytes
        .get(cursor..cursor.saturating_add("Infinity".len()))
        .is_some_and(|candidate| candidate == b"Infinity")
    {
        return if negative {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }

    let integer_start = cursor;
    while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
        cursor += 1;
    }
    let integer_digits = cursor - integer_start;
    let mut fraction_digits = 0;
    if bytes.get(cursor) == Some(&b'.') {
        cursor += 1;
        let fraction_start = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        fraction_digits = cursor - fraction_start;
    }
    if integer_digits == 0 && fraction_digits == 0 {
        return f64::NAN;
    }

    if matches!(bytes.get(cursor), Some(b'e') | Some(b'E')) {
        let exponent_marker = cursor;
        cursor += 1;
        if matches!(bytes.get(cursor), Some(b'+') | Some(b'-')) {
            cursor += 1;
        }
        let exponent_start = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        if exponent_start == cursor {
            cursor = exponent_marker;
        }
    }

    trimmed[..cursor].parse::<f64>().unwrap_or(f64::NAN)
}

fn canonical_integer_or_absent(
    value: CanonicalValue,
    operation: ExecutionOperation,
) -> Result<Option<i128>, InterpreterError> {
    match value {
        CanonicalValue::Absent | CanonicalValue::Blank => Ok(None),
        CanonicalValue::Integer(value) => Ok(Some(value)),
        actual => Err(InterpreterError::TypeMismatch {
            operation,
            expected: ValueType::Integer,
            actual: ValueKind::of(&actual),
        }),
    }
}

fn civil_day_ordinal(year: i128, month: i128, day: i128) -> Option<i128> {
    let month_index = month.checked_sub(1)?;
    let normalized_year = year.checked_add(month_index.div_euclid(12))?;
    let normalized_month = month_index.rem_euclid(12).checked_add(1)?;
    let adjusted_year = normalized_year.checked_sub(i128::from(normalized_month <= 2))?;
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year.checked_sub(era.checked_mul(400)?)?;
    let shifted_month = normalized_month.checked_add(if normalized_month > 2 { -3 } else { 9 })?;
    let day_of_year = 153_i128
        .checked_mul(shifted_month)?
        .checked_add(2)?
        .div_euclid(5)
        .checked_add(day)?
        .checked_sub(1)?;
    let day_of_era = year_of_era
        .checked_mul(365)?
        .checked_add(year_of_era.div_euclid(4))?
        .checked_sub(year_of_era.div_euclid(100))?
        .checked_add(day_of_year)?;
    era.checked_mul(146_097)?
        .checked_add(day_of_era)?
        .checked_sub(719_468)
}

fn evaluate_javascript_date_local_day(
    year: CanonicalValue,
    month_index: CanonicalValue,
    day: CanonicalValue,
) -> Result<CanonicalValue, InterpreterError> {
    let operation = ExecutionOperation::JavaScriptDateLocalDay;
    let Some(mut year) = canonical_integer_or_absent(year, operation)? else {
        return Ok(CanonicalValue::Absent);
    };
    let Some(month_index) = canonical_integer_or_absent(month_index, operation)? else {
        return Ok(CanonicalValue::Absent);
    };
    let Some(day) = canonical_integer_or_absent(day, operation)? else {
        return Ok(CanonicalValue::Absent);
    };
    if (0..=99).contains(&year) {
        year += 1900;
    }
    let Some(month) = month_index.checked_add(1) else {
        return Ok(CanonicalValue::Absent);
    };
    let Some(ordinal) = civil_day_ordinal(year, month, day) else {
        return Ok(CanonicalValue::Absent);
    };
    // ECMAScript TimeClip accepts exactly +/- 8.64e15 ms. At local midnight
    // that is the inclusive +/- 100,000,000 civil-day range.
    if !(-100_000_000..=100_000_000).contains(&ordinal) {
        return Ok(CanonicalValue::Absent);
    }
    Ok(CanonicalValue::Integer(ordinal))
}

fn evaluate_canonical_local_date_day(
    input: CanonicalValue,
) -> Result<CanonicalValue, InterpreterError> {
    match input {
        CanonicalValue::Absent | CanonicalValue::Blank => Ok(CanonicalValue::Absent),
        CanonicalValue::Date(value) => civil_day_ordinal(
            i128::from(value.year()),
            i128::from(value.month()),
            i128::from(value.day()),
        )
        .map(CanonicalValue::Integer)
        .ok_or(InterpreterError::Overflow {
            operation: ExecutionOperation::CanonicalLocalDateDay,
        }),
        actual => Err(InterpreterError::TypeMismatch {
            operation: ExecutionOperation::CanonicalLocalDateDay,
            expected: ValueType::Date,
            actual: ValueKind::of(&actual),
        }),
    }
}

fn field_value(
    field: FieldRef,
    environment: &Environment<'_>,
) -> Result<CanonicalValue, InterpreterError> {
    let instance = resolve_field_ref(field, environment)?;
    environment
        .canonical_inputs
        .binary_search_by(|candidate| candidate.field().cmp(&instance))
        .ok()
        .map(|index| environment.canonical_inputs[index].canonical().clone())
        .ok_or(InterpreterError::MissingInput { field: instance })
}

fn context_value(
    id: &'static str,
    environment: &Environment<'_>,
) -> Result<CanonicalValue, InterpreterError> {
    let id = parse_context_value_id(id)?;
    environment
        .request
        .context_values()
        .get(&id)
        .cloned()
        .ok_or(InterpreterError::MissingContextValue { id })
}

fn derived_value(
    calculation_id: &'static str,
    output_id: &'static str,
    selector: DerivedInstanceSelector,
    environment: &Environment<'_>,
) -> Result<CanonicalValue, InterpreterError> {
    let instance = resolve_derived_instance(calculation_id, output_id, selector, environment)?;
    let calculation_id = parse_calculation_id(calculation_id)?;
    let output_id = parse_output_id(output_id)?;
    environment
        .derived_outputs
        .iter()
        .find(|candidate| {
            candidate.calculation_id() == &calculation_id
                && candidate.output_id() == &output_id
                && candidate.instance() == instance.as_ref()
        })
        .map(|value| value.value().clone())
        .ok_or(InterpreterError::MissingDerivedValue {
            calculation_id,
            output_id,
            instance,
        })
}

fn resolve_derived_instance(
    calculation_id: &'static str,
    output_id: &'static str,
    selector: DerivedInstanceSelector,
    environment: &Environment<'_>,
) -> Result<Option<RepeatedGroupInstance>, InterpreterError> {
    let calculation = find_calculation(environment.spec, calculation_id).ok_or_else(|| {
        InterpreterError::MissingDerivedValue {
            calculation_id: parse_calculation_id(calculation_id)
                .expect("caller parsed the calculation ID"),
            output_id: parse_output_id(output_id).expect("caller parsed the output ID"),
            instance: None,
        }
    })?;
    match (calculation.scope, selector) {
        (EvaluationScope::Singleton, DerivedInstanceSelector::Singleton) => Ok(None),
        (EvaluationScope::EachGroup(group_id), DerivedInstanceSelector::CurrentGroupInstance) => {
            let instance = environment.current_group.clone().ok_or_else(|| {
                InterpreterError::MissingCurrentDerivedGroup {
                    calculation_id: parse_calculation_id(calculation_id)
                        .expect("caller parsed the calculation ID"),
                }
            })?;
            if instance.group_id().as_str() != group_id {
                return Err(InterpreterError::DerivedScopeMismatch {
                    calculation_id: parse_calculation_id(calculation_id)
                        .expect("caller parsed the calculation ID"),
                });
            }
            Ok(Some(instance))
        }
        (
            EvaluationScope::EachGroup(group_id),
            DerivedInstanceSelector::StableInstanceId(instance_id),
        ) => {
            let instance = RepeatedGroupInstance::new(
                parse_group_id(group_id)?,
                parse_instance_id(instance_id)?,
            );
            if environment
                .request
                .raw_inputs()
                .repeated_group_instances()
                .binary_search(&instance)
                .is_err()
            {
                return Err(InterpreterError::MissingDerivedValue {
                    calculation_id: parse_calculation_id(calculation_id)?,
                    output_id: parse_output_id(output_id)?,
                    instance: Some(instance),
                });
            }
            Ok(Some(instance))
        }
        _ => Err(InterpreterError::DerivedScopeMismatch {
            calculation_id: parse_calculation_id(calculation_id)?,
        }),
    }
}

fn resolve_field_ref(
    field: FieldRef,
    environment: &Environment<'_>,
) -> Result<FieldInstance, InterpreterError> {
    resolve_field_ref_for_instance(
        field,
        environment.spec,
        environment.request,
        environment.current_group.as_ref(),
    )
}

fn resolve_field_ref_for_instance(
    field: FieldRef,
    rule_set: &StaticRuleSetSpec,
    request: &EvaluationRequest,
    current_group: Option<&RepeatedGroupInstance>,
) -> Result<FieldInstance, InterpreterError> {
    let field_id = parse_field_id(field.field_id)?;
    let spec =
        find_field(rule_set, field.field_id).ok_or_else(|| InterpreterError::MissingInput {
            field: FieldInstance::singleton(field_id.clone()),
        })?;
    match (field.instance, spec.group_id) {
        (FieldInstanceSelector::Singleton, None) => Ok(FieldInstance::singleton(field_id)),
        (FieldInstanceSelector::CurrentGroupInstance, Some(group_id)) => {
            let current = current_group.ok_or_else(|| InterpreterError::MissingCurrentGroup {
                field_id: field_id.clone(),
            })?;
            if current.group_id().as_str() != group_id {
                return Err(InterpreterError::FieldScopeMismatch { field_id });
            }
            FieldInstance::try_new(field_id, vec![current.clone()]).map_err(|_| {
                InterpreterError::FieldScopeMismatch {
                    field_id: parse_field_id(field.field_id).expect("already parsed"),
                }
            })
        }
        (FieldInstanceSelector::StableInstanceId(instance_id), Some(group_id)) => {
            let group = RepeatedGroupInstance::new(
                parse_group_id(group_id)?,
                parse_instance_id(instance_id)?,
            );
            if !request
                .raw_inputs()
                .repeated_group_instances()
                .contains(&group)
            {
                let missing = FieldInstance::try_new(field_id, vec![group])
                    .expect("one group path cannot duplicate");
                return Err(InterpreterError::MissingInput { field: missing });
            }
            FieldInstance::try_new(field_id, vec![group]).map_err(|_| {
                InterpreterError::FieldScopeMismatch {
                    field_id: parse_field_id(field.field_id).expect("already parsed"),
                }
            })
        }
        _ => Err(InterpreterError::FieldScopeMismatch { field_id }),
    }
}

fn evaluate_unary(
    operator: UnaryOperator,
    operand: CanonicalValue,
) -> Result<CanonicalValue, InterpreterError> {
    if is_null(&operand) {
        return Ok(operand);
    }
    match (operator, operand) {
        (UnaryOperator::Negate, CanonicalValue::Integer(value)) => value
            .checked_neg()
            .map(CanonicalValue::Integer)
            .ok_or(InterpreterError::Overflow {
                operation: ExecutionOperation::UnaryNegate,
            }),
        (UnaryOperator::Negate, CanonicalValue::Decimal(value)) => value
            .coefficient()
            .checked_neg()
            .and_then(|coefficient| ExactDecimal::try_from_parts(coefficient, value.scale()).ok())
            .map(CanonicalValue::Decimal)
            .ok_or(InterpreterError::Overflow {
                operation: ExecutionOperation::UnaryNegate,
            }),
        (UnaryOperator::Absolute, CanonicalValue::Integer(value)) => value
            .checked_abs()
            .map(CanonicalValue::Integer)
            .ok_or(InterpreterError::Overflow {
                operation: ExecutionOperation::UnaryAbsolute,
            }),
        (UnaryOperator::Absolute, CanonicalValue::Decimal(value)) => value
            .coefficient()
            .checked_abs()
            .and_then(|coefficient| ExactDecimal::try_from_parts(coefficient, value.scale()).ok())
            .map(CanonicalValue::Decimal)
            .ok_or(InterpreterError::Overflow {
                operation: ExecutionOperation::UnaryAbsolute,
            }),
        (UnaryOperator::Length, CanonicalValue::Text(value)) => {
            i128::try_from(value.chars().count())
                .map(CanonicalValue::Integer)
                .map_err(|_| InterpreterError::Overflow {
                    operation: ExecutionOperation::UnaryLength,
                })
        }
        (operator, actual) => Err(InterpreterError::TypeMismatch {
            operation: match operator {
                UnaryOperator::Negate => ExecutionOperation::UnaryNegate,
                UnaryOperator::Absolute => ExecutionOperation::UnaryAbsolute,
                UnaryOperator::Length => ExecutionOperation::UnaryLength,
            },
            expected: match operator {
                UnaryOperator::Length => ValueType::String,
                _ => ValueType::Decimal,
            },
            actual: ValueKind::of(&actual),
        }),
    }
}

fn evaluate_binary(
    operator: BinaryOperator,
    division_policy: Option<DecimalDivisionPolicy>,
    left: CanonicalValue,
    right: CanonicalValue,
) -> Result<CanonicalValue, InterpreterError> {
    if is_null(&left) {
        return Ok(left);
    }
    if is_null(&right) {
        return Ok(right);
    }
    match (operator, left, right) {
        (BinaryOperator::Add, CanonicalValue::Integer(left), CanonicalValue::Integer(right)) => {
            left.checked_add(right)
                .map(CanonicalValue::Integer)
                .ok_or(InterpreterError::Overflow {
                    operation: ExecutionOperation::Add,
                })
        }
        (
            BinaryOperator::Subtract,
            CanonicalValue::Integer(left),
            CanonicalValue::Integer(right),
        ) => {
            left.checked_sub(right)
                .map(CanonicalValue::Integer)
                .ok_or(InterpreterError::Overflow {
                    operation: ExecutionOperation::Subtract,
                })
        }
        (
            BinaryOperator::Multiply,
            CanonicalValue::Integer(left),
            CanonicalValue::Integer(right),
        ) => {
            left.checked_mul(right)
                .map(CanonicalValue::Integer)
                .ok_or(InterpreterError::Overflow {
                    operation: ExecutionOperation::Multiply,
                })
        }
        (BinaryOperator::Remainder, CanonicalValue::Integer(_), CanonicalValue::Integer(0)) => {
            Err(InterpreterError::DivisionByZero {
                operation: binary_operation(operator),
            })
        }
        (
            BinaryOperator::Remainder,
            CanonicalValue::Integer(left),
            CanonicalValue::Integer(right),
        ) => {
            left.checked_rem(right)
                .map(CanonicalValue::Integer)
                .ok_or(InterpreterError::Overflow {
                    operation: ExecutionOperation::Remainder,
                })
        }
        (BinaryOperator::Add, CanonicalValue::Decimal(left), CanonicalValue::Decimal(right)) => {
            decimal_add(left, right).map(CanonicalValue::Decimal)
        }
        (
            BinaryOperator::Subtract,
            CanonicalValue::Decimal(left),
            CanonicalValue::Decimal(right),
        ) => decimal_subtract(left, right).map(CanonicalValue::Decimal),
        (
            BinaryOperator::Multiply,
            CanonicalValue::Decimal(left),
            CanonicalValue::Decimal(right),
        ) => decimal_multiply(left, right).map(CanonicalValue::Decimal),
        (BinaryOperator::Divide, CanonicalValue::Decimal(left), CanonicalValue::Decimal(right)) => {
            let policy = division_policy.ok_or(StaticSpecError::MissingDecimalDivisionPolicy)?;
            decimal_divide(left, right, policy).map(CanonicalValue::Decimal)
        }
        (
            BinaryOperator::Remainder,
            CanonicalValue::Decimal(left),
            CanonicalValue::Decimal(right),
        ) => decimal_remainder(left, right).map(CanonicalValue::Decimal),
        (BinaryOperator::Concat, CanonicalValue::Text(left), CanonicalValue::Text(right)) => {
            let mut value = left;
            value.push_str(&right);
            Ok(CanonicalValue::Text(value))
        }
        (operator, left, _) => Err(InterpreterError::TypeMismatch {
            operation: binary_operation(operator),
            expected: match operator {
                BinaryOperator::Concat => ValueType::String,
                _ => ValueType::Decimal,
            },
            actual: ValueKind::of(&left),
        }),
    }
}

fn evaluate_nary(
    operator: NaryOperator,
    operands: &[Expression],
    environment: &mut Environment<'_>,
) -> Result<CanonicalValue, InterpreterError> {
    let mut values = Vec::with_capacity(operands.len());
    for operand in operands {
        values.push(evaluate_expression(operand, environment)?);
    }
    match operator {
        NaryOperator::Coalesce => Ok(values
            .into_iter()
            .find(|value| !is_null(value))
            .unwrap_or(CanonicalValue::Absent)),
        NaryOperator::Concat => {
            let mut result = String::new();
            for value in values {
                match value {
                    CanonicalValue::Text(value) => result.push_str(&value),
                    actual => {
                        return Err(InterpreterError::TypeMismatch {
                            operation: ExecutionOperation::Concat,
                            expected: ValueType::String,
                            actual: ValueKind::of(&actual),
                        });
                    }
                }
            }
            Ok(CanonicalValue::Text(result))
        }
        NaryOperator::Sum => fold_sum(values),
        NaryOperator::Minimum => fold_extreme(values, Ordering::Less),
        NaryOperator::Maximum => fold_extreme(values, Ordering::Greater),
    }
}

fn fold_sum(values: Vec<CanonicalValue>) -> Result<CanonicalValue, InterpreterError> {
    let mut values = values.into_iter();
    let Some(mut result) = values.next() else {
        return Err(StaticSpecError::EmptyRequiredList {
            kind: SpecItemKind::Output,
            value: "sum",
        }
        .into());
    };
    for value in values {
        result = evaluate_binary(BinaryOperator::Add, None, result, value)?;
    }
    Ok(result)
}

fn fold_extreme(
    values: Vec<CanonicalValue>,
    desired: Ordering,
) -> Result<CanonicalValue, InterpreterError> {
    let mut values = values.into_iter();
    let Some(mut result) = values.next() else {
        return Err(StaticSpecError::EmptyRequiredList {
            kind: SpecItemKind::Output,
            value: "extreme",
        }
        .into());
    };
    for value in values {
        if compare_values(&value, &result)? == desired {
            result = value;
        }
    }
    Ok(result)
}

fn coerce_canonical(
    value: CanonicalValue,
    coercion: Coercion,
) -> Result<CanonicalValue, InterpreterError> {
    let owned = match value {
        CanonicalValue::Absent => OwnedNormalizedInput::Absent,
        CanonicalValue::Blank => OwnedNormalizedInput::Text(String::new()),
        CanonicalValue::Text(value) => OwnedNormalizedInput::Text(value),
        actual => {
            return Err(InterpreterError::TypeMismatch {
                operation: ExecutionOperation::Coercion,
                expected: ValueType::String,
                actual: ValueKind::of(&actual),
            });
        }
    };
    coerce_normalized(owned, coercion)
}

fn evaluate_group_aggregate(
    operator: GroupAggregateOperator,
    group_id: &'static str,
    value: &'static Expression,
    environment: &mut Environment<'_>,
) -> Result<CanonicalValue, InterpreterError> {
    if environment.current_group.is_some() {
        return Err(InterpreterError::InvalidStaticSpec(
            StaticSpecError::InvalidReference {
                kind: SpecItemKind::Output,
                value: "group-aggregate",
                target: "nested-group-aggregate",
            },
        ));
    }
    let instances: Vec<_> = environment
        .request
        .raw_inputs()
        .repeated_group_instances()
        .iter()
        .filter(|candidate| candidate.group_id().as_str() == group_id)
        .cloned()
        .collect();
    if operator == GroupAggregateOperator::Count {
        return i128::try_from(instances.len())
            .map(CanonicalValue::Integer)
            .map_err(|_| InterpreterError::Overflow {
                operation: ExecutionOperation::GroupAggregate,
            });
    }

    let previous = environment.current_group.clone();
    let mut values = Vec::new();
    for instance in instances {
        environment.current_group = Some(instance);
        let evaluated = evaluate_expression(value, environment);
        match evaluated {
            Ok(value) => values.push(value),
            Err(error) => {
                environment.current_group = previous;
                return Err(error);
            }
        }
    }
    environment.current_group = previous;

    match operator {
        GroupAggregateOperator::Count => unreachable!("handled above"),
        GroupAggregateOperator::CountPresent => {
            i128::try_from(values.iter().filter(|value| is_present(value)).count())
                .map(CanonicalValue::Integer)
                .map_err(|_| InterpreterError::Overflow {
                    operation: ExecutionOperation::GroupAggregate,
                })
        }
        GroupAggregateOperator::Sum if values.is_empty() => match expression_result_type(value) {
            ValueType::Integer => Ok(CanonicalValue::Integer(0)),
            ValueType::Decimal => Ok(CanonicalValue::Decimal(
                ExactDecimal::try_from_parts(0, 0).expect("zero decimal"),
            )),
            actual => Err(InterpreterError::TypeMismatch {
                operation: ExecutionOperation::GroupAggregate,
                expected: ValueType::Decimal,
                actual: match actual {
                    ValueType::Null => ValueKind::Absent,
                    ValueType::String => ValueKind::String,
                    ValueType::Boolean => ValueKind::Boolean,
                    ValueType::Integer => ValueKind::Integer,
                    ValueType::Decimal => ValueKind::Decimal,
                    ValueType::Date => ValueKind::Date,
                },
            }),
        },
        GroupAggregateOperator::Sum => fold_sum(values),
        GroupAggregateOperator::Minimum if values.is_empty() => Ok(CanonicalValue::Absent),
        GroupAggregateOperator::Minimum => fold_extreme(values, Ordering::Less),
        GroupAggregateOperator::Maximum if values.is_empty() => Ok(CanonicalValue::Absent),
        GroupAggregateOperator::Maximum => fold_extreme(values, Ordering::Greater),
    }
}

fn evaluate_predicate(
    predicate: &Predicate,
    environment: &mut Environment<'_>,
) -> Result<bool, InterpreterError> {
    match predicate {
        Predicate::Constant(value) => Ok(*value),
        Predicate::Not(predicate) => Ok(!evaluate_predicate(predicate, environment)?),
        Predicate::All(predicates) => {
            for predicate in *predicates {
                if !evaluate_predicate(predicate, environment)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Predicate::Any(predicates) => {
            for predicate in *predicates {
                if evaluate_predicate(predicate, environment)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Predicate::Compare {
            operator,
            left,
            right,
        } => {
            let left = evaluate_expression(left, environment)?;
            let right = evaluate_expression(right, environment)?;
            evaluate_compare(*operator, &left, &right)
        }
        Predicate::Presence { operator, value } => {
            let value = evaluate_expression(value, environment)?;
            Ok(match operator {
                PresenceOperator::IsEmpty => is_empty(&value),
                PresenceOperator::IsPresent => is_present(&value),
                PresenceOperator::IsNull => is_null(&value),
            })
        }
        Predicate::CoercionFailed { field } => Ok(matches!(
            field_value(*field, environment)?,
            CanonicalValue::Text(_)
        )),
        Predicate::JavaScriptParseFloat {
            operator,
            input,
            operand,
        } => evaluate_javascript_parse_float_predicate(
            *operator,
            evaluate_expression(input, environment)?,
            *operand,
        ),
        Predicate::JavaScriptGlobalIsNaNLogicalOr { inputs } => {
            let mut evaluated_inputs = Vec::new();
            for input in *inputs {
                let candidate = evaluate_expression(input, environment)?;
                let truthy = javascript_string_or_null_truthy(
                    &candidate,
                    ExecutionOperation::JavaScriptGlobalIsNaNLogicalOr,
                )?;
                evaluated_inputs.push(candidate);
                if truthy {
                    break;
                }
            }
            let selected = select_javascript_logical_or_value(
                &evaluated_inputs,
                ExecutionOperation::JavaScriptGlobalIsNaNLogicalOr,
            )?
            .clone();
            Ok(matches!(
                legacy_javascript_number_from_string_or_null(
                    selected,
                    ExecutionOperation::JavaScriptGlobalIsNaNLogicalOr,
                )?,
                LegacyJavaScriptNumber::NaN
            ))
        }
        Predicate::JavaScriptNumberCompare {
            operator,
            input,
            operand,
        } => {
            let input = evaluate_expression(input, environment)?;
            let operand = evaluate_expression(operand, environment)?;
            evaluate_javascript_number_compare(*operator, input, operand)
        }
        Predicate::Checksum { algorithm, input } => {
            evaluate_checksum_predicate(*algorithm, evaluate_expression(input, environment)?)
        }
        Predicate::Matches {
            value,
            pattern,
            case_sensitive,
        } => match evaluate_expression(value, environment)? {
            CanonicalValue::Text(value) => Ok((pattern.matcher)(&value, *case_sensitive)),
            actual => Err(InterpreterError::TypeMismatch {
                operation: ExecutionOperation::Matches,
                expected: ValueType::String,
                actual: ValueKind::of(&actual),
            }),
        },
        Predicate::In { value, candidates } => {
            let value = evaluate_expression(value, environment)?;
            for candidate in *candidates {
                if value == literal_value(*candidate)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Predicate::GroupQuantifier {
            quantifier,
            group_id,
            predicate,
        } => {
            let instances: Vec<_> = environment
                .request
                .raw_inputs()
                .repeated_group_instances()
                .iter()
                .filter(|candidate| candidate.group_id().as_str() == *group_id)
                .cloned()
                .collect();
            let previous = environment.current_group.clone();
            let mut any = false;
            let mut all = true;
            for instance in instances {
                environment.current_group = Some(instance);
                let matched = evaluate_predicate(predicate, environment)?;
                any |= matched;
                all &= matched;
                match quantifier {
                    GroupQuantifier::Any if any => break,
                    GroupQuantifier::All if !all => break,
                    GroupQuantifier::None if any => break,
                    _ => {}
                }
            }
            environment.current_group = previous;
            Ok(match quantifier {
                GroupQuantifier::Any => any,
                GroupQuantifier::All => all,
                GroupQuantifier::None => !any,
            })
        }
    }
}

fn evaluate_checksum_predicate(
    algorithm: ChecksumAlgorithm,
    input: CanonicalValue,
) -> Result<bool, InterpreterError> {
    let input = match input {
        CanonicalValue::Absent | CanonicalValue::Blank => return Ok(false),
        CanonicalValue::Text(value) => value,
        actual => {
            return Err(InterpreterError::TypeMismatch {
                operation: ExecutionOperation::Checksum,
                expected: ValueType::String,
                actual: ValueKind::of(&actual),
            });
        }
    };
    Ok(match algorithm {
        ChecksumAlgorithm::OfflineEbirTinV1 => offline_ebir_tin_v1_is_valid(&input),
    })
}

fn offline_ebir_tin_v1_is_valid(input: &str) -> bool {
    let Some((digits, unsigned_number)) = offline_ebir_tin_v1_input(input) else {
        return false;
    };
    if offline_ebir_tin_v1_check_digit(&digits[..8]) == digits[8] {
        return true;
    }

    // The shipped helper has a second, numeric retry path. It is entered only
    // for unsigned values beginning with 9 inside this strict interval. A
    // nonzero final digit is decremented; a final zero is replaced with 9.
    // Signed nine-character inputs pass the helper's integer gate, but their
    // leading sign is converted to the helper's error-position value 1 and
    // prevents this retry because the first character is not 9.
    let Some(number) = unsigned_number else {
        return false;
    };
    if digits[0] != 9 || number <= 900_000_004 || number >= 905_180_009 {
        return false;
    }
    let adjusted = if digits[8] == 0 {
        number + 9
    } else {
        number - 1
    };
    let mut adjusted_digits = [0_u8; 9];
    let mut remaining = adjusted;
    for slot in adjusted_digits.iter_mut().rev() {
        *slot = (remaining % 10) as u8;
        remaining /= 10;
    }
    offline_ebir_tin_v1_check_digit(&adjusted_digits[..8]) == adjusted_digits[8]
}

fn offline_ebir_tin_v1_input(input: &str) -> Option<([u8; 9], Option<u32>)> {
    let bytes = input.as_bytes();
    if bytes.len() != 9 {
        return None;
    }

    let mut digits = [0_u8; 9];
    if bytes.iter().all(u8::is_ascii_digit) {
        for (target, source) in digits.iter_mut().zip(bytes) {
            *target = *source - b'0';
        }
        let number = digits
            .iter()
            .fold(0_u32, |value, digit| value * 10 + u32::from(*digit));
        return Some((digits, Some(number)));
    }

    if matches!(bytes[0], b'+' | b'-') && bytes[1..].iter().all(u8::is_ascii_digit) {
        // The Pascal helper's character conversion reports error position one
        // for either sign; its caller stores that numeric value as digit one.
        digits[0] = 1;
        for (target, source) in digits[1..].iter_mut().zip(&bytes[1..]) {
            *target = *source - b'0';
        }
        return Some((digits, None));
    }

    None
}

fn offline_ebir_tin_v1_check_digit(prefix: &[u8]) -> u8 {
    debug_assert_eq!(prefix.len(), 8);
    let sum = prefix
        .iter()
        .enumerate()
        .map(|(index, digit)| {
            let offset = 8_u32 - index as u32;
            let shifted = (u32::from(*digit) + offset) % 10;
            let weighted = shifted * (1_u32 << offset);
            decimal_digital_root(weighted)
        })
        .sum::<u32>();
    let remainder = sum % 10;
    if remainder == 0 {
        0
    } else {
        (10 - remainder) as u8
    }
}

fn decimal_digital_root(value: u32) -> u32 {
    if value == 0 { 0 } else { 1 + (value - 1) % 9 }
}

fn evaluate_compare(
    operator: CompareOperator,
    left: &CanonicalValue,
    right: &CanonicalValue,
) -> Result<bool, InterpreterError> {
    match operator {
        CompareOperator::Equal => Ok(left == right),
        CompareOperator::NotEqual => Ok(left != right),
        CompareOperator::LessThan => Ok(compare_values(left, right)? == Ordering::Less),
        CompareOperator::LessThanOrEqual => Ok(compare_values(left, right)? != Ordering::Greater),
        CompareOperator::GreaterThan => Ok(compare_values(left, right)? == Ordering::Greater),
        CompareOperator::GreaterThanOrEqual => Ok(compare_values(left, right)? != Ordering::Less),
    }
}

fn compare_values(
    left: &CanonicalValue,
    right: &CanonicalValue,
) -> Result<Ordering, InterpreterError> {
    match (left, right) {
        (CanonicalValue::Text(left), CanonicalValue::Text(right)) => Ok(left.cmp(right)),
        (CanonicalValue::Integer(left), CanonicalValue::Integer(right)) => Ok(left.cmp(right)),
        (CanonicalValue::Decimal(left), CanonicalValue::Decimal(right)) => {
            decimal_compare(*left, *right)
        }
        (CanonicalValue::Date(left), CanonicalValue::Date(right)) => Ok((
            left.year(),
            left.month(),
            left.day(),
        )
            .cmp(&(right.year(), right.month(), right.day()))),
        (left, _) => Err(InterpreterError::TypeMismatch {
            operation: ExecutionOperation::Compare,
            expected: match ValueKind::of(left) {
                ValueKind::String => ValueType::String,
                ValueKind::Integer => ValueType::Integer,
                ValueKind::Decimal => ValueType::Decimal,
                ValueKind::Date => ValueType::Date,
                _ => ValueType::Null,
            },
            actual: ValueKind::of(right),
        }),
    }
}

fn apply_effect(
    effect: &Effect,
    rule: &RuleSpec,
    rule_id: &RuleId,
    occurrence: u32,
    effect_index: u32,
    mutate_working_fields: bool,
    environment: &mut Environment<'_>,
    violations: &mut Vec<RuleViolation>,
    field_value_assignments: &mut Vec<FieldValueAssignment>,
) -> Result<bool, InterpreterError> {
    match effect {
        Effect::EmitIssue {
            severity,
            message,
            official_message,
            assessment,
            fields,
        } => {
            let field_refs = fields
                .iter()
                .map(|field| resolve_field_ref(*field, environment).map(RuleFieldRef::semantic))
                .collect::<Result<Vec<_>, _>>()?;
            violations.push(RuleViolation::new(
                rule_id.clone(),
                environment.current_group.clone(),
                environment.request.context().phase(),
                IssueOrder::new(rule.order, occurrence),
                field_refs,
                official_message.map(str::to_owned),
                (*message).to_owned(),
                *assessment,
                *severity,
                environment.request.context().profile(),
            ));
            Ok(*severity == RuleSeverity::Blocking)
        }
        Effect::SetRawFieldValue { field, value } => {
            let field = resolve_field_ref(*field, environment)?;
            if environment.request.raw_inputs().raw_value(&field).is_none() {
                return Err(InterpreterError::MissingInput { field });
            }
            let value = (*value).to_raw_value();
            field_value_assignments.push(FieldValueAssignment::new(
                RuleExecution::new(rule_id.clone(), environment.current_group.clone()),
                effect_index,
                field.clone(),
                value.clone(),
            ));
            if mutate_working_fields {
                recanonicalize_working_field(&field, value, environment)?;
            }
            Ok(false)
        }
        Effect::EmitNotification { .. }
        | Effect::SetDerived { .. }
        | Effect::NormalizeField { .. }
        | Effect::SetWorkflowState { .. } => Err(InterpreterError::UnsupportedEffect {
            rule_id: rule_id.clone(),
            kind: effect.kind(),
        }),
    }
}

fn apply_calculation_writeback(
    writeback: CalculationWriteback,
    value: &CanonicalValue,
    environment: &mut Environment<'_>,
) -> Result<(), InterpreterError> {
    let field = resolve_field_ref(writeback.field, environment)?;
    let formatted = match writeback.format {
        CalculationWriteFormat::OfflineEbirFormatCurrencyV1 => {
            offline_ebir_format_currency_v1(value)?
        }
    };
    recanonicalize_working_field(&field, RawValue::Text(formatted), environment)?;
    Ok(())
}

fn recanonicalize_working_field(
    field: &FieldInstance,
    working_raw: RawValue,
    environment: &mut Environment<'_>,
) -> Result<(), InterpreterError> {
    let field_spec = find_field(environment.spec, field.field_id().as_str()).ok_or_else(|| {
        InterpreterError::UnexpectedInput {
            field: field.clone(),
        }
    })?;
    let behavior = select_branch(
        field_spec
            .behavior
            .select(environment.request.context().profile()),
        SpecItemKind::Field,
        field_spec.field_id,
        environment.request.context(),
    )?;
    let normalized = normalize_raw(&working_raw, behavior.normalization)?;
    let canonical = coerce_normalized(normalized, behavior.coercion)?;
    let Some(slot) = environment
        .canonical_inputs
        .iter_mut()
        .find(|candidate| candidate.field() == field)
    else {
        return Err(InterpreterError::MissingInput {
            field: field.clone(),
        });
    };
    let original_raw = slot.raw().clone();
    *slot = CanonicalFieldValue::new(field.clone(), original_raw, canonical);
    Ok(())
}

fn ensure_type(
    expected: ValueType,
    value: &CanonicalValue,
    operation: ExecutionOperation,
) -> Result<(), InterpreterError> {
    let actual = ValueKind::of(value);
    let matches = match (expected, actual) {
        (_, ValueKind::Absent | ValueKind::Blank) => true,
        (ValueType::Null, _) => false,
        (ValueType::String, ValueKind::String)
        | (ValueType::Boolean, ValueKind::Boolean)
        | (ValueType::Integer, ValueKind::Integer)
        | (ValueType::Decimal, ValueKind::Decimal)
        | (ValueType::Date, ValueKind::Date) => true,
        _ => false,
    };
    if matches {
        Ok(())
    } else {
        Err(InterpreterError::TypeMismatch {
            operation,
            expected,
            actual,
        })
    }
}

fn is_null(value: &CanonicalValue) -> bool {
    matches!(value, CanonicalValue::Absent | CanonicalValue::Blank)
}

fn is_empty(value: &CanonicalValue) -> bool {
    is_null(value) || matches!(value, CanonicalValue::Text(text) if text.is_empty())
}

fn is_present(value: &CanonicalValue) -> bool {
    !matches!(value, CanonicalValue::Absent)
}

fn binary_operation(operator: BinaryOperator) -> ExecutionOperation {
    match operator {
        BinaryOperator::Add => ExecutionOperation::Add,
        BinaryOperator::Subtract => ExecutionOperation::Subtract,
        BinaryOperator::Multiply => ExecutionOperation::Multiply,
        BinaryOperator::Divide => ExecutionOperation::Divide,
        BinaryOperator::Remainder => ExecutionOperation::Remainder,
        BinaryOperator::Concat => ExecutionOperation::Concat,
    }
}

fn nary_operation(operator: NaryOperator) -> ExecutionOperation {
    match operator {
        NaryOperator::Sum => ExecutionOperation::Sum,
        NaryOperator::Minimum => ExecutionOperation::Minimum,
        NaryOperator::Maximum => ExecutionOperation::Maximum,
        NaryOperator::Concat | NaryOperator::Coalesce => ExecutionOperation::Concat,
    }
}

fn parse_decimal_input(
    source: &str,
    grouping: InputGrouping,
) -> Result<ExactDecimal, CoercionFailure> {
    let normalized = match grouping {
        InputGrouping::Forbidden if source.contains(',') => {
            return Err(CoercionFailure::InvalidGrouping);
        }
        InputGrouping::Forbidden => source.to_owned(),
        InputGrouping::Comma => strip_valid_grouping(source)?,
    };
    normalized
        .parse()
        .map_err(|_| CoercionFailure::InvalidSyntax)
}

fn strip_valid_grouping(source: &str) -> Result<String, CoercionFailure> {
    let (sign, unsigned) = source
        .strip_prefix('-')
        .map_or(("", source), |value| ("-", value));
    let (integer, fraction) = unsigned
        .split_once('.')
        .map_or((unsigned, None), |(integer, fraction)| {
            (integer, Some(fraction))
        });
    if fraction.is_some_and(|value| value.contains(',')) {
        return Err(CoercionFailure::InvalidGrouping);
    }
    if !integer.contains(',') {
        return Ok(source.to_owned());
    }
    let groups: Vec<_> = integer.split(',').collect();
    if groups.first().is_none_or(|group| {
        group.is_empty() || group.len() > 3 || !group.bytes().all(|byte| byte.is_ascii_digit())
    }) || groups
        .iter()
        .skip(1)
        .any(|group| group.len() != 3 || !group.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(CoercionFailure::InvalidGrouping);
    }
    let mut result = String::from(sign);
    for group in groups {
        result.push_str(group);
    }
    if let Some(fraction) = fraction {
        result.push('.');
        result.push_str(fraction);
    }
    Ok(result)
}

fn constrain_decimal(
    value: ExactDecimal,
    policy: DecimalPolicy,
) -> Result<ExactDecimal, InterpreterError> {
    // `ExactDecimal` normalizes trailing zeroes, so checking coefficient
    // digits alone does not enforce a fixed-point precision/scale policy.
    // First reject a value that still exceeds the reviewed scale (notably
    // when rounding mode is `None`), then compare its coefficient after
    // aligning it to the policy scale.
    if value.scale() > policy.scale {
        return Err(InterpreterError::InvalidCoercion {
            target: ValueType::Decimal,
            reason: CoercionFailure::PrecisionOverflow,
        });
    }
    let maximum = checked_pow10(policy.precision)
        .and_then(|power| power.checked_sub(1))
        .ok_or(InterpreterError::Overflow {
            operation: ExecutionOperation::Coercion,
        })?;
    let aligned = checked_pow10(policy.scale - value.scale())
        .and_then(|factor| value.coefficient().checked_mul(factor));
    if aligned.is_some_and(|coefficient| coefficient.unsigned_abs() <= maximum as u128) {
        return Ok(value);
    }
    match policy.overflow {
        OverflowPolicy::Error => Err(InterpreterError::InvalidCoercion {
            target: ValueType::Decimal,
            reason: CoercionFailure::PrecisionOverflow,
        }),
        OverflowPolicy::Clamp => {
            let coefficient = if value.coefficient().is_negative() {
                maximum.checked_neg().ok_or(InterpreterError::Overflow {
                    operation: ExecutionOperation::Coercion,
                })?
            } else {
                maximum
            };
            ExactDecimal::try_from_parts(coefficient, policy.scale).map_err(|_| {
                InterpreterError::Overflow {
                    operation: ExecutionOperation::Coercion,
                }
            })
        }
    }
}

fn apply_output_rounding(
    value: CanonicalValue,
    rounding: Rounding,
) -> Result<CanonicalValue, InterpreterError> {
    if is_null(&value) {
        return Ok(value);
    }
    match value {
        CanonicalValue::Decimal(value) => {
            round_decimal(value, rounding).map(CanonicalValue::Decimal)
        }
        actual => Err(InterpreterError::TypeMismatch {
            operation: ExecutionOperation::Rounding,
            expected: ValueType::Decimal,
            actual: ValueKind::of(&actual),
        }),
    }
}

fn round_decimal(
    value: ExactDecimal,
    rounding: Rounding,
) -> Result<ExactDecimal, InterpreterError> {
    if rounding.scale > 18 {
        return Err(StaticSpecError::InvalidRoundingScale {
            scale: rounding.scale,
        }
        .into());
    }
    match round_exact_decimal_to_scale(value, rounding.scale, rounding.mode) {
        Ok(rounded) => Ok(rounded),
        // Existing calculation/coercion semantics use `None` to preserve the
        // exact value and let the enclosing precision policy reject it. The
        // serialization formatter calls the same exact primitive directly and
        // rejects this case because it would discard nonzero digits.
        Err(ExactRoundingError::Inexact) if rounding.mode == RoundingMode::None => Ok(value),
        Err(ExactRoundingError::ScaleTooLarge { scale, .. }) => {
            Err(StaticSpecError::InvalidRoundingScale { scale }.into())
        }
        Err(ExactRoundingError::Inexact | ExactRoundingError::Overflow) => {
            Err(InterpreterError::Overflow {
                operation: ExecutionOperation::Rounding,
            })
        }
    }
}

pub(crate) fn round_exact_decimal_to_scale(
    value: ExactDecimal,
    target_scale: u32,
    mode: RoundingMode,
) -> Result<ExactDecimal, ExactRoundingError> {
    if target_scale > ExactDecimal::MAX_SCALE {
        return Err(ExactRoundingError::ScaleTooLarge {
            scale: target_scale,
            maximum: ExactDecimal::MAX_SCALE,
        });
    }
    if value.scale() <= target_scale {
        return Ok(value);
    }

    let divisor = BigInt::from(10_u8).pow(value.scale() - target_scale);
    let coefficient = BigInt::from(value.coefficient());
    let quotient = &coefficient / &divisor;
    let remainder = &coefficient % &divisor;
    let rounded = round_exact_quotient(quotient, &remainder, &divisor, mode)?;
    let coefficient = i128::try_from(rounded).map_err(|_| ExactRoundingError::Overflow)?;
    ExactDecimal::try_from_parts(coefficient, target_scale)
        .map_err(|_| ExactRoundingError::Overflow)
}

fn round_exact_quotient(
    mut quotient: BigInt,
    remainder: &BigInt,
    denominator: &BigInt,
    mode: RoundingMode,
) -> Result<BigInt, ExactRoundingError> {
    if remainder.sign() == Sign::NoSign {
        return Ok(quotient);
    }

    let halfway_ordering = (remainder.magnitude() << 1_usize).cmp(denominator.magnitude());
    let adjust = match mode {
        RoundingMode::None => return Err(ExactRoundingError::Inexact),
        RoundingMode::TowardZero => false,
        RoundingMode::AwayFromZero => true,
        RoundingMode::Floor => remainder.sign() == Sign::Minus,
        RoundingMode::Ceiling => remainder.sign() == Sign::Plus,
        RoundingMode::HalfUp => halfway_ordering != Ordering::Less,
        RoundingMode::HalfEven => {
            halfway_ordering == Ordering::Greater
                || (halfway_ordering == Ordering::Equal && quotient.magnitude().bit(0))
        }
        RoundingMode::HalfCeiling => {
            halfway_ordering == Ordering::Greater
                || (halfway_ordering == Ordering::Equal && remainder.sign() == Sign::Plus)
        }
    };
    if adjust {
        match remainder.sign() {
            Sign::Minus => quotient -= BigInt::from(1_u8),
            Sign::Plus => quotient += BigInt::from(1_u8),
            Sign::NoSign => unreachable!("zero remainder returned before adjustment"),
        }
    }
    Ok(quotient)
}

fn decimal_add(left: ExactDecimal, right: ExactDecimal) -> Result<ExactDecimal, InterpreterError> {
    let (left, right, scale) = align_decimals(left, right, ExecutionOperation::Add)?;
    let coefficient = left.checked_add(right).ok_or(InterpreterError::Overflow {
        operation: ExecutionOperation::Add,
    })?;
    ExactDecimal::try_from_parts(coefficient, scale).map_err(|_| InterpreterError::Overflow {
        operation: ExecutionOperation::Add,
    })
}

fn decimal_subtract(
    left: ExactDecimal,
    right: ExactDecimal,
) -> Result<ExactDecimal, InterpreterError> {
    let (left, right, scale) = align_decimals(left, right, ExecutionOperation::Subtract)?;
    let coefficient = left.checked_sub(right).ok_or(InterpreterError::Overflow {
        operation: ExecutionOperation::Subtract,
    })?;
    ExactDecimal::try_from_parts(coefficient, scale).map_err(|_| InterpreterError::Overflow {
        operation: ExecutionOperation::Subtract,
    })
}

fn decimal_multiply(
    left: ExactDecimal,
    right: ExactDecimal,
) -> Result<ExactDecimal, InterpreterError> {
    let coefficient =
        left.coefficient()
            .checked_mul(right.coefficient())
            .ok_or(InterpreterError::Overflow {
                operation: ExecutionOperation::Multiply,
            })?;
    let scale = left
        .scale()
        .checked_add(right.scale())
        .filter(|scale| *scale <= ExactDecimal::MAX_SCALE)
        .ok_or(InterpreterError::Overflow {
            operation: ExecutionOperation::Multiply,
        })?;
    ExactDecimal::try_from_parts(coefficient, scale).map_err(|_| InterpreterError::Overflow {
        operation: ExecutionOperation::Multiply,
    })
}

fn decimal_divide(
    left: ExactDecimal,
    right: ExactDecimal,
    policy: DecimalDivisionPolicy,
) -> Result<ExactDecimal, InterpreterError> {
    validate_decimal_division_policy(policy)?;
    if right.coefficient() == 0 {
        return Err(InterpreterError::DivisionByZero {
            operation: ExecutionOperation::Divide,
        });
    }
    if left.coefficient() == 0 {
        return ExactDecimal::try_from_parts(0, policy.scale).map_err(|_| {
            InterpreterError::Overflow {
                operation: ExecutionOperation::Divide,
            }
        });
    }

    let mut numerator = BigInt::from(left.coefficient());
    let mut denominator = BigInt::from(right.coefficient());
    if denominator.sign() == Sign::Minus {
        numerator = -numerator;
        denominator = -denominator;
    }

    let exponent = i64::from(right.scale()) + i64::from(policy.scale) - i64::from(left.scale());
    if exponent >= 0 {
        let exponent = u32::try_from(exponent).map_err(|_| InterpreterError::Overflow {
            operation: ExecutionOperation::Divide,
        })?;
        numerator *= BigInt::from(10_u8).pow(exponent);
    } else {
        let exponent = u32::try_from(-exponent).map_err(|_| InterpreterError::Overflow {
            operation: ExecutionOperation::Divide,
        })?;
        denominator *= BigInt::from(10_u8).pow(exponent);
    }

    let quotient = &numerator / &denominator;
    let remainder = &numerator % &denominator;
    let quotient = round_exact_quotient(quotient, &remainder, &denominator, policy.rounding)
        .map_err(|error| match error {
            ExactRoundingError::Inexact => InterpreterError::NonTerminatingDecimalDivision,
            ExactRoundingError::ScaleTooLarge { .. } | ExactRoundingError::Overflow => {
                InterpreterError::Overflow {
                    operation: ExecutionOperation::Divide,
                }
            }
        })?;
    let coefficient = i128::try_from(quotient).map_err(|_| InterpreterError::Overflow {
        operation: ExecutionOperation::Divide,
    })?;
    ExactDecimal::try_from_parts(coefficient, policy.scale).map_err(|_| {
        InterpreterError::Overflow {
            operation: ExecutionOperation::Divide,
        }
    })
}

fn validate_decimal_division_policy(policy: DecimalDivisionPolicy) -> Result<(), InterpreterError> {
    if policy.scale > 18 {
        Err(StaticSpecError::InvalidDecimalDivisionScale {
            scale: policy.scale,
        }
        .into())
    } else {
        Ok(())
    }
}

fn decimal_remainder(
    left: ExactDecimal,
    right: ExactDecimal,
) -> Result<ExactDecimal, InterpreterError> {
    let (left, right, scale) = align_decimals(left, right, ExecutionOperation::Remainder)?;
    if right == 0 {
        return Err(InterpreterError::DivisionByZero {
            operation: ExecutionOperation::Remainder,
        });
    }
    let coefficient = left.checked_rem(right).ok_or(InterpreterError::Overflow {
        operation: ExecutionOperation::Remainder,
    })?;
    ExactDecimal::try_from_parts(coefficient, scale).map_err(|_| InterpreterError::Overflow {
        operation: ExecutionOperation::Remainder,
    })
}

fn decimal_compare(left: ExactDecimal, right: ExactDecimal) -> Result<Ordering, InterpreterError> {
    let (left, right, _) = align_decimals(left, right, ExecutionOperation::Compare)?;
    Ok(left.cmp(&right))
}

fn align_decimals(
    left: ExactDecimal,
    right: ExactDecimal,
    operation: ExecutionOperation,
) -> Result<(i128, i128, u32), InterpreterError> {
    let scale = left.scale().max(right.scale());
    let left = left
        .coefficient()
        .checked_mul(
            checked_pow10(scale - left.scale()).ok_or(InterpreterError::Overflow { operation })?,
        )
        .ok_or(InterpreterError::Overflow { operation })?;
    let right = right
        .coefficient()
        .checked_mul(
            checked_pow10(scale - right.scale()).ok_or(InterpreterError::Overflow { operation })?,
        )
        .ok_or(InterpreterError::Overflow { operation })?;
    Ok((left, right, scale))
}

fn checked_pow10(exponent: u32) -> Option<i128> {
    let mut value = 1_i128;
    for _ in 0..exponent {
        value = value.checked_mul(10)?;
    }
    Some(value)
}

fn parse_date_any(source: &str) -> Option<CanonicalDate> {
    [
        DateFormat::YearMonthDay,
        DateFormat::MonthSlashDaySlashYear,
        DateFormat::MonthDashDayDashYear,
    ]
    .into_iter()
    .find_map(|format| parse_date(source, format))
}

fn parse_date(source: &str, format: DateFormat) -> Option<CanonicalDate> {
    let (year, month, day) = match format {
        DateFormat::YearMonthDay => {
            let mut parts = source.split('-');
            let year = parts.next()?.parse().ok()?;
            let month = parts.next()?.parse().ok()?;
            let day = parts.next()?.parse().ok()?;
            if parts.next().is_some() {
                return None;
            }
            (year, month, day)
        }
        DateFormat::MonthSlashDaySlashYear => {
            let mut parts = source.split('/');
            let month = parts.next()?.parse().ok()?;
            let day = parts.next()?.parse().ok()?;
            let year = parts.next()?.parse().ok()?;
            if parts.next().is_some() {
                return None;
            }
            (year, month, day)
        }
        DateFormat::MonthDashDayDashYear => {
            let mut parts = source.split('-');
            let month = parts.next()?.parse().ok()?;
            let day = parts.next()?.parse().ok()?;
            let year = parts.next()?.parse().ok()?;
            if parts.next().is_some() {
                return None;
            }
            (year, month, day)
        }
    };
    CanonicalDate::try_new(year, month, day).ok()
}

fn format_date(date: CanonicalDate, format: DateFormat) -> String {
    match format {
        DateFormat::YearMonthDay => {
            format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
        }
        DateFormat::MonthSlashDaySlashYear => {
            format!("{:02}/{:02}/{:04}", date.month(), date.day(), date.year())
        }
        DateFormat::MonthDashDayDashYear => {
            format!("{:02}-{:02}-{:04}", date.month(), date.day(), date.year())
        }
    }
}

fn format_decimal(value: ExactDecimal, grouping: DecimalGrouping) -> String {
    let rendered = value.to_string();
    if grouping == DecimalGrouping::None {
        return rendered;
    }
    let (sign, unsigned) = rendered
        .strip_prefix('-')
        .map_or(("", rendered.as_str()), |value| ("-", value));
    let (integer, fraction) = unsigned
        .split_once('.')
        .map_or((unsigned, None), |(integer, fraction)| {
            (integer, Some(fraction))
        });
    let mut grouped = String::with_capacity(rendered.len() + integer.len() / 3);
    grouped.push_str(sign);
    for (index, character) in integer.chars().enumerate() {
        if index > 0 && (integer.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(character);
    }
    if let Some(fraction) = fraction {
        grouped.push('.');
        grouped.push_str(fraction);
    }
    grouped
}

#[derive(Clone, Copy)]
struct StaticValidationScope<'spec> {
    spec: &'spec StaticRuleSetSpec,
    profile: BehaviorProfile,
    phase: ValidationPhase,
    owner_kind: SpecItemKind,
    owner_id: &'static str,
    calculation: Option<&'spec CalculationSpec>,
    current_group: Option<&'static str>,
    trigger_field_ids: &'static [&'static str],
}

impl StaticValidationScope<'_> {
    fn with_current_group(self, current_group: &'static str) -> Self {
        Self {
            current_group: Some(current_group),
            ..self
        }
    }
}

const BEHAVIOR_PROFILES: [BehaviorProfile; 2] = [
    BehaviorProfile::OfficialCompatibility,
    BehaviorProfile::FilingSafe,
];

fn validate_static_spec(spec: &StaticRuleSetSpec) -> Result<(), InterpreterError> {
    validate_unique_ids(
        SpecItemKind::ContextValue,
        spec.context_values
            .iter()
            .map(|value| value.context_value_id),
    )?;
    validate_unique_ids(
        SpecItemKind::FieldGroup,
        spec.field_groups.iter().map(|group| group.group_id),
    )?;
    validate_unique_ids(
        SpecItemKind::Field,
        spec.fields.iter().map(|field| field.field_id),
    )?;
    validate_unique_ids(
        SpecItemKind::Calculation,
        spec.calculations
            .iter()
            .map(|calculation| calculation.calculation_id),
    )?;
    validate_unique_ids(
        SpecItemKind::Rule,
        spec.rules.iter().map(|rule| rule.rule_id),
    )?;

    for context in spec.context_values {
        parse_context_value_id(context.context_value_id)?;
    }
    for group in spec.field_groups {
        parse_group_id(group.group_id)?;
        if group
            .max_occurs
            .is_some_and(|maximum| group.min_occurs > maximum)
        {
            return Err(StaticSpecError::InvalidGroupCardinality {
                group_id: group.group_id,
                min_occurs: group.min_occurs,
                max_occurs: group.max_occurs.expect("checked as some"),
            }
            .into());
        }
        if group.members.is_empty() {
            return Err(StaticSpecError::EmptyRequiredList {
                kind: SpecItemKind::FieldGroup,
                value: group.group_id,
            }
            .into());
        }
        validate_unique_ids(SpecItemKind::Field, group.members.iter().copied())?;
        for member in group.members {
            let Some(field) = find_field(spec, member) else {
                return Err(StaticSpecError::InvalidReference {
                    kind: SpecItemKind::FieldGroup,
                    value: group.group_id,
                    target: member,
                }
                .into());
            };
            if field.group_id != Some(group.group_id) {
                return Err(StaticSpecError::InvalidReference {
                    kind: SpecItemKind::FieldGroup,
                    value: group.group_id,
                    target: member,
                }
                .into());
            }
        }
    }
    for field in spec.fields {
        parse_field_id(field.field_id)?;
        if let Some(group_id) = field.group_id {
            let Some(group) = find_group(spec, group_id) else {
                return Err(StaticSpecError::InvalidReference {
                    kind: SpecItemKind::Field,
                    value: field.field_id,
                    target: group_id,
                }
                .into());
            };
            if !group.members.contains(&field.field_id) {
                return Err(StaticSpecError::InvalidReference {
                    kind: SpecItemKind::Field,
                    value: field.field_id,
                    target: group_id,
                }
                .into());
            }
        }
        if let Some(calculation_id) = field.calculation_id {
            parse_calculation_id(calculation_id)?;
            if !spec
                .calculations
                .iter()
                .any(|calculation| calculation.calculation_id == calculation_id)
            {
                return Err(StaticSpecError::InvalidReference {
                    kind: SpecItemKind::Field,
                    value: field.field_id,
                    target: calculation_id,
                }
                .into());
            }
        }
        for branch in [field.behavior.official, field.behavior.filing_safe] {
            if let Branch::Executable(behavior) = branch {
                validate_coercion(behavior.coercion)?;
                validate_normalization(behavior.normalization)?;
                if behavior.normalization.iter().any(|step| {
                    matches!(
                        step,
                        NormalizationStep::OfflineEbirMoneyRoundV1
                            | NormalizationStep::OfflineEbirParseFloatFixedZeroV1
                    )
                }) {
                    return Err(StaticSpecError::InvalidEventBinding {
                        kind: SpecItemKind::Field,
                        value: field.field_id,
                    }
                    .into());
                }
                let mut event_phases = Vec::new();
                for event in behavior.event_normalization {
                    if !event.phase.is_field_event() {
                        return Err(StaticSpecError::InvalidEventBinding {
                            kind: SpecItemKind::Field,
                            value: field.field_id,
                        }
                        .into());
                    }
                    if event_phases.contains(&event.phase) {
                        return Err(StaticSpecError::DuplicatePhase {
                            kind: SpecItemKind::Field,
                            value: field.field_id,
                            phase: event.phase,
                        }
                        .into());
                    }
                    if event.normalization.is_empty() {
                        return Err(StaticSpecError::EmptyRequiredList {
                            kind: SpecItemKind::Field,
                            value: field.field_id,
                        }
                        .into());
                    }
                    let has_exact_offline_event_helper = event.normalization.iter().any(|step| {
                        matches!(
                            step,
                            NormalizationStep::OfflineEbirMoneyRoundV1
                                | NormalizationStep::OfflineEbirParseFloatFixedZeroV1
                        )
                    });
                    if has_exact_offline_event_helper
                        && (event.phase != ValidationPhase::Blur
                            || event.normalization.len() != 1
                            || !behavior.normalization.is_empty()
                            || field.value_type != ValueType::String
                            || !matches!(behavior.coercion, Coercion::String { .. }))
                    {
                        return Err(StaticSpecError::InvalidEventBinding {
                            kind: SpecItemKind::Field,
                            value: field.field_id,
                        }
                        .into());
                    }
                    event_phases.push(event.phase);
                    validate_normalization(event.normalization)?;
                }
                require_static_type(
                    field.value_type,
                    coercion_result_type(behavior.coercion),
                    ExecutionOperation::Coercion,
                    false,
                )?;
            }
        }
    }

    validate_evaluation_order(spec)?;
    let mut rule_orders = Vec::new();
    for rule in spec.rules {
        parse_rule_id(rule.rule_id)?;
        validate_evaluation_scope(spec, SpecItemKind::Rule, rule.rule_id, rule.scope)?;
        validate_phases(SpecItemKind::Rule, rule.rule_id, rule.phases)?;
        validate_event_binding(
            spec,
            SpecItemKind::Rule,
            rule.rule_id,
            rule.scope,
            rule.phases,
            rule.trigger_field_ids,
        )?;
        if rule.order == 0 {
            return Err(StaticSpecError::InvalidRuleOrder {
                rule_id: rule.rule_id,
            }
            .into());
        }
        for phase in rule.phases {
            if rule_orders.contains(&(*phase, rule.order)) {
                return Err(StaticSpecError::DuplicateRuleOrder {
                    phase: *phase,
                    order: rule.order,
                }
                .into());
            }
            rule_orders.push((*phase, rule.order));
        }
        for profile in BEHAVIOR_PROFILES {
            let branch = rule.profiles.select(profile);
            if let Branch::Executable(branch) = branch {
                if branch.effects.is_empty() {
                    return Err(StaticSpecError::EmptyRequiredList {
                        kind: SpecItemKind::Rule,
                        value: rule.rule_id,
                    }
                    .into());
                }
                for phase in rule.phases {
                    let scope = StaticValidationScope {
                        spec,
                        profile,
                        phase: *phase,
                        owner_kind: SpecItemKind::Rule,
                        owner_id: rule.rule_id,
                        calculation: None,
                        current_group: evaluation_scope_group(rule.scope),
                        trigger_field_ids: rule.trigger_field_ids,
                    };
                    validate_predicate(branch.predicate, scope)?;
                    for effect in branch.effects {
                        validate_effect(effect, scope)?;
                    }
                }
            }
        }
    }
    validate_field_event_programs(spec)?;
    validate_workflow(spec)?;
    Ok(())
}

fn validate_field_event_programs(spec: &StaticRuleSetSpec) -> Result<(), InterpreterError> {
    let mut bindings = HashSet::new();
    for program in spec.field_event_programs {
        if !program.phase.is_field_event()
            || find_field(spec, program.trigger_field_id).is_none()
            || !bindings.insert((program.phase, program.trigger_field_id))
        {
            return Err(StaticSpecError::InvalidEventBinding {
                kind: SpecItemKind::FieldEventProgram,
                value: program.trigger_field_id,
            }
            .into());
        }

        for profile in BEHAVIOR_PROFILES {
            let Branch::Executable(branch) = program.profiles.select(profile) else {
                continue;
            };
            let mut output_slots = BTreeSet::new();
            let mut seen_rules = BTreeSet::new();
            let mut previous_rule = None;
            for step in branch.steps {
                match step {
                    FieldEventStep::Calculation {
                        calculation_id,
                        output_ids,
                        write_mode,
                    } => {
                        let Some(calculation) = spec
                            .calculations
                            .iter()
                            .find(|candidate| candidate.calculation_id == *calculation_id)
                        else {
                            return Err(StaticSpecError::InvalidReference {
                                kind: SpecItemKind::FieldEventProgram,
                                value: program.trigger_field_id,
                                target: calculation_id,
                            }
                            .into());
                        };
                        if !calculation.phases.contains(&program.phase)
                            || !calculation
                                .trigger_field_ids
                                .contains(&program.trigger_field_id)
                            || output_ids.is_empty()
                        {
                            return Err(StaticSpecError::InvalidEventBinding {
                                kind: SpecItemKind::Calculation,
                                value: calculation.calculation_id,
                            }
                            .into());
                        }
                        let Branch::Executable(calculation_branch) =
                            calculation.profiles.select(profile)
                        else {
                            return Err(StaticSpecError::InvalidReference {
                                kind: SpecItemKind::FieldEventProgram,
                                value: program.trigger_field_id,
                                target: calculation.calculation_id,
                            }
                            .into());
                        };
                        let scope = StaticValidationScope {
                            spec,
                            profile,
                            phase: program.phase,
                            owner_kind: SpecItemKind::Calculation,
                            owner_id: calculation.calculation_id,
                            calculation: Some(calculation),
                            current_group: evaluation_scope_group(calculation.scope),
                            trigger_field_ids: calculation.trigger_field_ids,
                        };
                        let mut previous_position = None;
                        let mut selected = BTreeSet::new();
                        for output_id in *output_ids {
                            if !selected.insert(*output_id) {
                                return Err(StaticSpecError::DuplicateIdentifier {
                                    kind: SpecItemKind::Output,
                                    value: output_id,
                                }
                                .into());
                            }
                            let Some(position) = calculation_branch
                                .outputs
                                .iter()
                                .position(|output| output.output_id == *output_id)
                            else {
                                return Err(StaticSpecError::InvalidReference {
                                    kind: SpecItemKind::FieldEventProgram,
                                    value: program.trigger_field_id,
                                    target: output_id,
                                }
                                .into());
                            };
                            if previous_position.is_some_and(|previous| position <= previous) {
                                return Err(StaticSpecError::InvalidReference {
                                    kind: SpecItemKind::FieldEventProgram,
                                    value: program.trigger_field_id,
                                    target: output_id,
                                }
                                .into());
                            }
                            previous_position = Some(position);
                            let output = &calculation_branch.outputs[position];
                            let Some(writeback) = output.writeback else {
                                return Err(StaticSpecError::InvalidReference {
                                    kind: SpecItemKind::Output,
                                    value: output.output_id,
                                    target: "writeback",
                                }
                                .into());
                            };
                            let output_type = expression_result_type(output.value);
                            if !matches!(output_type, ValueType::Integer | ValueType::Decimal) {
                                return Err(StaticSpecError::TypeMismatch {
                                    operation: ExecutionOperation::CalculationWriteback,
                                    expected: ValueType::Decimal,
                                    actual: output_type,
                                }
                                .into());
                            }
                            let target = validate_field_ref(writeback.field, scope)?;
                            if target.value_type != ValueType::String
                                || target.calculation_id != Some(calculation.calculation_id)
                            {
                                return Err(StaticSpecError::InvalidReference {
                                    kind: SpecItemKind::Output,
                                    value: output.output_id,
                                    target: target.field_id,
                                }
                                .into());
                            }
                            let slot = (calculation.calculation_id, output.output_id);
                            let valid_write = match write_mode {
                                ScheduledOutputWriteMode::Insert => output_slots.insert(slot),
                                ScheduledOutputWriteMode::Replace => output_slots.contains(&slot),
                            };
                            if !valid_write {
                                return Err(StaticSpecError::InvalidReference {
                                    kind: SpecItemKind::Output,
                                    value: output.output_id,
                                    target: "scheduled-write-mode",
                                }
                                .into());
                            }
                        }
                    }
                    FieldEventStep::Rule { rule_id } => {
                        let Some(rule) = spec
                            .rules
                            .iter()
                            .find(|candidate| candidate.rule_id == *rule_id)
                        else {
                            return Err(StaticSpecError::InvalidReference {
                                kind: SpecItemKind::FieldEventProgram,
                                value: program.trigger_field_id,
                                target: rule_id,
                            }
                            .into());
                        };
                        if !rule.phases.contains(&program.phase)
                            || !rule.trigger_field_ids.contains(&program.trigger_field_id)
                            || !seen_rules.insert(rule.rule_id)
                            || previous_rule
                                .is_some_and(|previous| (rule.order, rule.rule_id) <= previous)
                            || !matches!(rule.profiles.select(profile), Branch::Executable(_))
                        {
                            return Err(StaticSpecError::InvalidEventBinding {
                                kind: SpecItemKind::Rule,
                                value: rule.rule_id,
                            }
                            .into());
                        }
                        previous_rule = Some((rule.order, rule.rule_id));
                    }
                }
            }
        }
    }

    for profile in BEHAVIOR_PROFILES {
        for field in spec.fields {
            let Branch::Executable(behavior) = field.behavior.select(profile) else {
                continue;
            };
            for normalization in behavior.event_normalization {
                if !spec.field_event_programs.iter().any(|program| {
                    program.phase == normalization.phase
                        && program.trigger_field_id == field.field_id
                        && matches!(program.profiles.select(profile), Branch::Executable(_))
                }) {
                    return Err(StaticSpecError::InvalidEventBinding {
                        kind: SpecItemKind::Field,
                        value: field.field_id,
                    }
                    .into());
                }
            }
        }
        for calculation in spec.calculations {
            let Branch::Executable(calculation_branch) = calculation.profiles.select(profile)
            else {
                continue;
            };
            let declared_event_phases = calculation
                .phases
                .iter()
                .copied()
                .filter(|phase| phase.is_field_event())
                .collect::<HashSet<_>>();
            let declared_triggers = calculation
                .trigger_field_ids
                .iter()
                .copied()
                .collect::<HashSet<_>>();
            let mut scheduled_event_phases = HashSet::new();
            let mut scheduled_triggers = HashSet::new();
            for program in spec.field_event_programs {
                if matches!(program.profiles.select(profile), Branch::Executable(branch)
                if branch.steps.iter().any(|step| matches!(
                    step,
                    FieldEventStep::Calculation { calculation_id, .. }
                        if *calculation_id == calculation.calculation_id
                ))) {
                    scheduled_event_phases.insert(program.phase);
                    scheduled_triggers.insert(program.trigger_field_id);
                }
            }
            if scheduled_event_phases != declared_event_phases
                || scheduled_triggers != declared_triggers
            {
                return Err(StaticSpecError::InvalidEventBinding {
                    kind: SpecItemKind::Calculation,
                    value: calculation.calculation_id,
                }
                .into());
            }
            for output in calculation_branch
                .outputs
                .iter()
                .filter(|output| output.writeback.is_some())
            {
                let output_is_scheduled = spec.field_event_programs.iter().any(|program| {
                    matches!(program.profiles.select(profile), Branch::Executable(branch)
                    if branch.steps.iter().any(|step| matches!(
                        step,
                        FieldEventStep::Calculation {
                            calculation_id,
                            output_ids,
                            ..
                        } if *calculation_id == calculation.calculation_id
                            && output_ids.contains(&output.output_id)
                    )))
                });
                if declared_event_phases.is_empty() || !output_is_scheduled {
                    return Err(StaticSpecError::InvalidEventBinding {
                        kind: SpecItemKind::Output,
                        value: output.output_id,
                    }
                    .into());
                }
            }
        }
        for rule in spec.rules {
            if !matches!(rule.profiles.select(profile), Branch::Executable(_)) {
                continue;
            }
            let declared_event_phases = rule
                .phases
                .iter()
                .copied()
                .filter(|phase| phase.is_field_event())
                .collect::<HashSet<_>>();
            let declared_triggers = rule
                .trigger_field_ids
                .iter()
                .copied()
                .collect::<HashSet<_>>();
            let mut scheduled_event_phases = HashSet::new();
            let mut scheduled_triggers = HashSet::new();
            for program in spec.field_event_programs {
                if matches!(program.profiles.select(profile), Branch::Executable(branch)
                if branch.steps.iter().any(|step| matches!(
                    step,
                    FieldEventStep::Rule { rule_id } if *rule_id == rule.rule_id
                ))) {
                    scheduled_event_phases.insert(program.phase);
                    scheduled_triggers.insert(program.trigger_field_id);
                }
            }
            if scheduled_event_phases != declared_event_phases
                || scheduled_triggers != declared_triggers
            {
                return Err(StaticSpecError::InvalidEventBinding {
                    kind: SpecItemKind::Rule,
                    value: rule.rule_id,
                }
                .into());
            }
        }
    }
    Ok(())
}

fn validate_workflow(spec: &StaticRuleSetSpec) -> Result<(), InterpreterError> {
    let Branch::Executable(workflow) = spec.workflow else {
        return Ok(());
    };
    parse_workflow_state_id(workflow.initial_state)?;
    validate_unique_ids(
        SpecItemKind::WorkflowState,
        workflow.states.iter().map(|state| state.state_id),
    )?;
    validate_unique_ids(
        SpecItemKind::WorkflowTransition,
        workflow
            .transitions
            .iter()
            .map(|transition| transition.transition_id),
    )?;
    if !workflow
        .states
        .iter()
        .any(|state| state.state_id == workflow.initial_state)
    {
        return Err(StaticSpecError::InvalidReference {
            kind: SpecItemKind::WorkflowState,
            value: workflow.initial_state,
            target: workflow.initial_state,
        }
        .into());
    }
    for state in workflow.states {
        parse_workflow_state_id(state.state_id)?;
    }
    for transition in workflow.transitions {
        parse_workflow_transition_id(transition.transition_id)?;
        for state_id in [transition.from_state, transition.to_state] {
            parse_workflow_state_id(state_id)?;
            if !workflow
                .states
                .iter()
                .any(|state| state.state_id == state_id)
            {
                return Err(StaticSpecError::InvalidReference {
                    kind: SpecItemKind::WorkflowTransition,
                    value: transition.transition_id,
                    target: state_id,
                }
                .into());
            }
        }
        for profile in BEHAVIOR_PROFILES {
            let Branch::Executable(branch) = transition.profiles.select(profile) else {
                continue;
            };
            let scope = StaticValidationScope {
                spec,
                profile,
                phase: transition.evaluation_phase,
                owner_kind: SpecItemKind::WorkflowTransition,
                owner_id: transition.transition_id,
                calculation: None,
                current_group: None,
                trigger_field_ids: &[],
            };
            validate_predicate(branch.guard, scope)?;
            let mut state_effects = 0;
            for effect in branch.effects {
                match effect {
                    Effect::SetWorkflowState { state_id } => {
                        parse_workflow_state_id(state_id)?;
                        if *state_id != transition.to_state {
                            return Err(StaticSpecError::InvalidReference {
                                kind: SpecItemKind::WorkflowTransition,
                                value: transition.transition_id,
                                target: state_id,
                            }
                            .into());
                        }
                        state_effects += 1;
                    }
                    Effect::EmitNotification { message, .. } => {
                        if message.is_empty() {
                            return Err(StaticSpecError::EmptyRequiredList {
                                kind: SpecItemKind::WorkflowTransition,
                                value: "notification-message",
                            }
                            .into());
                        }
                    }
                    effect => {
                        return Err(StaticSpecError::UnsupportedEffect {
                            kind: SpecItemKind::WorkflowTransition,
                            value: transition.transition_id,
                            effect: effect.kind(),
                        }
                        .into());
                    }
                }
            }
            if state_effects != 1 {
                return Err(StaticSpecError::EmptyRequiredList {
                    kind: SpecItemKind::WorkflowTransition,
                    value: transition.transition_id,
                }
                .into());
            }
        }
    }
    Ok(())
}

fn validate_evaluation_order(spec: &StaticRuleSetSpec) -> Result<(), InterpreterError> {
    for (index, calculation_id) in spec.evaluation_order.iter().enumerate() {
        parse_calculation_id(calculation_id)?;
        if spec.evaluation_order[..index].contains(calculation_id) {
            return Err(StaticSpecError::EvaluationOrderDuplicate { calculation_id }.into());
        }
        if !spec
            .calculations
            .iter()
            .any(|calculation| calculation.calculation_id == *calculation_id)
        {
            return Err(StaticSpecError::EvaluationOrderUnknown { calculation_id }.into());
        }
    }
    for calculation in spec.calculations {
        parse_calculation_id(calculation.calculation_id)?;
        validate_evaluation_scope(
            spec,
            SpecItemKind::Calculation,
            calculation.calculation_id,
            calculation.scope,
        )?;
        validate_phases(
            SpecItemKind::Calculation,
            calculation.calculation_id,
            calculation.phases,
        )?;
        validate_event_binding(
            spec,
            SpecItemKind::Calculation,
            calculation.calculation_id,
            calculation.scope,
            calculation.phases,
            calculation.trigger_field_ids,
        )?;
        let Some(position) = spec
            .evaluation_order
            .iter()
            .position(|id| *id == calculation.calculation_id)
        else {
            return Err(StaticSpecError::EvaluationOrderMissing {
                calculation_id: calculation.calculation_id,
            }
            .into());
        };
        validate_unique_ids(
            SpecItemKind::Calculation,
            calculation.depends_on.iter().copied(),
        )?;
        for dependency in calculation.depends_on {
            let Some(dependency_position) =
                spec.evaluation_order.iter().position(|id| id == dependency)
            else {
                return Err(StaticSpecError::InvalidReference {
                    kind: SpecItemKind::Calculation,
                    value: calculation.calculation_id,
                    target: dependency,
                }
                .into());
            };
            if dependency_position >= position {
                return Err(StaticSpecError::DependencyOutOfOrder {
                    calculation_id: calculation.calculation_id,
                    dependency_id: dependency,
                }
                .into());
            }
        }
        for profile in BEHAVIOR_PROFILES {
            let branch = calculation.profiles.select(profile);
            if let Branch::Executable(branch) = branch {
                if branch.outputs.is_empty() {
                    return Err(StaticSpecError::EmptyRequiredList {
                        kind: SpecItemKind::Calculation,
                        value: calculation.calculation_id,
                    }
                    .into());
                }
                validate_unique_ids(
                    SpecItemKind::Output,
                    branch.outputs.iter().map(|output| output.output_id),
                )?;
                for output in branch.outputs {
                    parse_output_id(output.output_id)?;
                    if let Some(rounding) = output.rounding {
                        if rounding.is_empty() {
                            return Err(StaticSpecError::EmptyRequiredList {
                                kind: SpecItemKind::Output,
                                value: output.output_id,
                            }
                            .into());
                        }
                        for step in rounding {
                            validate_rounding(*step)?;
                        }
                    }
                }
                for phase in calculation.phases {
                    validate_dependency_availability(spec, calculation, profile, *phase)?;
                    let scope = StaticValidationScope {
                        spec,
                        profile,
                        phase: *phase,
                        owner_kind: SpecItemKind::Calculation,
                        owner_id: calculation.calculation_id,
                        calculation: Some(calculation),
                        current_group: evaluation_scope_group(calculation.scope),
                        trigger_field_ids: calculation.trigger_field_ids,
                    };
                    validate_predicate(branch.condition, scope)?;
                    for output in branch.outputs {
                        let value_type = validate_expression(output.value, scope)?;
                        if output.rounding.is_some() {
                            require_static_type(
                                ValueType::Decimal,
                                value_type,
                                ExecutionOperation::Rounding,
                                true,
                            )?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn validate_evaluation_scope(
    spec: &StaticRuleSetSpec,
    kind: SpecItemKind,
    owner_id: &'static str,
    scope: EvaluationScope,
) -> Result<(), InterpreterError> {
    let EvaluationScope::EachGroup(group_id) = scope else {
        return Ok(());
    };
    parse_group_id(group_id)?;
    if find_group(spec, group_id).is_none() {
        return Err(StaticSpecError::InvalidReference {
            kind,
            value: owner_id,
            target: group_id,
        }
        .into());
    }
    Ok(())
}

const fn evaluation_scope_group(scope: EvaluationScope) -> Option<&'static str> {
    match scope {
        EvaluationScope::Singleton => None,
        EvaluationScope::EachGroup(group_id) => Some(group_id),
    }
}

fn validate_phases(
    kind: SpecItemKind,
    value: &'static str,
    phases: &[ValidationPhase],
) -> Result<(), InterpreterError> {
    if phases.is_empty() {
        return Err(StaticSpecError::EmptyRequiredList { kind, value }.into());
    }
    for (index, phase) in phases.iter().enumerate() {
        if phases[..index].contains(phase) {
            return Err(StaticSpecError::DuplicatePhase {
                kind,
                value,
                phase: *phase,
            }
            .into());
        }
    }
    Ok(())
}

fn validate_event_binding(
    spec: &StaticRuleSetSpec,
    kind: SpecItemKind,
    value: &'static str,
    scope: EvaluationScope,
    phases: &[ValidationPhase],
    trigger_field_ids: &[&'static str],
) -> Result<(), InterpreterError> {
    let event_phase = phases.iter().any(|phase| phase.is_field_event());
    if event_phase == trigger_field_ids.is_empty() {
        return Err(if event_phase {
            StaticSpecError::EmptyRequiredList { kind, value }
        } else {
            StaticSpecError::InvalidEventBinding { kind, value }
        }
        .into());
    }
    validate_unique_ids(SpecItemKind::Field, trigger_field_ids.iter().copied())?;
    for trigger_field_id in trigger_field_ids {
        parse_field_id(trigger_field_id)?;
        let Some(field) = find_field(spec, trigger_field_id) else {
            return Err(StaticSpecError::InvalidReference {
                kind,
                value,
                target: trigger_field_id,
            }
            .into());
        };
        let matches_scope = match scope {
            EvaluationScope::Singleton => field.group_id.is_none(),
            EvaluationScope::EachGroup(group_id) => field.group_id == Some(group_id),
        };
        if !matches_scope {
            return Err(StaticSpecError::InvalidReference {
                kind,
                value,
                target: trigger_field_id,
            }
            .into());
        }
    }
    Ok(())
}

fn validate_unique_ids(
    kind: SpecItemKind,
    values: impl IntoIterator<Item = &'static str>,
) -> Result<(), InterpreterError> {
    let mut seen = Vec::new();
    for value in values {
        if seen.contains(&value) {
            return Err(StaticSpecError::DuplicateIdentifier { kind, value }.into());
        }
        seen.push(value);
    }
    Ok(())
}

// `is_empty()` guards state the spec rule being enforced; matching an
// empty literal instead would obscure why the spec is rejected.
#[allow(clippy::redundant_guards)]
fn validate_coercion(coercion: Coercion) -> Result<(), InterpreterError> {
    match coercion {
        Coercion::Decimal { decimal, .. } => {
            validate_decimal_policy(decimal)?;
        }
        Coercion::Boolean {
            true_values,
            false_values,
            ..
        } => {
            if true_values.is_empty() || false_values.is_empty() {
                return Err(StaticSpecError::EmptyRequiredList {
                    kind: SpecItemKind::Field,
                    value: "boolean-coercion",
                }
                .into());
            }
            validate_unique_ids(SpecItemKind::Field, true_values.iter().copied())?;
            validate_unique_ids(SpecItemKind::Field, false_values.iter().copied())?;
            if let Some(value) = true_values
                .iter()
                .copied()
                .find(|value| false_values.contains(value))
            {
                return Err(StaticSpecError::AmbiguousBooleanCoercionValue { value }.into());
            }
        }
        Coercion::Date {
            accepted_formats, ..
        } if accepted_formats.is_empty() => {
            return Err(StaticSpecError::EmptyRequiredList {
                kind: SpecItemKind::Field,
                value: "date-coercion",
            }
            .into());
        }
        _ => {}
    }
    Ok(())
}

fn validate_decimal_policy(policy: DecimalPolicy) -> Result<(), InterpreterError> {
    if !(1..=38).contains(&policy.precision)
        || policy.scale > policy.precision
        || policy.scale > 18
        || policy.division_scale > 18
    {
        return Err(StaticSpecError::InvalidDecimalPolicy {
            precision: policy.precision,
            scale: policy.scale,
            division_scale: policy.division_scale,
        }
        .into());
    }
    validate_rounding(policy.rounding)
}

fn validate_rounding(rounding: Rounding) -> Result<(), InterpreterError> {
    if rounding.scale > 18 {
        Err(StaticSpecError::InvalidRoundingScale {
            scale: rounding.scale,
        }
        .into())
    } else {
        Ok(())
    }
}

// `is_empty()` guards state the spec rule being enforced; matching an
// empty literal instead would obscure why the spec is rejected.
#[allow(clippy::redundant_guards)]
fn validate_normalization(pipeline: &[NormalizationStep]) -> Result<(), InterpreterError> {
    for step in pipeline {
        match step {
            NormalizationStep::ReplaceLiteral { from, .. } if from.is_empty() => {
                return Err(StaticSpecError::EmptyRequiredList {
                    kind: SpecItemKind::Field,
                    value: "replace-literal",
                }
                .into());
            }
            NormalizationStep::StripCharacters { characters } if characters.is_empty() => {
                return Err(StaticSpecError::EmptyRequiredList {
                    kind: SpecItemKind::Field,
                    value: "strip-characters",
                }
                .into());
            }
            NormalizationStep::DecimalFormat { rounding, .. } => {
                validate_rounding(*rounding)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_expression(
    expression: &Expression,
    scope: StaticValidationScope<'_>,
) -> Result<ValueType, InterpreterError> {
    match expression {
        Expression::Literal(value) => {
            literal_value(*value)?;
            Ok(typed_value_type(*value))
        }
        Expression::Field { result_type, field } => {
            let field = validate_field_ref(*field, scope)?;
            require_static_type(
                field.value_type,
                *result_type,
                ExecutionOperation::FieldLookup,
                false,
            )?;
            Ok(*result_type)
        }
        Expression::Derived {
            result_type,
            calculation_id,
            output_id,
            instance,
        } => {
            let actual = resolve_derived_output(scope, calculation_id, output_id, *instance)?;
            require_static_type(
                actual,
                *result_type,
                ExecutionOperation::DerivedLookup,
                false,
            )?;
            Ok(*result_type)
        }
        Expression::Context {
            result_type,
            context_value_id,
        } => {
            parse_context_value_id(context_value_id)?;
            let Some(context) = scope
                .spec
                .context_values
                .iter()
                .find(|candidate| candidate.context_value_id == *context_value_id)
            else {
                return Err(invalid_static_reference(scope, context_value_id));
            };
            require_static_type(
                context.value_type,
                *result_type,
                ExecutionOperation::ContextLookup,
                false,
            )?;
            Ok(*result_type)
        }
        Expression::Unary {
            result_type,
            operator,
            operand,
        } => {
            let operand = validate_expression(operand, scope)?;
            match operator {
                UnaryOperator::Negate | UnaryOperator::Absolute => {
                    require_numeric_type(*result_type, unary_operation(*operator))?;
                    require_static_type(*result_type, operand, unary_operation(*operator), true)?;
                }
                UnaryOperator::Length => {
                    require_static_type(
                        ValueType::Integer,
                        *result_type,
                        ExecutionOperation::UnaryLength,
                        false,
                    )?;
                    require_static_type(
                        ValueType::String,
                        operand,
                        ExecutionOperation::UnaryLength,
                        true,
                    )?;
                }
            }
            Ok(*result_type)
        }
        Expression::Binary {
            result_type,
            operator,
            division_policy,
            left,
            right,
        } => {
            let left = validate_expression(left, scope)?;
            let right = validate_expression(right, scope)?;
            let operation = binary_operation(*operator);
            match operator {
                BinaryOperator::Divide => {
                    let policy =
                        (*division_policy).ok_or(StaticSpecError::MissingDecimalDivisionPolicy)?;
                    validate_decimal_division_policy(policy)?;
                    require_static_type(ValueType::Decimal, *result_type, operation, false)?;
                    require_static_type(ValueType::Decimal, left, operation, true)?;
                    require_static_type(ValueType::Decimal, right, operation, true)?;
                }
                BinaryOperator::Concat => {
                    if division_policy.is_some() {
                        return Err(StaticSpecError::UnexpectedDecimalDivisionPolicy {
                            operator: *operator,
                        }
                        .into());
                    }
                    require_static_type(ValueType::String, *result_type, operation, false)?;
                    require_static_type(ValueType::String, left, operation, true)?;
                    require_static_type(ValueType::String, right, operation, true)?;
                }
                _ => {
                    if division_policy.is_some() {
                        return Err(StaticSpecError::UnexpectedDecimalDivisionPolicy {
                            operator: *operator,
                        }
                        .into());
                    }
                    require_numeric_type(*result_type, operation)?;
                    require_static_type(*result_type, left, operation, true)?;
                    require_static_type(*result_type, right, operation, true)?;
                }
            }
            Ok(*result_type)
        }
        Expression::Nary {
            result_type,
            operator,
            operands,
        } => {
            if operands.is_empty() {
                return Err(StaticSpecError::EmptyRequiredList {
                    kind: SpecItemKind::Output,
                    value: "nary-expression",
                }
                .into());
            }
            let operation = nary_operation(*operator);
            match operator {
                NaryOperator::Sum => require_numeric_type(*result_type, operation)?,
                NaryOperator::Minimum | NaryOperator::Maximum => {
                    require_orderable_type(*result_type, operation)?;
                }
                NaryOperator::Concat => {
                    require_static_type(ValueType::String, *result_type, operation, false)?;
                }
                NaryOperator::Coalesce => {}
            }
            for operand in *operands {
                let operand = validate_expression(operand, scope)?;
                require_static_type(*result_type, operand, operation, true)?;
            }
            Ok(*result_type)
        }
        Expression::Conditional {
            result_type,
            condition,
            when_true,
            when_false,
        } => {
            validate_predicate(condition, scope)?;
            let when_true = validate_expression(when_true, scope)?;
            let when_false = validate_expression(when_false, scope)?;
            require_static_type(*result_type, when_true, ExecutionOperation::Compare, true)?;
            require_static_type(*result_type, when_false, ExecutionOperation::Compare, true)?;
            Ok(*result_type)
        }
        Expression::Coerce {
            result_type,
            input,
            coercion,
        } => {
            let input = validate_expression(input, scope)?;
            validate_coercion(*coercion)?;
            require_static_type(ValueType::String, input, ExecutionOperation::Coercion, true)?;
            require_static_type(
                coercion_result_type(*coercion),
                *result_type,
                ExecutionOperation::Coercion,
                false,
            )?;
            Ok(*result_type)
        }
        Expression::SplitComponent {
            result_type,
            input,
            delimiter,
            ..
        } => {
            if *delimiter != "/" {
                return Err(StaticSpecError::InvalidReference {
                    kind: scope.owner_kind,
                    value: scope.owner_id,
                    target: "split-component-delimiter",
                }
                .into());
            }
            let input = validate_expression(input, scope)?;
            require_static_type(
                ValueType::String,
                input,
                ExecutionOperation::SplitComponent,
                true,
            )?;
            require_static_type(
                ValueType::String,
                *result_type,
                ExecutionOperation::SplitComponent,
                false,
            )?;
            Ok(*result_type)
        }
        Expression::JavaScriptParseIntRadix10 { result_type, input } => {
            let input = validate_expression(input, scope)?;
            require_static_type(
                ValueType::String,
                input,
                ExecutionOperation::JavaScriptParseIntRadix10,
                true,
            )?;
            require_static_type(
                ValueType::Integer,
                *result_type,
                ExecutionOperation::JavaScriptParseIntRadix10,
                false,
            )?;
            Ok(*result_type)
        }
        Expression::JavaScriptDateLocalDay {
            result_type,
            year,
            month_index,
            day,
        } => {
            for component in [*year, *month_index, *day] {
                let component = validate_expression(component, scope)?;
                require_static_type(
                    ValueType::Integer,
                    component,
                    ExecutionOperation::JavaScriptDateLocalDay,
                    true,
                )?;
            }
            require_static_type(
                ValueType::Integer,
                *result_type,
                ExecutionOperation::JavaScriptDateLocalDay,
                false,
            )?;
            Ok(*result_type)
        }
        Expression::CanonicalLocalDateDay { result_type, input } => {
            let input = validate_expression(input, scope)?;
            require_static_type(
                ValueType::Date,
                input,
                ExecutionOperation::CanonicalLocalDateDay,
                true,
            )?;
            require_static_type(
                ValueType::Integer,
                *result_type,
                ExecutionOperation::CanonicalLocalDateDay,
                false,
            )?;
            Ok(*result_type)
        }
        Expression::GroupAggregate {
            result_type,
            operator,
            group_id,
            value,
        } => {
            parse_group_id(group_id)?;
            let Some(group) = find_group(scope.spec, group_id) else {
                return Err(invalid_static_reference(scope, group_id));
            };
            if scope.current_group.is_some() {
                return Err(StaticSpecError::InvalidReference {
                    kind: scope.owner_kind,
                    value: scope.owner_id,
                    target: "nested-group-aggregate",
                }
                .into());
            }
            let value_type = validate_expression(value, scope.with_current_group(group.group_id))?;
            let actual = match operator {
                GroupAggregateOperator::Count | GroupAggregateOperator::CountPresent => {
                    ValueType::Integer
                }
                GroupAggregateOperator::Sum => {
                    require_numeric_type(value_type, ExecutionOperation::GroupAggregate)?;
                    value_type
                }
                GroupAggregateOperator::Minimum | GroupAggregateOperator::Maximum => {
                    require_orderable_type(value_type, ExecutionOperation::GroupAggregate)?;
                    value_type
                }
            };
            require_static_type(
                actual,
                *result_type,
                ExecutionOperation::GroupAggregate,
                false,
            )?;
            Ok(*result_type)
        }
    }
}

fn validate_predicate(
    predicate: &Predicate,
    scope: StaticValidationScope<'_>,
) -> Result<(), InterpreterError> {
    match predicate {
        Predicate::Constant(_) => {}
        Predicate::Not(predicate) => validate_predicate(predicate, scope)?,
        Predicate::All(predicates) | Predicate::Any(predicates) => {
            if predicates.is_empty() {
                return Err(StaticSpecError::EmptyRequiredList {
                    kind: SpecItemKind::Rule,
                    value: "logical-predicate",
                }
                .into());
            }
            for predicate in *predicates {
                validate_predicate(predicate, scope)?;
            }
        }
        Predicate::Compare {
            operator,
            left,
            right,
        } => {
            let left = validate_expression(left, scope)?;
            let right = validate_expression(right, scope)?;
            require_compatible_types(left, right, ExecutionOperation::Compare)?;
            if !matches!(operator, CompareOperator::Equal | CompareOperator::NotEqual) {
                if left == ValueType::Null || right == ValueType::Null {
                    return Err(static_type_mismatch(
                        ExecutionOperation::Compare,
                        left,
                        right,
                    ));
                }
                require_orderable_type(left, ExecutionOperation::Compare)?;
            }
        }
        Predicate::Presence { value, .. } => {
            validate_expression(value, scope)?;
        }
        Predicate::CoercionFailed { field } => {
            let specification = validate_field_ref(*field, scope)?;
            let Branch::Executable(behavior) = specification.behavior.select(scope.profile) else {
                return Err(StaticSpecError::InvalidCoercionFailedPredicate {
                    field_id: specification.field_id,
                    profile: scope.profile,
                }
                .into());
            };
            if !coercion_preserves_invalid_raw(behavior.coercion) {
                return Err(StaticSpecError::InvalidCoercionFailedPredicate {
                    field_id: specification.field_id,
                    profile: scope.profile,
                }
                .into());
            }
        }
        Predicate::JavaScriptParseFloat {
            operator,
            input,
            operand,
        } => {
            let input = validate_expression(input, scope)?;
            require_static_type(
                ValueType::String,
                input,
                ExecutionOperation::JavaScriptParseFloat,
                true,
            )?;
            let has_operand = operand.is_some();
            let operand_is_valid = match operator {
                JavaScriptParseFloatOperator::IsNaN => !has_operand,
                JavaScriptParseFloatOperator::StrictEqual
                | JavaScriptParseFloatOperator::GreaterThan => has_operand,
            };
            if !operand_is_valid {
                return Err(StaticSpecError::InvalidJavaScriptParseFloatPredicate {
                    operator: *operator,
                    has_operand,
                }
                .into());
            }
            if let Some(operand) = operand {
                ExactDecimal::try_from_parts(operand.coefficient, operand.scale).map_err(|_| {
                    InterpreterError::Overflow {
                        operation: ExecutionOperation::JavaScriptParseFloat,
                    }
                })?;
            }
        }
        Predicate::JavaScriptGlobalIsNaNLogicalOr { inputs } => {
            if inputs.is_empty() {
                return Err(StaticSpecError::EmptyRequiredList {
                    kind: SpecItemKind::Rule,
                    value: "javascript-global-is-nan-logical-or",
                }
                .into());
            }
            for input in *inputs {
                let input = validate_expression(input, scope)?;
                require_static_type(
                    ValueType::String,
                    input,
                    ExecutionOperation::JavaScriptGlobalIsNaNLogicalOr,
                    true,
                )?;
            }
        }
        Predicate::JavaScriptNumberCompare { input, operand, .. } => {
            let input = validate_expression(input, scope)?;
            require_static_type(
                ValueType::String,
                input,
                ExecutionOperation::JavaScriptNumberCompare,
                true,
            )?;
            let operand = validate_expression(operand, scope)?;
            require_numeric_type(operand, ExecutionOperation::JavaScriptNumberCompare)?;
        }
        Predicate::Checksum { input, .. } => {
            let input = validate_expression(input, scope)?;
            require_static_type(ValueType::String, input, ExecutionOperation::Checksum, true)?;
        }
        Predicate::Matches { value, pattern, .. } => {
            let value = validate_expression(value, scope)?;
            require_static_type(ValueType::String, value, ExecutionOperation::Matches, false)?;
            if pattern.source.is_empty() {
                return Err(StaticSpecError::EmptyPattern.into());
            }
        }
        Predicate::In { value, candidates } => {
            let mut expected = validate_expression(value, scope)?;
            if candidates.is_empty() {
                return Err(StaticSpecError::EmptyRequiredList {
                    kind: SpecItemKind::Rule,
                    value: "membership-predicate",
                }
                .into());
            }
            for candidate in *candidates {
                literal_value(*candidate)?;
                let candidate = typed_value_type(*candidate);
                require_compatible_types(expected, candidate, ExecutionOperation::Compare)?;
                if expected == ValueType::Null {
                    expected = candidate;
                }
            }
        }
        Predicate::GroupQuantifier {
            group_id,
            predicate,
            ..
        } => {
            parse_group_id(group_id)?;
            let Some(group) = find_group(scope.spec, group_id) else {
                return Err(invalid_static_reference(scope, group_id));
            };
            validate_predicate(predicate, scope.with_current_group(group.group_id))?;
        }
    }
    Ok(())
}

fn coercion_preserves_invalid_raw(coercion: Coercion) -> bool {
    match coercion {
        Coercion::Decimal { on_invalid, .. }
        | Coercion::Integer { on_invalid, .. }
        | Coercion::Boolean { on_invalid, .. }
        | Coercion::Date { on_invalid, .. } => on_invalid == InvalidValuePolicy::PreserveRaw,
        Coercion::String { .. } => false,
    }
}

fn validate_effect(
    effect: &Effect,
    scope: StaticValidationScope<'_>,
) -> Result<(), InterpreterError> {
    match effect {
        Effect::EmitIssue {
            message, fields, ..
        } => {
            if message.is_empty() {
                return Err(StaticSpecError::EmptyRequiredList {
                    kind: SpecItemKind::Rule,
                    value: "issue-message",
                }
                .into());
            }
            for field in *fields {
                validate_field_ref(*field, scope)?;
            }
            Ok(())
        }
        Effect::EmitNotification { .. } => unsupported_static_effect(scope, effect.kind()),
        Effect::SetRawFieldValue { field, .. } => {
            if !scope.phase.is_field_event() || scope.owner_kind != SpecItemKind::Rule {
                return Err(StaticSpecError::InvalidEventBinding {
                    kind: scope.owner_kind,
                    value: scope.owner_id,
                }
                .into());
            }
            validate_field_ref(*field, scope)?;
            Ok(())
        }
        Effect::SetDerived { output_id, value } => {
            parse_output_id(output_id)?;
            let target = resolve_unique_output_target(scope, output_id)?;
            let replacement = validate_expression(value, scope)?;
            require_static_type(target, replacement, ExecutionOperation::DerivedLookup, true)?;
            unsupported_static_effect(scope, effect.kind())
        }
        Effect::NormalizeField {
            field,
            normalization,
        } => {
            let field = validate_field_ref(*field, scope)?;
            require_static_type(
                ValueType::String,
                field.value_type,
                ExecutionOperation::NormalizeField,
                false,
            )?;
            validate_normalization(normalization)?;
            unsupported_static_effect(scope, effect.kind())
        }
        Effect::SetWorkflowState { state_id } => {
            validate_static_id(SpecItemKind::Rule, state_id)?;
            unsupported_static_effect(scope, effect.kind())
        }
    }
}

fn validate_field_ref<'spec>(
    field: FieldRef,
    scope: StaticValidationScope<'spec>,
) -> Result<&'spec FieldSpec, InterpreterError> {
    parse_field_id(field.field_id)?;
    if let FieldInstanceSelector::StableInstanceId(instance_id) = field.instance {
        parse_instance_id(instance_id)?;
    }
    let Some(specification) = find_field(scope.spec, field.field_id) else {
        return Err(invalid_static_reference(scope, field.field_id));
    };
    let valid_scope = match (specification.group_id, field.instance) {
        (None, FieldInstanceSelector::Singleton) => true,
        (Some(group_id), FieldInstanceSelector::CurrentGroupInstance) => {
            scope.current_group == Some(group_id)
        }
        (Some(_), FieldInstanceSelector::StableInstanceId(_)) => true,
        _ => false,
    };
    if !valid_scope {
        return Err(invalid_static_reference(scope, field.field_id));
    }
    Ok(specification)
}

fn validate_dependency_availability(
    spec: &StaticRuleSetSpec,
    calculation: &CalculationSpec,
    profile: BehaviorProfile,
    phase: ValidationPhase,
) -> Result<(), InterpreterError> {
    let scope = StaticValidationScope {
        spec,
        profile,
        phase,
        owner_kind: SpecItemKind::Calculation,
        owner_id: calculation.calculation_id,
        calculation: Some(calculation),
        current_group: evaluation_scope_group(calculation.scope),
        trigger_field_ids: calculation.trigger_field_ids,
    };
    for dependency_id in calculation.depends_on {
        let Some(dependency) = find_calculation(spec, dependency_id) else {
            return Err(invalid_static_reference(scope, dependency_id));
        };
        if !dependency.phases.contains(&phase)
            || !matches!(dependency.profiles.select(profile), Branch::Executable(_))
            || (phase.is_field_event()
                && calculation
                    .trigger_field_ids
                    .iter()
                    .any(|trigger| !dependency.trigger_field_ids.contains(trigger)))
        {
            return Err(invalid_static_reference(scope, dependency_id));
        }
    }
    Ok(())
}

fn resolve_derived_output(
    scope: StaticValidationScope<'_>,
    calculation_id: &'static str,
    output_id: &'static str,
    instance: DerivedInstanceSelector,
) -> Result<ValueType, InterpreterError> {
    parse_calculation_id(calculation_id)?;
    parse_output_id(output_id)?;
    let Some(calculation) = find_calculation(scope.spec, calculation_id) else {
        return Err(invalid_static_reference(scope, calculation_id));
    };
    if let DerivedInstanceSelector::StableInstanceId(instance_id) = instance {
        parse_instance_id(instance_id)?;
    }
    let valid_instance = match (calculation.scope, instance) {
        (EvaluationScope::Singleton, DerivedInstanceSelector::Singleton) => true,
        (EvaluationScope::EachGroup(group_id), DerivedInstanceSelector::CurrentGroupInstance) => {
            scope.current_group == Some(group_id)
        }
        (EvaluationScope::EachGroup(_), DerivedInstanceSelector::StableInstanceId(_)) => true,
        _ => false,
    };
    if !valid_instance {
        return Err(invalid_static_reference(scope, calculation_id));
    }
    if let Some(owner) = scope.calculation {
        let owner_position = calculation_position(scope.spec, owner.calculation_id)
            .expect("validated evaluation order contains the owner calculation");
        let dependency_position = calculation_position(scope.spec, calculation_id)
            .expect("validated evaluation order contains every calculation");
        if dependency_position >= owner_position {
            return Err(StaticSpecError::DependencyOutOfOrder {
                calculation_id: owner.calculation_id,
                dependency_id: calculation_id,
            }
            .into());
        }
        if !owner.depends_on.contains(&calculation_id) {
            return Err(invalid_static_reference(scope, calculation_id));
        }
    }
    if !calculation.phases.contains(&scope.phase) {
        return Err(invalid_static_reference(scope, calculation_id));
    }
    if scope.phase.is_field_event()
        && scope
            .trigger_field_ids
            .iter()
            .any(|trigger| !calculation.trigger_field_ids.contains(trigger))
    {
        return Err(invalid_static_reference(scope, calculation_id));
    }
    let Branch::Executable(branch) = calculation.profiles.select(scope.profile) else {
        return Err(invalid_static_reference(scope, calculation_id));
    };
    let Some(output) = branch
        .outputs
        .iter()
        .find(|candidate| candidate.output_id == output_id)
    else {
        return Err(invalid_static_reference(scope, output_id));
    };
    Ok(expression_result_type(output.value))
}

fn resolve_unique_output_target(
    scope: StaticValidationScope<'_>,
    output_id: &'static str,
) -> Result<ValueType, InterpreterError> {
    let mut result = None;
    for calculation in scope.spec.calculations {
        if !calculation.phases.contains(&scope.phase) {
            continue;
        }
        let Branch::Executable(branch) = calculation.profiles.select(scope.profile) else {
            continue;
        };
        for output in branch
            .outputs
            .iter()
            .filter(|candidate| candidate.output_id == output_id)
        {
            if result.is_some() {
                return Err(invalid_static_reference(scope, output_id));
            }
            result = Some(expression_result_type(output.value));
        }
    }
    result.ok_or_else(|| invalid_static_reference(scope, output_id))
}

fn unsupported_static_effect(
    scope: StaticValidationScope<'_>,
    kind: EffectKind,
) -> Result<(), InterpreterError> {
    Err(InterpreterError::UnsupportedEffect {
        rule_id: parse_rule_id(scope.owner_id)?,
        kind,
    })
}

fn invalid_static_reference(
    scope: StaticValidationScope<'_>,
    target: &'static str,
) -> InterpreterError {
    StaticSpecError::InvalidReference {
        kind: scope.owner_kind,
        value: scope.owner_id,
        target,
    }
    .into()
}

fn require_static_type(
    expected: ValueType,
    actual: ValueType,
    operation: ExecutionOperation,
    allow_null: bool,
) -> Result<(), InterpreterError> {
    if expected == actual || (allow_null && actual == ValueType::Null) {
        Ok(())
    } else {
        Err(static_type_mismatch(operation, expected, actual))
    }
}

fn require_compatible_types(
    expected: ValueType,
    actual: ValueType,
    operation: ExecutionOperation,
) -> Result<(), InterpreterError> {
    if expected == actual || expected == ValueType::Null || actual == ValueType::Null {
        Ok(())
    } else {
        Err(static_type_mismatch(operation, expected, actual))
    }
}

fn require_numeric_type(
    actual: ValueType,
    operation: ExecutionOperation,
) -> Result<(), InterpreterError> {
    if matches!(actual, ValueType::Integer | ValueType::Decimal) {
        Ok(())
    } else {
        Err(static_type_mismatch(operation, ValueType::Decimal, actual))
    }
}

fn require_orderable_type(
    actual: ValueType,
    operation: ExecutionOperation,
) -> Result<(), InterpreterError> {
    if matches!(
        actual,
        ValueType::String | ValueType::Integer | ValueType::Decimal | ValueType::Date
    ) {
        Ok(())
    } else {
        Err(static_type_mismatch(operation, ValueType::String, actual))
    }
}

fn static_type_mismatch(
    operation: ExecutionOperation,
    expected: ValueType,
    actual: ValueType,
) -> InterpreterError {
    StaticSpecError::TypeMismatch {
        operation,
        expected,
        actual,
    }
    .into()
}

const fn typed_value_type(value: TypedValue) -> ValueType {
    match value {
        TypedValue::Null => ValueType::Null,
        TypedValue::String(_) => ValueType::String,
        TypedValue::Boolean(_) => ValueType::Boolean,
        TypedValue::Integer(_) => ValueType::Integer,
        TypedValue::Decimal(_) => ValueType::Decimal,
        TypedValue::Date(_) => ValueType::Date,
    }
}

const fn expression_result_type(expression: &Expression) -> ValueType {
    match expression {
        Expression::Literal(value) => typed_value_type(*value),
        Expression::Field { result_type, .. }
        | Expression::Derived { result_type, .. }
        | Expression::Context { result_type, .. }
        | Expression::Unary { result_type, .. }
        | Expression::Binary { result_type, .. }
        | Expression::Nary { result_type, .. }
        | Expression::Conditional { result_type, .. }
        | Expression::Coerce { result_type, .. }
        | Expression::SplitComponent { result_type, .. }
        | Expression::JavaScriptParseIntRadix10 { result_type, .. }
        | Expression::JavaScriptDateLocalDay { result_type, .. }
        | Expression::CanonicalLocalDateDay { result_type, .. }
        | Expression::GroupAggregate { result_type, .. } => *result_type,
    }
}

const fn coercion_result_type(coercion: Coercion) -> ValueType {
    match coercion {
        Coercion::String { .. } => ValueType::String,
        Coercion::Decimal { .. } => ValueType::Decimal,
        Coercion::Integer { .. } => ValueType::Integer,
        Coercion::Boolean { .. } => ValueType::Boolean,
        Coercion::Date { .. } => ValueType::Date,
    }
}

const fn unary_operation(operator: UnaryOperator) -> ExecutionOperation {
    match operator {
        UnaryOperator::Negate => ExecutionOperation::UnaryNegate,
        UnaryOperator::Absolute => ExecutionOperation::UnaryAbsolute,
        UnaryOperator::Length => ExecutionOperation::UnaryLength,
    }
}

fn calculation_position(spec: &StaticRuleSetSpec, calculation_id: &str) -> Option<usize> {
    spec.evaluation_order
        .iter()
        .position(|candidate| *candidate == calculation_id)
}

fn find_field<'spec>(spec: &'spec StaticRuleSetSpec, field_id: &str) -> Option<&'spec FieldSpec> {
    spec.fields
        .iter()
        .find(|candidate| candidate.field_id == field_id)
}

fn find_group<'spec>(
    spec: &'spec StaticRuleSetSpec,
    group_id: &str,
) -> Option<&'spec FieldGroupSpec> {
    spec.field_groups
        .iter()
        .find(|candidate| candidate.group_id == group_id)
}

fn find_calculation<'spec>(
    spec: &'spec StaticRuleSetSpec,
    calculation_id: &str,
) -> Option<&'spec CalculationSpec> {
    spec.calculations
        .iter()
        .find(|candidate| candidate.calculation_id == calculation_id)
}

fn validate_static_id(kind: SpecItemKind, value: &'static str) -> Result<(), InterpreterError> {
    StableInstanceId::parse(value)
        .map(|_| ())
        .map_err(|_| StaticSpecError::InvalidIdentifier { kind, value }.into())
}

fn parse_context_value_id(value: &'static str) -> Result<ContextValueId, InterpreterError> {
    ContextValueId::parse(value).map_err(|_| {
        StaticSpecError::InvalidIdentifier {
            kind: SpecItemKind::ContextValue,
            value,
        }
        .into()
    })
}

fn parse_field_id(value: &'static str) -> Result<FieldId, InterpreterError> {
    FieldId::parse(value).map_err(|_| {
        StaticSpecError::InvalidIdentifier {
            kind: SpecItemKind::Field,
            value,
        }
        .into()
    })
}

fn parse_group_id(value: &'static str) -> Result<RepeatedGroupId, InterpreterError> {
    RepeatedGroupId::parse(value).map_err(|_| {
        StaticSpecError::InvalidIdentifier {
            kind: SpecItemKind::FieldGroup,
            value,
        }
        .into()
    })
}

fn parse_instance_id(value: &'static str) -> Result<StableInstanceId, InterpreterError> {
    StableInstanceId::parse(value).map_err(|_| {
        StaticSpecError::InvalidIdentifier {
            kind: SpecItemKind::StableInstance,
            value,
        }
        .into()
    })
}

fn parse_calculation_id(value: &'static str) -> Result<CalculationId, InterpreterError> {
    CalculationId::parse(value).map_err(|_| {
        StaticSpecError::InvalidIdentifier {
            kind: SpecItemKind::Calculation,
            value,
        }
        .into()
    })
}

fn parse_output_id(value: &'static str) -> Result<OutputId, InterpreterError> {
    OutputId::parse(value).map_err(|_| {
        StaticSpecError::InvalidIdentifier {
            kind: SpecItemKind::Output,
            value,
        }
        .into()
    })
}

fn parse_rule_id(value: &'static str) -> Result<RuleId, InterpreterError> {
    RuleId::parse(value).map_err(|_| {
        StaticSpecError::InvalidIdentifier {
            kind: SpecItemKind::Rule,
            value,
        }
        .into()
    })
}

fn parse_workflow_state_id(value: &'static str) -> Result<WorkflowStateId, InterpreterError> {
    WorkflowStateId::parse(value).map_err(|_| {
        StaticSpecError::InvalidIdentifier {
            kind: SpecItemKind::WorkflowState,
            value,
        }
        .into()
    })
}

fn parse_workflow_transition_id(
    value: &'static str,
) -> Result<WorkflowTransitionId, InterpreterError> {
    WorkflowTransitionId::parse(value).map_err(|_| {
        StaticSpecError::InvalidIdentifier {
            kind: SpecItemKind::WorkflowTransition,
            value,
        }
        .into()
    })
}

#[cfg(test)]
mod tests;
