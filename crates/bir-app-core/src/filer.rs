//! The filer's own records around the returns: settings, deleting a draft,
//! saved payees, backup and restore, and the penalty a late period costs.
//!
//! Like every [`BirApp`] method that takes `&self`, these read or write the
//! database: call them from a worker thread.

use std::path::Path;

use bir_core::penalties::{PenaltyProfile, PenaltyRequest, compute_penalties};
use serde::{Deserialize, Serialize};

use crate::{AppError, AppResult, BirApp};

/// A payee the filer entered in a schedule once: offered again in the next
/// schedule's TIN picker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payee {
    pub name: String,
    /// Digits only, 9 or 14.
    pub tin: String,
}

/// What filing a return now costs on top of its tax, from the due date.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PenaltyEstimate {
    pub days_late: i64,
    pub surcharge: f64,
    pub interest: f64,
    pub compromise: f64,
}

impl PenaltyEstimate {
    pub fn total(&self) -> f64 {
        self.surcharge + self.interest + self.compromise
    }
}

/// What a restore found in the backup file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Restored {
    /// Tax profiles (taxpayers, branches, their returns).
    Profiles,
    /// A whole database.
    Database,
}

fn payees_key(tin_root: &str) -> String {
    format!("payees.{tin_root}")
}

impl BirApp {
    fn with_db<R>(&self, f: impl FnOnce(&bir_core::db::Database) -> Result<R, bir_core::db::DbError>) -> AppResult<R> {
        let db = self.db.lock().map_err(|_| AppError::Call("the database lock is poisoned".into()))?;
        Ok(f(&db)?)
    }

    // ---------------------------------------------------------- settings

    /// A stored setting, `None` when it was never set.
    pub fn get_setting(&self, key: &str) -> AppResult<Option<String>> {
        self.with_db(|db| db.get_setting(key))
    }

    pub fn set_setting(&self, key: &str, value: &str) -> AppResult<()> {
        self.with_db(|db| db.set_setting(key, value))
    }

    // ---------------------------------------------------------- drafts

    /// A stored return's draft JSON by its id, `None` when no return has it.
    pub fn draft_data(&self, draft_id: i64) -> AppResult<Option<serde_json::Value>> {
        self.with_db(|db| db.draft_data_json(draft_id))
    }

    /// Delete a saved return that is still a Draft (a queued or filed one
    /// stays). Refused with a sentence when there is nothing to delete.
    pub fn delete_draft(&self, draft_id: i64) -> AppResult<()> {
        if self.with_db(|db| db.delete_draft(draft_id))? {
            Ok(())
        } else {
            Err(AppError::Call("Only a draft can be deleted; this return has left Draft.".into()))
        }
    }

    // ---------------------------------------------------------- branches

    /// The next free branch code of `tin_root` (`00001` after the head office).
    pub fn suggest_branch_code(&self, tin_root: &str) -> AppResult<String> {
        let answer = self.call("suggest_next_branch_code", vec![tin_root.into()])?;
        Ok(answer.as_str().unwrap_or("00001").to_owned())
    }

    /// Add a branch to `tin_root`; its facts start as the head office's.
    pub fn add_branch(&self, tin_root: &str, code: &str, name: &str) -> AppResult<()> {
        let input = serde_json::json!({ "branch_code": code, "name": name });
        self.call("create_branch", vec![tin_root.into(), input]).map(drop)
    }

    /// Rename the branch `tin` (its 14-digit TIN).
    pub fn rename_branch(&self, tin: &str, name: &str) -> AppResult<()> {
        let input = serde_json::json!({ "name": name });
        self.call("update_branch", vec![tin.into(), input]).map(drop)
    }

    // ---------------------------------------------------------- payees

    /// The payees saved under the taxpayer `tin_root`, by name.
    pub fn saved_payees(&self, tin_root: &str) -> AppResult<Vec<Payee>> {
        let stored = self.get_setting(&payees_key(tin_root))?;
        let mut list: Vec<Payee> = stored.and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
        list.sort_by_key(|p| p.name.to_lowercase());
        Ok(list)
    }

    /// Save `payee` under `tin_root` (a payee with the same TIN is
    /// replaced). The TIN must be 9 or 14 digits and the name not empty.
    pub fn save_payee(&self, tin_root: &str, payee: &Payee) -> AppResult<Payee> {
        let tin: String = payee.tin.chars().filter(char::is_ascii_digit).collect();
        let name = payee.name.trim().to_owned();
        if name.is_empty() {
            return Err(AppError::Call("Enter the payee's registered name.".into()));
        }
        if tin.len() != 9 && tin.len() != 14 {
            return Err(AppError::Call("A TIN is 9 digits, or 14 with the branch code.".into()));
        }
        let tin = if tin.len() == 9 { format!("{tin}00000") } else { tin };
        let saved = Payee { name, tin };
        let mut list = self.saved_payees(tin_root)?;
        list.retain(|p| p.tin != saved.tin);
        list.push(saved.clone());
        let text = serde_json::to_string(&list).map_err(|error| AppError::Call(error.to_string()))?;
        self.set_setting(&payees_key(tin_root), &text)?;
        Ok(saved)
    }

    // ---------------------------------------------------------- backup

    /// Write every tax profile (taxpayers, branches and their returns) to a
    /// backup archive at `path` (D17: profiles only, no PIN).
    pub fn export_backup(&self, path: &Path) -> AppResult<()> {
        self.with_db(|db| bir_core::export::export_all_profiles_data(db, path))
    }

    /// Read a backup archive (profiles, or a whole database) back in.
    pub fn restore_backup(&self, path: &Path) -> AppResult<Restored> {
        let answer = self.call("import_archive", vec![path.display().to_string().into()])?;
        match answer.get("kind").and_then(|k| k.as_str()) {
            Some("profile") => Ok(Restored::Profiles),
            Some("database") => Ok(Restored::Database),
            _ => Err(AppError::Call("That file is not an eBIRForms backup.".into())),
        }
    }

    // ---------------------------------------------------------- penalties

    /// What filing `form_code` due on `due_date` costs if filed `today`,
    /// for `tax_due` still unpaid; `None` when it is not late. With no tax
    /// due the compromise of the no-tax-due tier still applies, so a late
    /// return never reads as free.
    pub fn penalty_estimate(form_code: &str, due_date: chrono::NaiveDate, today: chrono::NaiveDate, tax_due: f64) -> Option<PenaltyEstimate> {
        if today <= due_date {
            return None;
        }
        let schedule = if form_code.starts_with("16") || form_code.starts_with("0619") {
            PenaltyProfile::Withholding
        } else {
            PenaltyProfile::StandardFiling
        };
        let computed = compute_penalties(&PenaltyRequest {
            form_code,
            taxable_period: String::new(),
            schedule,
            taxpayer_class: None,
            due_date,
            filing_date: today,
            payment_date: None,
            basic_tax_due: tax_due,
            gross_sales_or_receipts: Some(0.0),
            is_amended_return: false,
        });
        Some(PenaltyEstimate {
            days_late: computed.inputs.days_late,
            surcharge: computed.surcharge,
            interest: computed.interest,
            compromise: computed.compromise,
        })
    }
}

impl BirApp {
    /// Start an amended return of a filed one: a new editable draft that
    /// says what it amends.
    pub fn amend_return(&self, open: &crate::OpenReturn) -> AppResult<crate::OpenedReturn> {
        let answer = self.call(
            "screen_form_amend",
            vec![
                open.tin.as_str().into(),
                open.form_code.as_str().into(),
                open.year.into(),
                open.period.into(),
                open.key.map_or(serde_json::Value::Null, serde_json::Value::from),
            ],
        )?;
        serde_json::from_value(answer).map_err(|error| AppError::Shape { function: "screen_form_amend".into(), message: error.to_string() })
    }

    /// A Form library sample's print sheet, with what was typed in it.
    pub fn print_sample_html(&self, form_code: &str, draft: &crate::Draft) -> AppResult<crate::PrintedReturn> {
        let answer = self.call(
            "screen_form_print_sample",
            vec![form_code.into(), draft.0.clone(), serde_json::json!({}), serde_json::json!([])],
        )?;
        serde_json::from_value(answer).map_err(|error| AppError::Shape { function: "screen_form_print_sample".into(), message: error.to_string() })
    }
}
