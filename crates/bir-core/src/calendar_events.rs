//! Profile filing deadlines as calendar events: which forms a profile puts
//! on its calendar, and the event set the iCalendar export renders
//! ([`crate::calendar_ics`]). No network; nothing here talks to a calendar
//! service.

use crate::calendar_rules::{DeadlineKind, DeadlinePeriod, DeadlineResolver};
use crate::db::Database;
use crate::forms::{FilingStatus, FormDraftSummary};
use crate::profile::TaxpayerProfile;
use chrono::Duration;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// Settings key prefix of a profile's [`CalendarFormSelection`]. The
/// `google_` name is kept so databases written by earlier builds still read.
pub(crate) const FORM_SELECTION_SETTING_PREFIX: &str = "google_calendar_form_selection:";

/// Which Forms Set entries feed a profile's calendar by default.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CalendarFormPreset {
    /// Every active form in the yearly Forms Sets (current behavior).
    #[default]
    AllForms,
    /// Only forms whose official filing frequency recurs (monthly, quarterly,
    /// annual). Event-driven forms such as one-time registrations are left
    /// off the calendar unless manually included.
    RecurringOnly,
}

impl CalendarFormPreset {
    fn allows(self, form_code: &str) -> bool {
        match self {
            CalendarFormPreset::AllForms => true,
            CalendarFormPreset::RecurringOnly => crate::forms::registry::find_form(form_code)
                .map(|definition| {
                    !matches!(
                        definition.frequency,
                        crate::forms::FilingFrequency::OpenEnded
                    )
                })
                // Custom codes have no official frequency; fail closed under
                // the recurring preset and let a manual include rescue them.
                .unwrap_or(false),
        }
    }
}

/// Per-profile choice of which forms produce calendar events.
///
/// The preset supplies the default; manual lists override it per form code.
/// An exclude always wins over an include.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CalendarFormSelection {
    #[serde(default)]
    pub preset: CalendarFormPreset,
    #[serde(default)]
    pub manual_include: Vec<String>,
    #[serde(default)]
    pub manual_exclude: Vec<String>,
}

impl CalendarFormSelection {
    pub fn allows(&self, form_code: &str) -> bool {
        if self.manual_exclude.iter().any(|code| code == form_code) {
            return false;
        }
        if self.manual_include.iter().any(|code| code == form_code) {
            return true;
        }
        self.preset.allows(form_code)
    }

    pub fn is_overridden(&self, form_code: &str) -> bool {
        self.manual_include.iter().any(|code| code == form_code)
            || self.manual_exclude.iter().any(|code| code == form_code)
    }

    /// Flip one form's effective state, recording a manual override only when
    /// the desired state differs from what the preset already produces.
    pub fn toggle(&mut self, form_code: &str) {
        let turn_on = !self.allows(form_code);
        self.manual_include.retain(|code| code != form_code);
        self.manual_exclude.retain(|code| code != form_code);
        let preset_allows = self.preset.allows(form_code);
        if turn_on && !preset_allows {
            self.manual_include.push(form_code.to_string());
            self.manual_include.sort();
        } else if !turn_on && preset_allows {
            self.manual_exclude.push(form_code.to_string());
            self.manual_exclude.sort();
        }
    }

    /// Switch presets and drop overrides made redundant by the new default.
    pub fn set_preset(&mut self, preset: CalendarFormPreset) {
        self.preset = preset;
        self.manual_include.retain(|code| !preset.allows(code));
        self.manual_exclude.retain(|code| preset.allows(code));
    }

    pub fn clear_overrides(&mut self) {
        self.manual_include.clear();
        self.manual_exclude.clear();
    }
}

pub fn calendar_form_selection(db: &Database, tin: &str) -> CalendarFormSelection {
    db.get_setting(&format!("{FORM_SELECTION_SETTING_PREFIX}{tin}"))
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

pub fn save_calendar_form_selection(
    db: &Database,
    tin: &str,
    selection: &CalendarFormSelection,
) -> Result<(), anyhow::Error> {
    db.set_setting(
        &format!("{FORM_SELECTION_SETTING_PREFIX}{tin}"),
        &serde_json::to_string(selection)?,
    )?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DesiredCalendarEvent {
    pub obligation_key: String,
    pub taxable_year: u16,
    pub form_code: String,
    pub period_label: String,
    pub content_hash: String,
    pub body: Value,
}

pub fn build_desired_events(
    db: &Database,
    profile: &TaxpayerProfile,
) -> Result<(Vec<DesiredCalendarEvent>, usize), anyhow::Error> {
    let mut desired = Vec::new();
    let mut excluded_undated = 0;
    let tin = profile.tin.full();
    let years = db.list_forms_set_years(&tin)?;
    let global_overrides = db.get_deadline_overrides();
    let selection = calendar_form_selection(db, &tin);

    for year in years {
        // The user's calendar form selection decides which Forms Set entries
        // become events; deselected forms also stay out of the undated count.
        let form_codes = db
            .active_form_codes_for_year(&tin, year)?
            .into_iter()
            .filter(|code| selection.allows(code))
            .collect::<Vec<_>>();
        let overrides =
            crate::forms::event_trigger::periodic_overrides(&global_overrides, profile, Some(year));
        let summaries = db.list_draft_summaries(&tin, year).unwrap_or_default();
        let deadlines = DeadlineResolver::deadlines_for_forms(&form_codes, year as i32, &overrides);
        let dated_codes = deadlines
            .iter()
            .map(|deadline| deadline.form_code.clone())
            .collect::<BTreeSet<_>>();
        excluded_undated += form_codes
            .iter()
            .filter(|code| !dated_codes.contains(code.as_str()))
            .count();

        for deadline in deadlines {
            if !crate::integration::deadline_applies_to_profile(profile, &deadline) {
                continue;
            }
            let DeadlineKind::Dated {
                original_deadline,
                final_deadline,
            } = deadline.deadline
            else {
                excluded_undated += 1;
                continue;
            };
            let paid = deadline_is_paid(&deadline.period, &deadline.form_code, &summaries);
            let period_key = period_key(&deadline.period);
            let obligation_key = format!("{}:{}:{}", year, deadline.form_code, period_key);
            let period_label = concise_period_label(&deadline.period);
            desired.push(desired_event(
                profile,
                &deadline,
                (original_deadline, final_deadline),
                paid,
                EventNames {
                    obligation_key,
                    taxable_year: year,
                    period_label,
                    period_line: deadline.period.label(),
                },
            )?);
        }
    }

    // Decision D8c: each saved as-needed return that holds its entered
    // trigger date is a dated event too, keyed by the return, so the user
    // keeps one event per return (`event_trigger::dated_event_returns`, the
    // rows Home and the Calendar list).
    let overrides = crate::forms::event_trigger::as_needed_overrides(&global_overrides, profile);
    let working = crate::official_holidays::business_day_calendar();
    for dated in crate::forms::event_trigger::dated_event_returns(db, &tin, &overrides, &working)? {
        if !selection.allows(&dated.summary.form_code) {
            continue;
        }
        let DeadlineKind::Dated {
            original_deadline,
            final_deadline,
        } = dated.deadline.deadline
        else {
            continue;
        };
        let trigger = format!(
            "{} {}",
            dated.trigger.rule.trigger,
            dated.trigger.date.format("%Y-%m-%d")
        );
        desired.push(desired_event(
            profile,
            &dated.deadline,
            (original_deadline, final_deadline),
            dated.summary.status == FilingStatus::Paid,
            EventNames {
                obligation_key: format!(
                    "{}:{}:return-{}",
                    dated.summary.taxable_year, dated.summary.form_code, dated.summary.id
                ),
                taxable_year: dated.summary.taxable_year,
                period_line: format!("As needed: {trigger}"),
                period_label: trigger,
            },
        )?);
    }
    desired.sort_by(|a, b| a.obligation_key.cmp(&b.obligation_key));
    Ok((desired, excluded_undated))
}

/// How one event is named: its stable key, year and the period's words.
struct EventNames {
    obligation_key: String,
    taxable_year: u16,
    /// Short, for the title ("2026-10", "Removal 2026-10-07").
    period_label: String,
    /// Long, for the description.
    period_line: String,
}

fn desired_event(
    profile: &TaxpayerProfile,
    deadline: &crate::calendar_rules::ResolvedTaxDeadline,
    (original_deadline, final_deadline): (chrono::NaiveDate, chrono::NaiveDate),
    paid: bool,
    names: EventNames,
) -> Result<DesiredCalendarEvent, anyhow::Error> {
    let tin = profile.tin.full();
    let EventNames {
        obligation_key,
        taxable_year,
        period_label,
        period_line,
    } = names;
    let title_prefix = if paid { "[Filed] " } else { "" };
    let masked_tin = mask_tin(&tin);
    let mut description = format!(
        "Tax profile: {} ({})\nForm: {} - {}\n{}\nFinal deadline: {}\nStatus: {}",
        profile.full_name,
        masked_tin,
        deadline.display_form_no,
        deadline.form_name,
        period_line,
        final_deadline,
        deadline.status.label(),
    );
    if original_deadline != final_deadline {
        description.push_str(&format!("\nOriginal deadline: {original_deadline}"));
    }
    if let Some(source) = &deadline.source_reference {
        description.push_str(&format!("\nSource: {source}"));
    }
    if !deadline.description.is_empty() {
        description.push_str(&format!("\n\n{}", deadline.description));
    }

    let reminders = if paid {
        json!({"useDefault": false, "overrides": []})
    } else {
        json!({
            "useDefault": false,
            "overrides": [
                {"method": "email", "minutes": 10080},
                {"method": "email", "minutes": 1440}
            ]
        })
    };
    let body = json!({
        "summary": format!("{title_prefix}[BIR] {} - {}", deadline.display_form_no, period_label),
        "description": description,
        "start": {"date": final_deadline.format("%Y-%m-%d").to_string()},
        "end": {"date": (final_deadline + Duration::days(1)).format("%Y-%m-%d").to_string()},
        "transparency": "transparent",
        "reminders": reminders,
        "extendedProperties": {
            "private": {
                "ebirformsManaged": "true",
                "profileKey": profile_key(&tin),
                "obligationKey": obligation_key
            }
        }
    });
    let content_hash = hex::encode(Sha256::digest(serde_json::to_vec(&body)?));
    Ok(DesiredCalendarEvent {
        obligation_key,
        taxable_year,
        form_code: deadline.form_code.clone(),
        period_label,
        content_hash,
        body,
    })
}

fn deadline_is_paid(
    period: &DeadlinePeriod,
    form_code: &str,
    summaries: &[FormDraftSummary],
) -> bool {
    summaries.iter().any(|summary| {
        summary.form_code == form_code
            && summary.status == FilingStatus::Paid
            && match period {
                DeadlinePeriod::Monthly { month, .. } => summary.month == Some(*month),
                DeadlinePeriod::Quarterly { quarter, .. } => summary.quarter == Some(*quarter),
                DeadlinePeriod::Annual { .. } => true,
                DeadlinePeriod::EventBased => false,
            }
    })
}

fn period_key(period: &DeadlinePeriod) -> String {
    match period {
        DeadlinePeriod::Monthly { month, .. } => format!("m{month:02}"),
        DeadlinePeriod::Quarterly { quarter, .. } => format!("q{quarter}"),
        DeadlinePeriod::Annual { .. } => "annual".to_string(),
        DeadlinePeriod::EventBased => "event".to_string(),
    }
}

fn concise_period_label(period: &DeadlinePeriod) -> String {
    match period {
        DeadlinePeriod::Monthly {
            taxable_year,
            month,
        } => format!("{taxable_year}-{month:02}"),
        DeadlinePeriod::Quarterly {
            taxable_year,
            quarter,
        } => format!("{taxable_year} Q{quarter}"),
        DeadlinePeriod::Annual { taxable_year } => format!("{taxable_year} Annual"),
        DeadlinePeriod::EventBased => "Event-based".to_string(),
    }
}

fn mask_tin(tin: &str) -> String {
    let suffix = tin
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("TIN ending {suffix}")
}

/// A profile's stable, non-identifying key: the SHA-256 of its TIN. Used to
/// scope the event's extendedProperties and the iCalendar UID without writing a
/// raw TIN into data that leaves the app.
pub(crate) fn profile_key(tin: &str) -> String {
    hex::encode(Sha256::digest(tin.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar_rules::DeadlinePeriod;
    use crate::forms::{FilingPeriod, FormSetSource, PerYearFormsSet};
    use crate::naming::Tin;
    use crate::profile::{TaxpayerProfile, TaxpayerType};
    use tempfile::NamedTempFile;

    #[test]
    fn default_selection_allows_every_form() {
        let selection = CalendarFormSelection::default();
        assert!(selection.allows("2551Q"));
        assert!(selection.allows("0605"));
        assert!(selection.allows("CUSTOM_FORM"));
    }

    #[test]
    fn recurring_preset_drops_event_driven_and_unknown_forms() {
        let selection = CalendarFormSelection {
            preset: CalendarFormPreset::RecurringOnly,
            ..Default::default()
        };
        assert!(selection.allows("2551Q"));
        assert!(selection.allows("1601C"));
        // 0605 is an event-driven payment form (OpenEnded frequency).
        assert!(!selection.allows("0605"));
        // Custom codes have no official frequency: fail closed.
        assert!(!selection.allows("CUSTOM_FORM"));
    }

    #[test]
    fn manual_overrides_beat_the_preset_and_exclude_beats_include() {
        let mut selection = CalendarFormSelection {
            preset: CalendarFormPreset::RecurringOnly,
            ..Default::default()
        };
        selection.toggle("0605"); // preset denies -> becomes manual include
        assert!(selection.allows("0605"));
        assert!(selection.is_overridden("0605"));

        selection.toggle("2551Q"); // preset allows -> becomes manual exclude
        assert!(!selection.allows("2551Q"));

        // Toggling back removes the override instead of stacking lists.
        selection.toggle("0605");
        assert!(!selection.allows("0605"));
        assert!(!selection.is_overridden("0605"));

        // Exclude wins if both lists ever contain the same code.
        let conflicted = CalendarFormSelection {
            preset: CalendarFormPreset::AllForms,
            manual_include: vec!["2550Q".to_string()],
            manual_exclude: vec!["2550Q".to_string()],
        };
        assert!(!conflicted.allows("2550Q"));
    }

    #[test]
    fn switching_presets_drops_redundant_overrides() {
        let mut selection = CalendarFormSelection {
            preset: CalendarFormPreset::RecurringOnly,
            manual_include: vec!["0605".to_string()],
            manual_exclude: vec!["2551Q".to_string()],
        };
        // Under AllForms the 0605 include is redundant; the 2551Q exclude
        // still differs from the preset and must survive.
        selection.set_preset(CalendarFormPreset::AllForms);
        assert!(selection.manual_include.is_empty());
        assert_eq!(selection.manual_exclude, vec!["2551Q".to_string()]);
        assert!(selection.allows("0605"));
        assert!(!selection.allows("2551Q"));
    }

    #[test]
    fn selection_round_trips_through_settings() {
        let file = NamedTempFile::new().unwrap();
        let db = Database::open(file.path()).unwrap();
        let tin = "123456789000";

        // Absent -> default.
        assert_eq!(
            calendar_form_selection(&db, tin),
            CalendarFormSelection::default()
        );

        let mut selection = CalendarFormSelection {
            preset: CalendarFormPreset::RecurringOnly,
            ..Default::default()
        };
        selection.toggle("0605");
        save_calendar_form_selection(&db, tin, &selection).unwrap();
        assert_eq!(calendar_form_selection(&db, tin), selection);

        // Corrupt JSON falls back to default instead of erroring.
        db.set_setting(&format!("{FORM_SELECTION_SETTING_PREFIX}{tin}"), "not json")
            .unwrap();
        assert_eq!(
            calendar_form_selection(&db, tin),
            CalendarFormSelection::default()
        );
    }

    fn test_profile() -> TaxpayerProfile {
        let mut profile = TaxpayerProfile {
            id: None,
            full_name: "Calendar Test Corp".into(),
            tin: Tin {
                segment1: "123".into(),
                segment2: "456".into(),
                segment3: "789".into(),
                branch: "000".into(),
            },
            rdo_code: "039".into(),
            line_of_business: "Software".into(),
            registered_address: "Quezon City".into(),
            zip_code: "1100".into(),
            phone: "09170000000".into(),
            email: "calendar@example.com".into(),
            default_form_type: "0619E".into(),
            taxpayer_type: TaxpayerType::Corporation,
            is_vat_registered: false,
            business_start_date: None,
            birth_date: None,
            tax_classification: None,
            eopt_tier: None,
            is_bmbe: false,
            is_gpp_partner: false,
            is_create_msme: false,
            is_expanded_withholding_agent: true,
            atc_codes: Vec::new(),
            excise_tax_categories: Vec::new(),
            tax_elections: Vec::new(),
            is_archived: false,
            profile_pin_hash: None,
            email_tracking_enabled: false,
            test_notification_enabled: false,
            has_employees: false,
            is_dormant: false,
            has_single_employer: false,
            withholds_compensation: false,
            withholds_expanded: true,
            withholds_final: false,
            is_top_withholding_agent: false,
            is_government_withholding_entity: false,
            registration_activity_status: Default::default(),
            profile_versions: Vec::new(),
            compliance_source_mode: Default::default(),
            per_year_forms: Default::default(),
        };
        profile.ensure_profile_version_ledger();
        profile.per_year_forms.insert(
            2026,
            PerYearFormsSet::from_codes(2026, ["0619E"], FormSetSource::Manual),
        );
        profile
    }

    fn test_database() -> (NamedTempFile, Database) {
        let file = NamedTempFile::new().unwrap();
        let db = Database::open(file.path()).unwrap();
        let forms = PerYearFormsSet::from_codes(2026, ["0619E"], FormSetSource::Manual);
        db.save_per_year_forms("123456789000", 2026, &forms)
            .unwrap();
        (file, db)
    }

    #[test]
    fn period_keys_are_stable() {
        assert_eq!(
            period_key(&DeadlinePeriod::Monthly {
                taxable_year: 2026,
                month: 2
            }),
            "m02"
        );
        assert_eq!(
            period_key(&DeadlinePeriod::Quarterly {
                taxable_year: 2026,
                quarter: 3
            }),
            "q3"
        );
        assert_eq!(
            period_key(&DeadlinePeriod::Annual { taxable_year: 2026 }),
            "annual"
        );
    }

    #[test]
    fn mask_tin_only_exposes_suffix() {
        assert_eq!(mask_tin("123456789000"), "TIN ending 9000");
        assert!(!mask_tin("123456789000").contains("12345678"));
    }

    #[test]
    fn build_desired_events_uses_authoritative_forms_set() {
        let (_file, db) = test_database();

        let (events, excluded) = build_desired_events(&db, &test_profile()).unwrap();

        assert_eq!(events.len(), 12);
        assert_eq!(excluded, 0);
        assert!(events.iter().all(|event| event.form_code == "0619E"));
    }

    /// Review F5: a saved as-needed return that holds its entered trigger
    /// date syncs as its own dated event, as the rows on Home and the
    /// Calendar; one that holds only a pre-filled date does not.
    #[test]
    fn a_dated_as_needed_return_syncs_as_its_own_event() {
        let (_file, db) = test_database();
        let gift = db
            .save_form_draft_v2(
                "123456789000",
                "1800",
                2026,
                &FilingPeriod::OpenEnded(1),
                &FilingStatus::Draft,
                &json!({"status": "Draft", "donation_month": 9, "donation_day": 15, "donation_year": 2026}),
            )
            .unwrap();
        db.save_form_draft_v2(
            "123456789000",
            "2200A",
            2026,
            &FilingPeriod::Monthly(9),
            &FilingStatus::Draft,
            &json!({"status": "Draft", "month": 9, "day": 14, "year": 2026}),
        )
        .unwrap();

        let (events, excluded) = build_desired_events(&db, &test_profile()).unwrap();
        assert_eq!(excluded, 0);
        let periodic = events
            .iter()
            .filter(|event| event.form_code == "0619E")
            .count();
        assert_eq!(periodic, 12, "the periodic events are unchanged");
        let dated: Vec<_> = events
            .iter()
            .filter(|event| event.form_code != "0619E")
            .collect();
        assert_eq!(dated.len(), 1, "the untouched 2200A has no event");
        let event = dated[0];
        assert_eq!(event.form_code, "1800");
        assert_eq!(event.obligation_key, format!("2026:1800:return-{gift}"));
        assert_eq!(
            event.body["start"]["date"], "2026-10-15",
            "30 days after the gift"
        );
        assert_eq!(event.body["summary"], "[BIR] 1800 - Donation 2026-09-15");
    }

    /// Decision P5-10: a damaged stored override is skipped, the good ones
    /// still move the synced events.
    #[test]
    fn the_sync_sees_the_good_overrides_beside_a_damaged_one() {
        let (_file, db) = test_database();
        db.set_setting(
            "deadline_overrides",
            r#"[{"id":"broken"},{"id":"a","title":"Good","source_reference":"RMC","affected_form_codes":["0619E"],"original_deadline":"2026-10-12","adjusted_deadline":"2026-10-20","affected_regions":[],"affected_taxpayer_types":[],"effective_from":null,"effective_until":null,"expires_at":null}]"#,
        )
        .unwrap();
        let (events, _) = build_desired_events(&db, &test_profile()).unwrap();
        let september = events
            .iter()
            .find(|event| event.obligation_key == "2026:0619E:m09")
            .unwrap();
        assert_eq!(september.body["start"]["date"], "2026-10-20");
    }

    #[test]
    fn paid_filing_disables_reminders_and_marks_event_filed() {
        let (_file, db) = test_database();
        db.save_form_draft_v2(
            "123456789000",
            "0619E",
            2026,
            &FilingPeriod::Monthly(1),
            &FilingStatus::Paid,
            &json!({"status": "paid"}),
        )
        .unwrap();

        let (events, _) = build_desired_events(&db, &test_profile()).unwrap();
        let january = events
            .iter()
            .find(|event| event.obligation_key.ends_with(":m01"))
            .unwrap();

        assert!(
            january.body["summary"]
                .as_str()
                .is_some_and(|summary| summary.starts_with("[Filed]"))
        );
        assert_eq!(
            january.body["reminders"]["overrides"],
            Value::Array(Vec::new())
        );
    }
}
