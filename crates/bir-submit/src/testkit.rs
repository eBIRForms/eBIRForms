//! Test helpers: a loopback HTTP server that records every request with its
//! headers, a signed trust list, and a signed-in submitter. Nothing here
//! leaves 127.0.0.1.

use std::io::{BufRead, BufReader, Read, Write};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::client::{CLIENT_ID, SubmitConfig, Submitter};
use crate::secrets::{MemoryStore, SecretStore};

/// One request the fake server saw.
#[derive(Clone, Debug)]
pub(crate) struct Seen {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// An answer: status, extra headers, body.
pub(crate) struct Answer {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: String,
}

impl Answer {
    pub fn json(status: u16, body: &Value) -> Self {
        Self { status, headers: Vec::new(), body: body.to_string() }
    }

    pub fn with(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }
}

/// A loopback server answering each request with `answer`.
pub(crate) fn serve(
    answer: impl Fn(&Seen) -> Answer + Send + 'static,
) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                continue;
            };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap_or_default();
            let mut parts = line.split_whitespace();
            let method = parts.next().unwrap_or_default().to_owned();
            let path = parts.next().unwrap_or_default().to_owned();
            let mut headers = Vec::new();
            let mut length = 0;
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) == 0 || header.trim().is_empty() {
                    break;
                }
                if let Some((name, value)) = header.trim().split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                    headers.push((name.trim().to_owned(), value.trim().to_owned()));
                }
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            let request = Seen { method, path, headers };
            let reply = answer(&request);
            log.lock().unwrap().push(request);
            let mut head = format!(
                "HTTP/1.1 {} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
                reply.status,
                reply.body.len()
            );
            for (name, value) in reply.headers {
                head.push_str(&format!("{name}: {value}\r\n"));
            }
            let _ = write!(stream, "{head}\r\n{}", reply.body);
        }
    });
    (format!("http://{addr}"), seen)
}

/// A session with a far-off expiry and a registered device.
pub(crate) const SIGNED_IN: &str = r#"{"access_token":"a","refresh_token":"r","access_expires_at":4102444800,"email":"juana@example.test","device_id":"d"}"#;

/// A submitter against `url`, signed in when `signed_in`, pinning
/// `trust_keys`.
pub(crate) fn submitter(url: &str, signed_in: bool, trust_keys: Vec<String>) -> Submitter {
    let store = MemoryStore::default();
    if signed_in {
        store.set("session", SIGNED_IN).unwrap();
    }
    let config = SubmitConfig {
        server_url: url.to_owned(),
        client_id: CLIENT_ID.into(),
        device_name: "test".into(),
        trust_keys,
    };
    Submitter::new(config, Box::new(store))
}

/// The pinned key, a trust list it signed for `hosts`, and the sealing key
/// document the list names.
pub(crate) fn trust_documents(hosts: &[&str]) -> (String, String, String) {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use ed25519_dalek::{Signer, SigningKey};
    use sha2::{Digest, Sha256};

    let key = SigningKey::from_bytes(&[7; 32]);
    let public = URL_SAFE_NO_PAD.encode(key.verifying_key().as_bytes());
    let kid = crate::device::hex(&Sha256::digest(key.verifying_key().as_bytes())[..8]);
    let sealing = json!({ "kid": "s1", "kem": crate::seal::KEM, "public_key": URL_SAFE_NO_PAD.encode([1; 32]) });
    let claims = json!({
        "hosts": hosts,
        "sealing_keys": [sealing],
        "receipt_keys": [{ "kty": "OKP", "crv": "Ed25519", "kid": kid, "x": public }],
        "iat": 1,
        "exp": 4_102_444_800_i64,
    });
    let header = json!({ "alg": "EdDSA", "kid": kid, "typ": crate::trust::TYP });
    let input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap()),
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
    );
    let jws = format!("{input}.{}", URL_SAFE_NO_PAD.encode(key.sign(input.as_bytes()).to_bytes()));
    (public, jws, json!({ "keys": [sealing] }).to_string())
}

/// A fixture from `tests/fixtures`, named after its `API.md` section.
pub(crate) fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

pub(crate) fn fixture_json(name: &str) -> Value {
    serde_json::from_str(&fixture(name)).expect("fixture JSON")
}
