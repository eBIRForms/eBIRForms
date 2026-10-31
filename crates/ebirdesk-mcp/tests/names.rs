//! This plugin's names clash with none taken by the standalone eBIRForms
//! mini app, the desktop app or the old BIR mini app, and the manifest
//! passes the SDK's quick check.

use buwiz_plugin::{Plugin, Tool};
use ebirdesk_mcp::{Ebirdesk, manifest};

/// Names already taken, as an id, a short, a tool prefix, a binary name, an
/// environment prefix or a manifest file.
const TAKEN_IDS: [&str; 5] = [
    "app.buwiz.ebirforms",
    "com.buwiz.ebirforms",
    "com.buwiz.oss",
    "dev.goldcoders.bir",
    "group.dev.goldcoders.bir",
];
const TAKEN_PREFIXES: [&str; 4] = ["ebirforms_", "bir_", "bir_app_", "rox."];
const TAKEN_SHORTS: [&str; 3] = ["ebirforms", "bir", "rox"];
const TAKEN_BINARIES: [&str; 3] = ["ebirforms-mini-app", "buwiz", "buwiz-mcp"];
const TAKEN_ENV_PREFIXES: [&str; 3] = ["EBIRFORMS_", "BIR_", "EBIR_"];
const TAKEN_MANIFEST_FILES: [&str; 2] = [
    "app.buwiz.ebirforms.manifest.json",
    "dev.goldcoders.bir.manifest.json",
];

/// Whether `a` and `b` overlap as prefixes (one starts with the other).
fn overlaps(a: &str, b: &str) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

fn tools() -> Vec<Tool> {
    // what `Plugin::tools` lists; it needs no database
    ebirdesk_mcp::tool_list()
}

#[test]
fn the_plugin_takes_none_of_the_taken_names() {
    let m = Ebirdesk::manifest(Ebirdesk::RELEASE_KEY_ID);
    let id = m["id"].as_str().unwrap();
    let short = m["short"].as_str().unwrap();
    assert_eq!(id, "app.buwiz.ebirdesk");
    assert_eq!(id, Ebirdesk::ID);
    assert_eq!(short, "ebirdesk");
    assert_eq!(Ebirdesk::NAME, "ebirdesk-mcp");
    // the binary cargo built for these tests
    let binary = std::path::Path::new(env!("CARGO_BIN_EXE_ebirdesk-mcp"));
    assert_eq!(
        binary.file_stem().and_then(|stem| stem.to_str()),
        Some("ebirdesk-mcp")
    );
    assert_eq!(manifest::MANIFEST_ENV, "BUWIZ_EBIRDESK_MANIFEST");
    assert_eq!(manifest::file_name(), "app.buwiz.ebirdesk.manifest.json");

    for taken in TAKEN_IDS {
        assert!(!overlaps(id, taken), "the id {id} overlaps {taken}");
    }
    for taken in TAKEN_SHORTS {
        assert_ne!(short, taken, "the short {short} is taken");
        assert!(
            !short.starts_with(&format!("{taken}_")),
            "the short {short} overlaps {taken}"
        );
    }
    for taken in TAKEN_BINARIES {
        assert_ne!(Ebirdesk::NAME, taken, "the binary name is taken");
    }
    for taken in TAKEN_ENV_PREFIXES {
        assert!(
            !manifest::MANIFEST_ENV.starts_with(taken),
            "{} overlaps {taken}",
            manifest::MANIFEST_ENV
        );
    }
    for taken in TAKEN_MANIFEST_FILES {
        assert_ne!(manifest::file_name(), taken);
    }

    let tools = tools();
    assert!(!tools.is_empty());
    let prefix = format!("{short}_");
    for tool in &tools {
        assert!(
            tool.name.starts_with(&prefix),
            "{} lacks the prefix {prefix}",
            tool.name
        );
        for taken in TAKEN_PREFIXES {
            assert!(
                !tool.name.starts_with(taken),
                "{} has the taken prefix {taken}",
                tool.name
            );
        }
    }
    // every tool an app may call is one of ours, with our prefix
    for app in m["apps"].as_array().unwrap() {
        for tool in app["permissions"]["tools"].as_array().unwrap() {
            let tool = tool.as_str().unwrap();
            assert!(tool.starts_with(&prefix), "{tool}");
            assert!(
                TAKEN_PREFIXES.iter().all(|taken| !tool.starts_with(taken)),
                "{tool}"
            );
        }
    }
    // the scope hint names our tools and token, not the old ones
    let hint = m["scopes"][0]["hint"].as_str().unwrap();
    assert!(
        hint.contains("@ebirdesk:taxprofile:")
            && !hint.contains("@bir:")
            && !hint.contains(" bir_"),
        "{hint}"
    );
}

#[test]
fn the_manifest_has_no_problems() {
    assert_eq!(manifest::problems(&tools()), Vec::<String>::new());
    assert_eq!(
        buwiz_plugin::manifest::problems(&Ebirdesk::manifest("ebirdesk-test-01"), &tools()),
        Vec::<String>::new()
    );
}

#[test]
fn no_old_protocol_names_are_served() {
    let m = Ebirdesk::manifest(Ebirdesk::RELEASE_KEY_ID);
    let text = m.to_string();
    assert!(!text.contains("rox"), "the manifest names rox: {text}");
    assert_eq!(m["protocol"], buwiz_plugin::PROTOCOL);
    let source = Ebirdesk::app_source("forms").unwrap();
    assert!(
        !source.to_lowercase().contains("rox"),
        "forms.tsx still speaks protocol 1.0"
    );
    for tool in tools() {
        assert!(
            !tool.description.contains("rox") && !tool.description.contains(" bir_"),
            "{}: {}",
            tool.name,
            tool.description
        );
    }
}
