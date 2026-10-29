use crate::{CompiledRuleSet, FormRevisionKey};
use std::{error::Error, fmt};

/// One executable registry entry.
///
/// Identity is always read from the sealed rule set itself, preventing a
/// registry manifest from advertising one digest while dispatching another.
#[derive(Clone, Copy)]
pub struct RuleSetRegistryEntry {
    rule_set: &'static dyn CompiledRuleSet,
}

impl RuleSetRegistryEntry {
    pub const fn new(rule_set: &'static dyn CompiledRuleSet) -> Self {
        Self { rule_set }
    }

    pub fn identity(self) -> &'static FormRevisionKey {
        self.rule_set.identity()
    }

    pub const fn rule_set(self) -> &'static dyn CompiledRuleSet {
        self.rule_set
    }
}

impl fmt::Debug for RuleSetRegistryEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuleSetRegistryEntry")
            .field("identity", self.rule_set.identity())
            .finish_non_exhaustive()
    }
}

/// Exact lookup interface for reviewed packaged snapshots.
pub trait RuleSetRegistry: Send + Sync {
    fn entries(&self) -> &[RuleSetRegistryEntry];

    fn resolve(
        &self,
        requested: &FormRevisionKey,
    ) -> Result<&'static dyn CompiledRuleSet, RegistryError> {
        let mut matches = self
            .entries()
            .iter()
            .copied()
            .filter(|entry| entry.identity() == requested);
        let first = matches.next().ok_or_else(|| RegistryError::NotFound {
            requested: requested.clone(),
        })?;
        if matches.next().is_some() {
            return Err(RegistryError::DuplicateIdentity {
                identity: requested.clone(),
            });
        }
        Ok(first.rule_set())
    }
}

/// Registry backed by a reviewed static entry table.
#[derive(Debug, Clone, Copy)]
pub struct StaticRuleSetRegistry {
    entries: &'static [RuleSetRegistryEntry],
}

impl StaticRuleSetRegistry {
    pub fn try_new(entries: &'static [RuleSetRegistryEntry]) -> Result<Self, RegistryError> {
        for pair in entries.windows(2) {
            match pair[0].identity().cmp(pair[1].identity()) {
                std::cmp::Ordering::Less => {}
                std::cmp::Ordering::Equal => {
                    return Err(RegistryError::DuplicateIdentity {
                        identity: pair[0].identity().clone(),
                    });
                }
                std::cmp::Ordering::Greater => {
                    return Err(RegistryError::EntriesOutOfOrder {
                        previous: pair[0].identity().clone(),
                        current: pair[1].identity().clone(),
                    });
                }
            }
        }
        Ok(Self { entries })
    }
}

impl RuleSetRegistry for StaticRuleSetRegistry {
    fn entries(&self) -> &[RuleSetRegistryEntry] {
        self.entries
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    NotFound {
        requested: FormRevisionKey,
    },
    DuplicateIdentity {
        identity: FormRevisionKey,
    },
    EntriesOutOfOrder {
        previous: FormRevisionKey,
        current: FormRevisionKey,
    },
}

impl fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { requested } => write!(
                formatter,
                "no compiled rule set registered for {} / {} / {} / {} / {}",
                requested.rule_set_id(),
                requested.form_code(),
                requested.form_revision(),
                requested.official_package_version(),
                requested.source_set_sha256()
            ),
            Self::DuplicateIdentity { identity } => write!(
                formatter,
                "compiled rule-set registry contains duplicate exact identity {}",
                identity.rule_set_id()
            ),
            Self::EntriesOutOfOrder { previous, current } => write!(
                formatter,
                "registry entries are not in exact identity order: {} follows {}",
                current.rule_set_id(),
                previous.rule_set_id()
            ),
        }
    }
}

impl Error for RegistryError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EvaluationError, EvaluationExpectation, EvaluationOutput, EvaluationRequest, FormCode,
        FormRevision, OfficialPackageVersion, RuleSetId, Sha256Digest,
    };

    struct NoopRuleSet {
        identity: FormRevisionKey,
    }

    impl crate::provider::sealed::Sealed for NoopRuleSet {
        fn expected_evaluation(
            &self,
            _request: &EvaluationRequest,
        ) -> Result<EvaluationExpectation, EvaluationError> {
            EvaluationExpectation::try_new(Vec::new(), Vec::new())
        }

        fn evaluate_compiled(
            &self,
            _request: &EvaluationRequest,
        ) -> Result<EvaluationOutput, EvaluationError> {
            Ok(EvaluationOutput::new(
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ))
        }
    }

    impl CompiledRuleSet for NoopRuleSet {
        fn identity(&self) -> &FormRevisionKey {
            &self.identity
        }

        fn serialization_contract(&self) -> &'static crate::StaticSerializationContract {
            &crate::StaticSerializationContract::EMPTY_V1
        }
    }

    fn identity(digest_byte: u8) -> FormRevisionKey {
        FormRevisionKey::new(
            RuleSetId::parse("test-v1-p1").unwrap(),
            FormCode::parse("TEST").unwrap(),
            FormRevision::parse("v1").unwrap(),
            OfficialPackageVersion::parse("p1").unwrap(),
            Sha256Digest::from_bytes([digest_byte; 32]),
        )
    }

    #[test]
    fn registry_resolves_only_the_complete_exact_identity() {
        let rule_set: &'static NoopRuleSet = Box::leak(Box::new(NoopRuleSet {
            identity: identity(1),
        }));
        let entries: &'static [RuleSetRegistryEntry] =
            Box::leak(vec![RuleSetRegistryEntry::new(rule_set)].into_boxed_slice());
        let registry = StaticRuleSetRegistry::try_new(entries).unwrap();

        assert_eq!(
            registry
                .resolve(&identity(1))
                .unwrap()
                .identity()
                .source_set_sha256(),
            Sha256Digest::from_bytes([1; 32])
        );
        assert!(matches!(
            registry.resolve(&identity(2)),
            Err(RegistryError::NotFound { .. })
        ));
    }
}
