//! Inert raw-value adapter foundation for exact form `2550Qv2024`.
//!
//! This module records the observed seven repeating groups and their 28 v1
//! member descriptors, but it does not construct an evaluation request,
//! select/register a rule package, or authorize serialization/submission.
//! Only raw buffers explicitly captured in [`Form2550QRawEditorState`] enter
//! the snapshot. Missing buffers remain missing and are reported as capture
//! gaps; typed draft values are never formatted back into substitute input.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[cfg(test)]
use bir_rules::{
    BehaviorProfile, ContextValue, EvaluationError, EvaluationRequest, EvaluationResult,
    InputRevision, ValidationContext, ValidationPhase, ValidationReport,
};
use bir_rules::{
    FieldId, FieldInstance, FieldValueSource, FormRevisionKey, InputSnapshotError, RawFieldValue,
    RawInputSnapshot, RawValue, RegistryError, RepeatedGroupId, RepeatedGroupInstance,
    RuleSetRegistry, StableInstanceId, StaticRuleSetRegistry,
};

#[cfg(test)]
use super::{
    FormRuleEvaluator, ShadowEvaluationOutcome, TrustedEvaluation, TrustedEvaluationError,
};
use crate::forms::form_2550q::{
    Form2550QDraft, Form2550QRawEditorState, Form2550QRawEditorStateError, Form2550QRowFamily,
    Form2550QRowIdentityError,
};

const FORM_2550Q_CODE: &str = "2550Q";
const FORM_2550Q_REVISION: &str = "2024-04-01";
const FORM_2550Q_OFFICIAL_PACKAGE_VERSION: &str = "7.9.6.0";

/// Review-controlled activation gate for the repository-default diagnostic.
///
/// Registering or promoting a reviewed provider must not activate 2550Q on its
/// own. A dedicated review must replace `None` with one complete
/// [`FormRevisionKey`], including its rule-set ID and source-set digest.
fn reviewed_repo_default_designation() -> Option<FormRevisionKey> {
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Form2550QMemberBinding {
    field_id: &'static str,
    reviewed_xml_prefix: Option<&'static str>,
}

impl Form2550QMemberBinding {
    pub const fn field_id(self) -> &'static str {
        self.field_id
    }

    pub const fn reviewed_xml_prefix(self) -> Option<&'static str> {
        self.reviewed_xml_prefix
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Form2550QGroupBinding {
    family: Form2550QRowFamily,
    group_id: &'static str,
    members: &'static [Form2550QMemberBinding],
    has_live_editor_controls: bool,
}

impl Form2550QGroupBinding {
    pub const fn family(self) -> Form2550QRowFamily {
        self.family
    }

    pub const fn group_id(self) -> &'static str {
        self.group_id
    }

    pub const fn members(self) -> &'static [Form2550QMemberBinding] {
        self.members
    }

    pub const fn has_live_editor_controls(self) -> bool {
        self.has_live_editor_controls
    }
}

const SCHEDULE_1_MEMBERS: [Form2550QMemberBinding; 9] = [
    member("txtDatePurchase1", Some("txtDatePurchase")),
    member("txtSourceCode1", Some("txtSourceCode")),
    member("txtDescription1", Some("txtDescription")),
    member("txtAmountPurchase1", Some("txtAmountPurchase")),
    member("txtInputTax1", Some("txtInputTax")),
    member("txtEstimatedLife1", Some("txtEstimatedLife")),
    member("txtRecognizedLife1", Some("txtRecognizedLife")),
    member("txtAllowedInputTax1", Some("txtAllowedInputTax")),
    member("txtBalanceInputTax1", Some("txtBalanceInputTax")),
];

const SCHEDULE_3_MEMBERS: [Form2550QMemberBinding; 5] = [
    member("txtDateCovered3", Some("txtDateCovered3")),
    member("txtDateCovered3To", Some("txtDateCovered3To")),
    member("txtNameWithHoldingAgent3", Some("txtNameWithHoldingAgent3")),
    member("txtIncomePayment3", Some("txtIncomePayment3")),
    member("txtTotalTaxWithHeld3", Some("txtTotalTaxWithHeld3")),
];

const SCHEDULE_4_MEMBERS: [Form2550QMemberBinding; 6] = [
    member("txtDate4", Some("txtDate4")),
    member("txtDate4To", Some("txtDate4To")),
    member("txtNameOfMiller4", Some("txtNameOfMiller4")),
    member("txtNameOfTaxpayer4", Some("txtNameOfTaxpayer4")),
    member(
        "txtOfficialReceiptNumber4",
        Some("txtOfficialReceiptNumber4"),
    ),
    member("txtAmountPaid4", Some("txtAmountPaid4")),
];

const ITEM_19_ADDITIONAL_MEMBERS: [Form2550QMemberBinding; 2] = [
    member("frm2550qv2024:totalTaxPayableNo19Description", None),
    member("frm2550qv2024:totalTaxPayableNo19Amount", None),
];

const ITEM_42_ADDITIONAL_MEMBERS: [Form2550QMemberBinding; 2] = [
    member("frm2550qv2024:totalTaxPayableNo42Description", None),
    member("frm2550qv2024:totalTaxPayableNo42Amount", None),
];

const ITEM_47_ADDITIONAL_MEMBERS: [Form2550QMemberBinding; 2] = [
    member("frm2550qv2024:totalTaxPayableNo47Description", None),
    member("frm2550qv2024:totalTaxPayableNo47Amount", None),
];

const ITEM_56_ADDITIONAL_MEMBERS: [Form2550QMemberBinding; 2] = [
    member("frm2550qv2024:totalTaxPayableNo56Description", None),
    member("frm2550qv2024:totalTaxPayableNo56Amount", None),
];

const GROUP_BINDINGS: [Form2550QGroupBinding; 7] = [
    group(
        Form2550QRowFamily::Schedule1,
        "schedule-1-capital-good-row",
        &SCHEDULE_1_MEMBERS,
        true,
    ),
    group(
        Form2550QRowFamily::Schedule3,
        "schedule-3-creditable-vat-row",
        &SCHEDULE_3_MEMBERS,
        true,
    ),
    group(
        Form2550QRowFamily::Schedule4,
        "schedule-4-advance-vat-row",
        &SCHEDULE_4_MEMBERS,
        true,
    ),
    group(
        Form2550QRowFamily::Item19Additional,
        "item-19-additional-row",
        &ITEM_19_ADDITIONAL_MEMBERS,
        true,
    ),
    group(
        Form2550QRowFamily::Item42Additional,
        "item-42-additional-row",
        &ITEM_42_ADDITIONAL_MEMBERS,
        true,
    ),
    group(
        Form2550QRowFamily::Item47Additional,
        "item-47-additional-row",
        &ITEM_47_ADDITIONAL_MEMBERS,
        true,
    ),
    group(
        Form2550QRowFamily::Item56Additional,
        "item-56-additional-row",
        &ITEM_56_ADDITIONAL_MEMBERS,
        true,
    ),
];

const SINGLETON_FIELD_IDS: [&str; 66] = [
    "frm2550qv2024:calendarNo1",
    "frm2550qv2024:fiscalNo1",
    "frm2550qv2024:OptQuarter1",
    "frm2550qv2024:OptQuarter2",
    "frm2550qv2024:OptQuarter3",
    "frm2550qv2024:OptQuarter4",
    "frm2550qv2024:amendedReturnYesNo5",
    "frm2550qv2024:amendedReturnNo5",
    "frm2550qv2024:OptShortPrd1",
    "frm2550qv2024:OptShortPrd2",
    "frm2550qv2024:selectedMonthNo2",
    "frm2550qv2024:txtYearNo2",
    "frm2550qv2024:RtnPeriodFromNo4",
    "frm2550qv2024:RtnPeriodToNo4",
    "frm2550qv2024:txtTIN1",
    "frm2550qv2024:txtTIN2",
    "frm2550qv2024:txtTIN3",
    "frm2550qv2024:branchCode",
    "frm2550qv2024:txtRDOCode",
    "frm2550qv2024:taxpayerName",
    "frm2550qv2024:taxpayerAddress",
    "frm2550qv2024:taxpayerZip",
    "frm2550qv2024:taxpayerContactNumber",
    "frm2550qv2024:taxpayerEmailAddress",
    "frm2550qv2024:taxPayerClassification1",
    "frm2550qv2024:taxPayerClassification2",
    "frm2550qv2024:taxPayerClassification3",
    "frm2550qv2024:taxPayerClassification4",
    "frm2550qv2024:internationalTreatyYn",
    "frm2550qv2024:specialRateYn",
    "frm2550qv2024:specifyInternationalTreaty",
    "frm2550qv2024:vatPaidReturn",
    "frm2550qv2024:addSpecifyNo19",
    "frm2550qv2024:otherCreditsNo19",
    "frm2550qv2024:surcharge",
    "frm2550qv2024:interest",
    "frm2550qv2024:compromise",
    "frm2550qv2024:vatableSales",
    "frm2550qv2024:zeroRatedSales",
    "frm2550qv2024:exemptSales",
    "frm2550qv2024:lessOutputVat",
    "frm2550qv2024:addOutputVat",
    "frm2550qv2024:inputTaxCarried",
    "frm2550qv2024:transitionalInputTax",
    "frm2550qv2024:presumptiveInputTax",
    "frm2550qv2024:addSpecifyNo42",
    "frm2550qv2024:otherSpecify42",
    "frm2550qv2024:domesticPurchase",
    "frm2550qv2024:domesticInputTax",
    "frm2550qv2024:servicesPurchase",
    "frm2550qv2024:serviceInputTax",
    "frm2550qv2024:importPurchase",
    "frm2550qv2024:importInputTax",
    "frm2550qv2024:addSpecifyNo47",
    "frm2550qv2024:otherSpecify47",
    "otherSpecify47B",
    "frm2550qv2024:domesticPurchaseNoTax",
    "frm2550qv2024:vatExemptImports",
    "frm2550qv2024:vatRefund",
    "frm2550qv2024:inputVatUnpaid",
    "frm2550qv2024:addSpecifyNo56",
    "frm2550qv2024:otherSpecify56",
    "frm2550qv2024:addInputVat",
    "frm2550qv2024:sched2InputTaxDirect",
    "frm2550qv2024:sched2VatExemptSale",
    "frm2550qv2024:sched2AmountInputTax",
];

/// Closed inventory of the 20 non-v1 local-print buffers in the current
/// editor. They are persisted losslessly in raw editor state but are
/// intentionally excluded from the rules-engine `RawInputSnapshot`.
const LOCAL_PRINT_RAW_FIELD_KEYS: [&str; 20] = [
    "signatory",
    "signatory_title",
    "non_individual_officer",
    "tax_agent_number",
    "tax_agent_issue",
    "tax_agent_expiry",
    "cash_amount",
    "check_bank",
    "check_number",
    "check_date",
    "check_amount",
    "tdm_number",
    "tdm_date",
    "tdm_amount",
    "other_payment_description",
    "other_payment_bank",
    "other_payment_number",
    "other_payment_date",
    "other_payment_amount",
    "machine_validation",
];

const fn member(
    field_id: &'static str,
    reviewed_xml_prefix: Option<&'static str>,
) -> Form2550QMemberBinding {
    Form2550QMemberBinding {
        field_id,
        reviewed_xml_prefix,
    }
}

const fn group(
    family: Form2550QRowFamily,
    group_id: &'static str,
    members: &'static [Form2550QMemberBinding],
    has_live_editor_controls: bool,
) -> Form2550QGroupBinding {
    Form2550QGroupBinding {
        family,
        group_id,
        members,
        has_live_editor_controls,
    }
}

pub const fn form_2550q_group_bindings() -> &'static [Form2550QGroupBinding] {
    &GROUP_BINDINGS
}

pub const fn form_2550q_singleton_field_ids() -> &'static [&'static str] {
    &SINGLETON_FIELD_IDS
}

pub const fn form_2550q_local_print_raw_field_keys() -> &'static [&'static str] {
    &LOCAL_PRINT_RAW_FIELD_KEYS
}

/// Canonical key for one raw 2550Q editor control.
///
/// Fields are private so callers cannot invent malformed-field path strings.
/// Repeated keys bind the reviewed family, a fixed-width stable row identity,
/// and one declared member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form2550QRawFieldKey(Form2550QRawFieldKeyKind);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Form2550QRawFieldKeyKind {
    Singleton {
        field_key: &'static str,
    },
    Repeated {
        family: Form2550QRowFamily,
        instance_id: StableInstanceId,
        member_key: &'static str,
    },
}

impl Form2550QRawFieldKey {
    pub fn try_singleton(field_key: &str) -> Result<Self, Form2550QRawFieldKeyError> {
        let field_key = SINGLETON_FIELD_IDS
            .iter()
            .chain(LOCAL_PRINT_RAW_FIELD_KEYS.iter())
            .copied()
            .find(|candidate| *candidate == field_key)
            .ok_or_else(|| Form2550QRawFieldKeyError::UnknownSingleton {
                field_key: field_key.to_owned(),
            })?;
        Ok(Self(Form2550QRawFieldKeyKind::Singleton { field_key }))
    }

    pub fn try_repeated(
        family: Form2550QRowFamily,
        instance_id: StableInstanceId,
        member_key: &str,
    ) -> Result<Self, Form2550QRawFieldKeyError> {
        if !is_fixed_width_row_identity(instance_id.as_str()) {
            return Err(Form2550QRawFieldKeyError::InvalidRowIdentity { instance_id });
        }
        let member_key = binding_for_family(family)
            .members
            .iter()
            .map(|member| member.field_id)
            .find(|candidate| *candidate == member_key)
            .ok_or_else(|| Form2550QRawFieldKeyError::UnknownRepeatedMember {
                family,
                member_key: member_key.to_owned(),
            })?;
        Ok(Self(Form2550QRawFieldKeyKind::Repeated {
            family,
            instance_id,
            member_key,
        }))
    }

    pub fn stable_path(&self) -> String {
        match &self.0 {
            Form2550QRawFieldKeyKind::Singleton { field_key } => (*field_key).to_owned(),
            Form2550QRawFieldKeyKind::Repeated {
                family,
                instance_id,
                member_key,
            } => format!(
                "{}/{instance_id}/{member_key}",
                canonical_path_for_row_family(*family)
            ),
        }
    }

    /// Return the exact rules-engine identity for this editor control.
    ///
    /// Local-print-only buffers are deliberately outside the reviewed rules
    /// snapshot and therefore return `None`. Repeated controls retain their
    /// persisted row identity; no vector index or display position enters the
    /// semantic path.
    pub fn semantic_field_instance(&self) -> Option<FieldInstance> {
        match &self.0 {
            Form2550QRawFieldKeyKind::Singleton { field_key }
                if LOCAL_PRINT_RAW_FIELD_KEYS.contains(field_key) =>
            {
                None
            }
            Form2550QRawFieldKeyKind::Singleton { field_key } => {
                Some(FieldInstance::singleton(parse_field_id(field_key)))
            }
            Form2550QRawFieldKeyKind::Repeated {
                family,
                instance_id,
                member_key,
            } => {
                let group_id = binding_for_family(*family).group_id;
                Some(
                    FieldInstance::try_new(
                        parse_field_id(member_key),
                        vec![RepeatedGroupInstance::new(
                            parse_group_id(group_id),
                            instance_id.clone(),
                        )],
                    )
                    .expect("one reviewed repeated group cannot duplicate its own semantic path"),
                )
            }
        }
    }

    pub fn mark_malformed(
        &self,
        raw_state: &mut Form2550QRawEditorState,
        message: impl Into<String>,
    ) {
        raw_state.mark_malformed(self.stable_path(), message);
    }

    pub fn clear_malformed(&self, raw_state: &mut Form2550QRawEditorState) {
        raw_state.clear_malformed(&self.stable_path());
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form2550QRawFieldKeyError {
    UnknownSingleton {
        field_key: String,
    },
    UnknownRepeatedMember {
        family: Form2550QRowFamily,
        member_key: String,
    },
    InvalidRowIdentity {
        instance_id: StableInstanceId,
    },
}

impl fmt::Display for Form2550QRawFieldKeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownSingleton { field_key } => {
                write!(formatter, "unknown 2550Q singleton raw field {field_key}")
            }
            Self::UnknownRepeatedMember { family, member_key } => write!(
                formatter,
                "unknown 2550Q {} raw member {member_key}",
                family.label()
            ),
            Self::InvalidRowIdentity { instance_id } => write!(
                formatter,
                "2550Q malformed-field key requires a fixed-width row identity, got {instance_id}"
            ),
        }
    }
}

impl std::error::Error for Form2550QRawFieldKeyError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form2550QCaptureGap {
    MissingRawBuffer { field: FieldInstance },
    NoLiveEditorControls { family: Form2550QRowFamily },
    ExcludedLocalPrintRawBuffer { field_key: &'static str },
}

impl Form2550QCaptureGap {
    /// Whether this gap makes the captured snapshot unfit to evaluate.
    ///
    /// Not every gap is an obstruction. Two of the three describe deliberate,
    /// correct states of a complete capture:
    ///
    /// * `NoLiveEditorControls` — a repeated family has no editor UI. Every
    ///   family in the reviewed rule set is `min_occurs: 0` and no rule is
    ///   group-scoped, so contributing zero rows is a valid and complete
    ///   capture, not a missing one.
    /// * `ExcludedLocalPrintRawBuffer` — a local-print buffer holds a value.
    ///   These are deliberately outside the rules snapshot; their presence is
    ///   expected and says nothing about the fields that are in it.
    ///
    /// Only `MissingRawBuffer` is obstructive: it means a field the rules do
    /// evaluate has no captured input, so evaluating would judge data we do not
    /// have.
    ///
    /// Treating all three as blocking made `request_from_capture` unreachable —
    /// the four controlless families emit their gaps unconditionally, so no
    /// draft could ever be evaluated. `bir-rules-codegen`'s audit asserts the
    /// rule-set properties this distinction rests on, so a future snapshot that
    /// scopes a rule over a group forces this decision to be revisited.
    pub const fn is_blocking(&self) -> bool {
        matches!(self, Self::MissingRawBuffer { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form2550QCaptureError {
    RowIdentity(Form2550QRowIdentityError),
    RawEditorState(Form2550QRawEditorStateError),
    AmbiguousLegacyRepeatedBinding {
        family: Form2550QRowFamily,
    },
    UnexpectedSingletonField {
        field_key: String,
    },
    UnexpectedRepeatedField {
        family: Form2550QRowFamily,
        instance_id: StableInstanceId,
        field_key: String,
    },
    OrphanRepeatedInstance {
        family: Form2550QRowFamily,
        instance_id: StableInstanceId,
    },
    InvalidMalformedPath {
        path: String,
    },
    UnknownMalformedField {
        path: String,
    },
    OrphanMalformedRepeatedInstance {
        path: String,
        family: Form2550QRowFamily,
        instance_id: StableInstanceId,
    },
    MalformedPathWithoutRawValue {
        path: String,
    },
    Snapshot(InputSnapshotError),
}

impl fmt::Display for Form2550QCaptureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RowIdentity(error) => write!(formatter, "2550Q row identity is unsafe: {error}"),
            Self::RawEditorState(error) => {
                write!(formatter, "2550Q raw editor state is unsafe: {error}")
            }
            Self::AmbiguousLegacyRepeatedBinding { family } => write!(
                formatter,
                "2550Q {} has persisted raw repeated values while at least one row lacks stable identity",
                family.label()
            ),
            Self::UnexpectedSingletonField { field_key } => write!(
                formatter,
                "2550Q raw editor state contains unbound singleton field {field_key}"
            ),
            Self::UnexpectedRepeatedField {
                family,
                instance_id,
                field_key,
            } => write!(
                formatter,
                "2550Q raw editor state contains unbound {} field {field_key} for row {instance_id}",
                family.label()
            ),
            Self::OrphanRepeatedInstance {
                family,
                instance_id,
            } => write!(
                formatter,
                "2550Q raw editor state contains orphan {} row {instance_id}",
                family.label()
            ),
            Self::InvalidMalformedPath { path } => write!(
                formatter,
                "2550Q raw editor state contains noncanonical malformed-field path {path}"
            ),
            Self::UnknownMalformedField { path } => write!(
                formatter,
                "2550Q raw editor state contains malformed-field path for unknown field {path}"
            ),
            Self::OrphanMalformedRepeatedInstance {
                path,
                family,
                instance_id,
            } => write!(
                formatter,
                "2550Q malformed-field path {path} refers to orphan {} row {instance_id}",
                family.label()
            ),
            Self::MalformedPathWithoutRawValue { path } => write!(
                formatter,
                "2550Q malformed-field path {path} has no corresponding captured raw value"
            ),
            Self::Snapshot(error) => write!(formatter, "2550Q raw snapshot is invalid: {error}"),
        }
    }
}

impl std::error::Error for Form2550QCaptureError {}

impl From<Form2550QRowIdentityError> for Form2550QCaptureError {
    fn from(error: Form2550QRowIdentityError) -> Self {
        Self::RowIdentity(error)
    }
}

impl From<Form2550QRawEditorStateError> for Form2550QCaptureError {
    fn from(error: Form2550QRawEditorStateError) -> Self {
        Self::RawEditorState(error)
    }
}

impl From<InputSnapshotError> for Form2550QCaptureError {
    fn from(error: InputSnapshotError) -> Self {
        Self::Snapshot(error)
    }
}

/// Deterministic raw snapshot plus an explicit account of uncaptured surface.
///
/// This source is intentionally inert. Its existence does not imply a complete
/// 2550Q adapter or authorize evaluation against any package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Form2550QFieldValueSource {
    snapshot: RawInputSnapshot,
    gaps: Vec<Form2550QCaptureGap>,
    excluded_local_print_buffer_keys: Vec<&'static str>,
}

impl Form2550QFieldValueSource {
    pub(crate) fn try_capture(draft: &mut Form2550QDraft) -> Result<Self, Form2550QCaptureError> {
        prepare_form_2550q_live_row_identities(draft)?;

        let materialized = materialized_instances(draft);
        validate_raw_state_bindings_against(draft, &materialized)?;
        validate_malformed_bindings_against(draft, &materialized)?;
        let excluded_local_print_buffer_keys = LOCAL_PRINT_RAW_FIELD_KEYS
            .iter()
            .copied()
            .filter(|field_key| draft.raw_editor_state.singleton_value(field_key).is_some())
            .collect::<Vec<_>>();

        let mut repeated_group_instances = Vec::new();
        let mut fields = Vec::new();
        let mut gaps = Vec::new();
        gaps.extend(
            excluded_local_print_buffer_keys
                .iter()
                .copied()
                .map(|field_key| Form2550QCaptureGap::ExcludedLocalPrintRawBuffer { field_key }),
        );

        for field_key in SINGLETON_FIELD_IDS {
            let field = FieldInstance::singleton(parse_field_id(field_key));
            match draft.raw_editor_state.singleton_value(field_key) {
                Some(value) => fields.push(RawFieldValue::new(field, value.clone())),
                None => gaps.push(Form2550QCaptureGap::MissingRawBuffer { field }),
            }
        }

        for binding in GROUP_BINDINGS {
            if !binding.has_live_editor_controls {
                gaps.push(Form2550QCaptureGap::NoLiveEditorControls {
                    family: binding.family,
                });
            }
            let group_id = parse_group_id(binding.group_id);
            for instance_id in materialized.get(&binding.family).into_iter().flatten() {
                let group_instance =
                    RepeatedGroupInstance::new(group_id.clone(), instance_id.clone());
                repeated_group_instances.push(group_instance.clone());
                for member in binding.members {
                    let field = FieldInstance::try_new(
                        parse_field_id(member.field_id),
                        vec![group_instance.clone()],
                    )?;
                    match draft.raw_editor_state.repeated_value(
                        binding.family,
                        instance_id,
                        member.field_id,
                    ) {
                        Some(value) => fields.push(RawFieldValue::new(field, value.clone())),
                        None => gaps.push(Form2550QCaptureGap::MissingRawBuffer { field }),
                    }
                }
            }
        }

        let snapshot = RawInputSnapshot::try_new(repeated_group_instances, fields)?;
        Ok(Self {
            snapshot,
            gaps,
            excluded_local_print_buffer_keys,
        })
    }

    pub(crate) fn snapshot(&self) -> &RawInputSnapshot {
        &self.snapshot
    }

    pub(crate) fn gaps(&self) -> &[Form2550QCaptureGap] {
        &self.gaps
    }

    pub(crate) fn excluded_local_print_buffer_count(&self) -> usize {
        self.excluded_local_print_buffer_keys.len()
    }

    pub(crate) fn excluded_local_print_buffer_keys(&self) -> &[&'static str] {
        &self.excluded_local_print_buffer_keys
    }
}

/// Validate the raw-state version and legacy row/raw relationship before
/// allocating any missing stable IDs, then validate the complete identity set.
///
/// If persisted repeated raw values exist for a family containing an
/// unidentified row, this fails without mutating the draft because positional
/// rebinding would be ambiguous.
pub fn prepare_form_2550q_live_row_identities(
    draft: &mut Form2550QDraft,
) -> Result<bool, Form2550QCaptureError> {
    draft.raw_editor_state.validate_version()?;
    reject_ambiguous_legacy_repeated_bindings(draft)?;
    let changed = draft.ensure_repeating_row_ids()?;
    draft.validate_repeating_row_ids()?;
    Ok(changed)
}

impl FieldValueSource for Form2550QFieldValueSource {
    fn repeated_group_instances(&self) -> &[RepeatedGroupInstance] {
        self.snapshot.repeated_group_instances()
    }

    fn raw_fields(&self) -> &[RawFieldValue] {
        self.snapshot.fields()
    }
}

fn reject_ambiguous_legacy_repeated_bindings(
    draft: &Form2550QDraft,
) -> Result<(), Form2550QCaptureError> {
    for family in Form2550QRowFamily::ALL {
        let has_persisted_raw_rows = draft
            .raw_editor_state
            .repeated_fields()
            .get(&family)
            .is_some_and(|instances| !instances.is_empty());
        if has_persisted_raw_rows && family_has_missing_row_identity(draft, family) {
            return Err(Form2550QCaptureError::AmbiguousLegacyRepeatedBinding { family });
        }
    }
    Ok(())
}

fn family_has_missing_row_identity(draft: &Form2550QDraft, family: Form2550QRowFamily) -> bool {
    match family {
        Form2550QRowFamily::Schedule1 => {
            draft.schedule_1.iter().any(|row| row.instance_id.is_none())
        }
        Form2550QRowFamily::Schedule3 => {
            draft.schedule_3.iter().any(|row| row.instance_id.is_none())
        }
        Form2550QRowFamily::Schedule4 => {
            draft.schedule_4.iter().any(|row| row.instance_id.is_none())
        }
        Form2550QRowFamily::Item19Additional => draft
            .item_19_additional_rows
            .iter()
            .any(|row| row.instance_id.is_none()),
        Form2550QRowFamily::Item42Additional => draft
            .item_42_additional_rows
            .iter()
            .any(|row| row.instance_id.is_none()),
        Form2550QRowFamily::Item47Additional => draft
            .item_47_additional_rows
            .iter()
            .any(|row| row.instance_id.is_none()),
        Form2550QRowFamily::Item56Additional => draft
            .item_56_additional_rows
            .iter()
            .any(|row| row.instance_id.is_none()),
    }
}

/// Exact, complete rule-set identity accepted by the 2550Q live boundary.
///
/// This wrapper cannot represent "current 2550Q". Its inner
/// [`FormRevisionKey`] still contains the rule-set ID, form code, form
/// revision, official package version, and source-set digest used by exact
/// registry lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Form2550QRuleSetPin(FormRevisionKey);

impl Form2550QRuleSetPin {
    fn try_new(rule_set: FormRevisionKey) -> Result<Self, Form2550QRuleSetPinError> {
        if rule_set.form_code().as_str() != FORM_2550Q_CODE {
            return Err(Form2550QRuleSetPinError::WrongFormCode {
                actual: rule_set.form_code().as_str().to_owned(),
            });
        }
        if rule_set.form_revision().as_str() != FORM_2550Q_REVISION {
            return Err(Form2550QRuleSetPinError::WrongFormRevision {
                actual: rule_set.form_revision().as_str().to_owned(),
            });
        }
        if rule_set.official_package_version().as_str() != FORM_2550Q_OFFICIAL_PACKAGE_VERSION {
            return Err(Form2550QRuleSetPinError::WrongOfficialPackageVersion {
                actual: rule_set.official_package_version().as_str().to_owned(),
            });
        }
        Ok(Self(rule_set))
    }

    fn rule_set(&self) -> &FormRevisionKey {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Form2550QRuleSetPinError {
    WrongFormCode { actual: String },
    WrongFormRevision { actual: String },
    WrongOfficialPackageVersion { actual: String },
}

impl fmt::Display for Form2550QRuleSetPinError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongFormCode { actual } => write!(
                formatter,
                "2550Q live validation requires form code {FORM_2550Q_CODE}, got {actual}"
            ),
            Self::WrongFormRevision { actual } => write!(
                formatter,
                "2550Q live validation requires exact form revision {FORM_2550Q_REVISION}, got {actual}"
            ),
            Self::WrongOfficialPackageVersion { actual } => write!(
                formatter,
                "2550Q live validation requires exact official package {FORM_2550Q_OFFICIAL_PACKAGE_VERSION}, got {actual}"
            ),
        }
    }
}

impl std::error::Error for Form2550QRuleSetPinError {}

/// Why the 2550Q facade could not finish an available exact evaluation.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum Form2550QLiveValidationFailure {
    Capture(Form2550QCaptureError),
    Request(EvaluationError),
    Evaluation(EvaluationError),
    Trusted(TrustedEvaluationError),
}

#[cfg(test)]
impl fmt::Display for Form2550QLiveValidationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Capture(error) => write!(formatter, "2550Q capture failed: {error}"),
            Self::Request(error) => write!(formatter, "2550Q request assembly failed: {error}"),
            Self::Evaluation(error) => write!(formatter, "2550Q evaluation failed: {error}"),
            Self::Trusted(error) => write!(formatter, "2550Q trusted evaluation failed: {error}"),
        }
    }
}

#[cfg(test)]
impl std::error::Error for Form2550QLiveValidationFailure {}

/// A completed live evaluation with its authorization boundary encoded in the
/// variant. Official compatibility can only inhabit the diagnostic variant.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum Form2550QLiveEvaluation {
    OfficialDiagnostic(EvaluationResult),
    FilingSafeTrusted(TrustedEvaluation),
}

#[cfg(test)]
impl Form2550QLiveEvaluation {
    fn report(&self) -> &ValidationReport {
        match self {
            Self::OfficialDiagnostic(result) => result.report(),
            Self::FilingSafeTrusted(trusted) => trusted.result().report(),
        }
    }

    const fn trusted_evaluation(&self) -> Option<&TrustedEvaluation> {
        match self {
            Self::OfficialDiagnostic(_) => None,
            Self::FilingSafeTrusted(trusted) => Some(trusted),
        }
    }
}

/// Fail-closed state returned to live 2550Q callers.
///
/// Only `Evaluated(FilingSafeTrusted(_))` contains an opaque trusted proof.
/// Unavailable, incomplete, and failed states contain neither a report nor a
/// serialization artifact.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum Form2550QLiveValidationState {
    Unavailable {
        rule_set: FormRevisionKey,
        error: RegistryError,
    },
    IncompleteCapture {
        rule_set: FormRevisionKey,
        gaps: Vec<Form2550QCaptureGap>,
    },
    EvaluationFailure {
        rule_set: FormRevisionKey,
        error: Form2550QLiveValidationFailure,
    },
    Evaluated {
        rule_set: FormRevisionKey,
        evaluation: Form2550QLiveEvaluation,
    },
}

#[cfg(test)]
impl Form2550QLiveValidationState {
    const fn rule_set(&self) -> &FormRevisionKey {
        match self {
            Self::Unavailable { rule_set, .. }
            | Self::IncompleteCapture { rule_set, .. }
            | Self::EvaluationFailure { rule_set, .. }
            | Self::Evaluated { rule_set, .. } => rule_set,
        }
    }

    fn report(&self) -> Option<&ValidationReport> {
        match self {
            Self::Evaluated { evaluation, .. } => Some(evaluation.report()),
            _ => None,
        }
    }

    const fn trusted_evaluation(&self) -> Option<&TrustedEvaluation> {
        match self {
            Self::Evaluated { evaluation, .. } => evaluation.trusted_evaluation(),
            _ => None,
        }
    }
}

/// Live-validation boundary for exact 2550Q rule-set identities.
///
/// Desktop callers provide raw draft state and UI-safe capture metadata; this
/// facade alone constructs `EvaluationRequest`. It has no form-code lookup,
/// current-version selector, handwritten fallback, or profile fallback.
#[cfg(test)]
struct Form2550QLiveValidationEvaluator<'registry> {
    registry: &'registry dyn RuleSetRegistry,
    pin: Form2550QRuleSetPin,
}

#[cfg(test)]
impl<'registry> Form2550QLiveValidationEvaluator<'registry> {
    const fn new(registry: &'registry dyn RuleSetRegistry, pin: Form2550QRuleSetPin) -> Self {
        Self { registry, pin }
    }

    /// Run the exact official branch for observation only.
    fn evaluate_official_diagnostic(
        &self,
        draft: &mut Form2550QDraft,
        phase: ValidationPhase,
        input_revision: InputRevision,
        context_values: Vec<ContextValue>,
    ) -> Form2550QLiveValidationState {
        let request = match self.prepare_request(
            draft,
            ValidationContext::new(phase, BehaviorProfile::OfficialCompatibility),
            input_revision,
            context_values,
        ) {
            Ok(request) => request,
            Err(state) => return state,
        };
        match FormRuleEvaluator::new(self.registry).evaluate_shadow(&request) {
            ShadowEvaluationOutcome::Evaluated { result, .. } => {
                Form2550QLiveValidationState::Evaluated {
                    rule_set: self.pin.0.clone(),
                    evaluation: Form2550QLiveEvaluation::OfficialDiagnostic(result),
                }
            }
            ShadowEvaluationOutcome::RegistryFailure { error, .. } => {
                Form2550QLiveValidationState::Unavailable {
                    rule_set: self.pin.0.clone(),
                    error,
                }
            }
            ShadowEvaluationOutcome::EvaluationFailure { error, .. } => {
                Form2550QLiveValidationState::EvaluationFailure {
                    rule_set: self.pin.0.clone(),
                    error: Form2550QLiveValidationFailure::Evaluation(error),
                }
            }
        }
    }

    /// Run the filing-safe branch through the opaque trusted boundary.
    ///
    /// There is deliberately no profile parameter, so OfficialCompatibility
    /// cannot be selected for trusted behavior.
    fn evaluate_filing_safe_trusted(
        &self,
        draft: &mut Form2550QDraft,
        phase: ValidationPhase,
        input_revision: InputRevision,
        context_values: Vec<ContextValue>,
    ) -> Form2550QLiveValidationState {
        let request = match self.prepare_request(
            draft,
            ValidationContext::new(phase, BehaviorProfile::FilingSafe),
            input_revision,
            context_values,
        ) {
            Ok(request) => request,
            Err(state) => return state,
        };
        match FormRuleEvaluator::new(self.registry).evaluate_trusted(&request) {
            Ok(trusted) => Form2550QLiveValidationState::Evaluated {
                rule_set: self.pin.0.clone(),
                evaluation: Form2550QLiveEvaluation::FilingSafeTrusted(trusted),
            },
            Err(TrustedEvaluationError::Registry(error)) => {
                Form2550QLiveValidationState::Unavailable {
                    rule_set: self.pin.0.clone(),
                    error,
                }
            }
            Err(error) => Form2550QLiveValidationState::EvaluationFailure {
                rule_set: self.pin.0.clone(),
                error: Form2550QLiveValidationFailure::Trusted(error),
            },
        }
    }

    fn prepare_request(
        &self,
        draft: &mut Form2550QDraft,
        context: ValidationContext,
        input_revision: InputRevision,
        context_values: Vec<ContextValue>,
    ) -> Result<EvaluationRequest, Form2550QLiveValidationState> {
        if let Err(error) = self.registry.resolve(self.pin.rule_set()) {
            return Err(Form2550QLiveValidationState::Unavailable {
                rule_set: self.pin.0.clone(),
                error,
            });
        }
        let source = Form2550QFieldValueSource::try_capture(draft).map_err(|error| {
            Form2550QLiveValidationState::EvaluationFailure {
                rule_set: self.pin.0.clone(),
                error: Form2550QLiveValidationFailure::Capture(error),
            }
        })?;
        self.request_from_capture(source, context, input_revision, context_values)
    }

    fn request_from_capture(
        &self,
        source: Form2550QFieldValueSource,
        context: ValidationContext,
        input_revision: InputRevision,
        context_values: Vec<ContextValue>,
    ) -> Result<EvaluationRequest, Form2550QLiveValidationState> {
        let blocking: Vec<Form2550QCaptureGap> = source
            .gaps()
            .iter()
            .filter(|gap| gap.is_blocking())
            .cloned()
            .collect();
        if !blocking.is_empty() {
            return Err(Form2550QLiveValidationState::IncompleteCapture {
                rule_set: self.pin.0.clone(),
                gaps: blocking,
            });
        }
        EvaluationRequest::capture(
            self.pin.0.clone(),
            context,
            input_revision,
            context_values,
            &source,
        )
        .map_err(|error| Form2550QLiveValidationState::EvaluationFailure {
            rule_set: self.pin.0.clone(),
            error: Form2550QLiveValidationFailure::Request(error),
        })
    }
}

/// Use only reviewed generated entries. This repository-default surface
/// deliberately has no evaluated or trusted variant while the adapter has
/// known uncaptured families.
fn evaluate_repo_default_impl(
    pin: Form2550QRuleSetPin,
    draft: &mut Form2550QDraft,
) -> Form2550QRepoLiveValidationState {
    let registry =
        match StaticRuleSetRegistry::try_new(bir_rules::generated::reviewed_rule_set_entries()) {
            Ok(registry) => registry,
            Err(error) => {
                return Form2550QRepoLiveValidationState::Unavailable {
                    rule_set: pin.0,
                    reason: repo_unavailable_from_registry_error(error),
                };
            }
        };
    if let Err(error) = registry.resolve(pin.rule_set()) {
        return Form2550QRepoLiveValidationState::Unavailable {
            rule_set: pin.0,
            reason: repo_unavailable_from_registry_error(error),
        };
    }
    let source = match Form2550QFieldValueSource::try_capture(draft) {
        Ok(source) => source,
        Err(error) => {
            return Form2550QRepoLiveValidationState::Unavailable {
                rule_set: pin.0,
                reason: Form2550QRepoLiveValidationUnavailableReason::Capture(error),
            };
        }
    };
    let blocking: Vec<Form2550QCaptureGap> = source
        .gaps()
        .iter()
        .filter(|gap| gap.is_blocking())
        .cloned()
        .collect();
    if !blocking.is_empty() {
        return Form2550QRepoLiveValidationState::IncompleteCapture {
            rule_set: pin.0,
            gaps: blocking,
        };
    }
    Form2550QRepoLiveValidationState::Unavailable {
        rule_set: pin.0,
        reason: Form2550QRepoLiveValidationUnavailableReason::AdapterNotReviewedComplete,
    }
}

/// Repository-default 2550Q live-validation entry point.
///
/// This public facade cannot accept a registry, select a latest/current
/// package, expose a behavior profile, or return trusted/evaluated authority.
#[derive(Debug, Clone, Copy, Default)]
pub struct Form2550QLiveValidationFacade;

impl Form2550QLiveValidationFacade {
    /// Resolve only the explicitly designated complete identity in the
    /// generated reviewed registry.
    ///
    /// The repository currently has no review-controlled designation, so
    /// production callers receive `Unavailable(ExactRuleSetNotDesignated)`.
    /// Adding a reviewed or candidate provider cannot activate this setup.
    pub fn setup_repo_default_diagnostic() -> Form2550QRepoDiagnosticSetupOutcome {
        let Some(designated_rule_set) = reviewed_repo_default_designation() else {
            return Form2550QRepoDiagnosticSetupOutcome::Unavailable {
                reason: Form2550QRepoLiveValidationUnavailableReason::ExactRuleSetNotDesignated,
            };
        };
        let pin = match Form2550QRuleSetPin::try_new(designated_rule_set) {
            Ok(pin) => pin,
            Err(error) => {
                return Form2550QRepoDiagnosticSetupOutcome::Unavailable {
                    reason:
                        Form2550QRepoLiveValidationUnavailableReason::DesignatedRuleSetInvalid {
                            message: error.to_string(),
                        },
                };
            }
        };
        let registry =
            match StaticRuleSetRegistry::try_new(bir_rules::generated::reviewed_rule_set_entries())
            {
                Ok(registry) => registry,
                Err(error) => {
                    return Form2550QRepoDiagnosticSetupOutcome::Unavailable {
                        reason: repo_unavailable_from_registry_error(error),
                    };
                }
            };
        if let Err(error) = registry.resolve(pin.rule_set()) {
            return Form2550QRepoDiagnosticSetupOutcome::Unavailable {
                reason: repo_unavailable_from_registry_error(error),
            };
        }
        Form2550QRepoDiagnosticSetupOutcome::ExactRegistrationAvailable(Form2550QRepoDiagnostic {
            pin,
        })
    }
}

/// Opaque repository-default diagnostic prepared from the reviewed registry.
///
/// It exposes neither a behavior profile nor a trusted evaluation. Its only
/// output remains the unavailable/incomplete diagnostic state space.
#[derive(Debug, Clone)]
pub struct Form2550QRepoDiagnostic {
    pin: Form2550QRuleSetPin,
}

impl Form2550QRepoDiagnostic {
    pub fn evaluate(&self, draft: &mut Form2550QDraft) -> Form2550QRepoLiveValidationState {
        evaluate_repo_default_impl(self.pin.clone(), draft)
    }
}

/// Result of attempting to prepare the repository-default 2550Q diagnostic.
///
/// `ExactRegistrationAvailable` means only that the explicitly designated
/// complete identity resolved in the reviewed registry. It is not a validity,
/// readiness, serialization, or filing authorization.
#[derive(Debug, Clone)]
pub enum Form2550QRepoDiagnosticSetupOutcome {
    ExactRegistrationAvailable(Form2550QRepoDiagnostic),
    Unavailable {
        reason: Form2550QRepoLiveValidationUnavailableReason,
    },
}

/// Why the repository-default facade cannot evaluate. This is diagnostic
/// state only and cannot be converted into a report or artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form2550QRepoLiveValidationUnavailableReason {
    ExactRuleSetNotDesignated,
    ExactRuleSetNotRegistered,
    DesignatedRuleSetInvalid { message: String },
    ReviewedRegistryInvalid { message: String },
    Capture(Form2550QCaptureError),
    AdapterNotReviewedComplete,
}

impl fmt::Display for Form2550QRepoLiveValidationUnavailableReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExactRuleSetNotDesignated => formatter.write_str(
                "no review-controlled exact 2550Q rule-set identity is designated; reviewed registry entries alone do not authorize activation",
            ),
            Self::ExactRuleSetNotRegistered => write!(
                formatter,
                "no reviewed exact 2550Q April 2024 package {FORM_2550Q_OFFICIAL_PACKAGE_VERSION} validator is registered in this repository"
            ),
            Self::DesignatedRuleSetInvalid { message } => write!(
                formatter,
                "the review-controlled 2550Q rule-set designation is invalid: {message}"
            ),
            Self::ReviewedRegistryInvalid { message } => {
                write!(
                    formatter,
                    "the reviewed rules registry is invalid: {message}"
                )
            }
            Self::Capture(error) => write!(formatter, "raw 2550Q capture failed: {error}"),
            Self::AdapterNotReviewedComplete => formatter.write_str(
                "the 2550Q raw adapter is not reviewed complete for diagnostic evaluation",
            ),
        }
    }
}

impl std::error::Error for Form2550QRepoLiveValidationUnavailableReason {}

fn repo_unavailable_from_registry_error(
    error: RegistryError,
) -> Form2550QRepoLiveValidationUnavailableReason {
    match error {
        RegistryError::NotFound { .. } => {
            Form2550QRepoLiveValidationUnavailableReason::ExactRuleSetNotRegistered
        }
        error => Form2550QRepoLiveValidationUnavailableReason::ReviewedRegistryInvalid {
            message: error.to_string(),
        },
    }
}

/// The complete public repository-default 2550Q live-validation state space.
///
/// There is intentionally no `Evaluated`, report, trusted proof, or artifact
/// variant until all seven raw-control families and a reviewed generated
/// package are present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form2550QRepoLiveValidationState {
    Unavailable {
        rule_set: FormRevisionKey,
        reason: Form2550QRepoLiveValidationUnavailableReason,
    },
    IncompleteCapture {
        rule_set: FormRevisionKey,
        gaps: Vec<Form2550QCaptureGap>,
    },
}

impl Form2550QRepoLiveValidationState {
    pub const fn rule_set(&self) -> &FormRevisionKey {
        match self {
            Self::Unavailable { rule_set, .. } | Self::IncompleteCapture { rule_set, .. } => {
                rule_set
            }
        }
    }
}

pub(crate) fn validate_form_2550q_raw_bindings(
    draft: &Form2550QDraft,
) -> Result<(), Form2550QCaptureError> {
    reject_ambiguous_legacy_repeated_bindings(draft)?;
    let materialized = materialized_instances(draft);
    validate_raw_state_bindings_against(draft, &materialized)?;
    validate_malformed_bindings_against(draft, &materialized)
}

fn validate_raw_state_bindings_against(
    draft: &Form2550QDraft,
    materialized: &BTreeMap<Form2550QRowFamily, BTreeSet<StableInstanceId>>,
) -> Result<(), Form2550QCaptureError> {
    let singleton_fields = SINGLETON_FIELD_IDS
        .into_iter()
        .chain(LOCAL_PRINT_RAW_FIELD_KEYS)
        .collect::<BTreeSet<_>>();
    for field_key in draft.raw_editor_state.singleton_fields().keys() {
        if !singleton_fields.contains(field_key.as_str()) {
            return Err(Form2550QCaptureError::UnexpectedSingletonField {
                field_key: field_key.clone(),
            });
        }
    }

    for (family, instances) in draft.raw_editor_state.repeated_fields() {
        let binding = binding_for_family(*family);
        let allowed_members = binding
            .members
            .iter()
            .map(|member| member.field_id)
            .collect::<BTreeSet<_>>();
        let declared_instances = materialized
            .get(family)
            .expect("all 2550Q row families have materialized slots");
        for (instance_id, values) in instances {
            if !declared_instances.contains(instance_id) {
                return Err(Form2550QCaptureError::OrphanRepeatedInstance {
                    family: *family,
                    instance_id: instance_id.clone(),
                });
            }
            for field_key in values.keys() {
                if !allowed_members.contains(field_key.as_str()) {
                    return Err(Form2550QCaptureError::UnexpectedRepeatedField {
                        family: *family,
                        instance_id: instance_id.clone(),
                        field_key: field_key.clone(),
                    });
                }
            }
        }
    }
    Ok(())
}

fn validate_malformed_bindings_against(
    draft: &Form2550QDraft,
    materialized: &BTreeMap<Form2550QRowFamily, BTreeSet<StableInstanceId>>,
) -> Result<(), Form2550QCaptureError> {
    for path in draft.raw_editor_state.malformed_fields().keys() {
        if SINGLETON_FIELD_IDS.contains(&path.as_str())
            || LOCAL_PRINT_RAW_FIELD_KEYS.contains(&path.as_str())
        {
            if draft.raw_editor_state.singleton_value(path).is_none() {
                return Err(Form2550QCaptureError::MalformedPathWithoutRawValue {
                    path: path.clone(),
                });
            }
            continue;
        }

        let parts = path.split('/').collect::<Vec<_>>();
        if parts.len() != 3 {
            return Err(Form2550QCaptureError::InvalidMalformedPath { path: path.clone() });
        }
        let Some(family) = row_family_from_canonical_path(parts[0]) else {
            return Err(Form2550QCaptureError::InvalidMalformedPath { path: path.clone() });
        };
        if !is_fixed_width_row_identity(parts[1]) {
            return Err(Form2550QCaptureError::InvalidMalformedPath { path: path.clone() });
        }
        let instance_id = StableInstanceId::parse(parts[1])
            .map_err(|_| Form2550QCaptureError::InvalidMalformedPath { path: path.clone() })?;
        let binding = binding_for_family(family);
        if !binding
            .members
            .iter()
            .any(|member| member.field_id == parts[2])
        {
            return Err(Form2550QCaptureError::UnknownMalformedField { path: path.clone() });
        }
        if !materialized
            .get(&family)
            .expect("all 2550Q row families have materialized slots")
            .contains(&instance_id)
        {
            return Err(Form2550QCaptureError::OrphanMalformedRepeatedInstance {
                path: path.clone(),
                family,
                instance_id,
            });
        }
        if draft
            .raw_editor_state
            .repeated_value(family, &instance_id, parts[2])
            .is_none()
        {
            return Err(Form2550QCaptureError::MalformedPathWithoutRawValue { path: path.clone() });
        }
    }
    Ok(())
}

fn row_family_from_canonical_path(value: &str) -> Option<Form2550QRowFamily> {
    match value {
        "schedule-1" => Some(Form2550QRowFamily::Schedule1),
        "schedule-3" => Some(Form2550QRowFamily::Schedule3),
        "schedule-4" => Some(Form2550QRowFamily::Schedule4),
        "item-19-additional" => Some(Form2550QRowFamily::Item19Additional),
        "item-42-additional" => Some(Form2550QRowFamily::Item42Additional),
        "item-47-additional" => Some(Form2550QRowFamily::Item47Additional),
        "item-56-additional" => Some(Form2550QRowFamily::Item56Additional),
        _ => None,
    }
}

const fn canonical_path_for_row_family(family: Form2550QRowFamily) -> &'static str {
    match family {
        Form2550QRowFamily::Schedule1 => "schedule-1",
        Form2550QRowFamily::Schedule3 => "schedule-3",
        Form2550QRowFamily::Schedule4 => "schedule-4",
        Form2550QRowFamily::Item19Additional => "item-19-additional",
        Form2550QRowFamily::Item42Additional => "item-42-additional",
        Form2550QRowFamily::Item47Additional => "item-47-additional",
        Form2550QRowFamily::Item56Additional => "item-56-additional",
    }
}

fn is_fixed_width_row_identity(value: &str) -> bool {
    value.len() == "row-".len() + 20
        && value.starts_with("row-")
        && value["row-".len()..]
            .bytes()
            .all(|byte| byte.is_ascii_digit())
        && value["row-".len()..].bytes().any(|byte| byte != b'0')
}

fn materialized_instances(
    draft: &Form2550QDraft,
) -> BTreeMap<Form2550QRowFamily, BTreeSet<StableInstanceId>> {
    [
        (
            Form2550QRowFamily::Schedule1,
            draft
                .schedule_1
                .iter()
                .filter_map(|row| row.instance_id.clone())
                .collect(),
        ),
        (
            Form2550QRowFamily::Schedule3,
            draft
                .schedule_3
                .iter()
                .filter_map(|row| row.instance_id.clone())
                .collect(),
        ),
        (
            Form2550QRowFamily::Schedule4,
            draft
                .schedule_4
                .iter()
                .filter_map(|row| row.instance_id.clone())
                .collect(),
        ),
        (
            Form2550QRowFamily::Item19Additional,
            draft
                .item_19_additional_rows
                .iter()
                .filter_map(|row| row.instance_id.clone())
                .collect(),
        ),
        (
            Form2550QRowFamily::Item42Additional,
            draft
                .item_42_additional_rows
                .iter()
                .filter_map(|row| row.instance_id.clone())
                .collect(),
        ),
        (
            Form2550QRowFamily::Item47Additional,
            draft
                .item_47_additional_rows
                .iter()
                .filter_map(|row| row.instance_id.clone())
                .collect(),
        ),
        (
            Form2550QRowFamily::Item56Additional,
            draft
                .item_56_additional_rows
                .iter()
                .filter_map(|row| row.instance_id.clone())
                .collect(),
        ),
    ]
    .into_iter()
    .collect()
}

fn binding_for_family(family: Form2550QRowFamily) -> &'static Form2550QGroupBinding {
    GROUP_BINDINGS
        .iter()
        .find(|binding| binding.family == family)
        .expect("all 2550Q row families have an inert binding")
}

fn parse_field_id(value: &'static str) -> FieldId {
    FieldId::parse(value).expect("static 2550Q adapter field ID is valid")
}

fn parse_group_id(value: &'static str) -> RepeatedGroupId {
    RepeatedGroupId::parse(value).expect("static 2550Q adapter group ID is valid")
}

/// Seed the persisted raw-state boundary from a reviewed 160-key source map.
///
/// This is import-only glue. It copies exact source strings; it never formats
/// typed values. Additional-item groups deliberately have no reviewed XML
/// projection and therefore are not seeded.
pub(crate) fn seed_raw_editor_state_from_reviewed_fields(
    draft: &mut Form2550QDraft,
    fields: &BTreeMap<String, String>,
) -> Result<(), Form2550QCaptureError> {
    draft.raw_editor_state.validate_version()?;
    draft.ensure_repeating_row_ids()?;

    for field_key in SINGLETON_FIELD_IDS {
        if let Some(value) = fields.get(field_key) {
            draft
                .raw_editor_state
                .set_singleton(field_key, RawValue::Text(value.clone()));
        }
    }

    seed_repeated_family(
        &mut draft.raw_editor_state,
        Form2550QRowFamily::Schedule1,
        draft
            .schedule_1
            .iter()
            .map(|row| row.instance_id.clone())
            .collect(),
        &SCHEDULE_1_MEMBERS,
        fields,
        &[10, 11],
    );
    seed_repeated_family(
        &mut draft.raw_editor_state,
        Form2550QRowFamily::Schedule3,
        draft
            .schedule_3
            .iter()
            .map(|row| row.instance_id.clone())
            .collect(),
        &SCHEDULE_3_MEMBERS,
        fields,
        &[0, 1],
    );
    seed_repeated_family(
        &mut draft.raw_editor_state,
        Form2550QRowFamily::Schedule4,
        draft
            .schedule_4
            .iter()
            .map(|row| row.instance_id.clone())
            .collect(),
        &SCHEDULE_4_MEMBERS,
        fields,
        &[0, 1],
    );
    Ok(())
}

fn seed_repeated_family(
    raw_state: &mut Form2550QRawEditorState,
    family: Form2550QRowFamily,
    instance_ids: Vec<Option<StableInstanceId>>,
    members: &[Form2550QMemberBinding],
    fields: &BTreeMap<String, String>,
    suffixes: &[u8],
) {
    for (instance_id, suffix) in instance_ids.into_iter().zip(suffixes.iter().copied()) {
        let Some(instance_id) = instance_id else {
            continue;
        };
        for member in members {
            let Some(prefix) = member.reviewed_xml_prefix else {
                continue;
            };
            let xml_key = format!("{prefix}{suffix}");
            if let Some(value) = fields.get(&xml_key) {
                raw_state.set_repeated(
                    family,
                    instance_id.clone(),
                    member.field_id,
                    RawValue::Text(value.clone()),
                );
            }
        }
    }
}

#[cfg(test)]
mod live_validation_tests {
    use super::*;
    use bir_rules::{
        CompiledRuleSet, EvaluationExpectation, EvaluationOutput, FormCode, FormRevision,
        OfficialPackageVersion, RuleSetId, RuleSetRegistryEntry, Sha256Digest,
    };
    use std::sync::Mutex;

    use crate::profile::TaxpayerProfile;

    static NO_ENTRIES: [RuleSetRegistryEntry; 0] = [];

    #[derive(Default)]
    struct RecordingUnavailableRegistry {
        requests: Mutex<Vec<FormRevisionKey>>,
    }

    impl RuleSetRegistry for RecordingUnavailableRegistry {
        fn entries(&self) -> &[RuleSetRegistryEntry] {
            &NO_ENTRIES
        }

        fn resolve(
            &self,
            requested: &FormRevisionKey,
        ) -> Result<&'static dyn CompiledRuleSet, RegistryError> {
            self.requests
                .lock()
                .expect("recording registry lock")
                .push(requested.clone());
            Err(RegistryError::NotFound {
                requested: requested.clone(),
            })
        }
    }

    fn exact_2550q_identity(digest_byte: u8) -> FormRevisionKey {
        FormRevisionKey::new(
            RuleSetId::parse("2550q-v2024-p7.9.6.0").unwrap(),
            FormCode::parse(FORM_2550Q_CODE).unwrap(),
            FormRevision::parse(FORM_2550Q_REVISION).unwrap(),
            OfficialPackageVersion::parse("7.9.6.0").unwrap(),
            Sha256Digest::from_bytes([digest_byte; 32]),
        )
    }

    fn profile() -> TaxpayerProfile {
        serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": "JUAN DELA CRUZ",
            "tin": {
                "segment1": "000",
                "segment2": "000",
                "segment3": "000",
                "branch": "00000"
            },
            "rdo_code": "018",
            "line_of_business": "SOFTWARE DEVELOPMENT",
            "registered_address": "OLONGAPO",
            "zip_code": "2200",
            "phone": "09123456789",
            "email": "taxpayer@example.com",
            "default_form_type": "2550Qv2024",
            "taxpayer_type": "Individual",
            "tax_classification": "SelfEmployed",
            "is_vat_registered": true,
            "eopt_tier": "Medium",
            "withholding_obligations": [],
            "excise_tax_liabilities": [],
            "atc_codes": [],
            "business_start_date": null,
            "birth_date": null,
            "registration_activity_status": "Active",
            "is_dormant_entity": false,
            "is_government_withholding_entity": false,
            "is_gpp_partner": false,
            "is_top_withholding_agent": false,
            "income_tax_elections": { "elections": {} },
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        }))
        .expect("valid test profile")
    }

    fn draft() -> Form2550QDraft {
        Form2550QDraft::new_from_profile(&profile(), 2026, 1)
    }

    #[test]
    fn pin_requires_the_complete_exact_2550q_revision_and_package() {
        let exact = exact_2550q_identity(7);
        let pin = Form2550QRuleSetPin::try_new(exact.clone()).expect("exact 2550Q identity pins");
        assert_eq!(pin.rule_set(), &exact);
        assert_eq!(
            pin.rule_set().official_package_version().as_str(),
            "7.9.6.0"
        );

        let wrong_form = FormRevisionKey::new(
            RuleSetId::parse("other-v1-p1").unwrap(),
            FormCode::parse("2551Q").unwrap(),
            FormRevision::parse(FORM_2550Q_REVISION).unwrap(),
            OfficialPackageVersion::parse("p1").unwrap(),
            Sha256Digest::from_bytes([1; 32]),
        );
        assert!(matches!(
            Form2550QRuleSetPin::try_new(wrong_form),
            Err(Form2550QRuleSetPinError::WrongFormCode { .. })
        ));

        let wrong_revision = FormRevisionKey::new(
            RuleSetId::parse("2550q-v2023-p1").unwrap(),
            FormCode::parse(FORM_2550Q_CODE).unwrap(),
            FormRevision::parse("2023-01-01").unwrap(),
            OfficialPackageVersion::parse("p1").unwrap(),
            Sha256Digest::from_bytes([2; 32]),
        );
        assert!(matches!(
            Form2550QRuleSetPin::try_new(wrong_revision),
            Err(Form2550QRuleSetPinError::WrongFormRevision { .. })
        ));

        let wrong_package = FormRevisionKey::new(
            RuleSetId::parse("2550q-v2024-other-package").unwrap(),
            FormCode::parse(FORM_2550Q_CODE).unwrap(),
            FormRevision::parse(FORM_2550Q_REVISION).unwrap(),
            OfficialPackageVersion::parse("8.0.0").unwrap(),
            Sha256Digest::from_bytes([3; 32]),
        );
        assert!(matches!(
            Form2550QRuleSetPin::try_new(wrong_package),
            Err(Form2550QRuleSetPinError::WrongOfficialPackageVersion { .. })
        ));
    }

    #[test]
    fn unavailable_exact_identity_is_not_retried_with_a_fallback_or_provider() {
        let identity = exact_2550q_identity(8);
        let registry = RecordingUnavailableRegistry::default();
        let facade = Form2550QLiveValidationEvaluator::new(
            &registry,
            Form2550QRuleSetPin::try_new(identity.clone()).unwrap(),
        );

        let state = facade.evaluate_filing_safe_trusted(
            &mut draft(),
            ValidationPhase::Validate,
            InputRevision::new(3),
            Vec::new(),
        );

        assert!(matches!(
            &state,
            Form2550QLiveValidationState::Unavailable {
                rule_set,
                error: RegistryError::NotFound { requested },
            } if rule_set == &identity && requested == &identity
        ));
        assert_eq!(
            *registry.requests.lock().expect("recording registry lock"),
            vec![identity],
            "there must be one exact lookup and no current-version or handwritten fallback"
        );
    }

    /// Every repeated family has editor controls now: Schedules 1, 3 and 4
    /// and the item 19/42/47/56 "(Add more...)" popup rows. A controlless
    /// family would still be reported as a non-blocking capture gap.
    ///
    /// This test pins the set. If a family gains or loses controls, or the
    /// blocking classification changes, the seam's behaviour changed and the
    /// decision recorded on `Form2550QCaptureGap::is_blocking` must be revisited.
    #[test]
    fn controlless_families_are_reported_but_do_not_obstruct_capture() {
        let without_controls: Vec<&str> = GROUP_BINDINGS
            .iter()
            .filter(|binding| !binding.has_live_editor_controls)
            .map(|binding| binding.group_id)
            .collect();

        assert!(
            without_controls.is_empty(),
            "the set of families lacking editor controls changed; the seam blocker moved: {without_controls:?}"
        );

        let source =
            Form2550QFieldValueSource::try_capture(&mut draft()).expect("raw capture succeeds");
        let controlless = source
            .gaps()
            .iter()
            .filter(|gap| matches!(gap, Form2550QCaptureGap::NoLiveEditorControls { .. }))
            .count();
        assert_eq!(
            controlless,
            without_controls.len(),
            "every controlless family must surface as a capture gap"
        );
    }

    /// The seam, end to end: a fully captured draft reaches the evaluator.
    ///
    /// This is what could not happen before. `request_from_capture` refused on
    /// any gap, and the four controlless families emitted theirs
    /// unconditionally, so no draft — however complete — produced an
    /// `EvaluationRequest`. The adapter and the rules engine had never met.
    ///
    /// Constructing a request is not authorization. The reviewed registry is
    /// still empty, so resolving this request in a production build fails; that
    /// boundary is deliberate and unaffected.
    #[test]
    fn a_fully_captured_draft_produces_an_evaluation_request() {
        let mut draft = draft();
        for field_key in SINGLETON_FIELD_IDS {
            draft
                .raw_editor_state
                .set_singleton(field_key, RawValue::Text(String::new()));
        }

        // A fresh draft materializes rows for the three families that do have
        // controls, so their members need buffers too. Deriving the work from
        // the reported gaps keeps this honest: whatever capture says is missing
        // is exactly what gets filled.
        let probe = Form2550QFieldValueSource::try_capture(&mut draft).expect("probe capture");
        let missing: Vec<FieldInstance> = probe
            .gaps()
            .iter()
            .filter_map(|gap| match gap {
                Form2550QCaptureGap::MissingRawBuffer { field } => Some(field.clone()),
                _ => None,
            })
            .collect();
        for field in missing {
            let instance = field
                .group_path()
                .first()
                .expect("only repeated members can still be missing");
            let family = GROUP_BINDINGS
                .iter()
                .find(|binding| binding.group_id == instance.group_id().as_str())
                .expect("every materialized row belongs to a declared family")
                .family;
            draft.raw_editor_state.set_repeated(
                family,
                instance.instance_id().clone(),
                field.field_id().as_str(),
                RawValue::Text(String::new()),
            );
        }

        let source =
            Form2550QFieldValueSource::try_capture(&mut draft).expect("raw capture succeeds");
        assert!(
            !source.gaps().iter().any(|gap| gap.is_blocking()),
            "a fully populated draft must leave no obstructive gap: {:?}",
            source.gaps()
        );

        let registry = RecordingUnavailableRegistry::default();
        let facade = Form2550QLiveValidationEvaluator::new(
            &registry,
            Form2550QRuleSetPin::try_new(exact_2550q_identity(9)).unwrap(),
        );
        let request = facade
            .request_from_capture(
                source,
                ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe),
                InputRevision::new(1),
                Vec::new(),
            )
            .expect("a complete capture must produce an EvaluationRequest");

        // 66 singleton fields plus the 40 materialized repeated members —
        // the same 66/40 split the serialization binding inventory records for
        // the live control surface, arrived at independently from the app side.
        assert_eq!(request.raw_inputs().fields().len(), 106);
        assert_eq!(
            request.raw_inputs().fields().len(),
            SINGLETON_FIELD_IDS.len() + 40
        );
        assert_eq!(request.context().phase(), ValidationPhase::Validate);
    }

    #[test]
    fn every_capture_gap_blocks_request_construction_and_dispatch() {
        let registry = RecordingUnavailableRegistry::default();
        let facade = Form2550QLiveValidationEvaluator::new(
            &registry,
            Form2550QRuleSetPin::try_new(exact_2550q_identity(9)).unwrap(),
        );
        let source =
            Form2550QFieldValueSource::try_capture(&mut draft()).expect("raw capture succeeds");
        assert!(!source.gaps().is_empty());

        let state = facade
            .request_from_capture(
                source,
                ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe),
                InputRevision::new(4),
                Vec::new(),
            )
            .expect_err("capture gaps must prevent EvaluationRequest construction");

        assert!(matches!(
            state,
            Form2550QLiveValidationState::IncompleteCapture { ref gaps, .. }
                if !gaps.is_empty()
        ));
        assert!(
            registry
                .requests
                .lock()
                .expect("recording registry lock")
                .is_empty(),
            "incomplete capture must not dispatch to a provider"
        );
    }

    #[test]
    fn persisted_raw_rows_with_missing_legacy_identity_fail_before_allocation() {
        let mut draft = draft();
        let persisted_id = draft.schedule_1[0]
            .instance_id
            .clone()
            .expect("new row is identified");
        draft.raw_editor_state.set_repeated(
            Form2550QRowFamily::Schedule1,
            persisted_id,
            "txtInputTax1",
            RawValue::Text("10.00".to_string()),
        );
        draft.schedule_1[0].instance_id = None;

        assert!(matches!(
            Form2550QFieldValueSource::try_capture(&mut draft),
            Err(Form2550QCaptureError::AmbiguousLegacyRepeatedBinding {
                family: Form2550QRowFamily::Schedule1
            })
        ));
        assert!(
            draft.schedule_1[0].instance_id.is_none(),
            "capture must fail before assigning a positional replacement identity"
        );
    }

    #[test]
    fn malformed_paths_are_strictly_bound_and_stable_across_reorder() {
        let mut subject = draft();
        let instance_id = subject.schedule_1[0]
            .instance_id
            .clone()
            .expect("new row is identified");
        subject.raw_editor_state.set_repeated(
            Form2550QRowFamily::Schedule1,
            instance_id.clone(),
            "txtInputTax1",
            RawValue::Text("not-money".to_string()),
        );
        let key = Form2550QRawFieldKey::try_repeated(
            Form2550QRowFamily::Schedule1,
            instance_id.clone(),
            "txtInputTax1",
        )
        .expect("declared repeated key");
        assert_eq!(
            key.stable_path(),
            format!("schedule-1/{instance_id}/txtInputTax1")
        );
        key.mark_malformed(&mut subject.raw_editor_state, "not money");
        let first =
            Form2550QFieldValueSource::try_capture(&mut subject).expect("canonical path captures");
        subject.schedule_1.reverse();
        let reordered = Form2550QFieldValueSource::try_capture(&mut subject)
            .expect("reorder stays bound by ID");
        assert_eq!(first, reordered);
        assert!(matches!(
            Form2550QRawFieldKey::try_repeated(
                Form2550QRowFamily::Schedule1,
                instance_id,
                "futureMember"
            ),
            Err(Form2550QRawFieldKeyError::UnknownRepeatedMember { .. })
        ));

        let mut noncanonical = draft();
        noncanonical
            .raw_editor_state
            .mark_malformed("schedule-1/row-1/txtInputTax1", "not money");
        assert!(matches!(
            validate_form_2550q_raw_bindings(&noncanonical),
            Err(Form2550QCaptureError::InvalidMalformedPath { .. })
        ));
        assert!(matches!(
            Form2550QFieldValueSource::try_capture(&mut noncanonical),
            Err(Form2550QCaptureError::InvalidMalformedPath { .. })
        ));

        let mut unknown = draft();
        let unknown_id = unknown.schedule_1[0]
            .instance_id
            .clone()
            .expect("new row is identified");
        unknown
            .raw_editor_state
            .mark_malformed(format!("schedule-1/{unknown_id}/futureMember"), "not money");
        assert!(matches!(
            Form2550QFieldValueSource::try_capture(&mut unknown),
            Err(Form2550QCaptureError::UnknownMalformedField { .. })
        ));

        let mut missing_raw = draft();
        missing_raw
            .raw_editor_state
            .mark_malformed("frm2550qv2024:surcharge", "not money");
        assert!(matches!(
            Form2550QFieldValueSource::try_capture(&mut missing_raw),
            Err(Form2550QCaptureError::MalformedPathWithoutRawValue { .. })
        ));

        let mut orphan = draft();
        orphan.raw_editor_state.mark_malformed(
            "schedule-1/row-00000000000000000999/txtInputTax1",
            "not money",
        );
        assert!(matches!(
            Form2550QFieldValueSource::try_capture(&mut orphan),
            Err(Form2550QCaptureError::OrphanMalformedRepeatedInstance {
                family: Form2550QRowFamily::Schedule1,
                ..
            })
        ));
    }

    #[test]
    fn raw_keys_map_to_exact_semantic_instances_without_local_print_aliases() {
        let singleton = Form2550QRawFieldKey::try_singleton("frm2550qv2024:surcharge")
            .expect("reviewed singleton");
        let singleton = singleton
            .semantic_field_instance()
            .expect("rules-backed singleton has semantic identity");
        assert_eq!(singleton.field_id().as_str(), "frm2550qv2024:surcharge");
        assert!(singleton.group_path().is_empty());

        let instance_id = StableInstanceId::parse("row-00000000000000000042")
            .expect("fixed-width test row identity");
        let repeated = Form2550QRawFieldKey::try_repeated(
            Form2550QRowFamily::Schedule1,
            instance_id.clone(),
            "txtInputTax1",
        )
        .expect("reviewed repeated member")
        .semantic_field_instance()
        .expect("rules-backed repeated field has semantic identity");
        assert_eq!(repeated.field_id().as_str(), "txtInputTax1");
        assert_eq!(repeated.group_path().len(), 1);
        assert_eq!(
            repeated.group_path()[0].group_id().as_str(),
            "schedule-1-capital-good-row"
        );
        assert_eq!(repeated.group_path()[0].instance_id(), &instance_id);

        assert!(
            Form2550QRawFieldKey::try_singleton("cash_amount")
                .expect("known local-print key")
                .semantic_field_instance()
                .is_none(),
            "local-print-only controls must not masquerade as rule fields"
        );
    }

    #[test]
    fn candidate_radio_groups_capture_exact_raw_text() {
        let expected = [
            ("frm2550qv2024:calendarNo1", "false"),
            ("frm2550qv2024:fiscalNo1", "true"),
            ("frm2550qv2024:OptQuarter1", "false"),
            ("frm2550qv2024:OptQuarter2", "false"),
            ("frm2550qv2024:OptQuarter3", "true"),
            ("frm2550qv2024:OptQuarter4", "false"),
            ("frm2550qv2024:taxPayerClassification1", "false"),
            ("frm2550qv2024:taxPayerClassification2", "false"),
            ("frm2550qv2024:taxPayerClassification3", "true"),
            ("frm2550qv2024:taxPayerClassification4", "false"),
            ("frm2550qv2024:internationalTreatyYn", "true"),
            ("frm2550qv2024:specialRateYn", "false"),
        ];
        let mut subject = draft();
        for (field_key, raw) in expected {
            Form2550QRawFieldKey::try_singleton(field_key)
                .expect("candidate boolean has a closed raw binding");
            subject
                .raw_editor_state
                .set_singleton(field_key, RawValue::Text(raw.to_string()));
        }

        let source =
            Form2550QFieldValueSource::try_capture(&mut subject).expect("raw capture succeeds");
        for (field_key, raw) in expected {
            let field = FieldInstance::singleton(parse_field_id(field_key));
            assert_eq!(
                source.snapshot().raw_value(&field),
                Some(&RawValue::Text(raw.to_string())),
                "{field_key} must retain its exact InputState text"
            );
            assert!(!source.gaps().iter().any(|gap| matches!(
                gap,
                Form2550QCaptureGap::MissingRawBuffer { field: missing } if missing == &field
            )));
        }
    }

    #[test]
    fn candidate_boolean_snapshot_never_falls_back_to_typed_draft_defaults() {
        let mut subject = draft();
        assert_eq!(
            subject.filing_basis,
            crate::forms::form_2550q::Form2550QFilingBasis::Calendar
        );
        assert_eq!(
            subject.quarter,
            crate::forms::form_2550q::Form2550QQuarter::First
        );

        let source =
            Form2550QFieldValueSource::try_capture(&mut subject).expect("raw capture succeeds");
        for field_key in [
            "frm2550qv2024:calendarNo1",
            "frm2550qv2024:fiscalNo1",
            "frm2550qv2024:OptQuarter1",
            "frm2550qv2024:OptQuarter2",
            "frm2550qv2024:OptQuarter3",
            "frm2550qv2024:OptQuarter4",
            "frm2550qv2024:taxPayerClassification1",
            "frm2550qv2024:taxPayerClassification2",
            "frm2550qv2024:taxPayerClassification3",
            "frm2550qv2024:taxPayerClassification4",
            "frm2550qv2024:internationalTreatyYn",
            "frm2550qv2024:specialRateYn",
        ] {
            let field = FieldInstance::singleton(parse_field_id(field_key));
            assert_eq!(
                source.snapshot().raw_value(&field),
                None,
                "{field_key} must stay missing until its live control is materialized"
            );
            assert!(source.gaps().iter().any(|gap| matches!(
                gap,
                Form2550QCaptureGap::MissingRawBuffer { field: missing } if missing == &field
            )));
        }
    }

    #[test]
    fn candidate_taxable_year_snapshot_uses_only_captured_raw_text() {
        let field_key = "frm2550qv2024:txtYearNo2";
        let field = FieldInstance::singleton(parse_field_id(field_key));
        let mut subject = draft();
        assert_eq!(subject.taxable_year, 2026);

        let missing =
            Form2550QFieldValueSource::try_capture(&mut subject).expect("raw capture succeeds");
        assert_eq!(
            missing.snapshot().raw_value(&field),
            None,
            "the typed taxable year must not be formatted into raw authority"
        );
        assert!(missing.gaps().iter().any(|gap| matches!(
            gap,
            Form2550QCaptureGap::MissingRawBuffer { field: missing } if missing == &field
        )));

        subject
            .raw_editor_state
            .set_singleton(field_key, RawValue::Text("  2024  ".to_string()));
        let captured =
            Form2550QFieldValueSource::try_capture(&mut subject).expect("raw capture succeeds");
        assert_eq!(
            captured.snapshot().raw_value(&field),
            Some(&RawValue::Text("  2024  ".to_string())),
            "the candidate must receive the exact live control text"
        );
        assert_eq!(
            subject.taxable_year, 2026,
            "capturing raw Item 2 text must not rewrite the typed draft"
        );
    }

    #[test]
    fn candidate_profile_snapshot_uses_only_captured_raw_text() {
        let expected = [
            ("frm2550qv2024:txtTIN1", " 123 "),
            ("frm2550qv2024:txtTIN2", " 456 "),
            ("frm2550qv2024:txtTIN3", " 789 "),
            ("frm2550qv2024:branchCode", " 00001 "),
            ("frm2550qv2024:txtRDOCode", " 000 "),
            ("frm2550qv2024:taxpayerName", "  RAW TAXPAYER  "),
            ("frm2550qv2024:taxpayerAddress", "  RAW ADDRESS  "),
            ("frm2550qv2024:taxpayerZip", " 0000 "),
            ("frm2550qv2024:taxpayerContactNumber", " RAW CONTACT "),
            ("frm2550qv2024:taxpayerEmailAddress", " raw@example.test "),
        ];
        let mut subject = draft();
        let typed_before = (
            subject.tin.clone(),
            subject.rdo_code.clone(),
            subject.taxpayer_name.clone(),
            subject.registered_address.clone(),
            subject.zip_code.clone(),
            subject.contact_number.clone(),
            subject.email.clone(),
        );

        let missing =
            Form2550QFieldValueSource::try_capture(&mut subject).expect("raw capture succeeds");
        for (field_key, _) in expected {
            let field = FieldInstance::singleton(parse_field_id(field_key));
            assert_eq!(
                missing.snapshot().raw_value(&field),
                None,
                "{field_key} must not be formatted from the profile-backed typed draft"
            );
            assert!(missing.gaps().iter().any(|gap| matches!(
                gap,
                Form2550QCaptureGap::MissingRawBuffer { field: missing } if missing == &field
            )));
        }

        for (field_key, raw) in expected {
            subject
                .raw_editor_state
                .set_singleton(field_key, RawValue::Text(raw.to_string()));
        }
        let captured =
            Form2550QFieldValueSource::try_capture(&mut subject).expect("raw capture succeeds");
        for (field_key, raw) in expected {
            let field = FieldInstance::singleton(parse_field_id(field_key));
            assert_eq!(
                captured.snapshot().raw_value(&field),
                Some(&RawValue::Text(raw.to_string())),
                "{field_key} must retain the exact live control text"
            );
        }
        assert_eq!(
            (
                subject.tin,
                subject.rdo_code,
                subject.taxpayer_name,
                subject.registered_address,
                subject.zip_code,
                subject.contact_number,
                subject.email,
            ),
            typed_before,
            "capturing candidate profile text must not rewrite typed/profile fields"
        );
    }

    #[test]
    fn candidate_treaty_detail_snapshot_uses_only_captured_raw_text() {
        let field_key = "frm2550qv2024:specifyInternationalTreaty";
        let field = FieldInstance::singleton(parse_field_id(field_key));
        let mut subject = draft();
        subject.tax_relief_details = "TYPED TREATY DETAIL".to_string();

        let missing =
            Form2550QFieldValueSource::try_capture(&mut subject).expect("raw capture succeeds");
        assert_eq!(
            missing.snapshot().raw_value(&field),
            None,
            "typed Item 14A text must not become raw authority"
        );
        assert!(missing.gaps().iter().any(|gap| matches!(
            gap,
            Form2550QCaptureGap::MissingRawBuffer { field: missing } if missing == &field
        )));

        subject.raw_editor_state.set_singleton(
            field_key,
            RawValue::Text("  RAW TREATY DETAIL  ".to_string()),
        );
        let captured =
            Form2550QFieldValueSource::try_capture(&mut subject).expect("raw capture succeeds");
        assert_eq!(
            captured.snapshot().raw_value(&field),
            Some(&RawValue::Text("  RAW TREATY DETAIL  ".to_string()))
        );
        assert_eq!(
            subject.tax_relief_details, "TYPED TREATY DETAIL",
            "capturing raw Item 14A text must not reconcile the typed draft"
        );
    }

    #[test]
    fn excluded_local_print_buffers_are_named_gaps_and_never_authorize() {
        let registry = RecordingUnavailableRegistry::default();
        let facade = Form2550QLiveValidationEvaluator::new(
            &registry,
            Form2550QRuleSetPin::try_new(exact_2550q_identity(12)).unwrap(),
        );
        let mut draft = draft();
        draft.raw_editor_state.set_singleton(
            "signatory",
            RawValue::Text("AUTHORIZED REPRESENTATIVE".to_string()),
        );
        draft
            .raw_editor_state
            .set_singleton("cash_amount", RawValue::Text("not-money".to_string()));
        Form2550QRawFieldKey::try_singleton("cash_amount")
            .expect("known local-print raw key")
            .mark_malformed(&mut draft.raw_editor_state, "not money");
        let source =
            Form2550QFieldValueSource::try_capture(&mut draft).expect("raw capture succeeds");
        assert_eq!(
            source.excluded_local_print_buffer_keys(),
            ["signatory", "cash_amount"]
        );
        assert!(source.gaps().iter().any(|gap| matches!(
            gap,
            Form2550QCaptureGap::ExcludedLocalPrintRawBuffer {
                field_key: "signatory"
            }
        )));
        assert!(source.gaps().iter().any(|gap| matches!(
            gap,
            Form2550QCaptureGap::ExcludedLocalPrintRawBuffer {
                field_key: "cash_amount"
            }
        )));

        assert!(matches!(
            facade.request_from_capture(
                source,
                ValidationContext::new(ValidationPhase::Validate, BehaviorProfile::FilingSafe,),
                InputRevision::new(7),
                Vec::new(),
            ),
            Err(Form2550QLiveValidationState::IncompleteCapture { .. })
        ));
    }

    #[test]
    fn official_compatibility_result_has_no_trusted_authorization() {
        let request = EvaluationRequest::try_new(
            exact_2550q_identity(10),
            ValidationContext::new(
                ValidationPhase::Validate,
                BehaviorProfile::OfficialCompatibility,
            ),
            InputRevision::new(5),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
        let expectation = EvaluationExpectation::try_new(Vec::new(), Vec::new()).unwrap();
        let result = EvaluationResult::try_new(
            &request,
            &expectation,
            EvaluationOutput::new(Vec::new(), Vec::new(), Vec::new(), Vec::new()),
        )
        .unwrap();
        let evaluation = Form2550QLiveEvaluation::OfficialDiagnostic(result);

        assert!(evaluation.trusted_evaluation().is_none());
        assert_eq!(
            evaluation.report().context().profile(),
            BehaviorProfile::OfficialCompatibility
        );
    }

    #[test]
    fn repo_default_unreviewed_2550q_is_unavailable_without_report_or_artifact() {
        let identity = exact_2550q_identity(11);
        let state = evaluate_repo_default_impl(
            Form2550QRuleSetPin::try_new(identity.clone()).unwrap(),
            &mut draft(),
        );

        assert!(matches!(
            &state,
            Form2550QRepoLiveValidationState::Unavailable {
                rule_set,
                reason: Form2550QRepoLiveValidationUnavailableReason::ExactRuleSetNotRegistered,
            } if rule_set == &identity
        ));
        assert_eq!(state.rule_set(), &identity);
    }

    #[test]
    fn repo_default_setup_does_not_autoactivate_without_review_designation() {
        assert!(
            reviewed_repo_default_designation().is_none(),
            "reviewed providers and cfg(test) candidates must remain inert without designation"
        );
        let setup = Form2550QLiveValidationFacade::setup_repo_default_diagnostic();
        assert!(matches!(
            &setup,
            Form2550QRepoDiagnosticSetupOutcome::Unavailable {
                reason: Form2550QRepoLiveValidationUnavailableReason::ExactRuleSetNotDesignated,
            }
        ));
        let Form2550QRepoDiagnosticSetupOutcome::Unavailable { reason } = setup else {
            panic!("an undesignated repository must not activate 2550Q");
        };
        assert!(
            reason
                .to_string()
                .contains("reviewed registry entries alone do not authorize activation")
        );
    }
}
