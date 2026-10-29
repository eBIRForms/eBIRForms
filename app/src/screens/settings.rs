//! Settings: appearance (kept in the database's settings), due-date
//! reminders on or off, and backup and restore through bir-core's
//! export and import (D17: tax profiles only). The phone shows the same as a
//! short grouped list, reached from the taxpayer sheet.

use std::path::PathBuf;

use nsh_core::prelude::*;

use crate::live::store;
use crate::screens::{content, page_title};
use crate::store::{NoticeTone, SignIn, ThemePref, call, task};
use crate::theme;
use crate::widgets::{CLEAR, Variant, button_sm, list_group, section_title, segmented};

const THEMES: [ThemePref; 3] = [ThemePref::System, ThemePref::Light, ThemePref::Dark];

/// Where a backup goes: the save panel on macOS; on a phone the app's own
/// files, dated.
fn backup_target() -> Option<PathBuf> {
    let name = format!("Buwiz backup {}.zip", crate::store::today());
    #[cfg(target_os = "macos")]
    {
        crate::mac::save_panel(&name, "zip")
    }
    #[cfg(not(target_os = "macos"))]
    {
        let db = std::env::var_os("BIR_DATABASE_PATH").map(PathBuf::from)?;
        Some(db.parent()?.join(name))
    }
}

/// The backup to restore: the open panel on macOS; on a phone the newest
/// backup in the app's files.
fn restore_source() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        crate::mac::open_panel("zip")
    }
    #[cfg(not(target_os = "macos"))]
    {
        let db = std::env::var_os("BIR_DATABASE_PATH").map(PathBuf::from)?;
        let dir = db.parent()?.to_path_buf();
        std::fs::read_dir(dir)
            .ok()?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("Buwiz backup")))
            .max()
    }
}

pub(crate) fn back_up() {
    let st = store();
    task(async move {
        let Ok(Some(path)) = blocking(backup_target).await else { return };
        let shown = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match call(move |app| app.export_backup(&path)).await {
            Ok(()) => st.notify(NoticeTone::Success, format!("Backed up to {shown}.")),
            Err(why) => st.notify(NoticeTone::Danger, format!("Not backed up: {why}")),
        }
    });
}

pub(crate) fn restore() {
    let st = store();
    task(async move {
        let Ok(Some(path)) = blocking(restore_source).await else {
            if crate::mobile::phone() {
                st.notify(NoticeTone::Info, "No backup file in the app's files yet. Back up first.");
            }
            return;
        };
        match call(move |app| app.restore_backup(&path)).await {
            Ok(_) => {
                st.notify(NoticeTone::Success, "Restored from the backup.");
                st.reload_taxpayers(None);
                st.reload_lists();
            }
            Err(why) => st.notify(NoticeTone::Danger, format!("Not restored: {why}")),
        }
    });
}

/// A switch, `on` in accent.
pub(crate) fn switch(on: impl Fn() -> bool + Clone + 'static, flip: impl Fn() + 'static, label: &'static str) -> impl IntoElement {
    let (on1, on2) = (on.clone(), on.clone());
    view()
        .w(px(40.0))
        .h(px(24.0))
        .p(px(2.0))
        .rounded(px(theme::R_FULL))
        .flex_row()
        .bg(move || if on2() { theme::ACCENT } else { theme::BORDER_STRONG })
        .cursor_pointer()
        .label(label)
        .on_press(flip)
        .child(view().w(move || px(if on1() { 16.0 } else { 0.0 })))
        .child(view().size(px(20.0)).rounded(px(theme::R_FULL)).bg(theme::tone(theme::hsla(0.0, 0.0, 1.0, 1.0), theme::hsla(0.0, 0.0, 1.0, 1.0))).shadow_sm())
}

/// One settings row: a title over a line, the control at the right.
fn row(title: &'static str, line: impl IntoReactive<SharedString>, first: bool, control: impl IntoElement) -> impl IntoElement {
    let phone = crate::mobile::phone();
    view()
        .flex_row()
        .items_center()
        .gap(px(12.0))
        .min_h(px(if phone { 60.0 } else { 52.0 }))
        .px(px(16.0))
        .py(px(8.0))
        .border_t(px(if first { 0.0 } else { 1.0 }))
        .border_color(theme::BORDER)
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(text(title).text_size(px(if phone { 15.0 } else { 13.0 })).font_weight(FontWeight::MEDIUM).text_color(theme::FG))
                .child(text(line).text_size(px(12.0)).text_color(theme::FG2)),
        )
        .child(control)
}

/// Sending: signed out, "Sign in to send" (the device flow: the code big,
/// the sign-in page in the browser, polled until done); signed in, the
/// account and Sign out.
fn sending_section() -> impl IntoElement {
    let st = store();
    let phone = crate::mobile::phone();
    list_group().child(each(
        move || vec![st.sign_in.get()],
        |s| format!("{s:?}"),
        move |s, _| match s.get_untracked() {
            SignIn::Unknown | SignIn::Starting => row("Sending", "Getting ready…", true, view()).into_element(),
            SignIn::SignedOut => row(
                "Sign in to send",
                "Returns you queue send through your Buwiz account. Until you sign in they wait in the queue.",
                true,
                action("Sign in to send", move || st.sign_in_start()),
            )
            .into_element(),
            SignIn::SignedIn { email } => row(
                "Signed in",
                email.unwrap_or_else(|| "Your Buwiz account".to_owned()),
                true,
                action("Sign out", move || st.sign_out()),
            )
            .into_element(),
            SignIn::Pending { user_code, uri, uri_complete } => view()
                .p(px(16.0))
                .gap(px(10.0))
                .child(text("Approve this code on your Buwiz account").text_size(px(if phone { 15.0 } else { 13.0 })).font_weight(FontWeight::MEDIUM).text_color(theme::FG))
                .child(text(user_code).text_size(px(34.0)).font_weight(FontWeight::BOLD).font_family(crate::screens::form_mono()).text_color(theme::FG).label("Sign-in code"))
                .child(text(format!("Open {uri} and enter the code, or open the page with the code already filled in. This page waits until you approve it.")).text_size(px(12.0)).text_color(theme::FG2))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap(px(8.0))
                        .child(action("Open the sign-in page", move || {
                            let _ = open_url(&uri_complete);
                        }))
                        .child(button_sm("Cancel", Variant::Ghost, None, || false, move || st.sign_in_cancel())),
                )
                .into_element(),
        },
    ))
}

/// A row's action: a text button on a phone, a small button on desktop.
fn action(label: &'static str, act: impl Fn() + 'static) -> AnyElement {
    if crate::mobile::phone() {
        text(label).h(px(44.0)).text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::ACCENT).cursor_pointer().label(label).on_press(act).into_element()
    } else {
        button_sm(label, Variant::Secondary, None, || false, act).label(label).into_element()
    }
}

pub(crate) fn screen() -> impl IntoElement {
    let st = store();
    let phone = crate::mobile::phone();
    let theme_line = move || match st.theme.get() {
        ThemePref::System => "Follows your system",
        ThemePref::Light => "Always light",
        ThemePref::Dark => "Always dark",
    };
    let reminders = move || st.reminders.get();
    let pressable = |label: &'static str, act: fn()| {
        if crate::mobile::phone() {
            text(label).text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme::ACCENT).cursor_pointer().on_press(act).into_element()
        } else {
            button_sm(label, Variant::Secondary, None, || false, act).into_element()
        }
    };
    let db_path = move || st.app().map(|a| a.database_path().display().to_string()).unwrap_or_default();
    content(
        view()
            .gap(px(4.0))
            .child(when(!phone, || page_title("Settings")))
            .child(when(!phone, || view().h(px(16.0))))
            .child(section_title("Sending", true))
            .child(sending_section())
            .child(section_title("Look", false))
            .child(list_group().child(row(
                "Appearance",
                theme_line,
                true,
                segmented(vec!["System".into(), "Light".into(), "Dark".into()], move || THEMES.iter().position(|t| *t == st.theme.get()).unwrap_or(0), move |i| st.set_theme(THEMES[i])),
            )))
            .child(section_title("Reminders", false))
            .child(list_group().child(row(
                "Due-date reminders",
                move || if reminders() { "A note when you open Buwiz: returns due within 3 days" } else { "Off" },
                true,
                switch(reminders, move || st.set_reminders(!st.reminders.get_untracked()), "Due-date reminders"),
            )))
            .child(section_title("Your data", false))
            .child(
                list_group()
                    .child(row("Back up", "Every tax profile, with its branches and returns, in one file", true, pressable("Back up…", back_up)))
                    .child(row("Restore", "From a Buwiz backup file", false, pressable("Restore…", restore))),
            )
            .child(section_title("About", false))
            .child(
                list_group()
                    .child(row("Version", env!("CARGO_PKG_VERSION"), true, view().bg(CLEAR)))
                    .child(row("Database", db_path, false, view())),
            ),
    )
}
