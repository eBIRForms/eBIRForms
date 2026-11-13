//! The TINs on the signed-in Buwiz account, and the state of each:
//! `GET /api/tax-profiles?include=all,pending`.
//!
//! - One read is shared: concurrent callers wait for one request, and a
//!   list younger than the caller's `max_age` is answered from memory. A
//!   failure is kept for 30 seconds.
//! - Offline, the last list is answered for 10 minutes, marked `stale`,
//!   unless a send was refused for a TIN since that list's read started.
//! - A banned read forgets the earlier list.
//! - Signed out, there is no request. Signing out or in clears the list.
//! - The list is complete (a TIN missing from it is not on the account)
//!   only when the server says so with `"include":"all"`.
//! - A 400 on the `include` parameter (a server that rejects it) is asked
//!   again without it; a 403 never is.
//!
//! No row, TIN or name is logged, and the types' `Debug` shows none.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use bir_core::forms::taxform::TinKey;
use serde_json::Value;

use crate::client::{ClientError, Submitter};
use crate::device;

/// How long a list read before going offline is still answered.
pub const STALE_LIMIT_SECS: i64 = 600;
/// How long a failed read is answered before the next request.
const FAILURE_SECS: i64 = 30;
/// The shortest time between two forced reads ("Check again").
const FORCED_SECS: i64 = 15;
pub(crate) const PATH: &str = "/api/tax-profiles?include=all,pending";

/// The Buwiz account site: the only host a row's `action.url` may open,
/// and the site of every link the apps build for a TIN's step.
pub const ACCOUNT_SITE: &str = "https://account.buwiz.com";

/// `url` when it is a page of [`ACCOUNT_SITE`], as the URL parser writes
/// it: https, that host exactly, no user name or password, the default
/// port. `None` for anything else.
pub fn pinned_account_url(url: &str) -> Option<String> {
    let site = url::Url::parse(ACCOUNT_SITE).expect("the account site is a URL");
    let page = url::Url::parse(url.trim()).ok()?;
    (page.scheme() == "https"
        && page.host_str().is_some()
        && page.host_str() == site.host_str()
        && page.username().is_empty()
        && page.password().is_none()
        && page.port().is_none())
    .then(|| page.to_string())
}
pub(crate) const PLAIN_PATH: &str = "/api/tax-profiles";

/// A row's state on the account. An unknown value is kept as `Other` and
/// never blocks anything.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TinStatus {
    Approved,
    Unverified,
    Pending,
    NeedsFix,
    Paused,
    Other(String),
}

impl TinStatus {
    /// A missing `status` is `approved` (servers before the state list).
    fn parse(status: Option<&str>) -> Self {
        match status {
            None | Some("approved") => TinStatus::Approved,
            Some("unverified") => TinStatus::Unverified,
            Some("pending") => TinStatus::Pending,
            Some("needs_fix") => TinStatus::NeedsFix,
            Some("paused") => TinStatus::Paused,
            Some(other) => TinStatus::Other(other.to_owned()),
        }
    }
}

/// One TIN on the account.
#[derive(Clone, PartialEq, Eq)]
pub struct AccountTin {
    /// The one key a return's TIN is compared by.
    pub key: TinKey,
    pub registered_name: String,
    pub status: TinStatus,
    pub can_send: bool,
    /// The fixed reason sentences of a `needs_fix` row.
    pub reasons: Vec<String>,
    /// Staff's free-text note of a `needs_fix` row. For app-only surfaces:
    /// never copied into a stored reason, a line an assistant reads, or an
    /// alert.
    pub note: Option<String>,
    /// The row's own page for its Verify or Fix step (`action.url`), kept
    /// only when it is a page of [`ACCOUNT_SITE`] ([`pinned_account_url`]).
    pub action_url: Option<String>,
}

impl AccountTin {
    fn parse(row: &Value) -> Option<Self> {
        let key = TinKey::parse(row["tin"].as_str()?)?;
        let status = TinStatus::parse(row["status"].as_str());
        let text = |value: &Value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        };
        Some(Self {
            key,
            registered_name: text(&row["registered_name"]).unwrap_or_default(),
            can_send: row["can_send"]
                .as_bool()
                .unwrap_or(status == TinStatus::Approved),
            status,
            reasons: row["reasons"]
                .as_array()
                .map(|reasons| reasons.iter().filter_map(text).collect())
                .unwrap_or_default(),
            note: text(&row["note"]),
            action_url: text(&row["action"]["url"]).and_then(|url| pinned_account_url(&url)),
        })
    }
}

impl std::fmt::Debug for AccountTin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountTin")
            .field("key", &self.key)
            .field("status", &self.status)
            .field("can_send", &self.can_send)
            .field("reasons", &self.reasons.len())
            .field("note", &self.note.is_some())
            .field("action_url", &self.action_url.is_some())
            .finish_non_exhaustive()
    }
}

/// The account's list as one read saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountTins {
    pub rows: Vec<AccountTin>,
    /// The server listed every state (`"include":"all"`): a TIN missing
    /// from `rows` is not on the account.
    pub complete: bool,
    /// When the list was read, in seconds since the epoch.
    pub fetched_at: i64,
    /// Answered offline from an earlier read.
    pub stale: bool,
}

impl AccountTins {
    /// A `/api/tax-profiles` answer read at `fetched_at`. A row whose `tin`
    /// is not 9, 12 or 14 digits is left out, and then the list is not
    /// complete: a TIN missing from it may be that row. It is complete only
    /// when `include` is exactly `"all"` and `profiles` is a list whose every
    /// row was read.
    pub fn parse(body: &Value, fetched_at: i64) -> Self {
        let profiles = body["profiles"].as_array();
        let rows: Vec<AccountTin> = profiles
            .map(|profiles| profiles.iter().filter_map(AccountTin::parse).collect())
            .unwrap_or_default();
        let complete = body["include"].as_str() == Some("all")
            && profiles.is_some_and(|profiles| profiles.len() == rows.len());
        Self {
            rows,
            complete,
            fetched_at,
            stale: false,
        }
    }

    /// The row for `key`.
    pub fn find(&self, key: &TinKey) -> Option<&AccountTin> {
        self.rows.iter().find(|row| &row.key == key)
    }
}

/// Why the list could not be read.
#[derive(Clone, PartialEq, Eq, thiserror::Error)]
pub enum TinsError {
    #[error("Sign in to Buwiz to see your TINs.")]
    SignedOut,
    #[error("This Buwiz account is banned.")]
    Banned,
    #[error("Buwiz can't be reached.")]
    Offline,
    /// The server's refusal or an unreadable answer; `message` is the
    /// server's and is not shown by `Debug`.
    #[error("The Buwiz server could not list your TINs ({code}).")]
    Server { code: String, message: String },
}

impl std::fmt::Debug for TinsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TinsError::SignedOut => f.write_str("SignedOut"),
            TinsError::Banned => f.write_str("Banned"),
            TinsError::Offline => f.write_str("Offline"),
            TinsError::Server { code, .. } => write!(f, "Server({code})"),
        }
    }
}

pub type TinsRead = Result<AccountTins, TinsError>;

/// The last reads, kept on the [`Submitter`].
#[derive(Default)]
pub(crate) struct TinsCache {
    state: Mutex<CacheState>,
    /// Held while a request is out, so concurrent callers share it.
    flight: tokio::sync::Mutex<()>,
}

/// Ages in the cache are timed on the monotonic clock, so a wall clock set
/// back or ahead cannot make a read look young. `fetched_at` stays the wall
/// clock's, for the gate.
///
/// Each read is kept with the generation it started in. An invalidation (a
/// send refused for a TIN) starts a new generation: a read from an older
/// one, even one still in flight then, is neither answered from memory nor
/// offline.
#[derive(Default)]
struct CacheState {
    /// The sign-in the reads below belong to.
    login: Option<String>,
    /// Bumped by every invalidation and every reset; never goes back.
    generation: u64,
    /// The last read, when it ended and the generation it started in.
    last: Option<(Instant, u64, TinsRead)>,
    /// The last list read, the same way, for answering offline.
    good: Option<(Instant, u64, AccountTins)>,
}

/// Whether a read that ended at `at` is younger than `secs` at `now`. A
/// read that seems to have ended after `now` has no known age: it is not.
fn younger(at: Instant, now: Instant, secs: i64) -> bool {
    let limit = Duration::from_secs(u64::try_from(secs).unwrap_or(0));
    now.checked_duration_since(at)
        .is_some_and(|age| age < limit)
}

impl CacheState {
    /// Forget every read, for `login` (none: signed out), in a generation
    /// no earlier read started in.
    fn reset(&mut self, login: Option<String>) {
        *self = CacheState {
            login,
            generation: self.generation + 1,
            ..CacheState::default()
        };
    }

    /// What a failed read answers: offline, the last list (marked stale)
    /// while it is under [`STALE_LIMIT_SECS`] old and of this generation.
    fn fallback(&self, now: Instant, error: TinsError) -> TinsRead {
        match (&error, &self.good) {
            (TinsError::Offline, Some((at, generation, good)))
                if *generation == self.generation && younger(*at, now, STALE_LIMIT_SECS) =>
            {
                Ok(AccountTins {
                    stale: true,
                    ..good.clone()
                })
            }
            _ => Err(error),
        }
    }
}

impl TinsCache {
    /// Forget every read (a sign-out, or a new sign-in).
    pub(crate) fn clear(&self) {
        self.state.lock().expect("tins cache").reset(None);
    }

    /// The next read asks the server, and no read from before answers
    /// offline (a send was just refused for a TIN).
    pub(crate) fn invalidate(&self) {
        self.state.lock().expect("tins cache").generation += 1;
    }

    /// The generation a read starting now belongs to.
    fn generation(&self) -> u64 {
        self.state.lock().expect("tins cache").generation
    }

    /// The answer from memory for `login` at `now`, when the last read is
    /// of this generation and young enough: a list younger than `max_age`,
    /// a failure younger than 30 seconds (and than `max_age`). `None`: ask
    /// the server.
    fn answer(&self, login: &str, now: Instant, max_age: i64) -> Option<TinsRead> {
        let mut state = self.state.lock().expect("tins cache");
        if state.login.as_deref() != Some(login) {
            state.reset(Some(login.to_owned()));
            return None;
        }
        match &state.last {
            Some((_, generation, _)) if *generation != state.generation => None,
            Some((at, _, Ok(list))) if younger(*at, now, max_age) => Some(Ok(list.clone())),
            Some((at, _, Err(error))) if younger(*at, now, max_age.min(FAILURE_SECS)) => {
                Some(state.fallback(now, error.clone()))
            }
            _ => None,
        }
    }

    /// Keep a read that started in generation `started` and ended at `now`,
    /// and answer it (offline, the last list while it is young enough). A
    /// banned read forgets the list from before the ban.
    fn store(&self, login: &str, started: u64, now: Instant, read: TinsRead) -> TinsRead {
        let mut state = self.state.lock().expect("tins cache");
        if read == Err(TinsError::SignedOut) {
            state.reset(None);
            return read;
        }
        if state.login.as_deref() != Some(login) {
            state.reset(Some(login.to_owned()));
        }
        state.last = Some((now, started, read.clone()));
        match read {
            Ok(list) => {
                state.good = Some((now, started, list.clone()));
                Ok(list)
            }
            Err(TinsError::Banned) => {
                state.good = None;
                Err(TinsError::Banned)
            }
            Err(error) => state.fallback(now, error),
        }
    }

    /// Make every read `secs` older, as if that much time had passed (a
    /// negative `secs`: as if it ended that far ahead of now).
    #[cfg(test)]
    fn age_for_tests(&self, secs: i64) {
        let shift = |at: &mut Instant| {
            let by = Duration::from_secs(secs.unsigned_abs());
            *at = if secs >= 0 { *at - by } else { *at + by };
        };
        let mut state = self.state.lock().expect("tins cache");
        if let Some((at, _, _)) = &mut state.last {
            shift(at);
        }
        if let Some((at, _, good)) = &mut state.good {
            shift(at);
            good.fetched_at -= secs;
        }
    }
}

impl Submitter {
    /// The account's TINs, from memory when the last read is younger than
    /// `max_age_secs` (screens pass 60, an assistant's card 300).
    pub async fn account_tins(&self, max_age_secs: i64) -> TinsRead {
        let Some(login) = self.login() else {
            self.tins.clear();
            return Err(TinsError::SignedOut);
        };
        if let Some(read) = self.tins.answer(&login, Instant::now(), max_age_secs) {
            return read;
        }
        let _flight = self.tins.flight.lock().await;
        // Another caller may have read it while this one waited.
        if let Some(read) = self.tins.answer(&login, Instant::now(), max_age_secs) {
            return read;
        }
        let started = self.tins.generation();
        let fetched_at = device::now();
        let read = match self.bearer_get(PATH).await {
            Ok(body) => Ok(AccountTins::parse(&body, fetched_at)),
            // A server that rejects `include`: the plain list, which says
            // nothing about a TIN missing from it.
            Err(ClientError::Server { status: 400, .. }) => match self.bearer_get(PLAIN_PATH).await
            {
                Ok(body) => Ok(AccountTins {
                    complete: false,
                    ..AccountTins::parse(&body, fetched_at)
                }),
                Err(error) => Err(tins_error(error)),
            },
            Err(error) => Err(tins_error(error)),
        };
        self.tins.store(&login, started, Instant::now(), read)
    }

    /// "Check again": a new read, unless one ended in the last 15 seconds.
    pub async fn refresh_account_tins(&self) -> TinsRead {
        self.account_tins(FORCED_SECS).await
    }

    /// The last list read for this sign-in, without a request (for
    /// rendering); judge its age by `fetched_at`. `None` after a banned
    /// read.
    pub fn cached_account_tins(&self) -> Option<AccountTins> {
        let login = self.login()?;
        let state = self.tins.state.lock().expect("tins cache");
        (state.login.as_deref() == Some(login.as_str()))
            .then(|| state.good.as_ref().map(|(_, _, good)| good.clone()))
            .flatten()
    }

    /// The next [`Self::account_tins`] asks the server whatever its age.
    pub fn invalidate_account_tins(&self) {
        self.tins.invalidate();
    }
}

fn tins_error(error: ClientError) -> TinsError {
    match error {
        ClientError::SignedOut => TinsError::SignedOut,
        ClientError::Network(_) => TinsError::Offline,
        ClientError::Server { code, .. } if code == "account_banned" => TinsError::Banned,
        ClientError::Server { code, message, .. } => TinsError::Server { code, message },
        ClientError::Other(message) => TinsError::Server {
            code: "unreadable".to_owned(),
            message,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::sync::{Arc, Mutex};

    use bir_core::tin_refusal::TinRefusal;
    use serde_json::json;

    use super::*;
    use crate::client::PollOutcome;
    use crate::client::{CLIENT_ID, SubmitConfig};
    use crate::secrets::{MemoryStore, SecretStore};

    /// What the fake server answers: a status and a body, or `None` to
    /// drop the connection unanswered (an outage).
    type Answer = Option<(u16, String)>;

    /// A loopback server that answers each request with `answer(n, path)`
    /// (`n` counts from 0) after `delay_ms`; the request lines it saw.
    fn fake_server(
        delay_ms: u64,
        answer: impl Fn(usize, &str) -> Answer + Send + 'static,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for (n, stream) in listener.incoming().enumerate() {
                let Ok(mut stream) = stream else {
                    continue;
                };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap_or_default();
                let mut length = 0;
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap_or(0) == 0 || header.trim().is_empty() {
                        break;
                    }
                    if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:")
                    {
                        length = value.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; length];
                let _ = reader.read_exact(&mut body);
                let request = line.trim().trim_end_matches(" HTTP/1.1").to_owned();
                let path = request
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned();
                log.lock().unwrap().push(request);
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                if let Some((status, json)) = answer(n, &path) {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{json}",
                        json.len()
                    );
                }
            }
        });
        (format!("http://{addr}"), seen)
    }

    const SIGNED_IN: &str = r#"{"access_token":"a","refresh_token":"r","access_expires_at":4102444800,"email":"juana@example.com","device_id":"d"}"#;

    fn submitter(url: &str, signed_in: bool) -> Submitter {
        let store = SharedStore::default();
        if signed_in {
            store.set("session", SIGNED_IN).unwrap();
        }
        submitter_with(url, store, Vec::new())
    }

    /// A submitter on `store` that pins `trust_keys`.
    fn submitter_with(url: &str, store: SharedStore, trust_keys: Vec<String>) -> Submitter {
        let config = SubmitConfig {
            server_url: url.to_owned(),
            client_id: CLIENT_ID.into(),
            device_name: "test".into(),
            trust_keys,
        };
        Submitter::new(config, Box::new(store))
    }

    /// A secret store the test keeps a handle on, to write a session back
    /// the way a sign-in elsewhere would.
    #[derive(Clone, Default)]
    struct SharedStore(Arc<MemoryStore>);

    impl SecretStore for SharedStore {
        fn get(&self, name: &str) -> Result<Option<String>, crate::secrets::StoreError> {
            self.0.get(name)
        }

        fn set(&self, name: &str, value: &str) -> Result<(), crate::secrets::StoreError> {
            self.0.set(name, value)
        }

        fn delete(&self, name: &str) -> Result<(), crate::secrets::StoreError> {
            self.0.delete(name)
        }
    }

    /// How many list reads the fake server saw.
    fn list_reads(seen: &Mutex<Vec<String>>) -> usize {
        seen.lock()
            .unwrap()
            .iter()
            .filter(|request| request.starts_with("GET /api/tax-profiles"))
            .count()
    }

    /// The pinned key, a trust list it signed and the sealing key document
    /// the list names: what a send reads before its ticket.
    fn trust_documents() -> (String, String, String) {
        use base64::Engine;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use ed25519_dalek::{Signer, SigningKey};
        use sha2::{Digest, Sha256};

        let key = SigningKey::from_bytes(&[7; 32]);
        let public = URL_SAFE_NO_PAD.encode(key.verifying_key().as_bytes());
        let kid = crate::device::hex(&Sha256::digest(key.verifying_key().as_bytes())[..8]);
        let sealing = json!({ "kid": "s1", "kem": crate::seal::KEM, "public_key": URL_SAFE_NO_PAD.encode([1; 32]) });
        let claims = json!({
            "hosts": ["127.0.0.1:9"],
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
        let jws = format!(
            "{input}.{}",
            URL_SAFE_NO_PAD.encode(key.sign(input.as_bytes()).to_bytes())
        );
        (public, jws, json!({ "keys": [sealing] }).to_string())
    }

    fn ok(body: Value) -> Answer {
        Some((200, body.to_string()))
    }

    fn key(tin: &str) -> TinKey {
        TinKey::parse(tin).unwrap()
    }

    #[tokio::test]
    async fn account_tins_reads_today_servers_list() {
        let (url, seen) = fake_server(0, |_, _| {
            ok(
                json!({"profiles":[{"id":"p1","tin":"000-123-453-00000","registered_name":"X","permissions":["filing.submit"]}]}),
            )
        });
        let list = submitter(&url, true).account_tins(60).await.unwrap();
        assert_eq!(list.rows.len(), 1);
        let row = &list.rows[0];
        assert_eq!(row.key, key("000123453"));
        assert_eq!(row.status, TinStatus::Approved);
        assert!(row.can_send);
        assert!(!list.complete);
        assert!(!list.stale);
        assert_eq!(
            *seen.lock().unwrap(),
            ["GET /api/tax-profiles?include=all,pending"]
        );
    }

    #[tokio::test]
    async fn account_tins_reads_pending_and_needs_fix_rows_of_today_server() {
        let (url, _) = fake_server(0, |_, _| {
            ok(json!({"profiles":[
                {"tin":"000-123-456-00000","registered_name":"X","status":"pending"},
                {"tin":"000-123-457-00000","registered_name":"Y","status":"needs_fix"}
            ]}))
        });
        let list = submitter(&url, true).account_tins(60).await.unwrap();
        assert_eq!(list.rows[0].status, TinStatus::Pending);
        assert_eq!(list.rows[1].status, TinStatus::NeedsFix);
        assert!(!list.rows[0].can_send && !list.rows[1].can_send);
        assert!(!list.complete);
    }

    #[tokio::test]
    async fn account_tins_new_server_marker_makes_it_complete() {
        let (url, _) = fake_server(0, |_, _| ok(json!({"include":"all","profiles":[]})));
        let list = submitter(&url, true).account_tins(60).await.unwrap();
        assert!(list.complete);
        assert!(list.rows.is_empty());
    }

    /// A row that cannot be read leaves a hole: a TIN missing from the list
    /// may be that row, so the list is not complete.
    #[test]
    fn a_list_with_an_unreadable_row_is_not_complete() {
        let good = json!({"tin":"000-123-453-00000","registered_name":"X","status":"approved","can_send":true});
        for bad in [
            json!({"tin_masked":"***-***-454-00000","registered_name":"Y","status":"paused"}),
            json!({"tin":"000-123-454-0000","registered_name":"Y"}),
            json!({"tin":123454,"registered_name":"Y"}),
            json!("not a row"),
        ] {
            let list = AccountTins::parse(&json!({"include":"all","profiles":[good, bad]}), 1);
            assert_eq!(list.rows.len(), 1, "{bad}");
            assert!(!list.complete, "{bad}");
        }
        for body in [
            json!({"include":"all"}),
            json!({"include":"all","profiles":null}),
            json!({"include":"all","profiles":{"tin":"000123453"}}),
            json!({"include":"all","tins":[good]}),
        ] {
            assert!(!AccountTins::parse(&body, 1).complete, "{body}");
        }
        assert!(AccountTins::parse(&json!({"include":"all","profiles":[good]}), 1).complete);
    }

    /// Only `"include":"all"` itself marks the list complete; a server that
    /// echoes the request's include string does not.
    #[test]
    fn only_include_all_exactly_is_complete() {
        for include in [
            json!("all,pending"),
            json!("pending,all"),
            json!(" all"),
            json!("ALL"),
            json!(["all"]),
            json!(true),
        ] {
            let list = AccountTins::parse(&json!({"include":include,"profiles":[]}), 1);
            assert!(!list.complete, "{include}");
        }
    }

    #[tokio::test]
    async fn account_tins_reads_unverified_and_paused_with_can_send_false() {
        let (url, _) = fake_server(0, |_, _| {
            ok(json!({"include":"all","profiles":[
                {"tin":"000-123-455-00000","registered_name":"X","status":"unverified","can_send":false},
                {"tin":"000-123-454-00002","registered_name":"Y","status":"paused","can_send":false},
                {"tin":"000-123-458-00000","registered_name":"Z","status":"approved","can_send":false}
            ]}))
        });
        let list = submitter(&url, true).account_tins(60).await.unwrap();
        assert_eq!(list.rows[0].status, TinStatus::Unverified);
        assert_eq!(list.rows[1].status, TinStatus::Paused);
        assert_eq!(list.rows[1].key, key("000-123-454-002"));
        assert_eq!(list.rows[2].status, TinStatus::Approved);
        assert!(list.rows.iter().all(|row| !row.can_send));
    }

    #[tokio::test]
    async fn account_tins_needs_fix_keeps_reasons_and_note() {
        let (url, _) = fake_server(0, |_, _| {
            ok(json!({"include":"all","profiles":[{
                "tin":"000-123-457-00000","registered_name":"X","status":"needs_fix","can_send":false,
                "reasons":["A photo is too blurry to read."],"note":"Retake it in daylight.",
                "action":{"label":"Fix and send again","url":"https://account.buwiz.com/tins/claims/c1/fix"}
            }]}))
        });
        let list = submitter(&url, true).account_tins(60).await.unwrap();
        let row = &list.rows[0];
        assert_eq!(row.reasons, ["A photo is too blurry to read."]);
        assert_eq!(row.note.as_deref(), Some("Retake it in daylight."));
        assert_eq!(
            row.action_url.as_deref(),
            Some("https://account.buwiz.com/tins/claims/c1/fix")
        );
    }

    #[tokio::test]
    async fn an_unknown_status_is_kept_as_other() {
        let (url, _) = fake_server(0, |_, _| {
            ok(
                json!({"include":"all","profiles":[{"tin":"000123453","registered_name":"X","status":"frozen"}]}),
            )
        });
        let list = submitter(&url, true).account_tins(60).await.unwrap();
        assert_eq!(list.rows[0].status, TinStatus::Other("frozen".into()));
        assert!(!list.rows[0].can_send);
    }

    #[tokio::test]
    async fn account_tins_falls_back_when_include_is_rejected() {
        let (url, seen) = fake_server(0, |n, _| match n {
            0 => Some((
                400,
                r#"{"error":{"code":"bad_request","message":"unknown include"}}"#.into(),
            )),
            _ => ok(
                json!({"include":"all","profiles":[{"tin":"000-123-453-00000","registered_name":"X"}]}),
            ),
        });
        let list = submitter(&url, true).account_tins(60).await.unwrap();
        assert_eq!(list.rows.len(), 1);
        assert!(
            !list.complete,
            "a list read without include is never complete"
        );
        assert_eq!(
            *seen.lock().unwrap(),
            [
                "GET /api/tax-profiles?include=all,pending",
                "GET /api/tax-profiles"
            ]
        );
    }

    #[tokio::test]
    async fn account_tins_banned_is_its_own_error_and_is_not_asked_again_without_include() {
        let (url, seen) = fake_server(0, |_, _| {
            Some((
                403,
                r#"{"error":{"code":"account_banned","message":"This Buwiz account is banned."}}"#
                    .into(),
            ))
        });
        let submitter = submitter(&url, true);
        assert_eq!(submitter.account_tins(60).await, Err(TinsError::Banned));
        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "a 403 is never retried without include"
        );
        // Still signed in: a ban is not a sign-out.
        assert!(matches!(
            submitter.sign_in_status(),
            crate::SignInStatus::SignedIn { .. }
        ));
    }

    #[tokio::test]
    async fn another_403_is_not_asked_again_either() {
        let (url, seen) = fake_server(0, |_, _| {
            Some((
                403,
                r#"{"error":{"code":"forbidden","message":"no"}}"#.into(),
            ))
        });
        let read = submitter(&url, true).account_tins(60).await;
        assert!(
            matches!(read, Err(TinsError::Server { ref code, .. }) if code == "forbidden"),
            "{read:?}"
        );
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn account_tins_is_cached_and_single_flight() {
        let (url, seen) = fake_server(200, |_, _| ok(json!({"include":"all","profiles":[]})));
        let submitter = Arc::new(submitter(&url, true));
        let reads = (0..8).map(|_| {
            let submitter = submitter.clone();
            tokio::spawn(async move { submitter.account_tins(60).await })
        });
        for read in futures_util::future::join_all(reads).await {
            assert!(read.unwrap().is_ok());
        }
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert!(submitter.account_tins(60).await.is_ok());
        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "a read inside max_age makes no request"
        );
        assert!(submitter.cached_account_tins().is_some());
        // A refusal seen on a send asks again.
        submitter.invalidate_account_tins();
        assert!(submitter.account_tins(60).await.is_ok());
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    /// A send refused while a list read is out: the list that read brings
    /// back is from before the refusal, so the next read still asks.
    #[tokio::test]
    async fn a_read_in_flight_when_the_list_is_invalidated_does_not_clear_it() {
        let (url, seen) = fake_server(300, |_, _| ok(json!({"include":"all","profiles":[]})));
        let submitter = Arc::new(submitter(&url, true));
        let reading = {
            let submitter = submitter.clone();
            tokio::spawn(async move { submitter.account_tins(60).await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        submitter.invalidate_account_tins();
        assert!(reading.await.unwrap().is_ok());
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert!(submitter.account_tins(60).await.is_ok());
        assert_eq!(
            seen.lock().unwrap().len(),
            2,
            "the read that started before the refusal does not count"
        );
        assert!(submitter.account_tins(60).await.is_ok());
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    /// After a refusal the list from before it is not answered offline: the
    /// read is offline, not the old list marked stale.
    #[tokio::test]
    async fn offline_after_an_invalidation_does_not_answer_the_older_list() {
        let (url, _) = fake_server(0, |n, _| match n {
            0 => {
                ok(json!({"include":"all","profiles":[{"tin":"000123453","registered_name":"X"}]}))
            }
            _ => None,
        });
        let submitter = submitter(&url, true);
        assert!(submitter.account_tins(60).await.is_ok());
        submitter.invalidate_account_tins();
        assert_eq!(submitter.account_tins(60).await, Err(TinsError::Offline));
        assert_eq!(submitter.account_tins(0).await, Err(TinsError::Offline));
    }

    /// After a banned read neither the cache nor the offline answer hands
    /// back the list from before the ban.
    #[tokio::test]
    async fn a_banned_read_hides_the_earlier_list() {
        let (url, _) = fake_server(0, |n, _| match n {
            0 => {
                ok(json!({"include":"all","profiles":[{"tin":"000123453","registered_name":"X"}]}))
            }
            1 => Some((
                403,
                r#"{"error":{"code":"account_banned","message":"banned"}}"#.into(),
            )),
            _ => None,
        });
        let submitter = submitter(&url, true);
        assert!(submitter.account_tins(60).await.is_ok());
        assert!(submitter.cached_account_tins().is_some());
        assert_eq!(submitter.account_tins(0).await, Err(TinsError::Banned));
        assert_eq!(submitter.cached_account_tins(), None);
        assert_eq!(submitter.account_tins(0).await, Err(TinsError::Offline));
    }

    #[tokio::test]
    async fn a_failure_is_kept_for_thirty_seconds() {
        let (url, seen) = fake_server(0, |_, _| Some((500, "{}".into())));
        let submitter = submitter(&url, true);
        assert!(matches!(
            submitter.account_tins(60).await,
            Err(TinsError::Server { .. })
        ));
        assert!(matches!(
            submitter.account_tins(60).await,
            Err(TinsError::Server { .. })
        ));
        assert_eq!(seen.lock().unwrap().len(), 1);
        // A forced read waits out only its own 15 seconds; `max_age` 0 asks.
        assert!(submitter.account_tins(0).await.is_err());
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn account_tins_offline_serves_stale_then_errors() {
        let (url, _) = fake_server(0, |n, _| match n {
            0 => {
                ok(json!({"include":"all","profiles":[{"tin":"000123453","registered_name":"X"}]}))
            }
            _ => None,
        });
        let submitter = submitter(&url, true);
        let first = submitter.account_tins(60).await.unwrap();
        assert!(!first.stale);
        let offline = submitter.account_tins(0).await.unwrap();
        assert!(offline.stale);
        assert_eq!(offline.rows, first.rows);
        assert_eq!(offline.fetched_at, first.fetched_at);
        // Past the limit the list is too old to answer.
        submitter.tins.age_for_tests(STALE_LIMIT_SECS + 1);
        assert_eq!(submitter.account_tins(0).await, Err(TinsError::Offline));
    }

    /// A read whose time is ahead of now (the clock was set back) is of no
    /// known age: it is asked again, a failure does not hold requests off,
    /// and the list is not answered offline.
    #[tokio::test]
    async fn a_read_from_ahead_of_now_is_expired() {
        let (url, seen) = fake_server(0, |n, _| match n {
            1 => None,
            _ => {
                ok(json!({"include":"all","profiles":[{"tin":"000123453","registered_name":"X"}]}))
            }
        });
        let submitter = submitter(&url, true);
        assert!(submitter.account_tins(60).await.is_ok());
        submitter.tins.age_for_tests(-86_400);
        assert_eq!(
            submitter.account_tins(60).await,
            Err(TinsError::Offline),
            "a list from ahead of now is neither answered from memory nor offline"
        );
        assert_eq!(seen.lock().unwrap().len(), 2);
        submitter.tins.age_for_tests(-86_400);
        assert!(submitter.account_tins(60).await.is_ok());
        assert_eq!(
            seen.lock().unwrap().len(),
            3,
            "a failure from ahead of now does not hold requests off"
        );
    }

    #[tokio::test]
    async fn an_unreachable_server_reads_offline() {
        let addr = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let submitter = submitter(&format!("http://{addr}"), true);
        assert_eq!(submitter.account_tins(60).await, Err(TinsError::Offline));
    }

    #[tokio::test]
    async fn account_tins_signed_out_makes_no_request() {
        let (url, seen) = fake_server(0, |_, _| ok(json!({"profiles":[]})));
        let submitter = submitter(&url, false);
        assert_eq!(submitter.account_tins(60).await, Err(TinsError::SignedOut));
        assert!(submitter.cached_account_tins().is_none());
        assert!(seen.lock().unwrap().is_empty());
    }

    /// Signing out forgets the list: signed in again as the same login,
    /// with no read in between, the next read asks the server.
    #[tokio::test]
    async fn sign_out_clears_the_cached_list() {
        let (url, seen) = fake_server(0, |_, _| ok(json!({"include":"all","profiles":[]})));
        let store = SharedStore::default();
        store.set("session", SIGNED_IN).unwrap();
        let submitter = submitter_with(&url, store.clone(), Vec::new());
        assert!(submitter.account_tins(60).await.is_ok());
        assert!(submitter.cached_account_tins().is_some());
        submitter.sign_out().await.unwrap();
        assert!(submitter.cached_account_tins().is_none());
        // The same login signs in again (its session written back, as a
        // sign-in in another window would leave it).
        store.set("session", SIGNED_IN).unwrap();
        assert!(submitter.cached_account_tins().is_none());
        assert!(submitter.account_tins(60).await.is_ok());
        assert_eq!(list_reads(&seen), 2, "a fresh read after signing in again");
        submitter.sign_out().await.unwrap();
        assert_eq!(submitter.account_tins(60).await, Err(TinsError::SignedOut));
    }

    /// A device sign-in, another login or the same one, forgets the list.
    #[tokio::test]
    async fn a_new_sign_in_clears_the_cached_list() {
        let login = Arc::new(Mutex::new("juana@example.com".to_owned()));
        let me = login.clone();
        let (url, seen) = fake_server(0, move |_, path| match path {
            "/auth/device/start" => ok(json!({
                "device_code": "dc", "user_code": "ABCD",
                "verification_uri": "http://127.0.0.1/device",
                "verification_uri_complete": "http://127.0.0.1/device?code=ABCD",
                "expires_in": 600, "interval": 5
            })),
            "/auth/device/poll" => {
                ok(json!({"access_token":"a2","refresh_token":"r2","expires_in":900}))
            }
            "/me" => ok(json!({ "email": *me.lock().unwrap() })),
            "/api/devices" => ok(json!({"id":"d2"})),
            _ => ok(json!({"include":"all","profiles":[]})),
        });
        let submitter = submitter(&url, true);
        let sign_in = || async {
            submitter.start_sign_in().await.unwrap();
            assert_eq!(
                submitter.poll_sign_in().await.unwrap(),
                PollOutcome::SignedIn
            );
        };
        assert!(submitter.account_tins(60).await.is_ok());
        assert_eq!(list_reads(&seen), 1);

        *login.lock().unwrap() = "pedro@example.com".to_owned();
        sign_in().await;
        assert!(submitter.cached_account_tins().is_none());
        assert!(submitter.account_tins(60).await.is_ok());
        assert_eq!(list_reads(&seen), 2, "another login reads its own list");

        sign_in().await;
        assert!(submitter.cached_account_tins().is_none());
        assert!(submitter.account_tins(60).await.is_ok());
        assert_eq!(
            list_reads(&seen),
            3,
            "the same login signing in again reads anew"
        );
    }

    /// A 401 `invalid_token` is asked once more with a refreshed token, and
    /// only once.
    #[tokio::test]
    async fn an_expired_token_is_refreshed_and_the_list_asked_once_more() {
        let (url, seen) = fake_server(0, |n, path| match (n, path) {
            (0, _) => Some((
                401,
                r#"{"error":{"code":"invalid_token","message":"expired"}}"#.into(),
            )),
            (_, "/oauth/token") => {
                ok(json!({"access_token":"a2","refresh_token":"r2","expires_in":900}))
            }
            _ => ok(json!({"include":"all","profiles":[]})),
        });
        assert!(submitter(&url, true).account_tins(60).await.is_ok());
        assert_eq!(
            *seen.lock().unwrap(),
            [
                "GET /api/tax-profiles?include=all,pending",
                "POST /oauth/token",
                "GET /api/tax-profiles?include=all,pending"
            ]
        );

        let (url, seen) = fake_server(0, |_, path| match path {
            "/oauth/token" => {
                ok(json!({"access_token":"a2","refresh_token":"r2","expires_in":900}))
            }
            _ => Some((
                401,
                r#"{"error":{"code":"invalid_token","message":"expired"}}"#.into(),
            )),
        });
        let read = submitter(&url, true).account_tins(60).await;
        assert!(
            matches!(read, Err(TinsError::Server { ref code, .. }) if code == "invalid_token"),
            "{read:?}"
        );
        assert_eq!(seen.lock().unwrap().len(), 3, "refreshed once, not again");
    }

    /// A send the server refuses for its TIN makes the next read ask; one
    /// that fails otherwise does not.
    #[tokio::test]
    async fn a_send_refused_for_its_tin_makes_the_next_read_ask() {
        const FILE: &str = "00012345300000-1701Qv2018-2026Q1#juana@example.com#.xml";
        for (ticket, invalidates) in [
            (
                (
                    403,
                    r#"{"error":{"code":"tin_paused","message":"Sending is paused for this TIN."}}"#,
                ),
                true,
            ),
            (
                (
                    500,
                    r#"{"error":{"code":"internal","message":"try again later"}}"#,
                ),
                false,
            ),
        ] {
            let (public, trust, hpke) = trust_documents();
            let (url, seen) = fake_server(0, move |_, path| match path {
                "/.well-known/buwiz-trust.json" => Some((200, trust.clone())),
                "/.well-known/buwiz-hpke.json" => Some((200, hpke.clone())),
                "/api/tickets" => Some((ticket.0, ticket.1.to_owned())),
                _ => ok(json!({"include":"all","profiles":[]})),
            });
            let store = SharedStore::default();
            store.set("session", SIGNED_IN).unwrap();
            let submitter = submitter_with(&url, store, vec![public]);
            assert!(submitter.account_tins(60).await.is_ok());
            let sent = submitter.send(FILE, b"<xml/>").await;
            let Err(error) = sent else {
                panic!("the ticket was refused");
            };
            assert!(matches!(error, crate::SendError::NotSent(_)), "{error:?}");
            assert_eq!(error.refusal().is_some(), invalidates, "{error:?}");
            assert!(submitter.account_tins(60).await.is_ok());
            assert_eq!(
                list_reads(&seen),
                if invalidates { 2 } else { 1 },
                "{error:?}"
            );
        }
    }

    /// A refusal at session open (here a banned account registering its
    /// device) makes the next read ask too.
    #[tokio::test]
    async fn a_refusal_at_session_open_makes_the_next_read_ask() {
        let (public, trust, _) = trust_documents();
        let (url, seen) = fake_server(0, move |_, path| match path {
            "/.well-known/buwiz-trust.json" => Some((200, trust.clone())),
            "/api/devices" => Some((
                403,
                r#"{"error":{"code":"account_banned","message":"This Buwiz account is banned."}}"#
                    .into(),
            )),
            _ => ok(json!({"include":"all","profiles":[]})),
        });
        let store = SharedStore::default();
        store
            .set(
                "session",
                r#"{"access_token":"a","refresh_token":"r","access_expires_at":4102444800,"email":"juana@example.com"}"#,
            )
            .unwrap();
        let submitter = submitter_with(&url, store, vec![public]);
        assert!(submitter.account_tins(60).await.is_ok());
        let ready = submitter.ready().await;
        assert_eq!(
            ready.as_ref().err().and_then(ClientError::tin_refusal),
            Some(TinRefusal::AccountBanned),
            "{ready:?}"
        );
        assert!(submitter.account_tins(60).await.is_ok());
        assert_eq!(list_reads(&seen), 2);
    }

    /// Pages that look like the account site but are not, and the account
    /// site with something odd about it.
    const LOOKALIKES: [&str; 14] = [
        "https://account.buwiz.com.evil.example/tins/claims/c1/fix",
        "https://account.buwiz.com@evil.example/tins/claims/c1/fix",
        "https://evil.example/tins/claims/c1/fix",
        "https://evil.example\\@account.buwiz.com/tins",
        "http://account.buwiz.com/tins/claims/c1/fix",
        "https://user@account.buwiz.com/tins/claims/c1/fix",
        "https://user:secret@account.buwiz.com/tins/claims/c1/fix",
        "https://account.buwiz.com:8443/tins/claims/c1/fix",
        "https://account.buwiz.com./tins/claims/c1/fix",
        "https://xaccount.buwiz.com/tins",
        "https://buwiz.com/tins",
        "//account.buwiz.com/tins",
        "javascript:alert(1)",
        "account.buwiz.com/tins",
    ];

    #[test]
    fn only_a_page_of_the_account_site_is_kept_as_a_rows_link() {
        for url in LOOKALIKES {
            assert_eq!(pinned_account_url(url), None, "{url}");
            let body = json!({"include":"all","profiles":[{
                "tin":"000-123-457-00000","registered_name":"X","status":"needs_fix","can_send":false,
                "action":{"label":"Fix","url":url}
            }]});
            assert_eq!(
                AccountTins::parse(&body, 1).rows[0].action_url,
                None,
                "{url}"
            );
        }
        for (url, kept) in [
            (
                "https://account.buwiz.com/tins/claims/c1/fix",
                "https://account.buwiz.com/tins/claims/c1/fix",
            ),
            (
                "https://account.buwiz.com:443/tins/claims/c1/verify",
                "https://account.buwiz.com/tins/claims/c1/verify",
            ),
            (
                "https://ACCOUNT.buwiz.com/tins",
                "https://account.buwiz.com/tins",
            ),
        ] {
            assert_eq!(pinned_account_url(url).as_deref(), Some(kept), "{url}");
        }
        assert!(ACCOUNT_SITE.starts_with("https://"));
    }

    #[test]
    fn debug_output_never_shows_a_tin() {
        let body: Value =
            serde_json::from_str(include_str!("../tests/fixtures/tax_profiles_all.json")).unwrap();
        let list = AccountTins::parse(&body, 1);
        assert_eq!(list.rows.len(), 5);
        let shown = format!(
            "{list:?} {:?} {:?}",
            TinsError::Server {
                code: "x".into(),
                message: "TIN 000-123-453 SYNTHETIC TAXPAYER ONE".into()
            },
            Err::<AccountTins, _>(TinsError::Banned)
        );
        for secret in ["123", "453", "SYNTHETIC", "daylight", "claims"] {
            assert!(!shown.contains(secret), "{secret} in {shown}");
        }
    }
}
