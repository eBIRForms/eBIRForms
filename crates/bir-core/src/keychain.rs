//! macOS keychain access for generic passwords: the SQLCipher master key.
//!
//! Every operation goes to the Security framework in-process first and falls
//! back to the `/usr/bin/security` CLI that earlier releases used exclusively.
//! The native call is what works under the App Store sandbox (a sandboxed
//! child `security` process may fail or prompt). The CLI fallback is what
//! keeps existing installs working: their items were created by the
//! `security` tool, so the item ACL trusts that tool and the app's own native
//! read may be refused until the user approves it.
//!
//! The decision logic is written against [`KeychainStore`] so the lookup
//! order and the never-overwrite rules are unit-tested without a keychain.

use std::sync::mpsc;
use std::time::Duration;

/// `OSStatus` for "the specified item could not be found in the keychain".
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
/// `OSStatus` for "the specified item already exists in the keychain".
const ERR_SEC_DUPLICATE_ITEM: i32 = -25299;

/// How long a native keychain call may block before it is treated as
/// unreadable. Native calls run on a helper thread so the caller (often the
/// GPUI main thread) is never parked inside Security.framework, which earlier
/// releases observed deadlocking the main run loop. Long enough for a user to
/// answer a keychain access prompt.
const NATIVE_CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Result of reading one generic password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Lookup {
    /// The item exists and its value was read.
    Found(String),
    /// The keychain answered definitively that no such item exists.
    Missing,
    /// The item may exist but could not be read (access denied, prompt
    /// dismissed, keychain locked, timed out, ...). Never treat as absent.
    Unreadable(String),
    /// This store could not be consulted at all (e.g. the `security` binary
    /// could not be launched). Carries no information about the item.
    Unavailable(String),
}

/// Result of adding an item that must not already exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AddOutcome {
    Added,
    AlreadyExists,
    Failed(String),
}

/// One way of reaching the user's keychain.
pub(crate) trait KeychainStore {
    fn name(&self) -> &'static str;
    fn read(&self, service: &str, account: &str) -> Lookup;
    /// Add an item, failing with [`AddOutcome::AlreadyExists`] rather than
    /// replacing one that is already there.
    fn add_new(&self, service: &str, account: &str, value: &str) -> AddOutcome;
}

/// Read an item: native first, then the legacy CLI.
///
/// The CLI is consulted whenever the native read did not produce a value,
/// because an item created by the `security` tool may be readable only by
/// it. A [`Lookup::Missing`] result is returned only when nothing suggests the
/// item exists: the native store said "not found" and the CLI either agreed
/// or could not be launched at all (which tells us nothing new). Any other
/// combination is [`Lookup::Unreadable`].
pub(crate) fn read_with_fallback(
    native: &dyn KeychainStore,
    legacy: &dyn KeychainStore,
    service: &str,
    account: &str,
) -> Lookup {
    let native_result = native.read(service, account);
    if let Lookup::Found(value) = native_result {
        return Lookup::Found(value);
    }
    let legacy_result = legacy.read(service, account);
    match (native_result, legacy_result) {
        (_, Lookup::Found(value)) => Lookup::Found(value),
        (Lookup::Missing, Lookup::Missing | Lookup::Unavailable(_)) => Lookup::Missing,
        (Lookup::Missing, Lookup::Unreadable(error)) => {
            Lookup::Unreadable(format!("{}: {error}", legacy.name()))
        }
        (native_result, legacy_result) => Lookup::Unreadable(format!(
            "{}: {}; {}: {}",
            native.name(),
            describe(&native_result),
            legacy.name(),
            describe(&legacy_result)
        )),
    }
}

fn describe(lookup: &Lookup) -> String {
    match lookup {
        Lookup::Found(_) => "found".to_string(),
        Lookup::Missing => "not found".to_string(),
        Lookup::Unreadable(error) => error.clone(),
        Lookup::Unavailable(error) => format!("unavailable ({error})"),
    }
}

/// Whether [`resolve_master_key`] may create a key when none exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyMode {
    CreateIfMissing,
    ExistingOnly,
}

/// Find the SQLCipher master key, creating one only on a genuine first run.
///
/// Data-loss rules this enforces:
/// - an existing key is never replaced: a malformed or unreadable key is an
///   error, and creation uses add-only calls that fail if an item exists;
/// - a key is generated only when both stores agree it is absent
///   ([`read_with_fallback`] returned [`Lookup::Missing`]).
pub(crate) fn resolve_master_key(
    native: &dyn KeychainStore,
    legacy: &dyn KeychainStore,
    service: &str,
    account: &str,
    mode: KeyMode,
    generate: impl FnOnce() -> String,
) -> Result<String, String> {
    match read_with_fallback(native, legacy, service, account) {
        Lookup::Found(hex_key) if hex_key.len() == 64 => Ok(hex_key),
        // A key is present but malformed. Replacing it would destroy the
        // only means of decrypting an existing database, so stop.
        Lookup::Found(hex_key) => Err(format!(
            "The SQLCipher master key in the keychain is {} characters, expected 64. \
             Refusing to replace it - if it were regenerated the existing database \
             could never be decrypted again.",
            hex_key.len()
        )),
        Lookup::Unreadable(error) | Lookup::Unavailable(error) => Err(format!(
            "Could not read the existing SQLCipher master key ({error}). Refusing to \
             generate a replacement, because that would make the database permanently \
             unreadable. If a keychain prompt appeared, approve it and retry."
        )),
        Lookup::Missing if mode == KeyMode::ExistingOnly => {
            Err("Existing SQLCipher master key was not available: not found".to_string())
        }
        Lookup::Missing => {
            let hex_key = generate();
            match native.add_new(service, account, &hex_key) {
                AddOutcome::Added => Ok(hex_key),
                AddOutcome::AlreadyExists => Err(already_exists_error(native.name())),
                AddOutcome::Failed(native_error) => {
                    match legacy.add_new(service, account, &hex_key) {
                        AddOutcome::Added => Ok(hex_key),
                        AddOutcome::AlreadyExists => Err(already_exists_error(legacy.name())),
                        AddOutcome::Failed(legacy_error) => Err(format!(
                            "Failed to store master key in keychain: {}: {native_error}; {}: \
                         {legacy_error}",
                            native.name(),
                            legacy.name()
                        )),
                    }
                }
            }
        }
    }
}

fn already_exists_error(store: &str) -> String {
    format!(
        "A SQLCipher master key appeared in the keychain ({store}) while a new one was \
         being stored. Refusing to overwrite it; relaunch to use the existing key."
    )
}

// =========================================================================
// Native: Security.framework
// =========================================================================

/// The login keychain through Security.framework, in-process. Uses the same
/// generic-password items (service + account) as `security
/// find-generic-password`, so either store sees the other's items.
pub(crate) struct NativeKeychain;

/// Run `call` on a helper thread, giving up after [`NATIVE_CALL_TIMEOUT`].
fn bounded<T: Send + 'static>(call: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("keychain".to_string())
        .spawn(move || {
            let _ = sender.send(call());
        })
        .ok()?;
    receiver.recv_timeout(NATIVE_CALL_TIMEOUT).ok()
}

fn status_text(error: security_framework::base::Error) -> String {
    format!("{error} (OSStatus {})", error.code())
}

const TIMED_OUT: &str = "the Security framework did not answer in time";

impl KeychainStore for NativeKeychain {
    fn name(&self) -> &'static str {
        "Security framework"
    }

    fn read(&self, service: &str, account: &str) -> Lookup {
        let (service, account) = (service.to_owned(), account.to_owned());
        bounded(move || {
            use security_framework::os::macos::passwords::find_generic_password;
            match find_generic_password(None, &service, &account) {
                Ok((password, _item)) => match String::from_utf8(password.to_vec()) {
                    Ok(value) => Lookup::Found(value.trim().to_string()),
                    Err(_) => Lookup::Unreadable("stored value is not UTF-8".to_string()),
                },
                Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Lookup::Missing,
                Err(error) => Lookup::Unreadable(status_text(error)),
            }
        })
        .unwrap_or_else(|| Lookup::Unreadable(TIMED_OUT.to_string()))
    }

    fn add_new(&self, service: &str, account: &str, value: &str) -> AddOutcome {
        let (service, account, value) = (service.to_owned(), account.to_owned(), value.to_owned());
        bounded(move || {
            use security_framework::os::macos::keychain::SecKeychain;
            // `SecKeychainAddGenericPassword` never modifies an existing item;
            // it fails with errSecDuplicateItem instead.
            let result = SecKeychain::default().and_then(|keychain| {
                keychain.add_generic_password(&service, &account, value.as_bytes())
            });
            match result {
                Ok(()) => AddOutcome::Added,
                Err(error) if error.code() == ERR_SEC_DUPLICATE_ITEM => AddOutcome::AlreadyExists,
                Err(error) => AddOutcome::Failed(status_text(error)),
            }
        })
        .unwrap_or_else(|| AddOutcome::Failed(TIMED_OUT.to_string()))
    }
}

// =========================================================================
// Legacy: the `security` CLI
// =========================================================================

/// `/usr/bin/security`, the only store earlier releases used.
pub(crate) struct SecurityCli;

impl SecurityCli {
    fn run(args: &[&str]) -> std::io::Result<std::process::Output> {
        std::process::Command::new("/usr/bin/security")
            .args(args)
            .output()
    }

    fn stderr(output: &std::process::Output) -> String {
        String::from_utf8_lossy(&output.stderr).trim().to_string()
    }
}

impl KeychainStore for SecurityCli {
    fn name(&self) -> &'static str {
        "security CLI"
    }

    fn read(&self, service: &str, account: &str) -> Lookup {
        let output = match Self::run(&["find-generic-password", "-s", service, "-a", account, "-w"])
        {
            Ok(output) => output,
            Err(error) => return Lookup::Unavailable(format!("could not run: {error}")),
        };
        if output.status.success() {
            return Lookup::Found(String::from_utf8_lossy(&output.stdout).trim().to_string());
        }
        // Only exit 44 means the item is genuinely absent; see
        // `keychain_miss_is_genuine`.
        if crate::db::keychain_miss_is_genuine(output.status.code()) {
            return Lookup::Missing;
        }
        Lookup::Unreadable(format!(
            "exit {:?}: {}",
            output.status.code(),
            Self::stderr(&output)
        ))
    }

    fn add_new(&self, service: &str, account: &str, value: &str) -> AddOutcome {
        // Deliberately NOT -U ("update if exists"): without it the add fails
        // rather than overwrites.
        match Self::run(&[
            "add-generic-password",
            "-s",
            service,
            "-a",
            account,
            "-w",
            value,
        ]) {
            Ok(output) => {
                let stderr = Self::stderr(&output);
                // `security add-generic-password` reports an existing item on
                // stderr but may still exit 0, so the status alone cannot be
                // trusted here.
                if stderr.contains("already exists") {
                    AddOutcome::AlreadyExists
                } else if output.status.success() {
                    AddOutcome::Added
                } else {
                    AddOutcome::Failed(stderr)
                }
            }
            Err(error) => AddOutcome::Failed(format!("could not run: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const SERVICE: &str = "svc";
    const ACCOUNT: &str = "acct";
    const VALID: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const OTHER: &str = "2222222222222222222222222222222222222222222222222222222222222222";

    /// In-memory stand-in for one keychain store. Records every call.
    struct Fake {
        name: &'static str,
        read: Lookup,
        add: AddOutcome,
        calls: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new(name: &'static str, read: Lookup) -> Self {
            Self {
                name,
                read,
                add: AddOutcome::Added,
                calls: RefCell::new(Vec::new()),
            }
        }
        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
        fn wrote(&self) -> bool {
            self.calls
                .borrow()
                .iter()
                .any(|call| call.starts_with("add"))
        }
    }

    impl KeychainStore for Fake {
        fn name(&self) -> &'static str {
            self.name
        }
        fn read(&self, _: &str, _: &str) -> Lookup {
            self.calls.borrow_mut().push("read".into());
            self.read.clone()
        }
        fn add_new(&self, _: &str, _: &str, value: &str) -> AddOutcome {
            self.calls.borrow_mut().push(format!("add {value}"));
            self.add.clone()
        }
    }

    fn unreadable() -> Lookup {
        Lookup::Unreadable("denied".into())
    }

    fn resolve(native: &Fake, legacy: &Fake, mode: KeyMode) -> Result<String, String> {
        resolve_master_key(native, legacy, SERVICE, ACCOUNT, mode, || OTHER.to_string())
    }

    // ---- lookup order ----

    #[test]
    fn native_hit_never_consults_the_cli() {
        let native = Fake::new("native", Lookup::Found(VALID.into()));
        let legacy = Fake::new("cli", Lookup::Found(OTHER.into()));
        assert_eq!(
            read_with_fallback(&native, &legacy, SERVICE, ACCOUNT),
            Lookup::Found(VALID.into())
        );
        assert!(legacy.calls().is_empty());
    }

    #[test]
    fn cli_created_item_unreadable_natively_is_found_through_the_cli() {
        let native = Fake::new("native", unreadable());
        let legacy = Fake::new("cli", Lookup::Found(VALID.into()));
        assert_eq!(
            read_with_fallback(&native, &legacy, SERVICE, ACCOUNT),
            Lookup::Found(VALID.into())
        );
    }

    #[test]
    fn native_miss_still_asks_the_cli() {
        let native = Fake::new("native", Lookup::Missing);
        let legacy = Fake::new("cli", Lookup::Found(VALID.into()));
        assert_eq!(
            read_with_fallback(&native, &legacy, SERVICE, ACCOUNT),
            Lookup::Found(VALID.into())
        );
    }

    #[test]
    fn missing_only_when_nothing_suggests_the_item_exists() {
        for legacy_read in [Lookup::Missing, Lookup::Unavailable("no binary".into())] {
            let native = Fake::new("native", Lookup::Missing);
            let legacy = Fake::new("cli", legacy_read);
            assert_eq!(
                read_with_fallback(&native, &legacy, SERVICE, ACCOUNT),
                Lookup::Missing
            );
        }
        // Any unreadable answer, from either side, is not a miss.
        let cases = [
            (Lookup::Missing, unreadable()),
            (unreadable(), Lookup::Missing),
            (unreadable(), unreadable()),
            (unreadable(), Lookup::Unavailable("no binary".into())),
        ];
        for (native_read, legacy_read) in cases {
            let native = Fake::new("native", native_read);
            let legacy = Fake::new("cli", legacy_read);
            assert!(matches!(
                read_with_fallback(&native, &legacy, SERVICE, ACCOUNT),
                Lookup::Unreadable(_)
            ));
        }
    }

    // ---- master key: never overwrite, never generate over an unreadable key ----

    #[test]
    fn existing_key_is_returned_and_nothing_is_written() {
        let native = Fake::new("native", Lookup::Found(VALID.into()));
        let legacy = Fake::new("cli", Lookup::Missing);
        assert_eq!(
            resolve(&native, &legacy, KeyMode::CreateIfMissing),
            Ok(VALID.into())
        );
        assert!(!native.wrote() && !legacy.wrote());
    }

    #[test]
    fn legacy_key_is_returned_and_not_migrated_or_rewritten() {
        let native = Fake::new("native", unreadable());
        let legacy = Fake::new("cli", Lookup::Found(VALID.into()));
        assert_eq!(
            resolve(&native, &legacy, KeyMode::CreateIfMissing),
            Ok(VALID.into())
        );
        assert!(!native.wrote() && !legacy.wrote());
    }

    #[test]
    fn unreadable_key_is_an_error_and_no_key_is_generated() {
        for (native_read, legacy_read) in [
            (unreadable(), Lookup::Missing),
            (unreadable(), unreadable()),
            (Lookup::Missing, unreadable()),
            (unreadable(), Lookup::Unavailable("no binary".into())),
        ] {
            let native = Fake::new("native", native_read);
            let legacy = Fake::new("cli", legacy_read);
            let mut generated = false;
            let result = resolve_master_key(
                &native,
                &legacy,
                SERVICE,
                ACCOUNT,
                KeyMode::CreateIfMissing,
                || {
                    generated = true;
                    OTHER.to_string()
                },
            );
            assert!(result.is_err());
            assert!(!generated, "must not generate over an unreadable key");
            assert!(!native.wrote() && !legacy.wrote());
        }
    }

    #[test]
    fn malformed_key_is_an_error_and_is_not_replaced() {
        let native = Fake::new("native", Lookup::Found("abc".into()));
        let legacy = Fake::new("cli", Lookup::Missing);
        assert!(resolve(&native, &legacy, KeyMode::CreateIfMissing).is_err());
        assert!(!native.wrote() && !legacy.wrote());
    }

    #[test]
    fn genuine_first_run_adds_natively_without_overwrite() {
        let native = Fake::new("native", Lookup::Missing);
        let legacy = Fake::new("cli", Lookup::Missing);
        assert_eq!(
            resolve(&native, &legacy, KeyMode::CreateIfMissing),
            Ok(OTHER.into())
        );
        assert_eq!(
            native.calls(),
            vec!["read".to_string(), format!("add {OTHER}")]
        );
        assert!(!legacy.wrote());
    }

    #[test]
    fn native_add_failure_falls_back_to_cli_add() {
        let mut native = Fake::new("native", Lookup::Missing);
        native.add = AddOutcome::Failed("sandbox".into());
        let legacy = Fake::new("cli", Lookup::Missing);
        assert_eq!(
            resolve(&native, &legacy, KeyMode::CreateIfMissing),
            Ok(OTHER.into())
        );
        assert!(legacy.calls().contains(&format!("add {OTHER}")));
        assert!(!native.calls().iter().any(|c| c.starts_with("upsert")));
        assert!(!legacy.calls().iter().any(|c| c.starts_with("upsert")));
    }

    #[test]
    fn a_key_that_appears_during_creation_is_never_overwritten() {
        let mut native = Fake::new("native", Lookup::Missing);
        native.add = AddOutcome::AlreadyExists;
        let legacy = Fake::new("cli", Lookup::Missing);
        assert!(resolve(&native, &legacy, KeyMode::CreateIfMissing).is_err());
        assert!(!legacy.wrote(), "must not retry the write elsewhere");

        let mut native = Fake::new("native", Lookup::Missing);
        native.add = AddOutcome::Failed("x".into());
        let mut legacy = Fake::new("cli", Lookup::Missing);
        legacy.add = AddOutcome::AlreadyExists;
        assert!(resolve(&native, &legacy, KeyMode::CreateIfMissing).is_err());
    }

    #[test]
    fn existing_only_mode_never_creates() {
        let native = Fake::new("native", Lookup::Missing);
        let legacy = Fake::new("cli", Lookup::Missing);
        assert!(resolve(&native, &legacy, KeyMode::ExistingOnly).is_err());
        assert!(!native.wrote() && !legacy.wrote());

        let native = Fake::new("native", unreadable());
        let legacy = Fake::new("cli", Lookup::Found(VALID.into()));
        assert_eq!(
            resolve(&native, &legacy, KeyMode::ExistingOnly),
            Ok(VALID.into())
        );
    }
}
