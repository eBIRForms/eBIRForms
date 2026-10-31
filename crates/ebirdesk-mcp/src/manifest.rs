//! The manifest (Mini App Protocol 2.0): identity, the `taxprofile` scope
//! and the one app this plugin ships, Forms, with what it may reach.
//!
//! [`unsigned`] builds it from this build's code and the app source it
//! serves, so the fingerprint is the SHA-256 of the exact bytes
//! `resources/read` answers and manifest and app cannot drift.
//! `ebirdesk-mcp manifest` prints it as JCS text for the owner to sign with
//! publisher key `gc-2026-01` (outside this repo; see the crate README).
//! The signed file is what this process serves, byte for byte:
//! [`MANIFEST_ENV`], else [`file_name`] next to the executable. Without one
//! the unsigned manifest is served and the Bot refuses the plugin, which is
//! the point: only a signed build runs in the Bot.
//!
//! The names are this plugin's own: `app.buwiz.ebirdesk`, short `ebirdesk`,
//! so it clashes with neither the standalone eBIRForms mini app
//! (`app.buwiz.ebirforms`) nor the old one (`dev.goldcoders.bir`).

use std::path::PathBuf;

use buwiz_plugin::manifest::{APP_MIME, APP_SDK, APP_URI_PREFIX, PROTOCOL, Served, fingerprint};
use buwiz_plugin::{Tool, jcs};
use serde_json::{Value, json};

use crate::scope;
use crate::tools::{PAYMENTS_LIST, PRINT_PREVIEW, RESOLVE, TICKET_ISSUE, TICKET_SUBJECT};

/// The plugin id.
pub const ID: &str = "app.buwiz.ebirdesk";
/// Every tool starts with `ebirdesk_`.
pub const SHORT: &str = "ebirdesk";
/// The manifest's version: the crate's, so a release raises one number.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The publisher key a release is signed with. Only the owner signs.
pub const RELEASE_KEY_ID: &str = "gc-2026-01";
/// The name the user sees.
pub const TITLE: &str = "eBIRForms Desktop";
/// The signed manifest this process serves.
pub const MANIFEST_ENV: &str = "BUWIZ_EBIRDESK_MANIFEST";

/// The Forms app: `buwiz-app://forms`, the TSX module the Bot renders.
pub const FORMS_ID: &str = "forms";
pub(crate) const FORMS_TITLE: &str = "BIR Forms";
pub(crate) const FORMS_DESCRIPTION: &str = "Prepare, check and queue a taxpayer's BIR returns: pick a form and period, fill it, validate it with the official rules, save the draft, and queue it for filing.";
pub(crate) const FORMS_SOURCE: &str = include_str!("../mini-apps/forms.tsx");

/// What the agent is told when a taxpayer token is in a message or attached.
const TAXPROFILE_HINT: &str = "An `@ebirdesk:taxprofile:<ticket>` token names one taxpayer; the \
ticket stands for its TIN, which you never see. Attached to this chat, eBIRForms Desktop tools \
default to that taxpayer: pass the ticket as `tin` to ebirdesk_drafts_list, ebirdesk_dues_list, \
ebirdesk_form_open and ebirdesk_app_open (forms), unless the user names another taxpayer. \
ebirdesk_profiles_list lists the taxpayers (as tickets).";

/// The file name the signed manifest ships under, next to the executable.
pub fn file_name() -> String {
    buwiz_plugin::manifest::file_name(ID)
}

/// The tools the Forms app calls: the ticket helpers, the print preview,
/// the (empty) payments list and one `ebirdesk_app_<function>` per function.
pub(crate) fn forms_tools() -> Vec<&'static str> {
    let mut tools = vec![TICKET_SUBJECT, TICKET_ISSUE, PRINT_PREVIEW, PAYMENTS_LIST];
    tools.extend(scope::FORMS.iter().map(|(name, ..)| *name));
    tools
}

/// The Forms app's manifest entry.
fn forms_entry() -> Value {
    json!({
        "id": FORMS_ID,
        "title": FORMS_TITLE,
        "description": FORMS_DESCRIPTION,
        "entry": format!("{APP_URI_PREFIX}{FORMS_ID}"),
        "mime": APP_MIME,
        "sdk": APP_SDK,
        "fingerprint": fingerprint(FORMS_SOURCE),
        "props": {
            "type": "object",
            "properties": {
                "ticket": { "type": "string", "maxLength": 30 },
                "form": { "type": "string", "maxLength": 64 },
                "period": { "maxLength": 32 },
                "action": { "type": "string", "enum": ["view", "file", "submit"] },
                "draft_id": { "type": "integer" },
                "form_code": { "type": "string", "maxLength": 16 },
                "year": { "type": "integer", "minimum": 1900, "maximum": 2200 },
                "key": { "type": "integer" }
            }
        },
        "placement": ["full"],
        "permissions": {
            "tools": forms_tools(),
            "attach": ["taxprofile"],
            // the print preview in the Bot's document view
            "preview": {},
        },
    })
}

/// The manifest without its signature, under publisher key id `key_id`.
pub fn unsigned(key_id: &str) -> Value {
    json!({
        "protocol": PROTOCOL,
        "id": ID,
        "short": SHORT,
        "version": VERSION,
        "title": TITLE,
        "description": "Philippine BIR returns for the taxpayers kept in the eBIRForms desktop app on this computer.",
        "publisher": { "id": "app.buwiz", "key_id": key_id, "name": "Buwiz" },
        "commands": [],
        "scopes": [
            { "name": "taxprofile", "description": "Taxpayer by TIN or name", "resolve": RESOLVE, "attach": true, "hint": TAXPROFILE_HINT },
        ],
        "routines": [],
        "apps": [forms_entry()],
    })
}

/// A TSX app's source exactly as served.
pub fn app_source(id: &str) -> Option<&'static str> {
    (id == FORMS_ID).then_some(FORMS_SOURCE)
}

/// The signed manifest to serve: [`MANIFEST_ENV`], else [`file_name`] next
/// to the executable when it exists, else none (the unsigned one is served).
pub fn signed_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(MANIFEST_ENV).filter(|path| !path.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let exe = std::env::current_exe().ok()?;
    let path = exe.parent()?.join(file_name());
    path.is_file().then_some(path)
}

/// What this process serves: the signed file, else the unsigned manifest
/// under the release key.
pub fn served() -> Served {
    Served::new(signed_path(), &unsigned(RELEASE_KEY_ID))
}

/// Why the served manifest is not this build's, if it is not: a changed
/// app, tool or text needs a new signature.
pub fn staleness(served: &Served) -> Option<String> {
    served.signed.as_ref()?;
    let mut body = match served.value() {
        Ok(value) => value,
        Err(error) => return Some(error),
    };
    let Some(map) = body.as_object_mut() else {
        return Some("the signed manifest is not a JSON object".into());
    };
    if map.remove("signature").is_none() {
        return Some("the signed manifest has no signature".into());
    }
    (jcs::canonical(&body).ok() != Some(served.unsigned.clone())).then(|| {
        "the signed manifest is not this build's (its app, tools or text changed): print it with `ebirdesk-mcp manifest` and have it signed again".to_string()
    })
}

/// What the manifest gets wrong against `tools` (the SDK's quick check).
pub fn problems(tools: &[Tool]) -> Vec<String> {
    buwiz_plugin::manifest::problems(&unsigned(RELEASE_KEY_ID), tools)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_names_only_forms_and_its_own_tools() {
        let m = unsigned(RELEASE_KEY_ID);
        assert_eq!(m["id"], ID);
        assert_eq!(m["protocol"], PROTOCOL);
        assert_eq!(m["publisher"]["key_id"], "gc-2026-01");
        assert_eq!(
            unsigned("ebirdesk-test-01")["publisher"]["key_id"],
            "ebirdesk-test-01"
        );
        let apps = m["apps"].as_array().unwrap();
        assert_eq!(apps.len(), 1);
        let forms = &apps[0];
        assert_eq!(forms["entry"], "buwiz-app://forms");
        assert_eq!(forms["fingerprint"], fingerprint(FORMS_SOURCE));
        let tools = forms["permissions"]["tools"].as_array().unwrap();
        assert!(
            !tools.is_empty() && tools.len() <= 64,
            "{} tools",
            tools.len()
        );
        // integers only, so it can be signed, and under 256 KiB
        assert!(jcs::canonical(&m).unwrap().len() < 256 * 1024);
        assert!(m["scopes"][0]["hint"].as_str().unwrap().len() <= 1500);
        assert!(staleness(&Served::new(None, &m)).is_none());
    }

    #[test]
    fn a_signed_file_that_is_not_this_build_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(file_name());
        let mut signed = unsigned(RELEASE_KEY_ID);
        signed["signature"] = json!({"alg": "ed25519", "key_id": RELEASE_KEY_ID, "value": "x"});
        std::fs::write(&path, signed.to_string()).unwrap();
        let served = Served::new(Some(path.clone()), &unsigned(RELEASE_KEY_ID));
        assert!(staleness(&served).is_none());
        assert_eq!(served.sha256().unwrap(), jcs::sha256_jcs(&signed).unwrap());
        signed["version"] = json!("0.0.1");
        std::fs::write(&path, signed.to_string()).unwrap();
        assert!(staleness(&served).is_some());
        let mut unsigned_file = unsigned(RELEASE_KEY_ID);
        unsigned_file.as_object_mut().unwrap().remove("signature");
        std::fs::write(&path, unsigned_file.to_string()).unwrap();
        assert!(staleness(&served).unwrap().contains("no signature"));
    }
}
