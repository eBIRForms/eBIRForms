//! The design system's wrapper components (DESIGN_SYSTEM.md "Wrapper
//! components"): screens build from these and never pick sizes by eye.
//! Every control carries a `label` for screen readers and gpui-agent.

use nsh_base::input;
use nsh_core::prelude::*;

use crate::icons::{Icon, icon};
use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Variant {
    /// Inverted ink: the one primary action.
    Primary,
    Secondary,
    Ghost,
}

/// `AppButton`: 32 tall (28 for `sm`), label 13/500, radius 6.
fn app_button(
    label: impl IntoReactive<SharedString>,
    variant: Variant,
    sm: bool,
    leading: Option<Icon>,
    disabled: impl Fn() -> bool + 'static,
    on_press: impl Fn() + 'static,
) -> View {
    let disabled = std::rc::Rc::new(disabled);
    let off = disabled.clone();
    let (bg, border, fg) = match variant {
        Variant::Primary => (theme::PRIMARY, theme::PRIMARY, theme::PRIMARY_FG),
        Variant::Secondary => (theme::SURFACE, theme::BORDER_STRONG, theme::FG),
        Variant::Ghost => (theme::tone(theme::hsla(0.0, 0.0, 0.0, 0.0), theme::hsla(0.0, 0.0, 0.0, 0.0)), theme::tone(theme::hsla(0.0, 0.0, 0.0, 0.0), theme::hsla(0.0, 0.0, 0.0, 0.0)), theme::FG),
    };
    let mut row = view()
        .h(px(if sm { theme::CONTROL_SM } else { theme::CONTROL }))
        .px(px(if sm { 10.0 } else { 12.0 }))
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .rounded(px(theme::R_CTL))
        .flex_shrink(0.0)
        .cursor_pointer()
        .bg(bg)
        .border_1()
        .border_color(border)
        .opacity(move || if off() { 0.45 } else { 1.0_f32 })
        .hover(move |s| if variant == Variant::Ghost || variant == Variant::Secondary { s.bg(theme::HOVER) } else { s.opacity(0.88) })
        .active(|s| s.opacity(0.7))
        .on_press(move || {
            if !disabled() {
                on_press();
            }
        });
    if let Some(i) = leading {
        row = row.child(icon(i, 16.0, move || fg));
    }
    row.child(text(label).text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(fg))
}

/// A 32 px button.
pub(crate) fn button(
    label: impl IntoReactive<SharedString>,
    variant: Variant,
    disabled: impl Fn() -> bool + 'static,
    on_press: impl Fn() + 'static,
) -> View {
    app_button(label, variant, false, None, disabled, on_press)
}

/// A 28 px button with an optional leading icon, for toolbars and dense
/// rows.
pub(crate) fn button_sm(
    label: impl IntoReactive<SharedString>,
    variant: Variant,
    leading: Option<Icon>,
    disabled: impl Fn() -> bool + 'static,
    on_press: impl Fn() + 'static,
) -> View {
    app_button(label, variant, true, leading, disabled, on_press)
}

/// `IconButton`: 28 × 28, radius 6, icon 16 in fg-2, `selected` fill
/// while `pressed`.
pub(crate) fn icon_button(
    i: Icon,
    label: &str,
    pressed: impl Fn() -> bool + 'static,
    on_press: impl Fn() + 'static,
) -> View {
    view()
        .size(px(theme::CONTROL_SM))
        .items_center()
        .justify_center()
        .rounded(px(theme::R_CTL))
        .flex_shrink(0.0)
        .cursor_pointer()
        .label(label.to_owned())
        .bg(move || if pressed() { theme::SELECTED } else { theme::tone(theme::hsla(0.0, 0.0, 0.0, 0.0), theme::hsla(0.0, 0.0, 0.0, 0.0)) })
        .hover(|s| s.bg(theme::HOVER))
        .active(|s| s.bg(theme::SELECTED))
        .on_press(on_press)
        .child(icon(i, 16.0, || theme::FG2))
}

/// `NavRow`: 32 tall, padding 0 8, gap 10, radius 8.
pub(crate) fn nav_row(
    i: Option<Icon>,
    label: impl IntoReactive<SharedString>,
    current: impl Fn() -> bool + 'static,
    on_press: impl Fn() + 'static,
) -> View {
    let current = std::rc::Rc::new(current);
    let c2 = current.clone();
    let mut row = view()
        .h(px(theme::CONTROL))
        .px(px(8.0))
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .rounded(px(theme::R_ROW))
        .cursor_pointer()
        .bg(move || if current() { theme::SELECTED } else { theme::tone(theme::hsla(0.0, 0.0, 0.0, 0.0), theme::hsla(0.0, 0.0, 0.0, 0.0)) })
        .hover(|s| s.bg(theme::HOVER))
        .on_press(on_press);
    if let Some(i) = i {
        let c3 = c2.clone();
        row = row.child(icon(i, 16.0, move || if c3() { theme::FG } else { theme::FG2 }));
    }
    row.child(
        text(label)
            .flex_1()
            .flex_shrink(1.0)
            .min_w(px(0.0))
            .text_size(px(13.0))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme::FG)
            .truncate(),
    )
}

/// `GroupLabel`: a 12/500 fg-3 heading over a group of rows (sentence
/// case).
pub(crate) fn group_label(label: impl IntoReactive<SharedString>) -> Text {
    text(label).px(px(8.0)).pt(px(16.0)).pb(px(4.0)).text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(theme::FG3)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Neutral,
    Accent,
    Success,
    Warning,
    Danger,
}

/// `Chip` / `StatusChip`: 20 tall, 11/500, soft tint, full radius.
pub(crate) fn chip(label: impl IntoReactive<SharedString>, tone: Tone) -> View {
    let (bg, fg) = match tone {
        Tone::Neutral => (theme::HOVER, theme::FG2),
        Tone::Accent => (theme::ACCENT_SOFT, theme::ACCENT),
        Tone::Success => (theme::SUCCESS_SOFT, theme::SUCCESS),
        Tone::Warning => (theme::WARNING_SOFT, theme::WARNING),
        Tone::Danger => (theme::DANGER_SOFT, theme::DANGER),
    };
    view()
        .h(px(20.0))
        .px(px(8.0))
        .rounded(px(theme::R_FULL))
        .justify_center()
        .flex_shrink(0.0)
        .bg(bg)
        .child(text(label).text_size(px(11.0)).font_weight(FontWeight::MEDIUM).text_color(fg))
}

/// The status chip of a return.
pub(crate) fn status_tone(status: &str) -> Tone {
    match status {
        "Draft" => Tone::Accent,
        "Queued" | "Sending" => Tone::Warning,
        "Failed" | "Needs attention" => Tone::Danger,
        "Not started" => Tone::Neutral,
        _ => Tone::Success,
    }
}

/// `Avatar::initials`: the first and last capitalised words, hue from the
/// taxpayer id.
pub(crate) fn avatar(name: &str, id: &str, size: f32) -> View {
    let words: Vec<&str> = name.split_whitespace().filter(|w| w.chars().next().is_some_and(char::is_uppercase)).collect();
    let initials: String = match (words.first(), words.last()) {
        (Some(a), Some(b)) if words.len() > 1 => format!("{}{}", a.chars().next().unwrap_or(' '), b.chars().next().unwrap_or(' ')),
        (Some(a), _) => a.chars().take(2).collect(),
        _ => name.chars().take(1).collect::<String>().to_uppercase(),
    };
    let hue = (id.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) % 360) as f32 / 360.0;
    let bg = theme::tone(theme::hsla(hue, 0.55, 0.90, 1.0), theme::hsla(hue, 0.40, 0.26, 1.0));
    let fg = theme::tone(theme::hsla(hue, 0.55, 0.30, 1.0), theme::hsla(hue, 0.40, 0.84, 1.0));
    view()
        .size(px(size))
        .rounded(px(theme::R_FULL))
        .items_center()
        .justify_center()
        .flex_shrink(0.0)
        .bg(bg)
        .child(text(initials.to_uppercase()).text_size(px((size * 0.4).max(9.0))).font_weight(FontWeight::SEMIBOLD).text_color(fg))
}

/// A bordered 32 px input over `value` (`TextField`): padding 0 10,
/// radius 8.
pub(crate) fn field_input(value: Signal<String>, placeholder: &str) -> Input {
    field_box(placeholder).value(value).on_change(move |v: &str| value.set(v.to_owned()))
}

/// [`field_input`]'s look with no value bound: the caller binds it.
pub(crate) fn field_box(placeholder: &str) -> Input {
    input()
        .placeholder(placeholder.to_owned())
        .placeholder_color(|| color(theme::FG3))
        .auto_correct(false)
        .h(px(theme::CONTROL))
        .px(px(10.0))
        .text_size(px(13.0))
        .text_color(theme::FG)
        .bg(theme::SURFACE)
        .border_1()
        .border_color(theme::BORDER_STRONG)
        .rounded(px(theme::R_ROW))
}

/// A 12 px fg-2 field label.
pub(crate) fn caption(label: impl IntoReactive<SharedString>) -> Text {
    text(label).text_size(px(12.0)).text_color(theme::FG2)
}

/// `SidePanel`'s 40 px header: a 14/600 title, no close button.
pub(crate) fn panel_header(title: impl IntoReactive<SharedString>) -> View {
    view()
        .h(px(40.0))
        .px(px(16.0))
        .flex_row()
        .items_center()
        .child(text(title).text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
}

/// The transparent color, for fills that come and go.
pub(crate) const CLEAR: AdaptiveColor = theme::tone(theme::hsla(0.0, 0.0, 0.0, 0.0), theme::hsla(0.0, 0.0, 0.0, 0.0));

/// `ListGroup`: rows in one bordered group, radius 12, hairlines between.
pub(crate) fn list_group() -> View {
    view().rounded(px(theme::R_CARD)).border_1().border_color(theme::BORDER).bg(theme::SURFACE).overflow_hidden()
}

/// `ListRow`: at least 52 tall, padding 8 × 16, a hairline above all but
/// the first (`index`).
pub(crate) fn list_row(index: ReadSignal<usize>) -> View {
    view()
        .flex_row()
        .items_center()
        .gap(px(12.0))
        .min_h(px(52.0))
        .px(px(16.0))
        .py(px(8.0))
        .border_t(move || px(if index.get() == 0 { 0.0 } else { 1.0_f32 }))
        .border_color(theme::BORDER)
}

/// `SectionTitle`: 12/500 fg-3 over a group (28 above unless `first`).
pub(crate) fn section_title(label: impl IntoReactive<SharedString>, first: bool) -> Text {
    text(label).pt(px(if first { 0.0 } else { 20.0 })).pb(px(6.0)).text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(theme::FG3)
}

/// One row saying a group is empty, and why.
pub(crate) fn empty_row(line: impl IntoReactive<SharedString>) -> View {
    view().min_h(px(52.0)).px(px(16.0)).justify_center().child(text(line).text_size(px(13.0)).text_color(theme::FG3))
}

/// A form code as the design writes it: 12/600 with tabular figures, the UI
/// face (never monospace).
pub(crate) fn code_text(code: impl IntoReactive<SharedString>) -> Text {
    text(code).w(px(68.0)).flex_shrink(0.0).text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).tabular_nums().text_color(theme::FG)
}

/// `NameCell`: a 13/500 line over a 12 fg-2 line, both truncating.
pub(crate) fn name_cell(title: impl IntoReactive<SharedString>, line: impl IntoReactive<SharedString>) -> View {
    view()
        .flex_1()
        .min_w(px(0.0))
        .child(text(title).text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(theme::FG).truncate())
        .child(text(line).text_size(px(12.0)).text_color(theme::FG2).truncate())
}

/// `Segmented`: a 2 px track of 24 px segments, 12/500; `on` is the
/// current value's index.
pub(crate) fn segmented(labels: Vec<String>, on: impl Fn() -> usize + Clone + 'static, pick: impl Fn(usize) + Clone + 'static) -> View {
    let mut track = view().flex_row().p(px(2.0)).gap(px(2.0)).rounded(px(theme::R_ROW)).bg(theme::HOVER).self_start();
    for (i, label) in labels.into_iter().enumerate() {
        let (on1, pick) = (on.clone(), pick.clone());
        track = track.child(
            view()
                .h(px(24.0))
                .px(px(10.0))
                .justify_center()
                .rounded(px(theme::R_CTL))
                .cursor_pointer()
                .label(label.clone())
                .bg(move || if on1() == i { theme::RAISED } else { CLEAR })
                .on_press(move || pick(i))
                .child(text(label).text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(theme::FG)),
        );
    }
    track
}

/// `StatusChip` for a deadline: overdue (red), due within 14 days (amber),
/// else nothing.
pub(crate) fn due_chip(days: i64) -> Option<View> {
    if days < 0 {
        Some(chip(format!("{} day{} late", -days, if days == -1 { "" } else { "s" }), Tone::Danger))
    } else if days <= 14 {
        Some(chip(if days == 0 { "Due today".to_owned() } else { format!("Due in {days} day{}", if days == 1 { "" } else { "s" }) }, Tone::Warning))
    } else {
        None
    }
}

/// `₱ 1,234.50`.
pub(crate) fn peso(n: f64) -> String {
    format!("₱{}", bir_app_core::display::format_amount(&format!("{:.2}", n.abs()), false))
}
