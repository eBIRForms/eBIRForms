//! `ebirdesk-mcp`: the desktop app without a window, as the eBIRForms
//! Desktop mini app the Buwiz Bot spawns (plugin `app.buwiz.ebirdesk`, Mini
//! App Protocol 2.0, stdio only).
//!
//! The SDK (`buwiz-plugin`) serves MCP, answers the reserved `buwiz.*`
//! tools (provision, session certificates, ticket titles, uninstall) and
//! holds every caller to the served manifest. This crate holds what is the
//! desktop app's:
//!
//! - [`manifest`]: the manifest, built in code, and the signed file served;
//! - `tools`: the `ebirdesk_*` tools over [`BirApp`];
//! - `scope`, `scrub`, `tickets`: tickets in place of TINs, and the Forms
//!   app held to the taxpayer of its ticket;
//! - `sending`: a queued return sent through bir-submit's relay, only when
//!   no other app runs the queue.
//!
//! It opens the desktop app's own database (same data dir, same keychain
//! key) through [`BirApp::open_live`], shared with the desktop app while
//! both run (`bir_core::db::shared`). stdout carries MCP; every log line
//! goes to stderr.

pub mod manifest;
mod scope;
mod scrub;
mod sending;
mod tickets;
mod tools;

use std::sync::Arc;

use bir_app_core::bir_core::forms::taxform;
use bir_app_core::{BirApp, OpenedReturn};
use buwiz_plugin::{Args, Caller, Plugin, Tool};
use serde_json::Value;

pub use tools::tool_list;

/// The MCP server name and the binary's.
pub const NAME: &str = "ebirdesk-mcp";

/// The return the assistant has open.
pub(crate) struct OpenForm {
    pub(crate) tin: String,
    pub(crate) form: String,
    pub(crate) opened: OpenedReturn,
    /// Edits since the last save.
    pub(crate) dirty: bool,
}

/// The plugin: the desktop app's database, the return the assistant has
/// open, and the sends this process started.
pub struct Ebirdesk {
    pub(crate) app: Result<Arc<BirApp>, String>,
    pub(crate) form: Option<OpenForm>,
    pub(crate) sends: sending::Sends,
}

impl Ebirdesk {
    /// The plugin the Bot spawns: the live database the desktop app uses. A
    /// database that cannot open (no desktop app on this computer yet, a
    /// key that cannot be read) leaves the protocol working and every tool
    /// saying why.
    pub fn open_live() -> Self {
        let path = bir_app_core::bir_core::db::app_database_path();
        let app = if path.is_file() {
            BirApp::open_live()
                .map(Arc::new)
                .map_err(|error| format!("the eBIRForms database did not open: {error}"))
        } else {
            Err("eBIRForms has no taxpayer database on this computer yet: open the eBIRForms desktop app once first.".to_string())
        };
        if let Err(problem) = &app {
            tracing::warn!("{problem}");
        }
        Self {
            app,
            form: None,
            sends: sending::Sends::default(),
        }
    }

    /// The plugin over `app` (tests).
    pub fn with_app(app: Arc<BirApp>) -> Self {
        Self {
            app: Ok(app),
            form: None,
            sends: sending::Sends::default(),
        }
    }

    /// Wait for the sends this process started (call before exiting).
    pub fn finish(&mut self) {
        self.sends.join();
    }
}

impl Plugin for Ebirdesk {
    const ID: &'static str = manifest::ID;
    const NAME: &'static str = NAME;
    const VERSION: &'static str = manifest::VERSION;
    const RELEASE_KEY_ID: &'static str = manifest::RELEASE_KEY_ID;
    const INSTRUCTIONS: &'static str = tools::INSTRUCTIONS;

    fn manifest(key_id: &str) -> Value {
        manifest::unsigned(key_id)
    }

    fn app_source(id: &str) -> Option<&'static str> {
        manifest::app_source(id)
    }

    fn tools(&self) -> Vec<Tool> {
        tools::tool_list()
    }

    fn call(&mut self, tool: &str, args: &Args, caller: &Caller) -> Value {
        self.run(tool, args, caller)
    }

    fn ticket_title(&mut self, ticket: &str) -> Option<String> {
        let app = self.app.as_deref().ok()?;
        tickets::subject(app, ticket)
            .ok()
            .flatten()?
            .title(app, taxform::today())
    }

    fn uninstall(&mut self) {
        if let Ok(app) = &self.app
            && let Err(error) = tickets::forget_all(app)
        {
            tracing::warn!("the tickets were not forgotten: {error}");
        }
    }
}
