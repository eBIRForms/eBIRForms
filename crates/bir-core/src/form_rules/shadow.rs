use bir_rules::{
    ContextFingerprint, EvaluationError, EvaluationRequest, EvaluationResult, FormRevisionKey,
    InputRevision, RegistryError, ValidationContext,
};

/// Metadata needed to correlate shadow observations and discard stale output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluationStamp {
    rule_set: FormRevisionKey,
    context: ValidationContext,
    input_revision: InputRevision,
    context_fingerprint: ContextFingerprint,
}

impl EvaluationStamp {
    pub(crate) fn from_request(request: &EvaluationRequest) -> Self {
        Self {
            rule_set: request.rule_set().clone(),
            context: request.context(),
            input_revision: request.input_revision(),
            context_fingerprint: request.context_fingerprint(),
        }
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

    pub const fn context_fingerprint(&self) -> ContextFingerprint {
        self.context_fingerprint
    }
}

/// Observational result that carries failure without becoming authorization.
///
/// Consuming this value must not change UI validity, capabilities, final-copy
/// state, or queue behavior. Trusted callers use
/// [`super::FormRuleEvaluator::evaluate_trusted`] instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShadowEvaluationOutcome {
    Evaluated {
        stamp: EvaluationStamp,
        result: EvaluationResult,
    },
    RegistryFailure {
        stamp: EvaluationStamp,
        error: RegistryError,
    },
    EvaluationFailure {
        stamp: EvaluationStamp,
        error: EvaluationError,
    },
}

impl ShadowEvaluationOutcome {
    pub const fn stamp(&self) -> &EvaluationStamp {
        match self {
            Self::Evaluated { stamp, .. }
            | Self::RegistryFailure { stamp, .. }
            | Self::EvaluationFailure { stamp, .. } => stamp,
        }
    }

    /// The evaluated report, when there is one.
    pub const fn result(&self) -> Option<&EvaluationResult> {
        match self {
            Self::Evaluated { result, .. } => Some(result),
            Self::RegistryFailure { .. } | Self::EvaluationFailure { .. } => None,
        }
    }
}

/// Which axis two evaluations disagree on.
///
/// A shadow run is only useful if something compares its output to the
/// behaviour it shadows. Recording an outcome and never diffing it produces a
/// result nobody reads, which is what this module did before.
///
/// The four axes are those named in
/// `docs/validation-rules/implementation-plan.md`: issue identity, calculation,
/// profile, and serialization coverage. They are deliberately separate — a
/// message that differs is a presentation question, whereas a rule that fires on
/// one side only is a correctness question, and collapsing them would hide the
/// second behind the first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ShadowDifferenceKind {
    /// Rule identity, order, targeted fields, or exact message.
    Issue,
    /// Calculation identity, inputs, output, or rounding.
    Calculation,
    /// The same input judged differently under official versus filing-safe.
    Profile,
    /// Serialized key, count, default, or occurrence coverage.
    SerializationCoverage,
}

impl ShadowDifferenceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Issue => "issue",
            Self::Calculation => "calculation",
            Self::Profile => "profile",
            Self::SerializationCoverage => "serialization-coverage",
        }
    }
}

/// One recorded disagreement.
///
/// Carries the stamp so a difference can never be read against the wrong input
/// revision or context, which is the same staleness hazard the controller
/// guards against on the UI side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowDifference {
    kind: ShadowDifferenceKind,
    stamp: EvaluationStamp,
    /// Stable identifier of the thing that differs — a rule ID, calculation ID
    /// or serialized key. Never a rendered message.
    subject: String,
    observed: String,
    expected: String,
}

impl ShadowDifference {
    pub fn new(
        kind: ShadowDifferenceKind,
        stamp: EvaluationStamp,
        subject: impl Into<String>,
        observed: impl Into<String>,
        expected: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            stamp,
            subject: subject.into(),
            observed: observed.into(),
            expected: expected.into(),
        }
    }

    pub const fn kind(&self) -> ShadowDifferenceKind {
        self.kind
    }

    pub const fn stamp(&self) -> &EvaluationStamp {
        &self.stamp
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn observed(&self) -> &str {
        &self.observed
    }

    pub fn expected(&self) -> &str {
        &self.expected
    }
}

/// An ordered, deduplicated set of differences from one shadow observation.
///
/// Ordering is by axis then subject, not by discovery, so two runs over the same
/// inputs produce the same report and a diff between runs means behaviour
/// changed rather than iteration order did.
///
/// **A shadow report is never authorization.** A non-empty report does not block
/// anything and an empty one permits nothing; `evaluate_trusted` is the only
/// path that carries authority.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShadowDifferenceReport {
    differences: Vec<ShadowDifference>,
}

impl ShadowDifferenceReport {
    pub fn from_differences(differences: impl IntoIterator<Item = ShadowDifference>) -> Self {
        let mut differences: Vec<ShadowDifference> = differences.into_iter().collect();
        differences.sort_by(|left, right| {
            left.kind
                .cmp(&right.kind)
                .then_with(|| left.subject.cmp(&right.subject))
                .then_with(|| left.observed.cmp(&right.observed))
                .then_with(|| left.expected.cmp(&right.expected))
        });
        differences.dedup();
        Self { differences }
    }

    pub fn differences(&self) -> &[ShadowDifference] {
        &self.differences
    }

    pub fn is_empty(&self) -> bool {
        self.differences.is_empty()
    }

    /// Differences on one axis, for a reviewer working through them by kind.
    pub fn by_kind(&self, kind: ShadowDifferenceKind) -> impl Iterator<Item = &ShadowDifference> {
        self.differences
            .iter()
            .filter(move |difference| difference.kind == kind)
    }

    /// Whether any difference concerns correctness rather than presentation.
    ///
    /// An `Issue` difference means one side raised something the other did not,
    /// or against a different field. That is a behavioural divergence and needs
    /// an approved decision before the compiled rules replace the handwritten
    /// path — unlike a `SerializationCoverage` difference, which may simply
    /// reflect surface the compiled set does not model yet.
    pub fn has_behavioural_difference(&self) -> bool {
        self.differences.iter().any(|difference| {
            matches!(
                difference.kind,
                ShadowDifferenceKind::Issue
                    | ShadowDifferenceKind::Calculation
                    | ShadowDifferenceKind::Profile
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ShadowDifference, ShadowDifferenceKind, ShadowDifferenceReport};
    use bir_rules::{BehaviorProfile, ValidationPhase};
    use bir_rules::{
        ContextFingerprint, ContextValueSnapshot, FormCode, FormRevision, FormRevisionKey,
        InputRevision, OfficialPackageVersion, RuleSetId, Sha256Digest, ValidationContext,
    };

    fn stamp() -> super::EvaluationStamp {
        super::EvaluationStamp {
            rule_set: FormRevisionKey::new(
                RuleSetId::parse("shadow-test-p1").unwrap(),
                FormCode::parse("2550Q").unwrap(),
                FormRevision::parse("2024-04-01").unwrap(),
                OfficialPackageVersion::parse("7.9.6.0").unwrap(),
                Sha256Digest::from_bytes([7; 32]),
            ),
            context: ValidationContext::new(
                ValidationPhase::Validate,
                BehaviorProfile::OfficialCompatibility,
            ),
            input_revision: InputRevision::new(1),
            context_fingerprint: ContextValueSnapshot::try_new(Vec::new())
                .expect("empty context snapshot")
                .fingerprint(),
        }
    }

    fn difference(kind: ShadowDifferenceKind, subject: &str) -> ShadowDifference {
        ShadowDifference::new(kind, stamp(), subject, "observed", "expected")
    }

    /// Two runs over the same inputs must produce the same report, or a diff
    /// between runs would report iteration order as a behaviour change.
    #[test]
    fn report_order_is_stable_regardless_of_discovery_order() {
        let forward = ShadowDifferenceReport::from_differences([
            difference(ShadowDifferenceKind::Profile, "b"),
            difference(ShadowDifferenceKind::Issue, "z"),
            difference(ShadowDifferenceKind::Issue, "a"),
        ]);
        let reverse = ShadowDifferenceReport::from_differences([
            difference(ShadowDifferenceKind::Issue, "a"),
            difference(ShadowDifferenceKind::Issue, "z"),
            difference(ShadowDifferenceKind::Profile, "b"),
        ]);
        assert_eq!(forward, reverse);
        let subjects: Vec<&str> = forward
            .differences()
            .iter()
            .map(ShadowDifference::subject)
            .collect();
        assert_eq!(subjects, ["a", "z", "b"]);
    }

    #[test]
    fn identical_differences_collapse() {
        let report = ShadowDifferenceReport::from_differences([
            difference(ShadowDifferenceKind::Issue, "same"),
            difference(ShadowDifferenceKind::Issue, "same"),
        ]);
        assert_eq!(report.differences().len(), 1);
    }

    /// Serialization coverage alone is not a behavioural divergence: it can mean
    /// the compiled set does not model that surface yet. A rule firing on one
    /// side only is.
    #[test]
    fn only_correctness_axes_count_as_behavioural() {
        let coverage = ShadowDifferenceReport::from_differences([difference(
            ShadowDifferenceKind::SerializationCoverage,
            "frm2550qv2024:txtTIN1",
        )]);
        assert!(!coverage.is_empty());
        assert!(!coverage.has_behavioural_difference());

        for kind in [
            ShadowDifferenceKind::Issue,
            ShadowDifferenceKind::Calculation,
            ShadowDifferenceKind::Profile,
        ] {
            let report = ShadowDifferenceReport::from_differences([difference(kind, "subject")]);
            assert!(
                report.has_behavioural_difference(),
                "{} must count as behavioural",
                kind.as_str()
            );
        }
    }

    #[test]
    fn an_empty_report_authorizes_nothing_it_is_merely_empty() {
        let report = ShadowDifferenceReport::default();
        assert!(report.is_empty());
        assert!(!report.has_behavioural_difference());
        assert_eq!(report.by_kind(ShadowDifferenceKind::Issue).count(), 0);
    }
}
