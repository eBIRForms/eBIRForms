//! Borrowed, generated serialization plans bound to an exact compiled rule set.
//!
//! This module is intentionally declarative. It describes reviewed plaintext
//! artifact nodes but does not materialize bytes, select a fallback artifact,
//! or model compression, encryption, signing, or transport framing.

use crate::serialization::{
    AbsentValuePolicy, BlankValuePolicy, BodyCodec, ExactDatePattern, SerializationArtifactTarget,
};
use crate::static_ir::{
    Branch, DerivedInstanceSelector, FieldRef, Predicate, Profiled, RoundingMode, TypedValue,
};

#[derive(Debug)]
pub struct StaticSerializationContract {
    pub contract_version: &'static str,
    /// SHA-256 of the canonical JSON `serialization` subtree.
    ///
    /// `None` is reserved for explicit non-generated empty test providers.
    pub canonical_sha256: Option<&'static str>,
    pub artifacts: &'static [SerializationArtifactSpec],
}

impl StaticSerializationContract {
    pub const EMPTY_V1: Self = Self {
        contract_version: "1.0.0",
        canonical_sha256: None,
        artifacts: &[],
    };
}

#[derive(Debug, Clone, Copy)]
pub struct SerializationArtifactSpec {
    pub artifact_id: &'static str,
    pub target: SerializationArtifactTarget,
    pub variant_id: &'static str,
    pub branches: Profiled<Branch<SerializationPlan>>,
}

#[derive(Debug, Clone, Copy)]
pub struct SerializationPlan {
    pub nodes: &'static [SerializationNode],
}

#[derive(Debug, Clone, Copy)]
pub enum SerializationNode {
    PseudoXmlField(PseudoXmlFieldNode),
    MetadataElement(MetadataElementNode),
    ReviewedLiteral(ReviewedLiteralNode),
    DynamicGroup(DynamicGroupNode),
}

impl SerializationNode {
    pub const fn ordinal(self) -> u32 {
        match self {
            Self::PseudoXmlField(node) => node.ordinal,
            Self::MetadataElement(node) => node.ordinal,
            Self::ReviewedLiteral(node) => node.ordinal,
            Self::DynamicGroup(node) => node.ordinal,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PseudoXmlFieldNode {
    pub ordinal: u32,
    pub key_projection: SerializationKeyProjection,
    pub occurrence_projection: SerializationOccurrenceProjection,
    pub value_projection: SerializationValueProjection,
    pub semantic_format: SerializationSemanticFormat,
    pub body_codec: BodyCodec,
    pub presence: SerializationPresence,
}

#[derive(Debug, Clone, Copy)]
pub struct MetadataElementNode {
    pub ordinal: u32,
    pub exact_tag: &'static str,
    pub value_projection: SerializationValueProjection,
    pub semantic_format: SerializationSemanticFormat,
    pub body_codec: BodyCodec,
    pub presence: SerializationPresence,
}

#[derive(Debug, Clone, Copy)]
pub struct ReviewedLiteralNode {
    pub ordinal: u32,
    pub exact_bytes: &'static [u8],
}

#[derive(Debug, Clone, Copy)]
pub struct DynamicGroupNode {
    pub ordinal: u32,
    pub group_id: &'static str,
    pub instance_order: SerializationGroupInstanceOrder,
    pub min_occurs: usize,
    pub max_occurs: Option<usize>,
    pub nodes: &'static [SerializationNode],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SerializationGroupInstanceOrder {
    StableInstanceIdAscending,
}

#[derive(Debug, Clone, Copy)]
pub enum SerializationKeyProjection {
    Exact(&'static str),
    GroupIndexed(IndexedKeyProjection),
}

#[derive(Debug, Clone, Copy)]
pub struct IndexedKeyProjection {
    pub group_id: &'static str,
    pub index_base: u32,
    pub index_step: u32,
    pub padding: u32,
    pub prefix: &'static str,
    pub suffix: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub enum SerializationOccurrenceProjection {
    Fixed(u32),
    GroupIndexed(IndexedOccurrenceProjection),
}

#[derive(Debug, Clone, Copy)]
pub struct IndexedOccurrenceProjection {
    pub group_id: &'static str,
    pub index_base: u32,
    pub index_step: u32,
}

#[derive(Debug, Clone, Copy)]
pub enum SerializationValueProjection {
    Field(FieldRef),
    Derived {
        calculation_id: &'static str,
        output_id: &'static str,
        instance: DerivedInstanceSelector,
    },
    Context {
        context_value_id: &'static str,
    },
    Constant(TypedValue),
    Default(TypedValue),
}

#[derive(Debug, Clone, Copy)]
pub struct SerializationSemanticFormat {
    pub absent: AbsentValuePolicy,
    pub blank: BlankValuePolicy,
    pub present: SerializationPresentFormat,
}

#[derive(Debug, Clone, Copy)]
pub enum SerializationPresentFormat {
    Text,
    Boolean {
        true_text: &'static str,
        false_text: &'static str,
    },
    Base10Integer,
    Decimal(SerializationDecimalFormat),
    Date(ExactDatePattern),
}

#[derive(Debug, Clone, Copy)]
pub struct SerializationDecimalFormat {
    pub scale: u32,
    pub rounding: RoundingMode,
    pub grouping: SerializationGrouping,
    pub decimal_separator: SerializationDecimalSeparator,
    pub negative: SerializationNegativeRepresentation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SerializationGrouping {
    None,
    Comma,
    Period,
    Space,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SerializationDecimalSeparator {
    Period,
    Comma,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SerializationNegativeRepresentation {
    LeadingMinus,
    TrailingMinus,
    Parentheses,
}

#[derive(Debug, Clone, Copy)]
pub enum SerializationPresence {
    Always,
    When(Predicate),
    Omitted,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_empty_contract_authorizes_no_artifact() {
        assert_eq!(
            StaticSerializationContract::EMPTY_V1.contract_version,
            "1.0.0"
        );
        assert_eq!(StaticSerializationContract::EMPTY_V1.canonical_sha256, None);
        assert!(StaticSerializationContract::EMPTY_V1.artifacts.is_empty());
    }
}
