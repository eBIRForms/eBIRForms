//! The taxpayer switcher at the top of the sidebar, and the New taxpayer
//! dialog.

use bir_app_core::{BranchFacts, NewTaxpayer, Taxpayer};
use nsh_core::prelude::*;

use crate::live::store;
use crate::icons::Icon;
use crate::store::{Loaded, NoticeTone, Selected, call, task};
use crate::theme;
use crate::picker::{List, Picker, picker};
use crate::widgets::{Note, Variant, avatar, button, button_sm, caption, field_block, focus_ring, group_label, nav_row, panel_header};

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

/// A field of the New taxpayer form, in the desktop's order (the order
/// [`problems`] reports them in).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    Tin,
    Birth,
    Name,
    Street,
    City,
    Zip,
    Rdo,
    Email,
}

/// What the New taxpayer form holds, as [`problems`] checks it.
#[derive(Clone, Debug, Default)]
struct Draft {
    tin: String,
    birth: String,
    individual: bool,
    name: String,
    street: String,
    city: Option<String>,
    zip: Option<String>,
    rdo: Option<String>,
    email: String,
    today: chrono::NaiveDate,
}

/// Every field of `d` that cannot be saved as it is, with what to do about
/// it, in the form's order.
fn problems(d: &Draft) -> Vec<(Field, &'static str)> {
    let mut out = Vec::new();
    if d.tin.chars().filter(char::is_ascii_digit).count() != 9 {
        out.push((Field::Tin, "Enter all 9 digits, like 000-123-453."));
    }
    if d.individual {
        let digits = d.birth.chars().filter(char::is_ascii_digit).count();
        let date = if digits == 8 { bir_app_core::display::parse_typed_date(&d.birth) } else { None };
        match date {
            _ if d.birth.trim().is_empty() => out.push((Field::Birth, "Enter the birth date, like 01/15/1990.")),
            None => out.push((Field::Birth, "Enter a real date as MM/DD/YYYY.")),
            Some(date) if date > d.today => out.push((Field::Birth, "The birth date cannot be in the future.")),
            Some(_) => {}
        }
    }
    if d.name.trim().is_empty() {
        out.push((Field::Name, "Enter the registered name."));
    }
    if d.street.trim().is_empty() {
        out.push((Field::Street, "Enter the street address."));
    }
    if d.city.is_none() {
        out.push((Field::City, "Pick the city or municipality from the list."));
    }
    if d.zip.is_none() {
        out.push((Field::Zip, "Pick the ZIP code from the list."));
    }
    if d.rdo.is_none() {
        out.push((Field::Rdo, "Pick the RDO from the list."));
    }
    if !email_ok(&d.email) {
        out.push((Field::Email, "Enter an email like name@example.com."));
    }
    out
}

/// The line under a field while it shows no error.
fn hint(field: Field) -> Option<&'static str> {
    match field {
        Field::Tin => Some("9 digits"),
        Field::Birth => Some("MM/DD/YYYY"),
        Field::Name => Some("As on the Certificate of Registration"),
        Field::Street => Some("House, street, barangay"),
        Field::Email => Some("BIR sends the confirmation here."),
        Field::City | Field::Zip | Field::Rdo => None,
    }
}

/// The footer's line after Add taxpayer found `n` fields to fix.
fn fix_line(n: usize) -> String {
    format!("Fix {n} field{} to continue.", if n == 1 { "" } else { "s" })
}

/// The taxpayer `d` adds, once [`problems`] has none; `city` is the
/// picked [`bir_app_core::BirApp::cities`] entry.
fn new_taxpayer(d: &Draft, kind: &str, city: &bir_app_core::Choice) -> NewTaxpayer {
    NewTaxpayer {
        tin_root: d.tin.chars().filter(char::is_ascii_digit).collect(),
        name: d.name.trim().to_owned(),
        taxpayer_type: kind.to_owned(),
        birth_date: if d.individual { bir_app_core::display::parse_typed_date(&d.birth).map(|date| date.to_string()) } else { None },
        head_office: BranchFacts {
            rdo_code: d.rdo.clone().unwrap_or_default(),
            registered_address: bir_app_core::registered_address(&d.street, city),
            zip_code: d.zip.as_deref().map(bir_app_core::zip_code).unwrap_or_default().to_owned(),
            phone: String::new(),
            email: d.email.trim().to_owned(),
        },
    }
}

/// The New taxpayer form's state, shared by the desktop dialog and the
/// phone page: the values, which fields the filer has left (their errors
/// show), which one has the focus and is being typed in (its error waits),
/// and the save's state.
#[derive(Clone, Copy)]
struct Form {
    tin: Signal<String>,
    birth: Signal<String>,
    kind: Signal<String>,
    name: Signal<String>,
    street: Signal<String>,
    city: Signal<Option<String>>,
    zip: Signal<Option<String>>,
    rdo: Signal<Option<String>>,
    email: Signal<String>,
    pickers: [Picker; 3],
    left: Signal<Vec<Field>>,
    typing: Signal<Option<Field>>,
    /// Add taxpayer was pressed with something to fix.
    tried: Signal<bool>,
    /// Why the save failed (bir-core's words).
    error: Signal<String>,
    busy: Signal<bool>,
}

impl Form {
    fn new() -> Self {
        Self {
            tin: signal(String::new()),
            birth: signal(String::new()),
            kind: signal("Individual".to_owned()),
            name: signal(String::new()),
            street: signal(String::new()),
            city: signal(None),
            zip: signal(None),
            rdo: signal(None),
            email: signal(String::new()),
            pickers: [Picker::new(), Picker::new(), Picker::new()],
            left: signal(Vec::new()),
            typing: signal(None),
            tried: signal(false),
            error: signal(String::new()),
            busy: signal(false),
        }
    }

    fn individual(self) -> bool {
        self.kind.get() == "Individual"
    }

    /// The form now (tracked).
    fn draft(self) -> Draft {
        Draft {
            tin: self.tin.get(),
            birth: self.birth.get(),
            individual: self.individual(),
            name: self.name.get(),
            street: self.street.get(),
            city: self.city.get(),
            zip: self.zip.get(),
            rdo: self.rdo.get(),
            email: self.email.get(),
            today: crate::store::today(),
        }
    }

    fn picker(self, field: Field) -> Option<Picker> {
        match field {
            Field::City => Some(self.pickers[0]),
            Field::Zip => Some(self.pickers[1]),
            Field::Rdo => Some(self.pickers[2]),
            _ => None,
        }
    }

    /// The filer left `field`: from now on its error shows.
    fn leave(self, field: Field) {
        if self.typing.get_untracked() == Some(field) {
            self.typing.set(None);
        }
        if !self.left.with_untracked(|l| l.contains(&field)) {
            self.left.update(|l| l.push(field));
        }
    }

    /// The error under `field` (tracked): once the filer left it or tried
    /// to add, unless they are typing in it or its list is open; gone as
    /// soon as the value is right.
    fn error_of(self, field: Field) -> Option<&'static str> {
        if !self.left.with(|l| l.contains(&field)) || self.typing.get() == Some(field) || self.picker(field).is_some_and(|p| p.open.get()) {
            return None;
        }
        problems(&self.draft()).into_iter().find(|(f, _)| *f == field).map(|(_, why)| why)
    }

    /// The message line under `field` (tracked).
    fn note(self, field: Field) -> Note {
        match self.error_of(field) {
            Some(why) => Note::Error(why),
            None => hint(field).map_or(Note::None, Note::Hint),
        }
    }

    /// The footer's line (tracked): a failed save, else how many fields
    /// stand in the way once Add taxpayer was pressed.
    fn footer(self) -> String {
        let error = self.error.get();
        if !error.is_empty() {
            return error;
        }
        let n = problems(&self.draft()).len();
        if self.tried.get() && n > 0 { fix_line(n) } else { String::new() }
    }

    /// Add taxpayer: with something to fix, every error shows and the
    /// first field to fix is returned; else the taxpayer is created.
    fn create(self) -> Option<Field> {
        if self.busy.get_untracked() {
            return None;
        }
        let draft = untracked(|| self.draft());
        let found = problems(&draft);
        if let Some((first, _)) = found.first() {
            self.tried.set(true);
            self.typing.set(None);
            self.left.set(found.iter().map(|(f, _)| *f).collect::<Vec<_>>().into_iter().chain(self.left.get_untracked()).collect());
            return Some(*first);
        }
        let Some(city) = draft.city.as_ref().and_then(|v| bir_app_core::BirApp::cities().into_iter().find(|c| &c.value == v)) else {
            return Some(Field::City);
        };
        let new = new_taxpayer(&draft, &self.kind.get_untracked(), &city);
        let st = store();
        self.busy.set(true);
        self.error.set(String::new());
        task(async move {
            let created = call(move |app| app.create_taxpayer(&new)).await;
            self.busy.set(false);
            match created {
                Ok(taxpayer) => {
                    let head = taxpayer.branches.first().map(|b| b.tin.clone());
                    st.reload_taxpayers(head);
                    st.new_taxpayer_open.set(false);
                    st.notify(NoticeTone::Success, format!("{} added.", taxpayer.name));
                }
                Err(why) => self.error.set(why),
            }
        });
        None
    }

    /// A text box of `field`: focus, typing and leaving tracked.
    fn track(self, field: Field, input: Input) -> Input {
        input.on_blur(move || self.leave(field))
    }

    /// The value of `field` changed while the filer typed in it.
    fn typed(self, field: Field) {
        if self.typing.get_untracked() != Some(field) {
            self.typing.set(Some(field));
        }
    }
}

fn desktop_new_taxpayer() -> impl IntoElement {
    let st = store();
    let form = Form::new();
    let at: [ElementRef; 5] = std::array::from_fn(|_| element_ref());
    let [tin_at, birth_at, name_at, street_at, email_at] = at;

    // Add taxpayer, then the focus on the first field to fix.
    let create = move || match form.create() {
        Some(Field::Tin) => tin_at.focus(),
        Some(Field::Birth) => birth_at.focus(),
        Some(Field::Name) => name_at.focus(),
        Some(Field::Street) => street_at.focus(),
        Some(Field::Email) => email_at.focus(),
        Some(field) => {
            if let Some(p) = form.picker(field) {
                p.focus();
            }
        }
        None => {}
    };
    // Escape closes the dialog, also from a text box (an open list takes
    // the first Escape itself).
    on_key_with("escape", KeyOptions { when: nsh_core::keymap::When::Always, repeat: false }, move |_| {
        match form.pickers.iter().find(|p| p.open.get_untracked()) {
            Some(p) => p.open.set(false),
            None => st.new_taxpayer_open.set(false),
        }
    });

    if cfg!(debug_assertions) {
        debug_hook(form, create);
    }

    let text_field = move |field: Field, value: Signal<String>, placeholder: &str, at: ElementRef| {
        form.track(
            field,
            crate::widgets::field_box(placeholder).value(value).element_ref(&at).on_change(move |v: &str| {
                value.set(v.to_owned());
                form.typed(field);
            }),
        )
    };
    let individual = move || form.individual();
    let birth = form
        .track(Field::Birth, date_input(form.birth, birth_at).on_change(move |_: &str| form.typed(Field::Birth)))
        .placeholder(move || if individual() { "01/15/1990".to_owned() } else { "Individuals only".to_owned() })
        .disabled(move || !individual())
        .readonly(move || !individual())
        .focusable(individual)
        .opacity(move || if individual() { 1.0_f32 } else { 0.5 });
    let tin = form.track(Field::Tin, tin_input_desktop(form.tin, tin_at).on_change(move |_: &str| form.typed(Field::Tin)));

    let note = move |field: Field| move || form.note(field);
    let pick = move |field: Field, list: List, value: Signal<Option<String>>| {
        let state = form.picker(field).expect("a picker field");
        field_block(list.label(), picker(list, value, state, false, move || form.leave(field)), note(field))
    };

    // 4 px round the fields, inside the scroll view, so it does not clip
    // their focus rings.
    let fields = view()
        .p(px(4.0))
        .gap(px(8.0))
        .child(
            view()
                .flex_row()
                .gap(px(12.0))
                .child(field_block("TIN", tin, note(Field::Tin)).w(px(270.0)))
                .child(field_block("Birth date", birth, move || if individual() { form.note(Field::Birth) } else { Note::None }).w(px(270.0))),
        )
        .child(field_block("Registered name", text_field(Field::Name, form.name, "Juana Santos", name_at), note(Field::Name)))
        .child(field_block("Taxpayer type", type_radios(form), || Note::None))
        .child(field_block("Street address", text_field(Field::Street, form.street, "1 Sample Street", street_at), note(Field::Street)))
        .child(pick(Field::City, List::City, form.city))
        .child(
            view()
                .flex_row()
                .gap(px(12.0))
                .child(pick(Field::Zip, List::Zip, form.zip).w(px(220.0)))
                .child(pick(Field::Rdo, List::Rdo, form.rdo).flex_1()),
        )
        .child(field_block("Email", text_field(Field::Email, form.email, "name@example.com", email_at), note(Field::Email)));

    let card = view()
        .w(move || px((window().width.0 - safe_area().left.0 - safe_area().right.0 - 24.0).min(600.0)))
        // One height whatever shows: the fields scroll only when the
        // window is too short for them.
        .h(move || px((window().height.0 - safe_area().top.0 - safe_area().bottom.0 - 24.0).min(682.0)))
        .p(px(24.0))
        .rounded(px(theme::R_DIALOG))
        .bg(theme::SURFACE)
        .border_1()
        .border_color(theme::BORDER)
        .shadow_lg()
        .child(
            view()
                .gap(px(2.0))
                .child(text("New taxpayer").h(px(20.0)).text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::FG))
                .child(text("Creates the head office (branch 00000). Add more branches later.").h(px(18.0)).text_size(px(13.0)).text_color(theme::FG2).truncate()),
        )
        .child(scroll().flex_1().min_h(px(0.0)).mt(px(8.0)).mb(px(-4.0)).mx(px(-4.0)).child(fields))
        .child(
            view()
                .h(px(theme::CONTROL))
                .mt(px(12.0))
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(text(move || form.footer()).flex_1().min_w(px(0.0)).text_size(px(12.0)).text_color(theme::DANGER).truncate())
                .child(button("Cancel", Variant::Secondary, || false, move || st.new_taxpayer_open.set(false)))
                .child(button(move || if form.busy.get() { "Adding…" } else { "Add taxpayer" }, Variant::Primary, move || form.busy.get(), create)),
        );

    view()
        .absolute()
        .inset(px(0.0))
        .items_center()
        .justify_center()
        .pt(move || px(safe_area().top.0 + 12.0))
        .pb(move || px(safe_area().bottom.0 + 12.0))
        .child(view().absolute().inset(px(0.0)).bg(theme::SCRIM).focusable(false).on_press(move || st.new_taxpayer_open.set(false)))
        .child(card)
}

/// Debug builds: `BUWIZ_TAXPAYER` drives the dialog for screenshots
/// (store.rs `launch_hooks`): `errors` presses Add taxpayer on the empty
/// form, `filled` fills every field, `city:<query>` (`zip:`, `rdo:`)
/// opens that list searching the query.
fn debug_hook(form: Form, create: impl Fn() + 'static) {
    let Some(how) = std::env::var("BUWIZ_TAXPAYER").ok().filter(|v| !v.is_empty()) else { return };
    store().owner.run(|| {
        set_timeout(std::time::Duration::from_millis(800), move || match how.split_once(':') {
            Some((list, query)) => {
                let field = match list {
                    "zip" => Field::Zip,
                    "rdo" => Field::Rdo,
                    _ => Field::City,
                };
                if let Some(p) = form.picker(field) {
                    p.search(query);
                }
            }
            None if how == "errors" => create(),
            None => {
                form.tin.set("000-123-453".to_owned());
                form.birth.set("01/15/1990".to_owned());
                form.name.set("Juana Santos".to_owned());
                form.street.set("1 Sample Street".to_owned());
                form.city.set(Some("City of Pasig, Metro Manila".to_owned()));
                form.zip.set(Some("1600 - Pasig CPO".to_owned()));
                form.rdo.set(Some("43A".to_owned()));
                form.email.set("juana@example.com".to_owned());
            }
        })
    });
}

/// The taxpayer type: six radios in one row, one stop of Tab whose arrow
/// keys move and check.
fn type_radios(form: Form) -> impl IntoElement {
    let mut group = nsh_base::radio_group().h(px(theme::CONTROL)).flex_row().items_center().gap(px(4.0));
    for label in TYPES {
        let checked = move || form.kind.get() == label;
        group = group.child(
            nsh_base::radio()
                .checked(checked)
                .on_change(move |_: &bool| {
                    form.kind.set(label.to_owned());
                    if label != "Individual" {
                        form.birth.set(String::new());
                    }
                })
                .h(px(28.0))
                .px(px(4.0))
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .rounded(px(theme::R_CTL))
                .border_1()
                .border_color(crate::widgets::CLEAR)
                .cursor_pointer()
                .focus(focus_ring)
                .child(
                    view()
                        .size(px(14.0))
                        .items_center()
                        .justify_center()
                        .rounded(px(theme::R_FULL))
                        .border(px(1.5))
                        .border_color(move || if checked() { theme::ACCENT } else { theme::BORDER_STRONG })
                        .child(view().size(px(6.0)).rounded(px(theme::R_FULL)).bg(theme::ACCENT).opacity(move || if checked() { 1.0_f32 } else { 0.0 })),
                )
                .child(text(label).text_size(px(12.0)).text_color(theme::FG)),
        );
    }
    group
}

// ---------------------------------------------------------------- phone

/// The phone's New taxpayer: a full-screen page in the form's style —
/// stacked fields, the TIN as one box with dashes, the type as option
/// cards, the birth date only for an individual, the city, ZIP code and RDO
/// each picked from its list (a sheet), a required email — with the
/// keyboard row while typing and the next-step bar ("Next · TIN", then
/// "Add taxpayer").
fn phone_new_taxpayer() -> impl IntoElement {
    let st = store();
    let form = Form::new();
    let Form { name, tin, kind, street, birth, email, busy, .. } = form;
    let typing = signal(None::<usize>);
    let refs: Vec<ElementRef> = (0..5).map(|_| element_ref()).collect();

    let individual = move || kind.get() == "Individual";
    // The next step: the first field to fix in the page's order, a box
    // (its index in `refs`) or a list.
    let order = [Field::Name, Field::Tin, Field::Birth, Field::Street, Field::City, Field::Zip, Field::Rdo, Field::Email];
    let missing = move || -> Option<Field> {
        let found = problems(&form.draft());
        order.into_iter().find(|f| found.iter().any(|(p, _)| p == f))
    };
    let label_of = |f: Field| match f {
        Field::Name => "Registered name",
        Field::Tin => "TIN",
        Field::Birth => "Birth date",
        Field::Street => "Street address",
        Field::City => "City",
        Field::Zip => "ZIP code",
        Field::Rdo => "RDO",
        Field::Email => "Email",
    };
    let ref_of = |f: Field| match f {
        Field::Name => Some(0),
        Field::Tin => Some(1),
        Field::Street => Some(2),
        Field::Birth => Some(3),
        Field::Email => Some(4),
        _ => None,
    };

    // The error under a field, the kit's way: a line that shows while it
    // has one.
    let error_line = move |field: Field| when(move || form.error_of(field).is_some(), move || text(move || form.error_of(field).unwrap_or_default()).mt(px(4.0)).text_size(px(12.0)).text_color(theme::DANGER));
    // A stacked field: the label above, the box under it, a line between.
    let field = move |label: &'static str, hint: &'static str, input: Input, at: ElementRef, index: usize, which: Field| {
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
                        form.leave(which);
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
            .child(error_line(which))
    };
    let text_input = move |value: Signal<String>, placeholder: &'static str, which: Field| {
        nsh_base::input()
            .value(value)
            .on_change(move |v: &str| {
                value.set(v.to_owned());
                form.typed(which);
            })
            .placeholder(placeholder)
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
            form.typed(Field::Tin);
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

    let pick_row = move |which: Field, list: List, value: Signal<Option<String>>| {
        let state = form.picker(which).expect("a picker field");
        view()
            .px(px(18.0))
            .py(px(10.0))
            .border_b(px(1.0))
            .border_color(theme::BORDER)
            .bg(move || if state.focused.get() { theme::ACCENT_SOFT } else { theme::SURFACE })
            .on_press(move || state.focus())
            .child(text(list.label()).text_size(px(13.0)).text_color(theme::FG2))
            .child(picker(list, value, state, true, move || form.leave(which)))
            .child(error_line(which))
    };

    let r = refs.clone();
    let body = view()
        .child(field("Registered name", "As on the Certificate of Registration", text_input(name, "Juana Santos", Field::Name), r[0], 0, Field::Name))
        .child(field("TIN", "9 digits", tin_input, r[1], 1, Field::Tin))
        .child(types)
        .child(when(individual, {
            let r = r.clone();
            move || {
                let input = slashed(birth, r[3]).on_change(move |_: &str| form.typed(Field::Birth)).placeholder("01/15/1990").keyboard_type(KeyboardType::Number);
                field("Birth date", "MM/DD/YYYY", input, r[3], 3, Field::Birth)
            }
        }))
        .child(field("Street address", "House, street, barangay", text_input(street, "1 Sample Street", Field::Street), r[2], 2, Field::Street))
        .child(pick_row(Field::City, List::City, form.city))
        .child(pick_row(Field::Zip, List::Zip, form.zip))
        .child(pick_row(Field::Rdo, List::Rdo, form.rdo))
        .child(field("Email", "BIR sends the confirmation here.", text_input(email, "name@example.com", Field::Email).keyboard_type(KeyboardType::Email), r[4], 4, Field::Email))
        .child(when(move || !form.error.get().is_empty(), move || text(move || form.error.get()).mx(px(18.0)).mt(px(8.0)).text_size(px(12.0)).text_color(theme::DANGER)))
        .child(view().h(move || px(320.0 + safe_area().bottom.0)));

    // The boxes ↑ ↓ move through, in the page's order (the birth date only
    // for an individual).
    let boxes = move || -> Vec<usize> { if individual() { vec![0, 1, 3, 2, 4] } else { vec![0, 1, 2, 4] } };
    let step = {
        let refs = refs.clone();
        move |by: isize| {
            let order = boxes();
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
            .on_press(move || match untracked(missing) {
                Some(f) => match (ref_of(f), form.picker(f)) {
                    (Some(i), _) => refs[i].focus(),
                    (None, Some(p)) => p.focus(),
                    (None, None) => {}
                },
                None => {
                    form.create();
                }
            })
            .child(
                text(move || match missing() {
                    Some(f) => format!("Next · {}", label_of(f)),
                    None if busy.get() => "Adding…".to_owned(),
                    None => "Add taxpayer".to_owned(),
                })
                .flex_1()
                .text_size(px(15.0))
                .font_weight(FontWeight::BOLD)
                .text_color(move || if missing().is_none() { theme::PRIMARY_FG } else { theme::SURFACE }),
            )
    };

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
fn date_input(value: Signal<String>, at: ElementRef) -> Input {
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
fn tin_input_desktop(value: Signal<String>, at: ElementRef) -> Input {
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

/// Whether `email` looks like an address: something@domain.tld, no spaces.
fn email_ok(email: &str) -> bool {
    let e = email.trim();
    e.contains('@') && e.rsplit('@').next().is_some_and(|d| d.contains('.')) && !e.contains(' ')
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

    /// A form that can be saved: Juana Santos in Pasig.
    fn complete() -> Draft {
        Draft {
            tin: "000-123-453".to_owned(),
            birth: "01/15/1990".to_owned(),
            individual: true,
            name: "Juana Santos".to_owned(),
            street: "1 Sample Street".to_owned(),
            city: Some("City of Pasig, Metro Manila".to_owned()),
            zip: Some("1600 - Pasig CPO".to_owned()),
            rdo: Some("43A".to_owned()),
            email: "juana@example.com".to_owned(),
            today: chrono::NaiveDate::from_ymd_opt(2026, 10, 31).expect("a date"),
        }
    }

    fn only(d: Draft) -> Vec<(Field, &'static str)> {
        problems(&d)
    }

    #[test]
    fn a_complete_form_has_no_problems() {
        assert!(problems(&complete()).is_empty());
    }

    #[test]
    fn an_empty_form_lists_every_field_in_the_forms_order() {
        let empty = Draft { individual: true, today: complete().today, ..Draft::default() };
        assert_eq!(
            problems(&empty),
            [
                (Field::Tin, "Enter all 9 digits, like 000-123-453."),
                (Field::Birth, "Enter the birth date, like 01/15/1990."),
                (Field::Name, "Enter the registered name."),
                (Field::Street, "Enter the street address."),
                (Field::City, "Pick the city or municipality from the list."),
                (Field::Zip, "Pick the ZIP code from the list."),
                (Field::Rdo, "Pick the RDO from the list."),
                (Field::Email, "Enter an email like name@example.com."),
            ]
        );
        assert_eq!(fix_line(problems(&empty).len()), "Fix 8 fields to continue.");
        assert_eq!(fix_line(1), "Fix 1 field to continue.");
    }

    #[test]
    fn each_rule_says_what_to_do() {
        assert_eq!(only(Draft { tin: "000-123-45".to_owned(), ..complete() }), [(Field::Tin, "Enter all 9 digits, like 000-123-453.")]);
        assert_eq!(only(Draft { birth: "01/15/19".to_owned(), ..complete() }), [(Field::Birth, "Enter a real date as MM/DD/YYYY.")]);
        assert_eq!(only(Draft { birth: "02/30/1990".to_owned(), ..complete() }), [(Field::Birth, "Enter a real date as MM/DD/YYYY.")]);
        assert_eq!(only(Draft { birth: "01/15/2027".to_owned(), ..complete() }), [(Field::Birth, "The birth date cannot be in the future.")]);
        assert_eq!(only(Draft { name: "  ".to_owned(), ..complete() }), [(Field::Name, "Enter the registered name.")]);
        assert_eq!(only(Draft { street: String::new(), ..complete() }), [(Field::Street, "Enter the street address.")]);
        assert_eq!(only(Draft { city: None, ..complete() }), [(Field::City, "Pick the city or municipality from the list.")]);
        assert_eq!(only(Draft { zip: None, ..complete() }), [(Field::Zip, "Pick the ZIP code from the list.")]);
        assert_eq!(only(Draft { rdo: None, ..complete() }), [(Field::Rdo, "Pick the RDO from the list.")]);
        assert_eq!(only(Draft { email: "juana@example".to_owned(), ..complete() }), [(Field::Email, "Enter an email like name@example.com.")]);
    }

    #[test]
    fn only_an_individual_needs_a_birth_date() {
        assert!(problems(&Draft { individual: false, birth: String::new(), ..complete() }).is_empty());
    }

    #[test]
    fn hints_name_what_goes_in_the_box() {
        assert_eq!(hint(Field::Tin), Some("9 digits"));
        assert_eq!(hint(Field::Birth), Some("MM/DD/YYYY"));
        assert_eq!(hint(Field::Name), Some("As on the Certificate of Registration"));
        assert_eq!(hint(Field::Street), Some("House, street, barangay"));
        assert_eq!(hint(Field::Email), Some("BIR sends the confirmation here."));
        assert_eq!(hint(Field::City), None);
    }

    #[test]
    fn the_new_taxpayer_stores_the_address_zip_code_and_rdo_code() {
        let pasig = bir_app_core::BirApp::cities().into_iter().find(|c| c.title == "City of Pasig").expect("Pasig");
        let new = new_taxpayer(&complete(), "Individual", &pasig);
        assert_eq!(new.tin_root, "000123453");
        assert_eq!(new.birth_date.as_deref(), Some("1990-01-15"));
        assert_eq!(new.head_office.registered_address, "1 Sample Street, City of Pasig, Metro Manila");
        assert_eq!(new.head_office.zip_code, "1600");
        assert_eq!(new.head_office.rdo_code, "43A");
        assert_eq!(new_taxpayer(&Draft { individual: false, ..complete() }, "Corporation", &pasig).birth_date, None);
    }

    #[test]
    fn an_email_needs_a_domain_and_no_spaces() {
        assert!(email_ok(" juana@example.com "));
        assert!(!email_ok("juana@"));
        assert!(!email_ok("juana@example"));
        assert!(!email_ok("juana santos@example.com"));
    }
}
