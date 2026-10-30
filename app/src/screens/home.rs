//! Home (desktop, bir-shell `home.js`): a greeting, the returns due in the
//! next 30 days (and late ones still open), the as-needed forms with Start a
//! return, and recently filed returns; the welcome steps until there is a
//! taxpayer. The deadline and return rows are shared with Calendar and
//! Filings.

use bir_app_core::{BirApp, DeadlineRow, OpenReturn, ReturnRow};
use nsh_core::prelude::*;

use crate::icons::Icon;
use crate::live::store;
use crate::screens::{Route, content, library, page_title, welcome};
use crate::store::{Db, Loaded, days_until, today};
use crate::theme;
use crate::widgets::{Tone, Variant, button_sm, chip, code_text, due_chip, empty_row, list_group, list_row, name_cell, peso, section_title, status_tone};

const MONTHS: [&str; 12] = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];

pub(crate) fn greeting() -> &'static str {
    use chrono::Timelike;
    match chrono::Local::now().hour() {
        0..=11 => "Good morning",
        12..=17 => "Good afternoon",
        _ => "Good evening",
    }
}

fn long_today() -> String {
    today().format("%A, %B %-d, %Y").to_string()
}

/// Whether a deadline's return is still to file.
pub(crate) fn open_status(status: &str) -> bool {
    matches!(status, "Not started" | "Draft")
}

/// The penalty filing `d` today adds, when it is late and still open.
pub(crate) fn late_penalty(d: &DeadlineRow) -> Option<f64> {
    if !open_status(&d.status) {
        return None;
    }
    let due = chrono::NaiveDate::parse_from_str(&d.due_date, "%Y-%m-%d").ok()?;
    BirApp::penalty_estimate(&d.form_code, due, today(), 0.0).map(|e| e.total())
}

pub(crate) fn open_deadline(d: &DeadlineRow) {
    let st = store();
    st.select(&d.tin);
    let open = OpenReturn { tin: d.tin.clone(), form_code: d.form_code.clone(), year: d.taxable_year, period: d.period, key: d.return_key };
    st.form.open(open, format!("{} · {}", d.form_code, d.period_label));
}

/// Who a row belongs to: the taxpayer, and the branch when it is not the
/// head office.
fn who(name: &str, branch: &str) -> String {
    if branch.is_empty() || branch == "Head office" { name.to_owned() } else { format!("{name} · {branch}") }
}

/// A deadline: date box, code, `<code> · <period>` over whose it is, when
/// (late with the penalty so far, due soon), status.
pub(crate) fn due_row(d: DeadlineRow, index: ReadSignal<usize>) -> impl IntoElement {
    let n = days_until(&d.due_date);
    let month = d.due_date.get(5..7).and_then(|m| m.parse::<usize>().ok()).filter(|m| (1..=12).contains(m)).unwrap_or(1);
    let day = d.due_date.get(8..10).and_then(|x| x.parse::<u32>().ok()).unwrap_or(1);
    let open = open_status(&d.status);
    let penalty = late_penalty(&d);
    let when_el = move || -> AnyElement {
        match (open, due_chip(n)) {
            // Late: the days and the penalty so far, in one chip.
            (true, Some(_)) if n < 0 => chip(format!("{} days late · ≈{}", -n, peso(penalty.unwrap_or(0.0))), Tone::Danger).into_element(),
            (true, Some(c)) => c.into_element(),
            _ => text(if n == 0 { "Today".to_owned() } else if n < 0 { format!("{} days ago", -n) } else { format!("In {n} days") }).text_size(px(12.0)).text_color(theme::FG2).into_element(),
        }
    };
    let compact = move || window().width.0 < 760.0;
    let row = d.clone();
    list_row(index)
        .cursor_pointer()
        .label(format!("{} · {}", d.form_code, d.period_label))
        .hover(|s| s.bg(theme::HOVER))
        .on_press(move || open_deadline(&row))
        .child(
            view()
                .w(px(44.0))
                .flex_shrink(0.0)
                .items_center()
                .child(text(MONTHS[month - 1]).text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG3))
                .child(text(day.to_string()).text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG)),
        )
        .child(code_text(d.form_code.clone()))
        .child(name_cell(format!("{} · {}", d.form_code, d.period_label), who(&d.taxpayer_name, &d.branch_name)))
        .child(when(move || !compact(), move || view().flex_row().child(when_el())))
        .child(view().w(px(88.0)).flex_row().justify_end().child(chip(d.status.clone(), status_tone(&d.status))))
}

/// A saved or filed return: code, name, edited date, status, Open.
pub(crate) fn return_row(r: ReturnRow, index: ReadSignal<usize>) -> impl IntoElement {
    let st = store();
    let compact = move || window().width.0 < 640.0;
    let row = r.clone();
    let row2 = r.clone();
    let status = crate::screens::filings::shown_status(&r);
    let edited = crate::screens::steps::short_date(r.updated_at.get(..10).unwrap_or(&r.updated_at));
    list_row(index)
        .cursor_pointer()
        .label(format!("{} · {}", r.form_code, r.period_label))
        .hover(|s| s.bg(theme::HOVER))
        .on_press(move || st.open_row(&row))
        .child(code_text(r.form_code.clone()))
        .child(name_cell(format!("{} · {}", r.form_code, r.period_label), who(&r.taxpayer_name, &r.branch_name)))
        .child(when(move || !compact(), move || text(format!("Edited {edited}")).w(px(104.0)).text_size(px(12.0)).text_color(theme::FG2)))
        .child(chip(status.clone(), status_tone(&status)))
        .child(button_sm("Open", Variant::Ghost, None, || false, move || st.open_row(&row2)))
}

pub(crate) fn screen() -> impl IntoElement {
    let st = store();
    content(
        view()
            .child(when(move || matches!(st.db.get(), Db::Failed(_)), library::db_error))
            .child(when(move || matches!(st.db.get(), Db::Ready(_)) && st.selected.get().is_none(), welcome))
            .child(when(move || st.selected.get().is_some(), dashboard)),
    )
}

fn dashboard() -> impl IntoElement {
    let st = store();
    let soon = move || match st.deadlines.get() {
        Loaded::Ready(rows) => {
            let mut rows: Vec<DeadlineRow> = rows
                .into_iter()
                .filter(|d| {
                    let n = days_until(&d.due_date);
                    n <= 30 && (n >= 0 || open_status(&d.status))
                })
                .collect();
            rows.sort_by(|a, b| a.due_date.cmp(&b.due_date));
            rows
        }
        _ => Vec::new(),
    };
    let soon2 = soon.clone();
    let soon3 = soon.clone();
    let lede = move || {
        let rows = soon3();
        let people = rows.iter().map(|d| d.tin.get(..9).unwrap_or("")).collect::<std::collections::BTreeSet<_>>().len();
        if rows.is_empty() {
            format!("Nothing is due in the next 30 days. {}.", long_today())
        } else {
            format!(
                "{} return{} due in the next 30 days across {people} taxpayer{}. {}.",
                rows.len(),
                if rows.len() == 1 { " is" } else { "s are" },
                if people == 1 { "" } else { "s" },
                long_today()
            )
        }
    };
    let needed = move || match st.as_needed.get() {
        Loaded::Ready(rows) => rows,
        _ => Vec::new(),
    };
    let needed2 = needed.clone();
    let filed = move || match st.returns.get() {
        Loaded::Ready(rows) => rows.into_iter().filter(|r| r.status != "Draft").take(3).collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    let filed2 = filed.clone();
    view()
        .gap(px(4.0))
        .child(page_title(greeting()))
        .child(text(lede).text_size(px(13.0)).text_color(theme::FG2))
        .child(view().h(px(20.0)))
        .child(section_title("Due in the next 30 days", true))
        .child(list_group().child(when(move || soon2().is_empty(), || empty_row("Deadlines show here once a taxpayer has forms for a tax year."))).child(each(
            soon,
            |d| format!("{}{}{}{}{:?}", d.tin, d.form_code, d.taxable_year, d.period, d.return_key),
            |d, index| due_row(d.get_untracked(), index),
        )))
        .child(when(move || !needed2().is_empty(), move || {
            let needed = needed.clone();
            view().child(section_title("As needed", false)).child(list_group().child(each(needed, |f| format!("{}{}", f.tin, f.form_code), |f, index| {
                let f = f.get_untracked();
                let code = f.form_code.clone();
                let tin = f.tin.clone();
                list_row(index)
                    .child(code_text(f.form_code.clone()))
                    .child(name_cell(format!("{} · {}", f.form_code, f.form_name), format!("{} · {}", f.taxpayer_name, f.due_rule)))
                    .child(button_sm("Start a return", Variant::Secondary, Some(Icon::Plus), || false, move || {
                        let st = store();
                        st.select(&tin);
                        st.library_query.set(code.clone());
                        st.library_pick.set(Some(code.clone()));
                        st.go(Route::Library);
                    }))
            })))
        }))
        .child(section_title("Recently filed", false))
        .child(list_group().child(when(move || filed2().is_empty(), || empty_row("Filed returns show here."))).child(each(filed, |r| r.id, |r, index| return_row(r.get_untracked(), index))))
}
