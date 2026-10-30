//! Calendar (desktop, bir-shell `calendar_page.js`): every deadline of the
//! forms the taxpayers file, from today through the next twelve months,
//! grouped by month, for the selected branch or every taxpayer. Returns
//! still open past their due date come first, each with the penalty filing
//! it today adds.

use bir_app_core::DeadlineRow;
use nsh_core::prelude::*;

use crate::live::store;
use crate::screens::home::{due_row, open_status};
use crate::screens::{content, page_title};
use crate::store::{Loaded, days_until};
use crate::theme;
use crate::widgets::{empty_row, list_group, section_title, segmented};

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// The deadlines the calendar lists (tracked): late and still open, then
/// today onwards; the selected branch's unless "All taxpayers".
pub(crate) fn calendar_rows(all: bool) -> Vec<DeadlineRow> {
    let st = store();
    let sel = st.selected.get();
    let mut rows: Vec<DeadlineRow> = match st.deadlines.get() {
        Loaded::Ready(rows) => rows
            .into_iter()
            .filter(|d| all || sel.as_ref().is_some_and(|s| s.tin == d.tin))
            .filter(|d| days_until(&d.due_date) >= 0 || open_status(&d.status))
            .collect(),
        _ => Vec::new(),
    };
    rows.sort_by(|a, b| a.due_date.cmp(&b.due_date));
    rows
}

/// `2026-11` as `November 2026`; late rows group as "Late".
pub(crate) fn month_of(d: &DeadlineRow) -> String {
    if days_until(&d.due_date) < 0 {
        return "Late".to_owned();
    }
    let m = d.due_date.get(5..7).and_then(|m| m.parse::<usize>().ok()).filter(|m| (1..=12).contains(m)).unwrap_or(1);
    format!("{} {}", MONTHS[m - 1], d.due_date.get(..4).unwrap_or(""))
}

pub(crate) fn screen() -> impl IntoElement {
    let st = store();
    let groups = move || {
        let mut out: Vec<(String, Vec<DeadlineRow>)> = Vec::new();
        for d in calendar_rows(st.calendar_all.get()) {
            let key = month_of(&d);
            match out.last_mut() {
                Some((k, list)) if *k == key => list.push(d),
                _ => out.push((key, vec![d])),
            }
        }
        out
    };
    let groups2 = groups.clone();
    let year = bir_app_core::display::current_year();
    content(
        view()
            .gap(px(4.0))
            .child(page_title("Calendar"))
            .child(text(format!("Every deadline for the forms your taxpayers file, from the {year} BIR tax calendar. Late returns show the penalty so far. Change a due date when BIR moves one; it changes for every taxpayer.")).text_size(px(13.0)).text_color(theme::FG2))
            .child(view().pt(px(20.0)).pb(px(8.0)).child(each(
                move || vec![st.selected.get().map(|s| s.name)],
                |n| n.clone(),
                move |name, _| {
                    let mine = name.get_untracked().unwrap_or_else(|| "This taxpayer".to_owned());
                    segmented(vec![mine, "All taxpayers".into()], move || usize::from(st.calendar_all.get()), move |i| st.calendar_all.set(i == 1))
                },
            )))
            .child(when(move || groups2().is_empty(), || list_group().child(empty_row("Deadlines show here once a taxpayer has forms for a tax year."))))
            .child(each(groups, |g| g.0.clone(), |g, index| {
                let label = g.get_untracked().0;
                let late = label == "Late";
                view()
                    .child(section_title(label, index.get_untracked() == 0).text_color(if late { theme::DANGER } else { theme::FG3 }))
                    .child(list_group().child(each(move || g.get().1, |d| format!("{}{}{}{}{:?}{}", d.tin, d.form_code, d.taxable_year, d.period, d.return_key, d.due_date), |d, i| due_row(d.get_untracked(), i, true))))
                    .into_element()
            })),
    )
}
