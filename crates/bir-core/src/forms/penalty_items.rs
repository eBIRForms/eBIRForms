//! The shared Surcharge / Interest / Compromise model for every form whose
//! return carries those three items.
//!
//! A form declares its items once, in a [`PenaltyItems`] value: which draft
//! fields hold the surcharge, interest and compromise, the amount the engine
//! is run on (the form's own tax still due before penalties), the compromise
//! schedule ([`PenaltyProfile`]), and how its period maps to the app's
//! deadline calendar. Everything else is shared, so every wired form behaves
//! identically:
//!
//! - **Computed default.** While the return is a Draft, [`PenaltyItems::apply`]
//!   (called from the form's own compute chain, just before it sums the
//!   penalties) asks the engine what the return owes if filed today and keeps
//!   that, with its inputs, in the draft's [`PenaltyState`].
//! - **Per-component override.** Each item files its override when one is
//!   set, else the computed value, so the XML, print and queue fingerprint
//!   carry what is filed. An optional note records the reason.
//! - **Clock.** A Queued return refreshes only at queue revalidation, where a
//!   change fails closed; Submitted, Confirmed and Paid returns never move.
//! - **No deadline, no computation.** A return whose period has no deadline in
//!   the app's calendar (event-based returns) computes nothing: its items are
//!   overrides only. A deadline is never invented.
//! - **Back-compat.** A draft saved while the items were manual-only (no
//!   `penalty_model_version`) keeps every non-zero hand-typed amount as an
//!   override, so nothing it carried changes value.
//! - **Typed trigger date (maintainer decision D8, 2026-10-18).** A form
//!   whose penalties run from a date the filer types, not from the calendar
//!   (the excise returns 2200A, 2200AN, 2200M, 2200P, 2200S and 2200T: the
//!   removal date; 2200C: the 10th of the month after its Item 1 month;
//!   maintainer decision (1800/1801 penalties run from the due date), 2026-10-19:
//!   1800, 30 days after the donation, and 1801,
//!   one year after the death, each moved to the next business day),
//!   declares [`PenaltyItems::typed_due_date`]. It is measured from that
//!   date once the draft is on [`TYPED_DUE_DATE_MODEL_VERSION`] (a new
//!   return, or one that joins); a draft stored before computes nothing
//!   until it joins, so it keeps what it holds.

use std::cell::Cell;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use super::FilingStatus;
use super::queueable::QueueableForm;
use crate::calendar_rules::{DeadlineOverride, DeadlinePeriod, DeadlineResolver};
use crate::penalties::{
    ComputedPenalties, PenaltyComponent, PenaltyOverrides, PenaltyProfile, PenaltyRequest,
    TaxpayerClass, compute_penalties,
};
use crate::profile::{EoptTier, TaxpayerProfile};

/// Current penalty model: computed default with a per-component override.
pub const PENALTY_MODEL_VERSION: u8 = 1;

/// The model of a form with a [`PenaltyItems::typed_due_date`] (decision
/// D8): computed from the date the filer typed. A draft of such a form
/// stored at [`PENALTY_MODEL_VERSION`] predates it and computes nothing.
pub const TYPED_DUE_DATE_MODEL_VERSION: u8 = 2;

/// The penalty engine's class for a profile's EOPT classification.
pub fn taxpayer_class_of(tier: &EoptTier) -> TaxpayerClass {
    match tier {
        EoptTier::Micro => TaxpayerClass::Micro,
        EoptTier::Small => TaxpayerClass::Small,
        EoptTier::Medium => TaxpayerClass::Medium,
        EoptTier::Large => TaxpayerClass::Large,
    }
}

/// What a wired draft stores about its penalties, next to the three items.
/// Embedded with `#[serde(flatten)]` (in the party's column on a two-column
/// return), so the stored JSON keys are the same on every form; 1701Q, whose
/// two columns share one flat draft, keeps one state per column under
/// `penalty` and `spouse_penalty`, left out while they are the default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PenaltyState {
    /// `0` for a draft saved while the items were manual-only; its non-zero
    /// hand-typed values become overrides the first time it is computed.
    #[serde(default)]
    pub penalty_model_version: u8,
    /// The app's resolved deadline for this period (business-day and deadline
    /// overrides applied). `None` falls back to the standard calendar rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub penalty_due_date: Option<NaiveDate>,
    /// The profile's EOPT classification; `None` assumes the standard class.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub penalty_taxpayer_class: Option<TaxpayerClass>,
    /// What the engine last computed, with its inputs. Frozen once the return
    /// leaves Draft, refreshed only by queue revalidation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub computed_penalties: Option<ComputedPenalties>,
    /// Per-component overrides and the reason recorded with them.
    #[serde(default)]
    pub penalty_overrides: PenaltyOverrides,
}

impl PenaltyState {
    /// The state of a new draft: current model, the profile's class.
    pub fn new(taxpayer_class: Option<TaxpayerClass>) -> Self {
        Self {
            penalty_model_version: PENALTY_MODEL_VERSION,
            penalty_taxpayer_class: taxpayer_class,
            ..Self::default()
        }
    }

    /// Clear every component's override (the note stays): what the form's
    /// own handlers do when they clear and disable the penalty items.
    pub fn clear_overrides(&mut self) {
        for component in PenaltyComponent::ALL {
            self.penalty_overrides.set(component, None);
        }
    }

    /// [`Self::new`] with the class read from `profile`.
    pub fn for_profile(profile: &TaxpayerProfile) -> Self {
        Self::new(profile.eopt_tier.as_ref().map(taxpayer_class_of))
    }

    /// [`Self::for_profile`] for a new return of a form whose penalties run
    /// from a date the filer types (decision D8): on
    /// [`TYPED_DUE_DATE_MODEL_VERSION`] from the start.
    pub fn for_profile_typed_due_date(profile: &TaxpayerProfile) -> Self {
        Self {
            penalty_model_version: TYPED_DUE_DATE_MODEL_VERSION,
            ..Self::for_profile(profile)
        }
    }

    /// The state of a return imported from official XML: current model, every
    /// item kept as an override by [`PenaltyItems::adopt_imported`].
    pub fn imported() -> Self {
        Self::new(None)
    }
}

/// A form field that holds one penalty item's amount.
pub trait PenaltyField {
    fn penalty_value(&self) -> f64;
    fn set_penalty_value(&mut self, value: f64);
}

impl PenaltyField for f64 {
    fn penalty_value(&self) -> f64 {
        *self
    }
    fn set_penalty_value(&mut self, value: f64) {
        *self = value;
    }
}

/// How the engine's values are held in the form's items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PenaltyRounding {
    /// Pesos and centavos.
    Cents,
    /// Whole pesos (the form's items take no centavos).
    WholePesos,
}

impl PenaltyRounding {
    fn round(self, value: f64) -> f64 {
        match self {
            Self::Cents => (value * 100.0).round() / 100.0,
            Self::WholePesos => value.round(),
        }
    }
}

/// A form's declaration of its Surcharge, Interest and Compromise items.
pub struct PenaltyItems<F> {
    /// The form code the app's deadline calendar uses (`"1601C"`).
    pub form_code: &'static str,
    /// Whose items these are on a return that carries a set per party
    /// (`"Spouse"`); `None` on a single-party return.
    pub party: Option<&'static str>,
    /// Whether this party's items are part of the return as it stands (the
    /// spouse's only on a joint return). Items that do not apply are left to
    /// the form's own rules and never computed.
    pub applies: fn(&F) -> bool,
    /// The compromise schedule (`compromise.rs`): withholding agents' returns
    /// use the withholding schedule, every other return the standard one.
    pub schedule: PenaltyProfile,
    /// Draft field names (stored JSON keys, agent `form.fill` keys) of the
    /// surcharge, interest and compromise items, in that order.
    pub fields: [&'static str; 3],
    /// Official control ids of the three items, in the same order.
    pub controls: [&'static str; 3],
    /// Item labels as the form prints them (`"32 Surcharge"`).
    pub labels: [&'static str; 3],
    /// The item the engine's basic tax due is read from (`"Item 31"`).
    pub basic_tax_due_label: &'static str,
    pub rounding: PenaltyRounding,
    pub get: fn(&F, PenaltyComponent) -> f64,
    pub set: fn(&mut F, PenaltyComponent, f64),
    /// The form's tax still due before penalties (the amount its total adds
    /// the penalties to).
    pub basic_tax_due: fn(&F) -> f64,
    /// Gross sales/receipts for the engine's no-tax-due compromise tier;
    /// `None` for a return that carries no such basis.
    pub gross_sales: Option<fn(&F) -> f64>,
    pub state: fn(&F) -> &PenaltyState,
    pub state_mut: fn(&mut F) -> &mut PenaltyState,
    pub status: fn(&F) -> FilingStatus,
    /// The draft was created under the typed-trigger model and its trigger
    /// date is not entered yet (`SubmissionLifecycle::trigger_date_model`
    /// and `trigger_date_entered`; see [`Self::awaits_trigger_date`]).
    pub trigger_date_pending: fn(&F) -> bool,
    /// The calendar period this return is filed for; `None` when the app's
    /// calendar has no period for it (event-based returns).
    pub deadline_period: fn(&F) -> Option<DeadlinePeriod>,
    /// A due date the return carries itself, read from what the filer typed
    /// (decision D8: the excise returns' removal date). When set it is the
    /// only due date, used once the draft is on
    /// [`TYPED_DUE_DATE_MODEL_VERSION`]; `None` for every other form.
    pub typed_due_date: Option<fn(&F) -> Option<NaiveDate>>,
    pub period_code: fn(&F) -> String,
    pub is_amended: fn(&F) -> bool,
    /// The form's full compute chain (which calls [`Self::apply`]).
    pub recompute: fn(&mut F),
}

impl<F> Clone for PenaltyItems<F> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<F> Copy for PenaltyItems<F> {}

/// The calendar period of a queueable form's filing period. Open-ended
/// (event) returns have none.
pub fn calendar_period<F: QueueableForm>(form: &F) -> Option<DeadlinePeriod> {
    let taxable_year = i32::from(form.taxable_year());
    match form.filing_period() {
        super::FilingPeriod::Monthly(month) => Some(DeadlinePeriod::Monthly {
            taxable_year,
            month,
        }),
        super::FilingPeriod::Quarterly(quarter) => Some(DeadlinePeriod::Quarterly {
            taxable_year,
            quarter,
        }),
        super::FilingPeriod::Annual => Some(DeadlinePeriod::Annual { taxable_year }),
        super::FilingPeriod::OpenEnded(_) => None,
    }
}

/// The clock the compute chain reads: today, and whether a non-Draft return
/// refreshes its computed values.
#[derive(Clone, Copy)]
struct Clock {
    today: NaiveDate,
    /// `None`: refresh only while the return is a Draft.
    refresh: Option<bool>,
}

thread_local! {
    static CLOCK: Cell<Option<Clock>> = const { Cell::new(None) };
}

/// Puts the previous clock back when dropped, so a pinned clock ends with
/// its scope even if the work panics.
struct ClockRestore(Option<Clock>);

impl Drop for ClockRestore {
    fn drop(&mut self) {
        let previous = self.0;
        CLOCK.with(|cell| cell.set(previous));
    }
}

fn with_clock_option<R>(clock: Option<Clock>, run: impl FnOnce() -> R) -> R {
    let _restore = ClockRestore(CLOCK.with(|cell| cell.replace(clock)));
    run()
}

fn with_clock_value<R>(clock: Clock, run: impl FnOnce() -> R) -> R {
    with_clock_option(Some(clock), run)
}

/// This thread's penalty clock, to carry into a worker thread
/// ([`CapturedClock::run`]): the clock is per thread.
#[derive(Clone, Copy)]
pub struct CapturedClock(Option<Clock>);

/// Capture this thread's penalty clock (none when not pinned).
pub fn capture_clock() -> CapturedClock {
    CapturedClock(CLOCK.with(Cell::get))
}

impl CapturedClock {
    /// Run `run` on the captured clock (on any thread).
    pub fn run<R>(self, run: impl FnOnce() -> R) -> R {
        with_clock_option(self.0, run)
    }
}

/// Today as the penalty model sees it (pinned by [`with_clock`] in tests).
pub fn today() -> NaiveDate {
    CLOCK
        .with(Cell::get)
        .map(|clock| clock.today)
        .unwrap_or_else(|| chrono::Local::now().date_naive())
}

/// Run `run` as if today were `today` (a Draft still refreshes, a Queued
/// return still waits for revalidation).
pub fn with_clock<R>(today: NaiveDate, run: impl FnOnce() -> R) -> R {
    with_clock_value(
        Clock {
            today,
            refresh: None,
        },
        run,
    )
}

/// The calendar a typed due date moves on (maintainer decisions: a typed due
/// date moves to the next working day, past weekends and official
/// holidays): the weekends plus the official holidays the app ships, the
/// `original_deadline` of each entry of
/// `official_holidays::OFFICIAL_HOLIDAY_YEARS` (built once, through
/// `official_holidays::business_day_calendar`). 1800, 1801
/// and 2200C read it through [`next_business_day`].
///
/// - Official holidays are known only for the years whose proclamation
///   ships (2026 today); other years roll off weekends only until theirs is
///   added in an app update (maintainer decision: ship the 2026 list now,
///   add each year's proclamation later). The 2026 seed lacks Eid'l Fitr
///   and Eid'l Adha, which are added when their proclamations are sourced.
/// - It is not the stored deadline overrides (`Database::
///   get_deadline_overrides`): those move only the calendar deadlines of the
///   forms in their `affected_form_codes`, and a typed due date has no
///   calendar deadline to move.
/// - This calendar uses the holiday dates themselves, not the seeded
///   overrides' `adjusted_deadline`s, so a typed due date on Maundy
///   Thursday (2026-04-02) or Good Friday (2026-04-03) moves to Monday,
///   2026-04-06.
pub fn deadline_calendar() -> &'static crate::calendar_rules::BusinessDayCalendar {
    use crate::calendar_rules::BusinessDayCalendar;
    static CALENDAR: std::sync::OnceLock<BusinessDayCalendar> = std::sync::OnceLock::new();
    CALENDAR.get_or_init(crate::official_holidays::business_day_calendar)
}

/// `date`, or the next working day after it when it is not one, on
/// [`deadline_calendar`] (weekends and the shipped official holidays): 1800
/// and 1801 (maintainer decision (1800/1801 penalties run from the due date))
/// and 2200C (maintainer decision: excise penalties run from the typed date)
/// move their typed due dates with
/// it.
pub fn next_business_day(date: NaiveDate) -> NaiveDate {
    deadline_calendar().adjust_to_next_business_day(date).0
}

/// The deadline `form_code`'s calendar resolves for `period`, with `overrides`.
pub fn resolve_due_date(
    form_code: &str,
    period: &DeadlinePeriod,
    overrides: &[DeadlineOverride],
) -> Option<NaiveDate> {
    let taxable_year = period.taxable_year()?;
    DeadlineResolver::resolve_taxable_year_with_overrides(taxable_year, overrides)
        .into_iter()
        .find(|deadline| deadline.form_code == form_code && &deadline.period == period)
        .and_then(|deadline| deadline.final_deadline_date())
        .or_else(|| month_rule_due_date(form_code, period, overrides))
}

/// 1606 only (decision P4-3 moved it off the monthly rule): its penalties
/// name the sale's month, and run from the same date its former monthly
/// row gave (the 10th of the next month, moved off weekends, then by
/// `overrides`), so the filed penalties do not change. No other form takes
/// this fallback.
fn month_rule_due_date(
    form_code: &str,
    period: &DeadlinePeriod,
    overrides: &[DeadlineOverride],
) -> Option<NaiveDate> {
    if form_code != "1606" {
        return None;
    }
    let DeadlinePeriod::Monthly {
        taxable_year,
        month,
    } = period
    else {
        return None;
    };
    let rule = crate::calendar_rules::event_due_rule(form_code)?;
    let first_of_month = NaiveDate::from_ymd_opt(*taxable_year, u32::from(*month), 1)?;
    DeadlineResolver::event_deadline(
        &rule,
        first_of_month,
        overrides,
        &crate::calendar_rules::BusinessDayCalendar::default(),
    )?
    .final_deadline_date()
}

impl<F> PenaltyItems<F> {
    fn state_of<'a>(&self, form: &'a F) -> &'a PenaltyState {
        (self.state)(form)
    }

    /// The deadline penalties are measured from: the one the app resolved
    /// when the form was opened, else the standard calendar rule. `None` when
    /// the calendar has no deadline for this return. A form with a
    /// [`Self::typed_due_date`] uses that alone, and only on a draft on its
    /// model ([`Self::on_typed_due_date`]).
    pub fn due_date(&self, form: &F) -> Option<NaiveDate> {
        if self.awaits_trigger_date(form) {
            return None;
        }
        if let Some(typed) = self.typed_due_date {
            return self.on_typed_due_date(form).then(|| typed(form)).flatten();
        }
        self.state_of(form).penalty_due_date.or_else(|| {
            (self.deadline_period)(form)
                .and_then(|period| resolve_due_date(self.form_code, &period, &[]))
        })
    }

    /// A Draft of a form whose constructor pre-fills its trigger date
    /// ([`crate::forms::event_trigger::PREFILLED_TRIGGER_FORMS`]) that does
    /// not hold an entered one yet: its date is the constructor's guess, so
    /// no penalty is computed from it and no penalty due date is kept
    /// (decisions P4-3, P5-7). Only a draft created under the typed-trigger
    /// model is gated: a row without the marker (every row stored before)
    /// computes from the date it holds, as before. Once dated, or once it
    /// leaves Draft, as before.
    pub fn awaits_trigger_date(&self, form: &F) -> bool {
        crate::forms::event_trigger::PREFILLED_TRIGGER_FORMS.contains(&self.form_code)
            && matches!((self.status)(form), FilingStatus::Draft)
            && (self.trigger_date_pending)(form)
    }

    /// Whether a form with a [`Self::typed_due_date`] has this draft on that
    /// model: a new return, or one that joined ([`Self::join_typed_due_date`]).
    pub fn on_typed_due_date(&self, form: &F) -> bool {
        self.typed_due_date.is_some()
            && self.state_of(form).penalty_model_version >= TYPED_DUE_DATE_MODEL_VERSION
    }

    /// Put a Draft of a form with a [`Self::typed_due_date`] on that model,
    /// so its penalties are computed from the typed date from now on. A
    /// return that has left Draft, or a form without one, is left alone.
    pub fn join_typed_due_date(&self, form: &mut F) {
        if self.typed_due_date.is_none() || !matches!((self.status)(form), FilingStatus::Draft) {
            return;
        }
        let state = (self.state_mut)(form);
        state.penalty_model_version = state
            .penalty_model_version
            .max(TYPED_DUE_DATE_MODEL_VERSION);
    }

    /// What the engine computes if the return is filed (and paid) on
    /// `filing_date`. `None` when no deadline is known.
    pub fn if_filed_on(&self, form: &F, filing_date: NaiveDate) -> Option<ComputedPenalties> {
        let due_date = self.due_date(form)?;
        let mut computed = compute_penalties(&PenaltyRequest {
            form_code: self.form_code,
            taxable_period: (self.period_code)(form),
            schedule: self.schedule,
            taxpayer_class: self.state_of(form).penalty_taxpayer_class,
            due_date,
            filing_date,
            payment_date: None,
            basic_tax_due: (self.basic_tax_due)(form),
            gross_sales_or_receipts: self.gross_sales.map(|gross| gross(form)),
            is_amended_return: (self.is_amended)(form),
        });
        computed.surcharge = self.rounding.round(computed.surcharge);
        computed.interest = self.rounding.round(computed.interest);
        computed.compromise = self.rounding.round(computed.compromise);
        Some(computed)
    }

    /// The three items as they stand on the form.
    pub fn filed(&self, form: &F) -> [f64; 3] {
        PenaltyComponent::ALL.map(|component| (self.get)(form, component))
    }

    /// The value each item files: its override, else the computed value.
    /// An override is passed on as entered; the form's compute chain holds
    /// it the way it holds every typed amount.
    pub fn used(&self, form: &F) -> [f64; 3] {
        let state = self.state_of(form);
        let computed = state.computed_penalties.as_ref();
        PenaltyComponent::ALL.map(|component| state.penalty_overrides.used(component, computed))
    }

    /// Set (`Some`) or clear (`None`) one component's override.
    pub fn set_override(&self, form: &mut F, component: PenaltyComponent, value: Option<f64>) {
        (self.state_mut)(form)
            .penalty_overrides
            .set(component, value);
    }

    /// The component whose draft field is `field`, if any.
    pub fn component_of_field(&self, field: &str) -> Option<PenaltyComponent> {
        PenaltyComponent::ALL
            .into_iter()
            .zip(self.fields)
            .find_map(|(component, name)| (name == field).then_some(component))
    }

    /// Record the profile-derived inputs. A return that has left Draft keeps
    /// the inputs it was reviewed with.
    pub fn set_context(
        &self,
        form: &mut F,
        due_date: Option<NaiveDate>,
        taxpayer_class: Option<TaxpayerClass>,
    ) {
        if !matches!((self.status)(form), FilingStatus::Draft) {
            return;
        }
        let state = (self.state_mut)(form);
        state.penalty_due_date = due_date;
        state.penalty_taxpayer_class = taxpayer_class;
    }

    /// Resolve the inputs from the profile: the period's deadline as the
    /// Calendar resolves it (global and profile deadline overrides applied)
    /// and the profile's EOPT class.
    pub fn apply_profile(
        &self,
        form: &mut F,
        profile: &TaxpayerProfile,
        global_overrides: &[DeadlineOverride],
    ) {
        let awaits = self.awaits_trigger_date(form);
        let due_date = (self.deadline_period)(form)
            .filter(|_| !awaits)
            .and_then(|period| {
                // The override set the Calendar row for this return uses
                // (decision P5-9): one definition for both.
                let year = period.taxable_year().and_then(|y| u16::try_from(y).ok());
                let overrides = crate::forms::event_trigger::calendar_overrides(
                    global_overrides,
                    profile,
                    self.form_code,
                    year,
                );
                resolve_due_date(self.form_code, &period, &overrides)
            });
        self.set_context(
            form,
            due_date,
            profile.eopt_tier.as_ref().map(taxpayer_class_of),
        );
    }

    /// An imported return carries the penalties it was prepared with: keep
    /// every item as an override so the source amounts round-trip.
    pub fn adopt_imported(&self, form: &mut F) {
        let filed = self.filed(form);
        let state = (self.state_mut)(form);
        for (component, value) in PenaltyComponent::ALL.into_iter().zip(filed) {
            state.penalty_overrides.set(component, Some(value));
        }
        state.penalty_model_version = PENALTY_MODEL_VERSION;
    }

    /// A draft saved while the items were manual-only: keep every non-zero
    /// hand-typed amount as an override so it files the same value.
    fn adopt_legacy_manual_penalties(&self, form: &mut F) {
        if self.state_of(form).penalty_model_version >= PENALTY_MODEL_VERSION {
            return;
        }
        let filed = self.filed(form);
        let state = (self.state_mut)(form);
        for (component, value) in PenaltyComponent::ALL.into_iter().zip(filed) {
            if value != 0.0 && state.penalty_overrides.get(component).is_none() {
                state.penalty_overrides.set(component, Some(value));
            }
        }
        state.penalty_model_version = PENALTY_MODEL_VERSION;
    }

    /// The compute-chain hook: call it where the form has its tax still due
    /// and is about to sum its penalties. Refreshes the computed values while
    /// the return is a Draft (or when the clock asks for it) and writes the
    /// used value into each item. Submitted, Confirmed and Paid returns keep
    /// the items they were filed with.
    pub fn apply(&self, form: &mut F) {
        // Items that are not part of the return (a spouse column on a
        // separate return) are left entirely to the form's own rules.
        if !(self.applies)(form) {
            return;
        }
        self.adopt_legacy_manual_penalties(form);
        let status = (self.status)(form);
        if !matches!(status, FilingStatus::Draft | FilingStatus::Queued) {
            return;
        }
        let clock = CLOCK.with(Cell::get);
        let refresh = clock
            .and_then(|clock| clock.refresh)
            .unwrap_or(matches!(status, FilingStatus::Draft));
        if refresh {
            if self.awaits_trigger_date(form) {
                // Undated and on the typed-trigger model: compute nothing
                // from the pre-filled date. A computed value is never turned
                // into an override; the filer's own overrides stay as they
                // are.
                let state = (self.state_mut)(form);
                state.computed_penalties = None;
                state.penalty_due_date = None;
            } else {
                let computed = self.if_filed_on(form, today());
                (self.state_mut)(form).computed_penalties = computed;
            }
        }
        for (component, value) in PenaltyComponent::ALL.into_iter().zip(self.used(form)) {
            (self.set)(form, component, value);
        }
    }

    /// Run the form's compute chain as of `today`; `refresh` recomputes the
    /// engine's values even for a Queued return.
    pub fn compute_on(&self, form: &mut F, today: NaiveDate, refresh: bool) {
        with_clock_value(
            Clock {
                today,
                refresh: Some(refresh),
            },
            || (self.recompute)(form),
        );
    }

    /// The total penalties this unfiled return would carry if filed on
    /// `filing_date` (overrides honoured). `None` for a filed return or when
    /// nothing can be computed.
    pub fn payable_if_filed_on(&self, form: &F, filing_date: NaiveDate) -> Option<f64>
    where
        F: Clone,
    {
        if !matches!(
            (self.status)(form),
            FilingStatus::Draft | FilingStatus::Queued
        ) || !(self.applies)(form)
        {
            return None;
        }
        let mut estimate = form.clone();
        self.compute_on(&mut estimate, filing_date, true);
        let state = self.state_of(&estimate);
        let computed = state.computed_penalties.as_ref()?;
        Some(state.penalty_overrides.used_total(Some(computed)))
    }

    /// The error queue revalidation reports when the filed items moved.
    pub fn changed_error(&self, form: &F, today: NaiveDate) -> (String, String) {
        let [surcharge, interest, compromise] = self.filed(form);
        let party = self
            .party
            .map(|party| format!(" ({party})"))
            .unwrap_or_default();
        (
            "computed_penalties".to_string(),
            format!(
                "Computed penalties{party} changed since the return was queued (filing on {today}: surcharge {surcharge:.2}, interest {interest:.2}, compromise {compromise:.2}); review the return and queue it again"
            ),
        )
    }
}

/// Build the [`PenaltyItems`] of a [`QueueableForm`] whose items are
/// [`PenaltyField`]s. The status, period code, amended flag, compute chain and
/// calendar period come from the trait; trailing `name: value` pairs replace
/// any default (e.g. `deadline_period`, `gross_sales`, `rounding`).
macro_rules! queueable_penalty_items {
    (
        $ty:ty,
        schedule: $schedule:ident,
        items: [
            $($s:ident).+ : $sl:expr,
            $($i:ident).+ : $il:expr,
            $($c:ident).+ : $cl:expr $(,)?
        ],
        controls: [$sc:expr, $ic:expr, $cc:expr $(,)?],
        state: $($state:ident).+,
        basic_tax_due: $basic_label:expr, |$d:ident| $basic:expr
        $(, $field:ident: $value:expr)* $(,)?
    ) => {{
        use $crate::forms::penalty_items::PenaltyField as _;
        use $crate::forms::queueable::QueueableForm as _;
        use $crate::penalties::PenaltyComponent as C;
        #[allow(clippy::needless_update)]
        let items = $crate::forms::penalty_items::PenaltyItems::<$ty> {
            $($field: $value,)*
            ..$crate::forms::penalty_items::PenaltyItems::<$ty> {
                form_code: <$ty as $crate::forms::queueable::QueueableForm>::FORM_CODE,
                party: None,
                applies: |_: &$ty| true,
                schedule: $crate::penalties::PenaltyProfile::$schedule,
                fields: [
                    stringify!($($s).+),
                    stringify!($($i).+),
                    stringify!($($c).+),
                ],
                controls: [$sc, $ic, $cc],
                labels: [$sl, $il, $cl],
                basic_tax_due_label: $basic_label,
                rounding: $crate::forms::penalty_items::PenaltyRounding::Cents,
                get: |d: &$ty, component| match component {
                    C::Surcharge => d.$($s).+.penalty_value(),
                    C::Interest => d.$($i).+.penalty_value(),
                    C::Compromise => d.$($c).+.penalty_value(),
                },
                set: |d: &mut $ty, component, value| match component {
                    C::Surcharge => d.$($s).+.set_penalty_value(value),
                    C::Interest => d.$($i).+.set_penalty_value(value),
                    C::Compromise => d.$($c).+.set_penalty_value(value),
                },
                basic_tax_due: |$d: &$ty| $basic,
                gross_sales: None,
                state: |d: &$ty| &d.$($state).+,
                state_mut: |d: &mut $ty| &mut d.$($state).+,
                status: |d: &$ty| d.lifecycle().status.clone(),
                trigger_date_pending: |d: &$ty| {
                    d.lifecycle().trigger_date_model && !d.lifecycle().trigger_date_entered
                },
                deadline_period: $crate::forms::penalty_items::calendar_period::<$ty>,
                typed_due_date: None,
                period_code: |d: &$ty| d.period_code(),
                is_amended: |d: &$ty| d.amended_return_answer().unwrap_or(false),
                recompute: |d: &mut $ty| d.compute(),
            }
        };
        items
    }};
}
pub(crate) use queueable_penalty_items;

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// Round 5, item 11: a pinned clock ends with its scope even when the
    /// work panics, and a captured clock carries into another thread.
    #[test]
    fn the_pinned_clock_is_restored_and_can_cross_threads() {
        let real = today();
        let pinned = day(2001, 2, 3);
        let caught = std::panic::catch_unwind(|| {
            with_clock(pinned, || {
                assert_eq!(today(), pinned);
                panic!("the work fails");
            })
        });
        assert!(caught.is_err());
        assert_ne!(today(), pinned, "restored after the panic");
        assert_eq!(today(), real);
        let seen = with_clock(pinned, || {
            let clock = capture_clock();
            std::thread::spawn(move || clock.run(today)).join().unwrap()
        });
        assert_eq!(seen, pinned, "the worker thread saw the pinned clock");
    }

    fn profile() -> TaxpayerProfile {
        serde_json::from_value(serde_json::json!({
            "id": null,
            "full_name": "Example Trading Corp",
            "tin": {"segment1": "123", "segment2": "456", "segment3": "788", "branch": "00000"},
            "rdo_code": "040",
            "line_of_business": "Trading",
            "registered_address": "1 Example Street, Pasig",
            "zip_code": "1600",
            "phone": "0280000000",
            "email": "office@example.com",
            "default_form_type": "2551Qv2018",
            "taxpayer_type": "Corporation"
        }))
        .unwrap()
    }

    fn sale_on_august_3(marked: bool) -> crate::forms::form_1606::Form1606Draft {
        let mut draft =
            crate::forms::form_1606::Form1606Draft::new_from_profile(&profile(), 2026, 1);
        (
            draft.transaction_month,
            draft.transaction_day,
            draft.transaction_year,
        ) = (8, 3, 2026);
        draft.lifecycle.trigger_date_model = marked;
        draft
    }

    /// Decisions P4-3 and P5-7: a 1606 Draft created under the typed-trigger
    /// model computes no penalties and keeps no penalty due date while its
    /// sale date is not entered; once entered, or once it leaves Draft, it
    /// computes from the sale.
    #[test]
    fn a_marked_undated_1606_draft_computes_no_penalties() {
        use crate::forms::queueable::QueueableForm as _;
        let items = crate::forms::form_1606::Form1606Draft::penalty_declaration();
        let mut draft = sale_on_august_3(true);
        items.apply_profile(&mut draft, &profile(), &[]);
        with_clock(day(2026, 10, 19), || draft.compute());
        assert!(items.awaits_trigger_date(&draft));
        assert_eq!(draft.penalty.penalty_due_date, None);
        assert_eq!(draft.penalty.computed_penalties, None);
        assert_eq!(items.due_date(&draft), None);
        assert_eq!(items.filed(&draft), [0.0; 3]);
        // Entered: due September 10, so October 19 is late.
        draft.lifecycle.trigger_date_entered = true;
        items.apply_profile(&mut draft, &profile(), &[]);
        with_clock(day(2026, 10, 19), || draft.compute());
        assert_eq!(draft.penalty.penalty_due_date, Some(day(2026, 9, 10)));
        let computed = draft.penalty.computed_penalties.clone().unwrap();
        assert_eq!(computed.inputs.due_date, day(2026, 9, 10));
        assert!(computed.inputs.days_late > 0, "{computed:?}");
        // Left Draft: computed as before.
        draft.lifecycle.trigger_date_entered = false;
        draft.lifecycle.status = FilingStatus::Queued;
        assert!(!items.awaits_trigger_date(&draft));
        assert_eq!(items.due_date(&draft), Some(day(2026, 9, 10)));
    }

    /// Decision P5-7: a row without the marker (every row stored before)
    /// is never gated: it computes from the date it holds, as before.
    #[test]
    fn an_unmarked_1606_draft_computes_from_the_date_it_holds() {
        use crate::forms::queueable::QueueableForm as _;
        let items = crate::forms::form_1606::Form1606Draft::penalty_declaration();
        let mut draft = sale_on_august_3(false);
        items.apply_profile(&mut draft, &profile(), &[]);
        with_clock(day(2026, 10, 19), || draft.compute());
        assert!(!items.awaits_trigger_date(&draft));
        assert_eq!(draft.penalty.penalty_due_date, Some(day(2026, 9, 10)));
        assert!(draft.penalty.computed_penalties.is_some());
    }

    /// Round 6, F2: no path turns computed values into overrides. A marked
    /// undated draft that somehow holds computed values drops them; the
    /// filer's own override stays as it was.
    #[test]
    fn computed_values_never_become_overrides() {
        use crate::forms::queueable::QueueableForm as _;
        let items = crate::forms::form_1606::Form1606Draft::penalty_declaration();
        // Computed while dated, then undated again.
        let mut draft = sale_on_august_3(true);
        draft.lifecycle.trigger_date_entered = true;
        items.apply_profile(&mut draft, &profile(), &[]);
        let mut held = with_clock(day(2026, 10, 19), || {
            draft.compute();
            draft.penalty.computed_penalties.clone().unwrap()
        });
        held.surcharge = 125.0;
        draft.penalty.computed_penalties = Some(held);
        draft.lifecycle.trigger_date_entered = false;
        with_clock(day(2026, 10, 19), || draft.compute());
        assert_eq!(draft.penalty.computed_penalties, None);
        for component in PenaltyComponent::ALL {
            assert_eq!(
                draft.penalty.penalty_overrides.get(component),
                None,
                "{component:?}"
            );
        }
        assert_eq!(draft.surcharge, 0.0);
        // A real override stays one.
        items.set_override(&mut draft, PenaltyComponent::Interest, Some(40.0));
        with_clock(day(2026, 10, 19), || draft.compute());
        assert_eq!(
            draft
                .penalty
                .penalty_overrides
                .get(PenaltyComponent::Interest),
            Some(40.0)
        );
        assert_eq!(draft.interest, 40.0);
    }

    /// Decision P4-3 moved 1606 off the monthly rule; its penalties still
    /// run from the date that rule gave. The rule it shared with 0620 (the
    /// 10th of the next month) still resolves 0620, so every 1606 month in
    /// 2025-2027, with no overrides and with the shipped holidays, must
    /// give 0620's date.
    #[test]
    fn form_1606_penalties_keep_the_former_monthly_due_date() {
        let holidays = crate::official_holidays::holiday_overrides(
            crate::official_holidays::OFFICIAL_HOLIDAY_YEARS,
        );
        for overrides in [Vec::new(), holidays] {
            for taxable_year in 2025..=2027 {
                for month in 1..=12 {
                    let period = DeadlinePeriod::Monthly {
                        taxable_year,
                        month,
                    };
                    let former = resolve_due_date("0620", &period, &overrides);
                    assert!(former.is_some());
                    assert_eq!(
                        resolve_due_date("1606", &period, &overrides),
                        former,
                        "{taxable_year}-{month}"
                    );
                    // 2000OT is due on a day of the following month too, but
                    // takes no fallback: its penalties name no month.
                    assert_eq!(
                        resolve_due_date("2000OT", &period, &overrides),
                        None,
                        "2000OT {taxable_year}-{month}"
                    );
                }
            }
        }
    }
}
