//! Change due date: move one deadline (a form and period) for every
//! taxpayer, with an optional note, or put it back to BIR's date. A dialog
//! on the desktop (typed MM/DD/YYYY), a bottom sheet on a phone (the phone
//! kit's date row and wheels). Edits save by themselves once the date is a
//! real one, like the rest of the app; Save saves now and closes.

use std::rc::Rc;
use std::time::Duration;

use bir_app_core::DeadlineRow;
use bir_app_core::display::{format_form_date, parse_typed_date};
use chrono::{Datelike, NaiveDate};
use nsh_base::input;
use nsh_core::prelude::*;

use crate::icons::{Icon, icon};
use crate::live::store;
use crate::store::{NoticeTone, task};
use crate::theme;
use crate::widgets::{CLEAR, Variant, button, caption, field_box};

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// `2026-11-23` as `Nov 23, 2026`.
fn long(date: NaiveDate) -> String {
    date.format("%b %-d, %Y").to_string()
}

fn iso(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()
}

/// The sheet's state: the deadline, what is typed, what was saved.
#[derive(Clone)]
struct Editor {
    row: Rc<DeadlineRow>,
    date: Signal<Option<NaiveDate>>,
    note: Signal<String>,
    /// An override of this deadline is stored.
    moved: Signal<bool>,
    /// The date and note as last saved.
    saved: Signal<Option<(NaiveDate, String)>>,
    /// The save line under the fields, and whether it is an error.
    line: Signal<(String, bool)>,
    seq: Signal<u64>,
}

impl Editor {
    fn new(row: DeadlineRow) -> Self {
        let date = iso(&row.due_date);
        let note = row.moved_reason.clone().filter(|r| r != bir_app_core::bir_core::deadline_overrides::DEFAULT_REASON).unwrap_or_default();
        let moved = row.is_moved();
        Self {
            saved: signal(if moved { date.map(|d| (d, note.clone())) } else { None }),
            date: signal(date),
            note: signal(note),
            moved: signal(moved),
            line: signal((String::new(), false)),
            seq: signal(0),
            row: Rc::new(row),
        }
    }

    /// The date and note differ from what is stored (or from BIR's date).
    fn pending(&self) -> Option<(NaiveDate, String)> {
        let date = self.date.get_untracked()?;
        let note = self.note.get_untracked().trim().to_owned();
        let now = (date, note);
        match self.saved.get_untracked() {
            Some(saved) if saved == now => None,
            None if iso(&self.row.due_date) == Some(now.0) && now.1.is_empty() => None,
            _ => Some(now),
        }
    }

    /// Save after typing pauses.
    fn schedule(&self) {
        let seq = self.seq.get_untracked() + 1;
        self.seq.set(seq);
        let ed = self.clone();
        store().owner.run(|| {
            set_timeout(Duration::from_millis(700), move || {
                if ed.seq.get_untracked() == seq && store().override_sheet.get_untracked().is_some() {
                    ed.save(false);
                }
            })
        });
    }

    /// Save what is pending; `close`: then close the sheet (Save, a tap
    /// outside). A date that is not a real one stays unsaved.
    fn save(&self, close: bool) {
        let st = store();
        self.seq.set(self.seq.get_untracked() + 1);
        if self.date.get_untracked().is_none() {
            self.line.set(("Type the date as MM/DD/YYYY, like 11/23/2026.".to_owned(), true));
            return;
        }
        let Some((date, note)) = self.pending() else {
            if close {
                st.override_sheet.set(None);
            }
            return;
        };
        self.line.set(("Saving…".to_owned(), false));
        let ed = self.clone();
        task(async move {
            let row = (*ed.row).clone();
            let what = format!("{} · {}", row.form_code, row.period_label);
            match st.move_deadline(row, date.to_string(), note.clone()).await {
                Ok(due) => {
                    ed.saved.set(Some((date, note)));
                    ed.moved.set(true);
                    let due = iso(&due).map(long).unwrap_or(due);
                    ed.line.set((format!("Saved. Due {due} for every taxpayer."), false));
                    if close {
                        st.override_sheet.set(None);
                        st.notify(NoticeTone::Success, format!("{what} is now due {due}, for every taxpayer."));
                    }
                }
                Err(why) => ed.line.set((why, true)),
            }
        });
    }

    fn restore(&self) {
        let st = store();
        self.seq.set(self.seq.get_untracked() + 1);
        st.override_sheet.set(None);
        st.restore_deadline((*self.row).clone());
    }

    /// What BIR's calendar says, and who the change reaches.
    fn lines(&self) -> (String, String) {
        let rule = self.row.original_due_date.as_deref().and_then(iso).map(long).unwrap_or_default();
        (format!("BIR's calendar date: {rule}."), format!("Applies to every taxpayer who files {}: the Calendar and the penalties follow it.", self.row.form_code))
    }
}

/// The sheet for the deadline in [`crate::store::Store::override_sheet`]:
/// re-made when it changes.
pub(crate) fn sheet() -> impl IntoElement {
    let st = store();
    each(
        move || vec![st.override_sheet.get().map(|r| format!("{}{}{}{}", r.tin, r.form_code, r.taxable_year, r.period))],
        |k| k.clone(),
        move |_, _| match st.override_sheet.get_untracked() {
            Some(row) if crate::mobile::phone() => phone_sheet(Editor::new(row)).into_element(),
            Some(row) => dialog(Editor::new(row)).into_element(),
            None => view().into_element(),
        },
    )
}

// ---------------------------------------------------------------- desktop

fn dialog(ed: Editor) -> impl IntoElement {
    let typed = signal(ed.date.get_untracked().map(format_form_date).unwrap_or_default());
    let (rule, reach) = ed.lines();
    let title = format!("{} · {}", ed.row.form_code, ed.row.period_label);
    let close = {
        let ed = ed.clone();
        move || ed.save(true)
    };
    let (e1, e2, e3, e4, e5, e6) = (ed.clone(), ed.clone(), ed.clone(), ed.clone(), ed.clone(), ed.clone());
    let date_box = field_box("MM/DD/YYYY")
        .value(typed)
        .label("New due date")
        .on_change(move |v: &str| {
            typed.set(v.to_owned());
            e1.date.set(parse_typed_date(v));
            if e1.date.get_untracked().is_some() {
                e1.line.set((String::new(), false));
                e1.schedule();
            }
        })
        .on_blur(move || {
            if e2.date.get_untracked().is_none() {
                e2.line.set(("Type the date as MM/DD/YYYY, like 11/23/2026.".to_owned(), true));
            }
        });
    let note_box = field_box("Why it moved, like an RMC number")
        .value(e3.note)
        .label("Note")
        .on_change(move |v: &str| {
            e3.note.set(v.to_owned());
            e3.schedule();
        });
    view()
        .absolute()
        .inset(px(0.0))
        .z_index(40)
        .items_center()
        .justify_center()
        .child(view().absolute().inset(px(0.0)).bg(theme::SCRIM).on_press(close))
        .child(
            view()
                .w(px(420.0))
                .p(px(24.0))
                .gap(px(12.0))
                .rounded(px(theme::R_DIALOG))
                .bg(theme::SURFACE)
                .border_1()
                .border_color(theme::BORDER)
                .shadow_lg()
                .child(
                    view()
                        .gap(px(4.0))
                        .child(text("Change due date").text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
                        .child(text(format!("{title}. {rule}")).text_size(px(13.0)).text_color(theme::FG2)),
                )
                .child(view().gap(px(6.0)).child(caption("New due date")).child(date_box))
                .child(view().gap(px(6.0)).child(caption("Note (optional)")).child(note_box))
                .child(text(reach).text_size(px(12.0)).text_color(theme::FG3))
                .child(when(move || !e4.line.get().0.is_empty(), move || {
                    let ed = e4.clone();
                    text(move || ed.line.get().0).text_size(px(12.0)).text_color(move || if ed.line.get().1 { theme::DANGER } else { theme::FG2 })
                }))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .pt(px(8.0))
                        .child(when(move || e5.moved.get(), {
                            let ed = e5.clone();
                            move || {
                                let ed = ed.clone();
                                button("Use BIR's date", Variant::Secondary, || false, move || ed.restore())
                            }
                        }))
                        .child(view().flex_1())
                        .child(button("Save", Variant::Primary, || false, move || e6.save(true))),
                ),
        )
}

// ---------------------------------------------------------------- phone

fn phone_sheet(ed: Editor) -> impl IntoElement {
    let (rule, reach) = ed.lines();
    let title = format!("{} · {}", ed.row.form_code, ed.row.period_label);
    let wheels = signal(false);
    let close = {
        let ed = ed.clone();
        move || crate::screens::taxpayer::close_after_blur({
            let ed = ed.clone();
            move || ed.save(true)
        })
    };
    let (e1, e2, e3, e4, e5, e6) = (ed.clone(), ed.clone(), ed.clone(), ed.clone(), ed.clone(), ed.clone());
    let shown = move || e1.date.get().map(long);
    let shown2 = shown.clone();
    let note_box = input()
        .value(e3.note)
        .on_change(move |v: &str| {
            e3.note.set(v.to_owned());
            e3.schedule();
        })
        .placeholder("Note (optional), like an RMC number")
        .placeholder_color(|| color(theme::FG3))
        .auto_correct(false)
        .label("Note")
        .h(px(48.0))
        .px(px(14.0))
        .text_size(px(16.0))
        .text_color(theme::FG)
        .bg(theme::HOVER)
        .rounded(px(12.0));
    let action = |label: &'static str, primary: bool, act: Box<dyn Fn()>| {
        view()
            .h(px(50.0))
            .rounded(px(14.0))
            .items_center()
            .justify_center()
            .bg(if primary { theme::ACCENT } else { theme::HOVER })
            .cursor_pointer()
            .label(label)
            .active(|s| s.opacity(0.7))
            .on_press(act)
            .child(text(label).text_size(px(16.0)).font_weight(FontWeight::BOLD).text_color(if primary { theme::PRIMARY_FG } else { theme::FG }))
    };
    let c2 = close.clone();
    view()
        .absolute()
        .inset(px(0.0))
        .z_index(40)
        .child(view().absolute().inset(px(0.0)).bg(theme::SCRIM).on_press(close))
        .child(
            view()
                .absolute()
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0))
                .max_h(move || px(window().height.0 * 0.9))
                .rounded_t(px(22.0))
                .bg(theme::SURFACE)
                .shadow_lg()
                .pl(move || px(safe_area().left.0))
                .pr(move || px(safe_area().right.0))
                .pb(move || px(safe_area().bottom.0 + 16.0))
                .child(view().w(px(36.0)).h(px(5.0)).rounded(px(99.0)).bg(theme::BORDER_STRONG).mt(px(8.0)).mb(px(6.0)).self_center().on_press(c2))
                .child(
                    scroll().child(
                        view()
                            .px(px(20.0))
                            .pt(px(6.0))
                            .gap(px(14.0))
                            .child(
                                view()
                                    .gap(px(2.0))
                                    .child(text("Change due date").text_size(px(20.0)).font_weight(FontWeight::BOLD).text_color(theme::FG))
                                    .child(text(title).text_size(px(14.0)).text_color(theme::FG2))
                                    .child(text(rule).text_size(px(13.0)).text_color(theme::FG3)),
                            )
                            // The kit's date row: the date in words, a tap
                            // opens the wheels.
                            .child(
                                view()
                                    .gap(px(6.0))
                                    .child(text("New due date").text_size(px(13.0)).text_color(theme::FG2))
                                    .child(
                                        view()
                                            .flex_row()
                                            .items_center()
                                            .gap(px(10.0))
                                            .h(px(48.0))
                                            .px(px(14.0))
                                            .rounded(px(12.0))
                                            .bg(theme::HOVER)
                                            .cursor_pointer()
                                            .label("Pick the date")
                                            .active(|s| s.opacity(0.6))
                                            .on_press(move || {
                                                dismiss_keyboard();
                                                wheels.update(|w| *w = !*w);
                                            })
                                            .child(text(move || shown().unwrap_or_else(|| "Choose a date".to_owned())).flex_1().text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(move || if shown2().is_some() { theme::FG } else { theme::FG3 }))
                                            .child(icon(Icon::Calendar, 18.0, || theme::FG3)),
                                    )
                                    .child(when(move || wheels.get(), move || date_wheels(e2.clone()))),
                            )
                            .child(note_box)
                            .child(text(reach).text_size(px(13.0)).text_color(theme::FG3))
                            .child(when(move || !e4.line.get().0.is_empty(), move || {
                                let ed = e4.clone();
                                text(move || ed.line.get().0).text_size(px(13.0)).text_color(move || if ed.line.get().1 { theme::DANGER } else { theme::FG2 })
                            }))
                            .child(action("Save", true, Box::new(move || e5.save(true))))
                            .child(when(move || e6.moved.get(), {
                                let ed = e6.clone();
                                move || {
                                    let ed = ed.clone();
                                    action("Use BIR's date", false, Box::new(move || ed.restore()))
                                }
                            })),
                    ),
                ),
        )
}

/// Month, day and year wheels under the date row; a pick that makes a real
/// date sets it and saves after a pause.
fn date_wheels(ed: Editor) -> impl IntoElement {
    let start = ed.date.get_untracked().unwrap_or_else(crate::store::today);
    let month = signal(start.month());
    let day = signal(start.day());
    let year = signal(start.year() as u32);
    let pick = {
        let ed = ed.clone();
        move || {
            if let Some(d) = NaiveDate::from_ymd_opt(year.get_untracked() as i32, month.get_untracked(), day.get_untracked()) {
                ed.date.set(Some(d));
                ed.line.set((String::new(), false));
                ed.schedule();
            } else {
                ed.line.set((format!("{} has no day {}.", MONTHS[month.get_untracked() as usize - 1], day.get_untracked()), true));
            }
        }
    };
    let column = move |items: Vec<(u32, String)>, sel: Signal<u32>, width: f32, pick: Rc<dyn Fn()>| {
        let mut col = view().py(px(8.0));
        for (v, label) in items {
            let pick = pick.clone();
            col = col.child(
                view()
                    .h(px(44.0))
                    .items_center()
                    .justify_center()
                    .rounded(px(10.0))
                    .bg(move || if sel.get() == v { theme::ACCENT_SOFT } else { CLEAR })
                    .cursor_pointer()
                    .on_press(move || {
                        sel.set(v);
                        pick();
                    })
                    .child(text(label).text_size(px(17.0)).font_weight(FontWeight::MEDIUM).text_color(move || if sel.get() == v { theme::ACCENT } else { theme::FG })),
            );
        }
        scroll().w(px(width)).h(px(176.0)).scrollbar(false).child(col)
    };
    let pick: Rc<dyn Fn()> = Rc::new(pick);
    let this_year = crate::store::today().year() as u32;
    let months: Vec<(u32, String)> = (1..=12).map(|m| (m, MONTHS[m as usize - 1].to_owned())).collect();
    let days: Vec<(u32, String)> = (1..=31).map(|d| (d, d.to_string())).collect();
    let years: Vec<(u32, String)> = (this_year - 1..=this_year + 2).map(|y| (y, y.to_string())).collect();
    view()
        .flex_row()
        .justify_center()
        .gap(px(8.0))
        .child(column(months, month, 150.0, pick.clone()))
        .child(column(days, day, 70.0, pick.clone()))
        .child(column(years, year, 90.0, pick))
}
