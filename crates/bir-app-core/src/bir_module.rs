//! The general functions of the old `bir` host module: the forms catalog,
//! settings, profiles, drafts, the submission queue, print HTML, import and
//! export, and deadlines.
//!
//! Ported from bir-shell without its account gate, PIN/TOTP sign-in, cloud
//! account, updater, hotkey, agent and inbox functions. A function that used
//! to open a file dialog takes the path instead; the UI picks the file.

use bir_core::db::{self, Database};
use bir_core::forms::FORM_REGISTRY;
use bir_core::forms::queueable::QueueableForm as _;

use crate::database::{json, serialize, to_json, wake_submission_worker, with_database};
use crate::host::{HostArguments, HostError, HostModule, HostObject, HostResult, HostValue};

fn system_info() -> HostResult {
    let retention = with_database(|db| Ok(bir_core::log_files::retention_days(db)))
        .unwrap_or(bir_core::log_files::DEFAULT_RETENTION_DAYS);
    let choices = bir_core::log_files::RETENTION_CHOICES
        .iter()
        .map(|days| HostValue::from(i64::from(*days)))
        .collect::<Vec<_>>();
    Ok(HostObject::new()
        .field("os", std::env::consts::OS)
        .field("arch", std::env::consts::ARCH)
        .field("app_version", env!("CARGO_PKG_VERSION"))
        .field(
            "database_path",
            db::app_database_path().display().to_string(),
        )
        .field(
            "data_dir",
            bir_core::platform::data_dir().display().to_string(),
        )
        .field(
            "logs_dir",
            bir_core::log_files::log_dir().display().to_string(),
        )
        .field("log_retention_days", i64::from(retention))
        .field("log_retention_choices", HostValue::Array(choices))
        .field("runtime", "nsh")
        .into())
}

/// How many days of `ebirforms.*.log` to keep. Out of range is refused.
fn set_log_retention(arguments: &HostArguments) -> HostResult {
    let days = arguments.integer(0)?;
    if !(i64::from(bir_core::log_files::MIN_RETENTION_DAYS)
        ..=i64::from(bir_core::log_files::MAX_RETENTION_DAYS))
        .contains(&days)
    {
        return Err(HostError::new(format!(
            "Log retention must be between {} and {} days.",
            bir_core::log_files::MIN_RETENTION_DAYS,
            bir_core::log_files::MAX_RETENTION_DAYS
        )));
    }
    with_database(|db| {
        db.set_setting(bir_core::log_files::RETENTION_SETTING, &days.to_string())
            .map(|_| HostValue::Null)
            .map_err(|error| HostError::new(error.to_string()))?;
        let _ = bir_core::log_files::maintain(&bir_core::log_files::log_dir(), db);
        Ok(HostValue::Null)
    })
}

fn list_profiles() -> HostResult {
    with_database(|db| {
        let profiles = db
            .list_profiles()
            .map_err(|error| HostError::new(error.to_string()))?;
        let mut rows = Vec::with_capacity(profiles.len());
        for profile in &profiles {
            let tin = profile.tin.full();
            rows.push(
                HostObject::new()
                    .field("id", profile.id.unwrap_or_default())
                    .field("tin", tin)
                    .field("full_name", profile.full_name.as_str())
                    .field("rdo_code", profile.rdo_code.as_str())
                    .field("email", profile.email.as_str())
                    .field("taxpayer_type", format!("{:?}", profile.taxpayer_type))
                    .field("is_vat_registered", profile.is_vat_registered)
                    .field("is_archived", profile.is_archived)
                    .field("default_form_type", profile.default_form_type.as_str())
                    .field(
                        "applicable_forms",
                        profile
                            .applicable_forms()
                            .into_iter()
                            .map(HostValue::Str)
                            .collect::<Vec<_>>(),
                    ),
            );
        }
        Ok(HostValue::Array(
            rows.into_iter().map(HostValue::from).collect(),
        ))
    })
}

fn forms_catalog() -> HostResult {
    let mut rows = Vec::with_capacity(FORM_REGISTRY.len());
    for form in FORM_REGISTRY {
        rows.push(HostValue::from(
            HostObject::new()
                .field("code", form.code)
                .field("title", form.title)
                .field("category", form.category)
                .field("frequency", format!("{:?}", form.frequency))
                .field("requires_employees", form.requires_employees)
                .field("is_deprecated", form.is_deprecated),
        ));
    }
    Ok(HostValue::Array(rows))
}

/// The settings key a setting call names. The UI runs in process and may
/// use any key (the old page was limited to its theme toggle).
fn page_setting_key(arguments: &HostArguments) -> Result<String, HostError> {
    Ok(arguments.string(0)?.to_owned())
}

fn get_setting(arguments: &HostArguments) -> HostResult {
    let key = page_setting_key(arguments)?;
    with_database(|db| {
        db.get_setting(&key)
            .map(HostValue::from)
            .map_err(|error| HostError::new(error.to_string()))
    })
}

fn set_setting(arguments: &HostArguments) -> HostResult {
    let key = page_setting_key(arguments)?;
    let value = arguments.string(1)?.to_owned();
    with_database(|db| {
        db.set_setting(&key, &value)
            .map(|_| HostValue::Null)
            .map_err(|error| HostError::new(error.to_string()))
    })
}

// ---------- profiles ----------

fn get_profile_by_tin(
    db: &Database,
    tin: &str,
) -> Result<bir_core::profile::TaxpayerProfile, HostError> {
    db.get_profile_by_tin(tin)
        .map_err(|error| HostError::new(error.to_string()))?
        .ok_or_else(|| HostError::new(format!("no taxpayer profile with TIN {tin}")))
}

/// A profile as the page receives it.
fn profile_record(profile: bir_core::profile::TaxpayerProfile) -> HostResult {
    serde_json::to_value(&profile).map(json).map_err(db_error)
}

/// The stored profile for `tin`.
fn profile_detail(arguments: &HostArguments) -> HostResult {
    let tin = arguments.string(0)?;
    with_database(|db| profile_record(get_profile_by_tin(db, tin)?))
}

/// Save the page's profile; the answer is the saved profile.
fn save_profile(arguments: &HostArguments) -> HostResult {
    let profile: bir_core::profile::TaxpayerProfile =
        serde_json::from_value(to_json(arguments.value(0)?))
            .map_err(|error| HostError::new(format!("profile payload: {error}")))?;
    with_database(|db| {
        db.save_profile(profile)
            .map_err(|error| HostError::new(error.to_string()))
            .and_then(profile_record)
    })
}

fn set_profile_archived(arguments: &HostArguments) -> HostResult {
    let tin = arguments.string(0)?.to_owned();
    let archived = arguments.boolean(1)?;
    with_database(|db| {
        let mut profile = get_profile_by_tin(db, &tin)?;
        profile.is_archived = archived;
        db.save_profile(profile)
            .map_err(|error| HostError::new(error.to_string()))
            .and_then(profile_record)
    })
}

fn delete_profile(arguments: &HostArguments) -> HostResult {
    let tin = arguments.string(0)?;
    with_database(|db| {
        db.delete_profile(tin)
            .map(|_| HostValue::Null)
            .map_err(|error| HostError::new(error.to_string()))
    })
}

// ---------- sign-in: one lock per taxpayer, admin by access mode ----------

fn db_error(error: impl std::fmt::Display) -> HostError {
    HostError::new(error.to_string())
}

fn tin_root_argument(raw: &str) -> Result<String, HostError> {
    bir_core::tax_profile::parse_tin_root(raw).map_err(db_error)
}

fn delete_setting(arguments: &HostArguments) -> HostResult {
    let key = page_setting_key(arguments)?;
    with_database(|db| {
        db.delete_setting(&key)
            .map(|_| HostValue::Null)
            .map_err(db_error)
    })
}

fn set_taxpayer_archived(arguments: &HostArguments) -> HostResult {
    let root = tin_root_argument(arguments.string(0)?)?;
    let archived = arguments.boolean(1)?;
    with_database(|db| {
        db.set_taxpayer_archived(&root, archived)
            .map(|_| HostValue::Null)
            .map_err(db_error)
    })
}

fn delete_taxpayer(arguments: &HostArguments) -> HostResult {
    let root = tin_root_argument(arguments.string(0)?)?;
    with_database(|db| {
        db.delete_taxpayer(&root)
            .map(|_| HostValue::Null)
            .map_err(db_error)
    })
}

// ---------- S3: form drafts ----------

fn list_drafts(arguments: &HostArguments) -> HostResult {
    let tin = arguments.string(0)?.to_owned();
    let year = arguments.integer(1)? as u16;
    with_database(|db| {
        let rows = db
            .list_draft_summaries(&tin, year)
            .map_err(|error| HostError::new(error.to_string()))?;
        Ok(HostValue::Array(
            rows.iter()
                .map(|row| {
                    HostValue::from(
                        HostObject::new()
                            .field("id", row.id)
                            .field("tin", row.tin.as_str())
                            .field("form_code", row.form_code.as_str())
                            .field("taxable_year", row.taxable_year as i64)
                            .field(
                                "quarter",
                                row.quarter.map_or(HostValue::Null, HostValue::from),
                            )
                            .field("month", row.month.map_or(HostValue::Null, HostValue::from))
                            .field("status", format!("{:?}", row.status))
                            .field("updated_at", row.updated_at.as_str()),
                    )
                })
                .collect(),
        ))
    })
}

fn filing_status(name: &str) -> Result<bir_core::forms::FilingStatus, HostError> {
    use bir_core::forms::FilingStatus::*;
    match name {
        "Draft" => Ok(Draft),
        "Queued" => Ok(Queued),
        "Sent" | "Submitted" | "Filed" => Ok(Sent),
        "Confirmed" => Ok(Confirmed),
        "Paid" => Ok(Paid),
        other => Err(HostError::new(format!("unknown filing status {other:?}"))),
    }
}

/// `quarter` argument is `Option<u8>`: pass null for annual/open-ended slots
/// and 0 for the first open-ended period (the DB auto-numbers those).
fn optional_quarter(arguments: &HostArguments, index: usize) -> Result<Option<u8>, HostError> {
    match arguments.get(index) {
        Some(HostValue::Null) | None => Ok(None),
        Some(HostValue::Number(q)) => Ok(Some(*q as u8)),
        Some(other) => Err(HostError::new(format!(
            "quarter must be a number or null, got {}",
            other.describe()
        ))),
    }
}

fn get_form_draft(arguments: &HostArguments) -> HostResult {
    let tin = arguments.string(0)?.to_owned();
    let form_code = arguments.string(1)?.to_owned();
    let year = arguments.integer(2)? as u16;
    let quarter = optional_quarter(arguments, 3)?;
    with_database(|db| {
        let draft: Option<serde_json::Value> = match form_code.as_str() {
            "2551Q" => {
                let q = quarter.unwrap_or(1);
                db.get_2551q_draft(&tin, year, q)
                    .map_err(|error| HostError::new(error.to_string()))?
                    .map(|d| {
                        serde_json::to_value(&d).map_err(|error| HostError::new(error.to_string()))
                    })
                    .transpose()?
            }
            "1601C" => {
                let month = quarter.unwrap_or(1);
                db.get_1601c_draft(&tin, year, month)
                    .map_err(|error| HostError::new(error.to_string()))?
                    .map(|d| {
                        serde_json::to_value(&d).map_err(|error| HostError::new(error.to_string()))
                    })
                    .transpose()?
            }
            _ => db
                .get_form_draft(&tin, &form_code, year, quarter)
                .map_err(|error| HostError::new(error.to_string()))?,
        };
        match draft {
            Some(value) => serialize(&value),
            None => Ok(HostValue::Null),
        }
    })
}

/// Fields the script reports merge over a profile-derived draft before the
/// typed save — the same overlay shape the painted app uses, so the draft
/// keeps every engine-required field even when the UI edits a subset.
fn overlay_fields<T: serde::Serialize + serde::de::DeserializeOwned>(
    base: T,
    data: &serde_json::Value,
) -> Result<T, HostError> {
    let mut merged =
        serde_json::to_value(&base).map_err(|error| HostError::new(error.to_string()))?;
    if let (Some(target), Some(source)) = (merged.as_object_mut(), data.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
    serde_json::from_value(merged).map_err(|error| HostError::new(error.to_string()))
}

/// A generic save never stores a client's `trigger_date_model`,
/// `trigger_date_entered` or `trigger_parts_typed` (D8c, P5-7). The row it
/// checks is the one the save writes
/// ([`bir_core::db::Database::form_draft_for_save`]: a new open-ended
/// entry has none); that row's three fields are kept when it is the same
/// draft (`created_at`) and holds exactly these trigger fields, otherwise
/// all three are removed — this path creates no marker itself.
fn keep_stored_trigger_flag(
    db: &Database,
    tin: &str,
    form_code: &str,
    year: u16,
    quarter: Option<u8>,
    data: &mut serde_json::Value,
) -> Result<(), HostError> {
    let Some(object) = data.as_object_mut() else {
        return Ok(());
    };
    let fields = bir_core::forms::event_trigger::trigger_paths(form_code);
    let stored: Option<serde_json::Value> = if fields.is_empty() {
        None
    } else {
        db.form_draft_for_save(tin, form_code, year, quarter)
            .map_err(|error| HostError::new(error.to_string()))?
    };
    let same = stored.filter(|stored| {
        stored.get("created_at").is_some()
            && stored.get("created_at") == object.get("created_at")
            && fields
                .iter()
                .all(|field| stored.get(*field) == object.get(*field))
    });
    // The marker is host state like the typed record beside it (decision
    // P5-7): the stored row's is copied, a missing stored marker removes
    // the client's — this save is not a creator and never mints one.
    for field in [
        "trigger_date_model",
        "trigger_date_entered",
        "trigger_parts_typed",
    ] {
        match same.as_ref().and_then(|stored| stored.get(field)) {
            Some(value) => object.insert(field.into(), value.clone()),
            None => object.remove(field),
        };
    }
    Ok(())
}

fn save_form_draft(arguments: &HostArguments) -> HostResult {
    let tin = arguments.string(0)?.to_owned();
    let form_code = arguments.string(1)?.to_owned();
    let year = arguments.integer(2)? as u16;
    let quarter = optional_quarter(arguments, 3)?;
    let status = filing_status(arguments.string(4)?)?;
    let mut data = to_json(arguments.value(5)?);
    with_database(|db| {
        // The engine requires dedicated typed persistence for these two forms;
        // route them inside the host so the script keeps one uniform call.
        match form_code.as_str() {
            "2551Q" => {
                let profile = db
                    .get_profile(&tin)
                    .map_err(|error| HostError::new(error.to_string()))?
                    .ok_or_else(|| HostError::new(format!("no profile for TIN {tin}")))?;
                let q = quarter.ok_or_else(|| {
                    HostError::new("2551Q drafts are quarterly: pass quarter 1-4")
                })?;
                let base = bir_core::forms::Form2551QDraft::new_from_profile(&profile, year, q);
                let draft = overlay_fields(base, &data)?;
                db.save_2551q_draft(&draft)
                    .map(|id| HostValue::Number(id as f64))
                    .map_err(|error| HostError::new(error.to_string()))
            }
            "1601C" => {
                let profile = db
                    .get_profile(&tin)
                    .map_err(|error| HostError::new(error.to_string()))?
                    .ok_or_else(|| HostError::new(format!("no profile for TIN {tin}")))?;
                let month = quarter.ok_or_else(|| {
                    HostError::new("1601C drafts are monthly: pass the month in the quarter slot")
                })?;
                let base = bir_core::forms::Form1601CDraft::new_from_profile(&profile, year, month);
                let draft = overlay_fields(base, &data)?;
                db.save_1601c_draft(&draft)
                    .map(|id| HostValue::Number(id as f64))
                    .map_err(|error| HostError::new(error.to_string()))
            }
            _ => {
                keep_stored_trigger_flag(db, &tin, &form_code, year, quarter, &mut data)?;
                db.save_form_draft(&tin, &form_code, year, quarter, &status, &data)
                    .map(|id| HostValue::Number(id as f64))
                    .map_err(|error| HostError::new(error.to_string()))
            }
        }
    })
}

/// Every queued submission across profiles — the queue page's feed.
fn list_queued() -> HostResult {
    with_database(|db| {
        let summaries = db
            .list_all_queued_submissions()
            .map_err(|error| HostError::new(error.to_string()))?;
        let items: Vec<serde_json::Value> = summaries
            .iter()
            .map(|s| {
                serde_json::json!({
                    "id": s.id,
                    "tin": s.tin,
                    "form_code": s.form_code,
                    "taxable_year": s.taxable_year,
                    "quarter": s.quarter,
                    "month": s.month,
                    "status": format!("{:?}", s.status),
                    "updated_at": s.updated_at,
                })
            })
            .collect();
        serialize(&items)
    })
}

/// Move a saved draft into the queue. Generic forms go through
/// `QueueableKind` + `save_queued_queueable`; the two engine-guarded forms
/// use their dedicated transition + persistence, mirroring the painted views.
fn queue_form_draft(arguments: &HostArguments) -> HostResult {
    let tin = arguments.string(0)?.to_owned();
    let form_code = arguments.string(1)?.to_owned();
    let year = arguments.integer(2)? as u16;
    let quarter = optional_quarter(arguments, 3)?;
    let queued = with_database(|db| {
        let queued: Result<i64, HostError> = (|| match form_code.as_str() {
            "2551Q" => crate::screen_form_module::out_of_line(|| {
                let q = quarter.ok_or_else(|| {
                    HostError::new("2551Q drafts are quarterly: pass quarter 1-4")
                })?;
                db.confirm_initial_profile_year(&tin, i32::from(year))
                    .map_err(|error| HostError::new(error.to_string()))?;
                let mut draft = db
                    .get_2551q_draft(&tin, year, q)
                    .map_err(|error| HostError::new(error.to_string()))?
                    .ok_or_else(|| HostError::new("No saved 2551Q draft for that period"))?;
                if let Some(profile) = db
                    .get_profile_by_tin(&tin)
                    .map_err(|error| HostError::new(error.to_string()))?
                {
                    // A stored resolution error clears once a confirmed version
                    // covers the quarter. The transition still runs the page's
                    // Validate (`filing_errors`).
                    let _ = draft.reconcile_with_effective_profile(&profile);
                    // Queued with the penalties of the Calendar's deadline now.
                    draft.apply_penalty_profile(&profile, &db.get_deadline_overrides());
                }
                draft.transition_to_queued().map_err(|errors| {
                    HostError::new(
                        errors
                            .iter()
                            .map(|(field, msg)| format!("{field}: {msg}"))
                            .collect::<Vec<_>>()
                            .join("; "),
                    )
                })?;
                db.save_queued_2551q_draft_and_election(&draft)
                    .map_err(|error| HostError::new(error.to_string()))
            }),
            "1601C" => crate::screen_form_module::out_of_line(|| {
                let month = quarter
                    .ok_or_else(|| HostError::new("1601C drafts are monthly: pass the month"))?;
                let mut draft = db
                    .get_1601c_draft(&tin, year, month)
                    .map_err(|error| HostError::new(error.to_string()))?
                    .ok_or_else(|| HostError::new("No saved 1601C draft for that period"))?;
                // Queued with the penalties of the Calendar's deadline now.
                if let Some(profile) = db
                    .get_profile_by_tin(&tin)
                    .map_err(|error| HostError::new(error.to_string()))?
                {
                    draft.apply_penalty_profile(&profile, &db.get_deadline_overrides());
                }
                draft.transition_to_queued().map_err(|errors| {
                    HostError::new(
                        errors
                            .iter()
                            .map(|(field, msg)| format!("{field}: {msg}"))
                            .collect::<Vec<_>>()
                            .join("; "),
                    )
                })?;
                db.save_queued_1601c_draft(&draft)
                    .map_err(|error| HostError::new(error.to_string()))
            }),
            _ => {
                use bir_core::forms::queueable::QueueableKind;
                let kind = QueueableKind::from_form_code(&form_code)
                    .ok_or_else(|| HostError::new(format!("Form {form_code} is not queueable")))?;
                bir_core::with_queueable_kind!(kind, F => crate::screen_form_module::out_of_line(|| {
                    let mut draft: F = db
                        .get_form_draft(&tin, &form_code, year, quarter)
                        .map_err(|error| HostError::new(error.to_string()))?
                        .ok_or_else(|| {
                            HostError::new(format!("No saved {form_code} draft for that period"))
                        })?;
                    draft
                        .queue(bir_core::filing_queue::QueueAuthSource::Gui)
                        .map_err(|errors| {
                            HostError::new(
                                errors
                                    .iter()
                                    .map(|(field, msg)| format!("{field}: {msg}"))
                                    .collect::<Vec<_>>()
                                    .join("; "),
                            )
                        })?;
                    db.save_queued_queueable(&draft)
                        .map_err(|error| HostError::new(error.to_string()))
                }))
            }
        })();
        queued.map(|id| HostValue::Number(id as f64))
    })?;
    wake_submission_worker();
    Ok(queued)
}

/// Cancel an unclaimed queued submission back to Draft (same guards as the
/// painted views: claimed or submitted work cannot be canceled this way).
fn cancel_queued(arguments: &HostArguments) -> HostResult {
    let tin = arguments.string(0)?.to_owned();
    let form_code = arguments.string(1)?.to_owned();
    let year = arguments.integer(2)? as u16;
    let quarter = optional_quarter(arguments, 3)?;
    let cancelled = with_database(|db| {
        let result: Result<(), HostError> = (|| match form_code.as_str() {
            "2551Q" => crate::screen_form_module::out_of_line(|| {
                let q = quarter.ok_or_else(|| {
                    HostError::new("2551Q drafts are quarterly: pass quarter 1-4")
                })?;
                let draft = db
                    .get_2551q_draft(&tin, year, q)
                    .map_err(|error| HostError::new(error.to_string()))?
                    .ok_or_else(|| HostError::new("No queued 2551Q draft for that period"))?;
                db.cancel_queued_2551q_submission(&draft)
                    .map_err(|error| HostError::new(error.to_string()))?;
                Ok(())
            }),
            "1601C" => crate::screen_form_module::out_of_line(|| {
                let month = quarter
                    .ok_or_else(|| HostError::new("1601C drafts are monthly: pass the month"))?;
                let draft = db
                    .get_1601c_draft(&tin, year, month)
                    .map_err(|error| HostError::new(error.to_string()))?
                    .ok_or_else(|| HostError::new("No queued 1601C draft for that period"))?;
                db.cancel_queued_1601c_submission(&draft)
                    .map_err(|error| HostError::new(error.to_string()))?;
                Ok(())
            }),
            _ => {
                use bir_core::forms::queueable::QueueableKind;
                let kind = QueueableKind::from_form_code(&form_code)
                    .ok_or_else(|| HostError::new(format!("Form {form_code} is not queueable")))?;
                bir_core::with_queueable_kind!(kind, F => crate::screen_form_module::out_of_line(|| {
                    let draft: F = db
                        .get_form_draft(&tin, &form_code, year, quarter)
                        .map_err(|error| HostError::new(error.to_string()))?
                        .ok_or_else(|| {
                            HostError::new(format!("No queued {form_code} draft for that period"))
                        })?;
                    db.cancel_queued_queueable(&draft)
                        .map_err(|error| HostError::new(error.to_string()))?;
                    Ok(())
                }))
            }
        })();
        result.map(|_| HostValue::Null)
    })?;
    wake_submission_worker();
    Ok(cancelled)
}

/// Today's rotated log (`ebirforms.YYYY-MM-DD.log`), tailed like the painted
/// Logs tab — it keeps at most the newest 5,000 lines.
pub(crate) fn job_log_tail() -> HostResult {
    let logs_path = bir_core::log_files::today_log_path(&bir_core::log_files::log_dir());
    let file_name = logs_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (content, lines) = match std::fs::read_to_string(&logs_path) {
        Ok(text) => {
            let total = text.lines().count();
            let tail = if total <= 5_000 {
                text
            } else {
                text.lines()
                    .skip(total - 5_000)
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            (tail, total)
        }
        Err(_) => ("No log entries yet today.".to_string(), 0),
    };
    serialize(&serde_json::json!({
        "file_name": file_name,
        "lines": lines,
        "content": content,
    }))
}

/// The frozen print-sheet slug each form's painted `PRINT_SLUG` names.
pub(crate) fn print_slug_for(form_code: &str) -> Option<&'static str> {
    Some(match form_code {
        "0605" => "0605-1999",
        "0619E" => "0619e-2018",
        "0619F" => "0619f-2018",
        "1600PT" => "1600-pt-2018",
        "1600VT" => "1600-vt-2018",
        "1600WP" => "1600wp-2010",
        "1601C" => "1601c-2018",
        "1601EQ" => "1601eq-2019",
        "1601FQ" => "1601-fq-2020",
        "1602Q" => "1602q-2019",
        "1603Q" => "1603q-2018",
        "1604C" => "1604c-2018",
        "1604E" => "1604e-2018",
        "1604F" => "1604f-2018",
        "1606" => "1606-2018",
        "1700" => "1700-2018",
        "1701" => "1701-2018",
        "1701A" => "1701a-2018",
        "1701MS" => "1701ms-2024",
        "1701Q" => "1701q-2018",
        "1702EX" => "1702ex-2018",
        "1702MX" => "1702mx-2018c",
        "1702Q" => "1702q-2018",
        "1702RT" => "1702rt-2018c",
        "1706" => "1706-2018",
        "1707" => "1707-2021",
        "1707A" => "1707a-2021",
        "1800" => "1800-2018",
        "1801" => "1801-2018",
        "2000" => "2000-dst-2018",
        "2000OT" => "2000-ot-2018",
        "2200A" => "2200a-2020",
        "2200AN" => "2200an-2018",
        "2200C" => "2200c-2018",
        "2200M" => "2200m-2018",
        "2200P" => "2200p-2020",
        "2200S" => "2200s-2018",
        "2200T" => "2200t-2022",
        "2550M" => "2550m-2007",
        "2550Q" => "2550q-2024",
        "2551Q" => "2551q-2018",
        "2552" => "2552-2018",
        "2553" => "2553-1999",
        _ => return None,
    })
}

/// The receipt page a confirmed draft appends — the shell port of the painted
/// `form_html_preview_launcher::receipt_page_for`.
pub(crate) fn receipt_page(
    db: &Database,
    receipt_id: Option<i64>,
    mailbox: &str,
) -> Option<bir_print::frozen_html::ReceiptPage> {
    let receipt_id = receipt_id?;
    let receipt = db.get_submission_receipt_by_id(receipt_id).ok().flatten()?;
    Some(bir_print::frozen_html::ReceiptPage {
        filename: receipt.filename.clone(),
        subject: "Tax Return Receipt Confirmation".to_string(),
        from: receipt
            .source_from
            .clone()
            .unwrap_or_else(|| "BIR".to_string()),
        to: mailbox.to_string(),
        received_at: bir_core::background_cron::display_received_at(
            &receipt.received_date,
            &receipt.received_time,
        ),
        email_received_at: receipt.email_received_at.clone(),
        body_text: receipt.raw_text.clone(),
        // Stored HTML is not rendered: the email sanitizer went with the
        // inbox code (D6), so the receipt page shows the plain text.
        body_html: None,
    })
}

pub(crate) fn print_fields(
    db: &Database,
    tin: &str,
    form_code: &str,
    year: u16,
    quarter: Option<u8>,
) -> Result<
    (
        std::collections::BTreeMap<String, String>,
        Option<bir_print::frozen_html::ReceiptPage>,
    ),
    HostError,
> {
    match form_code {
        "2551Q" => crate::screen_form_module::out_of_line(|| {
            let q = quarter.ok_or_else(|| HostError::new("2551Q drafts are quarterly"))?;
            let draft = db
                .get_2551q_draft(tin, year, q)
                .map_err(|error| HostError::new(error.to_string()))?
                .ok_or_else(|| HostError::new("No saved 2551Q draft for that period"))?;
            let page = receipt_page(db, draft.receipt_id, &draft.email);
            Ok((draft.to_print_field_map(), page))
        }),
        "1601C" => crate::screen_form_module::out_of_line(|| {
            let month = quarter.ok_or_else(|| HostError::new("1601C drafts are monthly"))?;
            let draft = db
                .get_1601c_draft(tin, year, month)
                .map_err(|error| HostError::new(error.to_string()))?
                .ok_or_else(|| HostError::new("No saved 1601C draft for that period"))?;
            let page = receipt_page(db, draft.receipt_id, &draft.email_address);
            Ok((draft.to_print_field_map(), page))
        }),
        _ => {
            use bir_core::forms::queueable::QueueableKind;
            let kind = QueueableKind::from_form_code(form_code)
                .ok_or_else(|| HostError::new(format!("Form {form_code} is not printable")))?;
            bir_core::with_queueable_kind!(kind, F => crate::screen_form_module::out_of_line(|| {
                let draft: F = db
                    .get_form_draft(tin, form_code, year, quarter)
                    .map_err(|error| HostError::new(error.to_string()))?
                    .ok_or_else(|| {
                        HostError::new(format!("No saved {form_code} draft for that period"))
                    })?;
                let page = receipt_page(
                    db,
                    draft.lifecycle().receipt_id,
                    draft.submission_email(),
                );
                Ok((draft.print_field_map(), page))
            }))
        }
    }
}

/// The form's frozen print sheet filled from the stored draft, as static
/// HTML: `{ html, title }`. The UI shows it in its print preview.
fn form_print_html(arguments: &HostArguments) -> HostResult {
    let tin = arguments.string(0)?.to_owned();
    let form_code = arguments.string(1)?.to_owned();
    let year = arguments.integer(2)? as u16;
    let quarter = optional_quarter(arguments, 3)?;
    let slug = print_slug_for(&form_code)
        .ok_or_else(|| HostError::new(format!("Form {form_code} has no print sheet")))?;
    with_database(|db| {
        let (fields, receipt) = print_fields(db, &tin, &form_code, year, quarter)?;
        let profile = db
            .get_profile_by_tin(&tin)
            .ok()
            .flatten()
            .map(|p| p.full_name.clone());
        let (html, title) =
            print_preview_page(slug, &form_code, &fields, receipt.as_ref(), profile)?;
        Ok(HostObject::new()
            .field("html", html)
            .field("title", title)
            .into())
    })
}

/// `slug`'s frozen sheet filled with `fields` as the preview shows it, and
/// the preview's title ("BIR Form 2551Q — <taxpayer>"). Opens and writes
/// nothing.
pub(crate) fn print_preview_page(
    slug: &str,
    form_code: &str,
    fields: &std::collections::BTreeMap<String, String>,
    receipt: Option<&bir_print::frozen_html::ReceiptPage>,
    profile_name: Option<String>,
) -> Result<(String, String), HostError> {
    let html = bir_print::frozen_html::filled_document_with_receipt(slug, fields, receipt)
        .map_err(HostError::new)?;
    // A4 layout + static markup (no scripts, values as text): prints one A4
    // page per form page from any engine; `print-overrides/<slug>.css` on top.
    let html = bir_print::static_html::staticize(&bir_print::print_sheet::prepare_html_for(
        slug, &html, true,
    ));
    let title = match profile_name.filter(|n| !n.trim().is_empty()) {
        Some(name) => format!("BIR Form {form_code} \u{2014} {name}"),
        None => format!("BIR Form {form_code}"),
    };
    Ok((html, title))
}

/// The UI's import: the archive at `path` (the UI picks the file).
fn import_archive(arguments: &HostArguments) -> HostResult {
    import_archive_from(std::path::Path::new(arguments.string(0)?))
}

/// Swap the live connection for an in-memory placeholder so the file can be
/// replaced, run `replace`, then reopen the file into the same handle (the
/// submission worker shares it).
fn replace_database_file(
    db_path: &std::path::Path,
    replace: impl FnOnce() -> Result<(), HostError>,
) -> Result<(), HostError> {
    let handle = crate::database::database_handle()?;
    let mut guard = handle
        .lock()
        .map_err(|_| HostError::new("database lock poisoned"))?;
    let placeholder = Database::open_ephemeral().map_err(db_error)?;
    let live = std::mem::replace(&mut *guard, placeholder);
    if let Err(error) = live.close() {
        tracing::warn!("database close failed: {error}");
    }
    let replaced = replace();
    *guard = Database::open(db_path).map_err(db_error)?;
    replaced
}

/// Import the archive at `path`; not a verb. Archive handling stays
/// host-side and mirrors `ImportExportView`: a zip with `bir_data.db` is a
/// full database backup, one with any `profile.json` — top-level, or under
/// `Profiles/<TIN>/` in a Settings all-profiles backup — is a profile
/// export, anything else is invalid.
fn import_archive_from(path: &std::path::Path) -> HostResult {
    // What `import_profile_data` accepts: any `profile.json` — top-level
    // (one profile) or under `Profiles/<TIN>/` (the Settings all-profiles
    // backup). A database backup's marker is a top-level `bir_data.db`.
    let entry_names: Vec<String> = std::fs::File::open(path)
        .ok()
        .and_then(|file| zip::ZipArchive::new(file).ok())
        .map(|mut archive| {
            (0..archive.len())
                .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
                .collect()
        })
        .unwrap_or_default();
    let is_db = entry_names.iter().any(|name| name == "bir_data.db");
    let is_profile = entry_names
        .iter()
        .any(|name| name.ends_with("profile.json"));

    if is_db {
        // Painted flow (import_export.rs): safety-backup the live file, then
        // extract the archive over it. Drop the cached connection first — it
        // holds the owner lock and an open handle on the file being replaced.
        let db_path = crate::database::database_path();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0);
        if db_path.exists()
            && let Some(parent) = db_path.parent()
        {
            // Flush the WAL first: a raw copy of bir_data.db alone can miss
            // commits still living in -wal, and this copy is the restore
            // point if the extract corrupts the database.
            with_database(|db| db.checkpoint().map_err(db_error))?;
            let _ = std::fs::copy(
                &db_path,
                parent.join(format!("bir_data.safety-{timestamp}.db")),
            );
        }
        replace_database_file(&db_path, || {
            bir_core::import::extract_database_zip(path, &db_path)
                .map_err(|error| HostError::new(error.to_string()))
        })?;
        return Ok(HostObject::new().field("kind", "database").into());
    }

    if is_profile {
        return with_database(|db| match bir_core::import::import_profile_data(db, path) {
            Ok(()) => Ok(HostObject::new().field("kind", "profile").into()),
            Err(error) => Err(HostError::new(error.to_string())),
        });
    }

    Ok(HostObject::new().field("kind", "invalid").into())
}

/// The UI's export: `tin`'s tax profile as a `.zip` archive at `path` (the
/// UI picks the file). No PIN gate (decision D17).
fn export_profile(arguments: &HostArguments) -> HostResult {
    let tin = arguments.string(0)?;
    let path = std::path::Path::new(arguments.string(1)?);
    with_database(|db| {
        bir_core::export_profile_data(db, tin, path).map_err(db_error)?;
        Ok(HostObject::new()
            .field("kind", "exported")
            .field("path", path.display().to_string())
            .into())
    })
}

/// Tax deadlines feed for the calendar page.
fn list_deadlines() -> HostResult {
    with_database(|db| {
        db.list_tax_deadlines()
            .map_err(|error| HostError::new(error.to_string()))
            .and_then(|deadlines| serialize(&deadlines))
    })
}

/// Every ported JSON-shaped function, by name.
pub fn module() -> HostModule {
    let module = HostModule::new("bir")
        .function("system_info", |_| system_info())
        .function("set_log_retention", set_log_retention)
        .function("list_profiles", |_| list_profiles())
        .function("forms_catalog", |_| forms_catalog())
        .function("get_setting", get_setting)
        .function("set_setting", set_setting)
        .function("delete_setting", delete_setting)
        .function("profile_detail", profile_detail)
        .function("save_profile", save_profile)
        .function("set_profile_archived", set_profile_archived)
        .function("delete_profile", delete_profile)
        .function("set_taxpayer_archived", set_taxpayer_archived)
        .function("delete_taxpayer", delete_taxpayer)
        .function("list_drafts", list_drafts)
        .function("get_form_draft", get_form_draft)
        .function("save_form_draft", save_form_draft)
        .function("list_queued", |_| list_queued())
        .function("queue_form_draft", queue_form_draft)
        .function("cancel_queued", cancel_queued)
        .function("list_deadlines", |_| list_deadlines())
        .function("import_archive", import_archive)
        .function("export_profile", export_profile)
        .function("form_print_html", form_print_html)
        .function("job_log_tail", |_| job_log_tail());
    let module = crate::tax_profile_module::register(module);
    let module = crate::screen_form_module::register(module);
    crate::dashboard_module::register(module)
}
