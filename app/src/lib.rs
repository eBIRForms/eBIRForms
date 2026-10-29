//! Buwiz: an offline BIR filer on nsh, edited live by ra-jit.
//!
//! The root is a thin shell ([`App`]): a `dynamic` mounts
//! [`hot::hot_area`], which the daemon's patches cause to rebuild.
//! [`live::install`] is the only glue between ra-jit and the engine;
//! [`live::init`] anchors the app's store under the root owner so the
//! whole model survives remounts.

mod hot;
pub mod live;
mod screens;
mod store;
mod theme;

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

    view().flex_1().h_full().bg(theme::MAIN).child(
        view().flex_1().child(each(
            move || vec![remount.get()],
            |&n| n,
            move |_, _| hot::hot_area(note),
        )),
    )
}
