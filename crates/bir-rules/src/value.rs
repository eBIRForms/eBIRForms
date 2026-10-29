use crate::{
    ContextFingerprint, ContextValueId, FieldId, RepeatedGroupId, Sha256Digest, StableInstanceId,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest, Sha256};
use std::{error::Error, fmt, str::FromStr};

const CONTEXT_VALUE_SNAPSHOT_FINGERPRINT_DOMAIN: &[u8] = b"bir-rules/context-value-snapshot/v1\0";

/// One materialized instance of a repeated field group.
///
/// `instance_id` must come from persisted draft identity (for example, a row
/// UUID), never from the row's current vector index.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepeatedGroupInstance {
    group_id: RepeatedGroupId,
    instance_id: StableInstanceId,
}

impl RepeatedGroupInstance {
    pub const fn new(group_id: RepeatedGroupId, instance_id: StableInstanceId) -> Self {
        Self {
            group_id,
            instance_id,
        }
    }

    pub fn group_id(&self) -> &RepeatedGroupId {
        &self.group_id
    }

    pub fn instance_id(&self) -> &StableInstanceId {
        &self.instance_id
    }
}

/// Exact identity of one scalar field occurrence.
///
/// `group_path` is ordered outermost to innermost, allowing nested repeated
/// schedules without reducing an instance to a transient numeric row index.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct FieldInstance {
    field_id: FieldId,
    group_path: Vec<RepeatedGroupInstance>,
}

impl FieldInstance {
    pub const fn singleton(field_id: FieldId) -> Self {
        Self {
            field_id,
            group_path: Vec::new(),
        }
    }

    pub fn try_new(
        field_id: FieldId,
        group_path: Vec<RepeatedGroupInstance>,
    ) -> Result<Self, InputSnapshotError> {
        for (index, item) in group_path.iter().enumerate() {
            if group_path[..index]
                .iter()
                .any(|prior| prior.group_id == item.group_id)
            {
                return Err(InputSnapshotError::DuplicateGroupInFieldPath {
                    field_id,
                    group_id: item.group_id.clone(),
                });
            }
        }
        Ok(Self {
            field_id,
            group_path,
        })
    }

    pub fn field_id(&self) -> &FieldId {
        &self.field_id
    }

    pub fn group_path(&self) -> &[RepeatedGroupInstance] {
        &self.group_path
    }
}

impl<'de> Deserialize<'de> for FieldInstance {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            field_id: FieldId,
            group_path: Vec<RepeatedGroupInstance>,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.field_id, wire.group_path).map_err(de::Error::custom)
    }
}

/// Lossless state of one user-editable buffer.
///
/// `Absent` means the adapter has proven that this field occurrence has no
/// materialized buffer. `Text("")` is a present, blank buffer and is not the
/// same state.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(
    tag = "state",
    content = "text",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
pub enum RawValue {
    Absent,
    Text(String),
}

impl RawValue {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Absent => None,
            Self::Text(value) => Some(value),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawFieldValue {
    field: FieldInstance,
    value: RawValue,
}

impl RawFieldValue {
    pub const fn new(field: FieldInstance, value: RawValue) -> Self {
        Self { field, value }
    }

    pub fn field(&self) -> &FieldInstance {
        &self.field
    }

    pub fn value(&self) -> &RawValue {
        &self.value
    }
}

/// Failure to capture a deterministic raw snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputSnapshotError {
    DuplicateGroupInFieldPath {
        field_id: FieldId,
        group_id: RepeatedGroupId,
    },
    DuplicateGroupInstance {
        instance: RepeatedGroupInstance,
    },
    UndeclaredGroupInstance {
        field: FieldInstance,
        instance: RepeatedGroupInstance,
    },
    DuplicateFieldInstance {
        field: FieldInstance,
    },
}

impl fmt::Display for InputSnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateGroupInFieldPath { field_id, group_id } => write!(
                formatter,
                "field {field_id} contains repeated group {group_id} more than once in its path"
            ),
            Self::DuplicateGroupInstance { instance } => write!(
                formatter,
                "repeated group {} contains duplicate stable instance {}",
                instance.group_id, instance.instance_id
            ),
            Self::UndeclaredGroupInstance { field, instance } => write!(
                formatter,
                "field {} refers to undeclared repeated-group instance {}:{}",
                field.field_id, instance.group_id, instance.instance_id
            ),
            Self::DuplicateFieldInstance { field } => {
                write!(
                    formatter,
                    "duplicate raw value for field {}",
                    field.field_id
                )
            }
        }
    }
}

impl Error for InputSnapshotError {}

/// Deterministic, owned raw inputs for one evaluator call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RawInputSnapshot {
    repeated_group_instances: Vec<RepeatedGroupInstance>,
    fields: Vec<RawFieldValue>,
}

impl RawInputSnapshot {
    pub fn try_new(
        mut repeated_group_instances: Vec<RepeatedGroupInstance>,
        mut fields: Vec<RawFieldValue>,
    ) -> Result<Self, InputSnapshotError> {
        repeated_group_instances.sort();
        for pair in repeated_group_instances.windows(2) {
            if pair[0] == pair[1] {
                return Err(InputSnapshotError::DuplicateGroupInstance {
                    instance: pair[0].clone(),
                });
            }
        }

        fields.sort_by(|left, right| left.field.cmp(&right.field));
        for pair in fields.windows(2) {
            if pair[0].field == pair[1].field {
                return Err(InputSnapshotError::DuplicateFieldInstance {
                    field: pair[0].field.clone(),
                });
            }
        }

        for raw in &fields {
            for instance in raw.field.group_path() {
                if repeated_group_instances.binary_search(instance).is_err() {
                    return Err(InputSnapshotError::UndeclaredGroupInstance {
                        field: raw.field.clone(),
                        instance: instance.clone(),
                    });
                }
            }
        }

        Ok(Self {
            repeated_group_instances,
            fields,
        })
    }

    pub fn capture(source: &dyn FieldValueSource) -> Result<Self, InputSnapshotError> {
        Self::try_new(
            source.repeated_group_instances().to_vec(),
            source.raw_fields().to_vec(),
        )
    }

    pub fn repeated_group_instances(&self) -> &[RepeatedGroupInstance] {
        &self.repeated_group_instances
    }

    pub fn fields(&self) -> &[RawFieldValue] {
        &self.fields
    }

    pub fn raw_value(&self, field: &FieldInstance) -> Option<&RawValue> {
        self.fields
            .binary_search_by(|candidate| candidate.field.cmp(field))
            .ok()
            .map(|index| &self.fields[index].value)
    }
}

impl<'de> Deserialize<'de> for RawInputSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            repeated_group_instances: Vec<RepeatedGroupInstance>,
            fields: Vec<RawFieldValue>,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.repeated_group_instances, wire.fields).map_err(de::Error::custom)
    }
}

/// Adapter boundary used by form-specific GPUI/core code.
///
/// Implementations expose an already captured set of stable field instances;
/// evaluation never asks for a numeric row index and never converts malformed
/// text to zero before rules see it.
pub trait FieldValueSource {
    fn repeated_group_instances(&self) -> &[RepeatedGroupInstance];
    fn raw_fields(&self) -> &[RawFieldValue];

    fn raw_value(&self, field: &FieldInstance) -> Option<&RawValue> {
        self.raw_fields()
            .iter()
            .find(|candidate| candidate.field() == field)
            .map(RawFieldValue::value)
    }
}

impl FieldValueSource for RawInputSnapshot {
    fn repeated_group_instances(&self) -> &[RepeatedGroupInstance] {
        self.repeated_group_instances()
    }

    fn raw_fields(&self) -> &[RawFieldValue] {
        self.fields()
    }
}

/// Fixed-point decimal represented without binary floating point.
///
/// Values are normalized (`1.00 == 1`) and serialized as decimal strings, so
/// JSON consumers cannot silently round them through IEEE-754.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExactDecimal {
    coefficient: i128,
    scale: u32,
}

impl ExactDecimal {
    pub const MAX_SCALE: u32 = 28;

    pub fn try_from_parts(mut coefficient: i128, mut scale: u32) -> Result<Self, DecimalError> {
        if scale > Self::MAX_SCALE {
            return Err(DecimalError::ScaleTooLarge {
                scale,
                max: Self::MAX_SCALE,
            });
        }
        while scale > 0 && coefficient % 10 == 0 {
            coefficient /= 10;
            scale -= 1;
        }
        Ok(Self { coefficient, scale })
    }

    pub const fn coefficient(self) -> i128 {
        self.coefficient
    }

    pub const fn scale(self) -> u32 {
        self.scale
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecimalError {
    Empty,
    InvalidSyntax,
    ScaleTooLarge { scale: u32, max: u32 },
    Overflow,
}

impl fmt::Display for DecimalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("decimal must not be empty"),
            Self::InvalidSyntax => formatter.write_str(
                "decimal must use canonical base-10 notation without exponent or whitespace",
            ),
            Self::ScaleTooLarge { scale, max } => {
                write!(formatter, "decimal scale {scale} exceeds maximum {max}")
            }
            Self::Overflow => {
                formatter.write_str("decimal coefficient exceeds signed 128-bit range")
            }
        }
    }
}

impl Error for DecimalError {}

impl FromStr for ExactDecimal {
    type Err = DecimalError;

    fn from_str(source: &str) -> Result<Self, Self::Err> {
        if source.is_empty() {
            return Err(DecimalError::Empty);
        }
        let (negative, unsigned) = match source.strip_prefix('-') {
            Some(rest) if !rest.is_empty() => (true, rest),
            Some(_) => return Err(DecimalError::InvalidSyntax),
            None => (false, source),
        };
        if unsigned.starts_with('+') {
            return Err(DecimalError::InvalidSyntax);
        }

        let (integer, fraction) = match unsigned.split_once('.') {
            Some((integer, fraction))
                if !integer.is_empty() && !fraction.is_empty() && !fraction.contains('.') =>
            {
                (integer, Some(fraction))
            }
            Some(_) => return Err(DecimalError::InvalidSyntax),
            None => (unsigned, None),
        };
        if !integer.bytes().all(|byte| byte.is_ascii_digit())
            || fraction.is_some_and(|digits| !digits.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return Err(DecimalError::InvalidSyntax);
        }

        let scale = u32::try_from(fraction.map_or(0, str::len)).map_err(|_| {
            DecimalError::ScaleTooLarge {
                scale: u32::MAX,
                max: Self::MAX_SCALE,
            }
        })?;
        if scale > Self::MAX_SCALE {
            return Err(DecimalError::ScaleTooLarge {
                scale,
                max: Self::MAX_SCALE,
            });
        }

        let mut coefficient = 0_i128;
        for byte in integer
            .bytes()
            .chain(fraction.into_iter().flat_map(str::bytes))
        {
            let digit = i128::from(byte - b'0');
            coefficient = coefficient
                .checked_mul(10)
                .and_then(|value| {
                    if negative {
                        value.checked_sub(digit)
                    } else {
                        value.checked_add(digit)
                    }
                })
                .ok_or(DecimalError::Overflow)?;
        }
        Self::try_from_parts(coefficient, scale)
    }
}

impl fmt::Display for ExactDecimal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let negative = self.coefficient.is_negative();
        let magnitude = self.coefficient.unsigned_abs().to_string();
        if negative {
            formatter.write_str("-")?;
        }
        let scale = self.scale as usize;
        if scale == 0 {
            return formatter.write_str(&magnitude);
        }
        if magnitude.len() <= scale {
            formatter.write_str("0.")?;
            for _ in 0..(scale - magnitude.len()) {
                formatter.write_str("0")?;
            }
            formatter.write_str(&magnitude)
        } else {
            let split = magnitude.len() - scale;
            formatter.write_str(&magnitude[..split])?;
            formatter.write_str(".")?;
            formatter.write_str(&magnitude[split..])
        }
    }
}

impl Serialize for ExactDecimal {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ExactDecimal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct CanonicalDate {
    year: u16,
    month: u8,
    day: u8,
}

impl CanonicalDate {
    pub fn try_new(year: u16, month: u8, day: u8) -> Result<Self, DateError> {
        if year == 0 || !(1..=12).contains(&month) {
            return Err(DateError { year, month, day });
        }
        let leap =
            year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
        let max_day = match month {
            2 if leap => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        if day == 0 || day > max_day {
            return Err(DateError { year, month, day });
        }
        Ok(Self { year, month, day })
    }

    pub const fn year(self) -> u16 {
        self.year
    }

    pub const fn month(self) -> u8 {
        self.month
    }

    pub const fn day(self) -> u8 {
        self.day
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateError {
    pub year: u16,
    pub month: u8,
    pub day: u8,
}

impl fmt::Display for DateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid canonical date {:04}-{:02}-{:02}",
            self.year, self.month, self.day
        )
    }
}

impl Error for DateError {}

impl<'de> Deserialize<'de> for CanonicalDate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            year: u16,
            month: u8,
            day: u8,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.year, wire.month, wire.day).map_err(de::Error::custom)
    }
}

/// Typed result of raw-to-canonical normalization.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "kebab-case")]
pub enum CanonicalValue {
    Absent,
    Blank,
    Text(String),
    Boolean(bool),
    Integer(i128),
    Decimal(ExactDecimal),
    Date(CanonicalDate),
}

/// One typed, externally supplied evaluation fact.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextValue {
    id: ContextValueId,
    value: CanonicalValue,
}

impl ContextValue {
    pub const fn new(id: ContextValueId, value: CanonicalValue) -> Self {
        Self { id, value }
    }

    pub fn id(&self) -> &ContextValueId {
        &self.id
    }

    pub fn value(&self) -> &CanonicalValue {
        &self.value
    }
}

/// Deterministically ordered external context used by an evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContextValueSnapshot {
    values: Vec<ContextValue>,
}

impl ContextValueSnapshot {
    pub fn try_new(mut values: Vec<ContextValue>) -> Result<Self, ContextSnapshotError> {
        values.sort_by(|left, right| left.id.cmp(&right.id));
        for pair in values.windows(2) {
            if pair[0].id == pair[1].id {
                return Err(ContextSnapshotError::DuplicateId {
                    id: pair[0].id.clone(),
                });
            }
        }
        Ok(Self { values })
    }

    pub fn values(&self) -> &[ContextValue] {
        &self.values
    }

    /// Compute the stable, domain-separated digest of this validated snapshot.
    ///
    /// `try_new` and `Deserialize` enforce canonical ID ordering before this
    /// JSON representation is hashed, so callers cannot assert a digest that
    /// is independent of the values.
    pub fn fingerprint(&self) -> ContextFingerprint {
        let encoded = serde_json::to_vec(self)
            .expect("a typed context value snapshot must always serialize to JSON");
        let mut hasher = Sha256::new();
        hasher.update(CONTEXT_VALUE_SNAPSHOT_FINGERPRINT_DOMAIN);
        hasher.update(encoded);
        ContextFingerprint::new(Sha256Digest::from_bytes(hasher.finalize().into()))
    }

    pub fn get(&self, id: &ContextValueId) -> Option<&CanonicalValue> {
        self.values
            .binary_search_by(|candidate| candidate.id.cmp(id))
            .ok()
            .map(|index| &self.values[index].value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextSnapshotError {
    DuplicateId { id: ContextValueId },
}

impl fmt::Display for ContextSnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateId { id } => {
                write!(
                    formatter,
                    "context snapshot contains duplicate value ID {id}"
                )
            }
        }
    }
}

impl Error for ContextSnapshotError {}

impl<'de> Deserialize<'de> for ContextValueSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            values: Vec<ContextValue>,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.values).map_err(de::Error::custom)
    }
}

/// One lossless normalization record returned by the evaluator.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalFieldValue {
    field: FieldInstance,
    raw: RawValue,
    canonical: CanonicalValue,
}

impl CanonicalFieldValue {
    pub const fn new(field: FieldInstance, raw: RawValue, canonical: CanonicalValue) -> Self {
        Self {
            field,
            raw,
            canonical,
        }
    }

    pub fn field(&self) -> &FieldInstance {
        &self.field
    }

    pub fn raw(&self) -> &RawValue {
        &self.raw
    }

    pub fn canonical(&self) -> &CanonicalValue {
        &self.canonical
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(group: &str, instance: &str) -> RepeatedGroupInstance {
        RepeatedGroupInstance::new(
            RepeatedGroupId::parse(group).unwrap(),
            StableInstanceId::parse(instance).unwrap(),
        )
    }

    #[test]
    fn decimal_is_exact_normalized_and_never_uses_float() {
        let value: ExactDecimal = "9007199254740993.2500".parse().unwrap();
        assert_eq!(value.to_string(), "9007199254740993.25");
        assert_eq!(value.scale(), 2);
        assert_eq!(
            "-170141183460469231731687303715884105728"
                .parse::<ExactDecimal>()
                .unwrap()
                .coefficient(),
            i128::MIN
        );
        assert!("1e2".parse::<ExactDecimal>().is_err());
        assert!(" 1.00".parse::<ExactDecimal>().is_err());
    }

    #[test]
    fn raw_snapshot_requires_stable_declared_group_instances() {
        let row = group("schedule-1", "row-5f1d");
        let field = FieldInstance::try_new(
            FieldId::parse("schedule-1:amount").unwrap(),
            vec![row.clone()],
        )
        .unwrap();
        let raw = RawFieldValue::new(field.clone(), RawValue::Text("12.00".into()));

        assert!(RawInputSnapshot::try_new(vec![row], vec![raw.clone()]).is_ok());
        assert!(matches!(
            RawInputSnapshot::try_new(Vec::new(), vec![raw]),
            Err(InputSnapshotError::UndeclaredGroupInstance { .. })
        ));
        assert!(matches!(
            RawInputSnapshot::try_new(
                Vec::new(),
                vec![
                    RawFieldValue::new(
                        FieldInstance::singleton(FieldId::parse("tin").unwrap()),
                        RawValue::Text("123".into()),
                    ),
                    RawFieldValue::new(
                        FieldInstance::singleton(FieldId::parse("tin").unwrap()),
                        RawValue::Text("456".into()),
                    ),
                ],
            ),
            Err(InputSnapshotError::DuplicateFieldInstance { .. })
        ));
    }

    #[test]
    fn raw_absence_and_present_blank_are_distinct() {
        assert_ne!(RawValue::Absent, RawValue::Text(String::new()));
    }

    #[test]
    fn raw_value_rejects_unknown_persisted_variant_members() {
        assert!(
            serde_json::from_str::<RawValue>(
                r#"{"state":"text","text":"123","future_metadata":{"version":2}}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<RawValue>(
                r#"{"state":"absent","future_metadata":{"version":2}}"#
            )
            .is_err()
        );
        assert_eq!(
            serde_json::from_str::<RawValue>(r#"{"state":"text","text":""}"#).unwrap(),
            RawValue::Text(String::new())
        );
        assert_eq!(
            serde_json::from_str::<RawValue>(r#"{"state":"absent"}"#).unwrap(),
            RawValue::Absent
        );
    }

    #[test]
    fn context_snapshot_rejects_duplicate_ids_before_evaluation() {
        let id = ContextValueId::parse("vat-rate").unwrap();
        let result = ContextValueSnapshot::try_new(vec![
            ContextValue::new(id.clone(), CanonicalValue::Decimal("0.12".parse().unwrap())),
            ContextValue::new(id, CanonicalValue::Decimal("0.10".parse().unwrap())),
        ]);

        assert!(matches!(
            result,
            Err(ContextSnapshotError::DuplicateId { .. })
        ));
    }

    #[test]
    fn context_snapshot_fingerprint_is_order_independent_and_value_sensitive() {
        let rate = ContextValue::new(
            ContextValueId::parse("vat-rate").unwrap(),
            CanonicalValue::Decimal("0.12".parse().unwrap()),
        );
        let period = ContextValue::new(
            ContextValueId::parse("filing-period").unwrap(),
            CanonicalValue::Text("2026-Q1".into()),
        );
        let first = ContextValueSnapshot::try_new(vec![rate.clone(), period.clone()]).unwrap();
        let reordered = ContextValueSnapshot::try_new(vec![period.clone(), rate]).unwrap();
        let changed = ContextValueSnapshot::try_new(vec![
            period,
            ContextValue::new(
                ContextValueId::parse("vat-rate").unwrap(),
                CanonicalValue::Decimal("0.10".parse().unwrap()),
            ),
        ])
        .unwrap();

        assert_eq!(first.fingerprint(), reordered.fingerprint());
        assert_ne!(first.fingerprint(), changed.fingerprint());
    }

    #[test]
    fn empty_context_snapshot_fingerprint_has_a_stable_domain_separated_vector() {
        let snapshot = ContextValueSnapshot::try_new(Vec::new()).unwrap();

        assert_eq!(
            snapshot.fingerprint().digest().to_hex(),
            "952b13dc2394748e06a06ba4dcc1a7e62ed64b7fa95217a89f68567add9b4d1e"
        );
    }
}
