//! Buwiz: an offline BIR filer on nsh, edited live by ra-jit.
//!
//! The root is a thin shell ([`App`]): a `dynamic` mounts
//! [`hot::hot_area`], which the daemon's patches cause to rebuild.
//! [`live::install`] is the only glue between ra-jit and the engine;
//! [`live::init`] anchors the app's store under the root owner so the
//! whole model survives remounts.

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
mod desktop;
mod hot;
mod icons;
#[cfg(target_os = "linux")]
mod linux;
pub mod live;
pub mod mobile;
mod screens;
mod session;
mod store;
mod theme;
mod widgets;
#[cfg(windows)]
mod windows;

use nsh_core::prelude::*;

pub const APP_NAME: &str = "Buwiz";
pub const APP_ID: &str = "com.buwiz.oss";

/// The app's UI set for nsh tooling (`nsh.json` `"rust"`).
#[native_module("buwiz/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
}

/// The shell: the hot area in a `dynamic` that rebuilds on each patch.
/// This view runs once, when the app starts, so its signals outlive every
/// remount.
#[component]
pub fn App() -> impl IntoElement {
    let remount = signal(0u64);
    let note = signal(String::new());
    live::install(remount, note);
    live::init();

    // A hidden title bar (full-size content, traffic lights kept): the
    // shell draws its own 40 px bar, as rox and bir-shell do.
    #[cfg(target_os = "macos")]
    nsh_core::element::desktop::set_title_bar(nsh_core::desktop::TitleBarStyle::Hidden, Some(px(theme::TITLE_BAR)));
    // Appearance follows the filer's pick, else the system's.
    effect(|| {
        set_color_scheme(match live::store().theme.get() {
            store::ThemePref::Light => Appearance::Light,
            store::ThemePref::Dark => Appearance::Dark,
            store::ThemePref::System => Appearance::System,
        });
    });
    // Esc closes the tax-profile panel (it has no close button).
    on_key("escape", |_| live::store().panel_open.set(false));
    // Android's back: a sheet closes first, then the page stack pops; at
    // Home the system takes it (the app goes to the background).
    on_back_press(|| {
        let st = live::store();
        if st.form.row_edit.get_untracked().is_some() {
            st.form.row_edit.set(None);
        } else if st.form.schedule_page.get_untracked().is_some() {
            st.form.schedule_page.set(None);
        } else if st.form.picker.get_untracked().is_some() {
            st.form.picker.set(None);
        } else if st.form.outline_open.get_untracked() {
            st.form.outline_open.set(false);
        } else if st.profile_open.get_untracked() {
            st.profile_open.set(false);
        } else if st.new_taxpayer_open.get_untracked() {
            st.new_taxpayer_open.set(false);
        } else if !st.history.with_untracked(Vec::is_empty) {
            st.back();
        } else {
            return false;
        }
        true
    });

    view().flex_1().h_full().bg(theme::BG).child(
        view().flex_1().child(each(
            move || vec![remount.get()],
            |&n| n,
            move |_, _| hot::hot_area(note),
        )),
    )
}
