//! Static official BIR tax calendar rules.
//!
//! Base recurring rules are compiled into the app. Calendar-year views are
//! resolved separately from taxable-year views so cross-year deadlines stay in
//! the correct UI year.

use chrono::{Datelike, Duration, NaiveDate};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeadlinePeriod {
    Monthly { taxable_year: i32, month: u8 },
    Quarterly { taxable_year: i32, quarter: u8 },
    Annual { taxable_year: i32 },
    EventBased,
}

impl DeadlinePeriod {
    pub fn taxable_year(&self) -> Option<i32> {
        match self {
            Self::Monthly { taxable_year, .. }
            | Self::Quarterly { taxable_year, .. }
            | Self::Annual { taxable_year } => Some(*taxable_year),
            Self::EventBased => None,
        }
    }

    pub fn quarter(&self) -> Option<u8> {
        match self {
            Self::Monthly { month, .. } => Some(((*month - 1) / 3) + 1),
            Self::Quarterly { quarter, .. } => Some(*quarter),
            Self::Annual { .. } | Self::EventBased => None,
        }
    }

    pub fn month(&self) -> Option<u8> {
        match self {
            Self::Monthly { month, .. } => Some(*month),
            Self::Quarterly { .. } | Self::Annual { .. } | Self::EventBased => None,
        }
    }

    fn matches_month_filter(&self, months: &[u8]) -> bool {
        match self {
            Self::Monthly { month, .. } => months.contains(month),
            Self::Quarterly { quarter, .. } => {
                let first_month = ((*quarter - 1) * 3) + 1;
                let last_month = first_month + 2;
                months
                    .iter()
                    .any(|month| (first_month..=last_month).contains(month))
            }
            Self::Annual { .. } | Self::EventBased => false,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::Monthly {
                taxable_year,
                month,
            } => format!("Tax period: {taxable_year}-{month:02}"),
            Self::Quarterly {
                taxable_year,
                quarter,
            } => format!("Tax period: {taxable_year} Q{quarter}"),
            Self::Annual { taxable_year } => format!("Tax period: {taxable_year} annual"),
            Self::EventBased => "Event-based obligation".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeadlineKind {
    Dated {
        original_deadline: NaiveDate,
        final_deadline: NaiveDate,
    },
    EventBased {
        trigger: String,
        statutory_window: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeadlineStatus {
    Normal,
    WeekendAdjusted,
    HolidayAdjusted,
    NonWorkingDayAdjusted,
    Extended,
    EventBased,
}

impl DeadlineStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::WeekendAdjusted => "Weekend Adjusted",
            Self::HolidayAdjusted => "Holiday Adjusted",
            Self::NonWorkingDayAdjusted => "Non-working Day Adjusted",
            Self::Extended => "Extended",
            Self::EventBased => "Event Based",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NonWorkingDayKind {
    RegularHoliday,
    LocalHoliday,
    SpecialNonWorkingDay,
    OtherClosure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NonWorkingDay {
    pub date: NaiveDate,
    pub name: String,
    pub kind: NonWorkingDayKind,
    pub regions: Vec<String>,
    pub source_reference: Option<String>,
}

impl NonWorkingDay {
    pub fn regular_holiday(date: NaiveDate, name: impl Into<String>) -> Self {
        Self {
            date,
            name: name.into(),
            kind: NonWorkingDayKind::RegularHoliday,
            regions: Vec::new(),
            source_reference: None,
        }
    }

    pub fn special_non_working_day(date: NaiveDate, name: impl Into<String>) -> Self {
        Self {
            date,
            name: name.into(),
            kind: NonWorkingDayKind::SpecialNonWorkingDay,
            regions: Vec::new(),
            source_reference: None,
        }
    }

    fn adjustment_status(&self) -> DeadlineStatus {
        match self.kind {
            NonWorkingDayKind::RegularHoliday | NonWorkingDayKind::LocalHoliday => {
                DeadlineStatus::HolidayAdjusted
            }
            NonWorkingDayKind::SpecialNonWorkingDay | NonWorkingDayKind::OtherClosure => {
                DeadlineStatus::NonWorkingDayAdjusted
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BusinessDayCalendar {
    pub non_working_days: Vec<NonWorkingDay>,
}

impl BusinessDayCalendar {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_non_working_days(non_working_days: Vec<NonWorkingDay>) -> Self {
        Self { non_working_days }
    }

    pub fn is_non_working_day(&self, date: NaiveDate) -> bool {
        self.non_working_reason(date).is_some()
    }

    pub fn is_business_day(&self, date: NaiveDate) -> bool {
        self.non_working_reason(date).is_none()
    }

    pub fn adjust_to_next_business_day(&self, date: NaiveDate) -> (NaiveDate, DeadlineStatus) {
        let Some(status) = self.non_working_reason(date) else {
            return (date, DeadlineStatus::Normal);
        };

        let mut adjusted = date + Duration::days(1);
        while self.is_non_working_day(adjusted) {
            adjusted += Duration::days(1);
        }

        (adjusted, status)
    }

    fn non_working_reason(&self, date: NaiveDate) -> Option<DeadlineStatus> {
        match date.weekday() {
            chrono::Weekday::Sat | chrono::Weekday::Sun => Some(DeadlineStatus::WeekendAdjusted),
            _ => self
                .non_working_days
                .iter()
                .find(|day| day.date == date)
                .map(NonWorkingDay::adjustment_status),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedTaxDeadline {
    pub form_code: String,
    pub display_form_no: String,
    pub form_name: String,
    pub period: DeadlinePeriod,
    pub period_start: Option<NaiveDate>,
    pub period_end: Option<NaiveDate>,
    pub deadline: DeadlineKind,
    pub status: DeadlineStatus,
    pub description: String,
    pub source_reference: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadlineOverride {
    pub id: String,
    pub title: String,
    pub source_reference: String,
    pub affected_form_codes: Vec<String>,
    pub original_deadline: NaiveDate,
    pub adjusted_deadline: NaiveDate,
    pub affected_regions: Vec<String>,
    pub affected_taxpayer_types: Vec<String>,
    pub effective_from: Option<NaiveDate>,
    pub effective_until: Option<NaiveDate>,
    pub expires_at: Option<NaiveDate>,
}

impl ResolvedTaxDeadline {
    fn dated(
        display_form_no: &str,
        form_name: &str,
        period: DeadlinePeriod,
        period_start: NaiveDate,
        period_end: NaiveDate,
        original_deadline: NaiveDate,
        description: &str,
    ) -> Self {
        let (final_deadline, status) =
            BusinessDayCalendar::default().adjust_to_next_business_day(original_deadline);

        Self {
            form_code: canonical_form_code(display_form_no).to_string(),
            display_form_no: display_form_no.to_string(),
            form_name: form_name.to_string(),
            period,
            period_start: Some(period_start),
            period_end: Some(period_end),
            deadline: DeadlineKind::Dated {
                original_deadline,
                final_deadline,
            },
            status,
            description: description.to_string(),
            source_reference: None,
        }
    }

    fn event_based(
        display_form_no: &str,
        form_name: &str,
        trigger: &str,
        statutory_window: &str,
        description: &str,
    ) -> Self {
        Self {
            form_code: canonical_form_code(display_form_no).to_string(),
            display_form_no: display_form_no.to_string(),
            form_name: form_name.to_string(),
            period: DeadlinePeriod::EventBased,
            period_start: None,
            period_end: None,
            deadline: DeadlineKind::EventBased {
                trigger: trigger.to_string(),
                statutory_window: statutory_window.to_string(),
            },
            status: DeadlineStatus::EventBased,
            description: description.to_string(),
            source_reference: None,
        }
    }

    pub fn final_deadline_date(&self) -> Option<NaiveDate> {
        match self.deadline {
            DeadlineKind::Dated { final_deadline, .. } => Some(final_deadline),
            DeadlineKind::EventBased { .. } => None,
        }
    }

    pub fn original_deadline_date(&self) -> Option<NaiveDate> {
        match self.deadline {
            DeadlineKind::Dated {
                original_deadline, ..
            } => Some(original_deadline),
            DeadlineKind::EventBased { .. } => None,
        }
    }

    pub fn final_deadline_string(&self) -> String {
        self.final_deadline_date()
            .map(|date| date.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "Event Based".to_string())
    }

    pub fn period_start_string(&self) -> String {
        self.period_start
            .map(|date| date.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "-".to_string())
    }

    pub fn period_end_string(&self) -> String {
        self.period_end
            .map(|date| date.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "-".to_string())
    }

    pub fn route_year_quarter(&self) -> Option<(u16, u8)> {
        match self.period {
            DeadlinePeriod::Monthly {
                taxable_year,
                month,
            } => Some((taxable_year as u16, month)),
            DeadlinePeriod::Quarterly {
                taxable_year,
                quarter,
            } => Some((taxable_year as u16, quarter)),
            DeadlinePeriod::Annual { taxable_year } => Some((taxable_year as u16, 1)),
            DeadlinePeriod::EventBased => None,
        }
    }

    pub fn matches_period_filter(&self, months: &[u8], quarters: &[u8]) -> bool {
        if months.is_empty() && quarters.is_empty() {
            return true;
        }

        let month_match = !months.is_empty() && self.period.matches_month_filter(months);
        let quarter_match = !quarters.is_empty()
            && self
                .period
                .quarter()
                .is_some_and(|quarter| quarters.contains(&quarter));

        month_match || quarter_match
    }

    /// Filter by the actual deadline DATE's month/quarter, not the filing period.
    /// This allows a quarterly form (e.g., 2551Q due Jul 25) to match a "July" month filter.
    pub fn matches_deadline_date_filter(&self, months: &[u8], quarters: &[u8]) -> bool {
        let Some(date) = self.final_deadline_date() else {
            // Event-based deadlines don't have a date; include them only when no filter is active
            return months.is_empty() && quarters.is_empty();
        };

        let date_month = date.month() as u8;
        let date_quarter = ((date_month - 1) / 3) + 1;

        let month_match = months.is_empty() || months.contains(&date_month);
        let quarter_match = quarters.is_empty() || quarters.contains(&date_quarter);

        month_match && quarter_match
    }

    fn apply_business_day_calendar(&mut self, calendar: &BusinessDayCalendar) {
        let DeadlineKind::Dated {
            original_deadline,
            final_deadline,
        } = &mut self.deadline
        else {
            return;
        };

        let (adjusted, status) = calendar.adjust_to_next_business_day(*original_deadline);
        *final_deadline = adjusted;
        self.status = status;
    }

    fn apply_override(
        &mut self,
        deadline_override: &DeadlineOverride,
        calendar: &BusinessDayCalendar,
    ) -> bool {
        if !deadline_override
            .affected_form_codes
            .iter()
            .any(|code| code == &self.form_code)
        {
            return false;
        }

        let DeadlineKind::Dated {
            original_deadline,
            final_deadline,
        } = &mut self.deadline
        else {
            return false;
        };

        if *original_deadline != deadline_override.original_deadline
            && *final_deadline != deadline_override.original_deadline
        {
            return false;
        }

        *final_deadline = calendar
            .adjust_to_next_business_day(deadline_override.adjusted_deadline)
            .0;
        // A shipped holiday (D8a) moves a deadline for the holiday; any
        // other override (a BIR extension, or one the user saved) extends it.
        // The desktop's former 2026 seed has random ids, not this prefix, so
        // the deadlines its stored entries move keep "Extended".
        self.status = if deadline_override
            .id
            .starts_with(crate::official_holidays::SEED_ID_PREFIX)
        {
            DeadlineStatus::HolidayAdjusted
        } else {
            DeadlineStatus::Extended
        };
        self.source_reference = Some(deadline_override.source_reference.clone());
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frequency {
    Monthly,
    Quarterly,
    Annual,
    AsNeeded,
}

/// What starts an obligation's filing clock (decision D8, 2026-10-18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilingTrigger {
    /// The close of a reporting period: a periodic return.
    PeriodEnd,
    /// A taxpayer event (a removal, a withholding, a sale, a death): an
    /// as-needed return, due relative to the event's date.
    Event,
}

/// The period one return of an obligation covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReportingPeriod {
    Monthly,
    Quarterly,
    Annual,
    /// One return per event.
    PerEvent,
}

#[derive(Clone)]
pub struct OfficialRule {
    pub form_nos: Vec<&'static str>,
    pub form_name: &'static str,
    pub frequency: Frequency,
    pub filing_trigger: FilingTrigger,
    pub reporting_period: ReportingPeriod,
    pub description: &'static str,
    pub generator: fn(i32, &[&str], &str, &str) -> Vec<ResolvedTaxDeadline>,
}

impl OfficialRule {
    /// An obligation filed when an event happens, not at a period's close.
    pub fn is_as_needed(&self) -> bool {
        self.filing_trigger == FilingTrigger::Event
    }
}

/// How an as-needed return's due date follows from its trigger date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DueFromTrigger {
    /// Due on the trigger date itself (the excise returns: on removal).
    OnTheDate,
    /// Due `n` calendar days after the trigger date.
    DaysAfter(u32),
    /// Due `n` banking (working) days after the trigger date.
    BankingDaysAfter(u32),
    /// Due on day `n` of the month after the trigger date's month.
    DayOfFollowingMonth(u32),
    /// Due one year after the trigger date.
    OneYearAfter,
    /// The trigger date the filer types is the due date (0605).
    IsTheDueDate,
}

/// The due-date rule of one as-needed form, read from its official guide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventDueRule {
    pub form_code: &'static str,
    /// What the trigger date is, as a row names it ("Removal").
    pub trigger: &'static str,
    pub due: DueFromTrigger,
    /// Whether a due date on a weekend or holiday moves to the next working
    /// day. The six removal excise returns (2200A, 2200AN, 2200M, 2200P,
    /// 2200S, 2200T) are due on the removal date, unmoved, as PR 11 measures
    /// their penalties from the typed removal date. 2200C is not one of
    /// them: it is monthly, due the 10th of the next month, and rolls to the
    /// next business day.
    pub rolls_to_business_day: bool,
    pub description: &'static str,
    /// Where the rule is printed.
    pub source: &'static str,
}

/// Every as-needed form whose due date follows from a date its return holds.
pub const EVENT_DUE_RULES: &[EventDueRule] = &[
    EventDueRule {
        form_code: "2200A",
        trigger: "Removal",
        due: DueFromTrigger::OnTheDate,
        rolls_to_business_day: false,
        description: "Before removal of the alcohol products",
        source: "rules/forms/2200a-v2020/workflow.json filing_deadlines",
    },
    EventDueRule {
        form_code: "2200AN",
        trigger: "Removal",
        due: DueFromTrigger::OnTheDate,
        rolls_to_business_day: false,
        description: "Before removal of the covered products",
        source: "rules/forms/2200an-v2018/workflow.json filing_deadlines",
    },
    EventDueRule {
        form_code: "2200M",
        trigger: "Removal",
        due: DueFromTrigger::OnTheDate,
        rolls_to_business_day: false,
        description: "Upon removal of the mineral products",
        source: "rules/forms/2200m-v2018/workflow.json filing_deadlines",
    },
    EventDueRule {
        form_code: "2200P",
        trigger: "Removal",
        due: DueFromTrigger::OnTheDate,
        rolls_to_business_day: false,
        description: "Before removal of the petroleum products",
        source: "rules/forms/2200p-v2020/workflow.json filing_deadlines",
    },
    EventDueRule {
        form_code: "2200S",
        trigger: "Removal",
        due: DueFromTrigger::OnTheDate,
        rolls_to_business_day: false,
        description: "Before removal of the covered products",
        source: "rules/forms/2200s-v2018/workflow.json filing_deadlines",
    },
    EventDueRule {
        form_code: "2200T",
        trigger: "Removal",
        due: DueFromTrigger::OnTheDate,
        rolls_to_business_day: false,
        description: "Before removal of the tobacco products",
        source: "rules/forms/2200t-v2020/workflow.json filing_deadlines",
    },
    EventDueRule {
        form_code: "1600WP",
        trigger: "Withholding",
        due: DueFromTrigger::DaysAfter(20),
        rolls_to_business_day: true,
        description: "Within 20 days from the date the tax was deducted and withheld",
        source: "html-frozen/1600wp-2010/guide.html When and Where to File and Remit/Pay",
    },
    EventDueRule {
        form_code: "0605",
        trigger: "Due",
        due: DueFromTrigger::IsTheDueDate,
        rolls_to_business_day: true,
        description: "The due date of the tax type paid",
        source: "html-frozen/0605-1999/guide.html When and Where to File and Pay",
    },
    // Decision P4-3 (2026-10-19): 1606 is as needed, dated by the sale, due
    // the 10th of the month after the sale, rolled to the next working day.
    // Not applied: rules/forms/1606-v2018/workflow.json's "large taxpayers
    // use the 25th" and its "December withholding is due January 25". Why:
    // no profile field marks a Large Taxpayers Service taxpayer (the EOPT
    // "Large" tier is a different classification), and both lines
    // paraphrase a help file the repo does not track. So every taxpayer
    // uses the 10th, and a December sale is due January 10, rolled.
    EventDueRule {
        form_code: "1606",
        trigger: "Sale",
        due: DueFromTrigger::DayOfFollowingMonth(10),
        rolls_to_business_day: true,
        description: "10th day of the month following the month of the sale",
        source: "Decision P4-3; rules/forms/1606-v2018/workflow.json filing_deadlines",
    },
    EventDueRule {
        form_code: "1706",
        trigger: "Transaction",
        due: DueFromTrigger::DaysAfter(30),
        rolls_to_business_day: true,
        description: "Within 30 days after the sale, exchange or disposition",
        source: "rules/forms/1706-v2018/workflow.json filing_deadlines",
    },
    EventDueRule {
        form_code: "1707",
        trigger: "Transaction",
        due: DueFromTrigger::DaysAfter(30),
        rolls_to_business_day: true,
        description: "Within 30 days after each sale, barter, exchange or disposition",
        source: "html-frozen/1707-2021/guide.html When and Where to File and Pay",
    },
    EventDueRule {
        form_code: "2000OT",
        trigger: "Transaction",
        due: DueFromTrigger::DayOfFollowingMonth(5),
        rolls_to_business_day: true,
        description: "Within five days after the close of the month of the taxable document",
        source: "rules/forms/2000ot-v2018/workflow.json filing_deadlines",
    },
    EventDueRule {
        form_code: "2552",
        trigger: "Collection",
        due: DueFromTrigger::BankingDaysAfter(5),
        rolls_to_business_day: true,
        description: "Within five banking days from the date of collection (LSE and secondary offering)",
        source: "html-frozen/2552-2018/guide.html When and Where to File and Pay",
    },
    EventDueRule {
        form_code: "1800",
        trigger: "Donation",
        due: DueFromTrigger::DaysAfter(30),
        rolls_to_business_day: true,
        description: "Within 30 days after the date the gift is made",
        source: "html-frozen/1800-2018/guide.html When and Where to File and Pay",
    },
    EventDueRule {
        form_code: "1801",
        trigger: "Death",
        due: DueFromTrigger::OneYearAfter,
        rolls_to_business_day: true,
        description: "Within one year from the decedent's death",
        source: "html-frozen/1801-2018/guide.html When and Where to File and Pay",
    },
];

/// 2552's primary public offering: within 30 days from the listing date.
pub const FORM_2552_PRIMARY_OFFERING_RULE: EventDueRule = EventDueRule {
    form_code: "2552",
    trigger: "Listing",
    due: DueFromTrigger::DaysAfter(30),
    rolls_to_business_day: true,
    description: "Within 30 days from the date of listing in the LSE (primary offering)",
    source: "html-frozen/2552-2018/guide.html When and Where to File and Pay",
};

/// The due-date rule of an as-needed form, if its return holds a trigger date.
pub fn event_due_rule(form_code: &str) -> Option<EventDueRule> {
    EVENT_DUE_RULES
        .iter()
        .find(|rule| rule.form_code == form_code)
        .copied()
}

impl DueFromTrigger {
    /// The statutory due date for a trigger on `trigger`, before any move to
    /// a working day. `working` decides banking days.
    pub fn due_date(self, trigger: NaiveDate, working: &BusinessDayCalendar) -> Option<NaiveDate> {
        match self {
            Self::OnTheDate | Self::IsTheDueDate => Some(trigger),
            Self::DaysAfter(days) => trigger.checked_add_signed(Duration::days(i64::from(days))),
            Self::BankingDaysAfter(days) => {
                let mut date = trigger;
                for _ in 0..days {
                    date = date.succ_opt()?;
                    while working.is_non_working_day(date) {
                        date = date.succ_opt()?;
                    }
                }
                Some(date)
            }
            Self::DayOfFollowingMonth(day) => {
                let (year, month) = next_month(trigger.year(), trigger.month());
                NaiveDate::from_ymd_opt(year, month, day)
            }
            Self::OneYearAfter => trigger.checked_add_months(chrono::Months::new(12)),
        }
    }
}

/// The as-needed codes and each one's rule name, built once from the static
/// rules (review F25: not per draft or row).
struct AsNeededIndex {
    codes: Vec<&'static str>,
    names: Vec<(&'static str, &'static str)>,
}

fn as_needed_index() -> &'static AsNeededIndex {
    static INDEX: std::sync::OnceLock<AsNeededIndex> = std::sync::OnceLock::new();
    INDEX.get_or_init(|| {
        let mut names = Vec::new();
        for rule in DeadlineResolver::rules() {
            if rule.is_as_needed() {
                for no in &rule.form_nos {
                    names.push((canonical_form_code(no), rule.form_name));
                }
            }
        }
        let mut codes: Vec<&'static str> = names.iter().map(|(code, _)| *code).collect();
        codes.sort_unstable();
        codes.dedup();
        AsNeededIndex { codes, names }
    })
}

pub struct DeadlineResolver;

impl DeadlineResolver {
    /// Compatibility alias for old call sites. New code should choose the
    /// taxable-year or calendar-year API explicitly.
    #[deprecated(note = "use resolve_taxable_year or resolve_deadline_calendar_year explicitly")]
    pub fn resolve_year(year: i32) -> Vec<ResolvedTaxDeadline> {
        Self::resolve_taxable_year(year)
    }

    pub fn resolve_taxable_year(taxable_year: i32) -> Vec<ResolvedTaxDeadline> {
        Self::resolve_taxable_year_with_overrides(taxable_year, &[])
    }

    pub fn resolve_taxable_year_with_calendar(
        taxable_year: i32,
        calendar: &BusinessDayCalendar,
    ) -> Vec<ResolvedTaxDeadline> {
        Self::resolve_taxable_year_with_overrides_and_calendar(taxable_year, &[], calendar)
    }

    pub fn resolve_taxable_year_with_overrides(
        taxable_year: i32,
        overrides: &[DeadlineOverride],
    ) -> Vec<ResolvedTaxDeadline> {
        Self::resolve_taxable_year_with_overrides_and_calendar(
            taxable_year,
            overrides,
            &BusinessDayCalendar::default(),
        )
    }

    pub fn resolve_taxable_year_with_overrides_and_calendar(
        taxable_year: i32,
        overrides: &[DeadlineOverride],
        calendar: &BusinessDayCalendar,
    ) -> Vec<ResolvedTaxDeadline> {
        let mut all_deadlines = Vec::new();

        for rule in Self::rules() {
            let deadlines = (rule.generator)(
                taxable_year,
                &rule.form_nos,
                rule.description,
                rule.form_name,
            );
            all_deadlines.extend(deadlines);
        }

        Self::apply_business_day_calendar(&mut all_deadlines, calendar);
        Self::apply_overrides(&mut all_deadlines, overrides, calendar);
        Self::sort_deadlines(all_deadlines)
    }

    /// Resolve the deadlines for a taxable year, restricted to a user-owned
    /// Forms Set (`form_codes`). This is the deadline source for the dashboard
    /// in the per-year Forms Set model: only the forms the taxpayer actually
    /// files produce deadlines. Form codes not backed by an official calendar
    /// rule (e.g. user-added custom forms) simply yield no deadlines.
    pub fn deadlines_for_forms(
        form_codes: &[String],
        taxable_year: i32,
        overrides: &[DeadlineOverride],
    ) -> Vec<ResolvedTaxDeadline> {
        let wanted: std::collections::BTreeSet<&str> =
            form_codes.iter().map(|code| code.as_str()).collect();
        Self::resolve_taxable_year_with_overrides(taxable_year, overrides)
            .into_iter()
            .filter(|deadline| wanted.contains(deadline.form_code.as_str()))
            .collect()
    }

    pub fn resolve_deadline_calendar_year(calendar_year: i32) -> Vec<ResolvedTaxDeadline> {
        Self::resolve_deadline_calendar_year_with_overrides(calendar_year, &[])
    }

    pub fn resolve_deadline_calendar_year_with_calendar(
        calendar_year: i32,
        calendar: &BusinessDayCalendar,
    ) -> Vec<ResolvedTaxDeadline> {
        Self::resolve_deadline_calendar_year_with_overrides_and_calendar(
            calendar_year,
            &[],
            calendar,
        )
    }

    pub fn resolve_deadline_calendar_year_with_overrides(
        calendar_year: i32,
        overrides: &[DeadlineOverride],
    ) -> Vec<ResolvedTaxDeadline> {
        Self::resolve_deadline_calendar_year_with_overrides_and_calendar(
            calendar_year,
            overrides,
            &BusinessDayCalendar::default(),
        )
    }

    pub fn resolve_deadline_calendar_year_with_overrides_and_calendar(
        calendar_year: i32,
        overrides: &[DeadlineOverride],
        calendar: &BusinessDayCalendar,
    ) -> Vec<ResolvedTaxDeadline> {
        let mut all_deadlines = Vec::new();

        for taxable_year in (calendar_year - 1)..=(calendar_year + 1) {
            all_deadlines.extend(Self::resolve_taxable_year_with_overrides_and_calendar(
                taxable_year,
                overrides,
                calendar,
            ));
        }

        let deadlines = all_deadlines
            .into_iter()
            .filter(|deadline| {
                deadline
                    .final_deadline_date()
                    .is_some_and(|date| date.year() == calendar_year)
            })
            .collect();

        Self::sort_deadlines(deadlines)
    }

    /// The canonical codes of every as-needed (event-triggered) obligation,
    /// sorted.
    pub fn as_needed_form_codes() -> Vec<&'static str> {
        as_needed_index().codes.clone()
    }

    /// Whether `form_code` (canonical) is filed as needed.
    pub fn is_as_needed(form_code: &str) -> bool {
        as_needed_index().codes.contains(&form_code)
    }

    /// The official rule `form_code` (canonical) is filed under, and the
    /// form number that rule names it by.
    fn rule_of(form_code: &str) -> Option<(&'static OfficialRule, &'static str)> {
        Self::rules().iter().find_map(|rule| {
            rule.form_nos
                .iter()
                .find(|no| canonical_form_code(no) == form_code)
                .map(|no| (rule, *no))
        })
    }

    /// How often `form_code` (canonical) is filed, from its official rule.
    /// `None` for a form with no calendar rule (e.g. the retired 2550M).
    pub fn frequency_of(form_code: &str) -> Option<Frequency> {
        Self::rule_of(form_code).map(|(rule, _)| rule.frequency.clone())
    }

    /// The latest period of `form_code` (canonical) that has closed by
    /// `today` (its last day is before `today`): the return to file now.
    /// Read off the form's own calendar rule, so a form without a fourth
    /// quarter (1701Q, filed for Q1 to Q3) gets its Q3 in January. `None`
    /// for an as-needed form or one with no calendar rule.
    pub fn last_closed_period(form_code: &str, today: NaiveDate) -> Option<DeadlinePeriod> {
        let (rule, form_no) = Self::rule_of(form_code)?;
        if rule.is_as_needed() {
            return None;
        }
        // On January 1 the previous year's annual return is the latest
        // closed one, so two years back is as far as it can be.
        ((today.year() - 2)..=today.year())
            .flat_map(|year| (rule.generator)(year, &[form_no], rule.description, rule.form_name))
            .filter(|deadline| deadline.period_end.is_some_and(|end| end < today))
            .max_by_key(|deadline| deadline.period_end)
            .map(|deadline| deadline.period)
    }

    /// The dated deadline of one as-needed return whose trigger date is
    /// `trigger`: due per `rule`, moved to the next working day (weekends,
    /// then `overrides` for that date) when the rule rolls. `working` is the
    /// calendar banking days are counted on. `None` when the date overflows.
    pub fn event_deadline(
        rule: &EventDueRule,
        trigger: NaiveDate,
        overrides: &[DeadlineOverride],
        working: &BusinessDayCalendar,
    ) -> Option<ResolvedTaxDeadline> {
        let due = rule.due.due_date(trigger, working)?;
        let form_name = as_needed_index()
            .names
            .iter()
            .find(|(code, _)| *code == rule.form_code)
            .map_or("As Needed / Special / Event-Based", |(_, name)| name);
        let mut deadline = ResolvedTaxDeadline {
            form_code: rule.form_code.to_string(),
            display_form_no: rule.form_code.to_string(),
            form_name: form_name.to_string(),
            period: DeadlinePeriod::EventBased,
            period_start: Some(trigger),
            period_end: Some(trigger),
            deadline: DeadlineKind::Dated {
                original_deadline: due,
                final_deadline: due,
            },
            status: DeadlineStatus::Normal,
            description: rule.description.to_string(),
            source_reference: Some(rule.source.to_string()),
        };
        if rule.rolls_to_business_day {
            deadline.apply_business_day_calendar(&BusinessDayCalendar::default());
            for deadline_override in overrides {
                if deadline_override.source_reference.trim().is_empty() {
                    continue;
                }
                deadline.apply_override(deadline_override, &BusinessDayCalendar::default());
            }
        }
        Some(deadline)
    }

    pub fn event_based_obligations() -> Vec<ResolvedTaxDeadline> {
        Self::rules()
            .iter()
            .filter(|rule| rule.frequency == Frequency::AsNeeded)
            .flat_map(|rule| (rule.generator)(0, &rule.form_nos, rule.description, rule.form_name))
            .collect()
    }

    /// The official rules (a copy; they are built once, see [`Self::rules`]).
    pub fn official_rules() -> Vec<OfficialRule> {
        Self::rules().to_vec()
    }

    /// The official rules, built once (review F25: not per call of the
    /// resolvers the feeds run per branch and year).
    fn rules() -> &'static [OfficialRule] {
        static RULES: std::sync::OnceLock<Vec<OfficialRule>> = std::sync::OnceLock::new();
        RULES.get_or_init(Self::build_rules)
    }

    fn build_rules() -> Vec<OfficialRule> {
        vec![
            // NOTE: Form 2000 (DST) is NOT a recurring monthly obligation.
            // It is event-based — only filed when a taxable document transaction
            // occurs in a given month. Moved to the AsNeeded bucket below.
            OfficialRule {
                // 1606 is as needed (decision P4-3), in the rule below.
                form_nos: vec!["0620", "1600-VT", "1600-PT", "2200-C"],
                form_name: "Monthly Remittance / Excise Tax",
                frequency: Frequency::Monthly,
                filing_trigger: FilingTrigger::PeriodEnd,
                reporting_period: ReportingPeriod::Monthly,
                description: "10th day of the following month",
                generator: |year, forms, desc, name| {
                    Self::gen_monthly(year, forms, desc, name, |y, m| {
                        let (ty, tm) = next_month(y, m);
                        NaiveDate::from_ymd_opt(ty, tm, 10).unwrap()
                    })
                },
            },
            OfficialRule {
                form_nos: vec!["0619-E", "0619-F", "1601-C"],
                form_name: "Withholding Tax Remittance",
                frequency: Frequency::Monthly,
                filing_trigger: FilingTrigger::PeriodEnd,
                reporting_period: ReportingPeriod::Monthly,
                description: "10th day of the following month (Non-eFPS) / 15th for Dec",
                generator: |year, forms, desc, name| {
                    Self::gen_monthly(year, forms, desc, name, |y, m| {
                        let (ty, tm) = next_month(y, m);
                        let day = if m == 12 { 15 } else { 10 };
                        NaiveDate::from_ymd_opt(ty, tm, day).unwrap()
                    })
                },
            },
            OfficialRule {
                form_nos: vec!["1601-EQ", "1601-FQ", "1602Q", "1603Q", "1621"],
                form_name: "Quarterly Withholding Tax",
                frequency: Frequency::Quarterly,
                filing_trigger: FilingTrigger::PeriodEnd,
                reporting_period: ReportingPeriod::Quarterly,
                description: "Last day of the month following the close of the quarter",
                generator: |year, forms, desc, name| {
                    Self::gen_quarterly(year, forms, desc, name, |y, q| {
                        let (ty, tm) = if q == 4 { (y + 1, 1) } else { (y, q * 3 + 1) };
                        Self::last_day_of_month(ty, tm)
                    })
                },
            },
            OfficialRule {
                form_nos: vec!["1701Q"],
                form_name: "Quarterly Income Tax Return (Individual)",
                frequency: Frequency::Quarterly,
                filing_trigger: FilingTrigger::PeriodEnd,
                reporting_period: ReportingPeriod::Quarterly,
                // BIR Form 1701Q (January 2018) guidelines, "When and Where
                // to File": May 15, August 15 and November 15 of the taxable
                // year (html-frozen/1701q-2018/guide.html;
                // rules/forms/1701q-v2018/workflow.json filing_deadlines).
                description: "Q1: May 15, Q2: Aug 15, Q3: Nov 15",
                generator: |year, forms, desc, name| {
                    let mut res = Vec::new();
                    let due_dates = [(1, 5, 15), (2, 8, 15), (3, 11, 15)];
                    for &form_no in forms {
                        for (quarter, due_month, due_day) in due_dates {
                            let end_month = quarter * 3;
                            let start = NaiveDate::from_ymd_opt(year, end_month - 2, 1).unwrap();
                            let end = Self::last_day_of_month(year, end_month);
                            let deadline =
                                NaiveDate::from_ymd_opt(year, due_month, due_day).unwrap();
                            res.push(ResolvedTaxDeadline::dated(
                                form_no,
                                name,
                                DeadlinePeriod::Quarterly {
                                    taxable_year: year,
                                    quarter: quarter as u8,
                                },
                                start,
                                end,
                                deadline,
                                desc,
                            ));
                        }
                    }
                    res
                },
            },
            OfficialRule {
                form_nos: vec!["1702Q"],
                form_name: "Quarterly Income Tax Return (Corporate)",
                frequency: Frequency::Quarterly,
                filing_trigger: FilingTrigger::PeriodEnd,
                reporting_period: ReportingPeriod::Quarterly,
                description: "60 days after the end of each quarter",
                generator: |year, forms, desc, name| {
                    Self::gen_quarterly(year, forms, desc, name, |y, q| {
                        let end = Self::last_day_of_month(y, q * 3);
                        end + Duration::days(60)
                    })
                },
            },
            OfficialRule {
                form_nos: vec!["2550-DS", "2550Q", "2551Q"],
                form_name: "Quarterly Percentage/Value-Added Tax",
                frequency: Frequency::Quarterly,
                filing_trigger: FilingTrigger::PeriodEnd,
                reporting_period: ReportingPeriod::Quarterly,
                description: "25th day following the close of taxable quarter",
                generator: |year, forms, desc, name| {
                    Self::gen_quarterly(year, forms, desc, name, |y, q| {
                        let (ty, tm) = if q == 4 { (y + 1, 1) } else { (y, q * 3 + 1) };
                        NaiveDate::from_ymd_opt(ty, tm, 25).unwrap()
                    })
                },
            },
            // 2200-M is due per removal (decision D8, 2026-10-18), so it is
            // only in the as-needed rule below; it has no quarterly rule.
            OfficialRule {
                form_nos: vec!["1604-C", "1604-F"],
                form_name: "Annual Information Return",
                frequency: Frequency::Annual,
                filing_trigger: FilingTrigger::PeriodEnd,
                reporting_period: ReportingPeriod::Annual,
                description: "January 31 following the calendar year",
                generator: |year, forms, desc, name| {
                    Self::gen_annual(year, forms, desc, name, |y| {
                        NaiveDate::from_ymd_opt(y + 1, 1, 31).unwrap()
                    })
                },
            },
            OfficialRule {
                form_nos: vec!["1604-E"],
                form_name: "Annual Information Return",
                frequency: Frequency::Annual,
                filing_trigger: FilingTrigger::PeriodEnd,
                reporting_period: ReportingPeriod::Annual,
                description: "March 1 following the calendar year",
                generator: |year, forms, desc, name| {
                    Self::gen_annual(year, forms, desc, name, |y| {
                        NaiveDate::from_ymd_opt(y + 1, 3, 1).unwrap()
                    })
                },
            },
            OfficialRule {
                form_nos: vec![
                    "1700", "1701", "1701A", "1701-MS", "1702", "1702-EX", "1702-MX", "1702-RT",
                    "1707-A",
                ],
                form_name: "Annual Income Tax Return",
                frequency: Frequency::Annual,
                filing_trigger: FilingTrigger::PeriodEnd,
                reporting_period: ReportingPeriod::Annual,
                description: "April 15 / 15th day of 4th month following taxable year",
                generator: |year, forms, desc, name| {
                    Self::gen_annual(year, forms, desc, name, |y| {
                        NaiveDate::from_ymd_opt(y + 1, 4, 15).unwrap()
                    })
                },
            },
            OfficialRule {
                form_nos: vec!["2316"],
                form_name: "Certificate of Compensation Payment",
                frequency: Frequency::Annual,
                filing_trigger: FilingTrigger::PeriodEnd,
                reporting_period: ReportingPeriod::Annual,
                description: "February 28 (submission to BIR)",
                generator: |year, forms, desc, name| {
                    Self::gen_annual(year, forms, desc, name, |y| {
                        let d = if NaiveDate::from_ymd_opt(y + 1, 2, 29).is_some() {
                            29
                        } else {
                            28
                        };
                        NaiveDate::from_ymd_opt(y + 1, 2, d).unwrap()
                    })
                },
            },
            OfficialRule {
                form_nos: vec![
                    "2552", "1600-WP", "1706", "1707", "1800", "1801", "0605", "0611-A", "0613",
                    "1905", "2200-A", "2200-AN", "2200-M", "2200P", "2200-S", "2200-T", "2553",
                    "2000", "2000-OT", "1606",
                ],
                form_name: "As Needed / Special / Event-Based",
                frequency: Frequency::AsNeeded,
                filing_trigger: FilingTrigger::Event,
                reporting_period: ReportingPeriod::PerEvent,
                description: "Based on Special Law / Upon transaction",
                generator: |_year, forms, desc, name| {
                    forms
                        .iter()
                        .map(|form_no| {
                            ResolvedTaxDeadline::event_based(
                                form_no,
                                name,
                                "Taxpayer transaction or statutory event",
                                "Depends on the specific BIR form and triggering event",
                                desc,
                            )
                        })
                        .collect()
                },
            },
        ]
    }

    fn sort_deadlines(mut deadlines: Vec<ResolvedTaxDeadline>) -> Vec<ResolvedTaxDeadline> {
        deadlines.sort_by(
            |a, b| match (a.final_deadline_date(), b.final_deadline_date()) {
                (Some(a_date), Some(b_date)) => a_date
                    .cmp(&b_date)
                    .then_with(|| a.form_code.cmp(&b.form_code)),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.form_code.cmp(&b.form_code),
            },
        );
        deadlines
    }

    fn apply_business_day_calendar(
        deadlines: &mut [ResolvedTaxDeadline],
        calendar: &BusinessDayCalendar,
    ) {
        for deadline in deadlines.iter_mut() {
            deadline.apply_business_day_calendar(calendar);
        }
    }

    fn apply_overrides(
        deadlines: &mut [ResolvedTaxDeadline],
        overrides: &[DeadlineOverride],
        calendar: &BusinessDayCalendar,
    ) {
        for deadline_override in overrides {
            if deadline_override.source_reference.trim().is_empty() {
                continue;
            }

            for deadline in deadlines.iter_mut() {
                deadline.apply_override(deadline_override, calendar);
            }
        }
    }

    fn gen_monthly(
        year: i32,
        forms: &[&str],
        desc: &str,
        name: &str,
        f: impl Fn(i32, u32) -> NaiveDate,
    ) -> Vec<ResolvedTaxDeadline> {
        let mut res = Vec::new();
        for &form_no in forms {
            for month in 1..=12 {
                let start = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
                let end = Self::last_day_of_month(year, month);
                let deadline = f(year, month);
                res.push(ResolvedTaxDeadline::dated(
                    form_no,
                    name,
                    DeadlinePeriod::Monthly {
                        taxable_year: year,
                        month: month as u8,
                    },
                    start,
                    end,
                    deadline,
                    desc,
                ));
            }
        }
        res
    }

    fn gen_quarterly(
        year: i32,
        forms: &[&str],
        desc: &str,
        name: &str,
        f: impl Fn(i32, u32) -> NaiveDate,
    ) -> Vec<ResolvedTaxDeadline> {
        let mut res = Vec::new();
        for &form_no in forms {
            for quarter in 1..=4 {
                let start_month = (quarter - 1) * 3 + 1;
                let start = NaiveDate::from_ymd_opt(year, start_month, 1).unwrap();
                let end = Self::last_day_of_month(year, start_month + 2);
                let deadline = f(year, quarter);
                res.push(ResolvedTaxDeadline::dated(
                    form_no,
                    name,
                    DeadlinePeriod::Quarterly {
                        taxable_year: year,
                        quarter: quarter as u8,
                    },
                    start,
                    end,
                    deadline,
                    desc,
                ));
            }
        }
        res
    }

    fn gen_annual(
        year: i32,
        forms: &[&str],
        desc: &str,
        name: &str,
        f: impl Fn(i32) -> NaiveDate,
    ) -> Vec<ResolvedTaxDeadline> {
        let mut res = Vec::new();
        for &form_no in forms {
            let start = NaiveDate::from_ymd_opt(year, 1, 1).unwrap();
            let end = NaiveDate::from_ymd_opt(year, 12, 31).unwrap();
            let deadline = f(year);
            res.push(ResolvedTaxDeadline::dated(
                form_no,
                name,
                DeadlinePeriod::Annual { taxable_year: year },
                start,
                end,
                deadline,
                desc,
            ));
        }
        res
    }

    fn last_day_of_month(year: i32, month: u32) -> NaiveDate {
        let (ny, nm) = if month == 12 {
            (year + 1, 1)
        } else {
            (year, month + 1)
        };
        NaiveDate::from_ymd_opt(ny, nm, 1).unwrap() - Duration::days(1)
    }
}

pub fn canonical_form_code(display_form_no: &str) -> &'static str {
    match display_form_no {
        "0619-E" => "0619E",
        "0619-F" => "0619F",
        "1600-WP" => "1600WP",
        "1601-C" => "1601C",
        "1601-EQ" => "1601EQ",
        "1601-FQ" => "1601FQ",
        "1604-C" | "1604C" => "1604C",
        "1604-F" | "1604F" => "1604F",
        "1604CF" => "1604CF",
        "1604-E" => "1604E",
        "1701-MS" => "1701MS",
        "1702-EX" => "1702EX",
        "1702-MX" => "1702MX",
        "1702-RT" => "1702RT",
        "1707-A" => "1707A",
        "2000-OT" => "2000OT",
        "2200-A" => "2200A",
        "2200-AN" => "2200AN",
        "2200-M" => "2200M",
        "2200-S" => "2200S",
        "2200-T" => "2200T",
        "2550-DS" => "2550DS",
        "1600-VT" => "1600VT",
        "1600-PT" => "1600PT",
        "2200-C" => "2200C",
        other => match other {
            "0605" => "0605",
            "0611-A" => "0611A",
            "0613" => "0613",
            "0620" => "0620",
            "1600PT" => "1600PT",
            "1600VT" => "1600VT",
            "1606" => "1606",
            "1602Q" => "1602Q",
            "1603Q" => "1603Q",
            "1621" => "1621",
            "1700" => "1700",
            "1701" => "1701",
            "1701A" => "1701A",
            "1701Q" => "1701Q",
            "1702" => "1702",
            "1702Q" => "1702Q",
            "1706" => "1706",
            "1707" => "1707",
            "1800" => "1800",
            "1801" => "1801",
            "1905" => "1905",
            "2000" => "2000",
            "2000OT" => "2000OT",
            "2200P" => "2200P",
            "2316" => "2316",
            "2550Q" => "2550Q",
            "2551Q" => "2551Q",
            "2552" => "2552",
            "2553" => "2553",
            _ => "UNKNOWN",
        },
    }
}

fn next_month(year: i32, month: u32) -> (i32, u32) {
    if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_year_includes_prior_taxable_year_annual_itr() {
        let deadlines = DeadlineResolver::resolve_deadline_calendar_year(2026);
        let annual_1701 = deadlines.iter().find(|d| {
            d.form_code == "1701" && d.period == (DeadlinePeriod::Annual { taxable_year: 2025 })
        });

        let annual_1701 = annual_1701.expect("1701 annual taxable year 2025 deadline missing");
        assert_eq!(
            annual_1701.final_deadline_date(),
            Some(NaiveDate::from_ymd_opt(2026, 4, 15).unwrap())
        );
    }

    #[test]
    fn calendar_year_includes_prior_december_monthly_deadlines() {
        let deadlines = DeadlineResolver::resolve_deadline_calendar_year(2026);
        let december_1601c = deadlines.iter().find(|d| {
            d.form_code == "1601C"
                && d.period
                    == (DeadlinePeriod::Monthly {
                        taxable_year: 2025,
                        month: 12,
                    })
        });

        let december_1601c = december_1601c.expect("December 2025 1601C deadline missing");
        assert_eq!(
            december_1601c.final_deadline_date(),
            Some(NaiveDate::from_ymd_opt(2026, 1, 15).unwrap())
        );
    }

    #[test]
    fn calendar_year_excludes_next_calendar_year_deadlines() {
        let deadlines = DeadlineResolver::resolve_deadline_calendar_year(2026);
        assert!(!deadlines.iter().any(|d| {
            d.form_code == "1701"
                && d.period == (DeadlinePeriod::Annual { taxable_year: 2026 })
                && d.final_deadline_date() == Some(NaiveDate::from_ymd_opt(2027, 4, 15).unwrap())
        }));
    }

    #[test]
    fn q1_1701q_keeps_taxable_quarter_metadata() {
        let deadlines = DeadlineResolver::resolve_taxable_year(2026);
        let q1 = deadlines
            .iter()
            .find(|d| {
                d.form_code == "1701Q"
                    && d.period
                        == (DeadlinePeriod::Quarterly {
                            taxable_year: 2026,
                            quarter: 1,
                        })
            })
            .expect("1701Q Q1 missing");

        assert_eq!(
            q1.final_deadline_date(),
            Some(NaiveDate::from_ymd_opt(2026, 5, 15).unwrap())
        );
        assert!(q1.matches_period_filter(&[], &[1]));
        assert!(!q1.matches_period_filter(&[], &[2]));
        assert!(q1.matches_period_filter(&[1], &[]));
        assert!(q1.matches_period_filter(&[2], &[]));
        assert!(q1.matches_period_filter(&[3], &[]));
        assert!(!q1.matches_period_filter(&[4], &[]));
        assert!(!q1.matches_period_filter(&[5], &[]));
    }

    /// The 1701Q deadlines as the January 2018 guidelines print them
    /// ("1st quarter On or before May 15 ... 2nd quarter On or before August
    /// 15 ... 3rd quarter On or before November 15 of the current taxable
    /// year", html-frozen/1701q-2018/guide.html), as the rules corpus records
    /// them (rules/forms/1701q-v2018/workflow.json filing_deadlines).
    #[test]
    fn form_1701q_quarters_are_due_may_15_august_15_and_november_15() {
        for year in [2025, 2026] {
            let deadlines = DeadlineResolver::resolve_taxable_year(year);
            for (quarter, month) in [(1u8, 5u32), (2, 8), (3, 11)] {
                let found = deadlines
                    .iter()
                    .find(|d| {
                        d.form_code == "1701Q"
                            && d.period
                                == (DeadlinePeriod::Quarterly {
                                    taxable_year: year,
                                    quarter,
                                })
                    })
                    .unwrap_or_else(|| panic!("1701Q {year} Q{quarter}"));
                assert_eq!(
                    found.original_deadline_date(),
                    NaiveDate::from_ymd_opt(year, month, 15),
                    "{year} Q{quarter}"
                );
            }
        }
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/rules-forms/1701q-v2018/workflow.json"
        ))
        .unwrap();
        let rules: Vec<&str> = corpus["filing_deadlines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["due_date_rule"].as_str().unwrap())
            .collect();
        assert_eq!(
            rules,
            [
                "On or before May 15 of the current taxable year.",
                "On or before August 15 of the current taxable year.",
                "On or before November 15 of the current taxable year.",
            ]
        );
    }

    #[test]
    fn weekend_adjustment_preserves_original_deadline() {
        let deadlines = DeadlineResolver::resolve_taxable_year(2026);
        let q2_1701q = deadlines
            .iter()
            .find(|d| {
                d.form_code == "1701Q"
                    && d.period
                        == (DeadlinePeriod::Quarterly {
                            taxable_year: 2026,
                            quarter: 2,
                        })
            })
            .expect("1701Q Q2 missing");

        assert_eq!(
            q2_1701q.original_deadline_date(),
            Some(NaiveDate::from_ymd_opt(2026, 8, 15).unwrap())
        );
        assert_eq!(
            q2_1701q.final_deadline_date(),
            Some(NaiveDate::from_ymd_opt(2026, 8, 17).unwrap())
        );
        assert_eq!(q2_1701q.status, DeadlineStatus::WeekendAdjusted);
    }

    #[test]
    fn business_day_calendar_moves_saturday_and_sunday_to_monday() {
        let calendar = BusinessDayCalendar::default();

        let saturday = NaiveDate::from_ymd_opt(2026, 8, 15).unwrap();
        let sunday = NaiveDate::from_ymd_opt(2026, 8, 16).unwrap();
        let monday = NaiveDate::from_ymd_opt(2026, 8, 17).unwrap();

        assert_eq!(
            calendar.adjust_to_next_business_day(saturday),
            (monday, DeadlineStatus::WeekendAdjusted)
        );
        assert_eq!(
            calendar.adjust_to_next_business_day(sunday),
            (monday, DeadlineStatus::WeekendAdjusted)
        );
    }

    #[test]
    fn business_day_calendar_keeps_advancing_after_weekend_into_holiday() {
        let calendar =
            BusinessDayCalendar::with_non_working_days(vec![NonWorkingDay::regular_holiday(
                NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
                "Configured Monday holiday",
            )]);

        assert_eq!(
            calendar.adjust_to_next_business_day(NaiveDate::from_ymd_opt(2026, 5, 30).unwrap()),
            (
                NaiveDate::from_ymd_opt(2026, 6, 2).unwrap(),
                DeadlineStatus::WeekendAdjusted
            )
        );
    }

    #[test]
    fn business_day_calendar_moves_friday_holiday_past_following_weekend() {
        let calendar =
            BusinessDayCalendar::with_non_working_days(vec![NonWorkingDay::regular_holiday(
                NaiveDate::from_ymd_opt(2026, 6, 12).unwrap(),
                "Configured Friday holiday",
            )]);

        assert_eq!(
            calendar.adjust_to_next_business_day(NaiveDate::from_ymd_opt(2026, 6, 12).unwrap()),
            (
                NaiveDate::from_ymd_opt(2026, 6, 15).unwrap(),
                DeadlineStatus::HolidayAdjusted
            )
        );
    }

    #[test]
    fn business_day_calendar_distinguishes_special_non_working_days() {
        let calendar = BusinessDayCalendar::with_non_working_days(vec![
            NonWorkingDay::special_non_working_day(
                NaiveDate::from_ymd_opt(2026, 11, 2).unwrap(),
                "Configured special non-working day",
            ),
        ]);

        assert_eq!(
            calendar.adjust_to_next_business_day(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()),
            (
                NaiveDate::from_ymd_opt(2026, 11, 3).unwrap(),
                DeadlineStatus::NonWorkingDayAdjusted
            )
        );
    }

    #[test]
    fn q1_1702q_2026_keeps_weekend_adjusted_june_first_deadline() {
        let deadlines = DeadlineResolver::resolve_taxable_year(2026);
        let q1_1702q = deadlines
            .iter()
            .find(|d| {
                d.form_code == "1702Q"
                    && d.period
                        == (DeadlinePeriod::Quarterly {
                            taxable_year: 2026,
                            quarter: 1,
                        })
            })
            .expect("1702Q Q1 missing");

        assert_eq!(
            q1_1702q.original_deadline_date(),
            Some(NaiveDate::from_ymd_opt(2026, 5, 30).unwrap())
        );
        assert_eq!(
            q1_1702q.final_deadline_date(),
            Some(NaiveDate::from_ymd_opt(2026, 6, 1).unwrap())
        );
        assert_eq!(q1_1702q.status, DeadlineStatus::WeekendAdjusted);
    }

    #[test]
    fn event_based_obligations_are_not_dated_deadlines() {
        let calendar = DeadlineResolver::resolve_deadline_calendar_year(2026);
        assert!(calendar.iter().all(|d| d.final_deadline_date().is_some()));

        let events = DeadlineResolver::event_based_obligations();
        assert!(events.iter().any(|d| d.form_code == "1800"));
        assert!(events.iter().all(|d| d.final_deadline_date().is_none()));
    }

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// Decision D8: every rule says what triggers it and what period a
    /// return covers, and the two agree with its frequency.
    #[test]
    fn every_rule_models_its_trigger_and_reporting_period() {
        for rule in DeadlineResolver::official_rules() {
            let expected = match rule.frequency {
                Frequency::Monthly => (FilingTrigger::PeriodEnd, ReportingPeriod::Monthly),
                Frequency::Quarterly => (FilingTrigger::PeriodEnd, ReportingPeriod::Quarterly),
                Frequency::Annual => (FilingTrigger::PeriodEnd, ReportingPeriod::Annual),
                Frequency::AsNeeded => (FilingTrigger::Event, ReportingPeriod::PerEvent),
            };
            assert_eq!(
                (rule.filing_trigger, rule.reporting_period),
                expected,
                "{:?}",
                rule.form_nos
            );
            assert_eq!(rule.is_as_needed(), rule.frequency == Frequency::AsNeeded);
        }
    }

    #[test]
    fn the_as_needed_codes_are_the_event_rules_codes() {
        let codes = DeadlineResolver::as_needed_form_codes();
        for code in [
            "0605", "1600WP", "1606", "1706", "1707", "1800", "1801", "2000", "2000OT", "2200A",
            "2200AN", "2200M", "2200P", "2200S", "2200T", "2552", "2553",
        ] {
            assert!(codes.contains(&code), "{code} is as needed");
        }
        for code in ["1601C", "2200C", "2551Q", "1701", "1604C"] {
            assert!(!DeadlineResolver::is_as_needed(code), "{code} is periodic");
        }
        // Every form with a trigger-date rule is an as-needed obligation
        // (1606 too, decision P4-3).
        for rule in EVENT_DUE_RULES {
            assert!(codes.contains(&rule.form_code), "{}", rule.form_code);
        }
    }

    /// Decision P4-3: 1606 is as needed, dated by the sale, due the 10th of
    /// the next month on the next working day; it has no monthly rows.
    #[test]
    fn form_1606_is_due_the_10th_after_the_sale_month() {
        for year in 2025..=2027 {
            let rows: Vec<_> = DeadlineResolver::resolve_taxable_year(year)
                .into_iter()
                .filter(|d| d.form_code == "1606")
                .collect();
            let monthly = rows
                .iter()
                .filter(|d| matches!(d.period, DeadlinePeriod::Monthly { .. }))
                .count();
            assert_eq!(monthly, 0, "no monthly 1606 row in {year}");
            assert_eq!(rows.len(), 1, "one event row in {year}");
            assert_eq!(rows[0].period, DeadlinePeriod::EventBased);
        }
        let rule = event_due_rule("1606").unwrap();
        assert_eq!(rule.trigger, "Sale");
        let calendar = BusinessDayCalendar::default();
        let due = |sale| {
            DeadlineResolver::event_deadline(&rule, sale, &[], &calendar)
                .unwrap()
                .final_deadline_date()
        };
        assert_eq!(due(ymd(2026, 8, 3)), Some(ymd(2026, 9, 10)));
        assert_eq!(due(ymd(2026, 8, 31)), Some(ymd(2026, 9, 10)));
        // October 10, 2026 is a Saturday.
        assert_eq!(due(ymd(2026, 9, 18)), Some(ymd(2026, 10, 12)));
        // December's sale: January 10, 2027 is a Sunday.
        assert_eq!(due(ymd(2026, 12, 2)), Some(ymd(2027, 1, 11)));
    }

    /// Decision D8: 2200M is due per removal, not quarterly on the 15th.
    #[test]
    fn form_2200m_is_as_needed_only() {
        for year in 2025..=2027 {
            assert!(
                DeadlineResolver::resolve_taxable_year(year)
                    .iter()
                    .filter(|d| d.form_code == "2200M")
                    .all(|d| d.final_deadline_date().is_none()),
                "no dated 2200M row in {year}"
            );
        }
        let events = DeadlineResolver::event_based_obligations();
        assert_eq!(events.iter().filter(|d| d.form_code == "2200M").count(), 1);
        assert_eq!(
            event_due_rule("2200M").map(|rule| rule.due),
            Some(DueFromTrigger::OnTheDate)
        );
    }

    /// 2200C: monthly, within 10 days after the month's close, on the next
    /// working day when that is a weekend.
    #[test]
    fn form_2200c_is_due_the_10th_of_the_next_month_and_rolls() {
        let deadlines = DeadlineResolver::resolve_taxable_year(2026);
        let month = |m: u8| {
            deadlines
                .iter()
                .find(|d| {
                    d.form_code == "2200C"
                        && d.period
                            == (DeadlinePeriod::Monthly {
                                taxable_year: 2026,
                                month: m,
                            })
                })
                .unwrap()
        };
        assert_eq!(month(1).final_deadline_date(), Some(ymd(2026, 2, 10)));
        // October 10, 2026 is a Saturday.
        assert_eq!(month(9).original_deadline_date(), Some(ymd(2026, 10, 10)));
        assert_eq!(month(9).final_deadline_date(), Some(ymd(2026, 10, 12)));
        assert_eq!(month(9).status, DeadlineStatus::WeekendAdjusted);
        assert_eq!(month(12).final_deadline_date(), Some(ymd(2027, 1, 11)));
        assert_eq!(event_due_rule("2200C"), None, "2200C is periodic");
    }

    /// 1600WP: within 20 days from the withholding date
    /// (html-frozen/1600wp-2010/guide.html), on the next working day.
    #[test]
    fn form_1600wp_is_due_20_days_after_the_withholding() {
        let rule = event_due_rule("1600WP").unwrap();
        let calendar = BusinessDayCalendar::default();
        let on =
            |trigger| DeadlineResolver::event_deadline(&rule, trigger, &[], &calendar).unwrap();
        let weekday = on(ymd(2026, 10, 1));
        assert_eq!(weekday.final_deadline_date(), Some(ymd(2026, 10, 21)));
        assert_eq!(weekday.status, DeadlineStatus::Normal);
        assert_eq!(weekday.period_start, Some(ymd(2026, 10, 1)));
        // October 25, 2026 is a Sunday.
        let sunday = on(ymd(2026, 10, 5));
        assert_eq!(sunday.original_deadline_date(), Some(ymd(2026, 10, 25)));
        assert_eq!(sunday.final_deadline_date(), Some(ymd(2026, 10, 26)));
        assert_eq!(sunday.status, DeadlineStatus::WeekendAdjusted);
        assert_eq!(sunday.period, DeadlinePeriod::EventBased);
    }

    #[test]
    fn the_trigger_date_rules_compute_each_form_s_due_date() {
        let calendar = BusinessDayCalendar::default();
        let due = |code: &str, trigger| {
            DeadlineResolver::event_deadline(
                &event_due_rule(code).unwrap(),
                trigger,
                &[],
                &calendar,
            )
            .unwrap()
        };
        // On removal, and a Saturday removal is not moved.
        let removal = due("2200A", ymd(2026, 10, 10));
        assert_eq!(removal.final_deadline_date(), Some(ymd(2026, 10, 10)));
        assert_eq!(removal.status, DeadlineStatus::Normal);
        for code in ["2200AN", "2200M", "2200P", "2200S", "2200T"] {
            assert_eq!(
                due(code, ymd(2026, 10, 7)).final_deadline_date(),
                Some(ymd(2026, 10, 7)),
                "{code}"
            );
        }
        assert_eq!(
            due("0605", ymd(2026, 10, 15)).final_deadline_date(),
            Some(ymd(2026, 10, 15))
        );
        assert_eq!(
            due("1606", ymd(2026, 9, 18)).final_deadline_date(),
            Some(ymd(2026, 10, 12)),
            "October 10 is a Saturday"
        );
        assert_eq!(
            due("1706", ymd(2026, 9, 1)).final_deadline_date(),
            Some(ymd(2026, 10, 1))
        );
        assert_eq!(
            due("1707", ymd(2026, 9, 1)).final_deadline_date(),
            Some(ymd(2026, 10, 1))
        );
        assert_eq!(
            due("2000OT", ymd(2026, 12, 3)).final_deadline_date(),
            Some(ymd(2027, 1, 5))
        );
        // Five banking days from Thursday October 8 skip the weekend.
        assert_eq!(
            due("2552", ymd(2026, 10, 8)).final_deadline_date(),
            Some(ymd(2026, 10, 15))
        );
        let primary = DeadlineResolver::event_deadline(
            &FORM_2552_PRIMARY_OFFERING_RULE,
            ymd(2026, 10, 8),
            &[],
            &calendar,
        )
        .unwrap();
        assert_eq!(primary.final_deadline_date(), Some(ymd(2026, 11, 9)));
        assert_eq!(
            due("1800", ymd(2026, 9, 15)).final_deadline_date(),
            Some(ymd(2026, 10, 15))
        );
        assert_eq!(
            due("1801", ymd(2024, 2, 29)).final_deadline_date(),
            Some(ymd(2025, 2, 28))
        );
    }

    #[test]
    fn banking_days_skip_the_working_calendar_s_holidays() {
        let calendar =
            BusinessDayCalendar::with_non_working_days(vec![NonWorkingDay::regular_holiday(
                ymd(2026, 10, 12),
                "Configured Monday holiday",
            )]);
        assert_eq!(
            DueFromTrigger::BankingDaysAfter(5).due_date(ymd(2026, 10, 8), &calendar),
            Some(ymd(2026, 10, 16))
        );
    }

    #[test]
    fn a_holiday_override_moves_a_rolling_event_deadline_but_not_a_removal() {
        let holiday = DeadlineOverride {
            id: "h".to_string(),
            title: "Configured holiday".to_string(),
            source_reference: "test".to_string(),
            affected_form_codes: vec!["1600WP".to_string(), "2200A".to_string()],
            original_deadline: ymd(2026, 10, 21),
            adjusted_deadline: ymd(2026, 10, 22),
            affected_regions: Vec::new(),
            affected_taxpayer_types: Vec::new(),
            effective_from: None,
            effective_until: None,
            expires_at: None,
        };
        let calendar = BusinessDayCalendar::default();
        let wp = DeadlineResolver::event_deadline(
            &event_due_rule("1600WP").unwrap(),
            ymd(2026, 10, 1),
            std::slice::from_ref(&holiday),
            &calendar,
        )
        .unwrap();
        assert_eq!(wp.final_deadline_date(), Some(ymd(2026, 10, 22)));
        assert_eq!(wp.status, DeadlineStatus::Extended);
        let removal = DeadlineResolver::event_deadline(
            &event_due_rule("2200A").unwrap(),
            ymd(2026, 10, 21),
            std::slice::from_ref(&holiday),
            &calendar,
        )
        .unwrap();
        assert_eq!(removal.final_deadline_date(), Some(ymd(2026, 10, 21)));
    }

    #[test]
    fn known_display_codes_canonicalize_for_app_routing() {
        assert_eq!(canonical_form_code("1601-C"), "1601C");
        assert_eq!(canonical_form_code("0619-E"), "0619E");
        assert_eq!(canonical_form_code("1701-MS"), "1701MS");
        assert_eq!(canonical_form_code("1702-RT"), "1702RT");
        assert_eq!(canonical_form_code("1600-PT"), "1600PT");
        assert_eq!(canonical_form_code("1600-VT"), "1600VT");
        assert_eq!(canonical_form_code("1602Q"), "1602Q");
        assert_eq!(canonical_form_code("1603Q"), "1603Q");
        assert_eq!(canonical_form_code("2000-OT"), "2000OT");
    }

    #[test]
    fn every_official_rule_has_canonical_form_code() {
        for rule in DeadlineResolver::official_rules() {
            for form_no in rule.form_nos {
                assert_ne!(
                    canonical_form_code(form_no),
                    "UNKNOWN",
                    "missing canonical mapping for {form_no}"
                );
            }
        }
    }

    #[test]
    fn override_extends_matching_deadline_with_source_reference() {
        let override_rule = DeadlineOverride {
            id: "test-extension".to_string(),
            title: "Test extension".to_string(),
            source_reference: "BIR test advisory".to_string(),
            affected_form_codes: vec!["1701Q".to_string()],
            original_deadline: NaiveDate::from_ymd_opt(2026, 5, 15).unwrap(),
            adjusted_deadline: NaiveDate::from_ymd_opt(2026, 6, 15).unwrap(),
            affected_regions: Vec::new(),
            affected_taxpayer_types: Vec::new(),
            effective_from: None,
            effective_until: None,
            expires_at: None,
        };

        let deadlines =
            DeadlineResolver::resolve_taxable_year_with_overrides(2026, &[override_rule]);
        let q1 = deadlines
            .iter()
            .find(|d| {
                d.form_code == "1701Q"
                    && d.period
                        == (DeadlinePeriod::Quarterly {
                            taxable_year: 2026,
                            quarter: 1,
                        })
            })
            .expect("1701Q Q1 missing");

        assert_eq!(
            q1.original_deadline_date(),
            Some(NaiveDate::from_ymd_opt(2026, 5, 15).unwrap())
        );
        assert_eq!(
            q1.final_deadline_date(),
            Some(NaiveDate::from_ymd_opt(2026, 6, 15).unwrap())
        );
        assert!(q1.matches_period_filter(&[], &[1]));
        assert!(q1.matches_period_filter(&[1, 2, 3], &[]));
        assert!(!q1.matches_period_filter(&[6], &[]));
        assert_eq!(q1.status, DeadlineStatus::Extended);
        assert_eq!(q1.source_reference.as_deref(), Some("BIR test advisory"));
    }

    #[test]
    fn override_adjusted_date_is_moved_to_next_business_day() {
        let calendar =
            BusinessDayCalendar::with_non_working_days(vec![NonWorkingDay::regular_holiday(
                NaiveDate::from_ymd_opt(2026, 6, 15).unwrap(),
                "Configured extension-day holiday",
            )]);
        let override_rule = DeadlineOverride {
            id: "test-extension-holiday".to_string(),
            title: "Test extension onto holiday".to_string(),
            source_reference: "BIR test advisory".to_string(),
            affected_form_codes: vec!["1701Q".to_string()],
            original_deadline: NaiveDate::from_ymd_opt(2026, 5, 15).unwrap(),
            adjusted_deadline: NaiveDate::from_ymd_opt(2026, 6, 15).unwrap(),
            affected_regions: Vec::new(),
            affected_taxpayer_types: Vec::new(),
            effective_from: None,
            effective_until: None,
            expires_at: None,
        };

        let deadlines = DeadlineResolver::resolve_taxable_year_with_overrides_and_calendar(
            2026,
            &[override_rule],
            &calendar,
        );
        let q1 = deadlines
            .iter()
            .find(|d| {
                d.form_code == "1701Q"
                    && d.period
                        == (DeadlinePeriod::Quarterly {
                            taxable_year: 2026,
                            quarter: 1,
                        })
            })
            .expect("1701Q Q1 missing");

        assert_eq!(
            q1.original_deadline_date(),
            Some(NaiveDate::from_ymd_opt(2026, 5, 15).unwrap())
        );
        assert_eq!(
            q1.final_deadline_date(),
            Some(NaiveDate::from_ymd_opt(2026, 6, 16).unwrap())
        );
        assert_eq!(q1.status, DeadlineStatus::Extended);
        assert_eq!(q1.source_reference.as_deref(), Some("BIR test advisory"));
    }

    /// The return to file now is the latest closed period, read off each
    /// form's own rule, across month, quarter and year boundaries.
    #[test]
    fn last_closed_period_follows_each_rule_across_year_ends() {
        let closed =
            |code: &str, today: NaiveDate| DeadlineResolver::last_closed_period(code, today);
        assert_eq!(
            closed("1601C", ymd(2026, 10, 26)),
            Some(DeadlinePeriod::Monthly {
                taxable_year: 2026,
                month: 9
            })
        );
        assert_eq!(
            closed("1601C", ymd(2026, 1, 1)),
            Some(DeadlinePeriod::Monthly {
                taxable_year: 2025,
                month: 12
            })
        );
        assert_eq!(
            closed("2551Q", ymd(2026, 10, 1)),
            Some(DeadlinePeriod::Quarterly {
                taxable_year: 2026,
                quarter: 3
            })
        );
        // The last day of a quarter has not closed it yet.
        assert_eq!(
            closed("2551Q", ymd(2026, 9, 30)),
            Some(DeadlinePeriod::Quarterly {
                taxable_year: 2026,
                quarter: 2
            })
        );
        assert_eq!(
            closed("2551Q", ymd(2027, 1, 5)),
            Some(DeadlinePeriod::Quarterly {
                taxable_year: 2026,
                quarter: 4
            })
        );
        // 1701Q has no fourth quarter: the annual 1701 covers it.
        assert_eq!(
            closed("1701Q", ymd(2027, 1, 5)),
            Some(DeadlinePeriod::Quarterly {
                taxable_year: 2026,
                quarter: 3
            })
        );
        assert_eq!(
            closed("1701", ymd(2026, 1, 1)),
            Some(DeadlinePeriod::Annual { taxable_year: 2025 })
        );
        assert_eq!(
            closed("1701", ymd(2026, 12, 31)),
            Some(DeadlinePeriod::Annual { taxable_year: 2025 })
        );
        assert_eq!(closed("2200A", ymd(2026, 10, 26)), None, "as needed");
        assert_eq!(closed("2550M", ymd(2026, 10, 26)), None, "no calendar rule");
        assert_eq!(
            DeadlineResolver::frequency_of("2551Q"),
            Some(Frequency::Quarterly)
        );
        assert_eq!(
            DeadlineResolver::frequency_of("0605"),
            Some(Frequency::AsNeeded)
        );
        assert_eq!(DeadlineResolver::frequency_of("2550M"), None);
    }
}
