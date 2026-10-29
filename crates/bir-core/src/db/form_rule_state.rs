//! Inert persistence for versioned raw form state and rules-based Final Copies.
//!
//! This module deliberately does not participate in capability checks, queue
//! creation, or submission. Callers must opt into each operation explicitly.

use bir_rules::{
    BehaviorProfile, CanonicalFieldValue, ContextValueSnapshot, DerivedOutputExpectation,
    DerivedValue, EvaluationRequest, EvaluationResult, FormRevisionKey, RawInputSnapshot,
    Sha256Digest, ValidationPhase, ValidationReport,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

use super::Database;
use crate::form_rules::{CheckedFinalCopyPayload, TrustedEvaluation};

const MIGRATION_AUDIT_SCHEMA: &str = "form-rule-migration-v1";
const MIGRATION_RESULT_ACCEPTED: &str = "accepted";

/// The rules identity pinned to one editable draft.
///
/// The constituent types reject malformed IDs and non-canonical SHA-256 text
/// before the identity can reach persistence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormRuleIdentity {
    pub rule_set: FormRevisionKey,
    pub behavior_profile: BehaviorProfile,
}

impl FormRuleIdentity {
    pub const fn new(rule_set: FormRevisionKey, behavior_profile: BehaviorProfile) -> Self {
        Self {
            rule_set,
            behavior_profile,
        }
    }

    pub fn from_rule_set(rule_set: &FormRevisionKey, behavior_profile: BehaviorProfile) -> Self {
        Self::new(rule_set.clone(), behavior_profile)
    }
}

/// Current persisted raw editor state for one `form_drafts` row.
#[derive(Clone, PartialEq, Eq)]
pub struct FormRuleState {
    pub form_draft_id: i64,
    /// Exact bytes supplied to `save_form_rule_editor_state`.
    pub editor_state_json: Option<String>,
    pub storage_revision: u64,
    pub identity: Option<FormRuleIdentity>,
    pub active_finalization_id: Option<i64>,
}

impl fmt::Debug for FormRuleState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FormRuleState")
            .field("form_draft_id", &self.form_draft_id)
            .field(
                "editor_state_byte_len",
                &self.editor_state_json.as_ref().map(String::len),
            )
            .field("storage_revision", &self.storage_revision)
            .field("identity", &self.identity)
            .field("active_finalization_id", &self.active_finalization_id)
            .finish_non_exhaustive()
    }
}

/// An immutable, integrity-checked Final Copy snapshot.
#[derive(Clone, PartialEq, Eq)]
pub struct FormFinalCopy {
    pub id: i64,
    pub form_draft_id: i64,
    pub source_storage_revision: u64,
    pub identity: FormRuleIdentity,
    pub input_sha256: Sha256Digest,
    pub context_sha256: Sha256Digest,
    pub canonical_sha256: Sha256Digest,
    pub derived_sha256: Sha256Digest,
    pub report_sha256: Sha256Digest,
    pub xml_sha256: Sha256Digest,
    pub payload_proof_sha256: Sha256Digest,
    /// Exact raw editor JSON from the source draft revision.
    pub raw_snapshot_json: String,
    pub canonical_json: String,
    pub derived_json: String,
    pub validation_report_json: String,
    pub xml_payload: String,
    pub checked_payload: CheckedFinalCopyPayload,
    pub context_values: ContextValueSnapshot,
    pub evaluation: EvaluationResult,
    pub created_at: String,
    pub invalidated_at: Option<String>,
}

impl fmt::Debug for FormFinalCopy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FormFinalCopy")
            .field("id", &self.id)
            .field("form_draft_id", &self.form_draft_id)
            .field("source_storage_revision", &self.source_storage_revision)
            .field("identity", &self.identity)
            .field("input_sha256", &self.input_sha256)
            .field("context_sha256", &self.context_sha256)
            .field("canonical_sha256", &self.canonical_sha256)
            .field("derived_sha256", &self.derived_sha256)
            .field("report_sha256", &self.report_sha256)
            .field("xml_sha256", &self.xml_sha256)
            .field("payload_proof_sha256", &self.payload_proof_sha256)
            .field("raw_snapshot_byte_len", &self.raw_snapshot_json.len())
            .field("canonical_byte_len", &self.canonical_json.len())
            .field("derived_byte_len", &self.derived_json.len())
            .field(
                "validation_report_byte_len",
                &self.validation_report_json.len(),
            )
            .field("xml_payload_byte_len", &self.xml_payload.len())
            .field("created_at", &self.created_at)
            .field("invalidated_at", &self.invalidated_at)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FormRuleStateError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("form draft {0} does not exist")]
    DraftNotFound(i64),
    #[error("stored form-rule state for draft {draft_id} is corrupt: {reason}")]
    CorruptState { draft_id: i64, reason: String },
    #[error("stored Final Copy {finalization_id} is corrupt: {reason}")]
    CorruptFinalCopy {
        finalization_id: i64,
        reason: String,
    },
    #[error(
        "storage revision conflict for draft {draft_id}: expected {expected}, current {actual}"
    )]
    StorageRevisionConflict {
        draft_id: i64,
        expected: u64,
        actual: u64,
    },
    #[error("storage revision {0} cannot be represented by SQLite")]
    StorageRevisionTooLarge(u64),
    #[error("draft {draft_id} has no pinned rule-set identity")]
    IdentityNotPinned { draft_id: i64 },
    #[error("draft {draft_id} is pinned to a different rule-set identity")]
    IdentityMismatch { draft_id: i64 },
    #[error("draft {draft_id} already uses the requested rule-set identity")]
    MigrationIdentityUnchanged { draft_id: i64 },
    #[error("draft {draft_id} already has a complete exact rule-set identity")]
    LegacyIdentityRepairNotRequired { draft_id: i64 },
    #[error("reviewed exact identity does not match projected legacy pin for draft {draft_id}")]
    LegacyIdentityRepairMismatch { draft_id: i64 },
    #[error("{kind} is not valid JSON: {source}")]
    InvalidJson {
        kind: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("Final Copy precondition failed: {0}")]
    InvalidFinalCopy(String),
    #[error("draft {draft_id} already has active Final Copy {finalization_id}")]
    ActiveFinalCopyExists { draft_id: i64, finalization_id: i64 },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredDerivedEvaluation {
    context_values: ContextValueSnapshot,
    expected_outputs: Vec<DerivedOutputExpectation>,
    derived_outputs: Vec<DerivedValue>,
}

#[derive(Serialize)]
struct EvaluationWireRef<'a> {
    report: &'a ValidationReport,
    canonical_inputs: &'a [CanonicalFieldValue],
    expected_outputs: &'a [DerivedOutputExpectation],
    derived_outputs: &'a [DerivedValue],
}

#[derive(Debug, Serialize, Deserialize)]
struct MigrationAudit {
    schema: String,
    from_behavior_profile: BehaviorProfile,
    to_behavior_profile: BehaviorProfile,
    details: serde_json::Value,
}

struct RawFinalCopyRow {
    id: i64,
    form_draft_id: i64,
    source_storage_revision: i64,
    rule_set_id: String,
    rule_set_form_code: Option<String>,
    rule_set_form_revision: Option<String>,
    rule_set_official_package_version: Option<String>,
    rule_set_sha256: String,
    behavior_profile: String,
    input_sha256: String,
    context_sha256: String,
    canonical_sha256: String,
    derived_sha256: Option<String>,
    report_sha256: String,
    xml_sha256: String,
    raw_snapshot_json: String,
    canonical_json: String,
    derived_json: String,
    validation_report_json: String,
    xml_payload: String,
    payload_proof_json: Option<String>,
    payload_proof_sha256: Option<String>,
    created_at: String,
    invalidated_at: Option<String>,
}

impl Database {
    /// Load the exact persisted editor-state JSON and its optimistic-lock
    /// revision. Legacy drafts remain unpinned until their first rules-aware
    /// save.
    pub fn load_form_rule_state(
        &self,
        form_draft_id: i64,
    ) -> Result<Option<FormRuleState>, FormRuleStateError> {
        load_form_rule_state_from(&self.conn, form_draft_id)
    }

    /// Save exact raw editor-state JSON using compare-and-swap semantics.
    ///
    /// The first successful save pins `identity`. Subsequent calls must supply
    /// the same identity; changing it requires `migrate_form_rule_set`.
    /// Every successful save increments `storage_revision` and invalidates the
    /// previously active Final Copy, if any.
    pub fn save_form_rule_editor_state(
        &self,
        form_draft_id: i64,
        expected_storage_revision: u64,
        identity: &FormRuleIdentity,
        editor_state_json: &str,
    ) -> Result<FormRuleState, FormRuleStateError> {
        validate_json("editor state", editor_state_json)?;

        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let current = required_form_rule_state(&tx, form_draft_id)?;
        require_storage_revision(&current, expected_storage_revision)?;
        if let Some(stored_identity) = &current.identity
            && stored_identity != identity
        {
            return Err(FormRuleStateError::IdentityMismatch {
                draft_id: form_draft_id,
            });
        }

        let next_revision = next_storage_revision(expected_storage_revision)?;
        let affected = tx.execute(
            "UPDATE form_drafts
             SET editor_state_json = ?1,
                 storage_revision = ?2,
                 rule_set_id = ?3,
                 rule_set_form_code = ?4,
                 rule_set_form_revision = ?5,
                 rule_set_official_package_version = ?6,
                 rule_set_sha256 = ?7,
                 behavior_profile = ?8,
                 active_finalization_id = NULL,
                 updated_at = datetime('now')
             WHERE id = ?9 AND storage_revision = ?10",
            params![
                editor_state_json,
                revision_to_sql(next_revision)?,
                identity.rule_set.rule_set_id().as_str(),
                identity.rule_set.form_code().as_str(),
                identity.rule_set.form_revision().as_str(),
                identity.rule_set.official_package_version().as_str(),
                identity.rule_set.source_set_sha256().to_hex(),
                behavior_profile_to_db(identity.behavior_profile),
                form_draft_id,
                revision_to_sql(expected_storage_revision)?,
            ],
        )?;
        if affected != 1 {
            return Err(FormRuleStateError::StorageRevisionConflict {
                draft_id: form_draft_id,
                expected: expected_storage_revision,
                actual: current.storage_revision,
            });
        }
        invalidate_active_final_copy(&tx, &current)?;

        let saved = required_form_rule_state(&tx, form_draft_id)?;
        tx.commit()?;
        Ok(saved)
    }

    /// Explicitly migrate a pinned draft to a different rule-set identity.
    ///
    /// The old and new exact editor snapshots are hashed internally. The
    /// caller's JSON details are wrapped with both behavior profiles; the
    /// append-only audit row records every component of both exact rule-set
    /// keys in dedicated columns.
    pub fn migrate_form_rule_set(
        &self,
        form_draft_id: i64,
        expected_storage_revision: u64,
        to_identity: &FormRuleIdentity,
        migrated_editor_state_json: &str,
        migration_details_json: &str,
    ) -> Result<FormRuleState, FormRuleStateError> {
        validate_json("migrated editor state", migrated_editor_state_json)?;
        let migration_details = parse_json("rule-set migration details", migration_details_json)?;

        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let current = required_form_rule_state(&tx, form_draft_id)?;
        require_storage_revision(&current, expected_storage_revision)?;
        let from_identity =
            current
                .identity
                .as_ref()
                .ok_or(FormRuleStateError::IdentityNotPinned {
                    draft_id: form_draft_id,
                })?;
        if from_identity == to_identity {
            return Err(FormRuleStateError::MigrationIdentityUnchanged {
                draft_id: form_draft_id,
            });
        }
        let from_snapshot = current.editor_state_json.as_deref().ok_or_else(|| {
            FormRuleStateError::CorruptState {
                draft_id: form_draft_id,
                reason: "pinned identity has no editor-state JSON".to_string(),
            }
        })?;

        let audit_json = serde_json::to_string(&MigrationAudit {
            schema: MIGRATION_AUDIT_SCHEMA.to_string(),
            from_behavior_profile: from_identity.behavior_profile,
            to_behavior_profile: to_identity.behavior_profile,
            details: migration_details,
        })
        .map_err(|source| FormRuleStateError::InvalidJson {
            kind: "rule-set migration audit",
            source,
        })?;
        let from_snapshot_sha256 = sha256_digest(from_snapshot.as_bytes());
        let to_snapshot_sha256 = sha256_digest(migrated_editor_state_json.as_bytes());
        let next_revision = next_storage_revision(expected_storage_revision)?;

        let affected = tx.execute(
            "UPDATE form_drafts
             SET editor_state_json = ?1,
                 storage_revision = ?2,
                 rule_set_id = ?3,
                 rule_set_form_code = ?4,
                 rule_set_form_revision = ?5,
                 rule_set_official_package_version = ?6,
                 rule_set_sha256 = ?7,
                 behavior_profile = ?8,
                 active_finalization_id = NULL,
                 updated_at = datetime('now')
             WHERE id = ?9 AND storage_revision = ?10",
            params![
                migrated_editor_state_json,
                revision_to_sql(next_revision)?,
                to_identity.rule_set.rule_set_id().as_str(),
                to_identity.rule_set.form_code().as_str(),
                to_identity.rule_set.form_revision().as_str(),
                to_identity.rule_set.official_package_version().as_str(),
                to_identity.rule_set.source_set_sha256().to_hex(),
                behavior_profile_to_db(to_identity.behavior_profile),
                form_draft_id,
                revision_to_sql(expected_storage_revision)?,
            ],
        )?;
        if affected != 1 {
            return Err(FormRuleStateError::StorageRevisionConflict {
                draft_id: form_draft_id,
                expected: expected_storage_revision,
                actual: current.storage_revision,
            });
        }
        invalidate_active_final_copy(&tx, &current)?;

        tx.execute(
            "INSERT INTO form_rule_migrations (
                form_draft_id,
                from_rule_set_id,
                from_rule_set_form_code,
                from_rule_set_form_revision,
                from_rule_set_official_package_version,
                from_rule_set_sha256,
                to_rule_set_id,
                to_rule_set_form_code,
                to_rule_set_form_revision,
                to_rule_set_official_package_version,
                to_rule_set_sha256,
                from_snapshot_sha256,
                to_snapshot_sha256,
                diff_json,
                result
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15
             )",
            params![
                form_draft_id,
                from_identity.rule_set.rule_set_id().as_str(),
                from_identity.rule_set.form_code().as_str(),
                from_identity.rule_set.form_revision().as_str(),
                from_identity.rule_set.official_package_version().as_str(),
                from_identity.rule_set.source_set_sha256().to_hex(),
                to_identity.rule_set.rule_set_id().as_str(),
                to_identity.rule_set.form_code().as_str(),
                to_identity.rule_set.form_revision().as_str(),
                to_identity.rule_set.official_package_version().as_str(),
                to_identity.rule_set.source_set_sha256().to_hex(),
                from_snapshot_sha256.to_hex(),
                to_snapshot_sha256.to_hex(),
                audit_json,
                MIGRATION_RESULT_ACCEPTED,
            ],
        )?;

        let migrated = required_form_rule_state(&tx, form_draft_id)?;
        tx.commit()?;
        Ok(migrated)
    }

    /// Complete a projected v15/v16 draft pin after a human has reviewed the
    /// exact form code, revision, and official package version.
    ///
    /// This is deliberately separate from normal rule-set migration. It never
    /// consults a registry or infers an omitted key component. The supplied
    /// identity must preserve the legacy rule-set ID, source digest, behavior
    /// profile, and the draft's own form code. The raw snapshot is unchanged,
    /// its storage revision advances, any legacy active Final Copy is
    /// invalidated, and the append-only audit records NULL legacy components
    /// alongside the reviewed exact identity.
    pub fn repair_legacy_projected_form_rule_identity(
        &self,
        form_draft_id: i64,
        expected_storage_revision: u64,
        reviewed_identity: &FormRuleIdentity,
        review_details_json: &str,
    ) -> Result<FormRuleState, FormRuleStateError> {
        let review_details = parse_json("legacy identity repair details", review_details_json)?;
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        // The tuple mirrors the selected column list one-for-one; naming it
        // would hide which column each slot corresponds to.
        #[allow(clippy::type_complexity)]
        let row: Option<(
            Option<String>,
            i64,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i64>,
        )> = tx
            .query_row(
                "SELECT editor_state_json,
                        storage_revision,
                        form_code,
                        rule_set_id,
                        rule_set_form_code,
                        rule_set_form_revision,
                        rule_set_official_package_version,
                        rule_set_sha256,
                        behavior_profile,
                        active_finalization_id
                 FROM form_drafts
                 WHERE id = ?1",
                [form_draft_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                    ))
                },
            )
            .optional()?;
        let Some((
            editor_state_json,
            stored_revision,
            draft_form_code,
            rule_set_id,
            rule_set_form_code,
            rule_set_form_revision,
            rule_set_official_package_version,
            rule_set_sha256,
            behavior_profile,
            active_finalization_id,
        )) = row
        else {
            return Err(FormRuleStateError::DraftNotFound(form_draft_id));
        };
        let actual_revision = revision_from_sql(form_draft_id, stored_revision)?;
        if actual_revision != expected_storage_revision {
            return Err(FormRuleStateError::StorageRevisionConflict {
                draft_id: form_draft_id,
                expected: expected_storage_revision,
                actual: actual_revision,
            });
        }
        let editor_state_json =
            editor_state_json.ok_or_else(|| FormRuleStateError::CorruptState {
                draft_id: form_draft_id,
                reason: "projected legacy identity has no editor-state JSON".to_string(),
            })?;
        validate_stored_json(form_draft_id, "editor-state JSON", &editor_state_json)?;

        let (legacy_rule_set_id, legacy_rule_set_sha256, legacy_behavior_profile) = match (
            rule_set_id,
            rule_set_form_code,
            rule_set_form_revision,
            rule_set_official_package_version,
            rule_set_sha256,
            behavior_profile,
        ) {
            (None, None, None, None, None, None) => {
                return Err(FormRuleStateError::IdentityNotPinned {
                    draft_id: form_draft_id,
                });
            }
            (Some(_), Some(_), Some(_), Some(_), Some(_), Some(_)) => {
                return Err(FormRuleStateError::LegacyIdentityRepairNotRequired {
                    draft_id: form_draft_id,
                });
            }
            (
                Some(rule_set_id),
                None,
                None,
                None,
                Some(rule_set_sha256),
                Some(behavior_profile),
            ) => (rule_set_id, rule_set_sha256, behavior_profile),
            _ => {
                return Err(FormRuleStateError::CorruptState {
                    draft_id: form_draft_id,
                    reason: "legacy rule-set identity is neither projected nor complete"
                        .to_string(),
                });
            }
        };
        let legacy_behavior_profile = behavior_profile_from_db(&legacy_behavior_profile)
            .ok_or_else(|| FormRuleStateError::CorruptState {
                draft_id: form_draft_id,
                reason: format!("invalid behavior profile {legacy_behavior_profile:?}"),
            })?;
        if legacy_rule_set_id != reviewed_identity.rule_set.rule_set_id().as_str()
            || legacy_rule_set_sha256 != reviewed_identity.rule_set.source_set_sha256().to_hex()
            || legacy_behavior_profile != reviewed_identity.behavior_profile
            || draft_form_code != reviewed_identity.rule_set.form_code().as_str()
        {
            return Err(FormRuleStateError::LegacyIdentityRepairMismatch {
                draft_id: form_draft_id,
            });
        }

        let audit_json = serde_json::to_string(&MigrationAudit {
            schema: MIGRATION_AUDIT_SCHEMA.to_string(),
            from_behavior_profile: legacy_behavior_profile,
            to_behavior_profile: reviewed_identity.behavior_profile,
            details: serde_json::json!({
                "kind": "legacy-projected-identity-repair",
                "review": review_details,
            }),
        })
        .map_err(|source| FormRuleStateError::InvalidJson {
            kind: "legacy identity repair audit",
            source,
        })?;
        let snapshot_sha256 = sha256_digest(editor_state_json.as_bytes());
        let next_revision = next_storage_revision(expected_storage_revision)?;
        let affected = tx.execute(
            "UPDATE form_drafts
             SET storage_revision = ?1,
                 rule_set_form_code = ?2,
                 rule_set_form_revision = ?3,
                 rule_set_official_package_version = ?4,
                 active_finalization_id = NULL,
                 updated_at = datetime('now')
             WHERE id = ?5
               AND storage_revision = ?6
               AND rule_set_id = ?7
               AND rule_set_form_code IS NULL
               AND rule_set_form_revision IS NULL
               AND rule_set_official_package_version IS NULL
               AND rule_set_sha256 = ?8
               AND behavior_profile = ?9",
            params![
                revision_to_sql(next_revision)?,
                reviewed_identity.rule_set.form_code().as_str(),
                reviewed_identity.rule_set.form_revision().as_str(),
                reviewed_identity
                    .rule_set
                    .official_package_version()
                    .as_str(),
                form_draft_id,
                revision_to_sql(expected_storage_revision)?,
                legacy_rule_set_id,
                legacy_rule_set_sha256,
                behavior_profile_to_db(legacy_behavior_profile),
            ],
        )?;
        if affected != 1 {
            return Err(FormRuleStateError::CorruptState {
                draft_id: form_draft_id,
                reason: "projected legacy identity changed before reviewed repair".to_string(),
            });
        }
        invalidate_active_final_copy(
            &tx,
            &FormRuleState {
                form_draft_id,
                editor_state_json: Some(editor_state_json.clone()),
                storage_revision: actual_revision,
                identity: None,
                active_finalization_id,
            },
        )?;

        tx.execute(
            "INSERT INTO form_rule_migrations (
                form_draft_id,
                from_rule_set_id,
                from_rule_set_form_code,
                from_rule_set_form_revision,
                from_rule_set_official_package_version,
                from_rule_set_sha256,
                to_rule_set_id,
                to_rule_set_form_code,
                to_rule_set_form_revision,
                to_rule_set_official_package_version,
                to_rule_set_sha256,
                from_snapshot_sha256,
                to_snapshot_sha256,
                diff_json,
                result
             ) VALUES (
                ?1, ?2, NULL, NULL, NULL, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12
             )",
            params![
                form_draft_id,
                legacy_rule_set_id,
                legacy_rule_set_sha256,
                reviewed_identity.rule_set.rule_set_id().as_str(),
                reviewed_identity.rule_set.form_code().as_str(),
                reviewed_identity.rule_set.form_revision().as_str(),
                reviewed_identity
                    .rule_set
                    .official_package_version()
                    .as_str(),
                reviewed_identity.rule_set.source_set_sha256().to_hex(),
                snapshot_sha256.to_hex(),
                snapshot_sha256.to_hex(),
                audit_json,
                MIGRATION_RESULT_ACCEPTED,
            ],
        )?;

        let repaired = required_form_rule_state(&tx, form_draft_id)?;
        tx.commit()?;
        Ok(repaired)
    }

    /// Create an immutable Final Copy from the exact current raw state, a
    /// complete blocking-valid trusted evaluation, and an opaque serializer
    /// proof bound to that same request and evaluation.
    ///
    /// The context digest is computed from the request's validated, ordered
    /// `ContextValueSnapshot`. All persisted digests are computed here rather
    /// than accepted from the caller.
    pub fn create_form_final_copy(
        &self,
        form_draft_id: i64,
        expected_storage_revision: u64,
        trusted_evaluation: &TrustedEvaluation,
        checked_payload: &CheckedFinalCopyPayload,
    ) -> Result<FormFinalCopy, FormRuleStateError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let current = required_form_rule_state(&tx, form_draft_id)?;
        require_storage_revision(&current, expected_storage_revision)?;
        if let Some(finalization_id) = current.active_finalization_id {
            return Err(FormRuleStateError::ActiveFinalCopyExists {
                draft_id: form_draft_id,
                finalization_id,
            });
        }
        let identity = current
            .identity
            .as_ref()
            .ok_or(FormRuleStateError::IdentityNotPinned {
                draft_id: form_draft_id,
            })?;
        let raw_snapshot_json = current.editor_state_json.as_deref().ok_or_else(|| {
            FormRuleStateError::CorruptState {
                draft_id: form_draft_id,
                reason: "pinned identity has no editor-state JSON".to_string(),
            }
        })?;

        let request = trusted_evaluation.request();
        let evaluation = trusted_evaluation.result();
        validate_final_copy_inputs(
            form_draft_id,
            &current,
            identity,
            trusted_evaluation,
            checked_payload,
            raw_snapshot_json,
        )?;

        let canonical_json =
            serde_json::to_string(evaluation.canonical_inputs()).map_err(|source| {
                FormRuleStateError::InvalidJson {
                    kind: "canonical evaluation",
                    source,
                }
            })?;
        let derived_json = serde_json::to_string(&StoredDerivedEvaluation {
            context_values: request.context_values().clone(),
            expected_outputs: evaluation.expected_outputs().to_vec(),
            derived_outputs: evaluation.derived_outputs().to_vec(),
        })
        .map_err(|source| FormRuleStateError::InvalidJson {
            kind: "derived evaluation",
            source,
        })?;
        let validation_report_json =
            serde_json::to_string(evaluation.report()).map_err(|source| {
                FormRuleStateError::InvalidJson {
                    kind: "validation report",
                    source,
                }
            })?;

        let input_sha256 = sha256_digest(raw_snapshot_json.as_bytes());
        let context_sha256 = evaluation_context_digest(request);
        let canonical_sha256 = sha256_digest(canonical_json.as_bytes());
        let derived_sha256 = sha256_digest(derived_json.as_bytes());
        let report_sha256 = sha256_digest(validation_report_json.as_bytes());
        let xml_payload = checked_payload.xml_payload();
        let xml_sha256 = checked_payload.xml_sha256();
        let payload_proof_json = checked_payload.proof_json();
        let payload_proof_sha256 = checked_payload.proof_sha256();

        tx.execute(
            "INSERT INTO form_finalizations (
                form_draft_id,
                source_storage_revision,
                rule_set_id,
                rule_set_form_code,
                rule_set_form_revision,
                rule_set_official_package_version,
                rule_set_sha256,
                behavior_profile,
                input_sha256,
                context_sha256,
                canonical_sha256,
                derived_sha256,
                report_sha256,
                xml_sha256,
                raw_snapshot_json,
                canonical_json,
                derived_json,
                validation_report_json,
                xml_payload,
                payload_proof_json,
                payload_proof_sha256
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                ?17, ?18, ?19, ?20, ?21
             )",
            params![
                form_draft_id,
                revision_to_sql(expected_storage_revision)?,
                identity.rule_set.rule_set_id().as_str(),
                identity.rule_set.form_code().as_str(),
                identity.rule_set.form_revision().as_str(),
                identity.rule_set.official_package_version().as_str(),
                identity.rule_set.source_set_sha256().to_hex(),
                behavior_profile_to_db(identity.behavior_profile),
                input_sha256.to_hex(),
                context_sha256.to_hex(),
                canonical_sha256.to_hex(),
                derived_sha256.to_hex(),
                report_sha256.to_hex(),
                xml_sha256.to_hex(),
                raw_snapshot_json,
                canonical_json,
                derived_json,
                validation_report_json,
                xml_payload,
                payload_proof_json,
                payload_proof_sha256.to_hex(),
            ],
        )?;
        let finalization_id = tx.last_insert_rowid();

        let affected = tx.execute(
            "UPDATE form_drafts
             SET active_finalization_id = ?1
             WHERE id = ?2
               AND storage_revision = ?3
               AND active_finalization_id IS NULL",
            params![
                finalization_id,
                form_draft_id,
                revision_to_sql(expected_storage_revision)?,
            ],
        )?;
        if affected != 1 {
            return Err(FormRuleStateError::StorageRevisionConflict {
                draft_id: form_draft_id,
                expected: expected_storage_revision,
                actual: current.storage_revision,
            });
        }

        let final_copy = load_form_final_copy_from(&tx, finalization_id)?.ok_or_else(|| {
            FormRuleStateError::CorruptFinalCopy {
                finalization_id,
                reason: "inserted row could not be read back".to_string(),
            }
        })?;
        tx.commit()?;
        Ok(final_copy)
    }

    /// Read any immutable Final Copy by ID, including an invalidated historical
    /// copy. Stored hashes and the complete evaluation are revalidated.
    pub fn load_form_final_copy(
        &self,
        finalization_id: i64,
    ) -> Result<Option<FormFinalCopy>, FormRuleStateError> {
        load_form_final_copy_from(&self.conn, finalization_id)
    }

    /// Read the current active Final Copy only when it still matches the
    /// draft's exact revision, identity, and raw editor snapshot.
    pub fn load_active_form_final_copy(
        &self,
        form_draft_id: i64,
    ) -> Result<Option<FormFinalCopy>, FormRuleStateError> {
        let state = match load_form_rule_state_from(&self.conn, form_draft_id)? {
            Some(state) => state,
            None => return Ok(None),
        };
        let Some(finalization_id) = state.active_finalization_id else {
            return Ok(None);
        };
        let final_copy =
            load_form_final_copy_from(&self.conn, finalization_id)?.ok_or_else(|| {
                FormRuleStateError::CorruptState {
                    draft_id: form_draft_id,
                    reason: format!("active Final Copy {finalization_id} does not exist"),
                }
            })?;
        if final_copy.form_draft_id != form_draft_id {
            return Err(FormRuleStateError::CorruptState {
                draft_id: form_draft_id,
                reason: format!(
                    "active Final Copy {finalization_id} belongs to draft {}",
                    final_copy.form_draft_id
                ),
            });
        }
        if final_copy.invalidated_at.is_some()
            || final_copy.source_storage_revision != state.storage_revision
            || state.identity.as_ref() != Some(&final_copy.identity)
            || state.editor_state_json.as_deref() != Some(&final_copy.raw_snapshot_json)
        {
            return Err(FormRuleStateError::CorruptState {
                draft_id: form_draft_id,
                reason: format!(
                    "active Final Copy {finalization_id} does not match current draft state"
                ),
            });
        }
        Ok(Some(final_copy))
    }
}

fn load_form_rule_state_from(
    conn: &Connection,
    form_draft_id: i64,
) -> Result<Option<FormRuleState>, FormRuleStateError> {
    let row = conn
        .query_row(
            "SELECT editor_state_json,
                    storage_revision,
                    rule_set_id,
                    rule_set_form_code,
                    rule_set_form_revision,
                    rule_set_official_package_version,
                    rule_set_sha256,
                    behavior_profile,
                    active_finalization_id
             FROM form_drafts
             WHERE id = ?1",
            [form_draft_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                ))
            },
        )
        .optional()?;

    let Some((
        editor_state_json,
        storage_revision,
        rule_set_id,
        rule_set_form_code,
        rule_set_form_revision,
        rule_set_official_package_version,
        rule_set_sha256,
        behavior_profile,
        active_finalization_id,
    )) = row
    else {
        return Ok(None);
    };
    if let Some(editor_state_json) = &editor_state_json {
        validate_stored_json(form_draft_id, "editor-state JSON", editor_state_json)?;
    }
    let identity = decode_identity(
        form_draft_id,
        rule_set_id,
        rule_set_form_code,
        rule_set_form_revision,
        rule_set_official_package_version,
        rule_set_sha256,
        behavior_profile,
    )?;
    Ok(Some(FormRuleState {
        form_draft_id,
        editor_state_json,
        storage_revision: revision_from_sql(form_draft_id, storage_revision)?,
        identity,
        active_finalization_id,
    }))
}

fn required_form_rule_state(
    conn: &Connection,
    form_draft_id: i64,
) -> Result<FormRuleState, FormRuleStateError> {
    load_form_rule_state_from(conn, form_draft_id)?
        .ok_or(FormRuleStateError::DraftNotFound(form_draft_id))
}

fn decode_identity(
    form_draft_id: i64,
    rule_set_id: Option<String>,
    rule_set_form_code: Option<String>,
    rule_set_form_revision: Option<String>,
    rule_set_official_package_version: Option<String>,
    rule_set_sha256: Option<String>,
    behavior_profile: Option<String>,
) -> Result<Option<FormRuleIdentity>, FormRuleStateError> {
    match (
        rule_set_id,
        rule_set_form_code,
        rule_set_form_revision,
        rule_set_official_package_version,
        rule_set_sha256,
        behavior_profile,
    ) {
        (None, None, None, None, None, None) => Ok(None),
        (
            Some(rule_set_id),
            Some(form_code),
            Some(form_revision),
            Some(official_package_version),
            Some(rule_set_sha256),
            Some(behavior_profile),
        ) => {
            let rule_set = FormRevisionKey::parse(
                rule_set_id,
                form_code,
                form_revision,
                official_package_version,
                &rule_set_sha256,
            )
            .map_err(|error| {
                FormRuleStateError::CorruptState {
                    draft_id: form_draft_id,
                    reason: format!("invalid exact rule-set identity: {error}"),
                }
            })?;
            let behavior_profile =
                behavior_profile_from_db(&behavior_profile).ok_or_else(|| {
                    FormRuleStateError::CorruptState {
                        draft_id: form_draft_id,
                        reason: format!("invalid behavior profile {behavior_profile:?}"),
                    }
                })?;
            Ok(Some(FormRuleIdentity::new(rule_set, behavior_profile)))
        }
        _ => Err(FormRuleStateError::CorruptState {
            draft_id: form_draft_id,
            reason: "exact rule-set identity is only partially populated; reviewed migration is required"
                .to_string(),
        }),
    }
}

fn behavior_profile_to_db(profile: BehaviorProfile) -> &'static str {
    match profile {
        BehaviorProfile::OfficialCompatibility => "official",
        BehaviorProfile::FilingSafe => "filing_safe",
    }
}

fn behavior_profile_from_db(value: &str) -> Option<BehaviorProfile> {
    match value {
        "official" => Some(BehaviorProfile::OfficialCompatibility),
        "filing_safe" => Some(BehaviorProfile::FilingSafe),
        _ => None,
    }
}

fn validate_json(kind: &'static str, value: &str) -> Result<(), FormRuleStateError> {
    parse_json(kind, value).map(|_| ())
}

fn parse_json(kind: &'static str, value: &str) -> Result<serde_json::Value, FormRuleStateError> {
    serde_json::from_str(value).map_err(|source| FormRuleStateError::InvalidJson { kind, source })
}

fn validate_stored_json(
    form_draft_id: i64,
    kind: &str,
    value: &str,
) -> Result<(), FormRuleStateError> {
    serde_json::from_str::<serde_json::Value>(value)
        .map(|_| ())
        .map_err(|error| FormRuleStateError::CorruptState {
            draft_id: form_draft_id,
            reason: format!("{kind} is invalid: {error}"),
        })
}

fn require_storage_revision(
    state: &FormRuleState,
    expected: u64,
) -> Result<(), FormRuleStateError> {
    if state.storage_revision != expected {
        return Err(FormRuleStateError::StorageRevisionConflict {
            draft_id: state.form_draft_id,
            expected,
            actual: state.storage_revision,
        });
    }
    Ok(())
}

fn next_storage_revision(current: u64) -> Result<u64, FormRuleStateError> {
    let next = current
        .checked_add(1)
        .ok_or(FormRuleStateError::StorageRevisionTooLarge(current))?;
    revision_to_sql(next)?;
    Ok(next)
}

fn revision_to_sql(revision: u64) -> Result<i64, FormRuleStateError> {
    i64::try_from(revision).map_err(|_| FormRuleStateError::StorageRevisionTooLarge(revision))
}

fn revision_from_sql(form_draft_id: i64, revision: i64) -> Result<u64, FormRuleStateError> {
    u64::try_from(revision).map_err(|_| FormRuleStateError::CorruptState {
        draft_id: form_draft_id,
        reason: format!("negative storage revision {revision}"),
    })
}

fn invalidate_active_final_copy(
    tx: &Transaction<'_>,
    current: &FormRuleState,
) -> Result<(), FormRuleStateError> {
    let Some(finalization_id) = current.active_finalization_id else {
        return Ok(());
    };
    let affected = tx.execute(
        "UPDATE form_finalizations
         SET invalidated_at = datetime('now')
         WHERE id = ?1
           AND form_draft_id = ?2
           AND invalidated_at IS NULL",
        params![finalization_id, current.form_draft_id],
    )?;
    if affected != 1 {
        return Err(FormRuleStateError::CorruptState {
            draft_id: current.form_draft_id,
            reason: format!(
                "active Final Copy {finalization_id} is missing or already invalidated"
            ),
        });
    }
    Ok(())
}

fn validate_final_copy_inputs(
    form_draft_id: i64,
    current: &FormRuleState,
    identity: &FormRuleIdentity,
    trusted_evaluation: &TrustedEvaluation,
    checked_payload: &CheckedFinalCopyPayload,
    raw_snapshot_json: &str,
) -> Result<(), FormRuleStateError> {
    let request = trusted_evaluation.request();
    let evaluation = trusted_evaluation.result();
    if identity.behavior_profile != BehaviorProfile::FilingSafe
        || trusted_evaluation.context().profile() != BehaviorProfile::FilingSafe
    {
        return Err(FormRuleStateError::InvalidFinalCopy(
            "only a filing-safe trusted evaluation can authorize Final Copy".to_string(),
        ));
    }
    if request.context().phase() != ValidationPhase::FinalCopy
        || evaluation.context().phase() != ValidationPhase::FinalCopy
    {
        return Err(FormRuleStateError::InvalidFinalCopy(
            "evaluation phase is not Final Copy".to_string(),
        ));
    }
    if !evaluation.is_valid() {
        return Err(FormRuleStateError::InvalidFinalCopy(
            "evaluation contains blocking violations".to_string(),
        ));
    }
    checked_payload
        .validate_against_trusted(trusted_evaluation)
        .map_err(|error| FormRuleStateError::InvalidFinalCopy(error.to_string()))?;
    if request.rule_set() != evaluation.rule_set()
        || request.context() != evaluation.context()
        || request.input_revision() != evaluation.input_revision()
        || request.context_fingerprint() != evaluation.context_fingerprint()
    {
        return Err(FormRuleStateError::InvalidFinalCopy(
            "evaluation does not match its request".to_string(),
        ));
    }
    if request.input_revision().get() != current.storage_revision {
        return Err(FormRuleStateError::InvalidFinalCopy(format!(
            "evaluation input revision {} does not match draft revision {}",
            request.input_revision().get(),
            current.storage_revision
        )));
    }
    let evaluated_identity =
        FormRuleIdentity::from_rule_set(request.rule_set(), request.context().profile());
    if identity != &evaluated_identity {
        return Err(FormRuleStateError::IdentityMismatch {
            draft_id: form_draft_id,
        });
    }

    let stored_inputs: RawInputSnapshot =
        serde_json::from_str(raw_snapshot_json).map_err(|error| {
            FormRuleStateError::InvalidFinalCopy(format!(
                "editor-state JSON is not a validated raw input snapshot: {error}"
            ))
        })?;
    if &stored_inputs != request.raw_inputs() {
        return Err(FormRuleStateError::InvalidFinalCopy(
            "evaluation inputs do not match the persisted raw editor state".to_string(),
        ));
    }

    let context_sha256 = evaluation_context_digest(request);
    if request.context_fingerprint().digest() != context_sha256 {
        return Err(FormRuleStateError::InvalidFinalCopy(
            "evaluation context fingerprint does not match context values".to_string(),
        ));
    }
    ensure_raw_matches_canonical(None, request.raw_inputs(), evaluation.canonical_inputs())
}

fn ensure_raw_matches_canonical(
    finalization_id: Option<i64>,
    raw_inputs: &RawInputSnapshot,
    canonical_inputs: &[CanonicalFieldValue],
) -> Result<(), FormRuleStateError> {
    let matches = raw_inputs.fields().len() == canonical_inputs.len()
        && raw_inputs
            .fields()
            .iter()
            .zip(canonical_inputs)
            .all(|(raw, canonical)| {
                raw.field() == canonical.field() && raw.value() == canonical.raw()
            });
    if matches {
        return Ok(());
    }
    let reason = "canonical inputs do not exactly cover the raw snapshot".to_string();
    match finalization_id {
        Some(finalization_id) => Err(FormRuleStateError::CorruptFinalCopy {
            finalization_id,
            reason,
        }),
        None => Err(FormRuleStateError::InvalidFinalCopy(reason)),
    }
}

fn evaluation_context_digest(request: &EvaluationRequest) -> Sha256Digest {
    request.context_values().fingerprint().digest()
}

fn sha256_digest(value: &[u8]) -> Sha256Digest {
    Sha256Digest::from_bytes(Sha256::digest(value).into())
}

fn parse_final_copy_digest(
    finalization_id: i64,
    kind: &str,
    value: &str,
) -> Result<Sha256Digest, FormRuleStateError> {
    Sha256Digest::parse(value).map_err(|error| FormRuleStateError::CorruptFinalCopy {
        finalization_id,
        reason: format!("invalid {kind} SHA-256: {error}"),
    })
}

fn load_form_final_copy_from(
    conn: &Connection,
    finalization_id: i64,
) -> Result<Option<FormFinalCopy>, FormRuleStateError> {
    let row = conn
        .query_row(
            "SELECT id,
                    form_draft_id,
                    source_storage_revision,
                    rule_set_id,
                    rule_set_form_code,
                    rule_set_form_revision,
                    rule_set_official_package_version,
                    rule_set_sha256,
                    behavior_profile,
                    input_sha256,
                    context_sha256,
                    canonical_sha256,
                    derived_sha256,
                    report_sha256,
                    xml_sha256,
                    raw_snapshot_json,
                    canonical_json,
                    derived_json,
                    validation_report_json,
                    xml_payload,
                    payload_proof_json,
                    payload_proof_sha256,
                    created_at,
                    invalidated_at
             FROM form_finalizations
             WHERE id = ?1",
            [finalization_id],
            |row| {
                Ok(RawFinalCopyRow {
                    id: row.get(0)?,
                    form_draft_id: row.get(1)?,
                    source_storage_revision: row.get(2)?,
                    rule_set_id: row.get(3)?,
                    rule_set_form_code: row.get(4)?,
                    rule_set_form_revision: row.get(5)?,
                    rule_set_official_package_version: row.get(6)?,
                    rule_set_sha256: row.get(7)?,
                    behavior_profile: row.get(8)?,
                    input_sha256: row.get(9)?,
                    context_sha256: row.get(10)?,
                    canonical_sha256: row.get(11)?,
                    derived_sha256: row.get(12)?,
                    report_sha256: row.get(13)?,
                    xml_sha256: row.get(14)?,
                    raw_snapshot_json: row.get(15)?,
                    canonical_json: row.get(16)?,
                    derived_json: row.get(17)?,
                    validation_report_json: row.get(18)?,
                    xml_payload: row.get(19)?,
                    payload_proof_json: row.get(20)?,
                    payload_proof_sha256: row.get(21)?,
                    created_at: row.get(22)?,
                    invalidated_at: row.get(23)?,
                })
            },
        )
        .optional()?;
    row.map(validate_final_copy_row).transpose()
}

fn validate_final_copy_row(row: RawFinalCopyRow) -> Result<FormFinalCopy, FormRuleStateError> {
    let finalization_id = row.id;
    let source_storage_revision = u64::try_from(row.source_storage_revision).map_err(|_| {
        FormRuleStateError::CorruptFinalCopy {
            finalization_id,
            reason: format!(
                "negative source storage revision {}",
                row.source_storage_revision
            ),
        }
    })?;
    let identity = decode_identity(
        row.form_draft_id,
        Some(row.rule_set_id),
        row.rule_set_form_code,
        row.rule_set_form_revision,
        row.rule_set_official_package_version,
        Some(row.rule_set_sha256),
        Some(row.behavior_profile),
    )
    .map_err(|error| FormRuleStateError::CorruptFinalCopy {
        finalization_id,
        reason: error.to_string(),
    })?
    .ok_or_else(|| FormRuleStateError::CorruptFinalCopy {
        finalization_id,
        reason: "missing rule-set identity".to_string(),
    })?;

    let input_sha256 = parse_final_copy_digest(finalization_id, "input", &row.input_sha256)?;
    let context_sha256 = parse_final_copy_digest(finalization_id, "context", &row.context_sha256)?;
    let canonical_sha256 =
        parse_final_copy_digest(finalization_id, "canonical", &row.canonical_sha256)?;
    let derived_sha256 = parse_final_copy_digest(
        finalization_id,
        "derived",
        row.derived_sha256
            .as_deref()
            .ok_or_else(|| FormRuleStateError::CorruptFinalCopy {
                finalization_id,
                reason: "missing derived SHA-256".to_string(),
            })?,
    )?;
    let report_sha256 = parse_final_copy_digest(finalization_id, "report", &row.report_sha256)?;
    let xml_sha256 = parse_final_copy_digest(finalization_id, "XML", &row.xml_sha256)?;
    let payload_proof_json =
        row.payload_proof_json
            .as_deref()
            .ok_or_else(|| FormRuleStateError::CorruptFinalCopy {
                finalization_id,
                reason: "missing checked payload proof JSON".to_string(),
            })?;
    let payload_proof_sha256 = parse_final_copy_digest(
        finalization_id,
        "checked payload proof",
        row.payload_proof_sha256.as_deref().ok_or_else(|| {
            FormRuleStateError::CorruptFinalCopy {
                finalization_id,
                reason: "missing checked payload proof SHA-256".to_string(),
            }
        })?,
    )?;
    let checked_payload = CheckedFinalCopyPayload::from_stored_parts(
        payload_proof_json.to_string(),
        payload_proof_sha256,
        row.xml_payload.clone(),
    )
    .map_err(|error| FormRuleStateError::CorruptFinalCopy {
        finalization_id,
        reason: format!("checked payload proof is invalid: {error}"),
    })?;
    if checked_payload.xml_sha256() != xml_sha256 {
        return Err(FormRuleStateError::CorruptFinalCopy {
            finalization_id,
            reason: "checked payload XML digest does not match Final Copy XML digest".to_string(),
        });
    }

    for (kind, expected, actual) in [
        (
            "input",
            input_sha256,
            sha256_digest(row.raw_snapshot_json.as_bytes()),
        ),
        (
            "canonical",
            canonical_sha256,
            sha256_digest(row.canonical_json.as_bytes()),
        ),
        (
            "derived",
            derived_sha256,
            sha256_digest(row.derived_json.as_bytes()),
        ),
        (
            "report",
            report_sha256,
            sha256_digest(row.validation_report_json.as_bytes()),
        ),
        ("XML", xml_sha256, sha256_digest(row.xml_payload.as_bytes())),
    ] {
        if expected != actual {
            return Err(FormRuleStateError::CorruptFinalCopy {
                finalization_id,
                reason: format!("{kind} digest does not match stored payload"),
            });
        }
    }

    let raw_inputs: RawInputSnapshot =
        serde_json::from_str(&row.raw_snapshot_json).map_err(|error| {
            FormRuleStateError::CorruptFinalCopy {
                finalization_id,
                reason: format!("raw snapshot is invalid: {error}"),
            }
        })?;
    checked_payload
        .validate_input_snapshot(&raw_inputs)
        .map_err(|error| FormRuleStateError::CorruptFinalCopy {
            finalization_id,
            reason: format!("checked payload input binding is invalid: {error}"),
        })?;
    let canonical_inputs: Vec<CanonicalFieldValue> = serde_json::from_str(&row.canonical_json)
        .map_err(|error| FormRuleStateError::CorruptFinalCopy {
            finalization_id,
            reason: format!("canonical inputs are invalid: {error}"),
        })?;
    let stored_derived: StoredDerivedEvaluation =
        serde_json::from_str(&row.derived_json).map_err(|error| {
            FormRuleStateError::CorruptFinalCopy {
                finalization_id,
                reason: format!("derived evaluation is invalid: {error}"),
            }
        })?;
    if stored_derived.context_values.fingerprint().digest() != context_sha256 {
        return Err(FormRuleStateError::CorruptFinalCopy {
            finalization_id,
            reason: "context digest does not match stored context values".to_string(),
        });
    }
    let report: ValidationReport =
        serde_json::from_str(&row.validation_report_json).map_err(|error| {
            FormRuleStateError::CorruptFinalCopy {
                finalization_id,
                reason: format!("validation report is invalid: {error}"),
            }
        })?;

    let evaluation_json = serde_json::to_vec(&EvaluationWireRef {
        report: &report,
        canonical_inputs: &canonical_inputs,
        expected_outputs: &stored_derived.expected_outputs,
        derived_outputs: &stored_derived.derived_outputs,
    })
    .map_err(|error| FormRuleStateError::CorruptFinalCopy {
        finalization_id,
        reason: format!("evaluation could not be reconstructed: {error}"),
    })?;
    let evaluation: EvaluationResult =
        serde_json::from_slice(&evaluation_json).map_err(|error| {
            FormRuleStateError::CorruptFinalCopy {
                finalization_id,
                reason: format!("evaluation is incomplete or invalid: {error}"),
            }
        })?;
    checked_payload
        .validate_against_evaluation(&evaluation)
        .map_err(|error| FormRuleStateError::CorruptFinalCopy {
            finalization_id,
            reason: format!("checked payload evaluation binding is invalid: {error}"),
        })?;

    if identity.behavior_profile != BehaviorProfile::FilingSafe
        || evaluation.context().profile() != BehaviorProfile::FilingSafe
        || !evaluation.is_valid()
        || evaluation.context().phase() != ValidationPhase::FinalCopy
        || evaluation.input_revision().get() != source_storage_revision
        || evaluation.context_fingerprint().digest() != context_sha256
        || checked_payload.context_fingerprint().digest() != context_sha256
        || FormRuleIdentity::from_rule_set(evaluation.rule_set(), evaluation.context().profile())
            != identity
    {
        return Err(FormRuleStateError::CorruptFinalCopy {
            finalization_id,
            reason: "evaluation metadata does not match Final Copy identity and hashes".to_string(),
        });
    }
    ensure_raw_matches_canonical(
        Some(finalization_id),
        &raw_inputs,
        evaluation.canonical_inputs(),
    )?;

    Ok(FormFinalCopy {
        id: row.id,
        form_draft_id: row.form_draft_id,
        source_storage_revision,
        identity,
        input_sha256,
        context_sha256,
        canonical_sha256,
        derived_sha256,
        report_sha256,
        xml_sha256,
        payload_proof_sha256,
        raw_snapshot_json: row.raw_snapshot_json,
        canonical_json: row.canonical_json,
        derived_json: row.derived_json,
        validation_report_json: row.validation_report_json,
        xml_payload: row.xml_payload,
        checked_payload,
        context_values: stored_derived.context_values,
        evaluation,
        created_at: row.created_at,
        invalidated_at: row.invalidated_at,
    })
}

#[cfg(test)]
mod tests {
    use bir_rules::{
        CalculationId, CanonicalValue, ContextValue, ContextValueId, EvaluationExpectation,
        EvaluationOutput, FieldId, FieldInstance, InputRevision, OutputId, RawFieldValue, RawValue,
        SerializedOccurrence, ValidationContext, XmlKey,
    };

    use super::*;
    use crate::form_rules::{
        CheckedFinalCopyPayloadError, FinalCopyFieldCoverage, SubmissionPreflightError,
        TrustedEvaluationError, preflight_active_form_submission,
    };

    const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn database_with_draft() -> (Database, i64) {
        let database = Database::open_in_memory_for_tests().unwrap();
        database
            .conn
            .execute(
                "INSERT INTO form_drafts (
                    tin, form_code, taxable_year, quarter, period_key, status, data_json
                 ) VALUES (?1, '2550Q', 2026, 1, 'Q1', 'Draft', '{}')",
                ["123456789000"],
            )
            .unwrap();
        let draft_id = database.conn.last_insert_rowid();
        (database, draft_id)
    }

    fn rule_set(rule_set_id: &str, sha256: &str) -> FormRevisionKey {
        exact_rule_set(rule_set_id, "2550Q", "2024-04-01", "7.9.6.0", sha256)
    }

    fn exact_rule_set(
        rule_set_id: &str,
        form_code: &str,
        form_revision: &str,
        official_package_version: &str,
        sha256: &str,
    ) -> FormRevisionKey {
        FormRevisionKey::parse(
            rule_set_id,
            form_code,
            form_revision,
            official_package_version,
            sha256,
        )
        .unwrap()
    }

    fn identity(rule_set_id: &str, sha256: &str, profile: BehaviorProfile) -> FormRuleIdentity {
        FormRuleIdentity::from_rule_set(&rule_set(rule_set_id, sha256), profile)
    }

    fn raw_snapshot(text: &str) -> RawInputSnapshot {
        RawInputSnapshot::try_new(
            Vec::new(),
            vec![RawFieldValue::new(
                FieldInstance::singleton(FieldId::parse("amount").unwrap()),
                RawValue::Text(text.to_string()),
            )],
        )
        .unwrap()
    }

    fn exact_editor_json(text: &str) -> String {
        serde_json::to_string_pretty(&raw_snapshot(text)).unwrap()
    }

    fn trusted_evaluation(
        rule_set: FormRevisionKey,
        profile: BehaviorProfile,
        storage_revision: u64,
        raw_text: &str,
        phase: ValidationPhase,
    ) -> TrustedEvaluation {
        trusted_evaluation_with_context(rule_set, profile, storage_revision, raw_text, phase, "p-1")
    }

    fn trusted_evaluation_with_context(
        rule_set: FormRevisionKey,
        profile: BehaviorProfile,
        storage_revision: u64,
        raw_text: &str,
        phase: ValidationPhase,
        profile_version: &str,
    ) -> TrustedEvaluation {
        let snapshot = raw_snapshot(raw_text);
        let context_values = vec![ContextValue::new(
            ContextValueId::parse("profile-version").unwrap(),
            CanonicalValue::Text(profile_version.to_string()),
        )];
        let request = EvaluationRequest::try_new(
            rule_set,
            ValidationContext::new(phase, profile),
            InputRevision::new(storage_revision),
            context_values,
            snapshot.repeated_group_instances().to_vec(),
            snapshot.fields().to_vec(),
        )
        .unwrap();
        let raw = RawValue::Text(raw_text.to_string());
        let output = EvaluationOutput::new(
            vec![CanonicalFieldValue::new(
                FieldInstance::singleton(FieldId::parse("amount").unwrap()),
                raw.clone(),
                CanonicalValue::Text(raw_text.to_string()),
            )],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        let expectation = EvaluationExpectation::try_new(Vec::new(), Vec::new()).unwrap();
        let result = EvaluationResult::try_new(&request, &expectation, output).unwrap();
        TrustedEvaluation::try_from_parts_for_test(request, result).unwrap()
    }

    fn checked_payload(
        trusted_evaluation: &TrustedEvaluation,
        semantic_value: &str,
    ) -> CheckedFinalCopyPayload {
        let encoded_value = urlencoding::encode(semantic_value).into_owned();
        let xml_payload = format!("<div>amount={encoded_value}amount=</div>");
        let coverage = vec![FinalCopyFieldCoverage::canonical_field(
            XmlKey::parse("amount").unwrap(),
            SerializedOccurrence::new(1).unwrap(),
            FieldInstance::singleton(FieldId::parse("amount").unwrap()),
            CanonicalValue::Text(semantic_value.to_string()),
            encoded_value,
        )];
        CheckedFinalCopyPayload::try_from_coverage_for_test(
            trusted_evaluation,
            coverage,
            &xml_payload,
        )
        .unwrap()
    }

    #[test]
    fn persisted_rule_state_debug_redacts_editor_plaintext() {
        let state = FormRuleState {
            form_draft_id: 42,
            editor_state_json: Some(r#"{"taxpayer":"LEAK-ME-123"}"#.to_string()),
            storage_revision: 7,
            identity: None,
            active_finalization_id: None,
        };

        let debug = format!("{state:?}");
        assert!(!debug.contains("LEAK-ME-123"));
        assert!(!debug.contains("taxpayer"));
        assert!(debug.contains("editor_state_byte_len"));
    }

    #[test]
    fn final_copy_debug_redacts_all_persisted_plaintext_views() {
        let (database, draft_id) = database_with_draft();
        let rule_set = rule_set("2550q-v2024", SHA_A);
        let identity = FormRuleIdentity::from_rule_set(&rule_set, BehaviorProfile::FilingSafe);
        let sentinel = "LEAK-ME-123";
        database
            .save_form_rule_editor_state(draft_id, 0, &identity, &exact_editor_json(sentinel))
            .unwrap();
        let trusted = trusted_evaluation(
            rule_set,
            BehaviorProfile::FilingSafe,
            1,
            sentinel,
            ValidationPhase::FinalCopy,
        );
        let checked_payload = checked_payload(&trusted, sentinel);
        let final_copy = database
            .create_form_final_copy(draft_id, 1, &trusted, &checked_payload)
            .unwrap();

        let debug = format!("{final_copy:?}");
        assert!(!debug.contains(sentinel));
        assert!(!debug.contains("<div>"));
        assert!(debug.contains("raw_snapshot_byte_len"));
        assert!(debug.contains("xml_payload_byte_len"));
    }

    #[test]
    fn submission_preflight_fails_closed_on_empty_reviewed_registry_without_mutation() {
        let (database, draft_id) = database_with_draft();
        let pinned_rule_set = rule_set("2550q-v2024-safe", SHA_A);
        let pinned_identity =
            FormRuleIdentity::from_rule_set(&pinned_rule_set, BehaviorProfile::FilingSafe);
        let editor_json = exact_editor_json("10");
        database
            .save_form_rule_editor_state(draft_id, 0, &pinned_identity, &editor_json)
            .unwrap();
        let trusted = trusted_evaluation(
            pinned_rule_set,
            BehaviorProfile::FilingSafe,
            1,
            "10",
            ValidationPhase::FinalCopy,
        );
        let payload = checked_payload(&trusted, "10");
        database
            .create_form_final_copy(draft_id, 1, &trusted, &payload)
            .unwrap();

        let state_before = database.load_form_rule_state(draft_id).unwrap().unwrap();
        let final_copy_before = database
            .load_active_form_final_copy(draft_id)
            .unwrap()
            .unwrap();
        let total_changes_before: i64 = database
            .conn
            .query_row("SELECT total_changes()", [], |row| row.get(0))
            .unwrap();

        assert!(matches!(
            preflight_active_form_submission(&database, draft_id),
            Err(SubmissionPreflightError::TrustedEvaluation(
                TrustedEvaluationError::Registry(bir_rules::RegistryError::NotFound { .. })
            ))
        ));

        let total_changes_after: i64 = database
            .conn
            .query_row("SELECT total_changes()", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total_changes_after, total_changes_before);
        assert_eq!(
            database.load_form_rule_state(draft_id).unwrap().unwrap(),
            state_before
        );
        assert_eq!(
            database
                .load_active_form_final_copy(draft_id)
                .unwrap()
                .unwrap(),
            final_copy_before
        );
    }

    #[test]
    fn editor_state_round_trips_exactly_and_uses_cas() {
        let (database, draft_id) = database_with_draft();
        let pinned_identity =
            identity("2550q-v2024", SHA_A, BehaviorProfile::OfficialCompatibility);
        let exact_json = exact_editor_json("not-a-number");

        let saved = database
            .save_form_rule_editor_state(draft_id, 0, &pinned_identity, &exact_json)
            .unwrap();
        assert_eq!(saved.storage_revision, 1);
        assert_eq!(
            saved.editor_state_json.as_deref(),
            Some(exact_json.as_str())
        );
        assert_eq!(saved.identity.as_ref(), Some(&pinned_identity));
        assert_eq!(
            database.load_form_rule_state(draft_id).unwrap().unwrap(),
            saved
        );

        assert!(matches!(
            database.save_form_rule_editor_state(draft_id, 0, &pinned_identity, &exact_json),
            Err(FormRuleStateError::StorageRevisionConflict {
                expected: 0,
                actual: 1,
                ..
            })
        ));

        let other_identity = identity("2550q-v2024-new", SHA_B, BehaviorProfile::FilingSafe);
        assert!(matches!(
            database.save_form_rule_editor_state(draft_id, 1, &other_identity, &exact_json),
            Err(FormRuleStateError::IdentityMismatch { .. })
        ));
        let unchanged = database.load_form_rule_state(draft_id).unwrap().unwrap();
        assert_eq!(unchanged.storage_revision, 1);
        assert_eq!(unchanged.identity.as_ref(), Some(&pinned_identity));
    }

    #[test]
    fn exact_rule_identity_rejects_every_projected_identity_collision() {
        let (database, draft_id) = database_with_draft();
        let pinned_rule_set = rule_set("2550q-v2024", SHA_A);
        let pinned_identity =
            FormRuleIdentity::from_rule_set(&pinned_rule_set, BehaviorProfile::FilingSafe);
        let editor_json = exact_editor_json("12");
        let saved = database
            .save_form_rule_editor_state(draft_id, 0, &pinned_identity, &editor_json)
            .unwrap();
        assert_eq!(saved.identity.as_ref(), Some(&pinned_identity));
        assert_eq!(saved.identity.as_ref().unwrap().rule_set, pinned_rule_set);

        let projected_collisions = [
            exact_rule_set("2550q-v2024", "1701Q", "2024-04-01", "7.9.6.0", SHA_A),
            exact_rule_set("2550q-v2024", "2550Q", "2024-07-01", "7.9.6.0", SHA_A),
            exact_rule_set("2550q-v2024", "2550Q", "2024-04-01", "7.9.7.0", SHA_A),
        ];
        for colliding_rule_set in &projected_collisions {
            assert_eq!(
                colliding_rule_set.rule_set_id(),
                pinned_rule_set.rule_set_id()
            );
            assert_eq!(
                colliding_rule_set.source_set_sha256(),
                pinned_rule_set.source_set_sha256()
            );
            let colliding_identity =
                FormRuleIdentity::from_rule_set(colliding_rule_set, BehaviorProfile::FilingSafe);
            assert_ne!(colliding_identity, pinned_identity);
            assert!(matches!(
                database.save_form_rule_editor_state(
                    draft_id,
                    1,
                    &colliding_identity,
                    &editor_json,
                ),
                Err(FormRuleStateError::IdentityMismatch { .. })
            ));
        }

        let switched_rule_set = projected_collisions[2].clone();
        let switched_evaluation = trusted_evaluation(
            switched_rule_set,
            BehaviorProfile::FilingSafe,
            1,
            "12",
            ValidationPhase::FinalCopy,
        );
        let switched_payload = checked_payload(&switched_evaluation, "12");
        assert!(matches!(
            database.create_form_final_copy(draft_id, 1, &switched_evaluation, &switched_payload,),
            Err(FormRuleStateError::IdentityMismatch { .. })
        ));
        assert!(
            database
                .load_active_form_final_copy(draft_id)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn explicit_migration_can_change_full_key_with_same_projected_identity() {
        let (database, draft_id) = database_with_draft();
        let from_rule_set = rule_set("2550q-v2024", SHA_A);
        let to_rule_set = exact_rule_set("2550q-v2024", "1701Q", "2024-07-01", "7.9.7.0", SHA_A);
        let from_identity =
            FormRuleIdentity::from_rule_set(&from_rule_set, BehaviorProfile::FilingSafe);
        let to_identity =
            FormRuleIdentity::from_rule_set(&to_rule_set, BehaviorProfile::FilingSafe);
        let editor_json = exact_editor_json("12");
        database
            .save_form_rule_editor_state(draft_id, 0, &from_identity, &editor_json)
            .unwrap();

        let migrated = database
            .migrate_form_rule_set(
                draft_id,
                1,
                &to_identity,
                &editor_json,
                r#"{"review":"accepted exact-key change"}"#,
            )
            .unwrap();
        assert_eq!(migrated.storage_revision, 2);
        assert_eq!(migrated.identity.as_ref(), Some(&to_identity));

        let audit: (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
        ) = database
            .conn
            .query_row(
                "SELECT from_rule_set_form_code,
                        from_rule_set_form_revision,
                        from_rule_set_official_package_version,
                        from_rule_set_sha256,
                        to_rule_set_form_code,
                        to_rule_set_form_revision,
                        to_rule_set_official_package_version,
                        to_rule_set_sha256
                 FROM form_rule_migrations
                 WHERE form_draft_id = ?1",
                [draft_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            audit,
            (
                "2550Q".to_string(),
                "2024-04-01".to_string(),
                "7.9.6.0".to_string(),
                SHA_A.to_string(),
                "1701Q".to_string(),
                "2024-07-01".to_string(),
                "7.9.7.0".to_string(),
                SHA_A.to_string(),
            )
        );
    }

    #[test]
    fn legacy_projected_identity_fails_closed_without_guessing_full_key() {
        let (database, draft_id) = database_with_draft();
        database
            .conn
            .execute(
                "UPDATE form_drafts
                 SET editor_state_json = ?1,
                     storage_revision = 1,
                     rule_set_id = '2550q-v2024',
                     rule_set_sha256 = ?2,
                     behavior_profile = 'filing_safe'
                 WHERE id = ?3",
                params![exact_editor_json("12"), SHA_A, draft_id],
            )
            .unwrap();

        let error = database.load_form_rule_state(draft_id).unwrap_err();
        assert!(matches!(
            &error,
            FormRuleStateError::CorruptState {
                draft_id: corrupt_draft_id,
                ..
            } if *corrupt_draft_id == draft_id
        ));
        assert!(error.to_string().contains("reviewed migration is required"));
    }

    #[test]
    fn reviewed_legacy_identity_repair_completes_key_and_invalidates_old_final_copy() {
        let (database, draft_id) = database_with_draft();
        let rule_set = rule_set("2550q-v2024", SHA_A);
        let identity = FormRuleIdentity::from_rule_set(&rule_set, BehaviorProfile::FilingSafe);
        let editor_json = exact_editor_json("12");
        database
            .save_form_rule_editor_state(draft_id, 0, &identity, &editor_json)
            .unwrap();
        let trusted = trusted_evaluation(
            rule_set,
            BehaviorProfile::FilingSafe,
            1,
            "12",
            ValidationPhase::FinalCopy,
        );
        let payload = checked_payload(&trusted, "12");
        let final_copy = database
            .create_form_final_copy(draft_id, 1, &trusted, &payload)
            .unwrap();

        database
            .conn
            .execute(
                "UPDATE form_drafts
                 SET rule_set_form_code = NULL,
                     rule_set_form_revision = NULL,
                     rule_set_official_package_version = NULL
                 WHERE id = ?1",
                [draft_id],
            )
            .unwrap();
        database
            .conn
            .execute(
                "UPDATE form_finalizations
                 SET rule_set_form_code = NULL,
                     rule_set_form_revision = NULL,
                     rule_set_official_package_version = NULL
                 WHERE id = ?1",
                [final_copy.id],
            )
            .unwrap();
        assert!(database.load_form_rule_state(draft_id).is_err());

        let repaired = database
            .repair_legacy_projected_form_rule_identity(
                draft_id,
                1,
                &identity,
                r#"{"reviewer":"manual-test","evidence":"pinned-package"}"#,
            )
            .unwrap();
        assert_eq!(repaired.storage_revision, 2);
        assert_eq!(repaired.identity.as_ref(), Some(&identity));
        assert_eq!(
            repaired.editor_state_json.as_deref(),
            Some(editor_json.as_str())
        );
        assert_eq!(repaired.active_finalization_id, None);

        let invalidated_at: Option<String> = database
            .conn
            .query_row(
                "SELECT invalidated_at FROM form_finalizations WHERE id = ?1",
                [final_copy.id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(invalidated_at.is_some());
        let audit: (
            Option<String>,
            Option<String>,
            Option<String>,
            String,
            String,
            String,
            String,
            String,
            String,
        ) = database
            .conn
            .query_row(
                "SELECT from_rule_set_form_code,
                        from_rule_set_form_revision,
                        from_rule_set_official_package_version,
                        to_rule_set_form_code,
                        to_rule_set_form_revision,
                        to_rule_set_official_package_version,
                        from_snapshot_sha256,
                        to_snapshot_sha256,
                        diff_json
                 FROM form_rule_migrations
                 WHERE form_draft_id = ?1",
                [draft_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!((audit.0, audit.1, audit.2), (None, None, None));
        assert_eq!(
            (audit.3, audit.4, audit.5),
            (
                "2550Q".to_string(),
                "2024-04-01".to_string(),
                "7.9.6.0".to_string(),
            )
        );
        assert_eq!(audit.6, audit.7);
        let audit_details: MigrationAudit = serde_json::from_str(&audit.8).unwrap();
        assert_eq!(
            audit_details.details["kind"],
            "legacy-projected-identity-repair"
        );

        assert!(matches!(
            database.repair_legacy_projected_form_rule_identity(
                draft_id,
                2,
                &identity,
                r#"{"reviewer":"again"}"#,
            ),
            Err(FormRuleStateError::LegacyIdentityRepairNotRequired { .. })
        ));
    }

    #[test]
    fn legacy_identity_repair_rejects_unreviewed_projected_mismatch() {
        let (database, draft_id) = database_with_draft();
        database
            .conn
            .execute(
                "UPDATE form_drafts
                 SET editor_state_json = ?1,
                     storage_revision = 1,
                     rule_set_id = '2550q-v2024',
                     rule_set_sha256 = ?2,
                     behavior_profile = 'filing_safe'
                 WHERE id = ?3",
                params![exact_editor_json("12"), SHA_A, draft_id],
            )
            .unwrap();
        let wrong_form = exact_rule_set("2550q-v2024", "1701Q", "2024-04-01", "7.9.6.0", SHA_A);
        let wrong_identity =
            FormRuleIdentity::from_rule_set(&wrong_form, BehaviorProfile::FilingSafe);

        assert!(matches!(
            database.repair_legacy_projected_form_rule_identity(
                draft_id,
                1,
                &wrong_identity,
                r#"{"reviewer":"manual-test"}"#,
            ),
            Err(FormRuleStateError::LegacyIdentityRepairMismatch { .. })
        ));
        let exact_components: (Option<String>, Option<String>, Option<String>) = database
            .conn
            .query_row(
                "SELECT rule_set_form_code,
                        rule_set_form_revision,
                        rule_set_official_package_version
                 FROM form_drafts
                 WHERE id = ?1",
                [draft_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(exact_components, (None, None, None));
    }

    #[test]
    fn explicit_migration_changes_identity_and_appends_audit() {
        let (database, draft_id) = database_with_draft();
        let from_identity = identity("2550q-v2024", SHA_A, BehaviorProfile::OfficialCompatibility);
        let to_identity = identity("2550q-v2024-safe", SHA_B, BehaviorProfile::FilingSafe);
        let original_json = exact_editor_json("12x");
        let migrated_json = exact_editor_json("12");
        database
            .save_form_rule_editor_state(draft_id, 0, &from_identity, &original_json)
            .unwrap();

        let migrated = database
            .migrate_form_rule_set(
                draft_id,
                1,
                &to_identity,
                &migrated_json,
                r#"{"review":"accepted after comparison"}"#,
            )
            .unwrap();
        assert_eq!(migrated.storage_revision, 2);
        assert_eq!(migrated.identity.as_ref(), Some(&to_identity));
        assert_eq!(
            migrated.editor_state_json.as_deref(),
            Some(migrated_json.as_str())
        );

        let audit: (String, String, String, String, String, String, String) = database
            .conn
            .query_row(
                "SELECT from_rule_set_id,
                        from_rule_set_sha256,
                        to_rule_set_id,
                        to_rule_set_sha256,
                        from_snapshot_sha256,
                        to_snapshot_sha256,
                        diff_json
                 FROM form_rule_migrations
                 WHERE form_draft_id = ?1",
                [draft_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(audit.0, "2550q-v2024");
        assert_eq!(audit.1, SHA_A);
        assert_eq!(audit.2, "2550q-v2024-safe");
        assert_eq!(audit.3, SHA_B);
        assert_eq!(audit.4, sha256_digest(original_json.as_bytes()).to_hex());
        assert_eq!(audit.5, sha256_digest(migrated_json.as_bytes()).to_hex());
        let details: MigrationAudit = serde_json::from_str(&audit.6).unwrap();
        assert_eq!(details.schema, MIGRATION_AUDIT_SCHEMA);
        assert_eq!(
            details.from_behavior_profile,
            BehaviorProfile::OfficialCompatibility
        );
        assert_eq!(details.to_behavior_profile, BehaviorProfile::FilingSafe);
    }

    #[test]
    fn final_copy_is_bound_to_current_revision_identity_inputs_and_context() {
        let (database, draft_id) = database_with_draft();
        let rule_set = rule_set("2550q-v2024", SHA_A);
        let identity = FormRuleIdentity::from_rule_set(&rule_set, BehaviorProfile::FilingSafe);
        let editor_json = exact_editor_json("not-a-number");
        database
            .save_form_rule_editor_state(draft_id, 0, &identity, &editor_json)
            .unwrap();
        let trusted = trusted_evaluation(
            rule_set,
            BehaviorProfile::FilingSafe,
            1,
            "not-a-number",
            ValidationPhase::FinalCopy,
        );
        let checked_payload = checked_payload(&trusted, "not-a-number");

        assert!(matches!(
            database.create_form_final_copy(draft_id, 0, &trusted, &checked_payload),
            Err(FormRuleStateError::StorageRevisionConflict { .. })
        ));
        let final_copy = database
            .create_form_final_copy(draft_id, 1, &trusted, &checked_payload)
            .unwrap();
        let finalization_id = final_copy.id;
        let original_derived_json = final_copy.derived_json.clone();
        assert_eq!(final_copy.source_storage_revision, 1);
        assert_eq!(final_copy.identity, identity);
        assert_eq!(final_copy.raw_snapshot_json, editor_json);
        assert_eq!(
            final_copy.input_sha256,
            sha256_digest(final_copy.raw_snapshot_json.as_bytes())
        );
        assert_eq!(
            final_copy.context_sha256,
            trusted.context_values().fingerprint().digest()
        );
        assert_eq!(
            final_copy.derived_sha256,
            sha256_digest(final_copy.derived_json.as_bytes())
        );
        assert_eq!(
            final_copy.payload_proof_sha256,
            sha256_digest(final_copy.checked_payload.proof_json().as_bytes())
        );
        assert_eq!(
            final_copy.xml_payload,
            final_copy.checked_payload.xml_payload()
        );
        assert!(final_copy.evaluation.is_valid());

        assert_eq!(
            database
                .load_active_form_final_copy(draft_id)
                .unwrap()
                .unwrap(),
            final_copy
        );
        assert!(matches!(
            database.create_form_final_copy(draft_id, 1, &trusted, &checked_payload),
            Err(FormRuleStateError::ActiveFinalCopyExists { .. })
        ));

        database
            .conn
            .execute(
                "UPDATE form_finalizations
                 SET rule_set_form_revision = '2024-07-01'
                 WHERE id = ?1",
                [finalization_id],
            )
            .unwrap();
        assert!(matches!(
            database.load_form_final_copy(finalization_id),
            Err(FormRuleStateError::CorruptFinalCopy { .. })
        ));
        database
            .conn
            .execute(
                "UPDATE form_finalizations
                 SET rule_set_form_revision = '2024-04-01'
                 WHERE id = ?1",
                [finalization_id],
            )
            .unwrap();

        database
            .conn
            .execute(
                "UPDATE form_finalizations SET derived_json = '{}' WHERE id = ?1",
                [finalization_id],
            )
            .unwrap();
        assert!(matches!(
            database.load_form_final_copy(finalization_id),
            Err(FormRuleStateError::CorruptFinalCopy { .. })
        ));

        let mut changed_context: StoredDerivedEvaluation =
            serde_json::from_str(&original_derived_json).unwrap();
        changed_context.context_values =
            bir_rules::ContextValueSnapshot::try_new(vec![ContextValue::new(
                ContextValueId::parse("profile-version").unwrap(),
                CanonicalValue::Text("p-2".to_string()),
            )])
            .unwrap();
        let changed_context_json = serde_json::to_string(&changed_context).unwrap();
        let changed_context_derived_sha = sha256_digest(changed_context_json.as_bytes()).to_hex();
        database
            .conn
            .execute(
                "UPDATE form_finalizations
                 SET derived_json = ?1, derived_sha256 = ?2
                 WHERE id = ?3",
                rusqlite::params![
                    changed_context_json,
                    changed_context_derived_sha,
                    finalization_id
                ],
            )
            .unwrap();
        assert!(matches!(
            database.load_form_final_copy(finalization_id),
            Err(FormRuleStateError::CorruptFinalCopy { .. })
        ));
    }

    #[test]
    fn checked_payload_requires_an_exact_xml_manifest_bijection() {
        let trusted = trusted_evaluation(
            rule_set("2550q-v2024", SHA_A),
            BehaviorProfile::FilingSafe,
            1,
            "12",
            ValidationPhase::FinalCopy,
        );
        let canonical = |key: &str, occurrence: u16, semantic_value: &str, encoded_value: &str| {
            FinalCopyFieldCoverage::canonical_field(
                XmlKey::parse(key).unwrap(),
                SerializedOccurrence::new(occurrence).unwrap(),
                FieldInstance::singleton(FieldId::parse("amount").unwrap()),
                CanonicalValue::Text(semantic_value.to_string()),
                encoded_value.to_string(),
            )
        };

        assert!(matches!(
            CheckedFinalCopyPayload::try_new(
                &trusted,
                vec![canonical("amount", 1, "12", "12")],
                "<div>amount=12amount=</div>".to_string(),
            ),
            Err(CheckedFinalCopyPayloadError::MissingSerializationContract)
        ));
        assert!(matches!(
            CheckedFinalCopyPayload::try_from_coverage_for_test(
                &trusted,
                vec![canonical("amount", 1, "12", "12")],
                "",
            ),
            Err(CheckedFinalCopyPayloadError::EmptyPayload)
        ));
        assert!(matches!(
            CheckedFinalCopyPayload::try_from_coverage_for_test(
                &trusted,
                vec![canonical("amount", 1, "12", "12")],
                "<div>amount=12amount=</div><div>amount=12amount=</div>",
            ),
            Err(CheckedFinalCopyPayloadError::ExtraXmlKey { .. })
        ));
        assert!(matches!(
            CheckedFinalCopyPayload::try_from_coverage_for_test(
                &trusted,
                vec![canonical("amount", 1, "12", "12")],
                "<div>other=12other=</div>",
            ),
            Err(CheckedFinalCopyPayloadError::MissingXmlKey { .. })
        ));
        assert!(matches!(
            CheckedFinalCopyPayload::try_from_coverage_for_test(
                &trusted,
                vec![canonical("amount", 1, "12", "12")],
                "<div>amount=12amount=</div><div>other=12other=</div>",
            ),
            Err(CheckedFinalCopyPayloadError::ExtraXmlKey { .. })
        ));
        assert!(matches!(
            CheckedFinalCopyPayload::try_from_coverage_for_test(
                &trusted,
                vec![canonical("amount", 1, "12", "13")],
                "<div>amount=12amount=</div>",
            ),
            Err(CheckedFinalCopyPayloadError::EncodedValueMismatch { .. })
        ));
        assert!(matches!(
            CheckedFinalCopyPayload::try_from_coverage_for_test(
                &trusted,
                vec![canonical("amount", 2, "12", "12")],
                "<div>amount=12amount=</div>",
            ),
            Err(CheckedFinalCopyPayloadError::OccurrenceMismatch {
                expected: 1,
                actual: 2,
                ..
            })
        ));
        assert!(matches!(
            CheckedFinalCopyPayload::try_from_coverage_for_test(
                &trusted,
                vec![
                    canonical("amount", 1, "12", "12"),
                    FinalCopyFieldCoverage::reviewed_constant(
                        XmlKey::parse("amount").unwrap(),
                        SerializedOccurrence::new(1).unwrap(),
                        FieldId::parse("reviewed-amount").unwrap(),
                        CanonicalValue::Text("12".to_string()),
                        "12".to_string(),
                    ),
                ],
                "<div>amount=12amount=</div>",
            ),
            Err(CheckedFinalCopyPayloadError::DuplicateManifestKey { .. })
        ));

        let repeated_coverage = vec![
            canonical("amount", 1, "12", "12"),
            FinalCopyFieldCoverage::reviewed_constant(
                XmlKey::parse("amount").unwrap(),
                SerializedOccurrence::new(2).unwrap(),
                FieldId::parse("reviewed-second-amount").unwrap(),
                CanonicalValue::Text("13".to_string()),
                "13".to_string(),
            ),
        ];
        CheckedFinalCopyPayload::try_from_coverage_for_test(
            &trusted,
            repeated_coverage.clone(),
            "<div>amount=12amount=</div><div>amount=13amount=</div>",
        )
        .unwrap();
        assert!(matches!(
            CheckedFinalCopyPayload::try_from_coverage_for_test(
                &trusted,
                repeated_coverage,
                "<div>amount=12amount=</div>",
            ),
            Err(CheckedFinalCopyPayloadError::MissingXmlKey { .. })
        ));

        let ordered_coverage = vec![
            canonical("amount", 1, "12", "12"),
            FinalCopyFieldCoverage::reviewed_constant(
                XmlKey::parse("other").unwrap(),
                SerializedOccurrence::new(1).unwrap(),
                FieldId::parse("reviewed-other").unwrap(),
                CanonicalValue::Text("13".to_string()),
                "13".to_string(),
            ),
        ];
        assert!(matches!(
            CheckedFinalCopyPayload::try_from_coverage_for_test(
                &trusted,
                ordered_coverage,
                "<div>other=13other=</div><div>amount=12amount=</div>",
            ),
            Err(CheckedFinalCopyPayloadError::XmlOrderMismatch { .. })
        ));
    }

    #[test]
    fn checked_payload_closes_semantic_value_sources() {
        let trusted = trusted_evaluation(
            rule_set("2550q-v2024", SHA_A),
            BehaviorProfile::FilingSafe,
            1,
            "12",
            ValidationPhase::FinalCopy,
        );
        let occurrence = || SerializedOccurrence::new(1).unwrap();

        let missing_field = FinalCopyFieldCoverage::canonical_field(
            XmlKey::parse("missing").unwrap(),
            occurrence(),
            FieldInstance::singleton(FieldId::parse("missing").unwrap()),
            CanonicalValue::Text("12".to_string()),
            "12".to_string(),
        );
        assert!(matches!(
            CheckedFinalCopyPayload::try_from_coverage_for_test(
                &trusted,
                vec![missing_field],
                "<div>missing=12missing=</div>",
            ),
            Err(CheckedFinalCopyPayloadError::ValueSourceMultiplicity { actual: 0, .. })
        ));

        let missing_derived = FinalCopyFieldCoverage::derived_output(
            XmlKey::parse("derived").unwrap(),
            occurrence(),
            CalculationId::parse("tax-due").unwrap(),
            OutputId::parse("total").unwrap(),
            CanonicalValue::Text("12".to_string()),
            "12".to_string(),
        );
        assert!(matches!(
            CheckedFinalCopyPayload::try_from_coverage_for_test(
                &trusted,
                vec![missing_derived],
                "<div>derived=12derived=</div>",
            ),
            Err(CheckedFinalCopyPayloadError::ValueSourceMultiplicity { actual: 0, .. })
        ));

        let mismatched_semantic = FinalCopyFieldCoverage::canonical_field(
            XmlKey::parse("amount").unwrap(),
            occurrence(),
            FieldInstance::singleton(FieldId::parse("amount").unwrap()),
            CanonicalValue::Text("13".to_string()),
            "13".to_string(),
        );
        assert!(matches!(
            CheckedFinalCopyPayload::try_from_coverage_for_test(
                &trusted,
                vec![mismatched_semantic],
                "<div>amount=13amount=</div>",
            ),
            Err(CheckedFinalCopyPayloadError::SemanticValueMismatch { .. })
        ));

        let duplicate_source = vec![
            FinalCopyFieldCoverage::canonical_field(
                XmlKey::parse("amount-a").unwrap(),
                occurrence(),
                FieldInstance::singleton(FieldId::parse("amount").unwrap()),
                CanonicalValue::Text("12".to_string()),
                "12".to_string(),
            ),
            FinalCopyFieldCoverage::canonical_field(
                XmlKey::parse("amount-b").unwrap(),
                occurrence(),
                FieldInstance::singleton(FieldId::parse("amount").unwrap()),
                CanonicalValue::Text("12".to_string()),
                "12".to_string(),
            ),
        ];
        assert!(matches!(
            CheckedFinalCopyPayload::try_from_coverage_for_test(
                &trusted,
                duplicate_source,
                "<div>amount-a=12amount-a=</div><div>amount-b=12amount-b=</div>",
            ),
            Err(CheckedFinalCopyPayloadError::DuplicateValueSource { .. })
        ));

        let reviewed_values = vec![
            FinalCopyFieldCoverage::reviewed_constant(
                XmlKey::parse("constant").unwrap(),
                occurrence(),
                FieldId::parse("constant-id").unwrap(),
                CanonicalValue::Text("BIR".to_string()),
                "BIR".to_string(),
            ),
            FinalCopyFieldCoverage::reviewed_default(
                XmlKey::parse("default").unwrap(),
                occurrence(),
                FieldId::parse("default-id").unwrap(),
                CanonicalValue::Text("0".to_string()),
                "0".to_string(),
            ),
        ];
        CheckedFinalCopyPayload::try_from_coverage_for_test(
            &trusted,
            reviewed_values,
            "<div>constant=BIRconstant=</div><div>default=0default=</div>",
        )
        .unwrap();
    }

    #[test]
    fn checked_payload_binds_literal_percent_as_the_exact_encoded_body() {
        let trusted = trusted_evaluation(
            rule_set("2550q-v2024", SHA_A),
            BehaviorProfile::FilingSafe,
            1,
            "12%",
            ValidationPhase::FinalCopy,
        );
        let coverage = vec![FinalCopyFieldCoverage::canonical_field(
            XmlKey::parse("amount").unwrap(),
            SerializedOccurrence::new(1).unwrap(),
            FieldInstance::singleton(FieldId::parse("amount").unwrap()),
            CanonicalValue::Text("12%".to_string()),
            "12%".to_string(),
        )];

        let payload = CheckedFinalCopyPayload::try_from_coverage_for_test(
            &trusted,
            coverage,
            "<div>amount=12%amount=</div>",
        )
        .unwrap();
        assert_eq!(
            payload.serializer_version(),
            "bir-pseudo-xml-encoded-occurrences-v2"
        );
        assert_eq!(payload.xml_payload(), "<div>amount=12%amount=</div>");
    }

    #[test]
    fn checked_payload_rejects_wrong_identity_revision_context_and_request() {
        let pinned_rule_set = rule_set("2550q-v2024", SHA_A);
        let trusted = trusted_evaluation(
            pinned_rule_set.clone(),
            BehaviorProfile::FilingSafe,
            1,
            "12",
            ValidationPhase::FinalCopy,
        );
        let payload = checked_payload(&trusted, "12");

        let wrong_identity = trusted_evaluation(
            rule_set("2550q-v2024-other", SHA_B),
            BehaviorProfile::FilingSafe,
            1,
            "12",
            ValidationPhase::FinalCopy,
        );
        assert!(matches!(
            payload.validate_against_trusted(&wrong_identity),
            Err(CheckedFinalCopyPayloadError::BindingMismatch { field: "rule_set" })
        ));

        let wrong_revision = trusted_evaluation(
            pinned_rule_set.clone(),
            BehaviorProfile::FilingSafe,
            2,
            "12",
            ValidationPhase::FinalCopy,
        );
        assert!(matches!(
            payload.validate_against_trusted(&wrong_revision),
            Err(CheckedFinalCopyPayloadError::BindingMismatch {
                field: "input_revision"
            })
        ));

        let wrong_context = trusted_evaluation_with_context(
            pinned_rule_set.clone(),
            BehaviorProfile::FilingSafe,
            1,
            "12",
            ValidationPhase::FinalCopy,
            "p-2",
        );
        assert!(matches!(
            payload.validate_against_trusted(&wrong_context),
            Err(CheckedFinalCopyPayloadError::BindingMismatch {
                field: "context_fingerprint"
            })
        ));

        let wrong_request = trusted_evaluation(
            pinned_rule_set,
            BehaviorProfile::FilingSafe,
            1,
            "13",
            ValidationPhase::FinalCopy,
        );
        assert!(matches!(
            payload.validate_against_trusted(&wrong_request),
            Err(CheckedFinalCopyPayloadError::RequestInputMismatch)
        ));
    }

    #[test]
    fn database_rejects_a_checked_payload_from_another_trusted_request() {
        let (database, draft_id) = database_with_draft();
        let rule_set = rule_set("2550q-v2024", SHA_A);
        let identity = FormRuleIdentity::from_rule_set(&rule_set, BehaviorProfile::FilingSafe);
        database
            .save_form_rule_editor_state(draft_id, 0, &identity, &exact_editor_json("13"))
            .unwrap();
        let old_trusted = trusted_evaluation(
            rule_set.clone(),
            BehaviorProfile::FilingSafe,
            1,
            "12",
            ValidationPhase::FinalCopy,
        );
        let current_trusted = trusted_evaluation(
            rule_set,
            BehaviorProfile::FilingSafe,
            1,
            "13",
            ValidationPhase::FinalCopy,
        );
        let mismatched_payload = checked_payload(&old_trusted, "12");

        assert!(matches!(
            database.create_form_final_copy(draft_id, 1, &current_trusted, &mismatched_payload,),
            Err(FormRuleStateError::InvalidFinalCopy(_))
        ));
        assert!(
            database
                .load_active_form_final_copy(draft_id)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn stored_checked_payload_fails_closed_after_xml_proof_or_legacy_tampering() {
        let (database, draft_id) = database_with_draft();
        let rule_set = rule_set("2550q-v2024", SHA_A);
        let identity = FormRuleIdentity::from_rule_set(&rule_set, BehaviorProfile::FilingSafe);
        database
            .save_form_rule_editor_state(draft_id, 0, &identity, &exact_editor_json("12"))
            .unwrap();
        let trusted = trusted_evaluation(
            rule_set,
            BehaviorProfile::FilingSafe,
            1,
            "12",
            ValidationPhase::FinalCopy,
        );
        let payload = checked_payload(&trusted, "12");
        let final_copy = database
            .create_form_final_copy(draft_id, 1, &trusted, &payload)
            .unwrap();
        let original_xml = final_copy.xml_payload.clone();
        let original_xml_sha256 = final_copy.xml_sha256.to_hex();
        let original_proof_json = final_copy.checked_payload.proof_json().to_string();
        let original_proof_sha256 = final_copy.payload_proof_sha256.to_hex();

        let tampered_xml = "<div>amount=13amount=</div>";
        database
            .conn
            .execute(
                "UPDATE form_finalizations
                 SET xml_payload = ?1, xml_sha256 = ?2
                 WHERE id = ?3",
                rusqlite::params![
                    tampered_xml,
                    sha256_digest(tampered_xml.as_bytes()).to_hex(),
                    final_copy.id
                ],
            )
            .unwrap();
        assert!(matches!(
            database.load_form_final_copy(final_copy.id),
            Err(FormRuleStateError::CorruptFinalCopy { .. })
        ));

        let tampered_proof_json = original_proof_json.replacen(
            "checked-final-copy-payload-v2",
            "checked-final-copy-payload-v999",
            1,
        );
        database
            .conn
            .execute(
                "UPDATE form_finalizations
                 SET xml_payload = ?1,
                     xml_sha256 = ?2,
                     payload_proof_json = ?3,
                     payload_proof_sha256 = ?4
                 WHERE id = ?5",
                rusqlite::params![
                    original_xml,
                    original_xml_sha256,
                    tampered_proof_json,
                    sha256_digest(tampered_proof_json.as_bytes()).to_hex(),
                    final_copy.id
                ],
            )
            .unwrap();
        assert!(matches!(
            database.load_form_final_copy(final_copy.id),
            Err(FormRuleStateError::CorruptFinalCopy { .. })
        ));

        database
            .conn
            .execute(
                "UPDATE form_finalizations
                 SET payload_proof_json = NULL, payload_proof_sha256 = NULL
                 WHERE id = ?1",
                [final_copy.id],
            )
            .unwrap();
        assert!(matches!(
            database.load_form_final_copy(final_copy.id),
            Err(FormRuleStateError::CorruptFinalCopy { .. })
        ));

        database
            .conn
            .execute(
                "UPDATE form_finalizations
                 SET payload_proof_json = ?1, payload_proof_sha256 = ?2
                 WHERE id = ?3",
                rusqlite::params![original_proof_json, original_proof_sha256, final_copy.id],
            )
            .unwrap();
        assert!(
            database
                .load_form_final_copy(final_copy.id)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn every_successful_edit_invalidates_the_active_final_copy() {
        let (database, draft_id) = database_with_draft();
        let rule_set = rule_set("2550q-v2024", SHA_A);
        let identity = FormRuleIdentity::from_rule_set(&rule_set, BehaviorProfile::FilingSafe);
        let editor_json = exact_editor_json("12x");
        database
            .save_form_rule_editor_state(draft_id, 0, &identity, &editor_json)
            .unwrap();
        let trusted = trusted_evaluation(
            rule_set,
            BehaviorProfile::FilingSafe,
            1,
            "12x",
            ValidationPhase::FinalCopy,
        );
        let checked_payload = checked_payload(&trusted, "12x");
        let final_copy = database
            .create_form_final_copy(draft_id, 1, &trusted, &checked_payload)
            .unwrap();

        let changed_json = exact_editor_json("12");
        let changed = database
            .save_form_rule_editor_state(draft_id, 1, &identity, &changed_json)
            .unwrap();
        assert_eq!(changed.storage_revision, 2);
        assert_eq!(changed.active_finalization_id, None);
        assert!(
            database
                .load_active_form_final_copy(draft_id)
                .unwrap()
                .is_none()
        );
        let historical = database
            .load_form_final_copy(final_copy.id)
            .unwrap()
            .unwrap();
        assert!(historical.invalidated_at.is_some());
        assert_eq!(historical.raw_snapshot_json, editor_json);

        let active_pointer: Option<i64> = database
            .conn
            .query_row(
                "SELECT active_finalization_id FROM form_drafts WHERE id = ?1",
                rusqlite::params![draft_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(active_pointer, None);
    }

    #[test]
    fn final_copy_rejects_non_final_phase() {
        let (database, draft_id) = database_with_draft();
        let rule_set = rule_set("2550q-v2024", SHA_A);
        let identity = FormRuleIdentity::from_rule_set(&rule_set, BehaviorProfile::FilingSafe);
        let editor_json = exact_editor_json("12");
        database
            .save_form_rule_editor_state(draft_id, 0, &identity, &editor_json)
            .unwrap();
        let trusted = trusted_evaluation(
            rule_set,
            BehaviorProfile::FilingSafe,
            1,
            "12",
            ValidationPhase::Validate,
        );

        let coverage = vec![FinalCopyFieldCoverage::canonical_field(
            XmlKey::parse("amount").unwrap(),
            SerializedOccurrence::new(1).unwrap(),
            FieldInstance::singleton(FieldId::parse("amount").unwrap()),
            CanonicalValue::Text("12".to_string()),
            "12".to_string(),
        )];
        assert!(matches!(
            CheckedFinalCopyPayload::try_from_coverage_for_test(
                &trusted,
                coverage,
                "<div>amount=12amount=</div>",
            ),
            Err(crate::form_rules::CheckedFinalCopyPayloadError::WrongPhase { .. })
        ));
    }
}
