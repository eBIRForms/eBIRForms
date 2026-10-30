//! The taxpayer switcher at the top of the sidebar, and the New taxpayer
//! dialog.

use bir_app_core::{BranchFacts, NewTaxpayer, Taxpayer};
use nsh_core::prelude::*;

use crate::live::store;
use crate::icons::Icon;
use crate::store::{Loaded, NoticeTone, Selected, call, task};
use crate::theme;
use crate::widgets::{Variant, avatar, button, button_sm, caption, field_input, group_label, nav_row, panel_header};

const TYPES: [&str; 6] = ["Individual", "Corporation", "Partnership", "Cooperative", "Estate", "Trust"];

/// The right panel: the selected taxpayer's profile, its branches, the
/// other taxpayers to switch to, and New taxpayer (DESIGN_SYSTEM.md "Right
/// panel, tax profile"). It closes from the pill or its toggle, never from
/// a close button of its own.
pub(crate) fn panel() -> impl IntoElement {
    let st = store();
    let body = view()
        .px(px(8.0))
        .pb(px(16.0))
        .child(each(
            move || vec![(st.selected.get(), st.taxpayers.get())],
            |(sel, _)| sel.as_ref().map(|s| s.tin.clone()),
            move |pair, _| {
                let (sel, list) = pair.get_untracked();
                match (sel, list) {
                    (Some(sel), Loaded::Ready(list)) => profile(sel, list).into_element(),
                    _ => empty_profile().into_element(),
                }
            },
        ));
    let phone = crate::mobile::phone();
    view()
        .w(if phone { Length::from(relative(1.0)) } else { Length::from(px(theme::PANEL_W)) })
        .flex_1()
        .h_full()
        .flex_shrink(0.0)
        .bg(theme::BG)
        .child(panel_header("Tax profile"))
        .child(scroll().flex_1().child(body))
}

fn empty_profile() -> impl IntoElement {
    let st = store();
    view()
        .px(px(8.0))
        .pt(px(8.0))
        .gap(px(12.0))
        .child(text("No taxpayer yet").text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
        .child(text("This panel shows the taxpayer you file for: their TIN, branches and details.").text_size(px(13.0)).text_color(theme::FG2))
        .child(view().flex_row().child(button("New taxpayer", Variant::Primary, || false, move || st.new_taxpayer_open.set(true))))
}

fn fact(label: &'static str, value: String) -> impl IntoElement {
    view()
        .gap(px(2.0))
        .child(caption(label))
        .child(text(if value.trim().is_empty() { "—".to_owned() } else { value }).text_size(px(13.0)).text_color(theme::FG))
}

fn profile(sel: Selected, list: Vec<Taxpayer>) -> impl IntoElement {
    let st = store();
    let Some(taxpayer) = list.iter().find(|t| t.branches.iter().any(|b| b.tin == sel.tin)).cloned() else {
        return empty_profile().into_element();
    };
    let branch = taxpayer.branches.iter().find(|b| b.tin == sel.tin).cloned();
    let facts = branch.as_ref().map(|b| b.facts.clone()).unwrap_or_default();
    let mut branches = view().gap(px(2.0));
    for b in &taxpayer.branches {
        let tin = b.tin.clone();
        let current = tin == sel.tin;
        branches = branches.child(nav_row(Some(Icon::Building), format!("{} · {}", b.name, b.code), move || current, move || st.select(&tin)));
    }
    let mut others = view().gap(px(2.0));
    for t in list.iter().filter(|t| t.tin_root != taxpayer.tin_root) {
        if let Some(head) = t.branches.first() {
            let tin = head.tin.clone();
            others = others.child(
                view()
                    .h(px(theme::CONTROL))
                    .px(px(8.0))
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .rounded(px(theme::R_ROW))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::HOVER))
                    .on_press(move || st.select(&tin))
                    .child(avatar(&t.name, &t.tin_root, 20.0))
                    .child(text(t.name.clone()).flex_1().min_w(px(0.0)).text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(theme::FG).truncate()),
            );
        }
    }
    let has_others = list.len() > 1;
    view()
        .child(
            view()
                .px(px(8.0))
                .pt(px(8.0))
                .pb(px(4.0))
                .gap(px(8.0))
                .child(avatar(&taxpayer.name, &taxpayer.tin_root, 56.0))
                .child(text(taxpayer.name.clone()).text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
                .child(text(sel.tin_display.clone()).text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).tabular_nums().text_color(theme::FG2)),
        )
        .child(
            view()
                .px(px(8.0))
                .pt(px(12.0))
                .gap(px(12.0))
                .child(fact("Branch", branch.as_ref().map(|b| format!("{} · {}", b.name, b.code)).unwrap_or_default()))
                .child(fact("Taxpayer type", taxpayer.taxpayer_type.clone()))
                .child(fact("RDO", facts.rdo_code.clone()))
                .child(fact("Registered address", if facts.zip_code.is_empty() { facts.registered_address.clone() } else { format!("{} {}", facts.registered_address, facts.zip_code) }))
                .child(fact("Email", facts.email.clone())),
        )
        .child(group_label("Branches"))
        .child(branches)
        .child(when(has_others, move || group_label("Switch taxpayer")))
        .child(others)
        .child(view().px(px(8.0)).pt(px(16.0)).flex_row().child(button_sm("New taxpayer", Variant::Secondary, Some(Icon::Plus), || false, move || st.new_taxpayer_open.set(true))))
        .into_element()
}

/// The New taxpayer dialog over the whole window. Everything is checked by
/// bir-core when it is created.
pub(crate) fn new_taxpayer_dialog() -> AnyElement {
    if crate::mobile::phone() {
        return phone_new_taxpayer().into_element();
    }
    desktop_new_taxpayer().into_element()
}

fn desktop_new_taxpayer() -> impl IntoElement {
    let st = store();
    let tin = signal(String::new());
    let name = signal(String::new());
    let kind = signal("Individual".to_owned());
    let place = signal(None::<bir_app_core::Place>);
    let street = signal(String::new());
    let email = signal(String::new());
    let birth = signal(String::new());
    let error = signal(String::new());
    let busy = signal(false);

    let create = move || {
        if busy.get_untracked() {
            return;
        }
        let tin_root: String = tin.get_untracked().chars().filter(char::is_ascii_digit).collect();
        if tin_root.len() != 9 {
            error.set("Enter the 9-digit TIN.".to_owned());
            return;
        }
        let Some(place_v) = place.get_untracked() else {
            error.set("Search the city or ZIP code and pick it from the list.".to_owned());
            return;
        };
        if let Some(gap) = place_gap(&place_v) {
            error.set(gap.to_owned());
            return;
        }
        let birth_text = birth.get_untracked();
        let birth_date = if birth_text.trim().is_empty() {
            if kind.get_untracked() == "Individual" {
                error.set("Enter the birth date as MM/DD/YYYY.".to_owned());
                return;
            }
            None
        } else {
            match bir_app_core::display::parse_typed_date(&birth_text) {
                Some(date) if date > crate::store::today() => {
                    error.set("The birth date is in the future.".to_owned());
                    return;
                }
                Some(date) => Some(date.to_string()),
                None => {
                    error.set("Enter a real birth date as MM/DD/YYYY.".to_owned());
                    return;
                }
            }
        };
        let street_v = street.get_untracked().trim().to_owned();
        let address = if street_v.is_empty() { place_v.city.clone() } else { format!("{street_v}, {}", place_v.city) };
        let new = NewTaxpayer {
            tin_root,
            name: name.get_untracked().trim().to_owned(),
            taxpayer_type: kind.get_untracked(),
            birth_date,
            head_office: BranchFacts {
                rdo_code: place_v.rdo_code.clone(),
                registered_address: address,
                zip_code: place_v.zip.clone(),
                phone: String::new(),
                email: email.get_untracked().trim().to_owned(),
            },
        };
        busy.set(true);
        error.set(String::new());
        task(async move {
            let created = call(move |app| app.create_taxpayer(&new)).await;
            busy.set(false);
            match created {
                Ok(taxpayer) => {
                    let head = taxpayer.branches.first().map(|b| b.tin.clone());
                    st.reload_taxpayers(head);
                    st.new_taxpayer_open.set(false);
                    st.notify(NoticeTone::Success, format!("{} added.", taxpayer.name));
                }
                Err(why) => error.set(why),
            }
        });
    };

    let labelled = |label: &'static str, input: Input| view().gap(px(3.0)).flex_1().child(caption(label)).child(input);
    let type_chip = move |label: &'static str| {
        view()
            .h(px(26.0))
            .px(px(9.0))
            .rounded(px(theme::R_FULL))
            .justify_center()
            .cursor_pointer()
            .border_1()
            .border_color(move || if kind.get() == label { theme::ACCENT } else { theme::BORDER_STRONG })
            .bg(move || if kind.get() == label { theme::ACCENT_SOFT } else { theme::SURFACE })
            .on_press(move || kind.set(label.to_owned()))
            .child(text(label).text_size(px(12.0)).text_color(theme::FG))
    };
    let mut types = view().flex_row().flex_wrap().gap(px(6.0));
    for label in TYPES {
        types = types.child(type_chip(label));
    }

    let card = view()
        .w(move || px((window().width.0 - safe_area().left.0 - safe_area().right.0 - 24.0).min(480.0)))
        .p(px(24.0))
        .gap(px(16.0))
        .rounded(px(theme::R_DIALOG))
        .bg(theme::SURFACE)
        .border_1()
        .border_color(theme::BORDER)
        .shadow_lg()
        .child(text("New taxpayer").text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
        .child(text("The head office (branch 00000) is created with it. Add branches later under Taxpayers.").text_size(px(13.0)).text_color(theme::FG2))
        .child(
            view()
                .flex_row()
                .gap(px(12.0))
                .child(labelled("TIN (9 digits)", tin_input_desktop(tin)))
                .child(labelled("Birth date (individuals)", date_input(birth))),
        )
        .child(labelled("Registered name", field_input(name, "As on the Certificate of Registration")))
        .child(view().gap(px(4.0)).child(caption("Taxpayer type")).child(types))
        .child(view().gap(px(3.0)).child(caption("City, ZIP and RDO")).child(place_combo(place)))
        .child(labelled("Street address", field_input(street, "House, street, barangay")))
        .child(labelled("Email", field_input(email, "name@example.com")))
        .child(when(move || !error.get().is_empty(), move || text(move || error.get()).text_size(px(12.0)).text_color(theme::DANGER)))
        .child(
            view()
                .flex_row()
                .justify_end()
                .gap(px(8.0))
                .child(button("Cancel", Variant::Secondary, || false, move || st.new_taxpayer_open.set(false)))
                .child(button(
                    move || if busy.get() { "Adding…" } else { "Add taxpayer" },
                    Variant::Primary,
                    move || busy.get(),
                    create,
                )),
        );

    // Clear of the status bar and the keyboard; the card scrolls when the
    // keyboard leaves it too little room.
    view()
        .absolute()
        .inset(px(0.0))
        .items_center()
        .justify_center()
        .pt(move || px(safe_area().top.0 + 12.0))
        .pb(move || px(keyboard_height().0.max(safe_area().bottom.0) + 12.0))
        .child(view().absolute().inset(px(0.0)).bg(theme::SCRIM).on_press(move || st.new_taxpayer_open.set(false)))
        .child(scroll().flex_shrink(1.0).child(card))
}

// ---------------------------------------------------------------- phone

/// The phone's New taxpayer: a full-screen page in the form's style —
/// stacked fields, the TIN as one box with dashes, one place search that
/// fills the city, ZIP and RDO, the type as option cards, the birth date
/// only for an individual, a required email — with the keyboard row while
/// typing and the next-step bar ("Next · TIN", then "Add taxpayer").
fn phone_new_taxpayer() -> impl IntoElement {
    let st = store();
    let name = signal(String::new());
    let tin = signal(String::new());
    let kind = signal("Individual".to_owned());
    let place = signal(None::<bir_app_core::Place>);
    let street = signal(String::new());
    let birth = signal(String::new());
    let email = signal(String::new());
    let error = signal(String::new());
    let busy = signal(false);
    let searching = signal(false);
    let typing = signal(None::<usize>);
    let refs: Vec<ElementRef> = (0..5).map(|_| element_ref()).collect();

    let digits = move || tin.get().chars().filter(char::is_ascii_digit).collect::<String>();
    let email_ok = move || {
        let e = email.get();
        let e = e.trim();
        e.contains('@') && e.rsplit('@').next().is_some_and(|d| d.contains('.')) && !e.contains(' ')
    };
    let individual = move || kind.get() == "Individual";
    // The first thing still missing, as the bar's next step.
    let missing = move || -> Option<(&'static str, usize)> {
        if name.get().trim().is_empty() {
            return Some(("Registered name", 0));
        }
        if digits().len() < 9 {
            return Some(("TIN", 1));
        }
        if place.get().as_ref().is_none_or(|p| place_gap(p).is_some()) {
            return Some(("City, ZIP and RDO", 99));
        }
        if street.get().trim().is_empty() {
            return Some(("Street address", 2));
        }
        if individual() && bir_app_core::display::parse_typed_date(&birth.get()).is_none_or(|d| d > crate::store::today()) {
            return Some(("Birth date", 3));
        }
        if !email_ok() {
            return Some(("Email", 4));
        }
        None
    };
    let missing2 = missing.clone();
    let create = move || {
        if busy.get_untracked() {
            return;
        }
        let Some(place_v) = place.get_untracked() else { return };
        let birth_date = if kind.get_untracked() == "Individual" { bir_app_core::display::parse_typed_date(&birth.get_untracked()).map(|d| d.to_string()) } else { None };
        let address = format!("{}, {}", street.get_untracked().trim(), place_v.city);
        let new = NewTaxpayer {
            tin_root: tin.get_untracked().chars().filter(char::is_ascii_digit).collect(),
            name: name.get_untracked().trim().to_owned(),
            taxpayer_type: kind.get_untracked(),
            birth_date,
            head_office: BranchFacts {
                rdo_code: place_v.rdo_code.clone(),
                registered_address: address,
                zip_code: place_v.zip.clone(),
                phone: String::new(),
                email: email.get_untracked().trim().to_owned(),
            },
        };
        busy.set(true);
        error.set(String::new());
        task(async move {
            let created = call(move |app| app.create_taxpayer(&new)).await;
            busy.set(false);
            match created {
                Ok(taxpayer) => {
                    let head = taxpayer.branches.first().map(|b| b.tin.clone());
                    st.reload_taxpayers(head);
                    st.new_taxpayer_open.set(false);
                    st.notify(NoticeTone::Success, format!("{} added.", taxpayer.name));
                }
                Err(why) => error.set(why),
            }
        });
    };

    // A stacked field: the label above, the box under it, a line between.
    let field = move |label: &'static str, hint: &'static str, input: Input, at: ElementRef, index: usize| {
        view()
            .px(px(18.0))
            .py(px(10.0))
            .border_b(px(1.0))
            .border_color(theme::BORDER)
            .bg(move || if typing.get() == Some(index) { theme::ACCENT_SOFT } else { theme::SURFACE })
            .on_press(move || at.focus())
            .child(
                view()
                    .flex_row()
                    .gap(px(8.0))
                    .child(text(label).flex_1().text_size(px(13.0)).text_color(theme::FG2))
                    .child(text(hint).text_size(px(11.0)).text_color(theme::FG3)),
            )
            .child(
                input
                    .element_ref(&at)
                    .on_focus(move || typing.set(Some(index)))
                    .on_blur(move || {
                        store().owner.run(|| {
                            set_timeout(std::time::Duration::from_millis(80), move || {
                                if typing.get_untracked() == Some(index) {
                                    typing.set(None);
                                }
                            })
                        });
                    })
                    .placeholder_color(|| color(theme::FG3))
                    .auto_correct(false)
                    .h(px(40.0))
                    .text_size(px(17.0))
                    .text_color(theme::FG)
                    .bg(crate::widgets::CLEAR),
            )
    };
    let text_input = |value: Signal<String>, placeholder: &'static str| {
        nsh_base::input().value(value).on_change(move |v: &str| value.set(v.to_owned())).placeholder(placeholder)
    };
    let tin_at = refs[1];
    let tin_input = nsh_base::input()
        .value(tin)
        .keyboard_type(KeyboardType::Number)
        .placeholder("000-000-000")
        .on_change(move |v: &str| {
            let prev = tin.get_untracked();
            let out = crate::typing::dash_tin(v, 9);
            crate::typing::show_formatted(tin, v, &out);
            if out != v {
                // The caret stays after the digit just typed.
                let caret = crate::typing::caret_after(&prev, v, &out, crate::typing::digit);
                tin_at.set_selection(caret, caret);
            }
        });

    let mut types = view().px(px(18.0)).py(px(10.0)).gap(px(8.0)).border_b(px(1.0)).border_color(theme::BORDER).child(text("Taxpayer type").text_size(px(13.0)).text_color(theme::FG2));
    for label in TYPES {
        types = types.child(
            view()
                .min_h(px(48.0))
                .px(px(14.0))
                .flex_row()
                .items_center()
                .rounded(px(14.0))
                .border(move || px(if kind.get() == label { 1.5 } else { 1.0_f32 }))
                .border_color(move || if kind.get() == label { theme::ACCENT } else { theme::BORDER_STRONG })
                .bg(move || if kind.get() == label { theme::ACCENT_SOFT } else { theme::SURFACE })
                .cursor_pointer()
                .label(label)
                .on_press(move || kind.set(label.to_owned()))
                .child(text(label).flex_1().text_size(px(15.0)).font_weight(FontWeight::MEDIUM).text_color(theme::FG))
                .child(when(move || kind.get() == label, || crate::icons::icon(Icon::Check, 18.0, || theme::ACCENT))),
        );
    }

    let place_row = view()
        .min_h(px(64.0))
        .px(px(18.0))
        .py(px(10.0))
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .border_b(px(1.0))
        .border_color(theme::BORDER)
        .cursor_pointer()
        .label("City, ZIP and RDO")
        .on_press(move || searching.set(true))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(text("City, ZIP and RDO").text_size(px(13.0)).text_color(theme::FG2))
                .child(
                    text(move || match place.get() {
                        Some(p) if p.zip.is_empty() => format!("{} · RDO {}", p.city, p.rdo_code),
                        Some(p) if p.rdo_code.is_empty() => format!("{} {} · pick the RDO too", p.city, p.zip),
                        Some(p) => format!("{} {} · RDO {}", p.city, p.zip, p.rdo_code),
                        None => "Search a city, province or RDO".to_owned(),
                    })
                    .text_size(px(17.0))
                    .text_color(move || if place.get().is_some() { theme::FG } else { theme::FG3 }),
                ),
        )
        .child(text("Change").text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::ACCENT));

    let r = refs.clone();
    let body = view()
        .child(field("Registered name", "As on the Certificate of Registration", text_input(name, "Juana Santos"), r[0], 0))
        .child(field("TIN", "9 digits", tin_input, r[1], 1))
        .child(types)
        .child(place_row)
        .child(field("Street address", "House, street, barangay", text_input(street, "1 Sample Street"), r[2], 2))
        .child(when(individual, {
            let r = r.clone();
            move || field("Birth date", "MM/DD/YYYY", slashed(birth, r[3]).placeholder("01/15/1990").keyboard_type(KeyboardType::Number), r[3], 3)
        }))
        .child(field("Email", "BIR sends the filing confirmation here", text_input(email, "name@example.com").keyboard_type(KeyboardType::Email), r[4], 4))
        .child(when(move || !email.get().trim().is_empty() && !email_ok() && typing.get() != Some(4), || {
            text("Enter an email like name@example.com: BIR sends the filing confirmation there.").mx(px(18.0)).mt(px(6.0)).text_size(px(12.0)).text_color(theme::DANGER)
        }))
        .child(when(move || !error.get().is_empty(), move || text(move || error.get()).mx(px(18.0)).mt(px(8.0)).text_size(px(12.0)).text_color(theme::DANGER)))
        .child(view().h(move || px(320.0 + safe_area().bottom.0)));

    // The order ↑ ↓ moves through (the birth date only for an individual).
    let order = move || -> Vec<usize> { if individual() { vec![0, 1, 2, 3, 4] } else { vec![0, 1, 2, 4] } };
    let step = {
        let refs = refs.clone();
        move |by: isize| {
            let order = order();
            let Some(now) = typing.get_untracked() else { return };
            let Some(at) = order.iter().position(|i| *i == now) else { return };
            let next = at as isize + by;
            if next >= 0 && (next as usize) < order.len() {
                refs[order[next as usize]].focus();
            }
        }
    };
    let step2 = step.clone();
    let labels = ["Registered name", "TIN", "Street address", "Birth date", "Email"];
    let accessory = move || {
        let step = step.clone();
        let step2 = step2.clone();
        let key = |i: Icon, label: &'static str, f: Box<dyn Fn()>| {
            view().size(px(44.0)).items_center().justify_center().cursor_pointer().label(label).on_press(f).child(crate::icons::icon(i, 16.0, || theme::ACCENT))
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
            .child(key(Icon::ChevronUp, "Previous field", Box::new(move || step(-1))))
            .child(key(Icon::ChevronDown, "Next field", Box::new(move || step2(1))))
            .child(text(move || typing.get().map(|i| labels[i].to_owned()).unwrap_or_default()).flex_1().px(px(4.0)).text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
            .child(view().h(px(44.0)).px(px(12.0)).justify_center().cursor_pointer().label("Done").on_press(dismiss_keyboard).child(text("Done").text_size(px(14.0)).font_weight(FontWeight::BOLD).text_color(theme::ACCENT)))
    };
    let next_refs = refs.clone();
    let bar = move || {
        let missing = missing2.clone();
        let missing_b = missing.clone();
        let refs = next_refs.clone();
        view()
            .absolute()
            .left(move || px(12.0 + safe_area().left.0))
            .right(move || px(12.0 + safe_area().right.0))
            .bottom(move || px(if safe_area().bottom.0 > 0.0 { safe_area().bottom.0 + 4.0 } else { 16.0 }))
            .h(px(56.0))
            .px(px(16.0))
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .rounded(px(22.0))
            .bg(move || if missing().is_none() { theme::ACCENT } else { theme::FG })
            .shadow_lg()
            .cursor_pointer()
            .label("Next step")
            .on_press(move || match missing_b() {
                Some((_, 99)) => searching.set(true),
                Some((_, i)) => refs[i].focus(),
                None => create(),
            })
            .child({
                let missing = missing2.clone();
                let missing_c = missing2.clone();
                text(move || match missing() {
                    Some((label, _)) => format!("Next · {label}"),
                    None if busy.get() => "Adding…".to_owned(),
                    None => "Add taxpayer".to_owned(),
                })
                .flex_1()
                .text_size(px(15.0))
                .font_weight(FontWeight::BOLD)
                .text_color(move || if missing_c().is_none() { theme::PRIMARY_FG } else { theme::SURFACE })
            })
    };
    let _ = missing;

    view()
        .absolute()
        .inset(px(0.0))
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
                        .h(px(crate::screens::phone_form::NAV_H))
                        .px(px(6.0))
                        .flex_row()
                        .items_center()
                        .child(view().size(px(44.0)).items_center().justify_center().cursor_pointer().label("Close").on_press(move || st.new_taxpayer_open.set(false)).child(text("✕").text_size(px(18.0)).text_color(theme::ACCENT)))
                        .child(text("New taxpayer").flex_1().text_center().text_size(px(17.0)).font_weight(FontWeight::BOLD).text_color(theme::FG))
                        .child(view().w(px(44.0))),
                ),
        )
        .child(
            scroll()
                .flex_1()
                .keyboard_dismiss_mode(KeyboardDismissMode::Interactive).keyboard_taps(KeyboardTaps::Handled)
                .content_inset(move || EdgeInsets { bottom: px(if typing.get().is_some() { 60.0 } else { 0.0 }), ..Default::default() })
                .child(view().pl(move || px(safe_area().left.0)).pr(move || px(safe_area().right.0)).child(body)),
        )
        .child(when(move || typing.get().is_none(), bar))
        .child(when(move || typing.get().is_some(), accessory))
        .child(when(move || searching.get(), move || place_search(place, searching)))
}

/// A date box that shows MM/DD/YYYY slashes as the digits come, the caret
/// kept after the digit typed.
fn slashed(value: Signal<String>, at: ElementRef) -> Input {
    nsh_base::input().value(value).on_change(move |v: &str| {
        let prev = value.get_untracked();
        let shown = crate::typing::slash_date(v);
        crate::typing::show_formatted(value, v, &shown);
        if shown != v {
            let caret = crate::typing::caret_after(&prev, v, &shown, crate::typing::digit);
            at.set_selection(caret, caret);
        }
    })
}

/// The desktop's birth date box: [`slashed`] in the field style.
fn date_input(value: Signal<String>) -> Input {
    let at = element_ref();
    crate::widgets::field_box("MM/DD/YYYY").value(value).element_ref(&at).on_change(move |v: &str| {
        let prev = value.get_untracked();
        let shown = crate::typing::slash_date(v);
        crate::typing::show_formatted(value, v, &shown);
        if shown != v {
            let caret = crate::typing::caret_after(&prev, v, &shown, crate::typing::digit);
            at.set_selection(caret, caret);
        }
    })
}

/// The desktop's TIN box: dashes as the digits come (9 digits).
fn tin_input_desktop(value: Signal<String>) -> Input {
    let at = element_ref();
    crate::widgets::field_box("000-123-453").value(value).element_ref(&at).on_change(move |v: &str| {
        let prev = value.get_untracked();
        let shown = crate::typing::dash_tin(v, 9);
        crate::typing::show_formatted(value, v, &shown);
        if shown != v {
            let caret = crate::typing::caret_after(&prev, v, &shown, crate::typing::digit);
            at.set_selection(caret, caret);
        }
    })
}

/// The place picked after `next` was chosen from the list: an RDO picked
/// after a ZIP keeps the ZIP, and a ZIP with no RDO of its own picked after
/// an RDO keeps the RDO.
fn merge_place(had: Option<bir_app_core::Place>, next: bir_app_core::Place) -> bir_app_core::Place {
    match had {
        Some(mut had) if next.zip.is_empty() && !had.zip.is_empty() => {
            had.rdo_code = next.rdo_code;
            had.rdo_name = next.rdo_name;
            had
        }
        Some(had) if had.zip.is_empty() && !next.zip.is_empty() && next.rdo_code.is_empty() => {
            bir_app_core::Place { rdo_code: had.rdo_code, rdo_name: had.rdo_name, ..next }
        }
        _ => next,
    }
}

/// What a picked place still lacks before a taxpayer can be added: the
/// ZIP code or the RDO.
fn place_gap(place: &bir_app_core::Place) -> Option<&'static str> {
    if place.zip.is_empty() {
        Some("Search the city or ZIP code too and pick it from the list.")
    } else if place.rdo_code.is_empty() {
        Some("Pick the RDO too: search its name or number.")
    } else {
        None
    }
}

/// The places whose name, ZIP or RDO contains `query` (two letters or
/// more), at most `limit`.
fn matching_places(all: &[bir_app_core::Place], query: &str, limit: usize) -> Vec<bir_app_core::Place> {
    let q = query.trim().to_lowercase();
    if q.len() < 2 {
        return Vec::new();
    }
    all.iter().filter(|p| p.label.to_lowercase().contains(&q) || p.rdo_name.to_lowercase().contains(&q)).take(limit).cloned().collect()
}

/// The desktop's place box: type a city, province, ZIP or RDO and pick from
/// the list under it, which fills the city, the ZIP and the RDO.
fn place_combo(place: Signal<Option<bir_app_core::Place>>) -> impl IntoElement {
    let query = signal(String::new());
    let all = std::rc::Rc::new(bir_app_core::BirApp::places());
    let filtered = move || matching_places(&all, &query.get(), 8);
    let shown = filtered.clone();
    view()
        .gap(px(4.0))
        .child(crate::widgets::field_box("Search a city, province, ZIP or RDO").value(query).on_change(move |v: &str| query.set(v.to_owned())))
        .child(when(
            move || !shown().is_empty(),
            move || {
                view().rounded(px(8.0)).border_1().border_color(theme::BORDER).bg(theme::SURFACE).child(each(
                    filtered.clone(),
                    |p| p.label.clone(),
                    move |p, _| {
                        let p = p.get_untracked();
                        let pick = p.clone();
                        let line = if p.zip.is_empty() { "RDO only".to_owned() } else if p.rdo_code.is_empty() { "No RDO matched: pick the RDO too".to_owned() } else { format!("RDO {} · {}", p.rdo_code, p.rdo_name) };
                        view()
                            .px(px(10.0))
                            .py(px(6.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::HOVER))
                            .on_press(move || {
                                place.set(Some(merge_place(place.get_untracked(), pick.clone())));
                                query.set(String::new());
                            })
                            .child(text(p.label.clone()).text_size(px(13.0)).text_color(theme::FG))
                            .child(text(line).text_size(px(11.0)).text_color(theme::FG2))
                    },
                ))
            },
        ))
        .child(
            text(move || match place.get() {
                Some(p) if p.zip.is_empty() => format!("{} · RDO {} · search the ZIP code too", p.city, p.rdo_code),
                Some(p) if p.rdo_code.is_empty() => format!("{} {} · search the RDO too", p.city, p.zip),
                Some(p) => format!("{} {} · RDO {} {}", p.city, p.zip, p.rdo_code, p.rdo_name),
                None => "Nothing picked yet".to_owned(),
            })
            .text_size(px(12.0))
            .text_color(move || if place.get().is_some() { theme::FG2 } else { theme::FG3 }),
        )
}

/// One search over every place: a ZIP area with its RDO, or an RDO alone.
fn place_search(place: Signal<Option<bir_app_core::Place>>, open: Signal<bool>) -> impl IntoElement {
    let query = signal(String::new());
    let all = std::rc::Rc::new(bir_app_core::BirApp::places());
    let filtered = move || matching_places(&all, &query.get(), 60);
    let search = nsh_base::input()
        .value(query)
        .on_change(move |v: &str| query.set(v.to_owned()))
        .placeholder("City, province, ZIP or RDO")
        .placeholder_color(|| color(theme::FG3))
        .auto_correct(false)
        .auto_focus(true)
        .h(px(44.0))
        .px(px(14.0))
        .text_size(px(16.0))
        .text_color(theme::FG)
        .bg(theme::HOVER)
        .rounded(px(12.0));
    crate::screens::phone_fields::full_sheet(
        "City, ZIP and RDO".to_owned(),
        move || close_after_blur(move || open.set(false)),
        view()
            .flex_1()
            .child(view().px(px(16.0)).py(px(10.0)).child(search))
            .child(scroll().flex_1().keyboard_dismiss_mode(KeyboardDismissMode::OnDrag).keyboard_taps(KeyboardTaps::Handled).child(view().pb(move || px(keyboard_height().0.max(safe_area().bottom.0) + 16.0)).child(each(
                filtered,
                |p| p.label.clone(),
                move |p, _| {
                    let p = p.get_untracked();
                    let pick = p.clone();
                    let line = if p.zip.is_empty() { "RDO only".to_owned() } else if p.rdo_code.is_empty() { "No RDO matched: pick the RDO too".to_owned() } else { format!("RDO {} · {}", p.rdo_code, p.rdo_name) };
                    view()
                        .min_h(px(56.0))
                        .px(px(18.0))
                        .py(px(8.0))
                        .border_b(px(1.0))
                        .border_color(theme::BORDER)
                        .cursor_pointer()
                        .active(|s| s.bg(theme::HOVER))
                        .on_press(move || {
                            place.set(Some(merge_place(place.get_untracked(), pick.clone())));
                            close_after_blur(move || open.set(false));
                        })
                        .child(text(p.label.clone()).text_size(px(15.0)).text_color(theme::FG))
                        .child(text(line).text_size(px(12.0)).text_color(theme::FG2))
                },
            )))),
    )
}

/// Hide the keyboard, then run `close` once it has let go of the input:
/// removing a focused input in the same commit leaves Android's keyboard
/// up with nothing to type into.
pub(crate) fn close_after_blur(close: impl FnOnce() + 'static) {
    dismiss_keyboard();
    store().owner.run(|| set_timeout(std::time::Duration::from_millis(120), close));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(label: &str, zip: &str, rdo: &str) -> bir_app_core::Place {
        bir_app_core::Place { label: label.to_owned(), city: label.to_owned(), zip: zip.to_owned(), rdo_code: rdo.to_owned(), rdo_name: format!("RDO {rdo}") }
    }

    #[test]
    fn an_rdo_picked_after_a_zip_keeps_the_zip() {
        let merged = merge_place(Some(place("Pasig", "1600", "")), place("RDO 043", "", "043"));
        assert_eq!((merged.city.as_str(), merged.zip.as_str(), merged.rdo_code.as_str()), ("Pasig", "1600", "043"));
        let replaced = merge_place(Some(place("Pasig", "1600", "043")), place("Makati", "1200", "050"));
        assert_eq!(replaced.zip, "1200");
    }

    #[test]
    fn a_zip_without_an_rdo_picked_after_an_rdo_keeps_the_rdo() {
        let merged = merge_place(Some(place("RDO 043", "", "043")), place("Pasig", "1600", ""));
        assert_eq!((merged.city.as_str(), merged.zip.as_str(), merged.rdo_code.as_str()), ("Pasig", "1600", "043"));
        let own = merge_place(Some(place("RDO 043", "", "043")), place("Laoag", "2900", "001"));
        assert_eq!(own.rdo_code, "001", "a ZIP with its own RDO replaces the earlier one");
    }

    #[test]
    fn a_taxpayer_needs_the_zip_and_the_rdo() {
        assert!(place_gap(&place("RDO 043", "", "043")).is_some());
        assert!(place_gap(&place("Pasig", "1600", "")).is_some());
        assert_eq!(place_gap(&place("Pasig", "1600", "043")), None);
    }

    #[test]
    fn the_place_list_comes_from_the_zip_and_rdo_tables() {
        let all = bir_app_core::BirApp::places();
        assert!(matching_places(&all, "1", 8).is_empty(), "one letter lists nothing");
        let by_zip = matching_places(&all, "1600", 8);
        assert!(by_zip.iter().any(|p| p.zip == "1600"), "a ZIP finds its area");
        let rdos = matching_places(&all, "RDO 043", 8);
        assert!(rdos.iter().any(|p| p.rdo_code == "043" && p.zip.is_empty()), "an RDO number finds the RDO");
    }
}
