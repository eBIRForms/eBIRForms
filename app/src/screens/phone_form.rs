//! The return on a phone (D22, the approved "focused return" design): one
//! long scroll of sections with sticky headers, fields stacked (a label
//! row, then the value), computed rows on one line, the filer as one card
//! and a summary at the end. A floating bar shows the one next step
//! ([`dock`], shared with the desktop); while a box is focused a row above
//! the keyboard replaces it ([`accessory`]): ↑ ↓, the item, Done. ≡ opens
//! the outline ([`outline_sheet`], shared too).
//!
//! Safe areas: the nav bar pads the status bar / notch / island, the list
//! and the bars pad the sides (landscape notch), the bar and the sheets
//! clear the home indicator or the navigation bar, and the list's end
//! leaves room for the bar.

use std::sync::Arc;

use bir_app_core::LayoutItem;
use bir_app_core::display::{format_amount, period_label};
use nsh_core::prelude::*;

use super::form::{
    amount_font, filer_card, is_optional, item_message, jump_to, mono, save_line, session, step_focus,
};
use super::steps::{self, Mark, Step};
use crate::icons::{Icon, icon};
use crate::live::store;
use crate::session::{FormSession, ItemId, OpenForm};
use crate::theme;
use crate::widgets::CLEAR;

/// The keyboard row's height.
pub(crate) const ACCESSORY_H: f32 = 60.0;
/// The nav bar's height under the status bar.
pub(crate) const NAV_H: f32 = 44.0;
/// The floating bar's height.
const DOCK_H: f32 = 56.0;
/// The summary's place in the rows map (it has no item).
const SUMMARY: ItemId = (usize::MAX, usize::MAX);

pub(super) fn page(form: Arc<OpenForm>) -> impl IntoElement {
    let st = session();
    let list = element_ref();
    let content = element_ref();
    st.list.set(Some((list, content)));
    let typing = move || st.focused.get().is_some();
    // Debug builds: `BUWIZ_PICK=menu` opens the more menu (evidence.sh).
    let menu = signal(cfg!(debug_assertions) && std::env::var("BUWIZ_PICK").as_deref() == Ok("menu"));
    let parts_form = form.clone();
    let parts = move || {
        (0..parts_form.layout.parts.len())
            .filter(|pi| !steps::part_hidden(st, &parts_form, *pi) && (0..parts_form.layout.parts[*pi].items.len()).any(|ii| !steps::item_hidden(st, &parts_form, (*pi, ii))))
            .collect::<Vec<_>>()
    };
    let section_form = form.clone();
    view()
        .flex_1()
        .h_full()
        .bg(theme::SURFACE)
        .child(nav_bar(form.clone(), menu))
        .child(
            scroll()
                .element_ref(&list)
                .flex_1()
                .scrollbar(false)
                .keyboard_dismiss_mode(KeyboardDismissMode::Interactive).keyboard_taps(KeyboardTaps::Handled)
                // While typing, the keyboard row covers the list's end too.
                .content_inset(move || EdgeInsets { bottom: px(if typing() { ACCESSORY_H } else { 0.0 }), ..Default::default() })
                .child(
                    view()
                        .element_ref(&content)
                        .pl(move || px(safe_area().left.0))
                        .pr(move || px(safe_area().right.0))
                        .child(view().px(px(14.0)).pt(px(14.0)).pb(px(6.0)).gap(px(10.0)).child(when(move || st.amending.get(), super::form::amend_band)).child(filer_card(form.clone())).child(super::form::profile_banner()).child(when(super::form::can_unlock_profile, || {
                            text("Edit on this return")
                                .self_end()
                                .py(px(6.0))
                                .text_size(px(14.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme::ACCENT)
                                .label("Edit on this return")
                                .on_press(super::form::unlock_profile)
                        })))
                        .child(each(parts, |pi| *pi, move |pi, _| section(section_form.clone(), pi.get_untracked()).into_element()))
                        .child(summary(form.clone()))
                        // Room to scroll the last field above the bar.
                        .child(view().h(move || px(300.0 + DOCK_H + safe_area().bottom.0))),
                ),
        )
        .child(when(move || !typing(), {
            let form = form.clone();
            move || {
                view()
                    .absolute()
                    .left(move || px(12.0 + safe_area().left.0))
                    .right(move || px(12.0 + safe_area().right.0))
                    .bottom(move || px(dock_bottom()))
                    .child(dock(form.clone()))
            }
        }))
        .child(when(typing, {
            let form = form.clone();
            move || accessory(form.clone())
        }))
        .child(when(move || menu.get(), move || more_menu(menu)))
        .child(when(move || st.outline_open.get(), {
            let form = form.clone();
            move || outline_sheet(form.clone())
        }))
        .child(each(move || vec![st.picker.get()], |p| *p, {
            let form = form.clone();
            move |p, _| match p.get_untracked() {
                Some(id) => super::phone_fields::picker_sheet(form.clone(), id).into_element(),
                None => view().into_element(),
            }
        }))
        .child(each(move || vec![st.schedule_page.get()], |p| *p, {
            let form = form.clone();
            move |p, _| match p.get_untracked() {
                Some(id) => super::phone_fields::schedule_page(form.clone(), id).into_element(),
                None => view().into_element(),
            }
        }))
        .child(each(move || vec![st.row_edit.get()], |p| *p, {
            let form = form.clone();
            move |p, _| match p.get_untracked() {
                Some((id, r)) => super::phone_fields::row_editor(form.clone(), id, r).into_element(),
                None => view().into_element(),
            }
        }))
        .child(each(move || vec![st.payee.get()], |p| *p, {
            let form = form.clone();
            move |p, _| match p.get_untracked() {
                Some((id, row)) => super::phone_fields::payee_sheet(form.clone(), id, row).into_element(),
                None => view().into_element(),
            }
        }))
        .child(each(move || vec![st.explain.get()], |p| *p, {
            let form = form.clone();
            move |p, _| match p.get_untracked() {
                Some(id) => super::phone_fields::explain_sheet(form.clone(), id).into_element(),
                None => view().into_element(),
            }
        }))
}

/// The bar's distance from the bottom: above the home indicator or the
/// navigation bar, 16 where there is none.
fn dock_bottom() -> f32 {
    let inset = safe_area().bottom.0;
    if inset > 0.0 { inset + 4.0 } else { 16.0 }
}

fn nav_bar(form: Arc<OpenForm>, menu: Signal<bool>) -> impl IntoElement {
    let p = &form.params;
    let title = format!("{} · {}", p.form_code, period_label(&form.layout.frequency, p.year, p.period));
    view()
        .pt(move || px(safe_area().top.0))
        .pl(move || px(safe_area().left.0))
        .pr(move || px(safe_area().right.0))
        .bg(theme::SURFACE)
        .border_b(px(1.0))
        .border_color(theme::BORDER)
        .child(
            view()
                .h(px(NAV_H))
                .px(px(10.0))
                .flex_row()
                .items_center()
                .child(nav_icon(Icon::ChevronLeft, "Back", || store().back()))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .items_center()
                        .child(text(title).text_size(px(15.0)).font_weight(FontWeight::BOLD).text_color(theme::FG).truncate())
                        .child(text(save_line).text_size(px(11.5)).text_color(theme::FG2)),
                )
                .child(nav_icon(Icon::More, "More", move || menu.update(|m| *m = !*m))),
        )
}

/// A 36 pt accent icon button of a nav bar.
pub(crate) fn nav_icon(i: Icon, label: &str, on_press: impl Fn() + 'static) -> View {
    view()
        .size(px(44.0))
        .items_center()
        .justify_center()
        .rounded(px(10.0))
        .cursor_pointer()
        .label(label.to_owned())
        .active(|s| s.opacity(0.5))
        .on_press(on_press)
        .child(icon(i, 20.0, || theme::ACCENT))
}

fn more_menu(menu: Signal<bool>) -> impl IntoElement {
    let st = session();
    let row = move |label: &'static str, i: Icon, act: fn()| {
        view()
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .px(px(10.0))
            .min_h(px(44.0))
            .rounded(px(9.0))
            .cursor_pointer()
            .active(|s| s.bg(theme::HOVER))
            .on_press(move || {
                menu.set(false);
                act();
            })
            .child(icon(i, 16.0, || theme::FG2))
            .child(text(label).text_size(px(14.0)).text_color(theme::FG))
    };
    view()
        .absolute()
        .inset(px(0.0))
        .child(view().absolute().inset(px(0.0)).on_press(move || menu.set(false)))
        .child(
            view()
                .absolute()
                .top(move || px(safe_area().top.0 + NAV_H))
                .right(move || px(12.0 + safe_area().right.0))
                .min_w(px(180.0))
                .p(px(6.0))
                .rounded(px(14.0))
                .bg(theme::RAISED)
                .border_1()
                .border_color(theme::BORDER)
                .shadow_lg()
                .child(row("Check the return", Icon::CircleCheck, || session().validate()))
                .child(row("Print", Icon::Printer, || session().print()))
                .child(row("Outline", Icon::List, || session().outline_open.set(true)))
                .child(when(move || st.status.get() == "Queued", move || row("Take off the queue", Icon::ArrowLeft, || session().cancel_queue())))
                .child(when(move || !st.editable.get() && !st.sample.get() && matches!(st.status.get().as_str(), "Sent" | "Submitted" | "Confirmed" | "Paid"), move || row("Amend", Icon::FileText, || session().amend()))),
        )
}

// ---------------------------------------------------------------- sections

fn section(form: Arc<OpenForm>, pi: usize) -> impl IntoElement {
    let st = session();
    let (tag, name) = steps::section_title(&form.layout.parts[pi].name, pi);
    let items_form = form.clone();
    let items = move || (0..items_form.layout.parts[pi].items.len()).map(|ii| (pi, ii)).filter(|id| !steps::item_hidden(st, &items_form, *id)).collect::<Vec<_>>();
    let at = element_ref();
    st.rows.borrow_mut().insert((pi, usize::MAX), at);
    view()
        .element_ref(&at)
        .child(section_head(tag, name))
        .child(view().pb(px(10.0)).child(each(items, |id| *id, move |id, index| item(form.clone(), id.get_untracked(), index).into_element())))
}

fn section_head(tag: String, name: String) -> impl IntoElement {
    view()
        .sticky()
        .top(px(0.0))
        .z_index(2)
        .px(px(18.0))
        .pt(px(14.0))
        .pb(px(8.0))
        .bg(theme::SURFACE)
        .child(text(tag.to_uppercase()).text_size(px(10.5)).font_family(mono()).font_weight(FontWeight::MEDIUM).text_color(theme::FG3))
        .child(text(name).text_size(px(19.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
}

/// The boxes of `item` that are drawn (switched-off ones are not).
fn shown_fields(st: &FormSession, item: &LayoutItem) -> Vec<bir_app_core::LayoutField> {
    item.fields.iter().filter(|f| steps::field_shown(st, &f.key)).cloned().collect()
}

fn item(form: Arc<OpenForm>, id: ItemId, index: ReadSignal<usize>) -> AnyElement {
    let st = session();
    let Some(item) = form.item(id).cloned() else { return view().into_element() };
    let all_computed = !item.is_schedule() && !item.fields.is_empty() && item.fields.iter().all(|f| f.computed);
    let at = element_ref();
    st.rows.borrow_mut().insert(id, at);
    let border = move || px(if index.get() == 0 { 0.0 } else { 1.0_f32 });
    if all_computed {
        return computed_row(item, at, border).into_element();
    }
    let keys = item.box_keys();
    let focused = move || st.focused.get().is_some_and(|k| keys.contains(&k));
    let focused2 = focused.clone();
    let message = item_message(form.clone(), id);
    let message2 = message.clone();
    let message3 = message.clone();
    let optional = is_optional(&item);
    let no = item.no.clone();
    // A long list says where its answer came from itself (variant D).
    let long_list = matches!(super::phone_fields::kind_of(&item, &item.fields), super::phone_fields::Kind::LongList | super::phone_fields::Kind::Place);
    let profile_keys = item.box_keys();
    let changed_keys = profile_keys.clone();
    let changed = move || super::form::profile_changed(&changed_keys);
    let lines_keys = profile_keys.clone();
    let shut_keys = profile_keys.clone();
    let unlock_keys = profile_keys.clone();
    let penalty_keys = profile_keys.clone();
    let from_profile = move || !long_list && !changed() && st.profile.with(|p| profile_keys.iter().any(|k| p.contains(k)));
    view()
        .element_ref(&at)
        .px(px(16.0))
        .py(px(12.0))
        .gap(px(2.0))
        .min_h(px(64.0))
        // A tap anywhere on the row types into its box.
        .on_press(move || super::form::focus_first(id))
        .border_t(border)
        .border_color(theme::BORDER)
        .bg(move || if focused() || st.flash.get() == Some(id) { theme::ACCENT_SOFT } else { theme::SURFACE })
        // The bar at the left edge: the focus, or an error.
        .child(
            view()
                .absolute()
                .left(px(0.0))
                .top(px(8.0))
                .bottom(px(8.0))
                .w(px(3.0))
                .rounded_r(px(3.0))
                .bg(move || if !message3().is_empty() { theme::DANGER } else if focused2() { theme::ACCENT } else { CLEAR }),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(text(steps::item_title(&item)).flex_1().min_w(px(0.0)).text_size(px(13.0)).text_color(theme::FG2))
                .child(when(optional, || text("Optional").text_size(px(11.0)).text_color(theme::FG3)))
                .child(text(no).text_size(px(10.5)).font_family(mono()).text_color(theme::FG3)),
        )
        .child(value_of(form.clone(), id, &item))
        .child(when(from_profile, || text("From your tax profile").text_size(px(12.0)).text_color(theme::FG3)))
        .child(when(move || super::form::profile_shut(&shut_keys), move || {
            let keys = unlock_keys.clone();
            text("Edit on this return")
                .self_start()
                .py(px(4.0))
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::ACCENT)
                .label("Edit on this return")
                .on_press(move || super::form::unlock_item_profile(&keys))
        }))
        .child(super::form::profile_lines(lines_keys, true))
        .child(when(move || super::form::penalty_opened(&penalty_keys), || text("You're entering this penalty yourself.").text_size(px(12.0)).text_color(theme::ACCENT)))
        .child(when(move || !message().is_empty(), move || {
            text(message2.clone()).text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(theme::DANGER)
        }))
        .into_element()
}

/// What a stacked field shows under its label: the control of its kind
/// ([`super::phone_fields`]), re-made when the boxes that are drawn change
/// (a choice switches some on or off).
fn value_of(form: Arc<OpenForm>, id: ItemId, item: &LayoutItem) -> AnyElement {
    let st = session();
    let item_c = item.clone();
    each(
        move || vec![shown_fields(st, &item_c).iter().map(|f| f.key.clone()).collect::<Vec<_>>()],
        |keys| keys.clone(),
        {
            let item = item.clone();
            move |_, _| {
                let shown = untracked(|| shown_fields(st, &item));
                view().pt(px(1.0)).child(super::phone_fields::control(&form, id, &item, shown)).into_element()
            }
        },
    )
    .into_element()
}

/// A computed item on one line: `= label`, its number, its value.
fn computed_row(item: LayoutItem, at: ElementRef, border: impl Fn() -> Points + 'static) -> impl IntoElement {
    let st = session();
    let strong = item.label.to_uppercase().contains("TAX DUE") || item.label.contains("Total");
    let fields = item.fields.clone();
    let part = st.form().and_then(|f| f.boxes.get(fields.first().map(|f| f.key.as_str()).unwrap_or("")).map(|b| b.item.0));
    let value = move || {
        let form = st.form();
        let shown: Vec<String> = fields
            .iter()
            .filter(|f| steps::field_shown(st, &f.key))
            .filter(|f| match (&form, part) {
                (Some(form), Some(part)) => steps::column_shown(st, form, part, &f.label),
                _ => true,
            })
            .map(|f| {
                let v = st.value(&f.key);
                if f.kind == "money" || v.trim().parse::<f64>().is_ok() {
                    if v.trim().is_empty() { "—".to_owned() } else { format_amount(&v, f.whole) }
                } else if v.trim().is_empty() {
                    "—".to_owned()
                } else {
                    v
                }
            })
            .collect();
        shown.join(" / ")
    };
    let id = st.form().and_then(|f| f.boxes.get(item.fields.first().map(|f| f.key.as_str()).unwrap_or("")).map(|b| b.item));
    view()
        .element_ref(&at)
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(18.0))
        .py(px(10.0))
        .min_h(px(super::phone_fields::TAP))
        .cursor_pointer()
        .label("How it's worked out")
        .active(|s| s.bg(theme::HOVER))
        .on_press(move || st.explain.set(id))
        .border_t(border)
        .border_color(theme::BORDER)
        .child(
            text(format!("= {}", steps::item_title(&item)))
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(13.0))
                .font_weight(if strong { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                .text_color(if strong { theme::FG } else { theme::FG2 })
                .number_of_lines(2u32),
        )
        .child(text(item.no.clone()).text_size(px(10.5)).font_family(mono()).text_color(theme::FG3))
        .child(
            text(value)
                .text_size(px(15.0))
                .font_family(amount_font())
                .tabular_nums()
                .font_weight(if strong { FontWeight::BOLD } else { FontWeight::MEDIUM })
                .text_color(theme::FG),
        )
}

// ---------------------------------------------------------------- summary

/// The amount still to pay (tracked): the form's amount payable box.
fn still_due(form: &OpenForm) -> Option<f64> {
    let key = steps::amount_due_key(form)?;
    steps::amount(&session().value(&key))
}

fn peso(n: f64) -> String {
    format!("₱ {}", format_amount(&format!("{:.2}", n.abs()), false))
}

fn summary(form: Arc<OpenForm>) -> impl IntoElement {
    let st = session();
    let at = element_ref();
    st.rows.borrow_mut().insert(SUMMARY, at);
    let due_form = form.clone();
    let due = move || still_due(&due_form);
    let due2 = due.clone();
    let lines = move || {
        let mut out: Vec<(String, String)> = Vec::new();
        let parties: Vec<_> = st.penalties.get().into_iter().filter(|p| p.applies).collect();
        for (n, label) in ["Surcharge", "Interest", "Compromise"].iter().enumerate() {
            let total: f64 = parties.iter().filter_map(|p| p.keys.get(n)).filter_map(|k| steps::amount(&st.value(k))).sum();
            out.push((label.to_string(), format_amount(&format!("{total:.2}"), false)));
        }
        out
    };
    let note = move || {
        if super::form::penalties_open() {
            return "You're entering the penalties yourself (an amount the RDO approved, say), in their items above.".to_owned();
        }
        let penalties: f64 = st.penalties.get().into_iter().filter(|p| p.applies).flat_map(|p| p.keys).filter_map(|k| steps::amount(&st.value(&k))).sum();
        match st.due.get() {
            Some(date) if penalties == 0.0 => format!("Due {}. No penalties if it's sent by then.", long_date(&date)),
            Some(date) => format!("Due {}. Penalties are worked out to today.", long_date(&date)),
            None if penalties == 0.0 => "No penalties so far.".to_owned(),
            None => "Penalties are worked out to today.".to_owned(),
        }
    };
    view()
        .element_ref(&at)
        .child(section_head("Summary".to_owned(), "What you pay".to_owned()))
        .child(
            view()
                .px(px(18.0))
                .pt(px(2.0))
                .pb(px(8.0))
                .child(text(move || due().map(peso).unwrap_or_else(|| "—".to_owned())).text_size(px(32.0)).font_weight(FontWeight::SEMIBOLD).tabular_nums().text_color(theme::FG))
                .child(text(move || if due2().is_some_and(|n| n < 0.0) { "Overpaid: carried to the next return" } else { "Tax still due" }).text_size(px(13.0)).text_color(theme::FG2)),
        )
        .child(each(lines, |l| l.0.clone(), |line, _| {
            let label = line.get_untracked().0;
            view()
                .flex_row()
                .justify_between()
                .px(px(18.0))
                .py(px(7.0))
                .border_t(px(1.0))
                .border_color(theme::BORDER)
                .child(text(label).text_size(px(13.0)).text_color(theme::FG2))
                .child(text(move || line.get().1).text_size(px(13.5)).font_family(mono()).tabular_nums().text_color(theme::FG))
        }))
        .child(text(note).mx(px(18.0)).mt(px(10.0)).text_size(px(12.5)).text_color(theme::FG2))
        .child(when(move || st.editable.get() && st.penalties.with(|p| p.iter().any(|p| p.applies)) && !super::form::penalties_open(), || {
            text("Edit penalties")
                .mx(px(18.0))
                .mt(px(6.0))
                .py(px(8.0))
                .self_start()
                .text_size(px(14.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::ACCENT)
                .label("Edit penalties")
                .on_press(super::form::unlock_penalties)
        }))
}

/// `2026-11-15` as `Nov 15, 2026`.
fn long_date(iso: &str) -> String {
    let year = iso.get(..4).unwrap_or("");
    format!("{}, {year}", steps::short_date(iso))
}

// ---------------------------------------------------------------- the bar

/// The floating bar: ≡ (the outline), then the one next step; a thin line
/// along it shows how much is filled.
pub(super) fn dock(form: Arc<OpenForm>) -> impl IntoElement {
    let st = session();
    let step_form = form.clone();
    let step = computed(move || steps::next_step(st, &step_form));
    let prog_form = form.clone();
    let progress = move || steps::progress(st, &prog_form);
    let amount_form = form.clone();
    let tone = move || match step.get() {
        Step::Fix { .. } => 1,
        Step::Fill { .. } => 0,
        Step::Queue => 2,
        Step::Done(s) if s == "Queued" => 3,
        Step::Done(_) => 0,
    };
    // Opaque: the bar floats over the form's rows.
    const QUEUED_BG: AdaptiveColor = theme::tone(theme::hsla(0.3977, 0.45, 0.93, 1.0), theme::hsla(0.397, 0.35, 0.14, 1.0));
    let bg = move || match tone() {
        2 => theme::ACCENT,
        3 => QUEUED_BG,
        _ => theme::FG,
    };
    let ink = move || match tone() {
        2 => theme::PRIMARY_FG,
        3 => theme::SUCCESS,
        _ => theme::SURFACE,
    };
    let words = move || -> (String, String) {
        if let Some(busy) = st.busy.get().filter(|b| *b != "Saving…") {
            return (String::new(), busy.to_owned());
        }
        match step.get() {
            Step::Fix { label, message, .. } => (format!("Fix · {label}"), message),
            Step::Fill { label, left, .. } => (format!("Next · {left} left to fill"), label),
            Step::Queue => {
                let due = still_due(&amount_form);
                let date = st.due.get().map(|d| steps::short_date(&d));
                let small = match (due, date) {
                    (Some(n), Some(d)) if n > 0.0 => format!("{} due {d}", peso(n).replace("₱ ", "₱")),
                    (Some(n), None) if n > 0.0 => format!("{} to pay", peso(n).replace("₱ ", "₱")),
                    _ => "Nothing to pay".to_owned(),
                };
                (small, "Queue for sending".to_owned())
            }
            Step::Done(status) if status == "Queued" => ("Sends by itself when you're online".to_owned(), "Queued".to_owned()),
            Step::Done(status) if status == "Sample" => ("A sample from the Form library".to_owned(), "Nothing here is saved".to_owned()),
            Step::Done(status) => ("Read-only".to_owned(), status),
        }
    };
    let words2 = words.clone();
    let words3 = words.clone();
    let act = move || match step.get_untracked() {
        Step::Fix { item, .. } | Step::Fill { item, .. } => jump_to(item, true),
        Step::Queue => st.queue(),
        Step::Done(_) => {}
    };
    let glyph = move || match step.get() {
        Step::Fix { .. } => Icon::Alert,
        Step::Fill { .. } => Icon::ArrowRight,
        Step::Queue => Icon::Send,
        Step::Done(_) => Icon::Check,
    };
    view()
        .h(px(DOCK_H))
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .p(px(6.0))
        .rounded(px(22.0))
        .bg(bg)
        .shadow_lg()
        .label("Next step")
        .child(
            view()
                .size(px(44.0))
                .rounded(px(16.0))
                .items_center()
                .justify_center()
                .flex_shrink(0.0)
                .bg(move || if tone() == 2 { theme::tone(theme::hsla(0.0, 0.0, 1.0, 0.18), theme::hsla(0.0, 0.0, 0.0, 0.14)) } else { theme::tone(theme::hsla(0.0, 0.0, 1.0, 0.12), theme::hsla(0.0, 0.0, 0.0, 0.08)) })
                .cursor_pointer()
                .label("Outline")
                .active(|s| s.opacity(0.6))
                .on_press(move || st.outline_open.set(true))
                .child(each(move || vec![tone()], |t| *t, move |_, _| icon(Icon::List, 18.0, ink))),
        )
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .pl(px(4.0))
                .pr(px(10.0))
                .h_full()
                .cursor_pointer()
                .active(|s| s.opacity(0.7))
                .on_press(act)
                .child(
                    view()
                        .size(px(30.0))
                        .rounded(px(theme::R_FULL))
                        .items_center()
                        .justify_center()
                        .flex_shrink(0.0)
                        .bg(move || match tone() {
                            1 => theme::DANGER,
                            2 => theme::tone(theme::hsla(0.0, 0.0, 1.0, 0.18), theme::hsla(0.0, 0.0, 0.0, 0.14)),
                            3 => theme::SUCCESS,
                            _ => theme::ACCENT,
                        })
                        .child(each(move || vec![(glyph(), tone())], |g| *g, move |g, _| {
                            let (gl, t) = g.get_untracked();
                            icon(gl, 15.0, move || if t == 2 { theme::PRIMARY_FG } else { theme::tone(theme::hsla(0.0, 0.0, 1.0, 1.0), theme::hsla(0.6667, 0.0667, 0.08, 1.0)) })
                        })),
                )
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(when(move || !words().0.is_empty(), move || {
                            let words = words2.clone();
                            text(move || words().0).text_size(px(11.0)).text_color(ink).opacity(0.7).truncate()
                        }))
                        .child(text(move || words3().1).text_size(px(14.5)).font_weight(FontWeight::BOLD).text_color(ink).truncate()),
                ),
        )
        // The progress line.
        .child(when(move || tone() < 2 && st.editable.get(), move || {
            view()
                .absolute()
                .left(px(22.0))
                .right(px(22.0))
                .bottom(px(3.0))
                .h(px(2.5))
                .rounded(px(99.0))
                .bg(theme::tone(theme::hsla(0.0, 0.0, 1.0, 0.16), theme::hsla(0.0, 0.0, 0.0, 0.12)))
                .overflow_hidden()
                .child(view().h_full().w({
                    let progress = progress.clone();
                    move || relative(progress())
                }).rounded(px(99.0)).bg(theme::SUCCESS))
        }))
}

// ---------------------------------------------------------------- keyboard row

/// Above the keyboard while a box is focused: ↑ ↓ to the previous and
/// next box, the item, Done.
fn accessory(form: Arc<OpenForm>) -> impl IntoElement {
    let st = session();
    let info = move || {
        let key = st.focused.get().unwrap_or_default();
        let Some(id) = form.boxes.get(&key).map(|b| b.item) else { return (String::new(), String::new()) };
        let Some(item) = form.item(id) else { return (String::new(), String::new()) };
        let optional = if is_optional(item) { " · optional" } else { "" };
        let no = if item.no.is_empty() { String::new() } else { format!("Item {}{optional}", item.no) };
        (no, steps::item_title(item))
    };
    let info2 = info.clone();
    let key_btn = move |i: Icon, label: &'static str, by: isize| {
        let can = move || {
            st.focused.get();
            step_focus(by, false)
        };
        let can2 = can.clone();
        view()
            .size(px(44.0))
            .rounded(px(10.0))
            .items_center()
            .justify_center()
            .cursor_pointer()
            .label(label)
            .opacity(move || if can() { 1.0_f32 } else { 0.3 })
            .on_press(move || {
                if can2() {
                    step_focus(by, true);
                }
            })
            .child(icon(i, 16.0, || theme::ACCENT))
    };
    view()
        .absolute()
        .left(px(0.0))
        .right(px(0.0))
        .bottom(move || px(keyboard_height().0.max(0.0)))
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .pt(px(8.0))
        .pb(move || px(8.0 + if keyboard_height().0 > 0.0 { 0.0 } else { safe_area().bottom.0 }))
        .pl(move || px(10.0 + safe_area().left.0))
        .pr(move || px(10.0 + safe_area().right.0))
        .bg(theme::HOVER)
        .border_t(px(1.0))
        .border_color(theme::BORDER)
        .child(key_btn(Icon::ChevronUp, "Previous field", -1))
        .child(key_btn(Icon::ChevronDown, "Next field", 1))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .px(px(4.0))
                .child(text(move || info().0).text_size(px(10.5)).font_family(mono()).text_color(theme::FG3))
                .child(text(move || info2().1).text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG).truncate()),
        )
        .child(
            view()
                .h(px(44.0))
                .px(px(12.0))
                .justify_center()
                .cursor_pointer()
                .label("Done")
                .active(|s| s.opacity(0.5))
                .on_press(dismiss_keyboard)
                .child(text("Done").text_size(px(14.0)).font_weight(FontWeight::BOLD).text_color(theme::ACCENT)),
        )
}

// ---------------------------------------------------------------- outline

fn mark_view(mark: Option<Mark>) -> impl IntoElement {
    let base = view().size(px(18.0)).rounded(px(theme::R_FULL)).items_center().justify_center().flex_shrink(0.0);
    match mark {
        Some(Mark::Done) => base.bg(theme::SUCCESS).child(icon(Icon::Check, 11.0, || theme::tone(theme::hsla(0.0, 0.0, 1.0, 1.0), theme::hsla(0.0, 0.0, 1.0, 1.0)))),
        Some(Mark::Fix) => base.bg(theme::DANGER).child(text("!").text_size(px(12.0)).font_weight(FontWeight::BOLD).text_color(theme::tone(theme::hsla(0.0, 0.0, 1.0, 1.0), theme::hsla(0.0, 0.0, 0.0, 1.0)))),
        Some(Mark::Left(_)) => base.border(px(1.8)).border_color(theme::BORDER_STRONG),
        None => base.bg(theme::ACCENT).child(icon(Icon::ArrowRight, 11.0, || theme::PRIMARY_FG)),
    }
}

/// The outline: the tax still due, then each section with one mark; a
/// press lands on it. A note says which parts are hidden and why.
pub(super) fn outline_sheet(form: Arc<OpenForm>) -> impl IntoElement {
    let st = session();
    let close = move || st.outline_open.set(false);
    let due_form = form.clone();
    let rows_form = form.clone();
    let rows = move || {
        let mut out: Vec<(usize, String, String, Option<Mark>)> = Vec::new();
        for pi in 0..rows_form.layout.parts.len() {
            if steps::part_hidden(st, &rows_form, pi) || !(0..rows_form.layout.parts[pi].items.len()).any(|ii| !steps::item_hidden(st, &rows_form, (pi, ii))) {
                continue;
            }
            let (tag, name) = steps::section_title(&rows_form.layout.parts[pi].name, pi);
            out.push((pi, name, tag, Some(steps::part_mark(st, &rows_form, pi))));
        }
        let ready = matches!(steps::next_step(st, &rows_form), Step::Queue | Step::Done(_));
        out.push((usize::MAX, "Summary".to_owned(), "Tax still due, penalties".to_owned(), if ready { None } else { Some(Mark::Left(0)) }));
        out
    };
    let hidden_form = form.clone();
    let hidden = move || {
        (0..hidden_form.layout.parts.len())
            .filter(|pi| steps::part_hidden(st, &hidden_form, *pi))
            .map(|pi| {
                let (tag, name) = steps::section_title(&hidden_form.layout.parts[pi].name, pi);
                let what = if tag.is_empty() { name } else { tag };
                let why = steps::hidden_reason(st, &hidden_form, pi).map(|w| lower_first(&w)).unwrap_or_else(|| "it does not apply".to_owned());
                format!("{what} is hidden: it {why}.")
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let hidden2 = hidden.clone();
    view()
        .absolute()
        .inset(px(0.0))
        .z_index(20)
        .child(view().absolute().inset(px(0.0)).bg(theme::SCRIM).on_press(close))
        .child(
            view()
                .absolute()
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0))
                .items_center()
                .pointer_events(PointerEvents::BoxNone)
                .child(
                    view()
                        .w_full()
                        .max_w(px(560.0))
                        .max_h(move || px(window().height.0 * 0.84))
                        .rounded_t(px(22.0))
                        .bg(theme::SURFACE)
                        .shadow_lg()
                        .pl(move || px(safe_area().left.0))
                        .pr(move || px(safe_area().right.0))
                        .pb(move || px(safe_area().bottom.0 + 16.0))
                        .child(view().w(px(36.0)).h(px(5.0)).rounded(px(99.0)).bg(theme::BORDER).mt(px(8.0)).mb(px(6.0)).self_center().on_press(close))
                        .child(
                            view()
                                .px(px(20.0))
                                .pt(px(2.0))
                                .pb(px(10.0))
                                .child(text(move || if still_due(&due_form).is_some_and(|n| n < 0.0) { "Overpaid" } else { "Tax still due" }).text_size(px(12.5)).text_color(theme::FG2))
                                .child(text(move || still_due(&form).map(peso).unwrap_or_else(|| "Fill in your income to see it".to_owned())).text_size(px(26.0)).font_weight(FontWeight::SEMIBOLD).tabular_nums().text_color(theme::FG)),
                        )
                        .child(scroll().flex_shrink(1.0).child(view().px(px(8.0)).child(each(rows, |r| r.0, move |row, _| {
                            let (pi, name, tag, mark) = row.get_untracked();
                            let tail = match mark {
                                Some(Mark::Done) => "Done".to_owned(),
                                Some(Mark::Left(0)) => String::new(),
                                Some(Mark::Left(n)) => format!("{n} left"),
                                Some(Mark::Fix) => "Fix".to_owned(),
                                None => "Ready".to_owned(),
                            };
                            let tail_color = match mark {
                                Some(Mark::Fix) => theme::DANGER,
                                None => theme::ACCENT,
                                _ => theme::FG2,
                            };
                            view()
                                .flex_row()
                                .items_center()
                                .gap(px(12.0))
                                .px(px(12.0))
                                .py(px(10.0))
                                .rounded(px(12.0))
                                .cursor_pointer()
                                .active(|s| s.bg(theme::HOVER))
                                .on_press(move || {
                                    st.outline_open.set(false);
                                    jump_to(if pi == usize::MAX { SUMMARY } else { (pi, usize::MAX) }, false);
                                })
                                .child(mark_view(mark))
                                .child(
                                    view()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .child(text(name).text_size(px(14.5)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG).truncate())
                                        .child(text(tag).text_size(px(11.5)).text_color(theme::FG2).truncate()),
                                )
                                .child(text(tail).text_size(px(12.5)).font_weight(if matches!(mark, Some(Mark::Fix) | None) { FontWeight::BOLD } else { FontWeight::NORMAL }).text_color(tail_color))
                        }))))
                        .child(when(move || !hidden().is_empty(), move || text(hidden2.clone()).mx(px(20.0)).mt(px(6.0)).mb(px(12.0)).text_size(px(12.0)).text_color(theme::FG2))),
                ),
        )
}

fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}
