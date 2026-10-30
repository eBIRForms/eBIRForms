//! The one door the app calls: typed functions over forms, taxpayers,
//! deadlines, penalties, print and the submission queue.
//!
//! [`BirApp`] is the typed facade. Every function ported from the old
//! shell's JSON host modules stays reachable by name through
//! [`BirApp::call`] until it gets a typed method.

mod app;
mod bir_module;
mod dashboard_module;
mod database;
mod filer;
mod sending;
pub mod display;
pub mod host;
mod schema;
mod screen_form_module;
mod submit;
mod tax_profile_module;

pub use app::*;
pub use filer::*;
pub use sending::*;
pub use schema::*;
pub use submit::{SERVER_URL_SETTING, block_on, set_sending_paused};
pub use {bir_core, bir_print, bir_submit};
