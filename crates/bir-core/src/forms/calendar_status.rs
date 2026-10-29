//! Where a filing period sits on a profile's Calendar tab and the one status
//! its entry shows.
//!
//! A return shows the next thing to do, not its history: a filed return with
//! nothing left to pay shows its done state, and one that still owes tax shows
//! "Mark as paid" instead of "Submitted" plus a separate payment pill.
//!
//! Columns follow from the same facts:
//!
//! * **Upcoming**: nobody has touched the period and its deadline has not
//!   passed.
//! * **Action required**: a draft or queued return not yet filed, a period
//!   past its deadline and still not filed, or a filed return that still owes
//!   tax.
//! * **Neither**: a filed return with nothing left to pay, or a paid one.

use chrono::NaiveDate;

use super::FilingStatus;

/// Label of the chip on a filed return that still owes tax.
pub const MARK_AS_PAID_LABEL: &str = "Mark as paid";

/// Why a period sits under Action required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionReason {
    /// A draft that has not been queued, before its deadline.
    Draft,
    /// Queued for submission, before its deadline.
    Queued,
    /// Past its deadline and not filed (no return, a draft, or a queued one).
    Overdue,
    /// Filed, but the return's total amount payable is still unpaid.
    NeedsPayment,
}

/// Which Calendar column a filing period belongs to. Exactly one per period,
/// so a period can never be listed under two columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalendarPlacement {
    Upcoming,
    ActionRequired(ActionReason),
    /// Filed with nothing left to do: listed in no column.
    Done,
}

/// Whether a filed return still has to be paid.
///
/// `amount_payable` is the return's own computed total amount payable (see
/// [`amount_payable_from_json`]). Zero or an overpayment means nothing to
/// pay. An unknown amount keeps asking for payment, so a return whose total
/// cannot be read is never silently treated as settled.
pub fn payment_required(status: &FilingStatus, amount_payable: Option<f64>) -> bool {
    match status {
        FilingStatus::Sent | FilingStatus::Confirmed => match amount_payable {
            Some(amount) if amount.is_finite() => amount > 0.0,
            _ => true,
        },
        FilingStatus::Draft | FilingStatus::Queued | FilingStatus::Paid => false,
    }
}

/// The single status chip a return shows: its next action, or its done state.
pub fn status_chip_label(status: &FilingStatus, amount_payable: Option<f64>) -> &'static str {
    if payment_required(status, amount_payable) {
        return MARK_AS_PAID_LABEL;
    }
    match status {
        FilingStatus::Draft => "Draft",
        FilingStatus::Queued => "Queued",
        FilingStatus::Sent => "Sent",
        FilingStatus::Confirmed => "Confirmed",
        FilingStatus::Paid => "Paid",
    }
}

/// Place one filing period on the Calendar.
///
/// `status` is the most advanced return saved for the period (`None` when
/// there is none), `deadline` its final deadline (`None` for an event-based
/// deadline, which never lapses), and `today` the local date.
pub fn calendar_placement(
    status: Option<&FilingStatus>,
    amount_payable: Option<f64>,
    deadline: Option<NaiveDate>,
    today: NaiveDate,
) -> CalendarPlacement {
    let past_deadline = deadline.is_some_and(|deadline| deadline < today);
    match status {
        None if past_deadline => CalendarPlacement::ActionRequired(ActionReason::Overdue),
        None => CalendarPlacement::Upcoming,
        Some(FilingStatus::Draft | FilingStatus::Queued) if past_deadline => {
            CalendarPlacement::ActionRequired(ActionReason::Overdue)
        }
        Some(FilingStatus::Draft) => CalendarPlacement::ActionRequired(ActionReason::Draft),
        Some(FilingStatus::Queued) => CalendarPlacement::ActionRequired(ActionReason::Queued),
        Some(status) if payment_required(status, amount_payable) => {
            CalendarPlacement::ActionRequired(ActionReason::NeedsPayment)
        }
        Some(_) => CalendarPlacement::Done,
    }
}

/// The total amount payable a saved return computed for itself, read from its
/// stored `form_drafts.data_json`.
///
/// Each form keeps that total under its own item (1601-C Item 36, 2551Q Item
/// 24, ...). `None` when the form has no mapped total or the JSON does not
/// parse; callers then treat the return as still owing tax.
pub fn amount_payable_from_json(form_code: &str, data_json: &str) -> Option<f64> {
    use super::queueable::QueueableForm;
    use super::queueable::QueueableKind;

    match form_code {
        "1601C" => serde_json::from_str::<super::Form1601CDraft>(data_json)
            .ok()
            .map(|draft| draft.tax_36_total_amount_payable),
        "2551Q" => serde_json::from_str::<super::Form2551QDraft>(data_json)
            .ok()
            .map(|draft| draft.total_amount_payable),
        code => {
            let kind = QueueableKind::from_form_code(code)?;
            crate::with_queueable_kind!(kind, F => serde_json::from_str::<F>(data_json)
                .ok()
                .and_then(|draft| draft.amount_payable()))
        }
    }
}

/// The total penalties a saved, unfiled return would carry if filed (and
/// paid) on `filing_date`, computed the way its form computes them: the
/// period's deadline as the Calendar resolves it (`deadline_overrides` and
/// the profile's own applied), the profile's EOPT class, and the return's
/// per-component overrides. `None` when the form has no penalty items on the
/// shared model, the return has left Queued, or no deadline is known.
pub fn penalties_payable_from_json(
    form_code: &str,
    data_json: &str,
    profile: &crate::profile::TaxpayerProfile,
    deadline_overrides: &[crate::calendar_rules::DeadlineOverride],
    filing_date: NaiveDate,
) -> Option<f64> {
    use super::queueable::QueueableForm;
    use super::queueable::QueueableKind;

    match form_code {
        "1601C" => {
            let mut draft = serde_json::from_str::<super::Form1601CDraft>(data_json).ok()?;
            draft.apply_penalty_profile(profile, deadline_overrides);
            draft.penalties_payable_if_filed_on(filing_date)
        }
        code => {
            let kind = QueueableKind::from_form_code(code)?;
            crate::with_queueable_kind!(kind, F => {
                let mut draft = serde_json::from_str::<F>(data_json).ok()?;
                draft.apply_penalty_profile(profile, deadline_overrides);
                F::penalty_items()
                    .iter()
                    .filter_map(|items| items.payable_if_filed_on(&draft, filing_date))
                    .reduce(|total, party| total + party)
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    /// 1601-C September 2026 is due 10 October 2026.
    const SEPT_1601C_DEADLINE: (i32, u32, u32) = (2026, 10, 10);

    fn deadline() -> Option<NaiveDate> {
        let (y, m, d) = SEPT_1601C_DEADLINE;
        Some(day(y, m, d))
    }

    fn before_deadline() -> NaiveDate {
        day(2026, 10, 1)
    }

    fn after_deadline() -> NaiveDate {
        day(2026, 10, 12)
    }

    fn lab_1601c(withheld: f64) -> super::super::Form1601CDraft {
        let mut draft = crate::filing_queue::mandatory_lab_1601c_draft_for(2026, 9);
        draft.tin = "111-111-114-000".to_string();
        if withheld > 0.0 {
            draft.set_any_taxes_withheld(true);
            draft.tax_14_total_compensation = withheld * 10.0;
            draft.tax_25_total_taxes_withheld = withheld;
        }
        // Pin Items 32-34 at zero: the period is past its deadline, and this
        // test is not about penalties, so they must not grow with the clock.
        for component in crate::penalties::PenaltyComponent::ALL {
            draft.set_penalty_override(component, Some(0.0));
        }
        draft.compute();
        draft
    }

    #[test]
    fn zero_due_submitted_1601c_is_done_without_needs_payment() {
        let json = serde_json::to_string(&lab_1601c(0.0)).unwrap();
        let due = amount_payable_from_json("1601C", &json);
        assert_eq!(due, Some(0.0));

        let status = FilingStatus::Sent;
        assert!(!payment_required(&status, due));
        assert_eq!(status_chip_label(&status, due), "Sent");
        for today in [before_deadline(), after_deadline()] {
            assert_eq!(
                calendar_placement(Some(&status), due, deadline(), today),
                CalendarPlacement::Done,
                "a filed zero-due return is in neither Upcoming nor Action required"
            );
        }
    }

    #[test]
    fn zero_due_confirmed_return_shows_confirmed_and_is_done() {
        let status = FilingStatus::Confirmed;
        assert_eq!(status_chip_label(&status, Some(0.0)), "Confirmed");
        assert_eq!(
            calendar_placement(Some(&status), Some(0.0), deadline(), before_deadline()),
            CalendarPlacement::Done
        );
    }

    #[test]
    fn overpaid_filed_return_needs_no_payment() {
        let status = FilingStatus::Sent;
        assert!(!payment_required(&status, Some(-250.0)));
        assert_eq!(status_chip_label(&status, Some(-250.0)), "Sent");
    }

    #[test]
    fn submitted_unpaid_return_with_tax_due_shows_mark_as_paid_in_action_required() {
        let json = serde_json::to_string(&lab_1601c(1500.0)).unwrap();
        let due = amount_payable_from_json("1601C", &json);
        assert_eq!(due, Some(1500.0));

        for status in [FilingStatus::Sent, FilingStatus::Confirmed] {
            assert!(payment_required(&status, due));
            assert_eq!(status_chip_label(&status, due), MARK_AS_PAID_LABEL);
            for today in [before_deadline(), after_deadline()] {
                assert_eq!(
                    calendar_placement(Some(&status), due, deadline(), today),
                    CalendarPlacement::ActionRequired(ActionReason::NeedsPayment)
                );
            }
        }
    }

    #[test]
    fn unreadable_amount_keeps_asking_for_payment() {
        assert_eq!(amount_payable_from_json("1601C", "not json"), None);
        assert_eq!(
            status_chip_label(&FilingStatus::Sent, None),
            MARK_AS_PAID_LABEL
        );
        assert_eq!(
            calendar_placement(
                Some(&FilingStatus::Sent),
                None,
                deadline(),
                before_deadline()
            ),
            CalendarPlacement::ActionRequired(ActionReason::NeedsPayment)
        );
    }

    #[test]
    fn paid_return_is_done() {
        let status = FilingStatus::Paid;
        assert_eq!(status_chip_label(&status, Some(1500.0)), "Paid");
        assert_eq!(
            calendar_placement(Some(&status), Some(1500.0), deadline(), after_deadline()),
            CalendarPlacement::Done
        );
    }

    #[test]
    fn draft_is_action_required_not_upcoming() {
        assert_eq!(
            calendar_placement(
                Some(&FilingStatus::Draft),
                None,
                deadline(),
                before_deadline()
            ),
            CalendarPlacement::ActionRequired(ActionReason::Draft)
        );
        assert_eq!(status_chip_label(&FilingStatus::Draft, None), "Draft");
    }

    #[test]
    fn queued_return_is_action_required_not_upcoming() {
        assert_eq!(
            calendar_placement(
                Some(&FilingStatus::Queued),
                None,
                deadline(),
                before_deadline()
            ),
            CalendarPlacement::ActionRequired(ActionReason::Queued)
        );
    }

    #[test]
    fn past_deadline_unfiled_period_is_action_required_overdue() {
        for status in [None, Some(FilingStatus::Draft), Some(FilingStatus::Queued)] {
            assert_eq!(
                calendar_placement(status.as_ref(), None, deadline(), after_deadline()),
                CalendarPlacement::ActionRequired(ActionReason::Overdue),
                "{status:?}"
            );
        }
    }

    #[test]
    fn untouched_future_period_is_upcoming_only() {
        assert_eq!(
            calendar_placement(None, None, deadline(), before_deadline()),
            CalendarPlacement::Upcoming
        );
        // The deadline day itself is still upcoming.
        assert_eq!(
            calendar_placement(None, None, deadline(), deadline().unwrap()),
            CalendarPlacement::Upcoming
        );
    }

    #[test]
    fn only_untouched_periods_are_ever_upcoming() {
        let statuses = [
            Some(FilingStatus::Draft),
            Some(FilingStatus::Queued),
            Some(FilingStatus::Sent),
            Some(FilingStatus::Confirmed),
            Some(FilingStatus::Paid),
        ];
        for status in &statuses {
            for due in [None, Some(0.0), Some(1500.0)] {
                for today in [before_deadline(), after_deadline()] {
                    assert_ne!(
                        calendar_placement(status.as_ref(), due, deadline(), today),
                        CalendarPlacement::Upcoming,
                        "{status:?} {due:?} {today}"
                    );
                }
            }
        }
    }

    #[test]
    fn amount_payable_reads_2551q_item_24() {
        let profile = crate::filing_queue::mandatory_lab_1601c_profile();
        let draft = crate::forms::Form2551QDraft::new_from_profile(&profile, 2026, 3);
        let mut value = serde_json::to_value(draft).unwrap();
        value["total_amount_payable"] = serde_json::json!(1500.0);
        let json = value.to_string();
        assert_eq!(amount_payable_from_json("2551Q", &json), Some(1500.0));
    }

    #[test]
    fn amount_payable_reads_a_queueable_forms_own_total() {
        let profile = crate::filing_queue::mandatory_lab_1601c_profile();
        let draft = crate::forms::form_1601eq::Form1601EqDraft::new_from_profile(&profile, 2026, 2);
        let mut value = serde_json::to_value(draft).unwrap();
        value["total_amount_due"] = serde_json::json!(1500.0);
        assert_eq!(
            amount_payable_from_json("1601EQ", &value.to_string()),
            Some(1500.0)
        );
        value["total_amount_due"] = serde_json::json!(0.0);
        assert_eq!(
            amount_payable_from_json("1601EQ", &value.to_string()),
            Some(0.0)
        );
    }

    #[test]
    fn amount_payable_is_unknown_for_an_unmapped_form() {
        assert_eq!(amount_payable_from_json("NOPE", "{}"), None);
    }
}
