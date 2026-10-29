use crate::Sha256Digest;
use serde::{Deserialize, Serialize};

/// The application event or trusted filing boundary being evaluated.
///
/// `DraftPreview` is deliberately distinct from `FinalCopy`: a preview may be
/// rendered from an incomplete local draft, while final-copy and submit
/// evaluation must fail closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ValidationPhase {
    Input,
    Blur,
    Change,
    BlurChange,
    PageNavigation,
    Save,
    DraftPreview,
    Validate,
    FinalCopy,
    Submit,
}

impl ValidationPhase {
    /// Exact field-event phases require an [`EvaluationRequest`](crate::EvaluationRequest)
    /// that names the field occurrence which dispatched the event.
    pub const fn is_field_event(self) -> bool {
        matches!(self, Self::Input | Self::Blur | Self::Change)
    }
}

/// Selects one independently reviewed behavior branch.
///
/// Official compatibility may reproduce a confirmed eBIRForms defect.
/// Filing-safe behavior is available only when separately reviewed and
/// compiled. An evaluator must never fall back from one branch to the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BehaviorProfile {
    #[serde(rename = "official")]
    OfficialCompatibility,
    #[serde(rename = "filing_safe")]
    FilingSafe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationContext {
    phase: ValidationPhase,
    profile: BehaviorProfile,
}

impl ValidationContext {
    pub const fn new(phase: ValidationPhase, profile: BehaviorProfile) -> Self {
        Self { phase, profile }
    }

    pub const fn phase(self) -> ValidationPhase {
        self.phase
    }

    pub const fn profile(self) -> BehaviorProfile {
        self.profile
    }
}

/// Monotonically increasing revision of the raw input snapshot.
///
/// UI controllers compare this value before installing a debounced result, so
/// a stale result cannot replace a newer edit.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(transparent)]
pub struct InputRevision(u64);

impl InputRevision {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Digest of all versioned external context used by one evaluation.
///
/// Rates, elections, clock/date policy, and profile facts are materialized in a
/// `ContextValueSnapshot`, which computes this fingerprint. It is not the
/// rule-set source digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContextFingerprint(Sha256Digest);

impl ContextFingerprint {
    pub const fn new(digest: Sha256Digest) -> Self {
        Self(digest)
    }

    pub const fn digest(self) -> Sha256Digest {
        self.0
    }
}

impl From<Sha256Digest> for ContextFingerprint {
    fn from(value: Sha256Digest) -> Self {
        Self::new(value)
    }
}
