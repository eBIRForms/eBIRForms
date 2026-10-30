//! The form screen: one schema-driven renderer for every BIR form.
//!
//! Parts are collapsible cards and only open parts are built, so a long
//! form (1701, 1702RT) costs what its open part costs; the side list jumps
//! between parts. Each box keeps the text being typed while it has the
//! focus and follows the return's value otherwise; edits go to the
//! session ([`crate::session::FormSession::edit`]), which writes them off
//! the main thread.

use std::sync::Arc;

use bir_app_core::display::{format_amount, format_form_date, parse_typed_date, period_label};
use bir_app_core::{FieldOption, FieldValue, LayoutField, LayoutItem};
use nsh_base::input;
use nsh_core::element::Os;
use nsh_core::prelude::*;

use crate::live::store;
use crate::screens::Route;
use crate::screens::steps;
use crate::session::{FormDoc, FormSession, ItemId, OpenForm, SaveState, plural};
use crate::store::task;
use crate::theme;
use crate::icons::{Icon, icon};
use crate::widgets::{CLEAR, Tone, Variant, avatar, button, button_sm, chip};

const DATE_ERROR: &str = "Enter a valid date as MM/DD/YYYY (e.g. 12/01/2020 or 12012020).";
const OFF_OPACITY: f32 = 0.45;

pub(super) fn session() -> &'static FormSession {
    &store().form
}

/// A monospace family for tags and amounts.
pub(crate) fn mono() -> &'static str {
    if os() == Os::Android { "monospace" } else { "Menlo" }
}

/// The family amounts use: Menlo on Apple platforms; Android's monospace
/// is set too wide, so amounts there keep the system font's tabular
/// figures.
pub(super) fn amount_font() -> &'static str {
    if os() == Os::Android { "sans-serif" } else { "Menlo" }
}

/// The title's saving line: `Saving…`, `✓ Saved` (tracked).
pub(super) fn save_line() -> String {
    let st = session();
    match st.save_state.get() {
        SaveState::Saving => "Saving…".to_owned(),
        SaveState::Saved => "✓ Saved".to_owned(),
        SaveState::Failed => "Not saved".to_owned(),
        SaveState::Idle if st.saved.get() => "✓ Saved".to_owned(),
        SaveState::Idle => String::new(),
    }
}

/// How far below the top of the list a row lands when the form jumps to it
/// (the phone's sticky section header covers the top).
fn jump_pad() -> f32 {
    if crate::mobile::phone() { 76.0 } else { 16.0 }
}

/// Scroll the form to `id` (opening its part) and draw attention to it;
/// `focus`: then focus its first box that takes typing.
pub(crate) fn jump_to(id: ItemId, focus: bool) {
    let st = session();
    if !st.open_parts.with_untracked(|o| o.contains(&id.0)) {
        st.open_parts.update(|o| {
            o.insert(id.0);
        });
    }
    st.flash.set(Some(id));
    store().owner.run(|| {
        set_timeout(std::time::Duration::from_millis(1400), move || {
            if st.flash.get_untracked() == Some(id) {
                st.flash.set(None);
            }
        })
    });
    task(async move {
        for _ in 0..90 {
            let row = st.rows.borrow().get(&id).copied();
            if let (Some(row), Some((list, content))) = (row, st.list.get())
                && let (Some(r), Some(c)) = (row.measure().await, content.measure().await)
            {
                // A section lands at the top (its header sticks there); an
                // item below the header.
                let pad = if id.1 == usize::MAX { 0.0 } else { jump_pad() };
                list.scroll_to(Point { x: px(0.0), y: px((r.page.y.0 - c.page.y.0 - pad).max(0.0)) }, true);
                if focus {
                    focus_first(id);
                }
                return;
            }
            next_frame().await;
        }
    });
}

/// Focus the first box of `id` that takes typing.
pub(super) fn focus_first(id: ItemId) {
    let st = session();
    let Some(form) = st.form() else { return };
    let Some(item) = form.item(id) else { return };
    let inputs = st.inputs.borrow();
    if let Some(at) = item.box_keys().iter().find_map(|k| inputs.get(k)) {
        at.focus();
    }
}

/// The boxes that take typing, in form order, that are on screen.
fn typing_order() -> Vec<String> {
    let st = session();
    let Some(form) = st.form() else { return Vec::new() };
    let inputs = st.inputs.borrow();
    untracked(|| {
        steps::shown_items(st, &form)
            .into_iter()
            .filter_map(|id| form.item(id))
            .flat_map(|item| item.box_keys())
            .filter(|k| inputs.contains_key(k) && steps::field_shown(st, k))
            .collect()
    })
}

/// Focus the `n`-th box that takes typing (a launch hook, `BUWIZ_FOCUS`).
pub(crate) fn focus_nth(n: usize) {
    let order = typing_order();
    if let Some(key) = order.get(n.min(order.len().saturating_sub(1))) {
        if let Some(item) = session().form().and_then(|f| f.boxes.get(key).map(|b| b.item)) {
            jump_to(item, false);
        }
        if let Some(at) = session().inputs.borrow().get(key) {
            at.focus();
        }
    }
}

/// Move the focus `by` boxes from the focused one (the keyboard row's
/// ↑ ↓) when `go`; whether there is a box there.
pub(super) fn step_focus(by: isize, go: bool) -> bool {
    let st = session();
    let order = typing_order();
    let Some(now) = st.focused.get_untracked() else { return false };
    let Some(at) = order.iter().position(|k| *k == now) else { return false };
    let next = at as isize + by;
    if next < 0 || next as usize >= order.len() {
        return false;
    }
    if go {
        let key = order[next as usize].clone();
        if let Some(at) = st.inputs.borrow().get(&key) {
            at.focus();
        }
    }
    true
}

/// Scroll `id` into the part of the list the keyboard and its row leave
/// visible, if it is not there already (a box just focused).
pub(super) fn reveal(id: ItemId) {
    let st = session();
    task(async move {
        // The keyboard comes up first.
        timer(std::time::Duration::from_millis(320)).await;
        let row = st.rows.borrow().get(&id).copied();
        let (Some(row), Some((list, content))) = (row, st.list.get()) else { return };
        let (Some(r), Some(c), Some(l)) = (row.measure().await, content.measure().await, list.measure().await) else { return };
        let bottom = untracked(|| window().height.0 - keyboard_height().0.max(safe_area().bottom.0) - crate::screens::phone_form::ACCESSORY_H - 8.0);
        let top = l.page.y.0 + jump_pad();
        let row_top = r.page.y.0;
        let row_bottom = row_top + r.frame.size.height.0;
        if row_bottom > bottom || row_top < top {
            list.scroll_to(Point { x: px(0.0), y: px((row_top - c.page.y.0 - jump_pad()).max(0.0)) }, true);
        }
    });
}

pub(crate) fn screen() -> impl IntoElement {
    let st = session();
    view().flex_1().h_full().child(each(
        move || vec![st.doc.get()],
        |doc| doc.key(),
        |doc, _| page(doc.get_untracked()),
    ))
}

fn page(doc: FormDoc) -> AnyElement {
    match doc {
        FormDoc::Closed => centered("No return is open. Pick a form under Forms.".to_owned(), false).into_element(),
        FormDoc::Opening { title, .. } => centered(format!("Opening {title}…"), false).into_element(),
        FormDoc::Failed { message, .. } => centered(format!("The return could not be opened: {message}"), true).into_element(),
        FormDoc::Open(form) if crate::mobile::phone() => crate::screens::phone_form::page(form).into_element(),
        FormDoc::Open(form) => open_page(form).into_element(),
    }
}

pub(super) fn centered(message: String, danger: bool) -> impl IntoElement {
    view()
        .flex_1()
        .items_center()
        .justify_center()
        .gap(px(12.0))
        .child(text(message).max_w(px(520.0)).text_size(px(14.0)).text_color(if danger { theme::DANGER } else { theme::FG2 }))
        .child(button("‹ Back to forms", Variant::Secondary, || false, || store().go(Route::Library)))
}

// ---------------------------------------------------------------- page

fn wide_nav() -> bool {
    window().width.0 >= theme::BREAK_FORM_NAV
}

/// The part navigator as a chip row, below `BREAK_FORM_NAV`.
fn chip_nav(form: Arc<OpenForm>) -> impl IntoElement {
    let st = session();
    let mut row = view().flex_row().flex_wrap().gap(px(6.0));
    for (pi, part) in form.layout.parts.iter().enumerate() {
        let name = part_name(&part.name, pi);
        row = row.child(
            view()
                .h(px(28.0))
                .px(px(10.0))
                .rounded(px(theme::R_FULL))
                .justify_center()
                .cursor_pointer()
                .border_1()
                .border_color(theme::BORDER)
                .bg(move || if st.open_parts.with(|o| o.contains(&pi)) { theme::SELECTED } else { theme::SURFACE })
                .on_press(move || st.open_parts.set(std::collections::BTreeSet::from([pi])))
                .child(text(name).text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(theme::FG)),
        );
    }
    row
}

/// Where each part's header is, for the navigator to scroll to.
type Heads = std::rc::Rc<std::cell::RefCell<std::collections::HashMap<usize, ElementRef>>>;

fn open_page(form: Arc<OpenForm>) -> impl IntoElement {
    let main = element_ref();
    let content = element_ref();
    let heads: Heads = Default::default();
    session().list.set(Some((main, content)));
    view()
        .flex_1()
        .h_full()
        .child(toolbar(form.clone()))
        .child(
            view()
                .flex_1()
                .flex_row()
                .child(when(wide_nav, {
                    let form = form.clone();
                    let heads = heads.clone();
                    move || side_nav(form.clone(), main, content, heads.clone())
                }))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(form_list(form.clone(), main, content, heads.clone()))
                        // The next step, floating over the list's end.
                        .child(
                            view()
                                .absolute()
                                .left(px(0.0))
                                .right(px(0.0))
                                .bottom(px(16.0))
                                .items_center()
                                .pointer_events(PointerEvents::BoxNone)
                                .child(view().w_full().max_w(px(520.0)).px(px(16.0)).child(crate::screens::phone_form::dock(form.clone()))),
                        ),
                ),
        )
        .child(when(move || session().outline_open.get(), {
            let form = form.clone();
            move || crate::screens::phone_form::outline_sheet(form.clone())
        }))
}

/// A row of the form list: a part's header, or one of its items (`last`:
/// the part's last item, which closes its card).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum FormRow {
    Head(usize),
    Item(usize, usize, bool),
}

/// The rows the list shows: every part's header, and the items of the
/// parts that are open.
fn form_rows(form: &OpenForm, open: &std::collections::BTreeSet<usize>) -> Vec<FormRow> {
    let st = session();
    let mut rows = Vec::new();
    for (pi, part) in form.layout.parts.iter().enumerate() {
        if steps::part_hidden(st, form, pi) {
            continue;
        }
        let shown: Vec<usize> = (0..part.items.len()).filter(|ii| !steps::item_hidden(st, form, (pi, *ii))).collect();
        if shown.is_empty() {
            continue;
        }
        rows.push(FormRow::Head(pi));
        if open.contains(&pi) {
            let n = shown.len();
            for (at, &ii) in shown.iter().enumerate() {
                let item = &part.items[ii];
                let last = at + 1 == n;
                let _ = item;
                rows.push(FormRow::Item(pi, ii, last));
            }
        }
    }
    rows
}

/// Rows mounted per display frame while parts open: AppKit makes each
/// row's views on the main thread, so opening every part of a long form
/// spreads over frames instead of stalling one.
const ROWS_PER_FRAME: usize = 3;

/// Once newly opened rows have all mounted, nudge the list one point and
/// back: AppKit lays out a long document it has just been given on its
/// next scroll, and doing that now, while nobody scrolls, keeps the
/// person's first scroll after "Expand all" from stalling a frame. It waits
/// until the list is at rest (no jump animating, the offset still for a few
/// frames), goes back only if nothing else scrolled in between, and skips
/// itself when the person has scrolled since the rows started mounting
/// (`from`), so it never cuts into their gesture.
fn warm_up(list: ElementRef, at: Signal<f32>, from: f32) {
    fn when_still(list: ElementRef, at: Signal<f32>, from: f32, last: f32, still: u32, tries: u32) {
        request_animation_frame(move |_| {
            let y = at.get_untracked();
            if (y - from).abs() > 0.5 {
                return;
            }
            let jumping = session().flash.get_untracked().is_some();
            let still = if !jumping && (y - last).abs() < 0.5 { still + 1 } else { 0 };
            if still >= 3 {
                list.scroll_to_offset(px(y + 1.0), false);
                request_animation_frame(move |_| {
                    request_animation_frame(move |_| {
                        if (at.get_untracked() - (y + 1.0)).abs() < 0.5 {
                            list.scroll_to_offset(px(y), false);
                        }
                    });
                });
            } else if tries < 240 {
                when_still(list, at, from, y, still, tries + 1);
            }
        });
    }
    when_still(list, at, from, at.get_untracked(), 0, 0);
}

/// The form as one scroll view of rows. Every row of an open part stays
/// mounted, so scrolling only moves native views (nothing is built while
/// scrolling); rows that a part opens mount a few per frame.
fn form_list(form: Arc<OpenForm>, list: ElementRef, content: ElementRef, heads: Heads) -> impl IntoElement {
    let st = session();
    let rows_form = form.clone();
    let rows = computed(move || {
        let open = st.open_parts.get();
        form_rows(&rows_form, &open)
    });
    // The rows mounted so far; rows of a collapsed part go at once.
    let mounted = signal(std::collections::HashSet::<FormRow>::new());
    let pumping = std::rc::Rc::new(std::cell::Cell::new(false));
    // Where the list is scrolled to, for the warm-up below.
    let at = signal(0.0f32);
    effect({
        let pumping = pumping.clone();
        move || {
            let wanted = rows.get();
            mounted.update(|m| m.retain(|r| wanted.contains(r)));
            if pumping.get() {
                return;
            }
            pumping.set(true);
            fn pump(rows: Memo<Vec<FormRow>>, mounted: Signal<std::collections::HashSet<FormRow>>, pumping: std::rc::Rc<std::cell::Cell<bool>>, list: ElementRef, at: Signal<f32>, from: f32, grew: bool) {
                let next: Vec<FormRow> = untracked(|| {
                    let have = mounted.get_untracked();
                    rows.get_untracked().into_iter().filter(|r| !have.contains(r)).take(ROWS_PER_FRAME).collect()
                });
                if next.is_empty() {
                    pumping.set(false);
                    if grew {
                        warm_up(list, at, from);
                    }
                    return;
                }
                mounted.update(|m| m.extend(next));
                request_animation_frame(move |_| pump(rows, mounted, pumping, list, at, from, true));
            }
            pump(rows, mounted, pumping.clone(), list, at, untracked(|| at.get_untracked()), false);
        }
    });
    let shown = move || {
        let have = mounted.get();
        rows.with(|r| r.iter().copied().filter(|row| have.contains(row)).collect::<Vec<_>>())
    };
    let head = view()
        .pt(px(24.0))
        .pb(px(16.0))
        .gap(px(16.0))
        .child(header(form.clone()))
        .child(when(move || !wide_nav(), {
            let form = form.clone();
            move || chip_nav(form.clone())
        }))
        .child(form_errors())
        .child(penalties_panel());
    let bench = scroll_bench(form.clone(), list, move || mounted.with(|m| m.len()) == rows.with(Vec::len));
    let row_form = form.clone();
    let scroller = scroll().element_ref(&list).flex_1();
    // The bench reads back where the list really got to.
    let scroller = scroller.on_scroll(move |e: &ScrollEvent| {
        at.set(e.offset.y.0);
        // The bench reads back where the list really got to.
        if let Some(reading) = bench {
            let (far, _, events) = reading.get_untracked();
            reading.set((far.max(e.offset.y.0), e.content_size.height.0 - e.size.height.0, events + 1));
        }
    });
    scroller.child(
        view()
            .element_ref(&content)
            .px(px(24.0))
            .pb(px(120.0))
            .child(head)
            .child(each(shown, |r| *r, move |r, _| match r.get_untracked() {
                FormRow::Head(pi) => {
                    let at = element_ref();
                    heads.borrow_mut().insert(pi, at);
                    session().rows.borrow_mut().insert((pi, usize::MAX), at);
                    part_head(row_form.clone(), pi).element_ref(&at).into_element()
                }
                FormRow::Item(pi, ii, last) => item_cell(row_form.clone(), (pi, ii), last).into_element(),
            })),
    )
}

/// What the scroll bench reads back from the list: the furthest offset it
/// reported, how far it could scroll, and how many scroll events came.
type BenchReading = Signal<(f32, f32, u32)>;

/// `BUWIZ_SCROLL_BENCH=<points per frame>` (a measurement hook, off
/// otherwise): once the form shows, open every part and scroll to the
/// bottom and back three times at that speed. When it runs, the reading
/// the list's scroll events feed.
fn scroll_bench(form: Arc<OpenForm>, list: ElementRef, settled: impl Fn() -> bool + 'static) -> Option<BenchReading> {
    let Some(speed) = std::env::var("BUWIZ_SCROLL_BENCH").ok().and_then(|v| v.parse::<f32>().ok()).filter(|v| *v > 0.0) else {
        return None;
    };
    let reading: BenchReading = crate::live::keep("scroll_bench", || (0.0, 0.0, 0));
    let st = session();
    // A window on a workspace no one is looking at is a hidden window:
    // without this the system naps the app and its frames slow to a crawl.
    #[cfg(target_os = "macos")]
    crate::mac::hold_off_app_nap("scroll bench");
    let all: std::collections::BTreeSet<usize> = (0..form.layout.parts.len()).collect();
    let code = form.params.form_code.clone();
    let settled: std::rc::Rc<dyn Fn() -> bool> = std::rc::Rc::new(settled);
    set_timeout(std::time::Duration::from_millis(800), move || {
        st.open_parts.set(all);
        log::warn!(target: "nsh::jank", "---- scroll bench {code} {speed}pt/frame ----");
        // The frame times the steps ran at: the last one, how many steps,
        // how many came more than one and a half 120 Hz periods after the
        // one before (a frame the scroll skipped), and the longest gap.
        type Gaps = (Option<std::time::Instant>, u32, u32, f32);
        fn step(list: ElementRef, reading: BenchReading, y: f32, dir: f32, legs: u32, speed: f32, gaps: Gaps) {
            request_animation_frame(move |at| {
                let (last, steps, skipped, longest) = gaps;
                let gap = last.map_or(0.0, |last| at.saturating_duration_since(last).as_secs_f32() * 1000.0);
                let gaps = (Some(at), steps + 1, skipped + u32::from(gap > 12.5), longest.max(gap));
                list.scroll_to_offset(px(y.max(0.0)), false);
                // Turn at the bottom the list reports (until it has, a
                // bound past any form's length).
                let bottom = reading.get_untracked().1;
                let bottom = if bottom > 0.0 { bottom } else { 60000.0 };
                let (y, dir, legs) = if y >= bottom { (bottom, -1.0, legs + 1) } else if y <= 0.0 && dir < 0.0 { (0.0, 1.0, legs + 1) } else { (y, dir, legs) };
                if legs < 6 {
                    step(list, reading, y + dir * speed, dir, legs, speed, gaps);
                } else {
                    let (far, max, events) = reading.get_untracked();
                    let (_, steps, skipped, longest) = gaps;
                    log::warn!(target: "nsh::jank", "==== bench end ==== reached {far:.0}pt of {max:.0}pt, {events} scroll events");
                    log::warn!(target: "nsh::jank", "frame gaps: {steps} steps, {skipped} over 12.5 ms, longest {longest:.1} ms");
                }
            });
        }
        // Scroll once every row is mounted, and the schedule tables (which
        // add their rows a few per frame after their item mounts) are full:
        // the list must stay settled for a while before scrolling starts.
        fn when_settled(list: ElementRef, reading: BenchReading, speed: f32, settled: std::rc::Rc<dyn Fn() -> bool>, calm: u32) {
            let steps = session().table_steps.get_untracked();
            set_timeout(std::time::Duration::from_millis(200), move || {
                let full = session().table_steps.get_untracked() == steps;
                let calm = if settled() && full { calm + 1 } else { 0 };
                if calm >= 4 {
                    reading.set((0.0, 0.0, 0));
                    log::warn!(target: "nsh::jank", "---- mounted; scrolling ----");
                    step(list, reading, 0.0, 1.0, 0, speed, (None, 0, 0, 0.0));
                } else {
                    when_settled(list, reading, speed, settled, calm);
                }
            });
        }
        when_settled(list, reading, speed, settled.clone(), 0);
    });
    Some(reading)
}

/// A part's header row: a card top, closed into a full card while the part
/// is collapsed.
fn part_head(form: Arc<OpenForm>, pi: usize) -> View {
    let st = session();
    let part = &form.layout.parts[pi];
    let open = move || st.open_parts.with(|o| o.contains(&pi));
    let count_form = form.clone();
    let errors = move || {
        let ids: Vec<ItemId> = st.messages.with(|m| m.iter().filter(|p| p.item.0 == pi).map(|p| p.item).collect());
        ids.into_iter().filter(|id| steps::error_shown(st, &count_form, *id)).count()
    };
    let count = part.items.len();
    view()
        .mt(px(if pi == 0 { 0.0 } else { 12.0 }))
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(12.0))
        .h(px(44.0))
        .border_1()
        .border_color(theme::BORDER)
        .bg(theme::SURFACE)
        .rounded_t(px(theme::R_CARD))
        .rounded_b(move || px(if open() { 0.0 } else { theme::R_CARD }))
        .cursor_pointer()
        .label(part_name(&part.name, pi))
        .active(|s| s.opacity(0.7))
        .on_press(move || st.toggle_part(pi))
        .child(each(move || vec![open()], |o| *o, |o, _| icon(if o.get_untracked() { Icon::ChevronDown } else { Icon::ChevronRight }, 16.0, || theme::FG2)))
        .child(text(part_name(&part.name, pi)).flex_1().flex_shrink(1.0).min_w(px(0.0)).text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
        .child(when({
            let errors = errors.clone();
            move || errors() > 0
        }, move || {
            let errors = errors.clone();
            chip(move || format!("{} to fix", errors()), Tone::Danger)
        }))
        .child(text(format!("{count} item{}", plural(count))).text_size(px(11.0)).text_color(theme::FG2))
}

/// An item row inside its part's card.
fn item_cell(form: Arc<OpenForm>, id: ItemId, last: bool) -> impl IntoElement {
    view()
        .border_l(px(1.0))
        .border_r(px(1.0))
        .border_b(px(if last { 1.0 } else { 0.0 }))
        .border_color(theme::BORDER)
        .rounded_b(px(if last { theme::R_CARD } else { 0.0 }))
        .overflow_hidden()
        .child(item_row(form, id))
}

fn toolbar(form: Arc<OpenForm>) -> impl IntoElement {
    let st = session();
    let freq = form.layout.frequency.clone();
    let p = &form.params;
    let title = format!("{} · {}", p.form_code, period_label(&freq, p.year, p.period));
    let busy = move || st.busy.get().is_some();
    let editable = move || st.editable.get();
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .h(px(52.0))
        .px(px(16.0))
        .border_b(px(1.0))
        .border_color(theme::BORDER)
        .bg(theme::SURFACE)
        .child(
            view()
                .flex_1()
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .child(text(title).text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG).truncate())
                .child(text(save_line).text_size(px(11.0)).text_color(theme::FG2)),
        )
        .child(when(move || st.busy.get().is_some(), move || chip(move || st.busy.get().unwrap_or("").to_owned(), Tone::Accent)))
        .child(when(move || st.status.get() == "Queued", move || {
            button_sm("Cancel queue", Variant::Secondary, None, busy, move || st.cancel_queue())
        }))
        .child(when(editable, move || {
            view()
                .flex_row()
                .gap(px(8.0))
                .child(button_sm("Check", Variant::Ghost, Some(Icon::CircleCheck), busy, move || st.validate()))
        }))
        .child(when(move || !st.editable.get() && !st.sample.get() && matches!(st.status.get().as_str(), "Sent" | "Submitted" | "Confirmed" | "Paid"), move || {
            button_sm("Amend", Variant::Secondary, None, busy, move || st.amend()).label("Amend")
        }))
        .child(button_sm("Print", Variant::Secondary, Some(Icon::Printer), busy, move || st.print()).label("Print"))
}

fn header(form: Arc<OpenForm>) -> impl IntoElement {
    let st = session();
    let layout = &form.layout;
    view()
        .gap(px(4.0))
        .child(text(format!("BIR Form {} · Revision {}", layout.code, layout.revision)).text_size(px(12.0)).text_color(theme::FG2))
        .child(text(layout.title.clone()).text_size(px(20.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(6.0))
                .mt(px(4.0))
                .child(chip(layout.frequency.clone(), Tone::Neutral))
                .child(chip(
                    move || {
                        let status = st.status.get();
                        if st.sample.get() { "Sample · not saved".to_owned() } else if st.editable.get() { status } else { format!("{status} · read-only") }
                    },
                    Tone::Accent,
                ))
                .child(when(move || st.busy.get() == Some("Checking…") || st.checking.get(), || chip("Checking…", Tone::Accent))),
        )
        .child(when(move || st.amending.get(), || view().mt(px(8.0)).max_w(px(520.0)).child(amend_band())))
        .child(view().mt(px(12.0)).max_w(px(520.0)).child(filer_card(form.clone())))
}

/// What an amendment is, over its form (desktop and phone).
pub(super) fn amend_band() -> impl IntoElement {
    view()
        .p(px(12.0))
        .gap(px(2.0))
        .rounded(px(theme::R_ROW))
        .bg(theme::ACCENT_SOFT)
        .child(text("Amending a sent return").text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::ACCENT))
        .child(text(AMEND_LINE).text_size(px(12.0)).text_color(theme::FG2))
}

pub(super) const AMEND_LINE: &str = "A new draft that amends the return you sent. Fix what was wrong, then check it and queue it.";

/// Open the penalties that apply for typing on this return (Edit
/// penalties).
pub(crate) fn unlock_penalties() {
    let st = session();
    let keys: Vec<String> = st.penalties.get_untracked().into_iter().filter(|p| p.applies).flat_map(|p| p.keys).collect();
    st.unlock(keys);
}

/// Open the profile's boxes for typing on this return (Edit on this
/// return); the profile itself is unchanged.
pub(crate) fn unlock_profile() {
    let st = session();
    st.unlock(st.profile.get_untracked());
    store().notify(crate::store::NoticeTone::Info, "The profile's boxes are open on this return. What you type stays here; the profile is unchanged.");
}

/// Whether "Edit on this return" applies (tracked): an editable return
/// with profile boxes not all opened yet.
pub(super) fn can_unlock_profile() -> bool {
    let st = session();
    st.editable.get() && !st.profile.with(|p| p.is_empty()) && !st.profile.with(|p| st.unlocked.with(|u| p.iter().all(|k| u.contains(k))))
}

/// Whether one of `keys` is a profile box opened on this return (tracked).
pub(super) fn profile_opened(keys: &[String]) -> bool {
    let st = session();
    st.unlocked.with(|u| st.profile.with(|p| keys.iter().any(|k| u.contains(k) && p.contains(k))))
}

/// Whether one of `keys` is a penalty the filer enters (tracked).
pub(super) fn penalty_opened(keys: &[String]) -> bool {
    let st = session();
    st.penalties.with(|parties| {
        parties.iter().filter(|p| p.applies).any(|p| (p.edited || st.unlocked.with(|u| p.keys.iter().any(|k| u.contains(k)))) && keys.iter().any(|k| p.keys.contains(k)))
    })
}

/// The filer, from the tax profile, as one read-only card: name, TIN and
/// RDO. A press opens the tax profile (the boxes they fill are not shown).
pub(super) fn filer_card(form: Arc<OpenForm>) -> impl IntoElement {
    let st = session();
    let tin = p_tin(&form);
    let rdo_key = form
        .layout
        .parts
        .iter()
        .flat_map(|p| p.items.iter())
        .find(|i| i.label.contains("RDO"))
        .and_then(|i| i.fields.first().map(|f| f.key.clone()));
    let name = {
        let tin = tin.clone();
        move || match store().selected.get() {
            Some(sel) if sel.tin == tin => sel.name,
            _ => String::new(),
        }
    };
    let name2 = name.clone();
    let tin_line = format_branch_tin(&tin);
    let rdo = move || rdo_key.as_ref().map(|k| st.value(k)).filter(|v| !v.trim().is_empty()).map(|v| format!("RDO {v}")).unwrap_or_default();
    let email_tin = tin.clone();
    let email = move || {
        let list = store().taxpayers.get();
        match list {
            crate::store::Loaded::Ready(list) => list
                .iter()
                .flat_map(|t| t.branches.iter())
                .find(|b| b.tin == email_tin)
                .map(|b| b.facts.email.clone())
                .unwrap_or_default(),
            _ => String::new(),
        }
    };
    let email2 = email.clone();
    let root = tin.get(..9).unwrap_or(&tin).to_owned();
    view()
        .flex_row()
        .items_center()
        .gap(px(12.0))
        .p(px(12.0))
        .rounded(px(16.0))
        .bg(theme::HOVER)
        .cursor_pointer()
        .label("Filer, from your tax profile")
        .active(|s| s.opacity(0.7))
        .on_press(|| store().open_profile())
        .child(each(move || vec![name2()], |n| n.clone(), move |n, _| avatar(&n.get_untracked(), &root, 38.0)))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(text(name).text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG).truncate())
                .child(text(tin_line).text_size(px(11.0)).font_family(mono()).tabular_nums().text_color(theme::FG2))
                .child(text(rdo).text_size(px(11.0)).font_family(mono()).text_color(theme::FG2))
                .child(when(move || !email().is_empty(), move || text(email2.clone()).text_size(px(11.0)).text_color(theme::FG2).truncate())),
        )
        .child(
            view()
                .items_end()
                .gap(px(6.0))
                .child(text("Change in profile").text_size(px(12.0)).text_color(theme::ACCENT))
                .child(when(can_unlock_profile, move || {
                    text("Edit on this return")
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::ACCENT)
                        .cursor_pointer()
                        .label("Edit on this return")
                        .on_press(unlock_profile)
                })),
        )
}

pub(super) fn p_tin(form: &OpenForm) -> String {
    form.params.tin.clone()
}

pub(super) fn format_branch_tin(tin: &str) -> String {
    if tin.len() == 14 {
        format!("{}-{}-{}-{}", &tin[..3], &tin[3..6], &tin[6..9], &tin[9..])
    } else {
        tin.to_owned()
    }
}

fn form_errors() -> impl IntoElement {
    let st = session();
    when(move || !st.form_errors.get().is_empty(), move || {
        text(move || st.form_errors.get().join(" · ")).text_size(px(12.0)).text_color(theme::DANGER)
    })
}

/// Whether the filer opened the penalties with Edit (tracked).
pub(super) fn penalties_open() -> bool {
    let st = session();
    st.penalties.with(|p| p.iter().filter(|p| p.applies).any(|p| p.edited || st.unlocked.with(|u| p.keys.iter().any(|k| u.contains(k)))))
}

/// Surcharge, interest, compromise and their total per party the
/// penalties apply to.
fn penalties_panel() -> impl IntoElement {
    let st = session();
    let parties = move || st.penalties.get().into_iter().filter(|p| p.applies).collect::<Vec<_>>();
    let many = move || st.penalties.with(|p| p.len() > 1);
    when(move || !parties().is_empty(), move || {
        view()
            .p(px(12.0))
            .gap(px(6.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(theme::BORDER)
            .bg(theme::SURFACE)
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(text("Penalties").flex_1().text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
                    .child(when(move || st.editable.get() && !penalties_open(), move || {
                        button_sm("Edit penalties", Variant::Ghost, None, || false, unlock_penalties)
                    })),
            )
            .child(text(move || if penalties_open() { "You're entering the penalties yourself (an amount the RDO approved, say). Type them in their items below." } else { "Computed from the due date and the amount payable; they change as you fill the return." }).text_size(px(11.0)).text_color(theme::FG2))
            .child(each(parties, |p| p.keys.clone(), move |party, index| {
                let party = party.get_untracked();
                let label = move || if !many() { String::new() } else if index.get() == 0 { "Taxpayer".to_owned() } else { "Spouse".to_owned() };
                let amount = |key: Option<String>| move || key.as_deref().map(|k| format_amount(&non_blank(&st.value(k)), false)).unwrap_or_default();
                let total = {
                    let party = party.clone();
                    move || {
                        let values: std::collections::BTreeMap<String, String> = party.keys.iter().map(|k| (k.clone(), st.value(k))).collect();
                        party.total(&values)
                    }
                };
                view()
                    .flex_row()
                    .flex_wrap()
                    .gap(px(14.0))
                    .items_end()
                    .child(text(label).w(px(64.0)).text_size(px(12.0)).text_color(theme::FG2))
                    .child(money_cell("Surcharge", amount(party.keys.first().cloned())))
                    .child(money_cell("Interest", amount(party.keys.get(1).cloned())))
                    .child(money_cell("Compromise", amount(party.keys.get(2).cloned())))
                    .child(money_cell("Total", total))
            }))
    })
}

pub(super) fn non_blank(text: &str) -> String {
    if text.trim().is_empty() { "0".to_owned() } else { text.to_owned() }
}

pub(super) fn money_cell(label: &'static str, value: impl Fn() -> String + 'static) -> impl IntoElement {
    view()
        .w(px(110.0))
        .gap(px(2.0))
        .child(text(label).text_size(px(11.0)).text_color(theme::FG2))
        .child(text(move || format!("₱ {}", value())).text_size(px(13.0)).text_color(theme::FG).tabular_nums().text_right())
}

fn side_nav(form: Arc<OpenForm>, main: ElementRef, content: ElementRef, heads: Heads) -> impl IntoElement {
    let st = session();
    let mut list = view().gap(px(2.0)).pt(px(16.0)).pb(px(32.0)).px(px(8.0));
    list = list.child(text(format!("On this form · {} parts", form.layout.parts.len())).px(px(8.0)).pb(px(8.0)).text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(theme::FG3));
    let all: std::collections::BTreeSet<usize> = (0..form.layout.parts.len()).collect();
    list = list.child(
        view()
            .flex_row()
            .gap(px(4.0))
            .px(px(4.0))
            .pb(px(8.0))
            .child(button_sm("Expand all", Variant::Ghost, None, || false, move || st.open_parts.set(all.clone())).label("Expand all"))
            .child(button_sm("Collapse all", Variant::Ghost, None, || false, move || st.open_parts.set(Default::default())).label("Collapse all")),
    );
    for (pi, part) in form.layout.parts.iter().enumerate() {
        let errors = move || st.messages.with(|m| m.iter().filter(|p| p.item.0 == pi).count());
        let heads = heads.clone();
        list = list.child(
            view()
                .flex_row()
                .items_start()
                .gap(px(6.0))
                .px(px(8.0))
                .py(px(7.0))
                .rounded(px(theme::R_ROW))
                .cursor_pointer()
                .bg(move || if st.open_parts.with(|o| o.contains(&pi)) { theme::SELECTED } else { CLEAR })
                .hover(|s| s.bg(theme::HOVER))
                .on_press(move || {
                    st.open_parts.update(|o| {
                        o.insert(pi);
                    });
                    // The header's place in the content, once it is mounted.
                    let heads = heads.clone();
                    task(async move {
                        for _ in 0..60 {
                            let at = heads.borrow().get(&pi).copied();
                            if let Some(at) = at
                                && let (Some(h), Some(c)) = (at.measure().await, content.measure().await)
                            {
                                main.scroll_to(Point { x: px(0.0), y: px((h.page.y.0 - c.page.y.0 - 8.0).max(0.0)) }, true);
                                return;
                            }
                            next_frame().await;
                        }
                    });
                })
                .child(text(part_name(&part.name, pi)).flex_1().flex_shrink(1.0).min_w(px(0.0)).text_size(px(13.0)).text_color(theme::FG))
                .child(when(move || errors() > 0, move || chip(move || errors().to_string(), Tone::Danger))),
        );
    }
    scroll().w(px(208.0)).h_full().flex_shrink(0.0).border_r(px(1.0)).border_color(theme::BORDER).child(list)
}

fn part_name(name: &str, index: usize) -> String {
    if index == 0 && name.starts_with("Page ") { "Return details".to_owned() } else { name.to_owned() }
}

// ---------------------------------------------------------------- parts

fn item_row(form: Arc<OpenForm>, id: ItemId) -> impl IntoElement {
    let st = session();
    let item = form.item(id).cloned().expect("an item of the layout");
    let all_computed = !item.is_schedule() && !item.fields.is_empty() && item.fields.iter().all(|f| f.computed);
    let required = item.fields.iter().any(|f| f.required);
    let from_profile = {
        let keys: Vec<String> = item.fields.iter().map(|f| f.key.clone()).collect();
        move || st.profile.with(|p| keys.iter().any(|k| p.contains(k)))
    };
    let keys = item.box_keys();
    let needs = {
        let keys = keys.clone();
        move || {
            st.off.with(|off| {
                let mut out: Vec<String> = Vec::new();
                for k in &keys {
                    if let Some(why) = off.get(k)
                        && !out.contains(why)
                    {
                        out.push(why.clone());
                    }
                }
                out
            })
        }
    };
    let notes = {
        let fields = item.fields.clone();
        move || lock_notes(st, &fields)
    };
    let message = item_message(form.clone(), id);
    let label = item.label.clone();
    let optional = !required && is_optional(&item);
    let border = id.1 > 0;
    let at = element_ref();
    st.rows.borrow_mut().insert(id, at);
    view()
        .element_ref(&at)
        .flex_row()
        .gap(px(10.0))
        .pt(px(10.0))
        .pb(px(8.0))
        .pl(px(10.0))
        .pr(px(16.0))
        .border_t(px(if border { 1.0 } else { 0.0 }))
        .border_color(theme::BORDER)
        .bg(move || if st.flash.get() == Some(id) { theme::ACCENT_SOFT } else if all_computed { theme::BG } else { theme::SURFACE })
        .min_h(px(52.0))
        .child(
            view().w(px(40.0)).flex_shrink(0.0).child(when(!item.no.is_empty(), {
                let no = item.no.clone();
                move || {
                    view()
                        .h(px(20.0))
                        .px(px(5.0))
                        .rounded(px(5.0))
                        .self_start()
                        .justify_center()
                        .bg(theme::BG)
                        .child(text(no.clone()).text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).tabular_nums().text_color(theme::FG2))
                }
            })),
        )
        .child(
            view()
                .flex_1()
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .gap(px(5.0))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap(px(6.0))
                        .child(text(label).flex_shrink(1.0).text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(theme::FG))
                        .child(when(optional, || text("Optional").text_size(px(11.0)).text_color(theme::FG3)))
                        .child({
                            let keys = keys.clone();
                            let opened = move || profile_opened(&keys);
                            let opened2 = opened.clone();
                            when(move || from_profile() && !opened(), || chip("From tax profile", Tone::Neutral)).otherwise(move || when(opened2.clone(), || chip("On this return only", Tone::Accent)))
                        })
                        .child({
                            let keys = keys.clone();
                            when(move || penalty_opened(&keys), || chip("You enter this penalty", Tone::Accent))
                        })
                        .child(when(all_computed, || chip("Computed", Tone::Neutral))),
                )
                .child(each(needs, |n| n.clone(), |n, _| text(n.get_untracked()).text_size(px(11.0)).text_color(theme::FG2)))
                .child(if item.is_schedule() { schedule_table(form.clone(), id).into_element() } else { controls(&item) })
                .child(each(notes, |n| n.clone(), |n, _| text(n.get_untracked()).text_size(px(11.0)).text_color(theme::FG3)))
                // The reserved 18 px message line: a text only while there
                // is a message (an empty one still costs a drawn layer).
                .child({
                    let shown = message.clone();
                    when(move || !shown().is_empty(), move || text(message.clone()).h(px(18.0)).text_size(px(12.0)).text_color(theme::DANGER).truncate()).otherwise(|| view().h(px(18.0)))
                }),
        )
}

/// `1234567.5` as `1,234,567.5`, as it is being typed (the decimals stay
/// as typed).
fn live_commas(clean: &str) -> String {
    let (sign, rest) = clean.strip_prefix('-').map_or(("", clean), |r| ("-", r));
    let (whole, frac) = match rest.split_once('.') {
        Some((w, f)) => (w, Some(f)),
        None => (rest, None),
    };
    if !whole.chars().all(|c| c.is_ascii_digit()) {
        return clean.to_owned();
    }
    let mut grouped = String::new();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    match frac {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    }
}

/// A box took the focus: the keyboard row follows it, and on a phone the
/// row scrolls clear of the keyboard.
pub(super) fn enter_box(key: String) {
    let st = session();
    let item = st.form().and_then(|f| f.boxes.get(&key).map(|b| b.item));
    st.focused.set(Some(key));
    if crate::mobile::phone()
        && let Some(item) = item
    {
        reveal(item);
    }
}

/// A box lost the focus: the keyboard row goes unless another box took it
/// (↑ ↓ blur one box and focus the next).
pub(super) fn leave_box(key: String) {
    let st = session();
    store().owner.run(|| {
        set_timeout(std::time::Duration::from_millis(80), move || {
            if st.focused.get_untracked().as_deref() == Some(key.as_str()) {
                st.focused.set(None);
            }
        })
    });
}

/// The messages on `id` once they show ([`steps::error_shown`]).
pub(super) fn item_message(form: Arc<OpenForm>, id: ItemId) -> impl Fn() -> String + Clone + 'static {
    let st = session();
    move || {
        if !steps::error_shown(st, &form, id) {
            return String::new();
        }
        st.messages.with(|m| m.iter().filter(|p| p.item == id).map(|p| p.message.clone()).collect::<Vec<_>>().join(" · "))
    }
}

/// Only optional boxes say so: text, dates and choices the filer may skip
/// (an amount left at zero is not "optional", it is zero).
pub(super) fn is_optional(item: &LayoutItem) -> bool {
    !item.is_schedule()
        && !item.fields.is_empty()
        && item.fields.iter().all(|f| !f.required && !f.computed && matches!(f.kind.as_str(), "text" | "date" | "sel" | "int"))
        && !from_profile(item)
}

/// A box the tax profile fills (the email, RDO, address, ZIP, name): the
/// rules want it even where the schema does not mark it required.
fn from_profile(item: &LayoutItem) -> bool {
    let label = item.label.to_lowercase();
    ["email", "rdo", "address", "zip", "name"].iter().any(|w| label.contains(w))
}

/// Why locked, blank boxes of an item stay so, once each.
pub(super) fn lock_notes(st: &'static FormSession, fields: &[LayoutField]) -> Vec<String> {
    if !st.editable.get() {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    for f in fields {
        if f.kind == "radio" || !st.is_locked(&f.key, f.computed) || st.off_reason(&f.key).is_some() {
            continue;
        }
        let blank = match f.kind.as_str() {
            "chk" => !st.is_ticked(&f.key),
            "money" if f.computed => false,
            _ => st.value(&f.key).trim().is_empty(),
        };
        if !blank {
            continue;
        }
        let note = st.lock_notes.with(|n| n.get(&f.key).cloned()).or_else(|| f.why.clone()).unwrap_or_default();
        if !note.is_empty() && !out.contains(&note) {
            out.push(note);
        }
    }
    out
}

pub(super) fn controls(item: &LayoutItem) -> AnyElement {
    if item.is_schedule() {
        return schedule_head(item).into_element();
    }
    if item.fields.is_empty() {
        return text(item.note.clone().unwrap_or_default()).text_size(px(12.0)).text_color(theme::FG2).into_element();
    }
    if item.one_date
        && let Some(parts) = date_parts(&item.fields)
    {
        return one_date(parts).into_element();
    }
    if is_tin_item(item) {
        return tin_boxes(item).into_element();
    }
    let multi = item.fields.len() > 1;
    let radios: Vec<LayoutField> = item.fields.iter().filter(|f| f.kind == "radio").cloned().collect();
    let mut row = view().flex_row().flex_wrap().items_end().gap(px(14.0));
    if !radios.is_empty() {
        row = row.child(radio_group(radios));
    }
    for field in item.fields.iter().filter(|f| f.kind != "radio") {
        row = row.child(field_control(field.clone(), &item.label, multi));
    }
    row.into_element()
}

/// A TIN item: three to five text boxes under a TIN label.
fn is_tin_item(item: &LayoutItem) -> bool {
    let named = item.label.contains("TIN") || item.label.contains("Identification");
    named && (3..=5).contains(&item.fields.len()) && item.fields.iter().all(|f| f.kind == "text" || f.kind == "int")
}

/// The TIN as one grouped control under the item's one label: a single box
/// that takes the digits and shows them 000-000-000-00000, each part going
/// to its own box of the form.
fn tin_boxes(item: &LayoutItem) -> impl IntoElement {
    super::phone_fields::tin_box(item.fields.clone())
}

fn control_width(kind: &str, max_len: Option<u32>) -> f32 {
    match kind {
        "money" => 210.0,
        "pct" => 96.0,
        "date" => 150.0,
        "int" if max_len.is_some_and(|m| m <= 4) => 90.0,
        "int" => 150.0,
        "sel" => 260.0,
        "text" if max_len.is_some_and(|m| m <= 4) => 90.0,
        _ => 320.0,
    }
}

pub(super) fn field_control(field: LayoutField, item_label: &str, multi: bool) -> AnyElement {
    if field.kind == "chk" {
        return check_field(field, item_label.to_owned()).into_element();
    }
    let st = session();
    let width = control_width(&field.kind, field.max_len);
    let sub = multi && !field.label.is_empty() && field.label != item_label && !field.label.chars().all(|c| c.is_ascii_alphanumeric() && c.is_ascii_digit());
    let key = field.key.clone();
    let off = move || st.off.with(|o| o.contains_key(&key));
    let control = if field.kind == "sel" {
        select_box(field.key.clone(), field.options.clone(), field.computed, width).into_element()
    } else {
        text_box(field.key.clone(), field.kind.clone(), field.whole, field.computed, width, field.max_len, field.pool.is_some()).into_element()
    };
    let wrapper = view().gap(px(3.0)).opacity(move || if off() { OFF_OPACITY } else { 1.0_f32 });
    if sub {
        wrapper.child(text(field.label.clone()).text_size(px(12.0)).text_color(theme::FG2)).child(control).into_element()
    } else {
        wrapper.child(control).into_element()
    }
}

// ---------------------------------------------------------------- boxes

/// A text, amount, number or date box over `key`: an input while it can be
/// typed into, its value (or why it is blank) while locked.
pub(super) fn text_box(key: String, kind: String, whole: bool, computed: bool, width: f32, max_len: Option<u32>, pool: bool) -> impl IntoElement {
    let st = session();
    let lock_key = key.clone();
    let k2 = key.clone();
    let kind2 = kind.clone();
    when(move || !st.is_locked(&lock_key, computed), move || entry(key.clone(), kind.clone(), whole, width, max_len, pool))
        .otherwise(move || locked_box(k2.clone(), kind2.clone(), whole, computed, width, pool))
}

pub(super) fn shown_text(kind: &str, whole: bool, value: &str) -> String {
    match kind {
        "money" if !value.trim().is_empty() => format_amount(value, whole),
        _ => value.to_owned(),
    }
}

fn entry(key: String, kind: String, whole: bool, width: f32, max_len: Option<u32>, pool: bool) -> AnyElement {
    let st = session();
    let read = {
        let key = key.clone();
        move || if pool { st.pool_text(&key) } else { st.value(&key) }
    };
    let kind_shown = kind.clone();
    let typed_text = signal(untracked(|| shown_text(&kind_shown, whole, &read())));
    let focused = signal(false);
    let date_error = signal(String::new());
    {
        // A refused edit: show the return's value again, not the typed text.
        let kind = kind.clone();
        let key = key.clone();
        let read = read.clone();
        effect(move || {
            let hit = st.reverted.with(|(_, keys)| keys.contains(&key));
            if hit {
                typed_text.set(shown_text(&kind, whole, &untracked(&read)));
            }
        });
    }
    {
        let kind = kind.clone();
        effect(move || {
            let value = read();
            if !focused.get_untracked() {
                typed_text.set(shown_text(&kind, whole, &value));
            }
        });
    }
    let numeric = matches!(kind.as_str(), "money" | "pct");
    let at = element_ref();
    let on_change = {
        let key = key.clone();
        let kind = kind.clone();
        move |v: &str| {
            let prev = typed_text.get_untracked();
            typed_text.set(v.to_owned());
            match kind.as_str() {
                "date" => {
                    // Slashes as the digits come, the caret after the same digit.
                    let shown = crate::typing::slash_date(v);
                    if shown != v {
                        let caret = crate::typing::caret_after(&prev, v, &shown, crate::typing::digit);
                        typed_text.set(shown.clone());
                        at.set_selection(caret, caret);
                    }
                    let v = shown.as_str();
                    if let Some(date) = parse_typed_date(v) {
                        date_error.set(String::new());
                        st.edit(key.clone(), FieldValue::Text(format_form_date(date)), false);
                    } else if v.trim().is_empty() {
                        date_error.set(String::new());
                        st.edit(key.clone(), FieldValue::Text(String::new()), false);
                    }
                }
                "money" | "pct" => {
                    let clean: String = v.chars().filter(|c| !matches!(c, ',' | '₱' | ' ')).collect();
                    // A phone groups the thousands as they are typed.
                    if kind == "money" && crate::mobile::phone() {
                        let grouped = live_commas(&clean);
                        if grouped != v {
                            // The caret stays after the digit just typed.
                            let caret = crate::typing::caret_after(&prev, v, &grouped, |c| c.is_ascii_digit() || c == '.' || c == '-');
                            typed_text.set(grouped);
                            at.set_selection(caret, caret);
                        }
                    }
                    st.edit(key.clone(), FieldValue::Text(clean), false);
                }
                _ => st.edit(key.clone(), FieldValue::Text(v.to_owned()), false),
            }
        }
    };
    let on_blur = {
        let kind = kind.clone();
        let blur_key = key.clone();
        move || {
            focused.set(false);
            let typed = typed_text.get_untracked();
            match kind.as_str() {
                "date" => match parse_typed_date(&typed) {
                    Some(date) => typed_text.set(format_form_date(date)),
                    None if !typed.trim().is_empty() => date_error.set(DATE_ERROR.to_owned()),
                    None => {}
                },
                "money" => typed_text.set(shown_text("money", whole, &typed)),
                _ => {}
            }
            st.touch_key(&blur_key);
            leave_box(blur_key.clone());
            task(async move { st.flush().await });
        }
    };
    let phone = crate::mobile::phone();
    st.inputs.borrow_mut().insert(key.clone(), at);
    let focus_key = key.clone();
    let mut field = input()
        .element_ref(&at)
        .value(typed_text)
        .on_change(on_change)
        .on_focus({
            let kind = kind.clone();
            move || {
                focused.set(true);
                // A zero amount clears for typing; the placeholder shows 0.00.
                if phone && kind == "money" && crate::screens::steps::amount(&typed_text.get_untracked()) == Some(0.0) {
                    typed_text.set(String::new());
                }
                enter_box(focus_key.clone());
            }
        })
        .on_blur(on_blur)
        .placeholder(match kind.as_str() {
            "date" => "MM/DD/YYYY",
            "money" if phone => "0.00",
            _ if phone => "Type here",
            _ => "",
        })
        .placeholder_color(|| color(theme::FG3))
        .auto_correct(false)
        .keyboard_type(match kind.as_str() {
            "money" | "pct" => KeyboardType::Decimal,
            "int" => KeyboardType::Number,
            _ => KeyboardType::Default,
        })
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .text_color(theme::FG);
    field = if phone {
        // The phone's stacked field: the value on the row, no box around it.
        field.flex_1().h(px(34.0)).text_size(px(if numeric { 18.0 } else { 17.0 })).bg(CLEAR)
    } else {
        field
            .w(if width > 0.0 { Length::from(px(if numeric { width - 18.0 } else { width })) } else { Length::from(relative(1.0)) })
            .h(px(theme::CONTROL))
            .px(px(10.0))
            .text_size(px(13.0))
            .bg(theme::SURFACE)
            .border_1()
            .border_color(theme::BORDER_STRONG)
            .rounded(px(theme::R_ROW))
    };
    if numeric {
        field = field.text_right().tabular_nums();
        if phone {
            field = field.font_family(amount_font());
        }
    }
    if let Some(max) = max_len.filter(|_| matches!(kind.as_str(), "text" | "int")) {
        field = field.max_length(max);
    }
    let (sign, sign_size) = if phone { (15.0, 15.0) } else { (12.0, 12.0) };
    let boxed: AnyElement = match kind.as_str() {
        "money" => view()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .w(if phone { Length::from(relative(1.0)) } else { Length::Auto })
            .child(text("₱").w(px(sign)).text_size(px(sign_size)).font_family(mono()).text_color(theme::FG3))
            .child(field)
            .into_element(),
        "pct" => view().flex_row().items_center().gap(px(4.0)).w(if phone { Length::from(relative(1.0)) } else { Length::Auto }).child(field).child(text("%").w(px(12.0)).text_size(px(sign_size)).text_color(theme::FG3)).into_element(),
        _ => field.into_element(),
    };
    if kind != "date" {
        return boxed.into_element();
    }
    view()
        .gap(px(2.0))
        .child(boxed)
        .child(when(move || !date_error.get().is_empty(), move || text(move || date_error.get()).max_w(px(320.0)).text_size(px(11.0)).text_color(theme::DANGER)))
        .into_element()
}

fn locked_box(key: String, kind: String, whole: bool, computed: bool, width: f32, pool: bool) -> AnyElement {
    let st = session();
    let numeric = matches!(kind.as_str(), "money" | "pct");
    let shown = {
        let key = key.clone();
        move || {
            let value = if pool { st.pool_text(&key) } else { st.value(&key) };
            let zero_ok = computed || st.penalties.with(|p| p.iter().any(|g| g.keys.contains(&key)));
            let value = if kind == "money" && value.trim().is_empty() && zero_ok { "0".to_owned() } else { value };
            if value.trim().is_empty() { (st.reason(&key), true) } else { (shown_text(&kind, whole, &value), false) }
        }
    };
    let hint = {
        let shown = shown.clone();
        move || shown().1
    };
    let hint2 = hint.clone();
    let (hint3, hint4) = (hint.clone(), hint.clone());
    let shown_phone = shown.clone();
    let shown = shown_phone;
    let mut label = text({
        let shown = shown.clone();
        move || shown().0
    })
        .text_size(move || px(if hint2() { 11.0 } else { 13.0 }))
        .text_color(move || if hint() { theme::FG3 } else { theme::FG })
        .truncate();
    if crate::mobile::phone() {
        // The kit's read-only row: the value as plain text (FG, amounts
        // in the amount face), or why it is blank in FG3; no box.
        let shown2 = shown.clone();
        let mut value = text(move || shown2().0)
            .text_size(move || px(if hint3() { 13.0 } else if numeric { 18.0 } else { 17.0 }))
            .text_color(move || if hint4() { theme::FG3 } else { theme::FG })
            .number_of_lines(2u32);
        if numeric {
            value = value.text_right().tabular_nums().font_family(amount_font());
        }
        return view().w_full().min_h(px(34.0)).justify_center().child(value).into_element();
    }
    if numeric {
        label = label.text_right().tabular_nums();
    }
    view()
        .w(if width > 0.0 { Length::from(px(width)) } else { Length::from(relative(1.0)) })
        .min_h(px(theme::CONTROL))
        .px(px(10.0))
        .justify_center()
        .rounded(px(theme::R_ROW))
        .bg(theme::BG)
        .child(label)
        .into_element()
}

fn tick(on: impl Fn() -> bool + 'static, locked: impl Fn() -> bool + 'static) -> impl IntoElement {
    let on = std::rc::Rc::new(on);
    let on2 = on.clone();
    let on3 = on.clone();
    view()
        .size(px(16.0))
        .rounded(px(4.0))
        .items_center()
        .justify_center()
        .flex_shrink(0.0)
        .border_1()
        .border_color(move || if on() { theme::ACCENT } else { theme::BORDER_STRONG })
        .bg(move || if on2() { theme::ACCENT } else { theme::SURFACE })
        .opacity(move || if locked() { 0.6 } else { 1.0_f32 })
        .child(text(move || if on3() { "✓" } else { "" }).text_size(px(11.0)).font_weight(FontWeight::BOLD).text_color(theme::PRIMARY_FG))
}

fn check_field(field: LayoutField, item_label: String) -> impl IntoElement {
    let st = session();
    let key = field.key.clone();
    let (k1, k2, k3) = (key.clone(), key.clone(), key.clone());
    let computed = field.computed;
    let label = if field.label.is_empty() { item_label } else { field.label.clone() };
    view()
        .flex_row()
        .items_center()
        .gap(px(7.0))
        .cursor_pointer()
        .opacity(move || if st.off.with(|o| o.contains_key(&k3)) { OFF_OPACITY } else { 1.0_f32 })
        .on_press(move || {
            if !st.is_locked(&key, computed) {
                let next = if st.is_ticked(&key) { "false" } else { "true" };
                st.choose(vec![(key.clone(), next.to_owned())]);
            }
        })
        .child(tick(move || st.is_ticked(&k1), move || st.is_locked(&k2, computed)))
        .child(text(label).text_size(px(13.0)).text_color(theme::FG))
}

/// The label of a choice without the item's prefix (`Filer type: Single`).
pub(super) fn choice_label(field: &LayoutField) -> String {
    match field.label.split_once(": ") {
        Some((_, rest)) => rest.to_owned(),
        None => field.label.clone(),
    }
}

/// A one-answer group drawn as the paper form's tick boxes.
pub(super) fn radio_group(radios: Vec<LayoutField>) -> impl IntoElement {
    let st = session();
    let all: Vec<(String, Option<String>)> = radios.iter().map(|f| (f.key.clone(), f.group.clone())).collect();
    let mut group = view().w_full().flex_row().flex_wrap().gap(px(6.0));
    for f in &radios {
        let key = f.key.clone();
        let computed = f.computed;
        let rg = f.group_size;
        let grp = f.group.clone();
        let all = all.clone();
        let (k1, k2, k3, k4, k5) = (key.clone(), key.clone(), key.clone(), key.clone(), key.clone());
        group = group.child(
            view()
                .min_h(px(28.0))
                .max_w(relative(1.0))
                .px(px(12.0))
                .py(px(4.0))
                .flex_row()
                .items_center()
                .rounded(px(14.0))
                .border_1()
                .border_color(move || if st.is_ticked(&k2) { theme::FG2 } else { theme::BORDER_STRONG })
                .bg(move || if st.is_ticked(&k5) { theme::SELECTED } else { theme::SURFACE })
                .cursor_pointer()
                .hover(|s| s.bg(theme::HOVER))
                .opacity(move || if st.off.with(|o| o.contains_key(&k4)) { OFF_OPACITY } else { 1.0_f32 })
                .on_press(move || {
                    if st.is_locked(&key, computed) {
                        return;
                    }
                    let on = st.is_ticked(&key);
                    if on {
                        // A group of three or more clears on a second press;
                        // a yes/no pair keeps its answer.
                        if rg.unwrap_or(0) >= 3 {
                            st.choose(vec![(key.clone(), "false".to_owned())]);
                        }
                        return;
                    }
                    let mut edits = vec![(key.clone(), "true".to_owned())];
                    if let Some(grp) = &grp {
                        for (other, other_grp) in &all {
                            if other != &key && other_grp.as_ref() == Some(grp) {
                                edits.push((other.clone(), "false".to_owned()));
                            }
                        }
                    }
                    st.choose(edits);
                })
                .child(text(choice_label(f)).flex_shrink(1.0).min_w(px(0.0)).text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(move || {
                    if st.is_ticked(&k1) { theme::FG } else if st.is_locked(&k3, computed) { theme::FG3 } else { theme::FG2 }
                })),
        );
    }
    let keys: Vec<(String, bool)> = radios.iter().map(|f| (f.key.clone(), f.computed)).collect();
    let hint = move || {
        let shut = keys.iter().all(|(k, c)| st.is_locked(k, *c)) && !keys.iter().any(|(k, _)| st.is_ticked(k));
        if shut { keys.iter().map(|(k, _)| st.reason(k)).find(|r| !r.is_empty()).unwrap_or_default() } else { String::new() }
    };
    let hint2 = hint.clone();
    view().w_full().gap(px(4.0)).child(group).child(when(move || !hint2().is_empty(), move || text(hint.clone()).text_size(px(11.0)).text_color(theme::FG3)))
}

/// A drop-down: the return's catalog choices, else the layout's.
pub(super) fn select_box(key: String, listed: Vec<FieldOption>, computed: bool, width: f32) -> impl IntoElement {
    let st = session();
    let open = signal(false);
    let query = signal(String::new());
    let options = {
        let key = key.clone();
        move || {
            let mut opts = st.options.with(|o| o.get(&key).cloned()).filter(|o| !o.is_empty()).unwrap_or_else(|| listed.clone());
            opts.insert(0, FieldOption { value: String::new(), label: "Choose…".to_owned(), selected: None });
            opts
        }
    };
    let current = {
        let key = key.clone();
        let options = options.clone();
        move || {
            let opts = options();
            let value = st.value(&key);
            let chosen = opts.iter().find(|o| o.selected == Some(true)).or_else(|| opts.iter().find(|o| o.value == value && !value.is_empty()));
            match chosen {
                Some(o) => o.label.clone(),
                None if value.is_empty() || value.chars().all(|c| c == '0' || c == '-') => String::new(),
                None => value,
            }
        }
    };
    let lock_key = key.clone();
    let locked = move || st.is_locked(&lock_key, computed);
    let reason_key = key.clone();
    let current_locked = current.clone();
    let locked_view = move || {
        let current = current_locked.clone();
        let reason_key = reason_key.clone();
        let shown = move || {
            let c = current();
            if c.is_empty() { st.reason(&reason_key) } else { c }
        };
        if crate::mobile::phone() {
            return view().w_full().min_h(px(34.0)).justify_center().child(text(shown).text_size(px(17.0)).text_color(theme::FG).number_of_lines(2u32)).into_element();
        }
        view().w(if width > 0.0 { Length::from(px(width)) } else { Length::from(relative(1.0)) }).min_h(px(28.0)).px(px(8.0)).justify_center().rounded(px(5.0)).bg(theme::BG).child(text(shown).text_size(px(13.0)).text_color(theme::FG).truncate()).into_element()
    };
    let editable_view = move || {
        let current = current.clone();
        let options = options.clone();
        let key = key.clone();
        let options_f = options.clone();
        let filtered = move || {
            let q = query.get().trim().to_lowercase();
            options_f().into_iter().filter(|o| q.is_empty() || o.label.to_lowercase().contains(&q)).collect::<Vec<_>>()
        };
        let many = {
            let options = options.clone();
            move || options().len() > 12
        };
        view()
            .w(if width > 0.0 { Length::from(px(width)) } else { Length::from(relative(1.0)) })
            .gap(px(4.0))
            .child(
                view()
                    .h(px(28.0))
                    .px(px(8.0))
                    .flex_row()
                    .items_center()
                    .rounded(px(5.0))
                    .border_1()
                    .border_color(theme::BORDER_STRONG)
                    .bg(theme::SURFACE)
                    .cursor_pointer()
                    .on_press(move || {
                        query.set(String::new());
                        open.update(|o| *o = !*o);
                    })
                    .child({
                        let current = current.clone();
                        let current2 = current.clone();
                        text(move || {
                            let c = current();
                            if c.is_empty() { "Choose…".to_owned() } else { c }
                        })
                        .flex_1()
                        .flex_shrink(1.0)
                        .min_w(px(0.0))
                        .text_size(px(13.0))
                        .text_color(move || if current2().is_empty() { theme::FG3 } else { theme::FG })
                        .truncate()
                    })
                    .child(text(move || if open.get() { "▴" } else { "▾" }).text_size(px(11.0)).text_color(theme::FG2)),
            )
            .child(when(move || open.get(), move || {
                let key = key.clone();
                let filtered = filtered.clone();
                view()
                    .rounded(px(theme::R_ROW))
                    .border_1()
                    .border_color(theme::BORDER)
                    .bg(theme::RAISED)
                    .shadow_md()
                    .p(px(4.0))
                    .gap(px(2.0))
                    .child(when(many.clone(), move || {
                        input()
                            .value(query)
                            .on_change(move |v: &str| query.set(v.to_owned()))
                            .placeholder("Find…")
                            .auto_focus(true)
                            .h(px(26.0))
                            .px(px(6.0))
                            .text_size(px(12.0))
                            .text_color(theme::FG)
                            .bg(theme::SURFACE)
                            .border_1()
                            .border_color(theme::BORDER_STRONG)
                            .rounded(px(4.0))
                    }))
                    .child(scroll().max_h(px(260.0)).child(view().child(each(
                        filtered,
                        |o| o.value.clone(),
                        move |o, _| {
                            let o = o.get_untracked();
                            let key = key.clone();
                            let value = o.value.clone();
                            view()
                                .px(px(8.0))
                                .py(px(5.0))
                                .rounded(px(4.0))
                                .cursor_pointer()
                                .hover(|s| s.bg(theme::BG))
                                .on_press(move || {
                                    open.set(false);
                                    st.choose(vec![(key.clone(), value.clone())]);
                                })
                                .child(text(o.label.clone()).text_size(px(12.0)).text_color(theme::FG))
                        },
                    ))))
            }))
    };
    when(move || !locked(), editable_view).otherwise(locked_view)
}

// ---------------------------------------------------------------- dates

/// Month, day and year boxes typed as one date.
#[derive(Clone)]
pub(super) struct DateParts {
    pub(super) month: LayoutField,
    pub(super) day: Option<LayoutField>,
    pub(super) year: LayoutField,
}

pub(super) fn date_parts(fields: &[LayoutField]) -> Option<DateParts> {
    let find = |want: &[&str], not: &[&str]| {
        fields
            .iter()
            .find(|f| {
                let text = format!("{} {}", f.label, f.key.rsplit(':').next().unwrap_or("")).to_lowercase();
                f.kind != "radio" && want.iter().any(|w| text.contains(w)) && !not.iter().any(|n| text.contains(n))
            })
            .cloned()
    };
    let month = find(&["month", "mm"], &["months"])?;
    let year = find(&["year", "yyyy"], &["month", "years"])?;
    let day = find(&["day", "dd"], &["days", "month", "year"]).filter(|d| d.key != month.key && d.key != year.key);
    (month.key != year.key).then_some(DateParts { month, day, year })
}

pub(super) fn one_date(parts: DateParts) -> impl IntoElement {
    let st = session();
    let keys: Vec<String> = [Some(&parts.month), parts.day.as_ref(), Some(&parts.year)].into_iter().flatten().map(|f| f.key.clone()).collect();
    let computed = parts.month.computed;
    let joined = {
        let parts = parts.clone();
        move || {
            let m = st.value(&parts.month.key);
            let d = parts.day.as_ref().map(|f| st.value(&f.key)).unwrap_or_default();
            let y = st.value(&parts.year.key);
            if m.trim().is_empty() && d.trim().is_empty() && y.trim().is_empty() { String::new() } else { format!("{m}/{d}/{y}") }
        }
    };
    let locked = {
        let keys = keys.clone();
        move || keys.iter().all(|k| st.is_locked(k, computed))
    };
    let joined_locked = joined.clone();
    let locked_view = move || {
        let j = joined_locked.clone();
        view().w(px(150.0)).min_h(px(28.0)).px(px(8.0)).justify_center().rounded(px(5.0)).bg(theme::BG).child(text(j).text_size(px(13.0)).text_color(theme::FG))
    };
    let editable_view = move || {
        let joined = joined.clone();
        let parts = parts.clone();
        let text_sig = signal(untracked(|| joined()));
        let date_at = element_ref();
        let date_key = parts.month.key.clone();
        let blur_date_key = date_key.clone();
        st.inputs.borrow_mut().insert(date_key.clone(), date_at);
        let focused = signal(false);
        let error = signal(String::new());
        effect(move || {
            let v = joined();
            if !focused.get_untracked() {
                text_sig.set(v);
            }
        });
        let write = move |date: Option<chrono_parts::Mdy>| {
            let (m, d, y) = date.map_or((String::new(), String::new(), String::new()), |p| (p.0, p.1, p.2));
            let mut edits = vec![(parts.month.key.clone(), m), (parts.year.key.clone(), y)];
            if let Some(day) = &parts.day {
                edits.push((day.key.clone(), d));
            }
            for (k, v) in edits {
                st.edit(k, FieldValue::Text(v), false);
            }
        };
        let write2 = write.clone();
        view()
            .gap(px(2.0))
            .child(
                input()
                    .value(text_sig)
                    .on_change(move |v: &str| {
                        let prev = text_sig.get_untracked();
                        let shown = crate::typing::slash_date(v);
                        text_sig.set(shown.clone());
                        if shown != v {
                            let caret = crate::typing::caret_after(&prev, v, &shown, crate::typing::digit);
                            date_at.set_selection(caret, caret);
                        }
                        let v = shown.as_str();
                        if let Some(date) = parse_typed_date(v) {
                            error.set(String::new());
                            write(Some(chrono_parts::split(date)));
                        } else if v.trim().is_empty() {
                            error.set(String::new());
                            write2(None);
                        }
                    })
                    .element_ref(&date_at)
                    .on_focus({
                        let k = date_key.clone();
                        move || {
                            focused.set(true);
                            enter_box(k.clone());
                        }
                    })
                    .on_blur(move || {
                        focused.set(false);
                        st.touch_key(&blur_date_key);
                        leave_box(blur_date_key.clone());
                        let typed = text_sig.get_untracked();
                        match parse_typed_date(&typed) {
                            Some(date) => text_sig.set(format_form_date(date)),
                            None if !typed.trim().is_empty() => error.set(DATE_ERROR.to_owned()),
                            None => {}
                        }
                        task(async move { st.flush().await });
                    })
                    .placeholder("MM/DD/YYYY")
                    .placeholder_color(|| color(theme::FG3))
                    .w(px(150.0))
                    .h(px(28.0))
                    .px(px(8.0))
                    .text_size(px(13.0))
                    .text_color(theme::FG)
                    .bg(theme::SURFACE)
                    .border_1()
                    .border_color(theme::BORDER_STRONG)
                    .rounded(px(5.0)),
            )
            .child(when(move || !error.get().is_empty(), move || text(move || error.get()).text_size(px(11.0)).text_color(theme::DANGER)))
    };
    when(move || !locked(), editable_view).otherwise(locked_view)
}

mod chrono_parts {
    /// A date's `MM`, `DD` and `YYYY` boxes.
    #[derive(Clone)]
    pub(super) struct Mdy(pub String, pub String, pub String);

    pub(super) fn split(date: impl std::fmt::Display) -> Mdy {
        // `NaiveDate` displays as `YYYY-MM-DD`.
        let text = date.to_string();
        let mut parts = text.split('-');
        let y = parts.next().unwrap_or_default().to_owned();
        let m = parts.next().unwrap_or_default().to_owned();
        let d = parts.next().unwrap_or_default().to_owned();
        Mdy(m, d, y)
    }
}

// ---------------------------------------------------------------- schedules

/// A schedule's column labels; its rows follow as list rows
/// ([`cells_row`]). Columns share the width, so every row lines up
/// without a sideways scroller.
pub(super) fn schedule_head(item: &LayoutItem) -> impl IntoElement {
    let mut head = view().flex_row().items_end().gap(px(8.0)).pb(px(2.0));
    if item.row_labels.is_none() || item.numbered != Some(false) {
        head = head.child(text("#").w(px(28.0)).text_size(px(12.0)).text_color(theme::FG3));
    }
    if let Some(labels) = &item.row_labels {
        head = head.child(text(labels.caption.clone()).flex_1().min_w(px(0.0)).text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(theme::FG2));
    }
    for col in item.cols.iter().filter(|c| c.kind != "hidden") {
        head = head.child(text(col.label.clone()).flex_1().min_w(px(0.0)).text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(theme::FG2));
    }
    head
}

/// A column's width in the desktop table, by its kind.
fn col_width(kind: &str, label: &str) -> f32 {
    match kind {
        "money" => 170.0,
        "pct" => 90.0,
        "int" => 110.0,
        "date" => 140.0,
        "radio" | "chk" => 64.0,
        "sel" => 220.0,
        _ => (label.chars().count() as f32 * 7.0).clamp(140.0, 240.0),
    }
}

/// Rows a schedule table adds per display frame while it mounts.
const TABLE_ROWS_PER_FRAME: usize = 4;

const TABLE_ROW_H: f32 = 40.0;
const TABLE_HEAD_H: f32 = 34.0;

/// A schedule on the desktop (DESIGN_SYSTEM "Form page"): one table whose
/// columns keep their widths and scroll sideways inside their own box, the
/// first column (the row number or the row's label) pinned at the left. Its
/// rows mount a few per frame, so a 40-row schedule never stalls one.
fn schedule_table(form: Arc<OpenForm>, id: ItemId) -> impl IntoElement {
    let st = session();
    let Some(item) = form.item(id).cloned() else { return view().into_element() };
    let cols: Vec<usize> = item.cols.iter().enumerate().filter(|(_, c)| c.kind != "hidden").map(|(ci, _)| ci).collect();
    let n = item.keys.len();
    let mounted = signal(n.min(8));
    fn grow(mounted: Signal<usize>, n: usize) {
        if mounted.get_untracked() >= n {
            return;
        }
        request_animation_frame(move |_| {
            mounted.update(|m| *m = (*m + TABLE_ROWS_PER_FRAME).min(n));
            session().table_steps.update(|s| *s += 1);
            grow(mounted, n);
        });
    }
    grow(mounted, n);
    let rows = move || (0..mounted.get()).collect::<Vec<usize>>();
    let labelled = item.row_labels.is_some();
    let numbered = item.row_labels.is_none() || item.numbered != Some(false);
    let pin_w = if labelled { 200.0 } else { 44.0 };
    let head_cell = |label: String, w: f32| {
        view().w(px(w)).h(px(TABLE_HEAD_H)).px(px(8.0)).justify_center().flex_shrink(0.0).child(text(label).text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(theme::FG2).number_of_lines(2u32))
    };
    // The pinned column.
    let pin_item = item.clone();
    let pinned = view()
        .w(px(pin_w))
        .flex_shrink(0.0)
        .border_r(px(1.0))
        .border_color(theme::BORDER)
        .bg(theme::SURFACE)
        .child(head_cell(item.row_labels.as_ref().map(|l| l.caption.clone()).unwrap_or_else(|| "#".to_owned()), pin_w))
        .child(each(rows.clone(), |r| *r, move |r, _| {
            let r = r.get_untracked();
            let label = match &pin_item.row_labels {
                Some(l) => {
                    let name = l.rows.get(r).cloned().unwrap_or_default();
                    if numbered { format!("{} {name}", pin_item.start.unwrap_or(1) as usize + r) } else { name }
                }
                None => (pin_item.start.unwrap_or(1) as usize + r).to_string(),
            };
            view()
                .h(px(TABLE_ROW_H))
                .px(px(8.0))
                .justify_center()
                .border_t(px(1.0))
                .border_color(theme::BORDER)
                .child(text(label).text_size(px(12.0)).tabular_nums().text_color(theme::FG2).number_of_lines(2u32))
        }));
    // The columns that scroll.
    let total: f32 = cols.iter().map(|ci| col_width(&item.cols[*ci].kind, &item.cols[*ci].label)).sum::<f32>() + 8.0 * cols.len() as f32;
    let mut head = view().flex_row().gap(px(8.0)).h(px(TABLE_HEAD_H));
    for ci in &cols {
        let c = &item.cols[*ci];
        head = head.child(head_cell(c.label.clone(), col_width(&c.kind, &c.label)));
    }
    let cell_item = item.clone();
    let cell_cols = cols.clone();
    let body = view().w(px(total)).child(head).child(each(rows, |r| *r, move |r, _| {
        let r = r.get_untracked();
        let mut row = view().flex_row().items_center().gap(px(8.0)).h(px(TABLE_ROW_H)).border_t(px(1.0)).border_color(theme::BORDER);
        let keys = cell_item.keys.get(r).cloned().unwrap_or_default();
        for ci in &cell_cols {
            let c = &cell_item.cols[*ci];
            let Some(key) = keys.get(*ci).cloned() else { continue };
            let w = col_width(&c.kind, &c.label);
            let cell: AnyElement = match c.kind.as_str() {
                "radio" | "chk" => {
                    let (k1, k2, k3) = (key.clone(), key.clone(), key.clone());
                    let computed = c.computed;
                    view()
                        .cursor_pointer()
                        .on_press(move || {
                            if !st.is_locked(&k3, computed) {
                                let next = if st.is_ticked(&k3) { "false" } else { "true" };
                                st.choose(vec![(k3.clone(), next.to_owned())]);
                            }
                        })
                        .child(tick(move || st.is_ticked(&k1), move || st.is_locked(&k2, computed)))
                        .into_element()
                }
                "sel" => select_box(key, c.options.clone(), c.computed, w).into_element(),
                kind => text_box(key, kind.to_owned(), c.whole, c.computed, w, None, false).into_element(),
            };
            row = row.child(view().w(px(w)).flex_shrink(0.0).child(cell));
        }
        row
    }));
    view()
        .flex_row()
        .mt(px(4.0))
        .rounded(px(theme::R_ROW))
        .border_1()
        .border_color(theme::BORDER)
        .overflow_hidden()
        .child(pinned)
        .child(scroll().horizontal().flex_1().min_w(px(0.0)).child(view().px(px(8.0)).child(body)))
        .into_element()
}
