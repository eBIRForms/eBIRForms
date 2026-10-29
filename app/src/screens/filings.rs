//! Filings and the Send queue (desktop).
//!
//! Filings lists every return of the selected taxpayer, filtered (All,
//! Drafts, Sending, Sent) and searchable, with Print, Delete (a draft, after
//! the one confirm the app has) and Cancel queue. The Send queue is the
//! bir-core submission queue: each queued return with where it is (Queued,
//! Sending, Failed with the reason), Retry and Cancel, then what was sent.
//! The transport is not wired yet (P5): Sending and Failed are local state.

use bir_app_core::ReturnRow;
use nsh_core::prelude::*;

use crate::icons::Icon;
use crate::live::store;
use crate::screens::{Route, content, page_title, preview};
use crate::store::{FilingsFilter, Loaded, SendState};
use crate::theme;
use crate::widgets::{Variant, button, button_sm, chip, code_text, empty_row, list_group, list_row, name_cell, section_title, segmented, status_tone};

/// The selected branch's returns (tracked).
pub(crate) fn branch_rows() -> Vec<ReturnRow> {
    let st = store();
    match (st.returns.get(), st.selected.get()) {
        (Loaded::Ready(rows), Some(sel)) => rows.into_iter().filter(|r| r.tin == sel.tin).collect(),
        _ => Vec::new(),
    }
}

/// A return's status with its local send state (tracked).
pub(crate) fn shown_status(r: &ReturnRow) -> String {
    match store().send_state_of(r.id) {
        Some(SendState::Sending) => "Sending".to_owned(),
        Some(SendState::Failed(_)) => "Failed".to_owned(),
        None => r.status.clone(),
    }
}

pub(crate) fn screen(queue: bool) -> impl IntoElement {
    if queue { queue_page().into_element() } else { filings_page().into_element() }
}

fn filings_page() -> impl IntoElement {
    let st = store();
    let query = signal(String::new());
    let filters = [FilingsFilter::All, FilingsFilter::Drafts, FilingsFilter::Sending, FilingsFilter::Sent];
    let rows = move || {
        let q = query.get().trim().to_lowercase();
        let f = st.filings_filter.get();
        branch_rows()
            .into_iter()
            .filter(|r| f.keeps(&shown_status(r)))
            .filter(|r| q.is_empty() || format!("{} {}", r.form_code, r.period_label).to_lowercase().contains(&q))
            .collect::<Vec<_>>()
    };
    let rows2 = rows.clone();
    let search = nsh_base::input()
        .value(query)
        .on_change(move |v: &str| query.set(v.to_owned()))
        .placeholder("Find a return, like 1701Q or Q3")
        .placeholder_color(|| color(theme::FG3))
        .auto_correct(false)
        .w(px(240.0))
        .h(px(theme::CONTROL))
        .px(px(10.0))
        .text_size(px(13.0))
        .text_color(theme::FG)
        .bg(theme::SURFACE)
        .border_1()
        .border_color(theme::BORDER_STRONG)
        .rounded(px(theme::R_ROW));
    content(
        view()
            .gap(px(4.0))
            .child(page_title("Filings"))
            .child(text("Drafts, the send queue and sent returns. Sent returns stay exactly as sent.").text_size(px(13.0)).text_color(theme::FG2))
            .child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(12.0))
                    .pt(px(20.0))
                    .pb(px(12.0))
                    .child(segmented(
                        vec!["All".into(), "Drafts".into(), "Sending".into(), "Sent".into()],
                        move || filters.iter().position(|f| *f == st.filings_filter.get()).unwrap_or(0),
                        move |i| st.filings_filter.set(filters[i]),
                    ))
                    .child(view().flex_1())
                    .child(search),
            )
            .child(list_group().child(when(move || rows2().is_empty(), move || {
                empty_row(move || match st.filings_filter.get() {
                    FilingsFilter::Drafts => "No drafts. A return you open saves itself as a draft.",
                    FilingsFilter::Sending => "Nothing is waiting to send.",
                    FilingsFilter::Sent => "Sent returns show here.",
                    FilingsFilter::All => "No returns yet. Open a form under Form library; it saves itself here.",
                })
            })).child(each(rows, |r| (r.id, r.status.clone()), |r, index| filing_row(r.get_untracked(), index))))
            .child(when(move || st.confirm_delete.get().is_some(), delete_dialog)),
    )
}

/// A return with its actions: Print; Delete for a draft; Cancel queue for a
/// queued one.
fn filing_row(r: ReturnRow, index: ReadSignal<usize>) -> impl IntoElement {
    let st = store();
    let compact = move || window().width.0 < 700.0;
    let edited = crate::screens::steps::short_date(r.updated_at.get(..10).unwrap_or(&r.updated_at));
    let (open, print, del, cancel) = (r.clone(), r.clone(), r.clone(), r.clone());
    let status_row = r.clone();
    let status = move || shown_status(&status_row);
    let status2 = status.clone();
    let status_now = r.status.clone();
    let actions = move || {
        let (print, del, cancel) = (print.clone(), del.clone(), cancel.clone());
        let mut actions = view().flex_row().gap(px(4.0)).items_center();
        actions = actions.child(button_sm("Print", Variant::Ghost, Some(Icon::Printer), || false, move || preview::print_row(print.clone())));
        if status_now == "Draft" {
            actions = actions.child(button_sm("Delete", Variant::Ghost, None, || false, move || st.confirm_delete.set(Some(del.clone()))));
        }
        if status_now == "Queued" {
            actions = actions.child(button_sm("Cancel queue", Variant::Ghost, None, || false, move || st.cancel_queued(cancel.clone())));
        }
        actions
    };
    list_row(index)
        .cursor_pointer()
        .hover(|s| s.bg(theme::HOVER))
        .on_press(move || st.open_row(&open))
        .label(format!("{} · {}", r.form_code, r.period_label))
        .child(code_text(r.form_code.clone()))
        .child(name_cell(format!("{} · {}", r.form_code, r.period_label), r.branch_name.clone()))
        .child(when(move || !compact(), move || text(format!("Edited {edited}")).w(px(104.0)).text_size(px(12.0)).text_color(theme::FG2)))
        .child(each(move || vec![status()], |s| s.clone(), move |s, _| {
            let s = s.get_untracked();
            chip(s.clone(), status_tone(&s))
        }))
        .child(when(move || !compact() || status2() == "Draft", actions))
}

/// "Delete this draft?" over the window: the only confirm in the app.
pub(crate) fn delete_dialog() -> impl IntoElement {
    let st = store();
    let row = st.confirm_delete.get_untracked();
    let what = row.as_ref().map(|r| format!("{} · {}", r.form_code, r.period_label)).unwrap_or_default();
    let close = move || st.confirm_delete.set(None);
    view()
        .absolute()
        .inset(px(0.0))
        .z_index(40)
        .items_center()
        .justify_center()
        .child(view().absolute().inset(px(0.0)).bg(theme::SCRIM).on_press(close))
        .child(
            view()
                .w(px(380.0))
                .p(px(24.0))
                .gap(px(12.0))
                .rounded(px(theme::R_DIALOG))
                .bg(theme::SURFACE)
                .border_1()
                .border_color(theme::BORDER)
                .shadow_lg()
                .child(text("Delete this draft?").text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
                .child(text(format!("{what}. This can't be undone.")).text_size(px(13.0)).text_color(theme::FG2))
                .child(
                    view()
                        .flex_row()
                        .justify_end()
                        .gap(px(8.0))
                        .pt(px(8.0))
                        .child(button("Keep it", Variant::Secondary, || false, close))
                        .child(button("Delete draft", Variant::Primary, || false, move || {
                            if let Some(r) = st.confirm_delete.get_untracked() {
                                st.delete_draft(r);
                            }
                            st.confirm_delete.set(None);
                        })),
                ),
        )
}

fn queue_page() -> impl IntoElement {
    let st = store();
    let waiting = move || {
        let mut rows: Vec<ReturnRow> = match st.returns.get() {
            Loaded::Ready(rows) => rows.into_iter().filter(|r| r.status == "Queued").collect(),
            _ => Vec::new(),
        };
        rows.sort_by(|a, b| a.updated_at.cmp(&b.updated_at));
        rows
    };
    let sent = move || match st.returns.get() {
        Loaded::Ready(rows) => rows.into_iter().filter(|r| !matches!(r.status.as_str(), "Draft" | "Queued")).take(20).collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    let (waiting2, sent2) = (waiting.clone(), sent.clone());
    content(
        view()
            .gap(px(4.0))
            .child(page_title("Send queue"))
            .child(text("Queued returns send by themselves, oldest first, when you're online. Every taxpayer's returns are here.").text_size(px(13.0)).text_color(theme::FG2))
            .child(view().h(px(20.0)))
            .child(section_title("Waiting to send", true))
            .child(list_group().child(when(move || waiting2().is_empty(), move || {
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(12.0))
                    .pr(px(16.0))
                    .child(empty_row("Nothing is waiting. A checked return's Queue for sending puts it here.").flex_1())
                    .child(button_sm("Open Filings", Variant::Secondary, None, || false, move || st.go(Route::Filings)))
            })).child(each(waiting, |r| r.id, |r, index| queue_row(r.get_untracked(), index))))
            .child(section_title("Sent", false))
            .child(list_group().child(when(move || sent2().is_empty(), || empty_row("Sent returns show here, each with BIR's receipt."))).child(each(sent, |r| r.id, |r, index| crate::screens::home::return_row(r.get_untracked(), index)))),
    )
}

/// A queued return: where it is (and why it failed), Retry, Cancel.
fn queue_row(r: ReturnRow, index: ReadSignal<usize>) -> impl IntoElement {
    let st = store();
    let id = r.id;
    let state = move || st.send_state_of(id);
    let (open, cancel) = (r.clone(), r.clone());
    let line = {
        let who = format!("{} · queued {}", r.taxpayer_name, crate::screens::steps::short_date(r.updated_at.get(..10).unwrap_or(&r.updated_at)));
        move || match state() {
            Some(SendState::Failed(why)) => why,
            Some(SendState::Sending) => "Sending to BIR now…".to_owned(),
            None => format!("{who} · sends when you're online"),
        }
    };
    let failed = move || matches!(state(), Some(SendState::Failed(_)));
    let sending = move || matches!(state(), Some(SendState::Sending));
    list_row(index)
        .cursor_pointer()
        .hover(|s| s.bg(theme::HOVER))
        .on_press(move || st.open_row(&open))
        .label(format!("{} · {}", r.form_code, r.period_label))
        .child(code_text(r.form_code.clone()))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(text(format!("{} · {}", r.form_code, r.period_label)).text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(theme::FG).truncate())
                .child(text(line).text_size(px(12.0)).text_color(move || if failed() { theme::DANGER } else { theme::FG2 }).truncate()),
        )
        .child(each(move || vec![match state() {
            Some(SendState::Failed(_)) => "Failed",
            Some(SendState::Sending) => "Sending",
            None => "Queued",
        }], |s| *s, |s, _| {
            let label = s.get_untracked();
            chip(label, status_tone(label))
        }))
        .child(when(failed, move || button_sm("Retry", Variant::Secondary, None, || false, move || st.retry(id))))
        .child(when(move || !sending(), move || {
            let cancel = cancel.clone();
            button_sm("Cancel", Variant::Ghost, None, || false, move || st.cancel_queued(cancel.clone()))
        }))
}
