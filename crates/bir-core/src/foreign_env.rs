//! Environment variables that belong to other apps on the same machine.
//!
//! eBIRForms (its gpui shell and bir-desktop) reads `BIR_DATABASE_PATH`,
//! `BIR_DB_KEY`, `DEVELOPER_MODE` and its own `BUWIZ_SERVER_URL` family.
//! Buwiz reads only its own `BUWIZ_*` names, so a variable set for the other
//! app never points Buwiz at its database, key or server. A foreign name that
//! is set is logged once, so a developer sees why it had no effect.
//! `scripts/check-secrets.sh` keeps these names out of the rest of the tree.

use std::sync::Once;

/// The names other apps read that Buwiz ignores.
pub const FOREIGN: &[&str] = &[
    "BIR_DATABASE_PATH",
    "BIR_DB_KEY",
    "BIR_DB_KEY_FILE",
    "BIR_NO_KEYCHAIN",
    "EBIR_TEST_ENV",
    "EBIR_RELAXED_VALIDATION",
    "DEVELOPER_MODE",
    "BUWIZ_SERVER_URL",
    "BUWIZ_ACCOUNT_URL",
    "BUWIZ_ACCESS_TOKEN",
];

/// The foreign names `var` reports as set (and not empty).
pub fn set_names(var: impl Fn(&str) -> Option<String>) -> Vec<&'static str> {
    FOREIGN.iter().copied().filter(|name| var(name).is_some_and(|value| !value.is_empty())).collect()
}

/// Logs, once per process, the foreign names set in this environment.
pub fn warn_if_set() {
    static WARNED: Once = Once::new();
    WARNED.call_once(|| {
        let set = set_names(|name| std::env::var(name).ok());
        if !set.is_empty() {
            tracing::warn!(
                "ignoring {}: another app's settings; Buwiz reads only BUWIZ_* names (BUWIZ_DATABASE_PATH, BUWIZ_DB_KEY, BUWIZ_DB_KEY_FILE, BUWIZ_API_URL)",
                set.join(", ")
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_other_apps_names_are_reported_when_set() {
        let env = |name: &str| match name {
            "BIR_DATABASE_PATH" => Some("/elsewhere/bir_data.db".to_owned()),
            "BIR_DB_KEY" => Some(String::new()),
            "BUWIZ_SERVER_URL" => Some("http://127.0.0.1:9".to_owned()),
            "BUWIZ_DATABASE_PATH" => Some("/mine.db".to_owned()),
            _ => None,
        };
        assert_eq!(set_names(env), ["BIR_DATABASE_PATH", "BUWIZ_SERVER_URL"], "an empty value counts as unset; Buwiz's own names are not foreign");
    }
}
