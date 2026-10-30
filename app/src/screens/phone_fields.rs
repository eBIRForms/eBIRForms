//! The phone's field controls, one component per field kind (D22), so a
//! kind's variant can be swapped without touching the renderer
//! ([`super::phone_form`]). The kinds and their current variants:
//!
//! | Kind | Control |
//! |---|---|
//! | long list (ATC, treaty) | the current answer and Change, which opens a full-screen search sheet |
//! | place / RDO | the same search sheet over the place list |
//! | short choice | full-width option cards |
//! | yes / no | two big buttons; a follow-up item shows below once it applies |
//! | money | right-aligned, ₱, live commas, the decimal keypad ([`super::form`]'s box) |
//! | TIN | one box, dashes added as you type, the number pad |
//! | date / period | typed MM/DD/YYYY, with Today and Yesterday |
//! | checkbox | the whole row taps, the check on the right |
//! | schedule table | row cards, each opening a full-screen row editor, a total under them |
//! | computed | an `=` row that opens "how it's worked out" ([`explain_sheet`]) |
//!
//! Every tap target is at least 44 pt (48 dp on Android, where points are
//! dp). The desktop keeps its pills and drop-downs.

use std::sync::Arc;

use bir_app_core::display::{format_amount, format_form_date};
use bir_app_core::{FieldOption, FieldValue, LayoutField, LayoutItem};
use nsh_base::input;
use nsh_core::prelude::*;

use super::form::{choice_label, date_parts, enter_box, leave_box, mono, one_date, session, text_box};
use super::steps;
use crate::icons::{Icon, icon};
use crate::session::{ItemId, OpenForm};
use crate::theme;
use crate::widgets::CLEAR;

/// The least height of anything tapped.
pub(crate) const TAP: f32 = 48.0;

/// The kind of control an item's drawn boxes get on a phone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    LongList,
    Place,
    ShortChoice,
    YesNo,
    Money,
    Tin,
    Date,
    Checkbox,
    Schedule,
    Computed,
    /// Text, numbers, an email: the plain stacked box.
    Text,
    /// Several kinds in one item: each part on its own row.
    Mixed,
}

/// A list the wiring always searches (ATC, treaty, nature of payment).
fn long_list_label(label: &str) -> bool {
    let l = label.to_lowercase();
    l.contains("atc") || l.contains("alphanumeric tax code") || l.contains("treaty") || l.contains("nature of")
}

pub(crate) fn kind_of(item: &LayoutItem, shown: &[LayoutField]) -> Kind {
    if item.is_schedule() {
        return Kind::Schedule;
    }
    if !shown.is_empty() && shown.iter().all(|f| f.computed) {
        return Kind::Computed;
    }
    if item.one_date && date_parts(&item.fields).is_some() {
        return Kind::Date;
    }
    let tin = item.label.contains("TIN") || item.label.contains("Identification");
    if tin && (3..=5).contains(&shown.len()) && shown.iter().all(|f| f.kind == "text" || f.kind == "int") {
        return Kind::Tin;
    }
    let radios = shown.iter().filter(|f| f.kind == "radio").count();
    if radios > 0 && radios == shown.len() {
        let labels: Vec<String> = shown.iter().map(|f| choice_label(f).to_lowercase()).collect();
        return if radios == 2 && labels.iter().any(|l| l == "yes") && labels.iter().any(|l| l == "no") {
            Kind::YesNo
        } else {
            Kind::ShortChoice
        };
    }
    if shown.len() == 1 {
        let f = &shown[0];
        return match f.kind.as_str() {
            "sel" if item.label.contains("RDO") || f.label.contains("RDO") => Kind::Place,
            "sel" if long_list_label(&format!("{} {}", item.label, f.label)) || f.options.len() > 12 || f.options.is_empty() => Kind::LongList,
            "sel" => Kind::ShortChoice,
            "money" => Kind::Money,
            "date" => Kind::Date,
            "chk" => Kind::Checkbox,
            _ => Kind::Text,
        };
    }
    if !shown.is_empty() && shown.iter().all(|f| f.kind == "chk") {
        return Kind::Checkbox;
    }
    Kind::Mixed
}

/// The control of `item` (its drawn boxes `shown`) on a phone.
pub(crate) fn control(form: &Arc<OpenForm>, id: ItemId, item: &LayoutItem, shown: Vec<LayoutField>) -> AnyElement {
    match kind_of(item, &shown) {
        Kind::LongList | Kind::Place => choice_row(id, shown, true).into_element(),
        Kind::ShortChoice => choice_row(id, shown, false).into_element(),
        Kind::YesNo => yes_no(shown).into_element(),
        Kind::Money | Kind::Text => {
            let f = &shown[0];
            text_box(f.key.clone(), f.kind.clone(), f.whole, f.computed, 0.0, f.max_len, f.pool.is_some()).into_element()
        }
        Kind::Tin => view()
            .child(tin_box(shown))
            .child(
                view()
                    .h(px(40.0))
                    .justify_center()
                    .self_start()
                    .cursor_pointer()
                    .label("Pick a saved payee")
                    .on_press(move || {
                        dismiss_keyboard();
                        session().payee.set(Some((id, None)));
                    })
                    .child(text("Pick a saved payee").text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::ACCENT)),
            )
            .into_element(),
        Kind::Date => date_row(id, item, shown).into_element(),
        Kind::Checkbox => checkboxes(shown).into_element(),
        Kind::Schedule => schedule_row(id, item).into_element(),
        Kind::Computed => view().into_element(),
        Kind::Mixed => mixed(form, id, item, shown).into_element(),
    }
}

/// Several kinds in one item (a "specify" box beside an amount, a choice
/// with a box): each part stacked, the boxes with their own labels.
fn mixed(form: &Arc<OpenForm>, id: ItemId, item: &LayoutItem, shown: Vec<LayoutField>) -> impl IntoElement {
    let radios: Vec<_> = shown.iter().filter(|f| f.kind == "radio").cloned().collect();
    let mut col = view().gap(px(8.0));
    if !radios.is_empty() {
        col = col.child(control(form, id, item, radios));
    }
    for f in shown.into_iter().filter(|f| f.kind != "radio") {
        let one = vec![f.clone()];
        let inner = if f.kind == "sel" { choice_row(id, one, true).into_element() } else { control(form, id, item, one) };
        col = col.child(view().gap(px(1.0)).child(text(sub_label(&f.label)).text_size(px(12.0)).text_color(theme::FG3)).child(inner));
    }
    col
}

/// `A) Taxpayer/Filer` reads `Taxpayer/Filer`.
pub(crate) fn sub_label(label: &str) -> String {
    match label.split_once(") ") {
        Some((head, rest)) if head.len() <= 2 => rest.to_owned(),
        _ => label.to_owned(),
    }
}

// ---------------------------------------------------------------- choices

/// One answer of a choice: what it reads, whether it is picked, the edits
/// picking it writes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Choice {
    pub label: String,
    pub key: String,
    pub edits: Vec<(String, String)>,
    pub locked: bool,
    pub computed: bool,
}

/// The answers of a radio group: picking one ticks it and unticks the rest
/// of its group.
fn radio_choices(radios: &[LayoutField]) -> Vec<Choice> {
    radios
        .iter()
        .map(|f| {
            let mut edits = vec![(f.key.clone(), "true".to_owned())];
            for other in radios {
                if other.key != f.key && (other.group == f.group || f.group.is_none()) {
                    edits.push((other.key.clone(), "false".to_owned()));
                }
            }
            Choice { label: choice_label(f), key: f.key.clone(), edits, locked: false, computed: f.computed }
        })
        .collect()
}

/// The answers of a drop-down: the return's catalog, else the layout's.
fn select_choices(field: &LayoutField) -> Vec<Choice> {
    let st = session();
    let listed: Vec<FieldOption> = st.options.with(|o| o.get(&field.key).cloned()).filter(|o| !o.is_empty()).unwrap_or_else(|| field.options.clone());
    listed
        .into_iter()
        .filter(|o| !o.value.is_empty())
        .map(|o| Choice { label: o.label, key: field.key.clone(), edits: vec![(field.key.clone(), o.value)], locked: false, computed: field.computed })
        .collect()
}

/// Whether `choice` is the picked answer (tracked).
fn picked(choice: &Choice) -> bool {
    let st = session();
    if choice.edits.first().is_some_and(|(_, v)| v == "true") {
        return st.is_ticked(&choice.key);
    }
    let value = st.value(&choice.key);
    choice.edits.first().is_some_and(|(_, v)| *v == value)
}

fn pick(choice: &Choice) {
    let st = session();
    if st.is_locked(&choice.key, choice.computed) {
        return;
    }
    if picked(choice) && choice.edits.first().is_some_and(|(_, v)| v == "true") {
        return;
    }
    st.touch_key(&choice.key);
    st.choose(choice.edits.clone());
}

/// The choices of an item's drawn boxes: its radios, else its drop-down.
fn item_choices(shown: &[LayoutField]) -> Vec<Choice> {
    let radios: Vec<_> = shown.iter().filter(|f| f.kind == "radio").cloned().collect();
    if !radios.is_empty() {
        return radio_choices(&radios);
    }
    shown.iter().find(|f| f.kind == "sel").map(select_choices).unwrap_or_default()
}

/// Full-width option cards, the picked one outlined with a check (short
/// choices' variant C; the current pick is the row and sheet, D).
#[allow(dead_code)]
fn short_choice(item: &LayoutItem, shown: Vec<LayoutField>) -> impl IntoElement {
    let _ = item;
    let mut col = view().gap(px(8.0)).pt(px(4.0));
    for choice in untracked(|| item_choices(&shown)) {
        let c1 = choice.clone();
        let c2 = choice.clone();
        let c3 = choice.clone();
        let c4 = choice.clone();
        col = col.child(
            view()
                .min_h(px(TAP))
                .px(px(14.0))
                .py(px(10.0))
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .rounded(px(14.0))
                .border(move || px(if picked(&c1) { 1.5 } else { 1.0_f32 }))
                .border_color(move || if picked(&c2) { theme::ACCENT } else { theme::BORDER_STRONG })
                .bg(move || if picked(&c3) { theme::ACCENT_SOFT } else { theme::SURFACE })
                .cursor_pointer()
                .label(choice.label.clone())
                .active(|s| s.opacity(0.7))
                .on_press(move || pick(&c4))
                .child(text(choice.label.clone()).flex_1().min_w(px(0.0)).text_size(px(15.0)).font_weight(FontWeight::MEDIUM).text_color(theme::FG))
                .child(check_mark({
                    let c = choice.clone();
                    move || picked(&c)
                })),
        );
    }
    col
}

/// A round check, filled when `on`.
fn check_mark(on: impl Fn() -> bool + Clone + 'static) -> impl IntoElement {
    let on2 = on.clone();
    let on3 = on.clone();
    view()
        .size(px(22.0))
        .rounded(px(theme::R_FULL))
        .items_center()
        .justify_center()
        .flex_shrink(0.0)
        .border(px(1.8))
        .border_color(move || if on() { theme::ACCENT } else { theme::BORDER_STRONG })
        .bg(move || if on2() { theme::ACCENT } else { CLEAR })
        .child(when(on3, || icon(Icon::Check, 13.0, || theme::PRIMARY_FG)))
}

/// Two big buttons, Yes and No.
fn yes_no(shown: Vec<LayoutField>) -> impl IntoElement {
    let mut row = view().flex_row().gap(px(10.0)).pt(px(4.0));
    for choice in radio_choices(&shown) {
        let (c1, c2, c3, c4) = (choice.clone(), choice.clone(), choice.clone(), choice.clone());
        row = row.child(
            view()
                .flex_1()
                .h(px(TAP))
                .items_center()
                .justify_center()
                .rounded(px(14.0))
                .border(move || px(if picked(&c1) { 1.5 } else { 1.0_f32 }))
                .border_color(move || if picked(&c2) { theme::ACCENT } else { theme::BORDER_STRONG })
                .bg(move || if picked(&c3) { theme::ACCENT_SOFT } else { theme::SURFACE })
                .cursor_pointer()
                .label(choice.label.clone())
                .active(|s| s.opacity(0.7))
                .on_press(move || pick(&c4))
                .child(text(choice.label.clone()).text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG)),
        );
    }
    row
}

/// A row showing the current answer (or what the profile supplied); a tap
/// opens the answers: a full-screen search for a long list (`search`), a
/// bottom sheet for a short one.
fn choice_row(id: ItemId, shown: Vec<LayoutField>, search: bool) -> impl IntoElement {
    let st = session();
    let current = {
        let shown = shown.clone();
        move || item_choices(&shown).into_iter().find(picked).map(|c| c.label)
    };
    let current2 = current.clone();
    // A long list the tax profile filled: the value, where it came from,
    // and Change (long lists' variant D over the search sheet, A).
    let keys: Vec<String> = shown.iter().map(|f| f.key.clone()).collect();
    let from_profile = {
        let current = current.clone();
        move || search && current().is_some() && st.profile.with(|p| keys.iter().any(|k| p.contains(k)))
    };
    let from_profile2 = from_profile.clone();
    let current3 = current.clone();
    when(from_profile, move || {
        let current3 = current3.clone();
        view()
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .min_h(px(TAP))
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(text(move || current3().unwrap_or_default()).text_size(px(16.0)).text_color(theme::FG).number_of_lines(2u32))
                    .child(text("From your tax profile").text_size(px(12.0)).text_color(theme::FG3)),
            )
            .child(
                view()
                    .h(px(44.0))
                    .px(px(16.0))
                    .rounded(px(12.0))
                    .items_center()
                    .justify_center()
                    .bg(theme::ACCENT_SOFT)
                    .cursor_pointer()
                    .label("Change")
                    .active(|s| s.opacity(0.6))
                    .on_press(move || {
                        dismiss_keyboard();
                        st.picker.set(Some(id));
                    })
                    .child(text("Change").text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::ACCENT)),
            )
    })
    .otherwise(move || {
    let _ = &from_profile2;
    let current = current.clone();
    let current2 = current2.clone();
    view()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .min_h(px(TAP))
        .cursor_pointer()
        .label(if search { "Search the answers" } else { "Choose an answer" })
        .active(|s| s.opacity(0.6))
        .on_press(move || {
            dismiss_keyboard();
            st.picker.set(Some(id));
        })
        .child(
            text(move || current().unwrap_or_else(|| "Choose".to_owned()))
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(16.0))
                .text_color(move || if current2().is_some() { theme::FG } else { theme::FG3 })
                .number_of_lines(2u32),
        )
        .child(icon(if search { Icon::Search } else { Icon::ChevronDown }, 18.0, || theme::ACCENT))
    })
}

/// A short list's answers as a bottom sheet with each answer's help.
fn choice_sheet(item: LayoutItem, all: Vec<Choice>) -> impl IntoElement {
    let st = session();
    let close = move || st.picker.set(None);
    let mut list = view().px(px(8.0));
    for c in all {
        let (c2, c3) = (c.clone(), c.clone());
        list = list.child(
            view()
                .min_h(px(56.0))
                .px(px(12.0))
                .py(px(8.0))
                .flex_row()
                .items_center()
                .gap(px(12.0))
                .rounded(px(12.0))
                .cursor_pointer()
                .label(c.label.clone())
                .active(|s| s.bg(theme::HOVER))
                .on_press(move || {
                    pick(&c2);
                    st.picker.set(None);
                })
                .child(text(c.label.clone()).flex_1().min_w(px(0.0)).text_size(px(16.0)).text_color(theme::FG))
                .child(check_mark(move || picked(&c3))),
        );
    }
    view()
        .absolute()
        .inset(px(0.0))
        .z_index(30)
        .child(view().absolute().inset(px(0.0)).bg(theme::SCRIM).on_press(close))
        .child(
            view()
                .absolute()
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0))
                .max_h(move || px(window().height.0 * 0.8))
                .rounded_t(px(22.0))
                .bg(theme::SURFACE)
                .pl(move || px(safe_area().left.0))
                .pr(move || px(safe_area().right.0))
                .pb(move || px(safe_area().bottom.0 + 16.0))
                .child(view().w(px(36.0)).h(px(5.0)).rounded(px(99.0)).bg(theme::BORDER).mt(px(8.0)).mb(px(6.0)).self_center())
                .child(text(steps::item_title(&item)).mx(px(20.0)).mb(px(6.0)).text_size(px(18.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
                .child(when(item.note.is_some(), {
                    let note = item.note.clone().unwrap_or_default();
                    move || text(note.clone()).mx(px(20.0)).mb(px(6.0)).text_size(px(13.0)).text_color(theme::FG2)
                }))
                .child(scroll().flex_shrink(1.0).child(list)),
        )
}

/// A full-screen page over the form: ✕, a title, then `body`.
pub(crate) fn full_sheet(title: String, close: impl Fn() + 'static, body: impl IntoElement) -> impl IntoElement {
    view()
        .absolute()
        .inset(px(0.0))
        .z_index(30)
        .bg(theme::SURFACE)
        .child(
            view()
                .pt(move || px(safe_area().top.0))
                .pl(move || px(safe_area().left.0))
                .pr(move || px(safe_area().right.0))
                .border_b(px(1.0))
                .border_color(theme::BORDER)
                .child(
                    view()
                        .h(px(super::phone_form::NAV_H))
                        .px(px(6.0))
                        .flex_row()
                        .items_center()
                        .child(
                            view()
                                .size(px(44.0))
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .label("Close")
                                .active(|s| s.opacity(0.5))
                                .on_press(close)
                                .child(text("✕").text_size(px(18.0)).text_color(theme::ACCENT)),
                        )
                        .child(text(title).flex_1().text_center().text_size(px(16.0)).font_weight(FontWeight::BOLD).text_color(theme::FG).truncate())
                        .child(view().w(px(44.0))),
                ),
        )
        .child(
            view()
                .flex_1()
                .pl(move || px(safe_area().left.0))
                .pr(move || px(safe_area().right.0))
                .child(body),
        )
}

/// The search sheet of a long list or a place: one search over every
/// answer, a tap picks and closes.
pub(crate) fn picker_sheet(form: Arc<OpenForm>, id: ItemId) -> impl IntoElement {
    let st = session();
    let Some(item) = form.item(id).cloned() else { return view().into_element() };
    let shown: Vec<LayoutField> = item.fields.iter().filter(|f| steps::field_shown(st, &f.key)).cloned().collect();
    if kind_of(&item, &shown) == Kind::Date {
        return date_wheels(item, shown).into_element();
    }
    if kind_of(&item, &shown) == Kind::ShortChoice {
        let all = untracked(|| item_choices(&shown));
        return choice_sheet(item, all).into_element();
    }
    let query = signal(String::new());
    let all = untracked(|| item_choices(&shown));
    let item_label = item.label.clone();
    // Searching: matches by code or any word. Not searching: what is picked
    // ("Used before") first, then the codes in their groups.
    let filtered = move || -> Vec<(Option<String>, Option<Choice>)> {
        let q = query.get().trim().to_lowercase();
        if !q.is_empty() {
            let words: Vec<&str> = q.split_whitespace().collect();
            return all
                .iter()
                .filter(|c| {
                    let l = c.label.to_lowercase();
                    words.iter().all(|w| l.contains(w))
                })
                .map(|c| (None, Some(c.clone())))
                .collect();
        }
        let mut out: Vec<(Option<String>, Option<Choice>)> = Vec::new();
        let used: Vec<Choice> = all.iter().filter(|c| picked(c)).cloned().collect();
        if !used.is_empty() {
            out.push((Some("Used before".to_owned()), None));
            out.extend(used.into_iter().map(|c| (None, Some(c))));
        }
        let mut last = String::new();
        for c in &all {
            let group = code_group(&c.label);
            if let Some(g) = &group
                && *g != last
            {
                out.push((Some(g.clone()), None));
                last = g.clone();
            }
            out.push((None, Some(c.clone())));
        }
        out
    };
    let filtered_none = filtered.clone();
    let no_match = move || !query.get().trim().is_empty() && filtered_none().is_empty();
    let hint_label = item_label.to_lowercase();
    let where_to_find = if hint_label.contains("atc") || hint_label.contains("tax code") {
        "No code matches. The ATC is printed on the BIR 2307 you received."
    } else {
        "Nothing matches. Try fewer letters."
    };
    let close = move || crate::screens::taxpayer::close_after_blur(move || st.picker.set(None));
    let place = item.label.contains("RDO");
    let search = input()
        .value(query)
        .on_change(move |v: &str| query.set(v.to_owned()))
        .placeholder(if place { "City, province or RDO" } else { "Search" })
        .placeholder_color(|| color(theme::FG3))
        .auto_correct(false)
        .auto_focus(true)
        .h(px(44.0))
        .px(px(14.0))
        .text_size(px(16.0))
        .text_color(theme::FG)
        .bg(theme::HOVER)
        .rounded(px(12.0));
    full_sheet(
        steps::item_title(&item),
        close,
        view()
            .flex_1()
            .child(view().px(px(16.0)).py(px(10.0)).child(search))
            .child(
                scroll().flex_1().keyboard_dismiss_mode(KeyboardDismissMode::OnDrag).keyboard_taps(KeyboardTaps::Handled).child(
                    view()
                        .pb(move || px(keyboard_height().0.max(safe_area().bottom.0) + 16.0))
                        .child(when(no_match, move || text(where_to_find).mx(px(18.0)).mt(px(12.0)).text_size(px(13.0)).text_color(theme::FG3)))
                        .child(each(filtered, |(h, c)| format!("{h:?}{:?}", c.as_ref().map(|c| c.label.clone())), move |row, _| {
                            let (head, c) = row.get_untracked();
                            if let Some(head) = head {
                                return text(head.to_uppercase()).px(px(18.0)).pt(px(14.0)).pb(px(4.0)).text_size(px(11.0)).font_family(mono()).font_weight(FontWeight::MEDIUM).text_color(theme::FG3).into_element();
                            }
                            let Some(c) = c else { return view().into_element() };
                            let c2 = c.clone();
                            let c3 = c.clone();
                            view()
                                .min_h(px(56.0))
                                .px(px(18.0))
                                .py(px(10.0))
                                .flex_row()
                                .items_center()
                                .gap(px(10.0))
                                .border_b(px(1.0))
                                .border_color(theme::BORDER)
                                .cursor_pointer()
                                .active(|s| s.bg(theme::HOVER))
                                .on_press(move || {
                                    pick(&c2);
                                    crate::screens::taxpayer::close_after_blur(move || st.picker.set(None));
                                })
                                .child(text(c.label.clone()).flex_1().min_w(px(0.0)).text_size(px(15.0)).text_color(theme::FG))
                                .child(when(move || picked(&c3), || icon(Icon::Check, 18.0, || theme::ACCENT)))
                                .into_element()
                        })),
                ),
            ),
    )
    .into_element()
}

/// The group of a coded answer (`WI010 …`, `WC160 …`): its family by the
/// code's two letters; `None` for an answer with no code.
fn code_group(label: &str) -> Option<String> {
    let code: String = label.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    let letters: String = code.chars().take_while(char::is_ascii_alphabetic).collect();
    if code.len() < 4 || letters.len() != 2 || !code[2..].chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(
        match letters.as_str() {
            "WI" => "Paid to individuals",
            "WC" => "Paid to corporations",
            "WB" => "Business tax withheld",
            "WV" => "VAT withheld",
            "WG" => "Government money payments",
            "WF" => "Final tax, foreign payees",
            "WE" => "Expanded withholding",
            "II" => "Individual income tax",
            "IC" => "Corporate income tax",
            "PT" => "Percentage tax",
            "VT" | "VB" | "VS" => "Value-added tax",
            "MC" => "Miscellaneous",
            other => return Some(format!("Codes {other}")),
        }
        .to_owned(),
    )
}

// ---------------------------------------------------------------- TIN

/// A TIN as one box: digits only, dashes added as typed (3-3-3-5); each
/// part goes to its own box of the form.
pub(crate) fn tin_box(shown: Vec<LayoutField>) -> impl IntoElement {
    let st = session();
    let keys: Vec<String> = shown.iter().map(|f| f.key.clone()).collect();
    let computed = shown.iter().any(|f| f.computed);
    let joined = {
        let keys = keys.clone();
        move || keys.iter().map(|k| st.value(k)).filter(|v| !v.is_empty()).collect::<Vec<_>>().join("-")
    };
    let locked = {
        let keys = keys.clone();
        move || keys.iter().all(|k| st.is_locked(k, computed))
    };
    let joined2 = joined.clone();
    when(move || !locked(), move || {
        let keys = keys.clone();
        let typed = signal(untracked(&joined2));
        let at = element_ref();
        st.inputs.borrow_mut().insert(keys[0].clone(), at);
        let first = keys[0].clone();
        let blur_key = first.clone();
        let field = input()
            .element_ref(&at)
            .label("Taxpayer Identification Number")
            .value(typed)
            .keyboard_type(KeyboardType::Number)
            .placeholder("000-000-000-00000")
            .placeholder_color(|| color(theme::FG3))
            .on_change(move |v: &str| {
                let digits: String = v.chars().filter(char::is_ascii_digit).take(14).collect();
                let cuts = [3usize, 3, 3, 5];
                let mut parts = Vec::new();
                let mut rest = digits.as_str();
                for (i, n) in cuts.iter().enumerate().take(keys.len()) {
                    let n = if i + 1 == keys.len() { rest.len() } else { (*n).min(rest.len()) };
                    let (head, tail) = rest.split_at(n);
                    parts.push(head.to_owned());
                    rest = tail;
                }
                let shown = parts.iter().filter(|p| !p.is_empty()).cloned().collect::<Vec<_>>().join("-");
                let prev = typed.get_untracked();
                typed.set(shown.clone());
                if shown != v {
                    // The caret stays after the digit just typed.
                    let caret = crate::typing::caret_after(&prev, v, &shown, crate::typing::digit);
                    at.set_selection(caret, caret);
                }
                for (key, part) in keys.iter().zip(parts) {
                    st.edit(key.clone(), FieldValue::Text(part), false);
                }
            })
            .on_focus(move || enter_box(first.clone()))
            .on_blur(move || {
                st.touch_key(&blur_key);
                leave_box(blur_key.clone());
                crate::store::task(async move { st.flush().await });
            });
        if crate::mobile::phone() {
            field.h(px(44.0)).text_size(px(17.0)).font_family(mono()).text_color(theme::FG).bg(CLEAR)
        } else {
            // The desktop's field style; TINs use the UI face's tabular figures.
            field.w(px(200.0)).h(px(theme::CONTROL)).px(px(10.0)).text_size(px(13.0)).tabular_nums().text_color(theme::FG).bg(theme::SURFACE).border_1().border_color(theme::BORDER_STRONG).rounded(px(theme::R_ROW))
        }
    })
    .otherwise(move || {
        if crate::mobile::phone() {
            text(joined.clone()).h(px(44.0)).text_size(px(17.0)).font_family(mono()).text_color(theme::FG).into_element()
        } else {
            view().w(px(200.0)).min_h(px(theme::CONTROL)).px(px(10.0)).justify_center().rounded(px(theme::R_ROW)).bg(theme::BG).child(text(joined.clone()).text_size(px(13.0)).tabular_nums().text_color(theme::FG)).into_element()
        }
    })
}

// ---------------------------------------------------------------- dates

/// Today and yesterday where the app runs.
fn today(back: i64) -> chrono::NaiveDate {
    chrono::Local::now().date_naive() - chrono::Duration::days(back)
}

/// A date typed as MM/DD/YYYY, with Today and Yesterday under it (dates'
/// variant A; the current pick is the wheels, B).
#[allow(dead_code)]
fn date(item: &LayoutItem, shown: Vec<LayoutField>) -> impl IntoElement {
    let st = session();
    let parts = if item.one_date { date_parts(&item.fields) } else { None };
    let field = shown.iter().find(|f| f.kind == "date").cloned();
    let box_view: AnyElement = match (&parts, &field) {
        (Some(parts), _) => one_date(parts.clone()).into_element(),
        (None, Some(f)) => text_box(f.key.clone(), f.kind.clone(), f.whole, f.computed, 0.0, f.max_len, false).into_element(),
        _ => view().into_element(),
    };
    let write = move |d: chrono::NaiveDate| match (&parts, &field) {
        (Some(parts), _) => {
            let text = d.to_string();
            let mut it = text.split('-');
            let (y, m, dd) = (it.next().unwrap_or("").to_owned(), it.next().unwrap_or("").to_owned(), it.next().unwrap_or("").to_owned());
            let mut edits = vec![(parts.month.key.clone(), m), (parts.year.key.clone(), y)];
            if let Some(day) = &parts.day {
                edits.push((day.key.clone(), dd));
            }
            st.choose(edits);
        }
        (None, Some(f)) => {
            st.edit(f.key.clone(), FieldValue::Text(format_form_date(d)), true);
            st.reverted.update(|(n, keys)| {
                *n += 1;
                *keys = vec![f.key.clone()];
            });
        }
        _ => {}
    };
    let write2 = write.clone();
    let chip = |label: &'static str, act: Box<dyn Fn()>| {
        view()
            .h(px(44.0))
            .px(px(16.0))
            .flex_shrink(0.0)
            .rounded(px(22.0))
            .items_center()
            .justify_center()
            .bg(theme::HOVER)
            .cursor_pointer()
            .label(label)
            .active(|s| s.opacity(0.6))
            .on_press(act)
            .child(text(label).text_size(px(14.0)).font_weight(FontWeight::MEDIUM).text_color(theme::FG))
    };
    view()
        .gap(px(8.0))
        .pt(px(2.0))
        .child(box_view)
        .child(
            view()
                .flex_row()
                .gap(px(8.0))
                .child(chip("Today", Box::new(move || write(today(0)))))
                .child(chip("Yesterday", Box::new(move || write2(today(1))))),
        )
}

/// The date's boxes: a one-date item's month, day and year, else its date box.
fn date_keys(item: &LayoutItem, shown: &[LayoutField]) -> (Option<super::form::DateParts>, Option<LayoutField>) {
    let parts = if item.one_date { date_parts(&item.fields) } else { None };
    (parts, shown.iter().find(|f| f.kind == "date").cloned())
}

/// The date as it reads now, MM/DD/YYYY, if one is set (tracked).
fn date_value(parts: &Option<super::form::DateParts>, field: &Option<LayoutField>) -> Option<(u32, u32, i32)> {
    let st = session();
    match (parts, field) {
        (Some(p), _) => {
            let m = st.value(&p.month.key).trim().parse().ok()?;
            let d = p.day.as_ref().map_or(Some(1), |f| st.value(&f.key).trim().parse().ok())?;
            let y = st.value(&p.year.key).trim().parse().ok()?;
            Some((m, d, y))
        }
        (None, Some(f)) => {
            let date = bir_app_core::display::parse_typed_date(&st.value(&f.key))?;
            let text = date.to_string();
            let mut it = text.split('-');
            let y = it.next()?.parse().ok()?;
            let m = it.next()?.parse().ok()?;
            let d = it.next()?.parse().ok()?;
            Some((m, d, y))
        }
        _ => None,
    }
}

/// A date as a row (its value, MM/DD/YYYY); a tap opens the wheels.
fn date_row(id: ItemId, item: &LayoutItem, shown: Vec<LayoutField>) -> impl IntoElement {
    let st = session();
    let (parts, field) = date_keys(item, &shown);
    let shown_text = move || date_value(&parts, &field).map(|(m, d, y)| format!("{m:02}/{d:02}/{y}"));
    let shown2 = shown_text.clone();
    view()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .min_h(px(TAP))
        .cursor_pointer()
        .label("Pick the date")
        .active(|s| s.opacity(0.6))
        .on_press(move || {
            dismiss_keyboard();
            st.picker.set(Some(id));
        })
        .child(text(move || shown_text().unwrap_or_else(|| "MM/DD/YYYY".to_owned())).flex_1().text_size(px(17.0)).font_family(mono()).text_color(move || if shown2().is_some() { theme::FG } else { theme::FG3 }))
        .child(icon(Icon::Calendar, 18.0, || theme::ACCENT))
}

/// Month, day and year wheels in a bottom sheet, the value above them;
/// Done writes it.
fn date_wheels(item: LayoutItem, shown: Vec<LayoutField>) -> impl IntoElement {
    const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let st = session();
    let (parts, field) = date_keys(&item, &shown);
    let now = today(0);
    let (m0, d0, y0) = untracked(|| date_value(&parts, &field)).unwrap_or_else(|| {
        use chrono::Datelike;
        (now.month(), now.day(), now.year())
    });
    let month = signal(m0);
    let day = signal(d0);
    let year = signal(y0);
    let close = move || st.picker.set(None);
    let column = |items: Vec<(u32, String)>, sel: Signal<u32>, width: f32| {
        let mut col = view().py(px(60.0));
        for (v, label) in items {
            col = col.child(
                view()
                    .h(px(44.0))
                    .items_center()
                    .justify_center()
                    .rounded(px(10.0))
                    .bg(move || if sel.get() == v { theme::ACCENT_SOFT } else { CLEAR })
                    .cursor_pointer()
                    .on_press(move || sel.set(v))
                    .child(text(label).text_size(px(17.0)).font_weight(FontWeight::MEDIUM).text_color(move || if sel.get() == v { theme::ACCENT } else { theme::FG })),
            );
        }
        scroll().w(px(width)).h(px(220.0)).scrollbar(false).child(col)
    };
    let year_sig: Signal<u32> = signal(y0 as u32);
    effect(move || year.set(year_sig.get() as i32));
    let months: Vec<(u32, String)> = (1..=12).map(|m| (m, MONTHS[m as usize - 1].to_owned())).collect();
    let days: Vec<(u32, String)> = (1..=31).map(|d| (d, d.to_string())).collect();
    let this_year = now.year_ce().1;
    let years: Vec<(u32, String)> = (this_year.saturating_sub(100)..=this_year + 1).rev().map(|y| (y, y.to_string())).collect();
    let write = move || {
        let (m, d, y) = (month.get_untracked(), day.get_untracked(), year.get_untracked());
        let Some(date) = chrono::NaiveDate::from_ymd_opt(y, m, d) else { return };
        match (&parts, &field) {
            (Some(p), _) => {
                let mut edits = vec![(p.month.key.clone(), format!("{m:02}")), (p.year.key.clone(), y.to_string())];
                if let Some(dk) = &p.day {
                    edits.push((dk.key.clone(), format!("{d:02}")));
                }
                st.touch_key(&p.month.key);
                st.choose(edits);
            }
            (None, Some(f)) => {
                st.touch_key(&f.key);
                st.edit(f.key.clone(), FieldValue::Text(format_form_date(date)), true);
                st.reverted.update(|(n, keys)| {
                    *n += 1;
                    *keys = vec![f.key.clone()];
                });
            }
            _ => {}
        }
        st.picker.set(None);
    };
    use chrono::Datelike;
    view()
        .absolute()
        .inset(px(0.0))
        .z_index(30)
        .child(view().absolute().inset(px(0.0)).bg(theme::SCRIM).on_press(close))
        .child(
            view()
                .absolute()
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0))
                .rounded_t(px(22.0))
                .bg(theme::SURFACE)
                .pl(move || px(safe_area().left.0))
                .pr(move || px(safe_area().right.0))
                .pb(move || px(safe_area().bottom.0 + 16.0))
                .child(view().w(px(36.0)).h(px(5.0)).rounded(px(99.0)).bg(theme::BORDER).mt(px(8.0)).mb(px(6.0)).self_center())
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .px(px(20.0))
                        .child(
                            view()
                                .flex_1()
                                .child(text(steps::item_title(&item)).text_size(px(13.0)).text_color(theme::FG2))
                                .child(text(move || format!("{:02}/{:02}/{}", month.get(), day.get(), year.get())).text_size(px(24.0)).font_family(mono()).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG)),
                        )
                        .child(
                            view()
                                .h(px(44.0))
                                .px(px(18.0))
                                .rounded(px(12.0))
                                .items_center()
                                .justify_center()
                                .bg(theme::ACCENT)
                                .cursor_pointer()
                                .label("Done")
                                .on_press(write)
                                .child(text("Done").text_size(px(15.0)).font_weight(FontWeight::BOLD).text_color(theme::PRIMARY_FG)),
                        ),
                )
                .child(
                    view()
                        .flex_row()
                        .justify_center()
                        .gap(px(8.0))
                        .mt(px(8.0))
                        .child(column(months, month, 150.0))
                        .child(column(days, day, 70.0))
                        .child(column(years, year_sig, 90.0)),
                ),
        )
}

// ---------------------------------------------------------------- checkboxes

/// Each box a full row: its label, the check at the right.
fn checkboxes(shown: Vec<LayoutField>) -> impl IntoElement {
    let st = session();
    let mut col = view();
    for f in shown {
        let (k1, k2) = (f.key.clone(), f.key.clone());
        let computed = f.computed;
        col = col.child(
            view()
                .min_h(px(TAP))
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .cursor_pointer()
                .label(f.label.clone())
                .active(|s| s.opacity(0.6))
                .on_press(move || {
                    if !st.is_locked(&k1, computed) {
                        let next = if st.is_ticked(&k1) { "false" } else { "true" };
                        st.touch_key(&k1);
                        st.choose(vec![(k1.clone(), next.to_owned())]);
                    }
                })
                .child(text(if f.label.is_empty() { "Yes".to_owned() } else { choice_label(&f) }).flex_1().min_w(px(0.0)).text_size(px(15.0)).text_color(theme::FG))
                .child(
                    view()
                        .size(px(24.0))
                        .rounded(px(6.0))
                        .items_center()
                        .justify_center()
                        .border(px(1.8))
                        .border_color(move || if st.is_ticked(&k2) { theme::ACCENT } else { theme::BORDER_STRONG })
                        .bg({
                            let k = f.key.clone();
                            move || if st.is_ticked(&k) { theme::ACCENT } else { CLEAR }
                        })
                        .child(when(
                            {
                                let k = f.key.clone();
                                move || st.is_ticked(&k)
                            },
                            || icon(Icon::Check, 15.0, || theme::PRIMARY_FG),
                        )),
                ),
        );
    }
    col
}

// ---------------------------------------------------------------- schedules

/// The schedule's amount column: its last money column.
fn amount_col(item: &LayoutItem) -> Option<usize> {
    item.cols.iter().rposition(|c| c.kind == "money")
}

/// Whether row `r` of `item` has anything typed (tracked).
fn row_filled(item: &LayoutItem, r: usize) -> bool {
    let st = session();
    item.keys.get(r).is_some_and(|keys| {
        keys.iter().zip(item.cols.iter()).any(|(k, c)| !c.computed && c.kind != "hidden" && !crate::session::is_blank_value(&st.value(k)))
    })
}

/// The rows a schedule lists: those filled, then the empty ones "+ Add"
/// opened (tracked).
fn listed_rows(id: ItemId, item: &LayoutItem) -> Vec<usize> {
    let st = session();
    let added = st.added_rows.with(|a| a.get(&id).copied().unwrap_or(0));
    let mut out: Vec<usize> = (0..item.keys.len()).filter(|r| row_filled(item, *r)).collect();
    let mut extra = added;
    for r in 0..item.keys.len() {
        if extra == 0 {
            break;
        }
        if !out.contains(&r) {
            out.push(r);
            extra -= 1;
        }
    }
    out.sort();
    out
}

/// The sum of the amount column over `rows` (tracked).
fn schedule_total(item: &LayoutItem, rows: &[usize]) -> f64 {
    let st = session();
    let Some(ci) = amount_col(item) else { return 0.0 };
    rows.iter().filter_map(|r| item.keys.get(*r).and_then(|k| k.get(ci))).filter_map(|k| steps::amount(&st.value(k))).sum()
}

/// A row's name (its row label, else its first text column) and its detail
/// (the other columns, `WI010 · ₱120,000.00 at 5%`) (tracked).
fn row_words(item: &LayoutItem, r: usize) -> (String, String) {
    let st = session();
    let keys = item.keys.get(r).cloned().unwrap_or_default();
    let ci_amount = amount_col(item);
    let mut name = item.row_labels.as_ref().and_then(|l| l.rows.get(r).cloned()).unwrap_or_default();
    let mut detail: Vec<String> = Vec::new();
    for (ci, (k, c)) in keys.iter().zip(item.cols.iter()).enumerate() {
        if c.kind == "hidden" || Some(ci) == ci_amount {
            continue;
        }
        let v = st.value(k);
        if crate::session::is_blank_value(&v) {
            continue;
        }
        let shown = match c.kind.as_str() {
            "money" => format!("₱{}", format_amount(&v, c.whole)),
            "pct" => format!("{v}%"),
            _ => v,
        };
        if name.is_empty() && matches!(c.kind.as_str(), "text" | "sel") {
            name = shown;
        } else {
            detail.push(shown);
        }
    }
    if name.is_empty() {
        name = format!("Row {}", item.start.unwrap_or(1) as usize + r);
    }
    (name, detail.join(" · "))
}

/// A schedule in the form: one row with its row count and total; a tap
/// pushes the schedule page.
fn schedule_row(id: ItemId, item: &LayoutItem) -> impl IntoElement {
    let st = session();
    let item_c = item.clone();
    let summary = move || {
        let rows: Vec<usize> = (0..item_c.keys.len()).filter(|r| row_filled(&item_c, *r)).collect();
        let total = schedule_total(&item_c, &rows);
        let count = format!("{} row{}", rows.len(), if rows.len() == 1 { "" } else { "s" });
        (count, if amount_col(&item_c).is_some() { format!("₱ {}", format_amount(&format!("{total:.2}"), false)) } else { String::new() })
    };
    let summary2 = summary.clone();
    view()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .min_h(px(TAP))
        .cursor_pointer()
        .label("Open the schedule")
        .active(|s| s.opacity(0.6))
        .on_press(move || st.schedule_page.set(Some(id)))
        .child(text(move || summary().0).flex_1().text_size(px(16.0)).text_color(theme::FG))
        .child(text(move || summary2().1).flex_shrink(0.0).text_size(px(18.0)).font_family(mono()).tabular_nums().text_color(theme::FG))
        .child(icon(Icon::ChevronRight, 16.0, || theme::FG3))
}

/// A schedule's page: "+ Add" in the nav, the rows as one inset group (the
/// row count over it, the amount column's name at the right), the total
/// pinned above the bottom inset; a row opens the row page.
pub(crate) fn schedule_page(form: Arc<OpenForm>, id: ItemId) -> impl IntoElement {
    let st = session();
    let Some(item) = form.item(id).cloned() else { return view().into_element() };
    let amount_label = amount_col(&item).map(|ci| sub_label(&item.cols[ci].label).to_uppercase()).unwrap_or_default();
    let it = item.clone();
    let rows = move || listed_rows(id, &it);
    let rows2 = rows.clone();
    let rows_hint = rows.clone();
    let rows3 = rows.clone();
    let rows4 = rows.clone();
    let rows5 = rows.clone();
    let it2 = item.clone();
    let it3 = item.clone();
    let add = move || {
        st.added_rows.update(|a| *a.entry(id).or_insert(0) += 1);
        // The new row is the first empty one: open it.
        let next = (0..it3.keys.len()).find(|r| !row_filled(&it3, *r));
        if let Some(r) = untracked(|| next) {
            st.row_edit.set(Some((id, r)));
        }
    };
    let add2 = add.clone();
    let count_line = move || {
        let n = rows2().len();
        format!("{n} row{}", if n == 1 { "" } else { "s" })
    };
    let group = view()
        .mx(px(16.0))
        .rounded(px(12.0))
        .bg(theme::SURFACE)
        .border_1()
        .border_color(theme::BORDER)
        .overflow_hidden()
        .child(each(rows, |r| *r, move |r, index| {
            let r = r.get_untracked();
            let it = it2.clone();
            let it_amount = it2.clone();
            let words = move || row_words(&it, r);
            let words2 = words.clone();
            let amount = move || {
                amount_col(&it_amount)
                    .and_then(|ci| it_amount.keys.get(r).and_then(|k| k.get(ci)).cloned())
                    .map(|k| format!("₱{}", format_amount(&non_blank(&st.value(&k)), false)))
                    .unwrap_or_default()
            };
            let clear_item = it2.clone();
            let delete: std::rc::Rc<dyn Fn()> = std::rc::Rc::new(move || delete_row(id, &clear_item, r));
            view()
                .child(when(move || index.get() > 0, || view().h(px(1.0)).ml(px(16.0)).bg(theme::BORDER)))
                .child(crate::screens::phone::swipe_row(
                    -1000 - r as i64,
                    view()
                        .min_h(px(64.0))
                        .px(px(16.0))
                        .py(px(10.0))
                        .flex_row()
                        .items_center()
                        .gap(px(10.0))
                        .cursor_pointer()
                        .active(|s| s.bg(theme::HOVER))
                        .bg(theme::SURFACE)
                        .on_press(move || {
                            if crate::live::store().swiped.get_untracked().is_some() {
                                crate::live::store().swiped.set(None);
                            } else {
                                st.row_edit.set(Some((id, r)));
                            }
                        })
                        .child(
                            view()
                                .flex_1()
                                .min_w(px(0.0))
                                .child(text(move || words().0).text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG).truncate())
                                .child(text(move || words2().1).text_size(px(12.0)).font_family(mono()).text_color(theme::FG3).truncate()),
                        )
                        .child(text(amount).flex_shrink(0.0).text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).font_family(mono()).tabular_nums().text_color(theme::FG))
                        .child(icon(Icon::ChevronRight, 16.0, || theme::FG3)),
                    vec![("Delete", theme::DANGER, delete)],
                ))
        }));
    let it_total = item.clone();
    let total = move || {
        let rows = rows3();
        format!("₱ {}", format_amount(&format!("{:.2}", schedule_total(&it_total, &rows)), false))
    };
    let body = view()
        .flex_1()
        .bg(theme::BG)
        .child(
            scroll().flex_1().child(
                view()
                    .pt(px(16.0))
                    .pb(move || px(96.0 + safe_area().bottom.0))
                    .child(when(move || !rows4().is_empty(), {
                        let amount_label = amount_label.clone();
                        let count_line = count_line.clone();
                        move || {
                            view()
                                .flex_row()
                                .mx(px(32.0))
                                .mb(px(6.0))
                                .child(text(count_line.clone()).flex_1().text_size(px(12.0)).text_color(theme::FG3))
                                .child(text(amount_label.clone()).text_size(px(12.0)).text_color(theme::FG3))
                        }
                    }))
                    .child(group)
                    .child(when({
                        let rows = rows_hint.clone();
                        move || !rows().is_empty()
                    }, || text("Swipe a row left to delete it.").mx(px(32.0)).mt(px(8.0)).text_size(px(12.0)).text_color(theme::FG3)))
                    .child(when(move || rows5().is_empty(), move || {
                        let add = add2.clone();
                        view()
                            .items_center()
                            .gap(px(12.0))
                            .py(px(40.0))
                            .child(text("No rows yet").text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
                            .child(
                                view()
                                    .h(px(48.0))
                                    .px(px(20.0))
                                    .rounded(px(14.0))
                                    .items_center()
                                    .justify_center()
                                    .bg(theme::PRIMARY)
                                    .cursor_pointer()
                                    .label("Add a row")
                                    .on_press(move || add())
                                    .child(text("+ Add a row").text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::PRIMARY_FG)),
                            )
                    })),
            ),
        )
        // The total, pinned above the bottom inset; the amount never wraps.
        .child(when(amount_col(&item).is_some(), {
            let count_line = count_line.clone();
            let label = amount_col(&item).map(|ci| sub_label(&item.cols[ci].label)).unwrap_or_default();
            move || {
                let count_line = count_line.clone();
                let total = total.clone();
                view()
                    .absolute()
                    .left(px(0.0))
                    .right(px(0.0))
                    .bottom(px(0.0))
                    .pb(move || px(safe_area().bottom.0 + 12.0))
                    .pt(px(12.0))
                    .pl(move || px(16.0 + safe_area().left.0))
                    .pr(move || px(16.0 + safe_area().right.0))
                    .flex_row()
                    .items_center()
                    .gap(px(12.0))
                    .bg(theme::SURFACE)
                    .border_t(px(1.0))
                    .border_color(theme::BORDER)
                    .child(
                        view()
                            .flex_1()
                            .min_w(px(0.0))
                            .child(text(format!("Total {}", label.to_lowercase())).text_size(px(13.0)).text_color(theme::FG2).truncate())
                            .child(text(count_line).text_size(px(12.0)).text_color(theme::FG3).truncate()),
                    )
                    .child(text(total).flex_shrink(0.0).text_size(px(19.0)).font_weight(FontWeight::BOLD).font_family(mono()).tabular_nums().text_color(theme::FG))
            }
        }));
    view()
        .absolute()
        .inset(px(0.0))
        .z_index(28)
        .bg(theme::BG)
        .child(
            view()
                .pt(move || px(safe_area().top.0))
                .pl(move || px(safe_area().left.0))
                .pr(move || px(safe_area().right.0))
                .bg(theme::SURFACE)
                .border_b(px(1.0))
                .border_color(theme::BORDER)
                .child(
                    view()
                        .h(px(44.0))
                        .px(px(6.0))
                        .flex_row()
                        .items_center()
                        .child(super::phone_form::nav_icon(Icon::ChevronLeft, "Back", move || st.schedule_page.set(None)))
                        .child(text(steps::item_title(&item)).flex_1().text_center().text_size(px(16.0)).font_weight(FontWeight::BOLD).text_color(theme::FG).truncate())
                        .child(
                            view()
                                .h(px(44.0))
                                .px(px(10.0))
                                .justify_center()
                                .cursor_pointer()
                                .label("Add a row")
                                .on_press(move || add())
                                .child(text("+ Add").text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::ACCENT)),
                        ),
                ),
        )
        .child(view().flex_1().pl(move || px(safe_area().left.0)).pr(move || px(safe_area().right.0)).child(body))
        .into_element()
}

fn non_blank(text: &str) -> String {
    if text.trim().is_empty() { "0".to_owned() } else { text.to_owned() }
}

/// Clear row `r` of the schedule `id` (its boxes the filer fills).
fn delete_row(id: ItemId, item: &LayoutItem, r: usize) {
    let st = session();
    let edits: Vec<(String, String)> = item
        .keys
        .get(r)
        .map(|keys| keys.iter().zip(item.cols.iter()).filter(|(_, c)| !c.computed && c.kind != "hidden").map(|(k, _)| (k.clone(), String::new())).collect())
        .unwrap_or_default();
    st.choose(edits);
    st.added_rows.update(|a| {
        if let Some(n) = a.get_mut(&id) {
            *n = n.saturating_sub(1);
        }
    });
    crate::live::store().notify(crate::store::NoticeTone::Info, "Row deleted.");
}

/// A schedule column that holds someone's TIN (a payee, a seller).
fn tin_col(label: &str) -> bool {
    let l = label.to_uppercase();
    l.contains("TIN") || l.contains("TAXPAYER IDENTIFICATION")
}

/// Whether row `r` is complete enough to keep (tracked): every choice
/// picked, the amount above zero, the TIN and name of a payee typed.
fn row_valid(item: &LayoutItem, r: usize) -> bool {
    let st = session();
    let Some(keys) = item.keys.get(r) else { return false };
    if !row_filled(item, r) {
        return false;
    }
    let amount = amount_col(item);
    for (ci, (k, c)) in keys.iter().zip(item.cols.iter()).enumerate() {
        if c.computed || c.kind == "hidden" {
            continue;
        }
        let v = st.value(k);
        let blank = v.trim().is_empty() || (c.kind == "sel" && v.chars().all(|ch| ch == '0' || ch == '-'));
        if c.kind == "sel" && blank {
            return false;
        }
        if Some(ci) == amount && !steps::amount(&v).is_some_and(|n| n > 0.0) {
            return false;
        }
        if (tin_col(&c.label) || c.label.to_lowercase().contains("name")) && c.kind == "text" && blank {
            return false;
        }
    }
    true
}

/// One schedule row as a page: Cancel (puts back what the row held) /
/// Row N / Done (FG3 until the row is complete), each column stacked
/// (worked-out columns as `=` rows, a payee's TIN as the saved-payee row),
/// Delete row at the bottom.
pub(crate) fn row_editor(form: Arc<OpenForm>, id: ItemId, r: usize) -> impl IntoElement {
    let st = session();
    let Some(item) = form.item(id).cloned() else { return view().into_element() };
    let keys = item.keys.get(r).cloned().unwrap_or_default();
    let label = format!("Row {}", item.start.unwrap_or(1) as usize + r);
    // What the row held when it opened, for Cancel.
    let before: Vec<(String, String)> = untracked(|| {
        keys.iter().zip(item.cols.iter()).filter(|(_, c)| !c.computed && c.kind != "hidden").map(|(k, _)| (k.clone(), st.value(k))).collect()
    });
    let mut body = view().pt(px(8.0));
    let mut payee_done = false;
    for (ci, c) in item.cols.iter().enumerate() {
        if c.kind == "hidden" {
            continue;
        }
        let Some(key) = keys.get(ci).cloned() else { continue };
        if c.computed {
            let (k, whole) = (key.clone(), c.whole);
            body = body.child(
                view()
                    .min_h(px(48.0))
                    .px(px(16.0))
                    .flex_row()
                    .items_center()
                    .border_b(px(1.0))
                    .border_color(theme::BORDER)
                    .child(text(format!("= {}", sub_label(&c.label))).flex_1().text_size(px(13.0)).text_color(theme::FG2))
                    .child(text(move || format_amount(&non_blank(&st.value(&k)), whole)).text_size(px(18.0)).font_family(mono()).tabular_nums().text_color(theme::FG)),
            );
            continue;
        }
        if tin_col(&c.label) && c.kind != "sel" {
            if !payee_done {
                payee_done = true;
                body = body.child(
                    view()
                        .min_h(px(64.0))
                        .px(px(16.0))
                        .py(px(12.0))
                        .gap(px(2.0))
                        .border_b(px(1.0))
                        .border_color(theme::BORDER)
                        .child(text("Payee").text_size(px(13.0)).text_color(theme::FG2))
                        .child(payee_row(&item, id, Some(r))),
                );
            }
            continue;
        }
        let control: AnyElement = match c.kind.as_str() {
            "sel" => choice_row(id, vec![col_field(c, key.clone())], true).into_element(),
            "radio" | "chk" => checkboxes(vec![LayoutField { kind: "chk".to_owned(), ..col_field(c, key) }]).into_element(),
            kind => text_box(key, kind.to_owned(), c.whole, c.computed, 0.0, None, false).into_element(),
        };
        body = body.child(
            view()
                .min_h(px(64.0))
                .px(px(16.0))
                .py(px(12.0))
                .gap(px(2.0))
                .border_b(px(1.0))
                .border_color(theme::BORDER)
                .child(text(sub_label(&c.label)).text_size(px(13.0)).text_color(theme::FG2))
                .child(control),
        );
    }
    let it_delete = item.clone();
    body = body.child(
        view()
            .mx(px(16.0))
            .mt(px(24.0))
            .h(px(48.0))
            .rounded(px(14.0))
            .items_center()
            .justify_center()
            .bg(theme::DANGER_SOFT)
            .cursor_pointer()
            .label("Delete row")
            .on_press(move || {
                delete_row(id, &it_delete, r);
                crate::screens::taxpayer::close_after_blur(move || st.row_edit.set(None));
            })
            .child(text("Delete row").text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::DANGER)),
    );
    let it_valid = item.clone();
    let valid = move || row_valid(&it_valid, r);
    let valid2 = valid.clone();
    let done = move || {
        if valid() {
            crate::screens::taxpayer::close_after_blur(move || st.row_edit.set(None));
        }
    };
    let cancel = move || {
        let before = before.clone();
        crate::screens::taxpayer::close_after_blur(move || {
            let now: Vec<(String, String)> = before.iter().filter(|(k, v)| st.value(k) != *v).cloned().collect();
            if !now.is_empty() {
                st.choose(now);
            }
            st.row_edit.set(None);
        });
    };
    view()
        .absolute()
        .inset(px(0.0))
        .z_index(32)
        .bg(theme::SURFACE)
        .child(
            view()
                .pt(move || px(safe_area().top.0))
                .pl(move || px(safe_area().left.0))
                .pr(move || px(safe_area().right.0))
                .border_b(px(1.0))
                .border_color(theme::BORDER)
                .child(
                    view()
                        .h(px(44.0))
                        .px(px(6.0))
                        .flex_row()
                        .items_center()
                        .child(view().h(px(44.0)).px(px(10.0)).justify_center().cursor_pointer().label("Cancel").on_press(cancel).child(text("Cancel").text_size(px(16.0)).text_color(theme::ACCENT)))
                        .child(text(label).flex_1().text_center().text_size(px(16.0)).font_weight(FontWeight::BOLD).text_color(theme::FG))
                        .child(
                            view()
                                .h(px(44.0))
                                .px(px(10.0))
                                .justify_center()
                                .cursor_pointer()
                                .label("Done")
                                .on_press(done)
                                .child(text("Done").text_size(px(16.0)).font_weight(FontWeight::BOLD).text_color(move || if valid2() { theme::ACCENT } else { theme::FG3 })),
                        ),
                ),
        )
        .child(
            scroll()
                .flex_1()
                .keyboard_dismiss_mode(KeyboardDismissMode::Interactive)
                .keyboard_taps(KeyboardTaps::Handled)
                .child(view().pl(move || px(safe_area().left.0)).pr(move || px(safe_area().right.0)).pb(move || px(keyboard_height().0.max(safe_area().bottom.0) + 24.0)).child(body)),
        )
        .into_element()
}

/// A schedule column as a field, for the controls that take fields.
fn col_field(c: &bir_app_core::LayoutColumn, key: String) -> LayoutField {
    LayoutField {
        key,
        label: c.label.clone(),
        kind: c.kind.clone(),
        computed: c.computed,
        max_len: None,
        required: false,
        whole: c.whole,
        options: c.options.clone(),
        group: None,
        group_size: None,
        pool: None,
        why: None,
    }
}

// ---------------------------------------------------------------- payees

/// The TIN boxes and the name box a picked payee fills: a schedule row's
/// TIN and name columns, else an item's TIN fields.
fn payee_keys(item: &LayoutItem, row: Option<usize>) -> (Vec<String>, Option<String>) {
    match row {
        Some(r) => {
            let keys = item.keys.get(r).cloned().unwrap_or_default();
            let mut tins = Vec::new();
            let mut name = None;
            for (k, c) in keys.iter().zip(item.cols.iter()) {
                if c.computed || c.kind == "hidden" {
                    continue;
                }
                if tin_col(&c.label) {
                    tins.push(k.clone());
                } else if name.is_none() && c.kind == "text" && c.label.to_lowercase().contains("name") {
                    name = Some(k.clone());
                }
            }
            (tins, name)
        }
        None => (item.fields.iter().filter(|f| f.kind == "text" || f.kind == "int").map(|f| f.key.clone()).collect(), None),
    }
}

/// Write `payee` into `tins` (3-3-3-5 when split, else whole) and `name`.
fn apply_payee(payee: &bir_app_core::Payee, tins: &[String], name: &Option<String>) {
    let st = session();
    let digits = payee.tin.as_str();
    let mut edits: Vec<(String, String)> = Vec::new();
    if tins.len() == 1 {
        edits.push((tins[0].clone(), crate::typing::dash_tin(digits, 14)));
    } else {
        let mut rest = digits;
        for (i, key) in tins.iter().enumerate() {
            let n = if i + 1 == tins.len() { rest.len() } else { 3.min(rest.len()) };
            let (head, tail) = rest.split_at(n);
            edits.push((key.clone(), head.to_owned()));
            rest = tail;
        }
    }
    if let Some(name) = name {
        edits.push((name.clone(), payee.name.clone()));
    }
    st.choose(edits);
}

/// The payee of a row (initials, name, TIN), or "Choose a payee"; a tap
/// opens the saved payees.
fn payee_row(item: &LayoutItem, id: ItemId, row: Option<usize>) -> impl IntoElement {
    let st = session();
    let (tins, name) = payee_keys(item, row);
    let shown = move || {
        let tin: String = tins.iter().map(|k| st.value(k)).collect::<String>().chars().filter(char::is_ascii_digit).collect();
        let who = name.as_ref().map(|k| st.value(k)).unwrap_or_default();
        (who, tin)
    };
    let shown2 = shown.clone();
    let shown3 = shown.clone();
    view()
        .flex_row()
        .items_center()
        .gap(px(12.0))
        .min_h(px(TAP))
        .cursor_pointer()
        .label("Choose a payee")
        .active(|s| s.opacity(0.6))
        .on_press(move || {
            dismiss_keyboard();
            st.payee.set(Some((id, row)));
        })
        .child(each(move || vec![shown3().0], |n| n.clone(), |n, _| {
            let n = n.get_untracked();
            let initials: String = n.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect::<String>().to_uppercase();
            view()
                .size(px(36.0))
                .rounded(px(theme::R_FULL))
                .items_center()
                .justify_center()
                .bg(theme::ACCENT_SOFT)
                .child(if initials.is_empty() { icon(Icon::User, 16.0, || theme::ACCENT) } else { text(initials).text_size(px(13.0)).font_weight(FontWeight::BOLD).text_color(theme::ACCENT).into_element() })
        }))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(text(move || {
                    let (who, tin) = shown();
                    if !who.is_empty() { who } else if tin.is_empty() { "Choose a payee".to_owned() } else { "Payee".to_owned() }
                }).text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG).truncate())
                .child(text(move || crate::typing::dash_tin(&shown2().1, 14)).text_size(px(12.5)).font_family(mono()).text_color(theme::FG3)),
        )
        .child(icon(Icon::ChevronRight, 14.0, || theme::FG3))
}

/// Saved payees: a search, "+ Add a new payee" first (name and TIN, saved
/// for the next schedule), then the payees saved under this taxpayer.
pub(crate) fn payee_sheet(form: Arc<OpenForm>, id: ItemId, row: Option<usize>) -> impl IntoElement {
    let st = session();
    let Some(item) = form.item(id).cloned() else { return view().into_element() };
    let root = form.params.tin.get(..9).unwrap_or(&form.params.tin).to_owned();
    let (tins, name_key) = payee_keys(&item, row);
    let saved = signal(Vec::<bir_app_core::Payee>::new());
    let query = signal(String::new());
    let adding = signal(false);
    let new_name = signal(String::new());
    let new_tin = signal(String::new());
    let error = signal(String::new());
    {
        let root = root.clone();
        crate::store::task(async move {
            if let Ok(list) = crate::store::call(move |app| app.saved_payees(&root)).await {
                saved.set(list);
            }
        });
    }
    let close = move || crate::screens::taxpayer::close_after_blur(move || st.payee.set(None));
    let filtered = move || {
        let q = query.get().trim().to_lowercase();
        let qd: String = q.chars().filter(char::is_ascii_digit).collect();
        saved.get().into_iter().filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q) || (!qd.is_empty() && p.tin.contains(&qd))).collect::<Vec<_>>()
    };
    let (tins2, name2) = (tins.clone(), name_key.clone());
    let pick = move |p: bir_app_core::Payee| {
        apply_payee(&p, &tins2, &name2);
        crate::screens::taxpayer::close_after_blur(move || st.payee.set(None));
    };
    let pick2 = pick.clone();
    let save_new = move || {
        let p = bir_app_core::Payee { name: new_name.get_untracked(), tin: new_tin.get_untracked() };
        let root = root.clone();
        let pick = pick2.clone();
        crate::store::task(async move {
            match crate::store::call(move |app| app.save_payee(&root, &p)).await {
                Ok(saved_one) => pick(saved_one),
                Err(why) => error.set(why),
            }
        });
    };
    let tin_at = element_ref();
    let search = move || input()
        .value(query)
        .on_change(move |v: &str| query.set(v.to_owned()))
        .placeholder("Search name or TIN")
        .placeholder_color(|| color(theme::FG3))
        .auto_correct(false)
        .h(px(40.0))
        .px(px(14.0))
        .text_size(px(16.0))
        .text_color(theme::FG)
        .bg(theme::HOVER)
        .rounded(px(10.0));
    let add_form = move || {
        let save_new = save_new.clone();
        view()
            .px(px(18.0))
            .py(px(12.0))
            .gap(px(10.0))
            .child(text("New payee").text_size(px(17.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
            .child(text("Registered name").text_size(px(13.0)).text_color(theme::FG2))
            .child(input().value(new_name).on_change(move |v: &str| new_name.set(v.to_owned())).placeholder("Reyes Law Office").placeholder_color(|| color(theme::FG3)).auto_focus(true).h(px(44.0)).text_size(px(17.0)).text_color(theme::FG).bg(CLEAR).border_b(px(1.0)).border_color(theme::BORDER))
            .child(text("TIN").text_size(px(13.0)).text_color(theme::FG2))
            .child(
                input()
                    .element_ref(&tin_at)
                    .value(new_tin)
                    .keyboard_type(KeyboardType::Number)
                    .on_change(move |v: &str| {
                        let prev = new_tin.get_untracked();
                        let shown = crate::typing::dash_tin(v, 14);
                        new_tin.set(shown.clone());
                        if shown != v {
                            let caret = crate::typing::caret_after(&prev, v, &shown, crate::typing::digit);
                            tin_at.set_selection(caret, caret);
                        }
                    })
                    .placeholder("000-000-000-00000")
                    .placeholder_color(|| color(theme::FG3))
                    .h(px(44.0))
                    .text_size(px(18.0))
                    .font_family(mono())
                    .text_color(theme::FG)
                    .bg(CLEAR)
                    .border_b(px(1.0))
                    .border_color(theme::BORDER),
            )
            .child(text(move || {
                let d = new_tin.get().chars().filter(char::is_ascii_digit).count();
                match d {
                    9 => "✓ Looks right · branch 00000 added".to_owned(),
                    14 => "✓ Looks right".to_owned(),
                    0 => "9 digits, or 14 with the branch code".to_owned(),
                    n => format!("{n} of 9 or 14 digits"),
                }
            }).text_size(px(12.5)).text_color(move || if matches!(new_tin.get().chars().filter(char::is_ascii_digit).count(), 9 | 14) { theme::SUCCESS } else { theme::FG3 }))
            .child(when(move || !error.get().is_empty(), move || text(move || error.get()).text_size(px(12.5)).text_color(theme::DANGER)))
            .child(
                view()
                    .h(px(48.0))
                    .rounded(px(12.0))
                    .items_center()
                    .justify_center()
                    .bg(theme::PRIMARY)
                    .cursor_pointer()
                    .label("Save payee")
                    .on_press(move || save_new())
                    .child(text("Save payee").text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::PRIMARY_FG)),
            )
    };
    full_sheet(
        "Payee".to_owned(),
        close,
        view()
            .flex_1()
            .child(when(move || !adding.get(), move || view().px(px(16.0)).py(px(10.0)).child(search())))
            .child(when(move || adding.get(), add_form))
            .child(when(move || !adding.get(), move || {
                let pick = pick.clone();
                let filtered = filtered.clone();
                scroll().flex_1().keyboard_dismiss_mode(KeyboardDismissMode::OnDrag).keyboard_taps(KeyboardTaps::Handled).child(
                    view()
                        .pb(move || px(keyboard_height().0.max(safe_area().bottom.0) + 16.0))
                        .child(
                            view()
                                .min_h(px(60.0))
                                .px(px(18.0))
                                .flex_row()
                                .items_center()
                                .gap(px(12.0))
                                .border_b(px(1.0))
                                .border_color(theme::BORDER)
                                .cursor_pointer()
                                .label("Add a new payee")
                                .active(|s| s.bg(theme::HOVER))
                                .on_press(move || adding.set(true))
                                .child(view().size(px(36.0)).rounded(px(theme::R_FULL)).items_center().justify_center().bg(theme::ACCENT_SOFT).child(icon(Icon::Plus, 16.0, || theme::ACCENT)))
                                .child(view().flex_1().child(text("Add a new payee").text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::ACCENT)).child(text("Name and TIN").text_size(px(12.5)).text_color(theme::FG3))),
                        )
                        .child(text("SAVED PAYEES").px(px(18.0)).pt(px(14.0)).pb(px(4.0)).text_size(px(11.0)).font_family(mono()).text_color(theme::FG3))
                        .child(when({
                            let filtered = filtered.clone();
                            move || filtered().is_empty()
                        }, || text("None yet. A payee you add is kept for the next schedule.").px(px(18.0)).py(px(8.0)).text_size(px(13.0)).text_color(theme::FG3)))
                        .child(each(filtered, |p| p.tin.clone(), move |p, _| {
                            let p = p.get_untracked();
                            let initials: String = p.name.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect::<String>().to_uppercase();
                            let pick = pick.clone();
                            let chosen = p.clone();
                            view()
                                .min_h(px(60.0))
                                .px(px(18.0))
                                .flex_row()
                                .items_center()
                                .gap(px(12.0))
                                .border_b(px(1.0))
                                .border_color(theme::BORDER)
                                .cursor_pointer()
                                .active(|s| s.bg(theme::HOVER))
                                .on_press(move || pick(chosen.clone()))
                                .child(view().size(px(36.0)).rounded(px(theme::R_FULL)).items_center().justify_center().bg(theme::ACCENT_SOFT).child(text(initials).text_size(px(13.0)).font_weight(FontWeight::BOLD).text_color(theme::ACCENT)))
                                .child(view().flex_1().min_w(px(0.0)).child(text(p.name.clone()).text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG).truncate()).child(text(crate::typing::dash_tin(&p.tin, 14)).text_size(px(12.5)).font_family(mono()).text_color(theme::FG3)))
                        })),
                )
            })),
    )
    .into_element()
}

// ---------------------------------------------------------------- computed

/// The item numbers a computed item's label names (`Sum of Items 47 and
/// 48`, `Item 51 Less Item 52`, `Items 55 to 61`).
fn referenced(label: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let lower = label.to_lowercase();
    let mut rest = lower.as_str();
    while let Some(at) = rest.find("item") {
        rest = &rest[at + 4..];
        let tail = rest.trim_start_matches('s').trim_start();
        // A run of numbers joined by `and`, `to`, `,`, `or`.
        let mut words = tail.split(|c: char| c.is_whitespace() || c == ',' || c == ')' || c == '(');
        let mut range_from: Option<u32> = None;
        let mut want_range = false;
        for w in words.by_ref() {
            if w.is_empty() || w == "and" || w == "or" {
                continue;
            }
            if w == "to" {
                want_range = true;
                continue;
            }
            let digits: String = w.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            if digits.is_empty() || !digits.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                break;
            }
            if want_range && let (Some(from), Ok(to)) = (range_from, digits.parse::<u32>()) {
                for n in from + 1..to {
                    out.push(n.to_string());
                }
            }
            want_range = false;
            range_from = digits.parse().ok();
            out.push(digits.to_uppercase());
        }
    }
    out.dedup();
    out
}

/// How a computed item is worked out: the rule its label states, then the
/// items it takes, each with its value.
pub(crate) fn explain_sheet(form: Arc<OpenForm>, id: ItemId) -> impl IntoElement {
    let st = session();
    let Some(item) = form.item(id).cloned() else { return view().into_element() };
    let rule = item.label.split_once('(').map(|(_, r)| r.trim_end_matches(')').to_owned()).unwrap_or_default();
    let refs = referenced(&item.label);
    let mut lines = view();
    for no in refs {
        let found = form
            .layout
            .parts
            .iter()
            .enumerate()
            .flat_map(|(pi, p)| p.items.iter().enumerate().map(move |(ii, it)| ((pi, ii), it)))
            .find(|(_, it)| it.no.eq_ignore_ascii_case(&no));
        let Some((_, other)) = found else { continue };
        let fields: Vec<LayoutField> = other.fields.iter().filter(|f| f.kind == "money" || f.computed).cloned().collect();
        let Some(f) = fields.first().cloned() else { continue };
        let title = format!("{} · {}", other.no, steps::item_title(other));
        lines = lines.child(
            view()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .px(px(20.0))
                .py(px(10.0))
                .border_t(px(1.0))
                .border_color(theme::BORDER)
                .child(text(title).flex_1().min_w(px(0.0)).text_size(px(13.0)).text_color(theme::FG2).number_of_lines(2u32))
                .child(text(move || format_amount(&st.value(&f.key), f.whole)).text_size(px(14.0)).tabular_nums().text_color(theme::FG)),
        );
    }
    let total_key = item.fields.first().map(|f| (f.key.clone(), f.whole));
    view()
        .absolute()
        .inset(px(0.0))
        .z_index(25)
        .child(view().absolute().inset(px(0.0)).bg(theme::SCRIM).on_press(move || st.explain.set(None)))
        .child(
            view()
                .absolute()
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0))
                .max_h(move || px(window().height.0 * 0.8))
                .rounded_t(px(22.0))
                .bg(theme::SURFACE)
                .pl(move || px(safe_area().left.0))
                .pr(move || px(safe_area().right.0))
                .pb(move || px(safe_area().bottom.0 + 16.0))
                .child(view().w(px(36.0)).h(px(5.0)).rounded(px(99.0)).bg(theme::BORDER).mt(px(8.0)).mb(px(6.0)).self_center())
                .child(text("How it's worked out").mx(px(20.0)).text_size(px(12.5)).text_color(theme::FG2))
                .child(text(format!("{} · {}", item.no, steps::item_title(&item).split(" (").next().unwrap_or_default())).mx(px(20.0)).text_size(px(18.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
                .child(when(!rule.is_empty(), {
                    let rule = rule.clone();
                    move || text(rule.clone()).mx(px(20.0)).mt(px(4.0)).mb(px(8.0)).text_size(px(13.0)).text_color(theme::FG2)
                }))
                .child(scroll().flex_shrink(1.0).child(lines))
                .child(when(total_key.is_some(), move || {
                    let (key, whole) = total_key.clone().unwrap_or_default();
                    view()
                        .flex_row()
                        .justify_between()
                        .px(px(20.0))
                        .py(px(12.0))
                        .border_t(px(1.0))
                        .border_color(theme::BORDER_STRONG)
                        .child(text("= Result").text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
                        .child(text(move || format_amount(&st.value(&key), whole)).text_size(px(16.0)).font_weight(FontWeight::BOLD).tabular_nums().text_color(theme::FG))
                }))
                .child(
                    view()
                        .mx(px(16.0))
                        .mt(px(8.0))
                        .h(px(TAP))
                        .rounded(px(14.0))
                        .items_center()
                        .justify_center()
                        .bg(theme::HOVER)
                        .cursor_pointer()
                        .active(|s| s.opacity(0.6))
                        .on_press(move || st.explain.set(None))
                        .child(text("Close").text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG)),
                ),
        )
        .into_element()
}
