use crate::{
    CanonicalValue, ExactDecimal, XmlKey,
    serialization_contract::{
        SerializationDecimalFormat, SerializationDecimalSeparator, SerializationGrouping,
        SerializationNegativeRepresentation, SerializationPresentFormat,
        SerializationSemanticFormat,
    },
    static_ir::{ExactRoundingError, round_exact_decimal_to_scale},
};
use serde::{Deserialize, Deserializer, Serialize, de};
use std::{error::Error, fmt, str::FromStr};

const MAX_ARTIFACT_VARIANT_ID_BYTES: usize = 128;
const UPPER_HEX: &[u8; 16] = b"0123456789ABCDEF";

/// The physical artifact for which a reviewed serialization contract applies.
///
/// The variants are intentionally closed. A new artifact kind requires a code
/// change instead of silently inheriting another artifact's occurrence plan or
/// codecs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SerializationArtifactTarget {
    EditableSave,
    FinalizedSave,
    EncryptedFinalCopy,
    SubmissionPayload,
    HistoricalImportCompatibility,
}

/// Stable identity for a form-specific serialization path within an artifact
/// target, such as a local savefile path or an encrypted RDO-copy path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ArtifactVariantId(String);

impl ArtifactVariantId {
    pub fn parse(value: impl Into<String>) -> Result<Self, SerializationError> {
        let value = value.into();
        validate_artifact_variant_id(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for ArtifactVariantId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ArtifactVariantId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ArtifactVariantId {
    type Err = SerializationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl TryFrom<String> for ArtifactVariantId {
    type Error = SerializationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl TryFrom<&str> for ArtifactVariantId {
    type Error = SerializationError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl<'de> Deserialize<'de> for ArtifactVariantId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(de::Error::custom)
    }
}

/// Exact artifact/path identity selected by a reviewed serialization contract.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SerializationArtifactIdentity {
    target: SerializationArtifactTarget,
    variant: ArtifactVariantId,
}

impl SerializationArtifactIdentity {
    pub const fn new(target: SerializationArtifactTarget, variant: ArtifactVariantId) -> Self {
        Self { target, variant }
    }

    pub const fn target(&self) -> SerializationArtifactTarget {
        self.target
    }

    pub fn variant(&self) -> &ArtifactVariantId {
        &self.variant
    }
}

/// Explicit handling for a semantically absent value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AbsentValuePolicy {
    Reject,
    OmitOccurrence,
}

/// Explicit handling for a semantic blank, kept separate from absence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlankValuePolicy {
    Reject,
    EmitEmptyBody,
    OmitOccurrence,
}

/// Exact spelling of the two serialized boolean values.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ExactBooleanFormat {
    true_text: String,
    false_text: String,
}

impl ExactBooleanFormat {
    pub fn try_new(
        true_text: impl Into<String>,
        false_text: impl Into<String>,
    ) -> Result<Self, SerializationError> {
        let true_text = true_text.into();
        let false_text = false_text.into();
        if true_text == false_text {
            return Err(SerializationError::IndistinguishableBooleanStrings);
        }
        Ok(Self {
            true_text,
            false_text,
        })
    }

    pub fn true_text(&self) -> &str {
        &self.true_text
    }

    pub fn false_text(&self) -> &str {
        &self.false_text
    }

    fn format(&self, value: bool) -> String {
        if value {
            self.true_text.clone()
        } else {
            self.false_text.clone()
        }
    }
}

impl<'de> Deserialize<'de> for ExactBooleanFormat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            true_text: String,
            false_text: String,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.true_text, wire.false_text).map_err(de::Error::custom)
    }
}

/// Exact base-10 decimal rendering bounds.
///
/// Formatting uses an ASCII period, never grouping or exponent notation, and
/// never rounds. Existing fractional digits are preserved; zeros are appended
/// to reach the minimum, while a value above the maximum is rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct ExactDecimalFormat {
    minimum_fraction_digits: u32,
    maximum_fraction_digits: u32,
}

impl ExactDecimalFormat {
    pub fn try_new(
        minimum_fraction_digits: u32,
        maximum_fraction_digits: u32,
    ) -> Result<Self, SerializationError> {
        if minimum_fraction_digits > maximum_fraction_digits
            || maximum_fraction_digits > ExactDecimal::MAX_SCALE
        {
            return Err(SerializationError::InvalidDecimalScaleRange {
                minimum: minimum_fraction_digits,
                maximum: maximum_fraction_digits,
                supported_maximum: ExactDecimal::MAX_SCALE,
            });
        }
        Ok(Self {
            minimum_fraction_digits,
            maximum_fraction_digits,
        })
    }

    pub fn fixed_scale(fraction_digits: u32) -> Result<Self, SerializationError> {
        Self::try_new(fraction_digits, fraction_digits)
    }

    pub const fn minimum_fraction_digits(self) -> u32 {
        self.minimum_fraction_digits
    }

    pub const fn maximum_fraction_digits(self) -> u32 {
        self.maximum_fraction_digits
    }

    fn format(self, value: ExactDecimal) -> Result<String, SerializationError> {
        let actual_scale = value.scale();
        if actual_scale > self.maximum_fraction_digits {
            return Err(SerializationError::DecimalScaleExceedsMaximum {
                actual: actual_scale,
                maximum: self.maximum_fraction_digits,
            });
        }

        let mut rendered = value.to_string();
        if actual_scale < self.minimum_fraction_digits {
            if actual_scale == 0 {
                rendered.push('.');
            }
            for _ in actual_scale..self.minimum_fraction_digits {
                rendered.push('0');
            }
        }
        Ok(rendered)
    }
}

impl<'de> Deserialize<'de> for ExactDecimalFormat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            minimum_fraction_digits: u32,
            maximum_fraction_digits: u32,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.minimum_fraction_digits, wire.maximum_fraction_digits)
            .map_err(de::Error::custom)
    }
}

/// Closed set of exact, four-digit-year date layouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExactDatePattern {
    YyyyMmDdHyphen,
    YyyyMmDdSlash,
    MmDdYyyySlash,
    DdMmYyyySlash,
    YyyyMmDdCompact,
}

impl ExactDatePattern {
    fn format(self, value: crate::CanonicalDate) -> Result<String, SerializationError> {
        let year = value.year();
        if year > 9_999 {
            return Err(SerializationError::DateYearOutsideFourDigits { year });
        }
        let month = value.month();
        let day = value.day();
        Ok(match self {
            Self::YyyyMmDdHyphen => format!("{year:04}-{month:02}-{day:02}"),
            Self::YyyyMmDdSlash => format!("{year:04}/{month:02}/{day:02}"),
            Self::MmDdYyyySlash => format!("{month:02}/{day:02}/{year:04}"),
            Self::DdMmYyyySlash => format!("{day:02}/{month:02}/{year:04}"),
            Self::YyyyMmDdCompact => format!("{year:04}{month:02}{day:02}"),
        })
    }
}

/// Formatting applied after absence/blank handling and before body encoding.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "settings", rename_all = "kebab-case")]
pub enum PresentValueFormat {
    Text,
    Boolean(ExactBooleanFormat),
    Base10Integer,
    Decimal(ExactDecimalFormat),
    Date(ExactDatePattern),
}

impl PresentValueFormat {
    pub const fn expected_kind(&self) -> CanonicalValueKind {
        match self {
            Self::Text => CanonicalValueKind::Text,
            Self::Boolean(_) => CanonicalValueKind::Boolean,
            Self::Base10Integer => CanonicalValueKind::Integer,
            Self::Decimal(_) => CanonicalValueKind::Decimal,
            Self::Date(_) => CanonicalValueKind::Date,
        }
    }
}

/// Complete semantic formatting policy for one serialized occurrence.
///
/// There is deliberately no `Default` implementation. Absence, blank values,
/// and the present value's spelling must all be selected explicitly.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticFormatPolicy {
    absent: AbsentValuePolicy,
    blank: BlankValuePolicy,
    present: PresentValueFormat,
}

impl SemanticFormatPolicy {
    pub const fn new(
        absent: AbsentValuePolicy,
        blank: BlankValuePolicy,
        present: PresentValueFormat,
    ) -> Self {
        Self {
            absent,
            blank,
            present,
        }
    }

    pub const fn absent_policy(&self) -> AbsentValuePolicy {
        self.absent
    }

    pub const fn blank_policy(&self) -> BlankValuePolicy {
        self.blank
    }

    pub const fn present_format(&self) -> &PresentValueFormat {
        &self.present
    }

    pub fn format(
        &self,
        value: &CanonicalValue,
    ) -> Result<FormattedSemanticValue, SerializationError> {
        match value {
            CanonicalValue::Absent => {
                return match self.absent {
                    AbsentValuePolicy::Reject => Err(SerializationError::AbsentValueRejected),
                    AbsentValuePolicy::OmitOccurrence => Ok(FormattedSemanticValue::Omitted),
                };
            }
            CanonicalValue::Blank => {
                return match self.blank {
                    BlankValuePolicy::Reject => Err(SerializationError::BlankValueRejected),
                    BlankValuePolicy::EmitEmptyBody => {
                        Ok(FormattedSemanticValue::Body(String::new()))
                    }
                    BlankValuePolicy::OmitOccurrence => Ok(FormattedSemanticValue::Omitted),
                };
            }
            _ => {}
        }

        let actual = CanonicalValueKind::of(value);
        let body = match (&self.present, value) {
            (PresentValueFormat::Text, CanonicalValue::Text(value)) => value.clone(),
            (PresentValueFormat::Boolean(format), CanonicalValue::Boolean(value)) => {
                format.format(*value)
            }
            (PresentValueFormat::Base10Integer, CanonicalValue::Integer(value)) => {
                value.to_string()
            }
            (PresentValueFormat::Decimal(format), CanonicalValue::Decimal(value)) => {
                return format.format(*value).map(FormattedSemanticValue::Body);
            }
            (PresentValueFormat::Date(pattern), CanonicalValue::Date(value)) => {
                return pattern.format(*value).map(FormattedSemanticValue::Body);
            }
            _ => {
                return Err(SerializationError::CanonicalTypeMismatch {
                    expected: self.present.expected_kind(),
                    actual,
                });
            }
        };
        Ok(FormattedSemanticValue::Body(body))
    }
}

/// Apply one already-selected generated serialization format to a canonical
/// value.
///
/// This pure formatter grants no artifact or filing authority. The sealed
/// materializer still owns contract selection; downstream proof code may use
/// this function only to independently check its semantic body.
pub fn format_serialization_value(
    value: &CanonicalValue,
    format: SerializationSemanticFormat,
) -> Result<FormattedSemanticValue, SerializationError> {
    match value {
        CanonicalValue::Absent => {
            return match format.absent {
                AbsentValuePolicy::Reject => Err(SerializationError::AbsentValueRejected),
                AbsentValuePolicy::OmitOccurrence => Ok(FormattedSemanticValue::Omitted),
            };
        }
        CanonicalValue::Blank => {
            return match format.blank {
                BlankValuePolicy::Reject => Err(SerializationError::BlankValueRejected),
                BlankValuePolicy::EmitEmptyBody => Ok(FormattedSemanticValue::Body(String::new())),
                BlankValuePolicy::OmitOccurrence => Ok(FormattedSemanticValue::Omitted),
            };
        }
        _ => {}
    }

    let expected = serialization_present_value_kind(format.present);
    let body = match (format.present, value) {
        (SerializationPresentFormat::Text, CanonicalValue::Text(value)) => value.clone(),
        (
            SerializationPresentFormat::Boolean {
                true_text,
                false_text,
            },
            CanonicalValue::Boolean(value),
        ) => {
            if true_text == false_text {
                return Err(SerializationError::IndistinguishableBooleanStrings);
            }
            (if *value { true_text } else { false_text }).to_owned()
        }
        (SerializationPresentFormat::Base10Integer, CanonicalValue::Integer(value)) => {
            value.to_string()
        }
        (SerializationPresentFormat::Decimal(settings), CanonicalValue::Decimal(value)) => {
            format_serialization_decimal(*value, settings)?
        }
        (SerializationPresentFormat::Date(pattern), CanonicalValue::Date(value)) => {
            pattern.format(*value)?
        }
        (_, actual) => {
            return Err(SerializationError::CanonicalTypeMismatch {
                expected,
                actual: CanonicalValueKind::of(actual),
            });
        }
    };
    Ok(FormattedSemanticValue::Body(body))
}

fn serialization_present_value_kind(format: SerializationPresentFormat) -> CanonicalValueKind {
    match format {
        SerializationPresentFormat::Text => CanonicalValueKind::Text,
        SerializationPresentFormat::Boolean { .. } => CanonicalValueKind::Boolean,
        SerializationPresentFormat::Base10Integer => CanonicalValueKind::Integer,
        SerializationPresentFormat::Decimal(_) => CanonicalValueKind::Decimal,
        SerializationPresentFormat::Date(_) => CanonicalValueKind::Date,
    }
}

fn format_serialization_decimal(
    value: ExactDecimal,
    format: SerializationDecimalFormat,
) -> Result<String, SerializationError> {
    if format.scale > ExactDecimal::MAX_SCALE {
        return Err(SerializationError::InvalidSerializationDecimalScale {
            scale: format.scale,
            supported_maximum: ExactDecimal::MAX_SCALE,
        });
    }
    if matches!(
        (format.grouping, format.decimal_separator),
        (
            SerializationGrouping::Comma,
            SerializationDecimalSeparator::Comma
        ) | (
            SerializationGrouping::Period,
            SerializationDecimalSeparator::Period
        )
    ) {
        return Err(SerializationError::ConflictingDecimalSeparators);
    }

    let rounded =
        round_exact_decimal_to_scale(value, format.scale, format.rounding).map_err(|error| {
            match error {
                ExactRoundingError::Inexact => SerializationError::InexactDecimalRounding {
                    actual_scale: value.scale(),
                    target_scale: format.scale,
                },
                ExactRoundingError::ScaleTooLarge { scale, maximum } => {
                    SerializationError::InvalidSerializationDecimalScale {
                        scale,
                        supported_maximum: maximum,
                    }
                }
                ExactRoundingError::Overflow => SerializationError::DecimalRoundingOverflow,
            }
        })?;

    let negative = rounded.coefficient().is_negative();
    let mut digits = rounded.coefficient().unsigned_abs().to_string();
    let additional_zeros = format.scale - rounded.scale();
    digits.extend(std::iter::repeat_n('0', additional_zeros as usize));

    let scale = format.scale as usize;
    let (integer_digits, fractional_digits) = if scale == 0 {
        (digits.as_str(), None)
    } else if digits.len() <= scale {
        let mut padded = String::with_capacity(scale + 1);
        padded.extend(std::iter::repeat_n('0', scale + 1 - digits.len()));
        padded.push_str(&digits);
        digits = padded;
        let split = digits.len() - scale;
        (&digits[..split], Some(&digits[split..]))
    } else {
        let split = digits.len() - scale;
        (&digits[..split], Some(&digits[split..]))
    };

    let mut rendered = group_integer_digits(integer_digits, format.grouping);
    if let Some(fractional_digits) = fractional_digits {
        rendered.push(match format.decimal_separator {
            SerializationDecimalSeparator::Period => '.',
            SerializationDecimalSeparator::Comma => ',',
        });
        rendered.push_str(fractional_digits);
    }

    if negative {
        rendered = match format.negative {
            SerializationNegativeRepresentation::LeadingMinus => format!("-{rendered}"),
            SerializationNegativeRepresentation::TrailingMinus => format!("{rendered}-"),
            SerializationNegativeRepresentation::Parentheses => format!("({rendered})"),
        };
    }
    Ok(rendered)
}

fn group_integer_digits(digits: &str, grouping: SerializationGrouping) -> String {
    let separator = match grouping {
        SerializationGrouping::None => return digits.to_owned(),
        SerializationGrouping::Comma => ',',
        SerializationGrouping::Period => '.',
        SerializationGrouping::Space => ' ',
    };
    if digits.len() <= 3 {
        return digits.to_owned();
    }

    let first_group = match digits.len() % 3 {
        0 => 3,
        remainder => remainder,
    };
    let mut grouped = String::with_capacity(digits.len() + (digits.len() - 1) / 3);
    grouped.push_str(&digits[..first_group]);
    for chunk in digits.as_bytes()[first_group..].chunks(3) {
        grouped.push(separator);
        // Decimal digits are ASCII by construction.
        grouped.push_str(std::str::from_utf8(chunk).expect("decimal digits are valid UTF-8"));
    }
    grouped
}

/// Result of semantic formatting before an occurrence body codec is applied.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FormattedSemanticValue {
    Omitted,
    Body(String),
}

impl FormattedSemanticValue {
    pub const fn is_omitted(&self) -> bool {
        matches!(self, Self::Omitted)
    }

    pub fn body(&self) -> Option<&str> {
        match self {
            Self::Omitted => None,
            Self::Body(body) => Some(body),
        }
    }

    pub fn into_body(self) -> Option<String> {
        match self {
            Self::Omitted => None,
            Self::Body(body) => Some(body),
        }
    }
}

/// Exact body codec for one serialized occurrence.
///
/// No variant is a default. In particular, the UTF-8 percent profile is
/// available only for a contract that names it explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BodyCodec {
    RawLiteral,
    #[serde(rename = "legacy-javascript-escape")]
    LegacyJavaScriptEscape,
    Utf8PercentRfc3986Unreserved,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub(crate) enum BodyEncodingBoundary<'a> {
    PseudoXmlKey(&'a XmlKey),
    MetadataTag(&'a str),
}

impl BodyCodec {
    pub fn encode(self, body: &str, serialized_key: &XmlKey) -> Result<String, SerializationError> {
        self.encode_for_boundary(body, BodyEncodingBoundary::PseudoXmlKey(serialized_key))
    }

    /// Encode a metadata-element body at its distinct raw-literal boundary.
    ///
    /// The exact tag is part of the reviewed contract. This utility performs
    /// no artifact selection and grants no filing authority.
    pub fn encode_metadata(
        self,
        body: &str,
        exact_tag: &str,
    ) -> Result<String, SerializationError> {
        self.encode_for_boundary(body, BodyEncodingBoundary::MetadataTag(exact_tag))
    }

    pub(crate) fn encode_for_boundary(
        self,
        body: &str,
        boundary: BodyEncodingBoundary<'_>,
    ) -> Result<String, SerializationError> {
        match self {
            Self::RawLiteral => encode_raw_literal(body, boundary),
            Self::LegacyJavaScriptEscape => Ok(encode_legacy_javascript_escape(body)),
            Self::Utf8PercentRfc3986Unreserved => Ok(encode_utf8_percent_rfc3986_unreserved(body)),
        }
    }
}

/// Format and encode one value while preserving omission as `None`.
pub fn format_and_encode(
    value: &CanonicalValue,
    format: &SemanticFormatPolicy,
    codec: BodyCodec,
    serialized_key: &XmlKey,
) -> Result<Option<String>, SerializationError> {
    match format.format(value)? {
        FormattedSemanticValue::Omitted => Ok(None),
        FormattedSemanticValue::Body(body) => codec.encode(&body, serialized_key).map(Some),
    }
}

/// Canonical value category used in strict mismatch errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CanonicalValueKind {
    Absent,
    Blank,
    Text,
    Boolean,
    Integer,
    Decimal,
    Date,
}

impl CanonicalValueKind {
    pub const fn of(value: &CanonicalValue) -> Self {
        match value {
            CanonicalValue::Absent => Self::Absent,
            CanonicalValue::Blank => Self::Blank,
            CanonicalValue::Text(_) => Self::Text,
            CanonicalValue::Boolean(_) => Self::Boolean,
            CanonicalValue::Integer(_) => Self::Integer,
            CanonicalValue::Decimal(_) => Self::Decimal,
            CanonicalValue::Date(_) => Self::Date,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Blank => "blank",
            Self::Text => "text",
            Self::Boolean => "boolean",
            Self::Integer => "integer",
            Self::Decimal => "decimal",
            Self::Date => "date",
        }
    }
}

/// Strict failure from artifact identity, semantic formatting, or body encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SerializationError {
    EmptyArtifactVariantId,
    ArtifactVariantIdTooLong {
        maximum: usize,
        actual: usize,
    },
    InvalidArtifactVariantBoundary,
    InvalidArtifactVariantCharacter {
        index: usize,
        character: char,
    },
    IndistinguishableBooleanStrings,
    InvalidDecimalScaleRange {
        minimum: u32,
        maximum: u32,
        supported_maximum: u32,
    },
    InvalidSerializationDecimalScale {
        scale: u32,
        supported_maximum: u32,
    },
    InexactDecimalRounding {
        actual_scale: u32,
        target_scale: u32,
    },
    DecimalRoundingOverflow,
    ConflictingDecimalSeparators,
    DecimalScaleExceedsMaximum {
        actual: u32,
        maximum: u32,
    },
    DateYearOutsideFourDigits {
        year: u16,
    },
    AbsentValueRejected,
    BlankValueRejected,
    CanonicalTypeMismatch {
        expected: CanonicalValueKind,
        actual: CanonicalValueKind,
    },
    RawBodyContainsMarkupDelimiter {
        index: usize,
    },
    RawBodyContainsRepeatedFieldDelimiter {
        index: usize,
        delimiter: String,
    },
}

impl fmt::Display for SerializationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyArtifactVariantId => {
                formatter.write_str("artifact variant ID must not be empty")
            }
            Self::ArtifactVariantIdTooLong { maximum, actual } => write!(
                formatter,
                "artifact variant ID is {actual} bytes; maximum is {maximum}"
            ),
            Self::InvalidArtifactVariantBoundary => formatter.write_str(
                "artifact variant ID must start and end with a lowercase ASCII letter or digit",
            ),
            Self::InvalidArtifactVariantCharacter { index, character } => write!(
                formatter,
                "artifact variant ID contains invalid character {character:?} at byte {index}"
            ),
            Self::IndistinguishableBooleanStrings => {
                formatter.write_str("serialized true and false strings must differ")
            }
            Self::InvalidDecimalScaleRange {
                minimum,
                maximum,
                supported_maximum,
            } => write!(
                formatter,
                "invalid decimal fraction range {minimum}..={maximum}; maximum supported scale is {supported_maximum}"
            ),
            Self::InvalidSerializationDecimalScale {
                scale,
                supported_maximum,
            } => write!(
                formatter,
                "serialization decimal scale {scale} exceeds maximum supported scale {supported_maximum}"
            ),
            Self::InexactDecimalRounding {
                actual_scale,
                target_scale,
            } => write!(
                formatter,
                "serialization decimal scale {actual_scale} cannot be reduced to {target_scale} without rounding"
            ),
            Self::DecimalRoundingOverflow => {
                formatter.write_str("serialization decimal rounding overflowed")
            }
            Self::ConflictingDecimalSeparators => formatter.write_str(
                "serialization decimal grouping and decimal separator must use different characters",
            ),
            Self::DecimalScaleExceedsMaximum { actual, maximum } => write!(
                formatter,
                "decimal scale {actual} exceeds configured maximum {maximum}; rounding is not permitted"
            ),
            Self::DateYearOutsideFourDigits { year } => write!(
                formatter,
                "date year {year} cannot be rendered by a four-digit-year pattern"
            ),
            Self::AbsentValueRejected => formatter
                .write_str("semantic value is absent but the format policy requires a value"),
            Self::BlankValueRejected => {
                formatter.write_str("semantic value is blank but the format policy rejects blanks")
            }
            Self::CanonicalTypeMismatch { expected, actual } => write!(
                formatter,
                "canonical value type mismatch: expected {}, got {}",
                expected.as_str(),
                actual.as_str()
            ),
            Self::RawBodyContainsMarkupDelimiter { index } => write!(
                formatter,
                "raw body contains '<' at byte {index}, which can terminate or nest a pseudo-XML div"
            ),
            Self::RawBodyContainsRepeatedFieldDelimiter { index, delimiter } => write!(
                formatter,
                "raw body contains repeated field delimiter {delimiter:?} at byte {index}"
            ),
        }
    }
}

impl Error for SerializationError {}

fn validate_artifact_variant_id(value: &str) -> Result<(), SerializationError> {
    if value.is_empty() {
        return Err(SerializationError::EmptyArtifactVariantId);
    }
    if value.len() > MAX_ARTIFACT_VARIANT_ID_BYTES {
        return Err(SerializationError::ArtifactVariantIdTooLong {
            maximum: MAX_ARTIFACT_VARIANT_ID_BYTES,
            actual: value.len(),
        });
    }
    if !value
        .as_bytes()
        .first()
        .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        || !value
            .as_bytes()
            .last()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        return Err(SerializationError::InvalidArtifactVariantBoundary);
    }
    for (index, character) in value.char_indices() {
        if !(character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || matches!(character, '-' | '_' | '.'))
        {
            return Err(SerializationError::InvalidArtifactVariantCharacter { index, character });
        }
    }
    Ok(())
}

fn encode_raw_literal(
    body: &str,
    boundary: BodyEncodingBoundary<'_>,
) -> Result<String, SerializationError> {
    if let Some(index) = body.find('<') {
        return Err(SerializationError::RawBodyContainsMarkupDelimiter { index });
    }
    match boundary {
        BodyEncodingBoundary::PseudoXmlKey(serialized_key) => {
            let delimiter = format!("{}=", serialized_key.as_str());
            if let Some(index) = body.find(&delimiter) {
                return Err(SerializationError::RawBodyContainsRepeatedFieldDelimiter {
                    index,
                    delimiter,
                });
            }
        }
        BodyEncodingBoundary::MetadataTag(tag) => {
            debug_assert!(!tag.is_empty(), "reviewed metadata tags are non-empty");
        }
    }
    Ok(body.to_string())
}

fn encode_legacy_javascript_escape(source: &str) -> String {
    let mut encoded = String::with_capacity(source.len());
    for unit in source.encode_utf16() {
        if is_legacy_javascript_escape_safe(unit) {
            encoded.push(char::from(unit as u8));
        } else if unit <= 0x00ff {
            encoded.push('%');
            push_hex_byte(&mut encoded, unit as u8);
        } else {
            encoded.push_str("%u");
            push_hex_u16(&mut encoded, unit);
        }
    }
    encoded
}

fn is_legacy_javascript_escape_safe(unit: u16) -> bool {
    unit <= 0x7f
        && ((unit as u8).is_ascii_alphanumeric()
            || matches!(unit as u8, b'@' | b'*' | b'_' | b'+' | b'-' | b'.' | b'/'))
}

fn encode_utf8_percent_rfc3986_unreserved(source: &str) -> String {
    let mut encoded = String::with_capacity(source.len());
    for byte in source.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            push_hex_byte(&mut encoded, byte);
        }
    }
    encoded
}

fn push_hex_byte(output: &mut String, byte: u8) {
    output.push(char::from(UPPER_HEX[usize::from(byte >> 4)]));
    output.push(char::from(UPPER_HEX[usize::from(byte & 0x0f)]));
}

fn push_hex_u16(output: &mut String, unit: u16) {
    output.push(char::from(UPPER_HEX[usize::from((unit >> 12) & 0x0f)]));
    output.push(char::from(UPPER_HEX[usize::from((unit >> 8) & 0x0f)]));
    output.push(char::from(UPPER_HEX[usize::from((unit >> 4) & 0x0f)]));
    output.push(char::from(UPPER_HEX[usize::from(unit & 0x0f)]));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CanonicalDate, static_ir::RoundingMode};

    fn xml_key() -> XmlKey {
        XmlKey::parse("frm2550qv2024:taxpayerName").unwrap()
    }

    fn required_policy(present: PresentValueFormat) -> SemanticFormatPolicy {
        SemanticFormatPolicy::new(AbsentValuePolicy::Reject, BlankValuePolicy::Reject, present)
    }

    fn contract_format(present: SerializationPresentFormat) -> SerializationSemanticFormat {
        SerializationSemanticFormat {
            absent: AbsentValuePolicy::Reject,
            blank: BlankValuePolicy::Reject,
            present,
        }
    }

    fn decimal_settings(scale: u32, rounding: RoundingMode) -> SerializationDecimalFormat {
        SerializationDecimalFormat {
            scale,
            rounding,
            grouping: SerializationGrouping::None,
            decimal_separator: SerializationDecimalSeparator::Period,
            negative: SerializationNegativeRepresentation::LeadingMinus,
        }
    }

    fn render_contract_decimal(
        source: &str,
        settings: SerializationDecimalFormat,
    ) -> Result<String, SerializationError> {
        let value = CanonicalValue::Decimal(source.parse().unwrap());
        format_serialization_value(
            &value,
            contract_format(SerializationPresentFormat::Decimal(settings)),
        )
        .map(|formatted| formatted.into_body().expect("decimal is emitted"))
    }

    #[test]
    fn artifact_identity_requires_an_explicit_stable_variant() {
        let variant = ArtifactVariantId::parse("iaf-rdo-copy").unwrap();
        let identity = SerializationArtifactIdentity::new(
            SerializationArtifactTarget::EncryptedFinalCopy,
            variant.clone(),
        );
        assert_eq!(
            identity.target(),
            SerializationArtifactTarget::EncryptedFinalCopy
        );
        assert_eq!(identity.variant(), &variant);

        assert_eq!(
            ArtifactVariantId::parse(""),
            Err(SerializationError::EmptyArtifactVariantId)
        );
        assert!(matches!(
            ArtifactVariantId::parse("Implicit Default"),
            Err(SerializationError::InvalidArtifactVariantBoundary)
                | Err(SerializationError::InvalidArtifactVariantCharacter { .. })
        ));
    }

    #[test]
    fn historical_import_compatibility_has_an_exact_closed_wire_name() {
        let target = SerializationArtifactTarget::HistoricalImportCompatibility;
        assert_eq!(
            serde_json::to_string(&target).unwrap(),
            r#""historical-import-compatibility""#
        );
        assert_eq!(
            serde_json::from_str::<SerializationArtifactTarget>(
                r#""historical-import-compatibility""#
            )
            .unwrap(),
            target
        );
        assert!(
            serde_json::from_str::<SerializationArtifactTarget>(r#""historical-import""#).is_err()
        );
    }

    #[test]
    fn text_blank_and_absent_are_distinct_and_explicit() {
        let policy = SemanticFormatPolicy::new(
            AbsentValuePolicy::OmitOccurrence,
            BlankValuePolicy::EmitEmptyBody,
            PresentValueFormat::Text,
        );
        assert_eq!(
            policy.format(&CanonicalValue::Absent).unwrap(),
            FormattedSemanticValue::Omitted
        );
        assert_eq!(
            policy.format(&CanonicalValue::Blank).unwrap(),
            FormattedSemanticValue::Body(String::new())
        );
        assert_eq!(
            policy
                .format(&CanonicalValue::Text("literal".to_string()))
                .unwrap(),
            FormattedSemanticValue::Body("literal".to_string())
        );

        let rejected = required_policy(PresentValueFormat::Text);
        assert_eq!(
            rejected.format(&CanonicalValue::Absent),
            Err(SerializationError::AbsentValueRejected)
        );
        assert_eq!(
            rejected.format(&CanonicalValue::Blank),
            Err(SerializationError::BlankValueRejected)
        );
    }

    #[test]
    fn booleans_use_exact_reviewed_strings() {
        let format = ExactBooleanFormat::try_new("true", "false").unwrap();
        let policy = required_policy(PresentValueFormat::Boolean(format));
        assert_eq!(
            policy.format(&CanonicalValue::Boolean(true)).unwrap(),
            FormattedSemanticValue::Body("true".to_string())
        );
        assert_eq!(
            policy.format(&CanonicalValue::Boolean(false)).unwrap(),
            FormattedSemanticValue::Body("false".to_string())
        );
        assert_eq!(
            ExactBooleanFormat::try_new("1", "1"),
            Err(SerializationError::IndistinguishableBooleanStrings)
        );
    }

    #[test]
    fn canonical_type_mismatches_fail_closed() {
        let policy = required_policy(PresentValueFormat::Base10Integer);
        assert_eq!(
            policy.format(&CanonicalValue::Text("12".to_string())),
            Err(SerializationError::CanonicalTypeMismatch {
                expected: CanonicalValueKind::Integer,
                actual: CanonicalValueKind::Text,
            })
        );
    }

    #[test]
    fn integers_use_plain_base_ten_without_grouping_or_plus_sign() {
        let policy = required_policy(PresentValueFormat::Base10Integer);
        for (value, expected) in [
            (0_i128, "0".to_string()),
            (12_345_i128, "12345".to_string()),
            (-12_345_i128, "-12345".to_string()),
            (i128::MIN, i128::MIN.to_string()),
            (i128::MAX, i128::MAX.to_string()),
        ] {
            assert_eq!(
                policy.format(&CanonicalValue::Integer(value)).unwrap(),
                FormattedSemanticValue::Body(expected)
            );
        }
    }

    #[test]
    fn decimals_preserve_exact_digits_pad_only_and_never_round() {
        let fixed_two = ExactDecimalFormat::fixed_scale(2).unwrap();
        let policy = required_policy(PresentValueFormat::Decimal(fixed_two));
        for (source, expected) in [
            ("0", "0.00"),
            ("1", "1.00"),
            ("1.2", "1.20"),
            ("-12.34", "-12.34"),
        ] {
            let decimal = source.parse::<ExactDecimal>().unwrap();
            assert_eq!(
                policy.format(&CanonicalValue::Decimal(decimal)).unwrap(),
                FormattedSemanticValue::Body(expected.to_string())
            );
        }

        let too_precise = "1.234".parse::<ExactDecimal>().unwrap();
        assert_eq!(
            policy.format(&CanonicalValue::Decimal(too_precise)),
            Err(SerializationError::DecimalScaleExceedsMaximum {
                actual: 3,
                maximum: 2,
            })
        );
        assert!(matches!(
            ExactDecimalFormat::try_new(3, 2),
            Err(SerializationError::InvalidDecimalScaleRange { .. })
        ));
    }

    #[test]
    fn bounded_decimal_scale_preserves_existing_exact_scale() {
        let format = ExactDecimalFormat::try_new(2, 4).unwrap();
        let policy = required_policy(PresentValueFormat::Decimal(format));
        for (source, expected) in [
            ("1", "1.00"),
            ("1.2", "1.20"),
            ("1.234", "1.234"),
            ("1.2345", "1.2345"),
        ] {
            let decimal = source.parse::<ExactDecimal>().unwrap();
            assert_eq!(
                policy.format(&CanonicalValue::Decimal(decimal)).unwrap(),
                FormattedSemanticValue::Body(expected.to_string())
            );
        }
    }

    #[test]
    fn contract_formatter_keeps_absent_blank_and_present_text_distinct() {
        let format = SerializationSemanticFormat {
            absent: AbsentValuePolicy::OmitOccurrence,
            blank: BlankValuePolicy::EmitEmptyBody,
            present: SerializationPresentFormat::Text,
        };
        assert_eq!(
            format_serialization_value(&CanonicalValue::Absent, format).unwrap(),
            FormattedSemanticValue::Omitted
        );
        assert_eq!(
            format_serialization_value(&CanonicalValue::Blank, format).unwrap(),
            FormattedSemanticValue::Body(String::new())
        );
        assert_eq!(
            format_serialization_value(&CanonicalValue::Text("value".into()), format).unwrap(),
            FormattedSemanticValue::Body("value".into())
        );

        let rejecting = contract_format(SerializationPresentFormat::Text);
        assert_eq!(
            format_serialization_value(&CanonicalValue::Absent, rejecting),
            Err(SerializationError::AbsentValueRejected)
        );
        assert_eq!(
            format_serialization_value(&CanonicalValue::Blank, rejecting),
            Err(SerializationError::BlankValueRejected)
        );
    }

    #[test]
    fn contract_decimal_formatter_supports_exact_scales_zero_through_twenty_eight() {
        for scale in 0..=ExactDecimal::MAX_SCALE {
            let expected = if scale == 0 {
                "0".to_string()
            } else {
                format!("0.{}", "0".repeat(scale as usize))
            };
            assert_eq!(
                render_contract_decimal("0", decimal_settings(scale, RoundingMode::None)).unwrap(),
                expected,
                "scale {scale}"
            );
        }
        assert_eq!(
            render_contract_decimal("123.9", decimal_settings(0, RoundingMode::TowardZero))
                .unwrap(),
            "123"
        );
        assert_eq!(
            render_contract_decimal("1.2", decimal_settings(2, RoundingMode::None)).unwrap(),
            "1.20"
        );
        assert_eq!(
            render_contract_decimal("1", decimal_settings(29, RoundingMode::None)),
            Err(SerializationError::InvalidSerializationDecimalScale {
                scale: 29,
                supported_maximum: ExactDecimal::MAX_SCALE,
            })
        );
    }

    #[test]
    fn contract_decimal_rounding_modes_are_exact_for_positive_and_negative_ties() {
        for (mode, positive, negative) in [
            (RoundingMode::TowardZero, "1.2", "-1.2"),
            (RoundingMode::AwayFromZero, "1.3", "-1.3"),
            (RoundingMode::Floor, "1.2", "-1.3"),
            (RoundingMode::Ceiling, "1.3", "-1.2"),
            (RoundingMode::HalfUp, "1.3", "-1.3"),
            (RoundingMode::HalfEven, "1.2", "-1.2"),
        ] {
            assert_eq!(
                render_contract_decimal("1.25", decimal_settings(1, mode)).unwrap(),
                positive,
                "positive {mode:?}"
            );
            assert_eq!(
                render_contract_decimal("-1.25", decimal_settings(1, mode)).unwrap(),
                negative,
                "negative {mode:?}"
            );
        }

        assert_eq!(
            render_contract_decimal("1.35", decimal_settings(1, RoundingMode::HalfEven)).unwrap(),
            "1.4"
        );
        assert_eq!(
            render_contract_decimal("-1.35", decimal_settings(1, RoundingMode::HalfEven)).unwrap(),
            "-1.4"
        );
    }

    #[test]
    fn contract_decimal_none_rejects_only_when_nonzero_digits_would_be_discarded() {
        assert_eq!(
            render_contract_decimal("1.2", decimal_settings(2, RoundingMode::None)).unwrap(),
            "1.20"
        );
        assert_eq!(
            render_contract_decimal("1.234", decimal_settings(2, RoundingMode::None)),
            Err(SerializationError::InexactDecimalRounding {
                actual_scale: 3,
                target_scale: 2,
            })
        );
    }

    #[test]
    fn contract_decimal_grouping_separator_and_negative_spelling_are_explicit() {
        for (grouping, decimal_separator, expected) in [
            (
                SerializationGrouping::None,
                SerializationDecimalSeparator::Comma,
                "1234567,80",
            ),
            (
                SerializationGrouping::Comma,
                SerializationDecimalSeparator::Period,
                "1,234,567.80",
            ),
            (
                SerializationGrouping::Period,
                SerializationDecimalSeparator::Comma,
                "1.234.567,80",
            ),
            (
                SerializationGrouping::Space,
                SerializationDecimalSeparator::Period,
                "1 234 567.80",
            ),
        ] {
            let mut settings = decimal_settings(2, RoundingMode::None);
            settings.grouping = grouping;
            settings.decimal_separator = decimal_separator;
            assert_eq!(
                render_contract_decimal("1234567.8", settings).unwrap(),
                expected
            );
        }

        for (negative, expected) in [
            (
                SerializationNegativeRepresentation::LeadingMinus,
                "-1,234.50",
            ),
            (
                SerializationNegativeRepresentation::TrailingMinus,
                "1,234.50-",
            ),
            (
                SerializationNegativeRepresentation::Parentheses,
                "(1,234.50)",
            ),
        ] {
            let mut settings = decimal_settings(2, RoundingMode::None);
            settings.grouping = SerializationGrouping::Comma;
            settings.negative = negative;
            assert_eq!(
                render_contract_decimal("-1234.5", settings).unwrap(),
                expected
            );
        }

        let mut conflicting = decimal_settings(2, RoundingMode::None);
        conflicting.grouping = SerializationGrouping::Comma;
        conflicting.decimal_separator = SerializationDecimalSeparator::Comma;
        assert_eq!(
            render_contract_decimal("1.2", conflicting),
            Err(SerializationError::ConflictingDecimalSeparators)
        );
    }

    #[test]
    fn contract_formatter_covers_boolean_integer_date_and_type_mismatch() {
        let boolean = contract_format(SerializationPresentFormat::Boolean {
            true_text: "Y",
            false_text: "N",
        });
        assert_eq!(
            format_serialization_value(&CanonicalValue::Boolean(true), boolean).unwrap(),
            FormattedSemanticValue::Body("Y".into())
        );
        assert_eq!(
            format_serialization_value(&CanonicalValue::Boolean(false), boolean).unwrap(),
            FormattedSemanticValue::Body("N".into())
        );

        let integer = contract_format(SerializationPresentFormat::Base10Integer);
        assert_eq!(
            format_serialization_value(&CanonicalValue::Integer(i128::MIN), integer).unwrap(),
            FormattedSemanticValue::Body(i128::MIN.to_string())
        );

        let date = contract_format(SerializationPresentFormat::Date(
            ExactDatePattern::YyyyMmDdCompact,
        ));
        assert_eq!(
            format_serialization_value(
                &CanonicalValue::Date(CanonicalDate::try_new(2024, 7, 4).unwrap()),
                date,
            )
            .unwrap(),
            FormattedSemanticValue::Body("20240704".into())
        );

        assert_eq!(
            format_serialization_value(&CanonicalValue::Text("12".into()), integer),
            Err(SerializationError::CanonicalTypeMismatch {
                expected: CanonicalValueKind::Integer,
                actual: CanonicalValueKind::Text,
            })
        );
    }

    #[test]
    fn dates_use_only_the_selected_exact_pattern() {
        let date = CanonicalValue::Date(CanonicalDate::try_new(2024, 7, 4).unwrap());
        for (pattern, expected) in [
            (ExactDatePattern::YyyyMmDdHyphen, "2024-07-04"),
            (ExactDatePattern::YyyyMmDdSlash, "2024/07/04"),
            (ExactDatePattern::MmDdYyyySlash, "07/04/2024"),
            (ExactDatePattern::DdMmYyyySlash, "04/07/2024"),
            (ExactDatePattern::YyyyMmDdCompact, "20240704"),
        ] {
            let policy = required_policy(PresentValueFormat::Date(pattern));
            assert_eq!(
                policy.format(&date).unwrap(),
                FormattedSemanticValue::Body(expected.to_string())
            );
        }

        let five_digit_year = CanonicalValue::Date(CanonicalDate::try_new(10_000, 1, 1).unwrap());
        let policy = required_policy(PresentValueFormat::Date(ExactDatePattern::YyyyMmDdSlash));
        assert_eq!(
            policy.format(&five_digit_year),
            Err(SerializationError::DateYearOutsideFourDigits { year: 10_000 })
        );
    }

    #[test]
    fn raw_literal_preserves_percent_safe_and_reserved_characters() {
        let source = "literal%20%GG @*_+-./'=&";
        assert_eq!(
            BodyCodec::RawLiteral.encode(source, &xml_key()).unwrap(),
            source
        );
    }

    #[test]
    fn raw_literal_rejects_markup_and_repeated_field_delimiters() {
        assert_eq!(
            BodyCodec::RawLiteral.encode("before<after", &xml_key()),
            Err(SerializationError::RawBodyContainsMarkupDelimiter { index: 6 })
        );
        let delimiter = format!("{}=", xml_key().as_str());
        assert_eq!(
            BodyCodec::RawLiteral.encode(&format!("before{delimiter}after"), &xml_key()),
            Err(SerializationError::RawBodyContainsRepeatedFieldDelimiter {
                index: 6,
                delimiter,
            })
        );
    }

    #[test]
    fn raw_literal_boundary_checks_repeated_keys_only_for_pseudo_xml() {
        let key = xml_key();
        let body = format!("before{}=after", key.as_str());
        assert!(matches!(
            BodyCodec::RawLiteral
                .encode_for_boundary(&body, BodyEncodingBoundary::PseudoXmlKey(&key)),
            Err(SerializationError::RawBodyContainsRepeatedFieldDelimiter { .. })
        ));
        assert_eq!(
            BodyCodec::RawLiteral
                .encode_for_boundary(&body, BodyEncodingBoundary::MetadataTag("dateFiled"))
                .unwrap(),
            body
        );

        for boundary in [
            BodyEncodingBoundary::PseudoXmlKey(&key),
            BodyEncodingBoundary::MetadataTag("dateFiled"),
        ] {
            assert_eq!(
                BodyCodec::RawLiteral.encode_for_boundary("before<after", boundary),
                Err(SerializationError::RawBodyContainsMarkupDelimiter { index: 6 })
            );
        }
    }

    #[test]
    fn encoded_body_codecs_are_boundary_independent() {
        let key = xml_key();
        for codec in [
            BodyCodec::LegacyJavaScriptEscape,
            BodyCodec::Utf8PercentRfc3986Unreserved,
        ] {
            assert_eq!(
                codec
                    .encode_for_boundary(
                        "50% / ä½  ðŸ˜€",
                        BodyEncodingBoundary::PseudoXmlKey(&key),
                    )
                    .unwrap(),
                codec
                    .encode_for_boundary(
                        "50% / ä½  ðŸ˜€",
                        BodyEncodingBoundary::MetadataTag("memo"),
                    )
                    .unwrap()
            );
        }
    }

    #[test]
    fn legacy_javascript_escape_uses_the_exact_safe_character_set() {
        let source = "AZaz09@*_+-./";
        assert_eq!(
            BodyCodec::LegacyJavaScriptEscape
                .encode(source, &xml_key())
                .unwrap(),
            source
        );
        assert_eq!(
            BodyCodec::LegacyJavaScriptEscape
                .encode(" %'+/", &xml_key())
                .unwrap(),
            "%20%25%27+/"
        );
    }

    #[test]
    fn legacy_javascript_escape_encodes_latin1_bmp_and_surrogate_pairs() {
        assert_eq!(
            BodyCodec::LegacyJavaScriptEscape
                .encode("Ñ你", &xml_key())
                .unwrap(),
            "%D1%u4F60"
        );
        assert_eq!(
            BodyCodec::LegacyJavaScriptEscape
                .encode("😀", &xml_key())
                .unwrap(),
            "%uD83D%uDE00"
        );
    }

    #[test]
    fn utf8_percent_profile_is_rfc3986_unreserved_and_byte_exact() {
        let source = "AZaz09-._~ %'+/你😀";
        assert_eq!(
            BodyCodec::Utf8PercentRfc3986Unreserved
                .encode(source, &xml_key())
                .unwrap(),
            "AZaz09-._~%20%25%27%2B%2F%E4%BD%A0%F0%9F%98%80"
        );
    }

    #[test]
    fn omission_survives_format_and_encode_without_an_implicit_body() {
        let policy = SemanticFormatPolicy::new(
            AbsentValuePolicy::OmitOccurrence,
            BlankValuePolicy::Reject,
            PresentValueFormat::Text,
        );
        assert_eq!(
            format_and_encode(
                &CanonicalValue::Absent,
                &policy,
                BodyCodec::RawLiteral,
                &xml_key()
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn formatting_and_all_codecs_are_deterministic() {
        let value = CanonicalValue::Text("50% / 你 😀".to_string());
        let policy = required_policy(PresentValueFormat::Text);
        for codec in [
            BodyCodec::RawLiteral,
            BodyCodec::LegacyJavaScriptEscape,
            BodyCodec::Utf8PercentRfc3986Unreserved,
        ] {
            let first = format_and_encode(&value, &policy, codec, &xml_key()).unwrap();
            for _ in 0..32 {
                assert_eq!(
                    format_and_encode(&value, &policy, codec, &xml_key()).unwrap(),
                    first
                );
            }
        }
    }

    #[test]
    fn body_codec_serde_uses_the_closed_contract_alphabet() {
        for (codec, spelling) in [
            (BodyCodec::RawLiteral, "raw-literal"),
            (
                BodyCodec::LegacyJavaScriptEscape,
                "legacy-javascript-escape",
            ),
            (
                BodyCodec::Utf8PercentRfc3986Unreserved,
                "utf8-percent-rfc3986-unreserved",
            ),
        ] {
            let encoded = serde_json::to_string(&codec).expect("serialize body codec");
            assert_eq!(encoded, format!("\"{spelling}\""));
            assert_eq!(
                serde_json::from_str::<BodyCodec>(&encoded).expect("deserialize body codec"),
                codec
            );
        }

        assert!(serde_json::from_str::<BodyCodec>(r#""legacy-java-script-escape""#).is_err());
    }

    #[test]
    fn invalid_validated_settings_are_rejected_during_deserialization() {
        assert!(
            serde_json::from_str::<ExactBooleanFormat>(
                r#"{"true_text":"same","false_text":"same"}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<ExactDecimalFormat>(
                r#"{"minimum_fraction_digits":3,"maximum_fraction_digits":2}"#
            )
            .is_err()
        );
        assert!(serde_json::from_str::<ArtifactVariantId>(r#""Implicit Default""#).is_err());
    }
}
