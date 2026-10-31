//! The app's ids: the bundle / application id and the keychain services
//! named after it.
//!
//! v0.2.0 renamed the app id from `com.buwiz.oss` to `com.buwiz.ebirforms`.
//! On macOS and iOS the keychain services follow the new id (macOS moves
//! v0.1.0's data over: [`crate::id_migration`]). Windows and Linux keep the
//! old services: their data folders did not move, so the database there is
//! still encrypted with the key stored under `com.buwiz.oss`, and a new
//! service would hide that key.

/// The bundle / application id since v0.2.0.
pub const APP_ID: &str = "com.buwiz.ebirforms";

/// v0.1.0's id.
pub const OLD_APP_ID: &str = "com.buwiz.oss";

const NEW_IDS: bool = cfg!(any(target_os = "macos", target_os = "ios"));

/// Keychain service of the SQLCipher master key.
pub const MASTER_KEY_SERVICE: &str = if NEW_IDS { APP_ID } else { OLD_APP_ID };

/// v0.1.0's service of the master key.
pub const OLD_MASTER_KEY_SERVICE: &str = OLD_APP_ID;

/// Keychain account of the SQLCipher master key.
pub const MASTER_KEY_ACCOUNT: &str = "sqlcipher_master_key";

/// Keychain service of bir-submit's secrets (`bir_submit::secrets::SERVICE`).
pub const SUBMIT_SERVICE: &str = if NEW_IDS {
    "com.buwiz.ebirforms.submit"
} else {
    "com.buwiz.oss.submit"
};

/// v0.1.0's service of bir-submit's secrets.
pub const OLD_SUBMIT_SERVICE: &str = "com.buwiz.oss.submit";

/// The names bir-submit stores under [`SUBMIT_SERVICE`]: the sign-in session
/// and the device key. bir-submit's tests check this list against its own.
pub const SUBMIT_SECRET_NAMES: [&str; 2] = ["session", "device_key"];
