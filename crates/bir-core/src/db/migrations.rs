//! Database schema migration engine.

use rusqlite::{Connection, OptionalExtension};
use tracing::info;

use crate::db::DbError;

const CURRENT_MIGRATION_VERSION: i32 = 24;

const NO_CONFIRMED_PROFILE_EVIDENCE_REASON: &str =
    "No confirmed effective profile evidence for this taxable year; review required";
const NO_LONGER_VALID_MIGRATION_SUGGESTION_REASON: &str =
    "No longer suggested by the confirmed effective profile; review required";

pub(crate) fn migrate_database(conn: &Connection) -> Result<(), DbError> {
    let mut version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;

    if version == 0 {
        // Check if this is an existing unversioned database (legacy)
        let has_profiles: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='profiles'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);

        if has_profiles {
            info!("Legacy database detected. Setting version to 1 to run incremental migrations.");
            // Set to v1 (not CURRENT) so v2+ migrations still execute sequentially.
            // v1 uses CREATE TABLE IF NOT EXISTS, so re-running it is safe.
            version = 1;
            conn.pragma_update(None, "user_version", version)?;
        }
    }

    let migrations = [
        // v1: The complete current schema
        "
        CREATE TABLE IF NOT EXISTS tax_deadlines (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            form_type TEXT NOT NULL,
            due_date TEXT NOT NULL,
            description TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS announcements (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source TEXT NOT NULL,
            title TEXT NOT NULL,
            content TEXT NOT NULL,
            published_at TEXT NOT NULL,
            read_status BOOLEAN NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS bir_notices (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            external_id TEXT NOT NULL,
            source TEXT NOT NULL,
            source_kind TEXT NOT NULL,
            source_url TEXT,
            title TEXT NOT NULL,
            body TEXT NOT NULL,
            notice_type TEXT NOT NULL,
            rdo_code TEXT,
            form_code TEXT,
            deadline TEXT,
            image_url TEXT,
            posted_at TEXT,
            fetched_at TEXT NOT NULL DEFAULT (datetime('now')),
            raw_json TEXT,
            read_status BOOLEAN NOT NULL DEFAULT 0,
            UNIQUE(source_kind, external_id)
        );
        CREATE INDEX IF NOT EXISTS idx_bir_notices_posted_at ON bir_notices(posted_at);
        CREATE INDEX IF NOT EXISTS idx_bir_notices_deadline ON bir_notices(deadline);
        CREATE INDEX IF NOT EXISTS idx_bir_notices_form_code ON bir_notices(form_code);
        CREATE INDEX IF NOT EXISTS idx_bir_notices_rdo_code ON bir_notices(rdo_code);

        CREATE TABLE IF NOT EXISTS penalties_cache (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            tin TEXT NOT NULL,
            form_type TEXT NOT NULL,
            period TEXT NOT NULL,
            penalty_amount REAL NOT NULL,
            reason TEXT NOT NULL,
            is_high_risk BOOLEAN NOT NULL DEFAULT 0,
            calculated_at TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (tin) REFERENCES profiles(tin)
        );

        CREATE TABLE IF NOT EXISTS profiles (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            tin TEXT UNIQUE NOT NULL,
            data_json TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS submissions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            tin TEXT NOT NULL,
            form_type TEXT NOT NULL,
            period TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'draft',
            form_data TEXT NOT NULL,
            submitted_at TEXT,
            filename TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (tin) REFERENCES profiles(tin)
        );

        CREATE TABLE IF NOT EXISTS form_drafts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            tin TEXT NOT NULL,
            form_code TEXT NOT NULL,
            taxable_year INTEGER NOT NULL,
            quarter INTEGER,
            status TEXT NOT NULL DEFAULT 'Draft',
            data_json TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(tin, form_code, taxable_year, quarter)
        );
        CREATE INDEX IF NOT EXISTS idx_form_drafts_tin_year ON form_drafts(tin, form_code, taxable_year);

        CREATE TABLE IF NOT EXISTS submission_receipts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            filename TEXT UNIQUE NOT NULL,
            tin TEXT NOT NULL,
            form_type TEXT NOT NULL,
            period TEXT NOT NULL,
            received_date TEXT NOT NULL,
            received_time TEXT NOT NULL,
            source_from TEXT,
            raw_text TEXT NOT NULL,
            raw_html TEXT,
            email_received_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- Actionable application health events surfaced in the Notifications
        -- page. Distinct from `bir_notices`, which carries BIR's own external
        -- announcements; these are conditions THIS app hit and the user can act
        -- on (an expired Google token, a profile row this build cannot read).
        --
        -- `tin` scopes an alert to one taxpayer profile; NULL means it applies
        -- to the whole application.
        --
        -- The UNIQUE(tin, kind) index is load-bearing. Sources fire repeatedly -
        -- the email cron retries every 60 seconds - so alerts are upserted:
        -- `occurrences` counts, `last_seen_at` moves, and the row stays single.
        -- Without it the table would gain 1,440 rows a day from one broken token.
        CREATE TABLE IF NOT EXISTS app_alerts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            tin TEXT,
            kind TEXT NOT NULL,
            severity TEXT NOT NULL,
            title TEXT NOT NULL,
            detail TEXT NOT NULL,
            action TEXT,
            occurrences INTEGER NOT NULL DEFAULT 1,
            first_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
            resolved_at TEXT
        );

        -- COALESCE keeps the uniqueness working for application-wide alerts;
        -- SQLite treats NULLs as distinct in a plain UNIQUE index, which would
        -- let every app-wide alert duplicate freely.
        CREATE UNIQUE INDEX IF NOT EXISTS idx_app_alerts_scope_kind
            ON app_alerts (COALESCE(tin, ''), kind);

        CREATE TABLE IF NOT EXISTS job_queue (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            job_type TEXT NOT NULL DEFAULT 'Custom',
            cron_expr TEXT,
            command TEXT,
            status TEXT NOT NULL DEFAULT 'Queued',
            retries INTEGER NOT NULL DEFAULT 0,
            last_run_at TEXT,
            next_run_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            output_log TEXT
        );

        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        ",
        // v2: created `data_providers` (integrations); dropped by v23 (D18),
        // so a fresh database no longer creates it.
        "SELECT 1; -- v2 marker: data providers (dropped in v23)",
        // v3: Add unique index on submissions for import deduplication.
        // Existing duplicate rows (from prior buggy imports) are cleaned up first.
        // Use COALESCE so NULL submitted_at values are treated as equal (SQLite NULL != NULL).
        "
        DELETE FROM submissions
        WHERE id NOT IN (
            SELECT MIN(id) FROM submissions
            GROUP BY tin, form_type, period, COALESCE(submitted_at, '')
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_submissions_dedup
            ON submissions(tin, form_type, period, COALESCE(submitted_at, ''));
        ",
        // v4: Promote legacy compat boolean flags from profile JSON into
        // normalized fields. This is a Rust-side data migration (no DDL change).
        // The SQL marker is a no-op that just advances the user_version.
        //
        // Handled below in `migrate_v4_compat_fields()` after the SQL loop.
        "SELECT 1; -- v4 marker: compat field promotion (Rust-side)",
        // v5: Add period_key column for unified period handling.
        // Column addition is handled in Rust (below) to be idempotent
        // since SQLite does not support ADD COLUMN IF NOT EXISTS.
        "SELECT 1; -- v5 marker: period_key column (Rust-side)",
        // v6: Static tax-calendar marker.
        //
        // The official recurring calendar now lives in `calendar_rules.rs` as
        // compiled base rules. Fresh databases should not create the removed
        // calendar CRUD tables (`tax_calendars`, `tax_deadline_rules`,
        // `tax_deadline_overrides`, `resolved_tax_deadlines`). Existing
        // databases keep those legacy tables untouched.
        "SELECT 1; -- v6 marker: static tax calendar rules",
        // v7: Per-year Forms Set — the user-owned, authoritative list of which forms a
        // taxpayer files in a given taxable year (replaces the temporal suggestion engine).
        // Populated from a COR (AI-assisted) or manually; read by the dashboard + deadlines.
        "
        CREATE TABLE IF NOT EXISTS per_year_forms (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            tin TEXT NOT NULL,
            taxable_year INTEGER NOT NULL,
            form_code TEXT NOT NULL,
            frequency TEXT NOT NULL,
            active INTEGER NOT NULL DEFAULT 1,
            source TEXT NOT NULL,
            custom INTEGER NOT NULL DEFAULT 0,
            reason TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(tin, taxable_year, form_code)
        );
        CREATE INDEX IF NOT EXISTS idx_per_year_forms_tin_year
            ON per_year_forms(tin, taxable_year);
        ",
        "SELECT 1; -- v8 marker: per-year forms backfill (Rust-side)",
        // v9: Re-run per-year forms backfill with correct obligation_allowed filtering.
        // v8 only checked registered_tax_types_allow_form, missing taxpayer_type,
        // VAT, deprecation, and other checks from obligation_allowed_for_version_and_profile.
        "SELECT 1; -- v9 marker: per-year forms heal (Rust-side)",
        // v10: created Google Calendar sync tables (`profile_calendar_links`,
        // `profile_calendar_events`); dropped by v23 (D18).
        "SELECT 1; -- v10 marker: calendar sync (dropped in v23)",
        // v11: Preserve Forms Set evidence/effective dates and fail-closed review state.
        // Columns are added by Rust below so partially upgraded legacy databases remain safe.
        "SELECT 1; -- v11 marker: Forms Set provenance and conflict state (Rust-side)",
        // v12: Persist the effective-dated profile-version ledger once for
        // legacy profiles. Undated backfills remain review-blocked and do not
        // replace an existing user-owned Forms Set.
        "SELECT 1; -- v12 marker: durable profile-version ledger backfill (Rust-side)",
        // v13: Correct early v12 undated migration backfills that were
        // serialized as Confirmed even though they required date review.
        "SELECT 1; -- v13 marker: explicit NeedsReview profile-version status (Rust-side)",
        // v14: Fail closed stale generated Forms Set rows left by the broad
        // v8/v9 migration inference. Rows are retained for audit and existing
        // draft/snapshot access; only their active filing authority changes.
        "SELECT 1; -- v14 marker: deactivate stale migration Forms Set rows (Rust-side)",
        // v15: Preserve exact raw editor state and immutable rules-based Final
        // Copy snapshots. Columns are added idempotently in Rust because early
        // development builds may contain a partially applied schema.
        "SELECT 1; -- v15 marker: versioned form-rule editor/finalization state (Rust-side)",
        // v16: Bind every immutable Final Copy XML payload to an opaque checked
        // serializer proof. Existing rows remain readable only after proof
        // columns are populated by a reviewed migration.
        "SELECT 1; -- v16 marker: checked Final Copy payload proof (Rust-side)",
        // v17: Persist every component of the exact reviewed FormRevisionKey.
        // Nullable additive columns leave projected legacy pins fail-closed
        // until a reviewed migration supplies the omitted identity fields.
        "SELECT 1; -- v17 marker: exact form-rule revision identity (Rust-side)",
        // v18: created `inbox_oauth_tokens` (Gmail OAuth); dropped by v23
        // (D18).
        "SELECT 1; -- v18 marker: inbox OAuth tokens (dropped in v23)",
        // v19: Read-only copies of what BIR received, kept when a Submitted or
        // Confirmed return is reverted to Draft for an amendment.
        "SELECT 1; -- v19 marker: amendment snapshots (Rust-side)",
        // v20: Taxpayer -> Branch -> form templates (tax-profile redesign,
        // Phase 1). `taxpayers` groups the existing full-TIN profiles by their
        // 9-digit root; a `profiles` row stays the branch. Tables and the
        // grouping are created in Rust so they can be healed every startup.
        "SELECT 1; -- v20 marker: taxpayers, branch settings, form templates (Rust-side)",
        // v21: Preserve imported historical eBIRForms returns as opaque files.
        "
        CREATE TABLE IF NOT EXISTS legacy_filed_returns (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            sha256 TEXT NOT NULL,
            tin TEXT NOT NULL,
            form_identity TEXT NOT NULL,
            form_id TEXT,
            period TEXT NOT NULL,
            amendment_version INTEGER,
            email_copy TEXT,
            encrypted INTEGER NOT NULL,
            source_filename TEXT NOT NULL,
            raw_file BLOB NOT NULL,
            fields_json TEXT NOT NULL,
            imported_at TEXT NOT NULL DEFAULT (datetime('now')),
            -- Same bytes under a different filename (email copy, new
            -- amendment version) is its own filing record.
            UNIQUE(sha256, source_filename)
        );
        CREATE INDEX IF NOT EXISTS idx_legacy_filed_returns_tin
            ON legacy_filed_returns(tin);
        ",
        // v22: moved PINs onto the taxpayer and inferred an access mode;
        // both went with D20 (v24 drops what it left).
        "SELECT 1; -- v22 marker: taxpayer credentials (dropped in v24)",
        // v23 (D18): drop what the email, OAuth, provider and calendar-sync
        // code left behind: tables, `taxpayers.totp_secret`, and the profile
        // JSON keys. Rust-side so it can heal every startup.
        "SELECT 1; -- v23 marker: drop OAuth/IMAP/providers/calendar sync (Rust-side)",
        // v24 (D20): drop the PIN: `taxpayers.pin_hash`, the profile JSON
        // key and the access-mode settings. Rust-side so it heals every
        // startup.
        "SELECT 1; -- v24 marker: drop PINs and access mode (Rust-side)",
    ];

    while version < CURRENT_MIGRATION_VERSION {
        let migration_idx = version as usize;
        if let Some(migration_sql) = migrations.get(migration_idx) {
            info!("Applying database migration v{}", version + 1);
            conn.execute_batch(migration_sql)?;
            version += 1;
            conn.pragma_update(None, "user_version", version)?;

            // v4: After the SQL marker is committed, run the Rust data migration.
            if version == 4 {
                migrate_v4_compat_fields(conn)?;
            }

            // v5: Backfill period_key from existing quarter column.
            if version == 5 {
                migrate_v5_backfill_period_key(conn)?;
            }

            // v8: Backfill per-year forms from profile versions.
            if version == 8 {
                migrate_v8_per_year_forms_backfill(conn)?;
            }

            // v9: Heal per-year forms with correct obligation filtering.
            if version == 9 {
                migrate_v9_per_year_forms_heal(conn)?;
            }

            if version == 12 {
                migrate_v12_profile_version_ledger(conn)?;
            }

            if version == 13 {
                migrate_v13_profile_version_review_status(conn)?;
            }

            if version == 14 {
                migrate_v14_stale_migration_backfills(conn)?;
            }

            if version == 15 {
                migrate_v15_form_rule_state(conn)?;
            }

            if version == 16 {
                migrate_v16_checked_final_copy_payload(conn)?;
            }

            if version == 17 {
                migrate_v17_exact_form_rule_identity(conn)?;
            }

            if version == 19 {
                ensure_amendment_snapshots_table(conn)?;
            }

            if version == 20 {
                migrate_v20_taxpayer_grouping(conn)?;
            }

            if version == 23 {
                migrate_v23_drop_unused(conn)?;
            }

            if version == 24 {
                migrate_v24_drop_pins(conn)?;
            }
        } else {
            break;
        }
    }

    // Heal partially upgraded databases even when their user_version already
    // reports the current schema. Early builds could advance past v11 after
    // adding only a subset of these columns.
    migrate_v11_forms_set_provenance(conn)?;

    // Retry/heal v14 even when an early build advanced `user_version` before
    // completing its Rust-side data migration. The update is idempotent and
    // only examines still-active generated migration rows.
    migrate_v14_stale_migration_backfills(conn)?;

    // Heal partially upgraded v15 databases. All DDL below is idempotent.
    migrate_v15_form_rule_state(conn)?;

    // Heal partially upgraded v16 databases. Nullable additions deliberately
    // leave legacy Final Copies fail-closed until a reviewed proof exists.
    migrate_v16_checked_final_copy_payload(conn)?;

    // Heal partially upgraded v17 databases without inferring any omitted
    // exact-key component from a registry or from mutable application state.
    migrate_v17_exact_form_rule_identity(conn)?;

    // Heal partially upgraded v19 databases. Idempotent.
    ensure_amendment_snapshots_table(conn)?;

    // Heal partially upgraded v20 databases, and group any profile created
    // since (by an older build or the legacy profile editor) under its
    // taxpayer. Only missing roots are inserted; existing rows never change.
    migrate_v20_taxpayer_grouping(conn)?;

    // v23 and v24 are idempotent; re-running them clears keys an older
    // build or a profile import wrote since.
    migrate_v23_drop_unused(conn)?;
    migrate_v24_drop_pins(conn)?;

    Ok(())
}

/// Tables v23 drops (D18): Gmail OAuth tokens, integration providers, and
/// Google Calendar sync.
const V23_DROPPED_TABLES: [&str; 4] = [
    "inbox_oauth_tokens",
    "data_providers",
    "profile_calendar_links",
    "profile_calendar_events",
];

/// Profile JSON keys v23 removes (D18): IMAP, OAuth and the authenticator.
const V23_DROPPED_PROFILE_KEYS: [&str; 8] = [
    "imap_email",
    "imap_host",
    "imap_app_password",
    "imap_enabled",
    "oauth_access_token",
    "oauth_refresh_token",
    "email_auth_method",
    "totp_secret",
];

/// v23 (D18): drop the OAuth/IMAP/providers/calendar-sync tables, the
/// taxpayer authenticator column, the admin authenticator setting, and the
/// matching keys of every profile's JSON. Idempotent: each step checks
/// before it changes anything.
fn migrate_v23_drop_unused(conn: &Connection) -> Result<(), DbError> {
    for table in V23_DROPPED_TABLES {
        conn.execute_batch(&format!("DROP TABLE IF EXISTS {table};"))?;
    }
    if table_exists(conn, "taxpayers")? {
        let has_totp: bool = conn
            .query_row(
                "SELECT 1 FROM pragma_table_info('taxpayers') WHERE name = 'totp_secret'",
                [],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if has_totp {
            conn.execute_batch("ALTER TABLE taxpayers DROP COLUMN totp_secret;")?;
        }
    }
    if table_exists(conn, "settings")? {
        conn.execute("DELETE FROM settings WHERE key = 'app_totp_secret'", [])?;
    }
    if table_exists(conn, "profiles")? {
        let paths: Vec<String> = V23_DROPPED_PROFILE_KEYS
            .iter()
            .map(|key| format!("'$.{key}'"))
            .collect();
        let present: Vec<String> = V23_DROPPED_PROFILE_KEYS
            .iter()
            .map(|key| format!("json_type(data_json, '$.{key}') IS NOT NULL"))
            .collect();
        let cleared = conn.execute(
            &format!(
                "UPDATE profiles SET data_json = json_remove(data_json, {})
                 WHERE json_valid(data_json) AND ({})",
                paths.join(", "),
                present.join(" OR ")
            ),
            [],
        )?;
        if cleared > 0 {
            info!("v23 migration: email/OAuth keys removed from {cleared} profiles");
        }
    }
    Ok(())
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool, DbError> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Settings v24 deletes (D20): the access mode and the help desk admin's
/// PIN and owner root.
const V24_DROPPED_SETTINGS: [&str; 3] = ["access_mode", "admin_tin_root", "app_lock_pin_hash"];

/// v24 (D20): drop the PIN. Removes `taxpayers.pin_hash`, the
/// `profile_pin_hash` key of every profile's JSON (only that key is edited,
/// so COR versions and every other stored byte stay), and the access-mode
/// settings. Idempotent: each step checks before it changes anything.
fn migrate_v24_drop_pins(conn: &Connection) -> Result<(), DbError> {
    if table_exists(conn, "taxpayers")? {
        let has_pin: bool = conn
            .query_row(
                "SELECT 1 FROM pragma_table_info('taxpayers') WHERE name = 'pin_hash'",
                [],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if has_pin {
            conn.execute_batch("ALTER TABLE taxpayers DROP COLUMN pin_hash;")?;
            info!("v24 migration: taxpayers.pin_hash dropped");
        }
    }
    if table_exists(conn, "settings")? {
        for key in V24_DROPPED_SETTINGS {
            conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
        }
    }
    if table_exists(conn, "profiles")? {
        let cleared = conn.execute(
            "UPDATE profiles SET data_json = json_remove(data_json, '$.profile_pin_hash')
             WHERE json_valid(data_json)
               AND json_type(data_json, '$.profile_pin_hash') IS NOT NULL",
            [],
        )?;
        if cleared > 0 {
            info!("v24 migration: PIN key removed from {cleared} profiles");
        }
    }
    Ok(())
}

/// Add the omitted exact [`FormRevisionKey`](bir_rules::FormRevisionKey)
/// components to draft pins, immutable Final Copies, and migration audit rows.
///
/// Columns remain nullable only for additive SQLite compatibility. Persistence
/// writes every component for new pins. The read boundary rejects a projected
/// legacy pin whose new columns are NULL, requiring a reviewed migration
/// instead of silently resolving it through a current registry.
fn migrate_v17_exact_form_rule_identity(conn: &Connection) -> Result<(), DbError> {
    let table_columns: [(&str, &[(&str, &str)]); 4] = [
        (
            "form_drafts",
            &[
                ("rule_set_form_code", "TEXT"),
                ("rule_set_form_revision", "TEXT"),
                ("rule_set_official_package_version", "TEXT"),
            ],
        ),
        (
            "form_finalizations",
            &[
                ("rule_set_form_code", "TEXT"),
                ("rule_set_form_revision", "TEXT"),
                ("rule_set_official_package_version", "TEXT"),
            ],
        ),
        (
            "submission_receipts",
            &[
                // The email's own Date header, RFC 3339. BIR's received stamp is
                // parsed from the body; this is when the message arrived.
                ("email_received_at", "TEXT"),
            ],
        ),
        (
            "form_rule_migrations",
            &[
                ("from_rule_set_form_code", "TEXT"),
                ("from_rule_set_form_revision", "TEXT"),
                ("from_rule_set_official_package_version", "TEXT"),
                ("to_rule_set_form_code", "TEXT"),
                ("to_rule_set_form_revision", "TEXT"),
                ("to_rule_set_official_package_version", "TEXT"),
            ],
        ),
    ];

    let mut added_columns = 0usize;
    for (table, required_columns) in table_columns {
        let table_exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |_| Ok(true),
            )
            .unwrap_or(false);
        if !table_exists {
            continue;
        }

        let column_names = {
            let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
            stmt.query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<std::collections::BTreeSet<_>, _>>()?
        };
        for (column, definition) in required_columns {
            if !column_names.contains(*column) {
                conn.execute(
                    &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
                    [],
                )?;
                added_columns += 1;
            }
        }
    }

    if added_columns > 0 {
        info!(
            "v17 migration: added {} exact form-rule identity columns",
            added_columns
        );
    }
    Ok(())
}

/// v20: taxpayer grouping, per-branch settings and one template per form per
/// branch (tax-profile redesign D2, D7, D11).
///
/// Schema choice: a `profiles` row stays the branch. Its full TIN is already
/// the key of every TIN-keyed table, so nothing is re-keyed; the branch name
/// and the per-field "same as head office" flags live in the nullable
/// `profiles.branch_json` column (NULL = legacy row: every fact is its own,
/// default name). Taxpayer-level facts live in `taxpayers`, keyed by root.
///
/// `profiles.data_json` is never rewritten here, so confirmed COR versions
/// and every other stored profile byte stay exactly as they were.
fn migrate_v20_taxpayer_grouping(conn: &Connection) -> Result<(), DbError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS taxpayers (
            tin_root TEXT PRIMARY KEY NOT NULL
                CHECK (length(tin_root) = 9 AND tin_root NOT GLOB '*[^0-9]*'),
            name TEXT NOT NULL,
            taxpayer_type TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            data_json TEXT NOT NULL DEFAULT '{}'
        );

        CREATE TABLE IF NOT EXISTS form_templates (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            tin TEXT NOT NULL,
            form_code TEXT NOT NULL,
            fields_json TEXT NOT NULL,
            source TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(tin, form_code)
        );",
    )?;

    let has_profiles: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='profiles'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !has_profiles {
        return Ok(());
    }
    let has_branch_json = {
        let mut stmt = conn.prepare("PRAGMA table_info(profiles)")?;
        stmt.query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<std::collections::BTreeSet<_>, _>>()?
            .contains("branch_json")
    };
    if !has_branch_json {
        conn.execute("ALTER TABLE profiles ADD COLUMN branch_json TEXT", [])?;
    }

    super::taxpayers::sync_taxpayer_rows(conn)
}

/// Shared Gmail / IMAP OAuth credentials keyed by inbox email.
///
/// Several taxpayer profiles can point at the same mailbox. The confirmation
/// poller must read this row rather than the first matching `profiles` row,
/// otherwise a reconnect on Jane leaves Alejandro's dead refresh token in use.
/// One row per Submitted or Confirmed return reverted for amendment. Rows are
/// never updated or deleted by the app.
fn ensure_amendment_snapshots_table(conn: &Connection) -> Result<(), DbError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS amendment_snapshots (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            form_code TEXT NOT NULL,
            tin TEXT NOT NULL,
            taxable_year INTEGER NOT NULL,
            period_key TEXT NOT NULL,
            status_at_revert TEXT NOT NULL,
            submitted_at TEXT,
            confirmed_at TEXT,
            receipt_id INTEGER,
            submission_filename TEXT,
            submitted_xml TEXT,
            payload_source TEXT NOT NULL,
            draft_json TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_amendment_snapshots_return
            ON amendment_snapshots(tin, form_code, taxable_year, period_key);",
    )?;
    Ok(())
}

/// Add the opaque checked-payload proof alongside existing Final Copy XML.
///
/// The columns are nullable only for additive SQLite compatibility. The load
/// boundary rejects NULL, malformed, or mismatched proof data.
fn migrate_v16_checked_final_copy_payload(conn: &Connection) -> Result<(), DbError> {
    let has_finalizations: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master
             WHERE type='table' AND name='form_finalizations'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !has_finalizations {
        return Ok(());
    }

    let column_names = {
        let mut stmt = conn.prepare("PRAGMA table_info(form_finalizations)")?;
        stmt.query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<std::collections::BTreeSet<_>, _>>()?
    };

    let mut added_columns = 0usize;
    for (column, definition) in [
        ("payload_proof_json", "TEXT"),
        ("payload_proof_sha256", "TEXT"),
    ] {
        if !column_names.contains(column) {
            conn.execute(
                &format!("ALTER TABLE form_finalizations ADD COLUMN {column} {definition}"),
                [],
            )?;
            added_columns += 1;
        }
    }

    if added_columns > 0 {
        info!(
            "v16 migration: added {} checked Final Copy proof columns",
            added_columns
        );
    }
    Ok(())
}

/// Add raw editor persistence and immutable rule-evaluation snapshots.
///
/// Raw input remains separate from `data_json`, which continues to hold the
/// last canonical typed draft during migration. Finalizations and explicit
/// ruleset migrations are append-only audit records.
fn migrate_v15_form_rule_state(conn: &Connection) -> Result<(), DbError> {
    let has_form_drafts: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='form_drafts'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !has_form_drafts {
        return Ok(());
    }

    let column_names = {
        let mut stmt = conn.prepare("PRAGMA table_info(form_drafts)")?;
        stmt.query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<std::collections::BTreeSet<_>, _>>()?
    };

    let mut added_columns = 0usize;
    for (column, definition) in [
        ("editor_state_json", "TEXT"),
        ("storage_revision", "INTEGER NOT NULL DEFAULT 0"),
        ("rule_set_id", "TEXT"),
        ("rule_set_sha256", "TEXT"),
        ("behavior_profile", "TEXT"),
        ("active_finalization_id", "INTEGER"),
    ] {
        if !column_names.contains(column) {
            conn.execute(
                &format!("ALTER TABLE form_drafts ADD COLUMN {column} {definition}"),
                [],
            )?;
            added_columns += 1;
        }
    }

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS form_finalizations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            form_draft_id INTEGER NOT NULL,
            source_storage_revision INTEGER NOT NULL,
            rule_set_id TEXT NOT NULL,
            rule_set_sha256 TEXT NOT NULL,
            behavior_profile TEXT NOT NULL,
            input_sha256 TEXT NOT NULL,
            context_sha256 TEXT NOT NULL,
            canonical_sha256 TEXT NOT NULL,
            derived_sha256 TEXT NOT NULL,
            report_sha256 TEXT NOT NULL,
            xml_sha256 TEXT NOT NULL,
            raw_snapshot_json TEXT NOT NULL,
            canonical_json TEXT NOT NULL,
            derived_json TEXT NOT NULL,
            validation_report_json TEXT NOT NULL,
            xml_payload TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            invalidated_at TEXT,
            FOREIGN KEY (form_draft_id) REFERENCES form_drafts(id) ON DELETE CASCADE,
            UNIQUE (
                form_draft_id,
                source_storage_revision,
                rule_set_sha256,
                input_sha256,
                context_sha256
            )
        );
        CREATE INDEX IF NOT EXISTS idx_form_finalizations_draft
            ON form_finalizations(form_draft_id, created_at);

        CREATE TABLE IF NOT EXISTS form_rule_migrations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            form_draft_id INTEGER NOT NULL,
            from_rule_set_id TEXT,
            from_rule_set_sha256 TEXT,
            to_rule_set_id TEXT NOT NULL,
            to_rule_set_sha256 TEXT NOT NULL,
            from_snapshot_sha256 TEXT NOT NULL,
            to_snapshot_sha256 TEXT NOT NULL,
            diff_json TEXT NOT NULL,
            result TEXT NOT NULL,
            accepted_at TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (form_draft_id) REFERENCES form_drafts(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_form_rule_migrations_draft
            ON form_rule_migrations(form_draft_id, accepted_at);",
    )?;

    let finalization_columns = {
        let mut stmt = conn.prepare("PRAGMA table_info(form_finalizations)")?;
        stmt.query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<std::collections::BTreeSet<_>, _>>()?
    };
    if !finalization_columns.contains("derived_sha256") {
        // Existing development rows remain readable only after the persistence
        // layer can prove a derived digest. NULL deliberately fails closed.
        conn.execute(
            "ALTER TABLE form_finalizations ADD COLUMN derived_sha256 TEXT",
            [],
        )?;
        added_columns += 1;
    }

    if added_columns > 0 {
        info!(
            "v15 migration: added {} missing form-rule state columns",
            added_columns
        );
    }
    Ok(())
}

/// Deactivate stale active Forms Set rows created by the v8/v9 broad migration
/// backfill without deleting filing history.
///
/// Only standard, active rows whose persisted source is exactly
/// `migration_backfill` participate. Manual rows, manual suppressions, and
/// custom rows are never rewritten. A generated row remains active only when
/// the profile has an unambiguous confirmed segment for that taxable year and
/// current suggestion reconciliation resolves that form to an active filing
/// obligation. This includes effective manual overrides stored on the
/// confirmed profile version.
fn migrate_v14_stale_migration_backfills(conn: &Connection) -> Result<(), DbError> {
    use crate::forms::forms_set::reconcile_forms_set_for_year;
    use crate::forms::registry::canonical_form_code;
    use crate::profile::TaxpayerProfile;
    use std::collections::{BTreeSet, HashMap};

    let table_exists = |table: &str| {
        conn.query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |_| Ok(true),
        )
        .unwrap_or(false)
    };
    if !table_exists("profiles") || !table_exists("per_year_forms") {
        return Ok(());
    }

    // Some development snapshots marked themselves current with only a
    // partial test schema. Never make a best-effort mutation against those.
    let per_year_columns = {
        let mut stmt = conn.prepare("PRAGMA table_info(per_year_forms)")?;
        stmt.query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<BTreeSet<_>, _>>()?
    };
    let required_columns = [
        "tin",
        "taxable_year",
        "form_code",
        "active",
        "source",
        "custom",
        "reason",
        "updated_at",
    ];
    if required_columns
        .iter()
        .any(|column| !per_year_columns.contains(*column))
    {
        tracing::warn!(
            "v14 migration: skipped stale Forms Set healing because per_year_forms is incomplete"
        );
        return Ok(());
    }

    let profiles = {
        let mut stmt = conn.prepare("SELECT tin, data_json FROM profiles")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut profiles = HashMap::new();
        for row in rows {
            let (stored_tin, data_json) = row?;
            match serde_json::from_str::<TaxpayerProfile>(&data_json) {
                Ok(profile) => {
                    profiles.insert(profile.tin.full(), profile.clone());
                    profiles.insert(stored_tin, profile);
                }
                Err(error) => {
                    tracing::warn!(
                        tin = %stored_tin,
                        %error,
                        "v14 migration: profile JSON could not be resolved; generated rows fail closed"
                    );
                }
            }
        }
        profiles
    };

    let candidates = {
        let mut stmt = conn.prepare(
            "SELECT id, tin, taxable_year, form_code
             FROM per_year_forms
             WHERE active = 1 AND source = 'migration_backfill' AND custom = 0",
        )?;
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u16>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?
    };

    let mut resolved_years = HashMap::<(String, u16), (bool, BTreeSet<String>)>::new();
    let mut deactivated = 0usize;
    for (id, tin, taxable_year, form_code) in candidates {
        let cache_key = (tin.clone(), taxable_year);
        let (has_confirmed_evidence, active_codes) =
            resolved_years.entry(cache_key).or_insert_with(|| {
                let Some(profile) = profiles.get(&tin) else {
                    return (false, BTreeSet::new());
                };
                let resolved = profile.resolve_tax_profile_for_year(taxable_year);
                if resolved.has_blocking_issues() || resolved.effective_segments.is_empty() {
                    return (false, BTreeSet::new());
                }

                let suggestions = crate::integration::validation::form_suggestions_for_profile_year(
                    profile,
                    taxable_year,
                );
                let reconciled = reconcile_forms_set_for_year(taxable_year, None, &suggestions);
                let active_codes = reconciled
                    .forms_set
                    .entries
                    .into_iter()
                    .filter(|entry| entry.is_filing_active())
                    .map(|entry| canonical_form_code(&entry.form_code))
                    .collect();
                (true, active_codes)
            });

        if active_codes.contains(&canonical_form_code(&form_code)) {
            continue;
        }

        let reason = if *has_confirmed_evidence {
            NO_LONGER_VALID_MIGRATION_SUGGESTION_REASON
        } else {
            NO_CONFIRMED_PROFILE_EVIDENCE_REASON
        };
        conn.execute(
            "UPDATE per_year_forms
             SET active = 0, reason = ?1, updated_at = datetime('now')
             WHERE id = ?2 AND active = 1 AND source = 'migration_backfill' AND custom = 0",
            rusqlite::params![reason, id],
        )?;
        deactivated += 1;
    }

    if deactivated > 0 {
        info!(
            "v14 migration: deactivated {} stale generated Forms Set rows without deleting history",
            deactivated
        );
    }
    Ok(())
}

/// Convert undated migration backfills from the early v12 `Confirmed` state
/// to the explicit fail-closed `NeedsReview` state.
///
/// This migration changes profile JSON only. Existing user-owned Forms Sets
/// are intentionally left untouched.
fn migrate_v13_profile_version_review_status(conn: &Connection) -> Result<(), DbError> {
    use crate::profile::TaxpayerProfile;

    let rows = {
        let mut stmt = conn.prepare("SELECT id, data_json FROM profiles")?;
        stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?
    };

    let mut normalized = 0usize;
    for (id, data_json) in rows {
        let mut profile: TaxpayerProfile = match serde_json::from_str(&data_json) {
            Ok(profile) => profile,
            Err(error) => {
                tracing::warn!(
                    profile_id = id,
                    %error,
                    "v13 migration: profile JSON could not be normalized"
                );
                continue;
            }
        };
        if !profile.normalize_profile_version_review_statuses() {
            continue;
        }

        conn.execute(
            "UPDATE profiles SET data_json = ?1 WHERE id = ?2",
            rusqlite::params![serde_json::to_string(&profile)?, id],
        )?;
        normalized += 1;
    }

    if normalized > 0 {
        info!(
            "v13 migration: moved {} undated profile versions to NeedsReview",
            normalized
        );
    }
    Ok(())
}

/// Persist one compatibility profile-version record for legacy profile JSON.
///
/// This is deliberately a one-time migration rather than a resolver fallback.
/// A reliable business start date becomes the effective start. Otherwise the
/// version is retained as `NeedsReview` and resolution remains fail-closed.
/// Existing `per_year_forms` rows are left untouched.
fn migrate_v12_profile_version_ledger(conn: &Connection) -> Result<(), DbError> {
    use crate::profile::TaxpayerProfile;

    let rows = {
        let mut stmt = conn.prepare("SELECT id, data_json FROM profiles")?;
        stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?
    };

    let mut backfilled = 0usize;
    for (id, data_json) in rows {
        let mut profile: TaxpayerProfile = match serde_json::from_str(&data_json) {
            Ok(profile) => profile,
            Err(error) => {
                tracing::warn!(
                    profile_id = id,
                    %error,
                    "v12 migration: profile JSON could not be backfilled"
                );
                continue;
            }
        };
        if !profile.profile_versions.is_empty() {
            continue;
        }

        profile.ensure_profile_version_ledger();
        conn.execute(
            "UPDATE profiles SET data_json = ?1 WHERE id = ?2",
            rusqlite::params![serde_json::to_string(&profile)?, id],
        )?;
        backfilled += 1;
    }

    if backfilled > 0 {
        info!(
            "v12 migration: persisted profile-version ledgers for {} legacy profiles",
            backfilled
        );
    }
    Ok(())
}

/// Add auditable suggestion provenance and explicit conflict state to `per_year_forms`.
///
/// Each addition is checked independently because early development builds may contain
/// only a subset of the columns. Existing rows default to `resolved`, preserving their
/// established manual/generated decision while new conflicts fail closed.
fn migrate_v11_forms_set_provenance(conn: &Connection) -> Result<(), DbError> {
    let has_per_year_forms: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='per_year_forms'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !has_per_year_forms {
        return Ok(());
    }

    let column_names = {
        let mut stmt = conn.prepare("PRAGMA table_info(per_year_forms)")?;
        stmt.query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<std::collections::BTreeSet<_>, _>>()?
    };

    let mut added_columns = 0usize;
    for (column, definition) in [
        ("source_reference", "TEXT"),
        ("effective_from", "TEXT"),
        ("effective_until", "TEXT"),
        ("review_status", "TEXT NOT NULL DEFAULT 'resolved'"),
        ("conflict_json", "TEXT"),
    ] {
        if !column_names.contains(column) {
            conn.execute(
                &format!("ALTER TABLE per_year_forms ADD COLUMN {column} {definition}"),
                [],
            )?;
            added_columns += 1;
        }
    }

    if added_columns > 0 {
        info!(
            "v11 migration: added {} missing Forms Set provenance columns",
            added_columns
        );
    }
    Ok(())
}

/// v4 data migration: promote legacy compat boolean flags out of stored profile JSON.
///
/// Reads the old field names directly from the raw `serde_json::Value` because
/// the compat fields have been removed from `TaxpayerProfile`. Promotes:
/// - `opted_for_8_percent_flat_rate: true` → appends `TaxElectionHistory(EightPercent)`
/// - `imap_enabled: true` → sets `email_tracking_enabled = true`
///
/// Both transformations are idempotent.
fn migrate_v4_compat_fields(conn: &Connection) -> Result<(), DbError> {
    use crate::profile::{IncomeTaxElection, TaxElectionHistory, TaxpayerProfile};
    use chrono::Datelike as _;

    let rows: Vec<(i64, String)> = {
        let mut stmt = conn.prepare("SELECT id, data_json FROM profiles")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(DbError::Sqlite)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(DbError::Sqlite)?
    };

    for (id, data_json) in rows {
        // Parse compat flags from raw JSON (the fields no longer exist on the typed struct)
        let raw: serde_json::Value = match serde_json::from_str(&data_json) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("v4 migration: failed to parse profile id={}: {}", id, e);
                continue;
            }
        };

        let opted_8pct = raw
            .get("opted_for_8_percent_flat_rate")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let imap_enabled_compat = raw
            .get("imap_enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if !opted_8pct && !imap_enabled_compat {
            continue;
        }

        let mut profile: TaxpayerProfile = match serde_json::from_str(&data_json) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(
                    "v4 migration: failed to deserialize profile id={}: {}",
                    id,
                    e
                );
                continue;
            }
        };

        let mut dirty = false;

        // Promote 8% compat flag
        if opted_8pct && profile.tax_elections.is_empty() {
            let retroactive_year = profile
                .business_start_date
                .map(|d| d.year() as u16)
                .unwrap_or_else(|| {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default();
                    let approx_year = (now.as_secs() / 31_557_600 + 1970) as u16;
                    approx_year.saturating_sub(1)
                });

            profile.tax_elections.push(TaxElectionHistory {
                taxable_year: retroactive_year,
                election: IncomeTaxElection::EightPercent,
                elected_at: chrono::NaiveDateTime::new(
                    chrono::NaiveDate::from_ymd_opt(retroactive_year as i32, 4, 15)
                        .unwrap_or_default(),
                    chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap_or_default(),
                ),
                source_form: "legacy_compat_migration_v4".into(),
            });
            tracing::info!(
                "v4 migration: promoted 8% election for profile id={} (year {})",
                id,
                retroactive_year
            );
            dirty = true;
        }

        // Promote imap_enabled compat flag
        if imap_enabled_compat && !profile.email_tracking_enabled {
            profile.email_tracking_enabled = true;
            tracing::info!(
                "v4 migration: promoted email_tracking_enabled for profile id={}",
                id
            );
            dirty = true;
        }

        if dirty {
            let updated_json =
                serde_json::to_string(&profile).map_err(|e| DbError::Other(e.to_string()))?;
            conn.execute(
                "UPDATE profiles SET data_json = ?1 WHERE id = ?2",
                rusqlite::params![updated_json, id],
            )
            .map_err(DbError::Sqlite)?;
        }
    }

    Ok(())
}

/// v5 data migration: add `period_key` column and backfill from the existing `quarter` column.
///
/// Uses form_code to determine the correct period format:
/// - Monthly forms (1601C, 0619E, 0619F, etc.) → `M{quarter:02}` (quarter holds month)
/// - Quarterly forms (2551Q, 1701Q, etc.) → `Q{quarter}`
/// - Annual/NULL quarter → `A`
fn migrate_v5_backfill_period_key(conn: &Connection) -> Result<(), DbError> {
    // Check if form_drafts table exists (some test scenarios skip v1)
    let has_form_drafts: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='form_drafts'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);

    if !has_form_drafts {
        return Ok(());
    }

    // Idempotent column addition: check PRAGMA table_info for `period_key`
    let has_period_key: bool = {
        let mut stmt = conn.prepare("PRAGMA table_info(form_drafts)")?;
        let col_names: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(DbError::Sqlite)?
            .filter_map(|r| r.ok())
            .collect();
        col_names.iter().any(|name| name == "period_key")
    };

    if !has_period_key {
        conn.execute_batch(
            "ALTER TABLE form_drafts ADD COLUMN period_key TEXT;
             CREATE INDEX IF NOT EXISTS idx_form_drafts_period_key
                 ON form_drafts(tin, form_code, taxable_year, period_key);",
        )?;
        info!("v5 migration: added period_key column to form_drafts");
    }

    // Monthly form codes that repurpose `quarter` as month
    let monthly_forms = [
        "1601C", "1601E", "1601F", "0619E", "0619F", "1600", "1600WP", "1602", "2550M", "2551M",
        "2200A", "2200AN", "2200M", "2200P", "2200T",
    ];
    let annual_forms = [
        "1604CF", "1604E", "1700", "1701", "1701A", "1701MS", "1702", "1702RT", "1702EX", "1702MX",
    ];
    let open_ended_forms = ["0605", "2000"];

    let rows: Vec<(i64, String, Option<i64>)> = {
        let mut stmt = conn
            .prepare("SELECT id, form_code, quarter FROM form_drafts WHERE period_key IS NULL")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            })
            .map_err(DbError::Sqlite)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(DbError::Sqlite)?
    };

    let count = rows.len();

    for (id, form_code, quarter_opt) in rows {
        let period_key = match quarter_opt {
            Some(q) => {
                if open_ended_forms.iter().any(|m| *m == form_code) {
                    format!("O{}", q.max(1))
                } else if annual_forms.iter().any(|m| *m == form_code) {
                    "A".to_string()
                } else if monthly_forms.iter().any(|m| *m == form_code) {
                    format!("M{q:02}")
                } else {
                    format!("Q{}", q.clamp(1, 4))
                }
            }
            None => "A".to_string(),
        };

        conn.execute(
            "UPDATE form_drafts SET period_key = ?1 WHERE id = ?2",
            rusqlite::params![period_key, id],
        )
        .map_err(DbError::Sqlite)?;
    }

    if count > 0 {
        info!("v5 migration: backfilled period_key for {} rows", count);
    }

    Ok(())
}

fn migrate_v8_per_year_forms_backfill(conn: &Connection) -> Result<(), DbError> {
    use crate::forms::forms_set::{FormSetEntry, FormSetSource};
    use crate::forms::registry::{FORM_REGISTRY, FilingFrequency, find_form};
    use crate::profile::{ManualObligationOverrideAction, TaxpayerProfile};
    use chrono::Datelike as _;

    fn frequency_to_str_local(f: &FilingFrequency) -> &'static str {
        match f {
            FilingFrequency::Quarterly => "quarterly",
            FilingFrequency::Annual => "annual",
            FilingFrequency::Monthly => "monthly",
            FilingFrequency::OpenEnded => "open_ended",
        }
    }

    let rows: Vec<(i64, String)> = {
        let mut stmt = conn.prepare("SELECT id, data_json FROM profiles")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(DbError::Sqlite)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(DbError::Sqlite)?
    };

    let mut backfilled_count = 0;

    for (id, data_json) in rows {
        let mut profile: TaxpayerProfile = match serde_json::from_str(&data_json) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(
                    "v8 migration: failed to deserialize profile id={}: {}",
                    id,
                    e
                );
                continue;
            }
        };

        profile.ensure_profile_version_ledger();
        let tin = profile.tin.full();
        let versions = profile.confirmed_profile_versions();
        let mut years = std::collections::BTreeSet::new();
        let current_year = chrono::Utc::now().year() as u16;
        for v in &versions {
            let start_year = v
                .effective_from
                .map(|d| d.year() as u16)
                .or_else(|| profile.business_start_date.map(|d| d.year() as u16))
                .unwrap_or(2018);
            let end_year = v
                .effective_until
                .map(|d| d.year() as u16)
                .unwrap_or(current_year);
            let end_year = end_year.max(current_year);
            for y in start_year..=end_year {
                years.insert(y);
            }
        }

        for year in years {
            let active_versions = profile.active_profile_versions_for_year(year);
            if let Some(version) = active_versions.last() {
                let mut entries = Vec::new();
                for def in FORM_REGISTRY {
                    if crate::integration::validation::registered_tax_types_allow_form(
                        version, def.code,
                    ) && crate::integration::validation::obligation_allowed_for_version_and_profile(
                        def, version, &profile, year,
                    ) {
                        entries.push(FormSetEntry::from_code(
                            def.code,
                            FormSetSource::MigrationBackfill,
                        ));
                    }
                }

                for r in &version.obligation_overrides {
                    if let Some(existing) = entries.iter_mut().find(|e| e.form_code == r.form_code)
                    {
                        match r.action {
                            ManualObligationOverrideAction::Include => {
                                existing.active = true;
                                existing.reason = Some(r.reason.clone());
                            }
                            ManualObligationOverrideAction::Exclude => {
                                existing.active = false;
                                existing.reason = Some(r.reason.clone());
                            }
                        }
                    } else {
                        let custom = find_form(&r.form_code).is_none();
                        let frequency = find_form(&r.form_code)
                            .map(|d| d.frequency.clone())
                            .unwrap_or(FilingFrequency::OpenEnded);
                        let mut entry = FormSetEntry::from_code(
                            r.form_code.clone(),
                            FormSetSource::MigrationBackfill,
                        );
                        entry.frequency = frequency;
                        entry.active = matches!(r.action, ManualObligationOverrideAction::Include);
                        entry.custom = custom;
                        entry.reason = Some(r.reason.clone());
                        entries.push(entry);
                    }
                }

                conn.execute(
                    "DELETE FROM per_year_forms WHERE tin = ?1 AND taxable_year = ?2",
                    rusqlite::params![tin, year],
                )
                .map_err(DbError::Sqlite)?;

                for entry in entries {
                    conn.execute(
                        "INSERT INTO per_year_forms
                         (tin, taxable_year, form_code, frequency, active, source, custom, reason)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        rusqlite::params![
                            tin,
                            year,
                            entry.form_code,
                            frequency_to_str_local(&entry.frequency),
                            entry.active as i64,
                            entry.source.as_str(),
                            entry.custom as i64,
                            entry.reason,
                        ],
                    )
                    .map_err(DbError::Sqlite)?;
                    backfilled_count += 1;
                }
            }
        }
    }

    if backfilled_count > 0 {
        info!(
            "v8 migration: backfilled {} forms in per_year_forms",
            backfilled_count
        );
    }

    Ok(())
}

/// v9 data migration: heal per_year_forms rows that were backfilled by v8 without
/// the `obligation_allowed_for_version_and_profile` check. This re-runs the same
/// logic as v8 but with both `registered_tax_types_allow_form` AND
/// `obligation_allowed_for_version_and_profile`, ensuring taxpayer_type, VAT,
/// deprecation, and other filters are applied. Existing manual overrides and
/// custom forms are preserved.
fn migrate_v9_per_year_forms_heal(conn: &Connection) -> Result<(), DbError> {
    use crate::forms::forms_set::{FormSetEntry, FormSetSource};
    use crate::forms::registry::{FORM_REGISTRY, FilingFrequency, find_form};
    use crate::profile::{ManualObligationOverrideAction, TaxpayerProfile};
    use chrono::Datelike as _;

    fn frequency_to_str_local(f: &FilingFrequency) -> &'static str {
        match f {
            FilingFrequency::Quarterly => "quarterly",
            FilingFrequency::Annual => "annual",
            FilingFrequency::Monthly => "monthly",
            FilingFrequency::OpenEnded => "open_ended",
        }
    }

    let rows: Vec<(i64, String)> = {
        let mut stmt = conn.prepare("SELECT id, data_json FROM profiles")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(DbError::Sqlite)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(DbError::Sqlite)?
    };

    let mut healed_count = 0;

    for (_id, data_json) in rows {
        let mut profile: TaxpayerProfile = match serde_json::from_str(&data_json) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("v9 migration: failed to deserialize profile: {}", e);
                continue;
            }
        };

        profile.ensure_profile_version_ledger();
        let tin = profile.tin.full();
        let versions = profile.confirmed_profile_versions();
        let mut years = std::collections::BTreeSet::new();
        let current_year = chrono::Utc::now().year() as u16;
        for v in &versions {
            let start_year = v
                .effective_from
                .map(|d| d.year() as u16)
                .or_else(|| profile.business_start_date.map(|d| d.year() as u16))
                .unwrap_or(2018);
            let end_year = v
                .effective_until
                .map(|d| d.year() as u16)
                .unwrap_or(current_year);
            let end_year = end_year.max(current_year);
            for y in start_year..=end_year {
                years.insert(y);
            }
        }

        for year in years {
            let active_versions = profile.active_profile_versions_for_year(year);
            if let Some(version) = active_versions.last() {
                let mut entries = Vec::new();
                for def in FORM_REGISTRY {
                    if crate::integration::validation::registered_tax_types_allow_form(
                        version, def.code,
                    ) && crate::integration::validation::obligation_allowed_for_version_and_profile(
                        def, version, &profile, year,
                    ) {
                        entries.push(FormSetEntry::from_code(
                            def.code,
                            FormSetSource::MigrationBackfill,
                        ));
                    }
                }

                for r in &version.obligation_overrides {
                    if let Some(existing) = entries.iter_mut().find(|e| e.form_code == r.form_code)
                    {
                        match r.action {
                            ManualObligationOverrideAction::Include => {
                                existing.active = true;
                                existing.reason = Some(r.reason.clone());
                            }
                            ManualObligationOverrideAction::Exclude => {
                                existing.active = false;
                                existing.reason = Some(r.reason.clone());
                            }
                        }
                    } else {
                        let custom = find_form(&r.form_code).is_none();
                        let frequency = find_form(&r.form_code)
                            .map(|d| d.frequency.clone())
                            .unwrap_or(FilingFrequency::OpenEnded);
                        let mut entry = FormSetEntry::from_code(
                            r.form_code.clone(),
                            FormSetSource::MigrationBackfill,
                        );
                        entry.frequency = frequency;
                        entry.active = matches!(r.action, ManualObligationOverrideAction::Include);
                        entry.custom = custom;
                        entry.reason = Some(r.reason.clone());
                        entries.push(entry);
                    }
                }

                // Also preserve any user-added custom forms and manually deactivated standard forms from existing data
                let existing_preserved: Vec<(String, String, bool, String, Option<String>, bool)> = {
                    let mut stmt = conn.prepare(
                        "SELECT form_code, frequency, active, source, reason, custom FROM per_year_forms \
                         WHERE tin = ?1 AND taxable_year = ?2 AND (custom = 1 OR active = 0)",
                    )?;
                    let rows = stmt
                        .query_map(rusqlite::params![tin, year], |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, bool>(2)?,
                                row.get::<_, String>(3)?,
                                row.get::<_, Option<String>>(4)?,
                                row.get::<_, bool>(5)?,
                            ))
                        })
                        .map_err(DbError::Sqlite)?;
                    rows.collect::<Result<Vec<_>, _>>()
                        .map_err(DbError::Sqlite)?
                };

                for (code, freq_str, active, source_str, reason, custom) in existing_preserved {
                    if let Some(existing) = entries.iter_mut().find(|e| e.form_code == code) {
                        if !active {
                            existing.active = false;
                            existing.reason = reason;
                        }
                    } else {
                        let frequency = match freq_str.as_str() {
                            "monthly" => FilingFrequency::Monthly,
                            "quarterly" => FilingFrequency::Quarterly,
                            "annual" => FilingFrequency::Annual,
                            _ => FilingFrequency::OpenEnded,
                        };
                        let mut entry = FormSetEntry::from_code(
                            code,
                            FormSetSource::from_str_lossy(&source_str),
                        );
                        entry.frequency = frequency;
                        entry.active = active;
                        entry.custom = custom;
                        entry.reason = reason;
                        entries.push(entry);
                    }
                }

                conn.execute(
                    "DELETE FROM per_year_forms WHERE tin = ?1 AND taxable_year = ?2",
                    rusqlite::params![tin, year],
                )
                .map_err(DbError::Sqlite)?;

                for entry in entries {
                    conn.execute(
                        "INSERT INTO per_year_forms
                         (tin, taxable_year, form_code, frequency, active, source, custom, reason)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        rusqlite::params![
                            tin,
                            year,
                            entry.form_code,
                            frequency_to_str_local(&entry.frequency),
                            entry.active as i64,
                            entry.source.as_str(),
                            entry.custom as i64,
                            entry.reason,
                        ],
                    )
                    .map_err(DbError::Sqlite)?;
                    healed_count += 1;
                }
            }
        }
    }

    if healed_count > 0 {
        info!(
            "v9 migration: healed {} forms in per_year_forms with correct obligation filtering",
            healed_count
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    type FormsSetMigrationRow = (
        String,
        i64,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        Option<String>,
    );

    /// Helper: opens an unencrypted in-memory SQLite connection for testing.
    fn test_conn() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    #[test]
    fn test_fresh_db_migrates_to_current_version() {
        let conn = test_conn();
        migrate_database(&conn).unwrap();
        let v: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, CURRENT_MIGRATION_VERSION);
    }

    #[test]
    fn test_fresh_db_creates_all_tables() {
        let conn = test_conn();
        migrate_database(&conn).unwrap();

        let tables = [
            "profiles",
            "submissions",
            "form_drafts",
            "submission_receipts",
            "job_queue",
            "settings",
            "bir_notices",
            "per_year_forms",
            "form_finalizations",
            "form_rule_migrations",
            "taxpayers",
            "form_templates",
        ];
        for table in tables {
            let exists: bool = conn
                .query_row(
                    &format!(
                        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='{}'",
                        table
                    ),
                    [],
                    |_| Ok(true),
                )
                .unwrap_or(false);
            assert!(exists, "Table '{}' should exist after migration", table);
        }
    }

    #[test]
    fn test_v15_adds_versioned_editor_and_finalization_state() {
        let conn = test_conn();
        migrate_database(&conn).unwrap();

        let form_draft_columns = {
            let mut stmt = conn.prepare("PRAGMA table_info(form_drafts)").unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<std::collections::BTreeSet<_>, _>>()
                .unwrap()
        };
        for column in [
            "editor_state_json",
            "storage_revision",
            "rule_set_id",
            "rule_set_sha256",
            "behavior_profile",
            "active_finalization_id",
        ] {
            assert!(
                form_draft_columns.contains(column),
                "missing v15 form_drafts column {column}"
            );
        }

        // The healer must remain safe after user_version has already advanced.
        migrate_v15_form_rule_state(&conn).unwrap();
        let finalization_tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table'
                   AND name IN ('form_finalizations', 'form_rule_migrations')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(finalization_tables, 2);

        let finalization_columns = {
            let mut stmt = conn
                .prepare("PRAGMA table_info(form_finalizations)")
                .unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<std::collections::BTreeSet<_>, _>>()
                .unwrap()
        };
        assert!(finalization_columns.contains("derived_sha256"));
    }

    #[test]
    fn test_v16_adds_checked_final_copy_proof_columns_idempotently() {
        let conn = test_conn();
        migrate_database(&conn).unwrap();
        migrate_v16_checked_final_copy_payload(&conn).unwrap();

        let finalization_columns = {
            let mut stmt = conn
                .prepare("PRAGMA table_info(form_finalizations)")
                .unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<std::collections::BTreeSet<_>, _>>()
                .unwrap()
        };

        assert!(finalization_columns.contains("payload_proof_json"));
        assert!(finalization_columns.contains("payload_proof_sha256"));
    }

    #[test]
    fn test_v17_adds_exact_rule_identity_columns_without_inference() {
        let conn = test_conn();
        conn.execute_batch(
            "CREATE TABLE form_drafts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                rule_set_id TEXT,
                rule_set_sha256 TEXT,
                behavior_profile TEXT
            );
            INSERT INTO form_drafts (
                rule_set_id,
                rule_set_sha256,
                behavior_profile
            ) VALUES (
                '2550q-v2024',
                'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                'filing_safe'
            );
            PRAGMA user_version = 16;",
        )
        .unwrap();

        migrate_database(&conn).unwrap();
        migrate_v17_exact_form_rule_identity(&conn).unwrap();

        let expected_columns = [
            (
                "form_drafts",
                &[
                    "rule_set_form_code",
                    "rule_set_form_revision",
                    "rule_set_official_package_version",
                ][..],
            ),
            (
                "form_finalizations",
                &[
                    "rule_set_form_code",
                    "rule_set_form_revision",
                    "rule_set_official_package_version",
                ][..],
            ),
            (
                "form_rule_migrations",
                &[
                    "from_rule_set_form_code",
                    "from_rule_set_form_revision",
                    "from_rule_set_official_package_version",
                    "to_rule_set_form_code",
                    "to_rule_set_form_revision",
                    "to_rule_set_official_package_version",
                ][..],
            ),
        ];
        for (table, required_columns) in expected_columns {
            let column_names = {
                let mut stmt = conn
                    .prepare(&format!("PRAGMA table_info({table})"))
                    .unwrap();
                stmt.query_map([], |row| row.get::<_, String>(1))
                    .unwrap()
                    .collect::<Result<std::collections::BTreeSet<_>, _>>()
                    .unwrap()
            };
            for column in required_columns {
                assert!(
                    column_names.contains(*column),
                    "missing v17 {table} column {column}"
                );
            }
        }

        let legacy_pin: (
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT rule_set_id,
                        rule_set_sha256,
                        behavior_profile,
                        rule_set_form_code,
                        rule_set_form_revision,
                        rule_set_official_package_version
                 FROM form_drafts
                 WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(legacy_pin.0, "2550q-v2024");
        assert_eq!(
            legacy_pin.1,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(legacy_pin.2, "filing_safe");
        assert_eq!(
            (legacy_pin.3, legacy_pin.4, legacy_pin.5),
            (None, None, None)
        );
    }

    /// D18: an install that had the email, OAuth, provider and calendar
    /// tables, the authenticator column and the profile keys loses them all,
    /// and a second run changes nothing.
    #[test]
    fn test_v23_drops_oauth_imap_provider_and_calendar_state() {
        let conn = test_conn();
        migrate_database(&conn).unwrap();
        conn.execute_batch(
            "CREATE TABLE inbox_oauth_tokens (email TEXT PRIMARY KEY);
             CREATE TABLE data_providers (id INTEGER PRIMARY KEY);
             CREATE TABLE profile_calendar_links (profile_tin TEXT PRIMARY KEY);
             CREATE TABLE profile_calendar_events (profile_tin TEXT);
             ALTER TABLE taxpayers ADD COLUMN totp_secret TEXT;
             INSERT INTO settings (key, value) VALUES ('app_totp_secret', 'x');
             INSERT INTO profiles (tin, data_json)
                 VALUES ('00012345300000', '{\"full_name\":\"Kept\",\"imap_email\":\"a@example.com\",\"oauth_refresh_token\":\"r\",\"totp_secret\":\"t\"}');
             PRAGMA user_version = 22;",
        )
        .unwrap();
        migrate_database(&conn).unwrap();
        migrate_database(&conn).unwrap();

        for table in V23_DROPPED_TABLES {
            assert!(
                !table_exists(&conn, table).unwrap(),
                "{table} was not dropped"
            );
        }
        let totp_column: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM pragma_table_info('taxpayers') WHERE name = 'totp_secret'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        assert_eq!(totp_column, None);
        let setting: Option<String> = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'app_totp_secret'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        assert_eq!(setting, None);
        let data: String = conn
            .query_row(
                "SELECT data_json FROM profiles WHERE tin = '00012345300000'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(data, "{\"full_name\":\"Kept\"}");
    }

    #[test]
    fn test_v15_preserves_legacy_draft_json_unpinned() {
        let conn = test_conn();
        conn.execute_batch(
            "CREATE TABLE form_drafts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tin TEXT NOT NULL,
                form_code TEXT NOT NULL,
                taxable_year INTEGER NOT NULL,
                quarter INTEGER,
                period_key TEXT,
                status TEXT NOT NULL DEFAULT 'Draft',
                data_json TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(tin, form_code, taxable_year, quarter)
            );
            INSERT INTO form_drafts
                (tin, form_code, taxable_year, quarter, period_key, status, data_json)
            VALUES
                ('123456789000', '2550Q', 2026, 1, 'Q1', 'Draft',
                 '{\"legacy\":true}');
            PRAGMA user_version = 14;",
        )
        .unwrap();

        migrate_database(&conn).unwrap();

        let stored: (String, Option<String>, i64, Option<String>) = conn
            .query_row(
                "SELECT data_json, editor_state_json, storage_revision, rule_set_id
                 FROM form_drafts WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(stored.0, "{\"legacy\":true}");
        assert_eq!(stored.1, None);
        assert_eq!(stored.2, 0);
        assert_eq!(stored.3, None);
    }

    #[test]
    fn test_v10_forms_set_rows_gain_resolved_provenance_defaults() {
        let conn = test_conn();
        conn.execute_batch(
            "CREATE TABLE profiles (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tin TEXT UNIQUE NOT NULL,
                data_json TEXT NOT NULL
            );
            CREATE TABLE per_year_forms (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tin TEXT NOT NULL,
                taxable_year INTEGER NOT NULL,
                form_code TEXT NOT NULL,
                frequency TEXT NOT NULL,
                active INTEGER NOT NULL DEFAULT 1,
                source TEXT NOT NULL,
                custom INTEGER NOT NULL DEFAULT 0,
                reason TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(tin, taxable_year, form_code)
            );
            INSERT INTO per_year_forms
                (tin, taxable_year, form_code, frequency, active, source, custom, reason)
            VALUES
                ('123456789000', 2026, '2551Q', 'quarterly', 1, 'reviewed_cor', 0,
                 'Reviewed before provenance migration');
            PRAGMA user_version = 10;",
        )
        .unwrap();

        migrate_database(&conn).unwrap();
        let row: (String, Option<String>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT review_status, source_reference, effective_from, conflict_json
                 FROM per_year_forms WHERE form_code = '2551Q'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();

        assert_eq!(row, ("resolved".into(), None, None, None));
    }

    #[test]
    fn test_current_version_heals_partial_forms_set_schema_without_losing_data() {
        let conn = test_conn();
        conn.execute_batch(
            "CREATE TABLE per_year_forms (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tin TEXT NOT NULL,
                taxable_year INTEGER NOT NULL,
                form_code TEXT NOT NULL,
                frequency TEXT NOT NULL,
                active INTEGER NOT NULL DEFAULT 1,
                source TEXT NOT NULL,
                custom INTEGER NOT NULL DEFAULT 0,
                reason TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                source_reference TEXT,
                UNIQUE(tin, taxable_year, form_code)
            );
            INSERT INTO per_year_forms
                (tin, taxable_year, form_code, frequency, active, source, custom, reason,
                 source_reference)
            VALUES
                ('00034567500000', 2026, '2551Q', 'quarterly', 1, 'reviewed_cor', 0,
                 'Existing reviewed decision', 'cor-document-1');
            PRAGMA user_version = 13;",
        )
        .unwrap();

        migrate_database(&conn).unwrap();
        migrate_database(&conn).unwrap();

        let row: FormsSetMigrationRow = conn
            .query_row(
                "SELECT tin, taxable_year, form_code, reason, source_reference,
                        effective_from, effective_until, review_status, conflict_json
                 FROM per_year_forms WHERE form_code = '2551Q'",
                [],
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

        assert_eq!(
            row,
            (
                "00034567500000".to_string(),
                2026,
                "2551Q".to_string(),
                "Existing reviewed decision".to_string(),
                Some("cor-document-1".to_string()),
                None,
                None,
                "resolved".to_string(),
                None,
            )
        );
    }

    #[test]
    fn test_v12_persists_legacy_profile_ledgers_without_touching_forms_sets() {
        use crate::profile::{TaxProfileVersionSource, TaxProfileVersionStatus, TaxpayerProfile};
        use chrono::NaiveDate;

        fn legacy_profile_json(
            full_name: &str,
            tin_segment: &str,
            business_start_date: Option<&str>,
        ) -> String {
            serde_json::json!({
                "id": null,
                "full_name": full_name,
                "tin": {
                    "segment1": tin_segment,
                    "segment2": "456",
                    "segment3": "789",
                    "branch": "000"
                },
                "rdo_code": "039",
                "line_of_business": "Consulting",
                "registered_address": "Quezon City",
                "zip_code": "1100",
                "phone": "09170000100",
                "email": "profile@example.com",
                "default_form_type": "2551Q",
                "taxpayer_type": "Individual",
                "is_vat_registered": false,
                "business_start_date": business_start_date,
                "compliance_source_mode": "CorVersioned"
            })
            .to_string()
        }

        let conn = test_conn();
        conn.execute_batch(
            "CREATE TABLE profiles (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tin TEXT UNIQUE NOT NULL,
                data_json TEXT NOT NULL
            );
            CREATE TABLE per_year_forms (
                tin TEXT NOT NULL,
                taxable_year INTEGER NOT NULL,
                form_code TEXT NOT NULL,
                reason TEXT
            );
            INSERT INTO per_year_forms (tin, taxable_year, form_code, reason)
            VALUES ('111456789000', 2026, '2551Q', 'User-owned decision');
            PRAGMA user_version = 11;",
        )
        .unwrap();

        let dated_json = legacy_profile_json("Dated Legacy", "111", Some("2020-04-15"));
        let undated_json = legacy_profile_json("Undated Legacy", "222", None);
        let mut already_versioned: TaxpayerProfile = serde_json::from_str(&legacy_profile_json(
            "Already Versioned",
            "333",
            Some("2021-01-01"),
        ))
        .unwrap();
        already_versioned.ensure_profile_version_ledger();
        already_versioned.profile_versions[0].id = "existing-version".to_string();
        let already_versioned_json = serde_json::to_string(&already_versioned).unwrap();

        for (tin, data_json) in [
            ("111456789000", dated_json),
            ("222456789000", undated_json),
            ("333456789000", already_versioned_json.clone()),
        ] {
            conn.execute(
                "INSERT INTO profiles (tin, data_json) VALUES (?1, ?2)",
                rusqlite::params![tin, data_json],
            )
            .unwrap();
        }

        migrate_database(&conn).unwrap();

        let load_profile = |tin: &str| -> TaxpayerProfile {
            let json: String = conn
                .query_row(
                    "SELECT data_json FROM profiles WHERE tin = ?1",
                    [tin],
                    |row| row.get(0),
                )
                .unwrap();
            serde_json::from_str(&json).unwrap()
        };

        let dated = load_profile("111456789000");
        assert_eq!(dated.profile_versions.len(), 1);
        let dated_version = &dated.profile_versions[0];
        assert_eq!(
            dated_version.source,
            TaxProfileVersionSource::MigrationBackfill
        );
        assert_eq!(dated_version.status, TaxProfileVersionStatus::Confirmed);
        assert_eq!(
            dated_version.effective_from,
            NaiveDate::from_ymd_opt(2020, 4, 15)
        );
        assert!(!dated_version.needs_effective_date_review);

        let undated = load_profile("222456789000");
        assert_eq!(undated.profile_versions.len(), 1);
        let undated_version = &undated.profile_versions[0];
        assert_eq!(
            undated_version.source,
            TaxProfileVersionSource::MigrationBackfill
        );
        assert_eq!(undated_version.status, TaxProfileVersionStatus::NeedsReview);
        assert_eq!(undated_version.effective_from, None);
        assert!(undated_version.needs_effective_date_review);

        let stored_versioned_json: String = conn
            .query_row(
                "SELECT data_json FROM profiles WHERE tin = '333456789000'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_versioned_json, already_versioned_json);

        let forms_row: (String, i64, String, String) = conn
            .query_row(
                "SELECT tin, taxable_year, form_code, reason FROM per_year_forms",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            forms_row,
            (
                "111456789000".to_string(),
                2026,
                "2551Q".to_string(),
                "User-owned decision".to_string()
            )
        );

        let before_second_run: Vec<String> = {
            let mut statement = conn
                .prepare("SELECT data_json FROM profiles ORDER BY tin")
                .unwrap();
            statement
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        migrate_database(&conn).unwrap();
        let after_second_run: Vec<String> = {
            let mut statement = conn
                .prepare("SELECT data_json FROM profiles ORDER BY tin")
                .unwrap();
            statement
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(before_second_run, after_second_run);
        assert_eq!(dated.profile_versions[0].id, "legacy-current-profile");
        assert_eq!(undated.profile_versions[0].id, "legacy-current-profile");
    }

    #[test]
    fn test_v13_normalizes_undated_confirmed_backfill_without_touching_forms_sets() {
        use crate::profile::{TaxProfileVersionStatus, TaxpayerProfile};

        let mut profile: TaxpayerProfile = serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": "Undated Versioned Legacy",
            "tin": {
                "segment1": "444",
                "segment2": "456",
                "segment3": "789",
                "branch": "000"
            },
            "rdo_code": "039",
            "line_of_business": "Consulting",
            "registered_address": "Quezon City",
            "zip_code": "1100",
            "phone": "09170000100",
            "email": "profile@example.com",
            "default_form_type": "2551Q",
            "taxpayer_type": "Individual",
            "is_vat_registered": false,
            "business_start_date": null,
            "compliance_source_mode": "CorVersioned"
        }))
        .unwrap();
        profile.ensure_profile_version_ledger();
        profile.profile_versions[0].status = TaxProfileVersionStatus::Confirmed;
        let legacy_v12_json = serde_json::to_string(&profile).unwrap();

        let conn = test_conn();
        conn.execute_batch(
            "CREATE TABLE profiles (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tin TEXT UNIQUE NOT NULL,
                data_json TEXT NOT NULL
            );
            CREATE TABLE per_year_forms (
                tin TEXT NOT NULL,
                taxable_year INTEGER NOT NULL,
                form_code TEXT NOT NULL,
                reason TEXT
            );
            INSERT INTO per_year_forms (tin, taxable_year, form_code, reason)
            VALUES ('444456789000', 2026, '2551Q', 'Manual include survives review migration');
            PRAGMA user_version = 12;",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO profiles (tin, data_json) VALUES (?1, ?2)",
            rusqlite::params!["444456789000", legacy_v12_json],
        )
        .unwrap();

        migrate_database(&conn).unwrap();

        let migrated_json: String = conn
            .query_row(
                "SELECT data_json FROM profiles WHERE tin = '444456789000'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let migrated: TaxpayerProfile = serde_json::from_str(&migrated_json).unwrap();
        assert_eq!(
            migrated.profile_versions[0].status,
            TaxProfileVersionStatus::NeedsReview
        );
        assert!(migrated.profile_versions[0].needs_effective_date_review);
        assert!(migrated.confirmed_profile_versions().is_empty());

        let forms_row: (String, i64, String, String) = conn
            .query_row(
                "SELECT tin, taxable_year, form_code, reason FROM per_year_forms",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            forms_row,
            (
                "444456789000".to_string(),
                2026,
                "2551Q".to_string(),
                "Manual include survives review migration".to_string(),
            )
        );
    }

    #[test]
    fn test_v14_deactivates_only_stale_generated_rows_and_preserves_filing_history() {
        let conn = test_conn();
        conn.execute_batch(
            "CREATE TABLE profiles (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tin TEXT UNIQUE NOT NULL,
                data_json TEXT NOT NULL
            );
            CREATE TABLE per_year_forms (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tin TEXT NOT NULL,
                taxable_year INTEGER NOT NULL,
                form_code TEXT NOT NULL,
                frequency TEXT NOT NULL,
                active INTEGER NOT NULL DEFAULT 1,
                source TEXT NOT NULL,
                custom INTEGER NOT NULL DEFAULT 0,
                reason TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                source_reference TEXT,
                effective_from TEXT,
                effective_until TEXT,
                review_status TEXT NOT NULL DEFAULT 'resolved',
                conflict_json TEXT,
                UNIQUE(tin, taxable_year, form_code)
            );
            CREATE TABLE form_drafts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tin TEXT NOT NULL,
                form_code TEXT NOT NULL,
                taxable_year INTEGER NOT NULL,
                quarter INTEGER,
                status TEXT NOT NULL,
                data_json TEXT NOT NULL
            );
            PRAGMA user_version = 13;",
        )
        .unwrap();

        let profile_json = serde_json::json!({
            "id": 1,
            "full_name": "Migration Healing Taxpayer",
            "tin": {
                "segment1": "123",
                "segment2": "456",
                "segment3": "789",
                "branch": "000"
            },
            "rdo_code": "039",
            "line_of_business": "Consulting",
            "registered_address": "Quezon City",
            "zip_code": "1100",
            "phone": "09170000100",
            "email": "profile@example.com",
            "default_form_type": "2551Q",
            "taxpayer_type": "Individual",
            "tax_classification": "SelfEmployed",
            "eopt_tier": "Micro",
            "is_vat_registered": false,
            "business_start_date": "2019-01-27",
            "compliance_source_mode": "CorVersioned",
            "profile_versions": [
                {
                    "id": "confirmed-2026",
                    "label": "Confirmed 2026",
                    "status": "Confirmed",
                    "source": "ManualCor",
                    "effective_from": "2026-01-01",
                    "effective_until": null,
                    "cor": {},
                    "registered_tax_types": ["IncomeTax", "PercentageTax"],
                    "taxpayer_type": "Individual",
                    "tax_classification": "SelfEmployed",
                    "eopt_tier": "Micro",
                    "is_vat_registered": false,
                    "obligation_overrides": [
                        {
                            "form_code": "1800",
                            "action": "Include",
                            "reason": "Reviewed manual include"
                        }
                    ]
                }
            ]
        })
        .to_string();
        conn.execute(
            "INSERT INTO profiles (tin, data_json) VALUES (?1, ?2)",
            rusqlite::params!["123456789000", profile_json],
        )
        .unwrap();

        for (year, code, active, source, custom, reason) in [
            (
                2025,
                "2551Q",
                1,
                "migration_backfill",
                0,
                Some("Legacy guessed historical obligation"),
            ),
            (
                2026,
                "2551Q",
                1,
                "migration_backfill",
                0,
                Some("Valid generated obligation"),
            ),
            (
                2026,
                "1706",
                1,
                "migration_backfill",
                0,
                Some("Over-broad v8 suggestion"),
            ),
            (
                2026,
                "1701",
                1,
                "migration_backfill",
                0,
                Some("Redundant annual ITR"),
            ),
            (
                2026,
                "1701A",
                1,
                "migration_backfill",
                0,
                Some("Redundant annual ITR"),
            ),
            (
                2026,
                "1701MS",
                1,
                "migration_backfill",
                0,
                Some("Resolver-selected annual ITR"),
            ),
            (
                2026,
                "1800",
                1,
                "migration_backfill",
                0,
                Some("Legacy row backed by a manual override"),
            ),
            (2026, "1707A", 1, "manual", 0, Some("User-owned include")),
            (
                2026,
                "ACCOUNTANT-ATTACHMENT",
                1,
                "migration_backfill",
                1,
                Some("Custom row must survive"),
            ),
            (
                2026,
                "2550Q",
                0,
                "migration_backfill",
                0,
                Some("User excluded VAT form"),
            ),
        ] {
            conn.execute(
                "INSERT INTO per_year_forms
                 (tin, taxable_year, form_code, frequency, active, source, custom, reason)
                 VALUES ('123456789000', ?1, ?2, 'open_ended', ?3, ?4, ?5, ?6)",
                rusqlite::params![year, code, active, source, custom, reason],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO form_drafts
             (tin, form_code, taxable_year, quarter, status, data_json)
             VALUES ('123456789000', '2551Q', 2025, 4, 'Submitted', '{\"snapshot\":true}')",
            [],
        )
        .unwrap();

        migrate_database(&conn).unwrap();

        let stored_rows = {
            let mut stmt = conn
                .prepare(
                    "SELECT taxable_year, form_code, active, source, custom, reason
                     FROM per_year_forms
                     WHERE tin = '123456789000'
                     ORDER BY taxable_year, form_code",
                )
                .unwrap();
            stmt.query_map([], |row| {
                Ok((
                    row.get::<_, u16>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, bool>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
        };
        assert_eq!(stored_rows.len(), 10, "v14 must not delete Forms Set rows");

        let find = |year, code: &str| {
            stored_rows
                .iter()
                .find(|row| row.0 == year && row.1 == code)
                .unwrap()
        };
        assert_eq!(
            (find(2025, "2551Q").2, find(2025, "2551Q").5.as_deref()),
            (false, Some(NO_CONFIRMED_PROFILE_EVIDENCE_REASON))
        );
        assert!(find(2026, "2551Q").2, "valid generated row stays active");
        assert_eq!(
            (find(2026, "1706").2, find(2026, "1706").5.as_deref()),
            (false, Some(NO_LONGER_VALID_MIGRATION_SUGGESTION_REASON))
        );
        for redundant in ["1701", "1701A"] {
            assert_eq!(
                (find(2026, redundant).2, find(2026, redundant).5.as_deref()),
                (false, Some(NO_LONGER_VALID_MIGRATION_SUGGESTION_REASON)),
                "only one annual ITR may remain active"
            );
        }
        assert!(
            find(2026, "1701MS").2,
            "the confirmed Micro profile's primary annual ITR stays active"
        );
        assert!(
            find(2026, "1800").2,
            "an effective manual profile override preserves the old migration row"
        );
        assert_eq!(find(2026, "1707A").3, "manual");
        assert!(find(2026, "1707A").2, "manual row remains active");
        assert!(find(2026, "ACCOUNTANT-ATTACHMENT").2);
        assert!(find(2026, "ACCOUNTANT-ATTACHMENT").4);
        assert_eq!(
            find(2026, "ACCOUNTANT-ATTACHMENT").5.as_deref(),
            Some("Custom row must survive")
        );
        assert!(!find(2026, "2550Q").2);
        assert_eq!(
            find(2026, "2550Q").5.as_deref(),
            Some("User excluded VAT form")
        );

        let draft: (String, String) = conn
            .query_row(
                "SELECT status, data_json FROM form_drafts
                 WHERE tin = '123456789000' AND form_code = '2551Q' AND taxable_year = 2025",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(draft, ("Submitted".into(), "{\"snapshot\":true}".into()));

        // The post-version healer is deliberately safe to repeat.
        migrate_database(&conn).unwrap();
        let rows_after_second_run: Vec<(u16, String, bool, Option<String>)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT taxable_year, form_code, active, reason
                     FROM per_year_forms
                     WHERE tin = '123456789000'
                     ORDER BY taxable_year, form_code",
                )
                .unwrap();
            stmt.query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
        };
        let rows_after_first_run = stored_rows
            .into_iter()
            .map(|row| (row.0, row.1, row.2, row.5))
            .collect::<Vec<_>>();
        assert_eq!(rows_after_first_run, rows_after_second_run);
    }

    #[test]
    fn test_v1_db_upgrades_to_current() {
        let conn = test_conn();

        // Simulate a v1 database: run only the first migration manually
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS profiles (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tin TEXT UNIQUE NOT NULL,
                data_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS submissions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tin TEXT NOT NULL,
                form_type TEXT NOT NULL,
                period TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'draft',
                form_data TEXT NOT NULL,
                submitted_at TEXT,
                filename TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 1i32).unwrap();

        // Run migrations — should apply v2 and v3
        migrate_database(&conn).unwrap();

        let v: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, CURRENT_MIGRATION_VERSION);

        // v23 (D18) drops the data_providers table v2 created.
        let has_dp: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='data_providers'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        assert!(!has_dp, "data_providers is dropped by v23 (D18)");

        // Verify dedup index from v3
        let has_idx: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='index' AND name='idx_submissions_dedup'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        assert!(
            has_idx,
            "submissions dedup index should exist after v3 migration"
        );
    }

    #[test]
    fn test_legacy_db_runs_incremental_migrations() {
        let conn = test_conn();

        // Create a legacy (unversioned) database with profiles table but no version
        conn.execute_batch(
            "CREATE TABLE profiles (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tin TEXT UNIQUE NOT NULL,
                data_json TEXT NOT NULL
            );
            CREATE TABLE submissions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tin TEXT NOT NULL,
                form_type TEXT NOT NULL,
                period TEXT NOT NULL,
                status TEXT NOT NULL,
                form_data TEXT NOT NULL,
                submitted_at TEXT,
                filename TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        // user_version is 0 by default (legacy)

        migrate_database(&conn).unwrap();

        let v: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, CURRENT_MIGRATION_VERSION);

        // v23 (D18) drops data_providers.
        let has_dp: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='data_providers'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        assert!(!has_dp, "data_providers is dropped by v23 (D18)");
    }

    #[test]
    fn test_v3_migration_deduplicates_existing_submissions() {
        let conn = test_conn();

        // Simulate v2 database with duplicate submissions
        conn.execute_batch(
            "CREATE TABLE profiles (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tin TEXT UNIQUE NOT NULL,
                data_json TEXT NOT NULL
            );
            CREATE TABLE submissions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tin TEXT NOT NULL,
                form_type TEXT NOT NULL,
                period TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'draft',
                form_data TEXT NOT NULL,
                submitted_at TEXT,
                filename TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE data_providers (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                profile_tin TEXT NOT NULL,
                provider_id TEXT NOT NULL,
                name TEXT NOT NULL,
                credentials_json TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT INTO submissions (tin, form_type, period, status, form_data) VALUES ('123', '2551Q', '2024Q1', 'Draft', '{}');
            INSERT INTO submissions (tin, form_type, period, status, form_data) VALUES ('123', '2551Q', '2024Q1', 'Draft', '{}');
            INSERT INTO submissions (tin, form_type, period, status, form_data) VALUES ('123', '2551Q', '2024Q2', 'Draft', '{}');",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 2i32).unwrap();

        // Count before
        let count_before: i64 = conn
            .query_row("SELECT COUNT(*) FROM submissions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count_before, 3);

        // Run v3 migration
        migrate_database(&conn).unwrap();

        // Duplicates should be removed
        let count_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM submissions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count_after, 2, "Duplicate submission should be removed");
    }

    #[test]
    fn test_idempotent_migration() {
        let conn = test_conn();
        migrate_database(&conn).unwrap();
        // Running migrations again should be a no-op
        migrate_database(&conn).unwrap();
        let v: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, CURRENT_MIGRATION_VERSION);
    }

    #[test]
    fn test_v21_creates_legacy_filed_returns() {
        let conn = test_conn();
        conn.pragma_update(None, "user_version", 19i32).unwrap();

        migrate_database(&conn).unwrap();

        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_MIGRATION_VERSION);
        let columns = {
            let mut statement = conn
                .prepare("PRAGMA table_info(legacy_filed_returns)")
                .unwrap();
            statement
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<std::collections::BTreeSet<_>, _>>()
                .unwrap()
        };
        for column in [
            "sha256",
            "tin",
            "form_identity",
            "form_id",
            "period",
            "amendment_version",
            "email_copy",
            "encrypted",
            "source_filename",
            "raw_file",
            "fields_json",
            "imported_at",
        ] {
            assert!(columns.contains(column), "missing column {column}");
        }
        // sha256 + filename dedupes a re-import of the same file, but the
        // same bytes under a different filename is a distinct record.
        conn.execute(
            "INSERT INTO legacy_filed_returns (
                sha256, tin, form_identity, period, encrypted,
                source_filename, raw_file, fields_json
             ) VALUES ('abc', '123456789000', '2551Qv2018', '2026Q1', 1,
                       'file.xml', X'01', '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO legacy_filed_returns (
                sha256, tin, form_identity, period, encrypted,
                source_filename, raw_file, fields_json
             ) VALUES ('abc', '123456789000', '2551Qv2018', '2026Q1', 1,
                       'file#copy.xml', X'01', '{}')",
            [],
        )
        .expect("same bytes under a new filename must insert");
        let err = conn
            .execute(
                "INSERT INTO legacy_filed_returns (
                    sha256, tin, form_identity, period, encrypted,
                    source_filename, raw_file, fields_json
                 ) VALUES ('abc', '123456789000', '2551Qv2018', '2026Q1', 1,
                           'file.xml', X'01', '{}')",
                [],
            )
            .expect_err("duplicate sha256+filename must be rejected");
        assert!(
            err.to_string().contains("UNIQUE"),
            "expected UNIQUE violation, got {err}"
        );
    }

    #[test]
    fn test_v4_promotes_8pct_compat_flag() {
        use crate::profile::{IncomeTaxElection, TaxpayerProfile};

        let conn = test_conn();

        // Run v1-v3 migrations to create the profiles table
        conn.execute_batch(
            "CREATE TABLE profiles (id INTEGER PRIMARY KEY AUTOINCREMENT, tin TEXT UNIQUE NOT NULL, data_json TEXT NOT NULL);",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 3i32).unwrap();

        // Insert a legacy profile with the old compat flag set to true
        let legacy_profile_json = serde_json::json!({
            "id": 1,
            "full_name": "Test Taxpayer",
            "tin": { "segment1": "123", "segment2": "456", "segment3": "789", "branch": "000" },
            "rdo_code": "039",
            "line_of_business": "Consulting",
            "registered_address": "QC",
            "zip_code": "1100",
            "phone": "09170000100",
            "email": "test@example.com",
            "default_form_type": "2551Q",
            "opted_for_8_percent_flat_rate": true,   // old compat field name
            "imap_enabled": true,                     // old compat field name
            "email_tracking_enabled": false,
            "tax_elections": []
        })
        .to_string();

        conn.execute(
            "INSERT INTO profiles (tin, data_json) VALUES (?1, ?2)",
            rusqlite::params!["123-456-789-000", legacy_profile_json],
        )
        .unwrap();

        // Run v4 migration
        migrate_database(&conn).unwrap();

        // Verify the profile was updated
        let updated_json: String = conn
            .query_row(
                "SELECT data_json FROM profiles WHERE tin = '123-456-789-000'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        let updated_profile: TaxpayerProfile = serde_json::from_str(&updated_json).unwrap();

        // 8% election should have been promoted to tax_elections
        assert!(
            !updated_profile.tax_elections.is_empty(),
            "tax_elections should have been populated by v4 migration"
        );
        assert!(
            matches!(
                updated_profile.tax_elections[0].election,
                IncomeTaxElection::EightPercent
            ),
            "election should be EightPercent"
        );
        assert_eq!(
            updated_profile.tax_elections[0].source_form,
            "legacy_compat_migration_v4"
        );

        // email_tracking should have been promoted
        assert!(
            updated_profile.email_tracking_enabled,
            "email_tracking_enabled should have been set to true"
        );
    }

    #[test]
    fn test_v8_migration_backfills_per_year_forms() {
        let conn = test_conn();

        // Initialize schema as if it is at v7
        conn.execute_batch(
            "CREATE TABLE profiles (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tin TEXT UNIQUE NOT NULL,
                data_json TEXT NOT NULL
            );
            CREATE TABLE per_year_forms (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tin TEXT NOT NULL,
                taxable_year INTEGER NOT NULL,
                form_code TEXT NOT NULL,
                frequency TEXT NOT NULL,
                active INTEGER NOT NULL DEFAULT 1,
                source TEXT NOT NULL,
                custom INTEGER NOT NULL DEFAULT 0,
                reason TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(tin, taxable_year, form_code)
            );",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 7i32).unwrap();

        // Let's create a TaxpayerProfile with one confirmed version active in 2026.
        let profile_json = serde_json::json!({
            "id": 1,
            "full_name": "Test Taxpayer",
            "tin": { "segment1": "123", "segment2": "456", "segment3": "789", "branch": "000" },
            "rdo_code": "039",
            "line_of_business": "Consulting",
            "registered_address": "QC",
            "zip_code": "1100",
            "phone": "09170000100",
            "email": "test@example.com",
            "default_form_type": "2551Q",
            "taxpayer_type": "Individual",
            "is_vat_registered": false,
            "compliance_source_mode": "CorVersioned",
            "profile_versions": [
                {
                    "id": "v1",
                    "label": "Version 1",
                    "status": "Confirmed",
                    "source": "ManualCor",
                    "effective_from": "2026-01-01",
                    "effective_until": null,
                    "cor": {},
                    "registered_tax_types": ["IncomeTax", "PercentageTax"],
                    "taxpayer_type": "Individual",
                    "is_vat_registered": false,
                    "obligation_overrides": [
                        {
                            "form_code": "1701",
                            "action": "Include",
                            "reason": "Required manual include"
                        },
                        {
                            "form_code": "2551Q",
                            "action": "Exclude",
                            "reason": "Not filing monthly/quarterly percentage tax"
                        }
                    ]
                }
            ]
        })
        .to_string();

        conn.execute(
            "INSERT INTO profiles (tin, data_json) VALUES (?1, ?2)",
            rusqlite::params!["123-456-789-000", profile_json],
        )
        .unwrap();

        // Run the v8 migration
        migrate_database(&conn).unwrap();

        // Check if user_version reaches the current schema.
        let v: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, CURRENT_MIGRATION_VERSION);

        // Check that per_year_forms has been backfilled
        let mut stmt = conn.prepare(
            "SELECT form_code, active, reason FROM per_year_forms WHERE tin = '123456789000' AND taxable_year = 2026"
        ).unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)? != 0,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .unwrap();

        let mut results = std::collections::HashMap::new();
        for r in rows {
            let (form, active, reason) = r.unwrap();
            results.insert(form, (active, reason));
        }

        // According to our logic:
        // - "1701" is an override to Include, so it should be active = true.
        // - "2551Q" is an override to Exclude, so it should be active = false.
        assert!(results.contains_key("1701"));
        assert!(results.get("1701").unwrap().0);
        assert_eq!(
            results.get("1701").unwrap().1.as_deref(),
            Some("Required manual include")
        );

        assert!(results.contains_key("2551Q"));
        assert!(!results.get("2551Q").unwrap().0);
        assert_eq!(
            results.get("2551Q").unwrap().1.as_deref(),
            Some("Not filing monthly/quarterly percentage tax")
        );
    }

    #[test]
    fn test_v9_migration_preserves_deactivated_standard_forms() {
        let conn = test_conn();

        // Initialize schema as if it is at v7
        conn.execute_batch(
            "CREATE TABLE profiles (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tin TEXT UNIQUE NOT NULL,
                data_json TEXT NOT NULL
            );
            CREATE TABLE per_year_forms (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tin TEXT NOT NULL,
                taxable_year INTEGER NOT NULL,
                form_code TEXT NOT NULL,
                frequency TEXT NOT NULL,
                active INTEGER NOT NULL DEFAULT 1,
                source TEXT NOT NULL,
                custom INTEGER NOT NULL DEFAULT 0,
                reason TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(tin, taxable_year, form_code)
            );",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 8i32).unwrap();

        // Store standard form 2550Q with active = 0 and custom = 0 for year 2026
        conn.execute(
            "INSERT INTO per_year_forms (tin, taxable_year, form_code, frequency, active, source, custom, reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                "123456789000",
                2026,
                "2550Q",
                "quarterly",
                0, // active = 0
                "migration_backfill",
                0, // custom = 0
                "Manually deactivated standard form"
            ],
        )
        .unwrap();

        // Insert a profile version for 2026 where 2550Q is normally active/suggested
        // e.g. a VAT registered profile
        let profile_json = serde_json::json!({
            "id": 1,
            "full_name": "VAT Taxpayer",
            "tin": { "segment1": "123", "segment2": "456", "segment3": "789", "branch": "000" },
            "rdo_code": "039",
            "line_of_business": "Consulting",
            "registered_address": "QC",
            "zip_code": "1100",
            "phone": "09170000100",
            "email": "test@example.com",
            "default_form_type": "2550Q",
            "taxpayer_type": "Individual",
            "is_vat_registered": true,
            "compliance_source_mode": "CorVersioned",
            "profile_versions": [
                {
                    "id": "v1",
                    "label": "Version 1",
                    "status": "Confirmed",
                    "source": "ManualCor",
                    "effective_from": "2026-01-01",
                    "effective_until": null,
                    "cor": {},
                    "registered_tax_types": ["IncomeTax", "ValueAddedTax"],
                    "taxpayer_type": "Individual",
                    "is_vat_registered": true,
                    "obligation_overrides": []
                }
            ]
        })
        .to_string();

        conn.execute(
            "INSERT INTO profiles (tin, data_json) VALUES (?1, ?2)",
            rusqlite::params!["123-456-789-000", profile_json],
        )
        .unwrap();

        // Run the v9 migration (will upgrade from v7 -> v8 -> v9)
        migrate_database(&conn).unwrap();

        // Check if user_version reaches the current schema.
        let v: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, CURRENT_MIGRATION_VERSION);

        // Assert that after migration v9, 2550Q is preserved and active is still false (0)
        let mut stmt = conn.prepare(
            "SELECT form_code, active, custom, reason FROM per_year_forms WHERE tin = '123456789000' AND taxable_year = 2026 AND form_code = '2550Q'"
        ).unwrap();
        let mut rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .unwrap();

        let (form, active, custom, reason) = rows.next().unwrap().unwrap();
        assert_eq!(form, "2550Q");
        assert!(!active);
        assert!(!custom);
        assert_eq!(
            reason.as_deref(),
            Some("Manually deactivated standard form")
        );
    }

    #[test]
    fn test_v9_migration_preserves_custom_forms() {
        let conn = test_conn();

        // Initialize schema as if it is at v7
        conn.execute_batch(
            "CREATE TABLE profiles (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tin TEXT UNIQUE NOT NULL,
                data_json TEXT NOT NULL
            );
            CREATE TABLE per_year_forms (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tin TEXT NOT NULL,
                taxable_year INTEGER NOT NULL,
                form_code TEXT NOT NULL,
                frequency TEXT NOT NULL,
                active INTEGER NOT NULL DEFAULT 1,
                source TEXT NOT NULL,
                custom INTEGER NOT NULL DEFAULT 0,
                reason TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(tin, taxable_year, form_code)
            );",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 8i32).unwrap();

        // Store custom form "9999" with active = 1 and custom = 1 for year 2026
        conn.execute(
            "INSERT INTO per_year_forms (tin, taxable_year, form_code, frequency, active, source, custom, reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                "123456789000",
                2026,
                "9999",
                "open_ended",
                1, // active = 1
                "user",
                1, // custom = 1
                "My custom tax form"
            ],
        )
        .unwrap();

        // Insert a profile version for 2026 where standard forms are suggested
        let profile_json = serde_json::json!({
            "id": 1,
            "full_name": "VAT Taxpayer",
            "tin": { "segment1": "123", "segment2": "456", "segment3": "789", "branch": "000" },
            "rdo_code": "039",
            "line_of_business": "Consulting",
            "registered_address": "QC",
            "zip_code": "1100",
            "phone": "09170000100",
            "email": "test@example.com",
            "default_form_type": "2550Q",
            "taxpayer_type": "Individual",
            "is_vat_registered": true,
            "compliance_source_mode": "CorVersioned",
            "profile_versions": [
                {
                    "id": "v1",
                    "label": "Version 1",
                    "status": "Confirmed",
                    "source": "ManualCor",
                    "effective_from": "2026-01-01",
                    "effective_until": null,
                    "cor": {},
                    "registered_tax_types": ["IncomeTax", "ValueAddedTax"],
                    "taxpayer_type": "Individual",
                    "is_vat_registered": true,
                    "obligation_overrides": []
                }
            ]
        })
        .to_string();

        conn.execute(
            "INSERT INTO profiles (tin, data_json) VALUES (?1, ?2)",
            rusqlite::params!["123-456-789-000", profile_json],
        )
        .unwrap();

        // Run the v9 migration
        migrate_database(&conn).unwrap();

        // Assert that after migration v9, custom form "9999" is preserved and active is true, custom is true
        let mut stmt = conn.prepare(
            "SELECT form_code, active, custom, reason FROM per_year_forms WHERE tin = '123456789000' AND taxable_year = 2026 AND form_code = '9999'"
        ).unwrap();
        let mut rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .unwrap();

        let (form, active, custom, reason) = rows.next().unwrap().unwrap();
        assert_eq!(form, "9999");
        assert!(active);
        assert!(custom);
        assert_eq!(reason.as_deref(), Some("My custom tax form"));
    }

    /// A database as v19 left it: the current schema minus everything v20
    /// adds.
    fn v19_conn() -> Connection {
        let conn = test_conn();
        migrate_database(&conn).unwrap();
        conn.execute_batch(
            "DROP TABLE taxpayers;
             DROP TABLE form_templates;
             ALTER TABLE profiles DROP COLUMN branch_json;
             PRAGMA user_version = 19;",
        )
        .unwrap();
        conn
    }

    fn v19_profile_json(name: &str, tin: &str) -> String {
        use crate::profile::{TaxProfileVersionSource, TaxpayerProfile};
        let mut profile: TaxpayerProfile = serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": name,
            "tin": {
                "segment1": &tin[0..3],
                "segment2": &tin[3..6],
                "segment3": &tin[6..9],
                "branch": &tin[9..]
            },
            "rdo_code": "039",
            "line_of_business": "Consulting",
            "registered_address": "Quezon City",
            "zip_code": "1100",
            "phone": "09170000100",
            "email": "profile@example.com",
            "default_form_type": "2551Q",
            "taxpayer_type": "Individual",
            "eopt_tier": "Micro",
            "business_start_date": "2020-04-15"
        }))
        .unwrap();
        // A confirmed COR version: D3 says it is never edited or deleted.
        profile.ensure_profile_version_ledger();
        profile.profile_versions[0].source = TaxProfileVersionSource::ManualCor;
        serde_json::to_string(&profile).unwrap()
    }

    fn all_rows(conn: &Connection, sql: &str) -> Vec<Vec<Option<String>>> {
        let mut stmt = conn.prepare(sql).unwrap();
        let columns = stmt.column_count();
        stmt.query_map([], |row| {
            (0..columns)
                .map(|index| {
                    row.get::<_, rusqlite::types::Value>(index)
                        .map(|value| match value {
                            rusqlite::types::Value::Null => None,
                            rusqlite::types::Value::Integer(v) => Some(v.to_string()),
                            rusqlite::types::Value::Real(v) => Some(v.to_string()),
                            rusqlite::types::Value::Text(v) => Some(v),
                            rusqlite::types::Value::Blob(v) => Some(format!("{v:?}")),
                        })
                })
                .collect()
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
    }

    fn setting(conn: &Connection, key: &str) -> Option<String> {
        conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()
        .unwrap()
    }

    /// v24 (D20) drops a v23 database's PIN column, the profiles' PIN key
    /// and the access-mode settings, and runs again as a no-op.
    #[test]
    fn test_v24_drops_pins_and_access_mode() {
        let conn = test_conn();
        migrate_database(&conn).unwrap();
        conn.execute_batch(
            "ALTER TABLE taxpayers ADD COLUMN pin_hash TEXT;
             INSERT INTO settings (key, value) VALUES ('access_mode', 'solo');
             INSERT INTO settings (key, value) VALUES ('admin_tin_root', '111111111');
             INSERT INTO settings (key, value) VALUES ('app_lock_pin_hash', 'ADMINHASH');
             PRAGMA user_version = 23;",
        )
        .unwrap();
        let mut profile: serde_json::Value =
            serde_json::from_str(&v19_profile_json("Owner", "11111111100000")).unwrap();
        profile["profile_pin_hash"] = serde_json::json!("old-pin");
        conn.execute(
            "INSERT INTO profiles (tin, data_json) VALUES (?1, ?2)",
            rusqlite::params!["11111111100000", profile.to_string()],
        )
        .unwrap();

        for _ in 0..2 {
            migrate_database(&conn).unwrap();
            let has_pin: bool = conn
                .query_row(
                    "SELECT 1 FROM pragma_table_info('taxpayers') WHERE name = 'pin_hash'",
                    [],
                    |_| Ok(true),
                )
                .optional()
                .unwrap()
                .unwrap_or(false);
            assert!(!has_pin, "taxpayers.pin_hash is dropped");
            for key in V24_DROPPED_SETTINGS {
                assert_eq!(setting(&conn, key), None, "{key} is deleted");
            }
            let left: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM profiles
                     WHERE json_type(data_json, '$.profile_pin_hash') IS NOT NULL",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(left, 0, "no profile keeps a PIN key");
        }
    }

    #[test]
    fn test_v20_groups_profiles_by_tin_root_without_touching_them() {
        use crate::tax_profile::{BranchName, GroupingReview};

        let conn = v19_conn();
        let profiles = [
            // Single branch.
            ("Solo Trader", "11111111100000"),
            // Two branches, same name; legacy 3-digit head-office code.
            ("Same Name Corp", "222222222000"),
            ("same  name corp", "22222222200001"),
            // Different names under one root.
            ("Juan Dela Cruz", "33333333300000"),
            ("Juan Dela Cruz Bakery", "33333333300002"),
            // No head office.
            ("Branch Only", "44444444400003"),
        ];
        for (name, tin) in profiles {
            conn.execute(
                "INSERT INTO profiles (tin, data_json) VALUES (?1, ?2)",
                rusqlite::params![tin, v19_profile_json(name, tin)],
            )
            .unwrap();
        }
        conn.execute_batch(
            "INSERT INTO form_drafts (tin, form_code, taxable_year, quarter, period_key, status, data_json)
             VALUES ('11111111100000', '2551Q', 2025, 4, 'Q4', 'Confirmed', '{\"filed\":true}');
             INSERT INTO per_year_forms (tin, taxable_year, form_code, frequency, source)
             VALUES ('33333333300002', 2026, '2551Q', 'quarterly', 'reviewed_cor');",
        )
        .unwrap();
        let protected = [
            "SELECT id, tin, data_json FROM profiles ORDER BY id",
            "SELECT * FROM form_drafts ORDER BY id",
            "SELECT * FROM per_year_forms ORDER BY id",
        ];
        let before: Vec<_> = protected.iter().map(|sql| all_rows(&conn, sql)).collect();

        migrate_database(&conn).unwrap();

        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_MIGRATION_VERSION);
        let after: Vec<_> = protected.iter().map(|sql| all_rows(&conn, sql)).collect();
        assert_eq!(before, after, "v20 must not rewrite profiles or returns");

        let db = crate::db::Database {
            conn,
            _open_lock: None,
        };
        let taxpayers = db.list_taxpayers().unwrap();
        let summary: Vec<_> = taxpayers
            .iter()
            .map(|entry| {
                (
                    entry.taxpayer.tin_root.as_str(),
                    entry.taxpayer.name.as_str(),
                    entry
                        .branches
                        .iter()
                        .map(|branch| (branch.tin.as_str(), branch.is_head_office))
                        .collect::<Vec<_>>(),
                    entry.taxpayer.grouping_review.clone(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                (
                    "444444444",
                    "Branch Only",
                    vec![("44444444400003", false)],
                    Some(GroupingReview {
                        missing_head_office: true,
                        ..GroupingReview::default()
                    }),
                ),
                (
                    "333333333",
                    "Juan Dela Cruz",
                    vec![("33333333300000", true), ("33333333300002", false)],
                    Some(GroupingReview {
                        differing_names: vec![
                            BranchName {
                                tin: "33333333300000".into(),
                                name: "Juan Dela Cruz".into(),
                            },
                            BranchName {
                                tin: "33333333300002".into(),
                                name: "Juan Dela Cruz Bakery".into(),
                            },
                        ],
                        ..GroupingReview::default()
                    }),
                ),
                (
                    "222222222",
                    "Same Name Corp",
                    vec![("222222222000", true), ("22222222200001", false)],
                    None,
                ),
                (
                    "111111111",
                    "Solo Trader",
                    vec![("11111111100000", true)],
                    None
                ),
            ]
        );
        assert_eq!(
            db.list_taxpayers().unwrap()[2].branches[0].branch_code,
            "00000"
        );

        // Old JSON, COR versions included, still loads through the normal API.
        let loaded = db.list_profiles().unwrap();
        assert_eq!(loaded.len(), profiles.len());
        assert!(loaded.iter().all(|profile| {
            profile.profile_versions[0].source == crate::profile::TaxProfileVersionSource::ManualCor
        }));

        // Idempotent: another startup changes nothing.
        let taxpayers_before = all_rows(&db.conn, "SELECT * FROM taxpayers ORDER BY tin_root");
        migrate_database(&db.conn).unwrap();
        assert_eq!(
            taxpayers_before,
            all_rows(&db.conn, "SELECT * FROM taxpayers ORDER BY tin_root")
        );
        assert_eq!(
            after,
            protected
                .iter()
                .map(|sql| all_rows(&db.conn, sql))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_startup_repair_groups_profiles_added_after_v20_and_drops_orphans() {
        let conn = test_conn();
        migrate_database(&conn).unwrap();
        // An older build (no grouping hook) adds a profile after v20 ran, and
        // its last profile under another root disappears.
        conn.execute(
            "INSERT INTO profiles (tin, data_json) VALUES (?1, ?2)",
            rusqlite::params![
                "55555555500000",
                v19_profile_json("Late Arrival", "55555555500000")
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO taxpayers (tin_root, name, taxpayer_type) VALUES ('666666666', 'Gone', 'Individual')",
            [],
        )
        .unwrap();

        migrate_database(&conn).unwrap();

        let rows = all_rows(
            &conn,
            "SELECT tin_root, name FROM taxpayers ORDER BY tin_root",
        );
        assert_eq!(
            rows,
            vec![vec![Some("555555555".into()), Some("Late Arrival".into())]]
        );
    }
}
