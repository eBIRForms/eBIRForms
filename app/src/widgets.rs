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
        "Queued" => Tone::Warning,
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
    input()
        .value(value)
        .on_change(move |v: &str| value.set(v.to_owned()))
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
