//! Taxpayers (desktop, bir-shell `taxpayer.js` and DESIGN_SYSTEM "Main area
//! for a taxpayer" / "Disclosure"; the phone reaches it from the tax
//! profile sheet): the selected taxpayer's page header, its profile in one
//! disclosure group (Taxpayer, Business, Branch, Address and contact; each
//! with Edit in place), its branches with Add branch, and every taxpayer to
//! switch to.

use bir_app_core::display::{format_form_date, parse_typed_date};
use bir_app_core::{BirApp, Choice, ProfileDetails, Taxpayer, TaxpayerUpdate};
use nsh_core::prelude::*;

use crate::icons::Icon;
use crate::live::store;
use crate::screens::{Route, content, page_title};
use crate::store::{Loaded, NoticeTone, Selected, call, task};
use crate::theme;
use crate::widgets::{Tone, Variant, avatar, button_sm, caption, chip, code_text, empty_row, field_input, list_group, list_row, name_cell, section_title, tick_row};

fn current() -> Option<(Selected, Taxpayer)> {
    let st = store();
    let sel = st.selected.get()?;
    match st.taxpayers.get() {
        Loaded::Ready(list) => list.into_iter().find(|t| t.branches.iter().any(|b| b.tin == sel.tin)).map(|t| (sel, t)),
        _ => None,
    }
}

pub(crate) fn screen() -> impl IntoElement {
    let st = store();
    content(each(
        move || vec![current().map(|(s, t)| (s.tin, format!("{t:?}")))],
        |k| k.clone(),
        move |k, _| match untracked(current) {
            Some((s, t)) if k.get_untracked().is_some() => page(s.tin, t).into_element(),
            _ => view()
                .gap(px(12.0))
                .child(page_title("Taxpayers"))
                .child(text("No taxpayer yet. Add one with their TIN and registered name.").text_size(px(13.0)).text_color(theme::FG2))
                .child(view().flex_row().child(button_sm("New taxpayer", Variant::Primary, Some(Icon::Plus), || false, move || st.new_taxpayer_open.set(true))))
                .into_element(),
        },
    ))
}

fn page(tin: String, t: Taxpayer) -> impl IntoElement {
    let st = store();
    let branch = t.branches.iter().find(|b| b.tin == tin).cloned();
    let Some(branch) = branch else { return view().into_element() };
    let tin_display = branch.tin_display.clone();
    let copy = tin_display.clone();
    let others: Vec<Taxpayer> = match st.taxpayers.get_untracked() {
        Loaded::Ready(list) => list,
        _ => Vec::new(),
    };
    view()
        .gap(px(4.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(12.0))
                .child(avatar(&t.name, &t.tin_root, 40.0))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(page_title(t.name.clone()))
                        .child(text(format!("{tin_display} · {} · {}", branch.name, t.taxpayer_type)).text_size(px(12.0)).tabular_nums().text_color(theme::FG2)),
                )
                .child(button_sm("Copy TIN", Variant::Ghost, None, || false, move || {
                    set_clipboard(&copy);
                    store().notify(NoticeTone::Info, "TIN copied.");
                }))
                .child(button_sm("New return", Variant::Primary, Some(Icon::Plus), || false, move || st.go(Route::Library))),
        )
        .child(view().h(px(16.0)))
        .child(section_title("Profile", true))
        .child(profile_group(t.clone(), branch.clone()))
        .child(section_title(format!("Branches · {}", t.branches.len()), false))
        .child(branches(t.clone(), tin.clone()))
        .child(when(others.len() > 1, move || section_title("Switch taxpayer", false)))
        .child(if others.len() <= 1 { view().into_element() } else { list_group().child({
            let mut col = view();
            for (i, o) in others.iter().filter(|o| o.tin_root != t.tin_root).enumerate() {
                let head = o.branches.first().map(|b| b.tin.clone()).unwrap_or_default();
                col = col.child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(12.0))
                        .min_h(px(52.0))
                        .px(px(16.0))
                        .border_t(px(if i == 0 { 0.0 } else { 1.0 }))
                        .border_color(theme::BORDER)
                        .cursor_pointer()
                        .hover(|s| s.bg(theme::HOVER))
                        .on_press(move || store().select(&head))
                        .child(avatar(&o.name, &o.tin_root, 20.0))
                        .child(name_cell(o.name.clone(), o.tin_display.clone()))
                        .child(text(o.taxpayer_type.clone()).text_size(px(12.0)).text_color(theme::FG2)),
                );
            }
            col
        }).into_element() })
        .into_element()
}

/// The profile's sections, built once the business and filing facts are
/// read (and again after they are saved).
fn profile_group(t: Taxpayer, branch: bir_app_core::Branch) -> impl IntoElement {
    let details: Signal<Option<ProfileDetails>> = signal(None);
    let reload = {
        let root = t.tin_root.clone();
        move || {
            let root = root.clone();
            task(async move {
                let year = election_year();
                match call(move |app| app.profile_details(&root, year)).await {
                    Ok(read) => details.set(Some(read)),
                    Err(why) => store().notify(NoticeTone::Danger, format!("Could not read the profile: {why}")),
                }
            });
        }
    };
    reload();
    each(
        move || vec![details.get().map(|d| format!("{d:?}"))],
        |k| k.clone(),
        move |_, _| {
            let Some(d) = details.get_untracked() else { return view().h(px(44.0)).into_element() };
            view()
                .rounded(px(theme::R_CARD))
                .border_1()
                .border_color(theme::BORDER)
                .overflow_hidden()
                .child(taxpayer_section(t.clone(), d.clone(), details))
                .child(business_section(t.tin_root.clone(), d, details))
                .child(branch_section(branch.clone(), true))
                .child(address_section(branch.clone()))
                .into_element()
        },
    )
}

/// The year the profile's income-tax election is shown and set for.
fn election_year() -> u16 {
    u16::try_from(bir_app_core::display::current_year()).unwrap_or(2026)
}

/// `YYYY-MM-DD` as the forms write a date, MM/DD/YYYY.
fn shown_date(iso: Option<&str>) -> String {
    iso.and_then(|d| d.parse::<chrono::NaiveDate>().ok()).map(format_form_date).unwrap_or_default()
}

/// A typed MM/DD/YYYY date as `YYYY-MM-DD`: `Ok(None)` when blank, an error
/// naming `what` when it is not a real past date.
fn typed_date(typed: &str, what: &str) -> Result<Option<String>, String> {
    if typed.trim().is_empty() {
        return Ok(None);
    }
    match parse_typed_date(typed) {
        Some(date) if date > crate::store::today() => Err(format!("The {what} cannot be in the future.")),
        Some(date) => Ok(Some(date.to_string())),
        None => Err(format!("Enter the {what} as MM/DD/YYYY, like 01/15/1990.")),
    }
}

/// A choice's title, "—" for none.
fn title_of(list: &[Choice], value: Option<&str>) -> String {
    value.and_then(|v| list.iter().find(|c| c.value == v)).map(|c| c.title.clone()).unwrap_or_default()
}

/// One pill per choice (and "Not set" when `none` is allowed); one is on.
fn pills(list: Vec<Choice>, value: Signal<Option<String>>, none: bool) -> impl IntoElement {
    let mut row = view().flex_row().flex_wrap().gap(px(6.0));
    let mut all: Vec<(Option<String>, String)> = list.into_iter().map(|c| (Some(c.value), c.title)).collect();
    if none {
        all.push((None, "Not set".to_owned()));
    }
    for (choice, title) in all {
        let (c1, c2, c3) = (choice.clone(), choice.clone(), choice);
        row = row.child(
            view()
                .h(px(26.0))
                .px(px(9.0))
                .rounded(px(theme::R_FULL))
                .justify_center()
                .cursor_pointer()
                .border_1()
                .border_color(move || if value.get() == c1 { theme::ACCENT } else { theme::BORDER_STRONG })
                .bg(move || if value.get() == c2 { theme::ACCENT_SOFT } else { theme::SURFACE })
                .label(title.clone())
                .on_press(move || value.set(c3.clone()))
                .child(text(title).text_size(px(12.0)).text_color(theme::FG)),
        );
    }
    row
}

/// A labelled group of edit controls, full width.
fn block(label: &'static str, control: impl IntoElement) -> impl IntoElement {
    view().pt(px(16.0)).gap(px(6.0)).child(caption(label)).child(control)
}

/// Save `d` for the taxpayer `root`; the sections show what is stored.
fn save_details(root: String, d: ProfileDetails, busy: Signal<bool>, details: Signal<Option<ProfileDetails>>, editing: Signal<bool>, saved: &'static str) {
    busy.set(true);
    task(async move {
        let done = call(move |app| app.update_profile_details(&root, &d)).await;
        busy.set(false);
        match done {
            Ok(stored) => {
                editing.set(false);
                store().notify(NoticeTone::Success, saved);
                details.set(Some(stored));
                store().reload_taxpayers(None);
            }
            Err(why) => store().notify(NoticeTone::Danger, format!("Not saved: {why}")),
        }
    });
}

/// A disclosure section: a 44 tall header (chevron, title, meta, Edit while
/// open), its body indented to the title.
fn section(title: &'static str, meta: String, first: bool, open_at_start: bool, body: impl Fn(Signal<bool>) -> AnyElement + 'static) -> impl IntoElement {
    let open = signal(open_at_start);
    let editing = signal(false);
    let body = std::rc::Rc::new(body);
    view()
        .border_t(px(if first { 0.0 } else { 1.0 }))
        .border_color(theme::BORDER)
        .bg(theme::SURFACE)
        .child(
            view()
                .h(px(44.0))
                .px(px(16.0))
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .cursor_pointer()
                .on_press(move || open.update(|o| *o = !*o))
                .child(each(move || vec![open.get()], |o| *o, |o, _| crate::icons::icon(if o.get_untracked() { Icon::ChevronDown } else { Icon::ChevronRight }, 16.0, || theme::FG2)))
                .child(text(title).text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
                .child(text(meta).flex_1().text_size(px(12.0)).text_color(theme::FG3).truncate())
                .child(when(move || open.get() && !editing.get(), move || button_sm("Edit", Variant::Ghost, None, || false, move || editing.set(true)))),
        )
        .child(when(move || open.get(), move || {
            let body = body.clone();
            view().pt(px(4.0)).pr(px(20.0)).pb(px(20.0)).pl(px(40.0)).child(each(move || vec![editing.get()], |e| *e, move |_, _| body(editing)))
        }))
}

/// A label over a value (read mode).
fn fact(label: &'static str, value: String) -> impl IntoElement {
    view()
        .w(px(260.0))
        .gap(px(2.0))
        .child(caption(label))
        .child(text(if value.trim().is_empty() { "—".to_owned() } else { value }).text_size(px(13.0)).text_color(theme::FG))
}

fn grid() -> View {
    view().flex_row().flex_wrap().gap(px(16.0))
}

fn labelled(label: &'static str, input: Input) -> impl IntoElement {
    view().w(px(260.0)).gap(px(3.0)).child(caption(label)).child(input)
}

/// Cancel and "Save changes" at the section's bottom right.
fn edit_bar(editing: Signal<bool>, busy: Signal<bool>, save: impl Fn() + 'static) -> impl IntoElement {
    view()
        .flex_row()
        .justify_end()
        .gap(px(8.0))
        .pt(px(16.0))
        .child(button_sm("Cancel", Variant::Ghost, None, || false, move || editing.set(false)))
        .child(button_sm(move || if busy.get() { "Saving…" } else { "Save changes" }, Variant::Primary, None, move || busy.get(), save))
}

const TYPES: [&str; 6] = ["Individual", "Corporation", "Partnership", "Cooperative", "Estate", "Trust"];

/// What the applicability flags say, as one line ("Has employees · VAT").
fn flags_line(d: &ProfileDetails) -> String {
    let all = [
        (d.has_employees, "Has employees"),
        (d.withholds_compensation, "Withholds on compensation"),
        (d.withholds_expanded, "Withholds expanded tax"),
        (d.withholds_final, "Withholds final tax"),
        (d.vat_registered, "VAT-registered"),
        (d.single_employer, "One employer"),
        (d.dormant, "Dormant"),
    ];
    let on: Vec<&str> = all.iter().filter(|(set, _)| *set).map(|(_, label)| *label).collect();
    if on.is_empty() { "None".to_owned() } else { on.join(" · ") }
}

fn taxpayer_section(t: Taxpayer, d: ProfileDetails, details: Signal<Option<ProfileDetails>>) -> impl IntoElement {
    let meta = "Shared by every branch".to_owned();
    section("Taxpayer", meta, true, true, move |editing| {
        let t = t.clone();
        let d = d.clone();
        let year = d.election_year;
        if !editing.get_untracked() {
            let classes = BirApp::tax_classifications(&t.taxpayer_type);
            let mut grid = grid()
                .child(fact("Registered name", t.name.clone()))
                .child(fact("TIN", t.tin_display.clone()))
                .child(fact("Taxpayer type", t.taxpayer_type.clone()));
            if t.taxpayer_type == "Individual" {
                grid = grid.child(fact("Birth date", shown_date(t.birth_date.as_deref())));
            }
            if !classes.is_empty() {
                grid = grid.child(fact("Tax classification", title_of(&classes, d.tax_classification.as_deref())));
            }
            if t.taxpayer_type == "Individual" {
                grid = grid.child(fact_owned(format!("Income-tax election, {year}"), title_of(&BirApp::income_tax_elections(), d.income_tax_election.as_deref())));
            }
            return view()
                .child(grid)
                .child(view().pt(px(16.0)).gap(px(2.0)).child(caption("What applies")).child(text(flags_line(&d)).text_size(px(13.0)).text_color(theme::FG)))
                .into_element();
        }
        let name = signal(t.name.clone());
        let kind = signal(t.taxpayer_type.clone());
        let birth = signal(shown_date(t.birth_date.as_deref()));
        let class = signal(d.tax_classification.clone());
        let election = signal(d.income_tax_election.clone());
        let flags: [(Signal<bool>, &'static str); 7] = [
            (signal(d.has_employees), "Has employees"),
            (signal(d.withholds_compensation), "Withholds tax on compensation"),
            (signal(d.withholds_expanded), "Withholds expanded tax from suppliers"),
            (signal(d.withholds_final), "Withholds final tax"),
            (signal(d.vat_registered), "VAT-registered"),
            (signal(d.single_employer), "Purely compensation, one employer"),
            (signal(d.dormant), "Dormant (no operations)"),
        ];
        let busy = signal(false);
        let root = t.tin_root.clone();
        let before = d.clone();
        let save = move || {
            let individual = kind.get_untracked() == "Individual";
            let birth_date = if individual {
                match typed_date(&birth.get_untracked(), "birth date") {
                    Ok(date) => date,
                    Err(why) => {
                        store().notify(NoticeTone::Danger, why);
                        return;
                    }
                }
            } else {
                None
            };
            let classes = BirApp::tax_classifications(&kind.get_untracked());
            let mut next = before.clone();
            next.tax_classification = class.get_untracked().filter(|c| classes.iter().any(|x| &x.value == c));
            next.income_tax_election = if individual { election.get_untracked() } else { None };
            let [a, b, c, e, f, g, h] = flags.map(|(sig, _)| sig.get_untracked());
            (next.has_employees, next.withholds_compensation, next.withholds_expanded, next.withholds_final, next.vat_registered, next.single_employer, next.dormant) = (a, b, c, e, f, g, h);
            let update = TaxpayerUpdate { name: Some(name.get_untracked().trim().to_owned()), taxpayer_type: Some(kind.get_untracked()), birth_date: Some(birth_date) };
            let root = root.clone();
            busy.set(true);
            task(async move {
                let done = call(move |app| {
                    app.update_taxpayer(&root, &update)?;
                    app.update_profile_details(&root, &next)
                })
                .await;
                busy.set(false);
                match done {
                    Ok(stored) => {
                        editing.set(false);
                        store().notify(NoticeTone::Success, "Taxpayer saved.");
                        details.set(Some(stored));
                        store().reload_taxpayers(None);
                    }
                    Err(why) => store().notify(NoticeTone::Danger, format!("Not saved: {why}")),
                }
            });
        };
        let mut types = view().flex_row().flex_wrap().gap(px(6.0));
        for label in TYPES {
            types = types.child(
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
                    .child(text(label).text_size(px(12.0)).text_color(theme::FG)),
            );
        }
        let birth_at = element_ref();
        let mut ticks = view().gap(px(4.0));
        for (sig, label) in flags {
            ticks = ticks.child(tick_row(label, 13.0, move || sig.get(), move |on| sig.set(on)));
        }
        view()
            .child(grid().child(labelled("Registered name", field_input(name, "As on the Certificate of Registration"))).child(fact("TIN", t.tin_display.clone())))
            .child(view().pt(px(16.0)).gap(px(4.0)).child(caption("Taxpayer type")).child(types))
            .child(when(move || kind.get() == "Individual", move || {
                view().pt(px(16.0)).child(labelled("Birth date", crate::screens::taxpayer::date_input(birth, birth_at)))
            }))
            .child(each(move || vec![kind.get()], |k| k.clone(), move |k, _| {
                let classes = BirApp::tax_classifications(&k.get_untracked());
                if classes.is_empty() {
                    return view().into_element();
                }
                block("Tax classification", pills(classes, class, true)).into_element()
            }))
            .child(when(move || kind.get() == "Individual", move || {
                view()
                    .child(block_owned(format!("Income-tax election, {year}"), pills(BirApp::income_tax_elections(), election, true)))
            }))
            .child(block("What applies", ticks))
            .child(edit_bar(editing, busy, save))
            .into_element()
    })
}

/// [`fact`] with a label made at run time.
fn fact_owned(label: String, value: String) -> impl IntoElement {
    view()
        .w(px(260.0))
        .gap(px(2.0))
        .child(caption(label))
        .child(text(if value.trim().is_empty() { "—".to_owned() } else { value }).text_size(px(13.0)).text_color(theme::FG))
}

/// [`block`] with a label made at run time.
fn block_owned(label: String, control: impl IntoElement) -> impl IntoElement {
    view().pt(px(16.0)).gap(px(6.0)).child(caption(label)).child(control)
}

fn business_section(root: String, d: ProfileDetails, details: Signal<Option<ProfileDetails>>) -> impl IntoElement {
    let meta = if d.line_of_business.trim().is_empty() { "Line of business, start date, EOPT tier".to_owned() } else { d.line_of_business.clone() };
    section("Business", meta, false, false, move |editing| {
        let d = d.clone();
        let tiers = BirApp::eopt_tiers();
        if !editing.get_untracked() {
            return grid()
                .child(fact("Line of business", d.line_of_business.clone()))
                .child(fact("Business start or incorporation", shown_date(d.business_start_date.as_deref())))
                .child(fact("EOPT tier", title_of(&tiers, d.eopt_tier.as_deref())))
                .child(fact("Government withholding entity", if d.government_withholding { "Yes".to_owned() } else { "No".to_owned() }))
                .into_element();
        }
        let line = signal(d.line_of_business.clone());
        let start = signal(shown_date(d.business_start_date.as_deref()));
        let tier = signal(d.eopt_tier.clone());
        let government = signal(d.government_withholding);
        let busy = signal(false);
        let root = root.clone();
        let before = d.clone();
        let save = move || {
            let start_date = match typed_date(&start.get_untracked(), "start date") {
                Ok(date) => date,
                Err(why) => {
                    store().notify(NoticeTone::Danger, why);
                    return;
                }
            };
            let next = ProfileDetails {
                line_of_business: line.get_untracked().trim().to_owned(),
                business_start_date: start_date,
                eopt_tier: tier.get_untracked(),
                government_withholding: government.get_untracked(),
                ..before.clone()
            };
            save_details(root.clone(), next, busy, details, editing, "Business saved. New returns prefill it.");
        };
        let start_at = element_ref();
        view()
            .child(
                grid()
                    .child(labelled("Line of business", field_input(line, "PSIC code and description, like 47190 Retail sale")))
                    .child(labelled("Business start or incorporation", crate::screens::taxpayer::date_input(start, start_at))),
            )
            .child(block("EOPT tier", pills(tiers, tier, true)))
            .child(view().pt(px(16.0)).child(tick_row("Government withholding entity", 13.0, move || government.get(), move |on| government.set(on))))
            .child(edit_bar(editing, busy, save))
            .into_element()
    })
}

fn branch_section(b: bir_app_core::Branch, _first: bool) -> impl IntoElement {
    let meta = format!("{} · {}", b.name, b.code);
    section("Branch", meta, false, false, move |editing| {
        let b = b.clone();
        if !editing.get_untracked() {
            return grid()
                .child(fact("Branch name", b.name.clone()))
                .child(fact("Branch code", b.code.clone()))
                .child(fact("Branch TIN", b.tin_display.clone()))
                .into_element();
        }
        let name = signal(b.name.clone());
        let busy = signal(false);
        let tin = b.tin.clone();
        let save = move || {
            let tin = tin.clone();
            let n = name.get_untracked().trim().to_owned();
            busy.set(true);
            task(async move {
                let done = call(move |app| app.rename_branch(&tin, &n)).await;
                busy.set(false);
                match done {
                    Ok(()) => {
                        editing.set(false);
                        store().notify(NoticeTone::Success, "Branch saved.");
                        store().reload_taxpayers(None);
                    }
                    Err(why) => store().notify(NoticeTone::Danger, format!("Not saved: {why}")),
                }
            });
        };
        view()
            .child(grid().child(labelled("Branch name", field_input(name, "Head office"))).child(fact("Branch code", b.code.clone())))
            .child(edit_bar(editing, busy, save))
            .into_element()
    })
}

fn address_section(b: bir_app_core::Branch) -> impl IntoElement {
    let inherited = b.inherited.len();
    let meta = if b.is_head_office || inherited == 0 { "RDO, address, email".to_owned() } else { format!("{inherited} of 5 from head office") };
    section("Address and contact", meta, false, true, move |editing| {
        let b = b.clone();
        let f = b.facts.clone();
        if !editing.get_untracked() {
            let from_head = |key: &str| !b.is_head_office && b.inherited.iter().any(|k| k == key);
            let with_chip = move |label: &'static str, key: &'static str, value: String| {
                let head = from_head(key);
                view().w(px(260.0)).gap(px(2.0)).child(view().flex_row().gap(px(6.0)).items_center().child(caption(label)).child(when(head, || chip("Head office", Tone::Neutral)))).child(
                    text(if value.trim().is_empty() { "—".to_owned() } else { value }).text_size(px(13.0)).text_color(if head { theme::FG2 } else { theme::FG }),
                )
            };
            return grid()
                .child(with_chip("RDO", "rdo_code", f.rdo_code.clone()))
                .child(with_chip("Registered address", "registered_address", f.registered_address.clone()))
                .child(with_chip("ZIP code", "zip_code", f.zip_code.clone()))
                .child(with_chip("Email", "email", f.email.clone()))
                .child(with_chip("Phone", "phone", f.phone.clone()))
                .into_element();
        }
        let rdo = signal(f.rdo_code.clone());
        let address = signal(f.registered_address.clone());
        let zip = signal(f.zip_code.clone());
        let email = signal(f.email.clone());
        let phone = signal(f.phone.clone());
        let busy = signal(false);
        let tin = b.tin.clone();
        let before = f.clone();
        let save = move || {
            let tin = tin.clone();
            let mut changes: Vec<(&'static str, String)> = Vec::new();
            for (key, sig, old) in [
                ("rdo_code", rdo, before.rdo_code.clone()),
                ("registered_address", address, before.registered_address.clone()),
                ("zip_code", zip, before.zip_code.clone()),
                ("email", email, before.email.clone()),
                ("phone", phone, before.phone.clone()),
            ] {
                let now = sig.get_untracked().trim().to_owned();
                if now != old {
                    changes.push((key, now));
                }
            }
            if changes.is_empty() {
                editing.set(false);
                return;
            }
            busy.set(true);
            task(async move {
                let done = call(move |app| {
                    for (key, value) in &changes {
                        app.set_branch_fact(&tin, key, value)?;
                    }
                    Ok(())
                })
                .await;
                busy.set(false);
                match done {
                    Ok(()) => {
                        editing.set(false);
                        store().notify(NoticeTone::Success, "Address and contact saved. New returns prefill them.");
                        store().reload_taxpayers(None);
                    }
                    Err(why) => store().notify(NoticeTone::Danger, format!("Not saved: {why}")),
                }
            });
        };
        view()
            .child(
                grid()
                    .child(labelled("RDO", field_input(rdo, "043")))
                    .child(labelled("Registered address", field_input(address, "")))
                    .child(labelled("ZIP code", field_input(zip, "1600")))
                    .child(labelled("Email", field_input(email, "name@example.com")))
                    .child(labelled("Phone", field_input(phone, ""))),
            )
            .child(edit_bar(editing, busy, save))
            .into_element()
    })
}

/// The taxpayer's branches, the selected one marked; Add branch opens an
/// inline band.
fn branches(t: Taxpayer, selected: String) -> impl IntoElement {
    let st = store();
    // Debug builds: `BUWIZ_SHEET=add-branch` opens the band (evidence.sh).
    let adding = signal(cfg!(debug_assertions) && std::env::var("BUWIZ_SHEET").as_deref() == Ok("add-branch"));
    let code = signal(String::new());
    let name = signal(String::new());
    let busy = signal(false);
    let root = t.tin_root.clone();
    let root2 = root.clone();
    let open_band = move || {
        adding.set(true);
        name.set(String::new());
        let root = root2.clone();
        task(async move {
            if let Ok(next) = call(move |app| app.suggest_branch_code(&root)).await {
                code.set(next);
            }
        });
    };
    let save = move || {
        let root = root.clone();
        let (c, n) = (code.get_untracked().trim().to_owned(), name.get_untracked().trim().to_owned());
        if c.len() != 5 || !c.chars().all(|ch| ch.is_ascii_digit()) {
            st.notify(NoticeTone::Danger, "Branch code is 5 digits, like 00001.");
            return;
        }
        busy.set(true);
        task(async move {
            let done = call(move |app| app.add_branch(&root, &c, &n)).await;
            busy.set(false);
            match done {
                Ok(()) => {
                    adding.set(false);
                    st.notify(NoticeTone::Success, "Branch added.");
                    st.reload_taxpayers(None);
                }
                Err(why) => st.notify(NoticeTone::Danger, format!("Not added: {why}")),
            }
        });
    };
    let mut group = list_group();
    for (i, b) in t.branches.iter().enumerate() {
        let tin = b.tin.clone();
        let on = b.tin == selected;
        let index = signal(i);
        group = group.child(
            list_row(index.read_only())
                .cursor_pointer()
                .hover(|s| s.bg(theme::HOVER))
                .on_press(move || st.select(&tin))
                .child(code_text(b.code.clone()))
                .child(name_cell(b.name.clone(), format!("{} · RDO {}", b.tin_display, if b.facts.rdo_code.is_empty() { "—" } else { &b.facts.rdo_code })))
                .child(when(on, || chip("Selected", Tone::Accent)))
                .child(when(b.is_head_office, || chip("Head office", Tone::Neutral))),
        );
    }
    if t.branches.is_empty() {
        group = group.child(empty_row("No branches."));
    }
    view()
        .gap(px(8.0))
        .child(group)
        .child(when(move || !adding.get(), move || view().flex_row().child(button_sm("Add branch", Variant::Ghost, Some(Icon::Plus), || false, open_band.clone()))))
        .child(when(move || adding.get(), move || {
            let save = save.clone();
            view()
                .p(px(16.0))
                .gap(px(12.0))
                .rounded(px(theme::R_CARD))
                .bg(theme::ACCENT_SOFT)
                .child(text("Add a branch").text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap(px(12.0))
                        .child(view().w(px(120.0)).gap(px(3.0)).child(caption("Branch code")).child(field_input(code, "00001")))
                        .child(view().w(px(260.0)).gap(px(3.0)).child(caption("Branch name")).child(field_input(name, "Makati branch"))),
                )
                .child(text("Its address and contact start as the head office's; change them under Address and contact.").text_size(px(12.0)).text_color(theme::FG2))
                .child(
                    view()
                        .flex_row()
                        .justify_end()
                        .gap(px(8.0))
                        .child(button_sm("Cancel", Variant::Ghost, None, || false, move || adding.set(false)))
                        .child(button_sm(move || if busy.get() { "Adding…" } else { "Add branch" }, Variant::Primary, None, move || busy.get(), save)),
                )
        }))
}
