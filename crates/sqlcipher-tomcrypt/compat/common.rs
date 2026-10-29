//! What both sides of the check do on an open, keyed connection.

use rusqlite::{Connection, params};

/// A marker row per writer: its label and a payload spanning many pages.
pub fn write_marker(conn: &Connection, label: &str) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS compat_marker (label TEXT PRIMARY KEY, note TEXT, payload BLOB)",
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO compat_marker (label, note, payload) VALUES (?1, ?2, ?3)",
        params![label, note(label), payload(label)],
    )?;
    Ok(())
}

/// Checks every expected writer's row, the page HMACs and SQLite's
/// integrity check. Prints the cipher settings, one `key=value` per line.
pub fn check(conn: &Connection, labels: &[String]) -> Result<(), String> {
    let err = |e: rusqlite::Error| e.to_string();
    for label in labels {
        let (n, p): (String, Vec<u8>) = conn
            .query_row(
                "SELECT note, payload FROM compat_marker WHERE label = ?1",
                [label],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| format!("marker {label}: {e}"))?;
        if n != note(label) || p != payload(label) {
            return Err(format!("marker {label}: content differs"));
        }
    }
    let mut stmt = conn.prepare("PRAGMA cipher_integrity_check").map_err(err)?;
    let problems: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .map_err(err)?
        .collect::<Result<_, _>>()
        .map_err(err)?;
    if !problems.is_empty() {
        return Err(format!("cipher_integrity_check: {problems:?}"));
    }
    let ok: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(err)?;
    if ok != "ok" {
        return Err(format!("integrity_check: {ok}"));
    }
    let user_version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(err)?;
    println!("user_version={user_version}");
    Ok(())
}

/// The settings that decide the file format, which must match.
pub fn print_settings(conn: &Connection) -> rusqlite::Result<()> {
    for pragma in [
        "cipher_provider",
        "cipher_version",
        "cipher_page_size",
        "kdf_iter",
        "cipher_hmac_algorithm",
        "cipher_kdf_algorithm",
        "cipher_use_hmac",
        "cipher_plaintext_header_size",
        "journal_mode",
    ] {
        let value: String = conn.query_row(&format!("PRAGMA {pragma}"), [], |r| {
            Ok(match r.get_ref(0)? {
                rusqlite::types::ValueRef::Integer(i) => i.to_string(),
                other => String::from_utf8_lossy(other.as_bytes().unwrap_or_default()).into_owned(),
            })
        })?;
        println!("{pragma}={value}");
    }
    Ok(())
}

fn note(label: &str) -> String {
    format!("written by {label}: Gross ₱1,234,567.89, ñ ü 日本")
}

fn payload(label: &str) -> Vec<u8> {
    let mut x: u32 = label.bytes().fold(2166136261, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    (0..200_000)
        .map(|_| {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            (x >> 24) as u8
        })
        .collect()
}
