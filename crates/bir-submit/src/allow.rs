//! Where the relay pipe may connect (decision D25).
//!
//! The server asks the app to open TCP connections to BIR's dispatcher and
//! upload hosts. The app opens nothing else, so the server can never use a
//! taxpayer's computer as a general proxy. The list holds only SHA-256
//! digests of `host:port`, so this source names no BIR host: a destination
//! is allowed when the digest of what the server asked for is on it.

use sha2::{Digest, Sha256};

/// SHA-256 of `host:port` (host in lowercase) for BIR's dispatcher, on
/// HTTPS and HTTP, its backup host on both, and the SFTP upload host.
const BIR_DESTINATIONS: [&str; 5] = [
    "7cabaf85db26d0905a7d562bb10fd6177dc33e9c37769ac99d82435492778f2c",
    "ffcc0dfa54f3124b58ac85bf3598853c8e409dd7652520ecabaf90fd78cccb6f",
    "25f1139ee66598d280a85cc1f6ff97ae25c17659818c882f7913686fca65148d",
    "351df5ace66cd6312fa7c99185e34b4b4c8e052e4ad71f8bf1265e4bcebdc908",
    "8972014f6a338013ebe836517192fa5cb996365709564836a8aca5da930fb370",
];

/// The digest a destination is looked up by.
pub fn digest(host: &str, port: u16) -> String {
    let hash = Sha256::digest(format!("{}:{port}", host.to_ascii_lowercase()).as_bytes());
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Clone, Debug)]
pub struct AllowList {
    digests: Vec<String>,
}

impl AllowList {
    /// BIR's hosts only. What every real send uses.
    pub fn bir() -> Self {
        Self {
            digests: BIR_DESTINATIONS
                .iter()
                .map(|digest| (*digest).to_owned())
                .collect(),
        }
    }

    /// Adds a destination, for a lab server or a test. Never for a real send.
    pub fn with(mut self, host: &str, port: u16) -> Self {
        self.digests.push(digest(host, port));
        self
    }

    pub fn permits(&self, host: &str, port: u16) -> bool {
        let wanted = digest(host, port);
        self.digests.iter().any(|allowed| *allowed == wanted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_is_digests_of_five_destinations() {
        assert_eq!(BIR_DESTINATIONS.len(), 5);
        for digest in BIR_DESTINATIONS {
            assert_eq!(digest.len(), 64);
            assert!(
                digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            );
        }
    }

    #[test]
    fn anything_else_is_refused() {
        let list = AllowList::bir();
        for (host, port) in [
            ("127.0.0.1", 22),
            ("localhost", 443),
            ("example.com", 443),
            ("10.0.0.1", 80),
            ("169.254.169.254", 80),
            ("", 0),
        ] {
            assert!(!list.permits(host, port), "{host}:{port}");
        }
    }

    #[test]
    fn an_added_destination_is_matched_by_host_and_port() {
        let list = AllowList::bir().with("127.0.0.1", 2222);
        assert!(list.permits("127.0.0.1", 2222));
        assert!(!list.permits("127.0.0.1", 2223));
        assert!(!list.permits("127.0.0.2", 2222));
        let named = AllowList::bir().with("Lab.Example", 22);
        assert!(
            named.permits("lab.example", 22),
            "hosts compare without case"
        );
    }

    #[test]
    fn digests_are_of_lowercase_host_and_port() {
        // printf 'example.com:443' | shasum -a 256
        let expected = "2d92752e69614799ea8467c10d252c76f79c85845cf23d54406ef49ab463ff0c";
        assert_eq!(digest("EXAMPLE.com", 443), expected);
    }
}
