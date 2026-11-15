//! The client of buwiz-server: device-flow sign-in, the device key,
//! tickets, sealed v2 submissions, the relay pipe and receipts.

use std::sync::{Mutex, RwLock};

use bir_core::bir_sign::{SIGN_HEADER, SeenSign, Sign, SignCell};
use bir_core::smart_queue::{
    Blame, FailureClass, capped_wait_until, is_final_reason, is_hold_code, is_limit_code,
};
use bir_core::tin_refusal::TinRefusal;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::device::{self, DeviceKey, HEADER_DEVICE, hex};
use crate::pipe::{self, Ended};
use crate::receipt::Receipt;
use crate::seal::{self, SealingKey};
use crate::secrets::SecretStore;
use crate::trust::{self, TrustList};

/// Where the server is when neither `BUWIZ_API_URL` nor the setting
/// says otherwise.
pub const DEFAULT_SERVER_URL: &str = "https://api.buwiz.com";
/// The OAuth client the desktop and phone apps sign in as.
pub const CLIENT_ID: &str = "buwiz-desktop";
/// The header that names the app and its version on every request to the
/// Buwiz server: `desktop/0.2.0`, `mini/0.5.0`.
pub const CLIENT_HEADER: &str = "buwiz-client";
const SESSION: &str = "session";
const DEVICE_KEY: &str = "device_key";

#[derive(Clone, Debug)]
pub struct SubmitConfig {
    pub server_url: String,
    pub client_id: String,
    /// Shown on the account site's device list.
    pub device_name: String,
    /// Public keys (base64url) the signed trust list must verify with:
    /// [`trust::pinned_keys`] unless a test or lab sets its own in code.
    pub trust_keys: Vec<String>,
}

/// An https URL, or an http one whose host is this machine.
fn is_private_url(url: &str) -> bool {
    if url.starts_with("https://") {
        return true;
    }
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or_default(),
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

impl SubmitConfig {
    /// `BUWIZ_API_URL` when set, else the app's setting, else
    /// [`DEFAULT_SERVER_URL`]. Only https, or http to this machine: sign-in
    /// codes and tokens never cross a network in plain text.
    pub fn new(setting: Option<&str>) -> Self {
        let from_env = std::env::var("BUWIZ_API_URL")
            .ok()
            .map(|url| url.trim().to_owned())
            .filter(|url| is_private_url(url));
        let url = from_env
            .or_else(|| {
                setting
                    .map(str::trim)
                    .filter(|url| is_private_url(url))
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| DEFAULT_SERVER_URL.to_owned());
        Self {
            server_url: url.trim_end_matches('/').to_owned(),
            client_id: CLIENT_ID.to_owned(),
            device_name: format!("Buwiz on {}", std::env::consts::OS),
            trust_keys: trust::pinned_keys(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Session {
    access_token: String,
    refresh_token: String,
    /// Unix seconds.
    access_expires_at: i64,
    email: Option<String>,
    device_id: Option<String>,
}

/// What the UI shows about sign-in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SignInStatus {
    SignedOut,
    /// Waiting for the person to approve `user_code` in a browser.
    Pending {
        user_code: String,
        verification_uri: String,
        verification_uri_complete: String,
        expires_at: i64,
    },
    SignedIn {
        email: Option<String>,
    },
}

/// `POST /auth/device/start`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: i64,
    pub interval: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PollOutcome {
    /// Keep polling every `interval` seconds.
    Pending {
        interval: i64,
    },
    SignedIn,
    Denied,
    Expired,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("Sign in to send returns.")]
    SignedOut,
    #[error("could not reach the Buwiz server: {0}")]
    Network(String),
    /// The server's sentence, with its code in brackets so the app can
    /// tell `tin_not_active` and the like apart.
    #[error("{message} ({code})")]
    Server {
        status: u16,
        code: String,
        message: String,
    },
    #[error("{0}")]
    Other(String),
    /// The server says this was not a try and nothing reached the BIR: a
    /// hold (`503` with `bir_offline`, `bir_busy`, `relay_paused` or
    /// `restarting`), or a fault on Buwiz's side (`class: "ours"`). Its
    /// text keeps the code in brackets, as [`ClientError::Server`]'s does.
    /// `wait_until` is the body's, or `Retry-After` from now.
    #[error("{message} ({code})")]
    NotATry {
        status: u16,
        code: String,
        message: String,
        wait_until: Option<DateTime<Utc>>,
    },
}

impl ClientError {
    /// The TIN refusal the server's code names, if any.
    pub fn tin_refusal(&self) -> Option<TinRefusal> {
        match self {
            ClientError::Server { code, .. } => TinRefusal::from_code(code),
            _ => None,
        }
    }

    /// Whether this error says nothing reached the BIR and no try was made:
    /// a hold or class `ours` ([`ClientError::NotATry`]), being signed out,
    /// or not reaching the Buwiz server at all.
    pub fn is_not_a_try(&self) -> bool {
        matches!(self, ClientError::NotATry { .. } | ClientError::SignedOut | ClientError::Network(_))
    }

    /// The server's `wait_until` on a hold.
    pub fn wait_until(&self) -> Option<DateTime<Utc>> {
        match self {
            ClientError::NotATry { wait_until, .. } => *wait_until,
            _ => None,
        }
    }
}

impl From<reqwest::Error> for ClientError {
    fn from(error: reqwest::Error) -> Self {
        Self::Network(error.to_string())
    }
}

impl From<crate::secrets::StoreError> for ClientError {
    fn from(error: crate::secrets::StoreError) -> Self {
        Self::Other(error.to_string())
    }
}

/// How a send ended when it did not end in a receipt.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SendError {
    /// Nothing reached BIR; sending again is safe.
    #[error("{0}")]
    NotSent(String),
    /// BIR may have the return. Never send it again automatically.
    #[error("{0}")]
    MaybeSent(String),
    /// Nothing reached BIR and it was not a try: Buwiz held the ticket, the
    /// relay failed on Buwiz's side (class `ours`), the app is signed out or
    /// could not reach Buwiz. Sending again is safe; the queue does not
    /// count it. Not before `wait_until` when the server gave one.
    #[error("{reason}")]
    NotATry {
        reason: String,
        wait_until: Option<DateTime<Utc>>,
    },
    /// Something touched the BIR's way in and it did not get through; sending
    /// again is safe. It counts as a try, and `blame` says where it failed:
    /// the whole queue backs off for a way in that is down, this return
    /// alone for the rest.
    #[error("{reason}")]
    Counted { reason: String, blame: Blame },
}

impl SendError {
    /// The TIN refusal a not-sent reason ends with
    /// ([`TinRefusal::from_reason`]). Never one for [`SendError::MaybeSent`]:
    /// BIR may have that return whatever its text says.
    pub fn refusal(&self) -> Option<TinRefusal> {
        match self {
            SendError::NotSent(reason) => TinRefusal::from_reason(reason),
            SendError::MaybeSent(_) | SendError::NotATry { .. } | SendError::Counted { .. } => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Sent {
    pub submission_id: String,
    pub receipt_jws: String,
    pub receipt: Receipt,
}

/// The parts of an eBIRForms file name, `{TIN}-{form}-{period}#{email}#.xml`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileName {
    pub tin: String,
    pub form: String,
    pub period: String,
    pub email: String,
}

impl FileName {
    pub fn parse(name: &str) -> Option<Self> {
        let stem = name.strip_suffix("#.xml")?;
        let (head, email) = stem.split_once('#')?;
        let mut parts = head.splitn(3, '-');
        let (tin, form, period) = (parts.next()?, parts.next()?, parts.next()?);
        let digits = matches!(tin.len(), 12 | 14) && tin.bytes().all(|byte| byte.is_ascii_digit());
        (digits && !form.is_empty() && !period.is_empty() && email.contains('@')).then(|| Self {
            tin: tin.to_owned(),
            form: form.to_owned(),
            period: period.to_owned(),
            email: email.to_owned(),
        })
    }
}

/// The longest the relay WebSocket may take to open.
const RELAY_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// One app's connection to buwiz-server. Cheap to share behind an `Arc`.
pub struct Submitter {
    config: SubmitConfig,
    http: reqwest::Client,
    store: Box<dyn SecretStore>,
    pending: Mutex<Option<(DeviceCode, i64)>>,
    /// Serializes token refreshes and device registration.
    session_lock: tokio::sync::Mutex<()>,
    /// The account's TIN list ([`crate::tins`]).
    pub(crate) tins: crate::tins::TinsCache,
    /// The [`CLIENT_HEADER`] value.
    client: RwLock<String>,
    /// The newest BIR sign heard on any answer (`buwiz-bir`, or a `bir`
    /// object in a body or a relay result).
    sign: SignCell,
}

/// The app name the header carries until the app says its own: `desktop`
/// for the desktop client id, else the client id itself, made header-safe.
fn default_client(client_id: &str) -> String {
    let app = if client_id == CLIENT_ID { "desktop" } else { client_id };
    format!("{}/{}", header_word(app, "app"), header_word(env!("CARGO_PKG_VERSION"), "0"))
}

/// A header-safe name or version: ASCII letters, digits and `._-+`, at most
/// 32 characters. Anything else (a space, a control character, a non-ASCII
/// letter) becomes `-`; a word with nothing left is `fallback`. Never an
/// error: the app is named whatever it was called.
fn header_word(word: &str, fallback: &str) -> String {
    let clean: String = word
        .trim()
        .chars()
        .take(32)
        .map(|c| if c.is_ascii_alphanumeric() || "._-+".contains(c) { c } else { '-' })
        .collect();
    if clean.chars().all(|c| c == '-') { fallback.to_owned() } else { clean }
}

impl Submitter {
    pub fn new(config: SubmitConfig, store: Box<dyn SecretStore>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent(concat!("buwiz-oss/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("an HTTP client");
        Self {
            http,
            store,
            pending: Mutex::new(None),
            session_lock: tokio::sync::Mutex::new(()),
            tins: crate::tins::TinsCache::default(),
            client: RwLock::new(default_client(&config.client_id)),
            sign: SignCell::default(),
            config,
        }
    }

    pub fn config(&self) -> &SubmitConfig {
        &self.config
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.config.server_url)
    }

    /// Name the app in the [`CLIENT_HEADER`] of every later request:
    /// `set_client("desktop", "0.2.0")`. A name or version with characters
    /// a header cannot carry is cleaned (each such character becomes `-`),
    /// never refused.
    pub fn set_client(&self, app: &str, version: &str) {
        if let Ok(mut client) = self.client.write() {
            *client = format!("{}/{}", header_word(app, "app"), header_word(version, "0"));
        }
    }

    /// The [`CLIENT_HEADER`] value requests carry now.
    pub fn client(&self) -> String {
        self.client
            .read()
            .map(|client| client.clone())
            .unwrap_or_else(|_| default_client(&self.config.client_id))
    }

    /// A request to the Buwiz server at `path`, with the [`CLIENT_HEADER`].
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, self.url(path))
            .header(CLIENT_HEADER, self.client())
    }

    /// The newest BIR sign this client has heard, with when (wall clock).
    /// `None` until a server sends one: every server today, and any older
    /// one, sends none, and nothing here acts on its absence.
    pub fn last_sign(&self) -> Option<SeenSign> {
        self.sign.latest()
    }

    /// Keep the sign of an answer's `buwiz-bir` header, if it has one.
    fn hear_header(&self, headers: &reqwest::header::HeaderMap) {
        if let Some(sign) = headers
            .get(SIGN_HEADER)
            .and_then(|value| value.to_str().ok())
            .and_then(Sign::from_header)
        {
            self.sign.observe(sign, Utc::now());
        }
    }

    /// Keep a full sign object (`bir` in a body or a relay result).
    pub(crate) fn hear_object(&self, value: &Value) {
        if let Some(sign) = Sign::from_json(value) {
            self.sign.observe(sign, Utc::now());
        }
    }

    fn session(&self) -> Result<Option<Session>, ClientError> {
        Ok(self
            .store
            .get(SESSION)?
            .and_then(|raw| serde_json::from_str(&raw).ok()))
    }

    fn save_session(&self, session: &Session) -> Result<(), ClientError> {
        Ok(self
            .store
            .set(SESSION, &serde_json::to_string(session).expect("json"))?)
    }

    // ------------------------------------------------------------ sign-in

    pub fn sign_in_status(&self) -> SignInStatus {
        if let Ok(Some(session)) = self.session() {
            return SignInStatus::SignedIn {
                email: session.email,
            };
        }
        match &*self.pending.lock().expect("pending") {
            Some((code, expires_at)) if *expires_at > device::now() => SignInStatus::Pending {
                user_code: code.user_code.clone(),
                verification_uri: code.verification_uri.clone(),
                verification_uri_complete: code.verification_uri_complete.clone(),
                expires_at: *expires_at,
            },
            _ => SignInStatus::SignedOut,
        }
    }

    /// Starts device-flow sign-in. Open `verification_uri_complete` in a
    /// browser, then call [`Self::poll_sign_in`] every `interval` seconds.
    pub async fn start_sign_in(&self) -> Result<DeviceCode, ClientError> {
        let response = self
            .request(reqwest::Method::POST, "/auth/device/start")
            .json(&json!({ "client_id": self.config.client_id }))
            .send()
            .await?;
        let code: DeviceCode = self.ok_json(response).await?;
        *self.pending.lock().expect("pending") =
            Some((code.clone(), device::now() + code.expires_in));
        Ok(code)
    }

    /// One poll of the sign-in [`Self::start_sign_in`] began. On approval
    /// the tokens are stored and the device key registered (or, on a server
    /// that can't register it yet, left for the first send).
    pub async fn poll_sign_in(&self) -> Result<PollOutcome, ClientError> {
        let Some((code, _)) = self.pending.lock().expect("pending").clone() else {
            return Ok(PollOutcome::Expired);
        };
        let response = self
            .request(reqwest::Method::POST, "/auth/device/poll")
            .json(&json!({ "client_id": self.config.client_id, "device_code": code.device_code }))
            .send()
            .await?;
        self.hear_header(response.headers());
        if !response.status().is_success() {
            let body: Value = response.json().await.unwrap_or_default();
            return Ok(match body["error"].as_str().unwrap_or_default() {
                "authorization_pending" => PollOutcome::Pending {
                    interval: code.interval,
                },
                "slow_down" => PollOutcome::Pending {
                    interval: code.interval + 5,
                },
                "access_denied" => {
                    *self.pending.lock().expect("pending") = None;
                    PollOutcome::Denied
                }
                _ => {
                    *self.pending.lock().expect("pending") = None;
                    PollOutcome::Expired
                }
            });
        }
        let tokens: Value = response.json().await?;
        self.store_tokens(&tokens, None)?;
        // A new sign-in, perhaps another login: no earlier TIN list holds.
        self.tins.clear();
        *self.pending.lock().expect("pending") = None;
        if let Ok(me) = self.authorized_get("/me").await {
            let mut session = self.session()?.ok_or(ClientError::SignedOut)?;
            session.email = me["email"].as_str().map(str::to_owned);
            self.save_session(&session)?;
        }
        // Register the device key now when the server can; if it can't yet
        // (a server without device keys), the first signed request does it.
        // Sign-in itself has succeeded either way.
        if let Err(error) = self.ensure_device().await {
            tracing::warn!("device key not registered at sign-in: {error}");
        }
        Ok(PollOutcome::SignedIn)
    }

    /// Ends this app's sign-in, on the server too when it can be reached.
    pub async fn sign_out(&self) -> Result<(), ClientError> {
        if let Ok(Some(session)) = self.session() {
            let _ = self
                .request(reqwest::Method::POST, "/api/auth/logout")
                .bearer_auth(&session.access_token)
                .send()
                .await;
        }
        self.forget_session()
    }

    fn forget_session(&self) -> Result<(), ClientError> {
        self.tins.clear();
        self.store.delete(SESSION)?;
        // A new sign-in registers a new key.
        self.store.delete(DEVICE_KEY)?;
        Ok(())
    }

    fn store_tokens(
        &self,
        tokens: &Value,
        previous: Option<&Session>,
    ) -> Result<Session, ClientError> {
        let (Some(access), Some(refresh)) = (
            tokens["access_token"].as_str(),
            tokens["refresh_token"].as_str(),
        ) else {
            return Err(ClientError::Other("the server sent no tokens".into()));
        };
        let session = Session {
            access_token: access.to_owned(),
            refresh_token: refresh.to_owned(),
            access_expires_at: device::now() + tokens["expires_in"].as_i64().unwrap_or(900),
            email: previous.and_then(|session| session.email.clone()),
            device_id: previous.and_then(|session| session.device_id.clone()),
        };
        self.save_session(&session)?;
        Ok(session)
    }

    /// A session with a fresh access token.
    async fn live_session(&self, force_refresh: bool) -> Result<Session, ClientError> {
        let _guard = self.session_lock.lock().await;
        let session = self.session()?.ok_or(ClientError::SignedOut)?;
        if !force_refresh && session.access_expires_at > device::now() + 30 {
            return Ok(session);
        }
        let response = self
            .request(reqwest::Method::POST, "/oauth/token")
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", self.config.client_id.as_str()),
                ("refresh_token", session.refresh_token.as_str()),
            ])
            .send()
            .await?;
        if response.status().is_client_error() {
            // The sign-in ended (expired, signed out, or revoked).
            self.forget_session()?;
            return Err(ClientError::SignedOut);
        }
        let tokens: Value = self.ok_json(response).await?;
        self.store_tokens(&tokens, Some(&session))
    }

    /// Who is signed in (the email, empty when unknown); `None` when no one
    /// is. Reads only the stored session.
    pub(crate) fn login(&self) -> Option<String> {
        let session = self.session().ok().flatten()?;
        Some(session.email.unwrap_or_default())
    }

    /// `GET path` with the bearer token alone and a 5 second limit, for
    /// [`crate::tins`]: once more with a refreshed token when the server says
    /// the token expired.
    pub(crate) async fn bearer_get(&self, path: &str) -> Result<Value, ClientError> {
        let mut refreshed = false;
        loop {
            let session = self.live_session(refreshed).await?;
            let response = self
                .request(reqwest::Method::GET, path)
                .bearer_auth(&session.access_token)
                .timeout(std::time::Duration::from_secs(5))
                .send()
                .await?;
            match self.ok_json(response).await {
                Err(ClientError::Server {
                    status: 401, code, ..
                }) if code == "invalid_token" && !refreshed => {
                    refreshed = true;
                }
                other => return other,
            }
        }
    }

    async fn authorized_get(&self, path: &str) -> Result<Value, ClientError> {
        let session = self.live_session(false).await?;
        let response = self
            .request(reqwest::Method::GET, path)
            .bearer_auth(&session.access_token)
            .send()
            .await?;
        self.ok_json(response).await
    }

    // -------------------------------------------------------- device key

    fn device_key(&self) -> Result<DeviceKey, ClientError> {
        if let Some(key) = self
            .store
            .get(DEVICE_KEY)?
            .as_deref()
            .and_then(DeviceKey::from_stored)
        {
            return Ok(key);
        }
        let key = DeviceKey::generate();
        self.store.set(DEVICE_KEY, &key.to_stored())?;
        Ok(key)
    }

    /// Registers the device key once per sign-in; answers its id.
    async fn ensure_device(&self) -> Result<String, ClientError> {
        let session = self.live_session(false).await?;
        if let Some(id) = &session.device_id {
            return Ok(id.clone());
        }
        let _guard = self.session_lock.lock().await;
        if let Some(id) = self.session()?.and_then(|session| session.device_id) {
            return Ok(id);
        }
        let key = self.device_key()?;
        let body = serde_json::to_vec(
            &json!({ "public_key": key.public_key(), "name": self.config.device_name }),
        )
        .expect("json");
        let path = "/api/devices";
        let mut request = self
            .request(reqwest::Method::POST, path)
            .bearer_auth(&session.access_token)
            .header("content-type", "application/json")
            .body(body.clone());
        for (name, value) in key.sign("POST", path, &body, device::now(), &device::nonce()) {
            request = request.header(name, value);
        }
        let registered: Value = self.ok_json(request.send().await?).await?;
        let id = registered["id"]
            .as_str()
            .ok_or_else(|| ClientError::Other("no device id".into()))?
            .to_owned();
        let mut session = self.session()?.ok_or(ClientError::SignedOut)?;
        session.device_id = Some(id.clone());
        self.save_session(&session)?;
        Ok(id)
    }

    /// A request signed with the device key. Retries once with a refreshed
    /// token when the access token has just expired.
    pub async fn signed(
        &self,
        method: &str,
        path: &str,
        body: &Value,
    ) -> Result<Value, ClientError> {
        let device_id = self.ensure_device().await?;
        let key = self.device_key()?;
        let body = serde_json::to_vec(body).expect("json");
        let mut refreshed = false;
        loop {
            let session = self.live_session(refreshed).await?;
            let method_value = reqwest::Method::from_bytes(method.as_bytes())
                .map_err(|e| ClientError::Other(e.to_string()))?;
            let mut request = self
                .request(method_value, path)
                .bearer_auth(&session.access_token)
                .header(HEADER_DEVICE, &device_id)
                .header("content-type", "application/json")
                .body(body.clone());
            for (name, value) in key.sign(method, path, &body, device::now(), &device::nonce()) {
                request = request.header(name, value);
            }
            match self.ok_json(request.send().await?).await {
                Err(ClientError::Server {
                    status: 401, code, ..
                }) if code == "invalid_token" && !refreshed => {
                    refreshed = true;
                }
                Err(ClientError::Server { code, .. }) if code == "device_revoked" => {
                    self.forget_session()?;
                    return Err(ClientError::Other(
                        "This device was removed from your Buwiz account. Sign in again to send returns.".into(),
                    ));
                }
                other => return other,
            }
        }
    }

    /// The server's relay trust list, checked against the pinned keys.
    /// Without a valid one there is no send.
    pub async fn trust_list(&self) -> Result<TrustList, ClientError> {
        let response = self
            .request(reqwest::Method::GET, "/.well-known/buwiz-trust.json")
            .send()
            .await?;
        self.hear_header(response.headers());
        if !response.status().is_success() {
            return Err(ClientError::Other(format!(
                "Can't send: the server has no signed trust list ({}).",
                response.status()
            )));
        }
        let text = response.text().await?;
        let jws = text.trim().trim_matches('"');
        trust::verify(&self.config.trust_keys, jws, device::now())
            .map_err(|error| ClientError::Other(format!("Can't send: {error}.")))
    }

    /// The Sender's sealing key: the one `/.well-known/buwiz-hpke.json`
    /// names as current, taken only if `list` carries it.
    pub async fn sealing_key(&self, list: &TrustList) -> Result<SealingKey, ClientError> {
        let document: Value = self
            .ok_json(
                self.request(reqwest::Method::GET, "/.well-known/buwiz-hpke.json")
                    .send()
                    .await?,
            )
            .await?;
        let offered: SealingKey = serde_json::from_value(document["keys"][0].clone())
            .map_err(|_| ClientError::Other("the server published no sealing key".into()))?;
        list.sealing_key(&offered)
            .map_err(|error| ClientError::Other(format!("Can't send: {error}.")))
    }

    /// Checks a receipt against the receipt keys of the signed trust list
    /// only; any other key is refused.
    pub async fn verify_receipt(
        &self,
        jws: &str,
        xml_sha256: &str,
        jti: Option<&str>,
    ) -> Result<Receipt, ClientError> {
        let list = self.trust_list().await?;
        list.verify_receipt(jws, xml_sha256, jti)
            .map_err(|error| ClientError::Other(error.to_string()))
    }

    /// Everything a send needs before the return itself: a live sign-in,
    /// a registered device, a valid trust list and a listed sealing key.
    /// A TIN refusal on the way (a banned account) makes the next TIN list
    /// read ask the server, as one on a send does.
    pub async fn ready(&self) -> Result<(), ClientError> {
        let ready = self.check_ready().await;
        if let Err(error) = &ready
            && error.tin_refusal().is_some()
        {
            self.tins.invalidate();
        }
        ready
    }

    async fn check_ready(&self) -> Result<(), ClientError> {
        let list = self.trust_list().await?;
        self.ensure_device().await?;
        self.sealing_key(&list).await?;
        Ok(())
    }

    // --------------------------------------------------------------- send

    /// Sends one return: ticket, sealed envelope, relay, verified receipt.
    /// A refusal for the TIN makes the next TIN list read ask the server.
    pub async fn send(&self, filename: &str, xml: &[u8]) -> Result<Sent, SendError> {
        let sent = self.send_return(filename, xml).await;
        if let Err(error) = &sent
            && error.refusal().is_some()
        {
            self.tins.invalidate();
        }
        sent
    }

    async fn send_return(&self, filename: &str, xml: &[u8]) -> Result<Sent, SendError> {
        let name = FileName::parse(filename)
            .ok_or_else(|| SendError::NotSent(format!("not an eBIRForms file name: {filename}")))?;
        let xml_sha256 = hex(&Sha256::digest(xml));
        let idempotency_key = hex(&Sha256::digest(
            format!("buwiz-send\0{filename}\0{xml_sha256}").as_bytes(),
        ))[..40]
            .to_owned();
        let not_sent = |error: ClientError| {
            if error.is_not_a_try() {
                SendError::NotATry { wait_until: error.wait_until(), reason: error.to_string() }
            } else {
                SendError::NotSent(error.to_string())
            }
        };
        // No valid trust list or listed sealing key, no ticket: nothing is
        // spent, and nothing sealed, for a send that can't be trusted.
        let list = self.trust_list().await.map_err(not_sent)?;
        let sealing_key = self.sealing_key(&list).await.map_err(not_sent)?;

        let ticket = match self
            .signed(
                "POST",
                "/api/tickets",
                &json!({
                    "tin": name.tin, "form": name.form, "period": name.period,
                    "payload_sha256": xml_sha256, "email": name.email, "idempotency_key": idempotency_key,
                }),
            )
            .await
        {
            Ok(ticket) => ticket,
            Err(ClientError::Server { code, message, .. }) if code == "already_submitted" => {
                return Err(SendError::MaybeSent(message));
            }
            Err(error) => return Err(not_sent(error)),
        };
        let (Some(jws), Some(jti)) = (ticket["ticket"].as_str(), ticket["jti"].as_str()) else {
            return Err(SendError::NotSent(
                "the server answered without a ticket".into(),
            ));
        };
        let envelope = seal::seal(&sealing_key, jti, xml)
            .map_err(|error| SendError::NotSent(error.to_string()))?;
        let submission = match self
            .signed(
                "POST",
                "/api/v2/submissions",
                &json!({ "ticket": jws, "envelope": envelope }),
            )
            .await
        {
            Ok(submission) => submission,
            Err(error) => return Err(not_sent(error)),
        };
        let (Some(id), Some(relay_path)) =
            (submission["id"].as_str(), submission["relay_path"].as_str())
        else {
            return Err(SendError::NotSent(
                "the server answered without a submission".into(),
            ));
        };

        let ended = self.relay_with(&list, relay_path).await.map_err(not_sent)?;
        let mut relay_class = None;
        let mut hold_until = None;
        let mut held = false;
        let touched;
        let (ending, status, error, receipt_jws) = match ended {
            Ended::Result {
                status,
                error,
                receipt,
                class,
                hold,
                bir,
                opened,
                path_down,
            } => {
                if let Some(bir) = &bir {
                    self.hear_object(bir);
                }
                relay_class = class.as_deref().map(FailureClass::from_wire);
                hold_until = hold.as_ref().and_then(|hold| hold.wait_until);
                held = hold.is_some();
                touched = Touched { opened, path_down, dropped: false };
                (Ending::Result, status, error, receipt)
            }
            Ended::Dropped { opened, path_down } => {
                touched = Touched { opened, path_down, dropped: true };
                let ending = if opened {
                    Ending::DroppedAfterOpening
                } else {
                    Ending::DroppedBeforeOpening
                };
                match self.authorized_get(&format!("/api/submissions/{id}")).await {
                    Ok(view) => (
                        ending,
                        view["status"].as_str().unwrap_or("unknown").to_owned(),
                        view["error"].as_str().map(str::to_owned),
                        view["receipt_jws"].as_str().map(str::to_owned),
                    ),
                    Err(_) if !opened => (
                        ending,
                        "failed".to_owned(),
                        Some("the connection ended before sending".into()),
                        None,
                    ),
                    Err(_) => (
                        ending,
                        "unknown".to_owned(),
                        Some("the connection ended during the send".into()),
                        None,
                    ),
                }
            }
        };
        let (receipt_jws, receipt) = outcome(ending, &status, error, receipt_jws, |receipt_jws| {
            list.verify_receipt(receipt_jws, &xml_sha256, Some(jti))
        })
        .map_err(|failure| relay_failure(failure, relay_class, held, hold_until, touched))?;
        Ok(Sent { submission_id: id.to_owned(), receipt_jws, receipt })
    }

    /// Opens the relay WebSocket for a submission and forwards, only to the
    /// hosts of the verified trust list, until the server's result.
    pub async fn relay(&self, relay_path: &str) -> Result<Ended, ClientError> {
        let list = self.trust_list().await?;
        self.relay_with(&list, relay_path).await
    }

    async fn relay_with(&self, list: &TrustList, relay_path: &str) -> Result<Ended, ClientError> {
        let device_id = self.ensure_device().await?;
        let key = self.device_key()?;
        let session = self.live_session(false).await?;
        let url = self.url(relay_path).replacen("http", "ws", 1);
        let mut request = url
            .into_client_request()
            .map_err(|error| ClientError::Other(error.to_string()))?;
        let headers = request.headers_mut();
        let mut put = |name: &'static str, value: String| -> Result<(), ClientError> {
            let value = value
                .parse()
                .map_err(|_| ClientError::Other(format!("bad {name} header")))?;
            headers.insert(name, value);
            Ok(())
        };
        put("authorization", format!("Bearer {}", session.access_token))?;
        put(CLIENT_HEADER, self.client())?;
        put(HEADER_DEVICE, device_id)?;
        for (name, value) in key.sign("GET", relay_path, b"", device::now(), &device::nonce()) {
            put(name, value)?;
        }
        let (socket, _) = tokio::time::timeout(
            RELAY_CONNECT_TIMEOUT,
            tokio_tungstenite::connect_async(request),
        )
        .await
        .map_err(|_| ClientError::Network("the relay did not answer in time".into()))?
        .map_err(|error| match error {
                tokio_tungstenite::tungstenite::Error::Http(response) => ClientError::Server {
                    status: response.status().as_u16(),
                    code: "relay_refused".into(),
                    message: format!("the server refused the relay ({})", response.status()),
                },
                other => ClientError::Network(other.to_string()),
            })?;
        Ok(pipe::serve(socket, list).await)
    }
}

/// How the relay ended, for [`outcome`].
#[derive(Clone, Copy, Debug, PartialEq)]
enum Ending {
    /// The server delivered its `result` over the socket.
    Result,
    /// The socket closed before any connection toward BIR was made; the
    /// status was read back from the server.
    DroppedBeforeOpening,
    /// The socket closed after the relay had opened a connection toward
    /// BIR; the status was read back from the server.
    DroppedAfterOpening,
}

/// What a send's final status means. `verify` checks a receipt and runs
/// only for `sent` or `dry_run` with one. Once the relay had opened toward
/// BIR and dropped, a status read back at that moment may not record the
/// upload yet, so only an explicit `failed` is not sent.
fn outcome<R, E: std::fmt::Display>(
    ending: Ending,
    status: &str,
    error: Option<String>,
    receipt_jws: Option<String>,
    verify: impl FnOnce(&str) -> Result<R, E>,
) -> Result<(String, R), SendError> {
    match (status, receipt_jws) {
        ("sent" | "dry_run", Some(receipt_jws)) => match verify(&receipt_jws) {
            Ok(receipt) => Ok((receipt_jws, receipt)),
            Err(error) => Err(SendError::MaybeSent(format!(
                "the server reports the return sent, but its receipt does not check out ({error}); check for BIR's confirmation email"
            ))),
        },
        ("expired" | "ready", _) if ending == Ending::DroppedAfterOpening => Err(SendError::MaybeSent(format!(
            "the connection ended after the relay opened toward BIR, and the server still reports the return {status}; BIR may have received this return; check for its confirmation email before sending again"
        ))),
        ("failed" | "expired" | "ready", _) => {
            Err(SendError::NotSent(error.unwrap_or_else(|| "the server did not send the return".into())))
        }
        _ => Err(SendError::MaybeSent(error.unwrap_or_else(|| {
            "BIR may have received this return; check for its confirmation email before sending again".into()
        }))),
    }
}

/// What the app saw of its own connections toward the BIR during a send.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Touched {
    /// Some connection opened: the BIR was reached, whatever the class says.
    opened: bool,
    /// Every connection the app attempted failed on the way in.
    path_down: bool,
    /// The relay's socket ended without a result.
    dropped: bool,
}

/// The BIR's front desk (the dispatcher) failed to answer: the try never got
/// past it.
fn front_desk_failed(reason: &str) -> bool {
    let reason = reason.to_ascii_lowercase();
    ["dispatcher did not answer", "dispatcher request failed", "dispatcher answered"]
        .iter()
        .any(|text| reason.contains(text))
}

/// The BIR has no upload service for this TIN (the dispatcher's mode 0).
pub fn no_service_reason(reason: &str) -> bool {
    reason.to_ascii_lowercase().contains("no upload server for this tin")
}

/// A refused login after the upload stream opened.
fn login_refused(reason: &str) -> bool {
    let reason = reason.to_ascii_lowercase();
    ["login", "authentication", "permission denied", "not authorized"]
        .iter()
        .any(|text| reason.contains(text))
}

/// What a relay that ended without a receipt means for the queue.
///
/// - A maybe-sent outcome stays maybe sent, whatever else is said.
/// - A refusal code (`tin_*`, `account_banned`) or `return_mismatch` always
///   wins: not sent, final, whatever class the relay gave.
/// - A hold, or class `ours` when NO stream opened, is not a try: nothing
///   touched the BIR. If any stream opened it is a counted failure whatever
///   its class, and nothing decides that from `class` or `bir`.
/// - A counted failure is blamed on the way in (every open the app attempted
///   was refused, or the front desk failed) or on the return (no upload
///   service, a refused login), else on the upload.
fn relay_failure(
    failure: SendError,
    class: Option<FailureClass>,
    held: bool,
    wait_until: Option<DateTime<Utc>>,
    touched: Touched,
) -> SendError {
    let SendError::NotSent(reason) = failure else {
        return failure;
    };
    if is_final_reason(&reason) {
        return SendError::NotSent(reason);
    }
    let wait_until = wait_until.map(|until| capped_wait_until(until, Utc::now()));
    if held {
        return SendError::NotATry { reason, wait_until };
    }
    if no_service_reason(&reason) {
        return SendError::Counted { reason, blame: Blame::NoService };
    }
    let blame = if touched.path_down || (!touched.opened && front_desk_failed(&reason)) {
        Blame::Path
    } else if !touched.opened && touched.dropped {
        // The pipe to Buwiz broke before any connection toward the BIR
        // opened: nothing touched the BIR.
        return SendError::NotATry { reason, wait_until };
    } else if touched.opened && login_refused(&reason) {
        Blame::Return
    } else if touched.opened || class != Some(FailureClass::Ours) {
        Blame::Upload
    } else {
        return SendError::NotATry { reason, wait_until };
    };
    SendError::Counted { reason, blame }
}

impl Submitter {
    /// The JSON body of a 2xx answer, or the server's `{"error":{…}}`.
    /// Every answer's sign is kept ([`Self::last_sign`]), and so is the
    /// `bir` object of an error body.
    async fn ok_json<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<T, ClientError> {
        self.hear_header(response.headers());
        let status = response.status();
        if status.is_success() {
            return response
                .json()
                .await
                .map_err(|error| ClientError::Other(format!("unreadable answer: {error}")));
        }
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<i64>().ok());
        let body: Value = response.json().await.unwrap_or_default();
        if let Some(bir) = body.get("bir") {
            self.hear_object(bir);
        }
        Err(server_error(status.as_u16(), retry_after, &body, Utc::now()))
    }
}

/// The error a non-2xx answer with `body` stands for. A hold code or class
/// `ours` is [`ClientError::NotATry`], with the body's `wait_until`, else
/// `Retry-After` (the header, then the body's `retry_after`) from `now`.
fn server_error(status: u16, retry_after: Option<i64>, body: &Value, now: DateTime<Utc>) -> ClientError {
    let (code, message) = error_code_and_message(status, body);
    let error = &body["error"];
    let class = error.get("class").or_else(|| body.get("class")).and_then(Value::as_str);
    let ours = class.map(FailureClass::from_wire) == Some(FailureClass::Ours);
    // A refusal code always wins over class: a TIN refusal or a mismatch is
    // final, whoever says it failed.
    let final_code = TinRefusal::from_code(&code).is_some() || code == bir_core::smart_queue::RETURN_MISMATCH;
    if final_code || !(is_hold_code(&code) || is_limit_code(&code) || ours) {
        return ClientError::Server { status, code, message };
    }
    let wait_until = error
        .get("wait_until")
        .and_then(Value::as_str)
        .and_then(bir_core::smart_queue::parse_time)
        .or_else(|| {
            retry_after
                .or_else(|| error.get("retry_after").and_then(Value::as_i64))
                .filter(|seconds| *seconds >= 0)
                .map(|seconds| now + chrono::Duration::seconds(seconds.min(bir_core::smart_queue::MAX_SERVER_WAIT_S)))
        })
        .map(|until| capped_wait_until(until, now));
    ClientError::NotATry { status, code, message, wait_until }
}

fn error_code_and_message(status: u16, body: &Value) -> (String, String) {
    match &body["error"] {
        Value::Object(error) => (
            error
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("error")
                .to_owned(),
            error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("the server refused")
                .to_owned(),
        ),
        Value::String(code) => (
            code.clone(),
            body["error_description"]
                .as_str()
                .unwrap_or(code)
                .to_owned(),
        ),
        _ => ("error".to_owned(), format!("the server answered {status}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// macOS copies v0.1.0's secrets to the new keychain service by these
    /// names (bir_core::id_migration): every secret stored here is listed.
    #[test]
    fn the_migration_knows_every_secret_name() {
        assert_eq!(bir_core::app_id::SUBMIT_SECRET_NAMES, [SESSION, DEVICE_KEY]);
    }

    #[test]
    fn file_names_split_into_ticket_fields() {
        let name =
            FileName::parse("00012345300000-1701Qv2018-2026Q1#juana@example.com#.xml").unwrap();
        assert_eq!(name.tin, "00012345300000");
        assert_eq!(
            (name.form.as_str(), name.period.as_str()),
            ("1701Qv2018", "2026Q1")
        );
        assert_eq!(name.email, "juana@example.com");
        assert_eq!(
            FileName::parse("00012345300000-1701Qv2018-2026Q1.xml"),
            None
        );
        assert_eq!(FileName::parse("0001-1701Qv2018-2026Q1#a@b.c#.xml"), None);
    }

    #[test]
    fn the_server_url_comes_from_the_environment_then_the_setting() {
        // SAFETY: this test alone touches the variable.
        unsafe { std::env::remove_var("BUWIZ_API_URL") };
        assert_eq!(SubmitConfig::new(None).server_url, DEFAULT_SERVER_URL);
        assert_eq!(
            SubmitConfig::new(Some("http://localhost:3008/")).server_url,
            "http://localhost:3008"
        );
        unsafe { std::env::set_var("BUWIZ_API_URL", "http://127.0.0.1:9") };
        assert_eq!(
            SubmitConfig::new(Some("http://localhost:3008")).server_url,
            "http://127.0.0.1:9"
        );
        unsafe { std::env::set_var("BUWIZ_API_URL", "http://buwiz.example") };
        assert_eq!(SubmitConfig::new(None).server_url, DEFAULT_SERVER_URL);
        unsafe { std::env::remove_var("BUWIZ_API_URL") };
        assert_eq!(
            SubmitConfig::new(Some("http://localhost.example:3008")).server_url,
            DEFAULT_SERVER_URL
        );
        assert_eq!(
            SubmitConfig::new(Some("http://[::1]:3008")).server_url,
            "http://[::1]:3008"
        );
        assert_eq!(
            SubmitConfig::new(Some("https://lab.example/")).server_url,
            "https://lab.example"
        );
    }

    /// What `outcome` answers, as a word: sent, not sent or maybe sent.
    fn decided(ending: Ending, status: &str, receipt: Option<&str>) -> &'static str {
        let verify = |jws: &str| if jws == "good" { Ok(()) } else { Err("bad signature") };
        match outcome(ending, status, None, receipt.map(str::to_owned), verify) {
            Ok(_) => "sent",
            Err(SendError::NotSent(_)) => "not sent",
            Err(SendError::MaybeSent(_)) => "maybe sent",
            Err(SendError::NotATry { .. }) => "not a try",
            Err(SendError::Counted { .. }) => "counted",
        }
    }

    /// RELAY-PROTOCOL.md §6: only `failed` says nothing reached BIR. Once
    /// the relay had opened toward BIR and dropped, a `ready` or `expired`
    /// read back may only mean the server has not recorded the upload yet.
    #[test]
    fn every_final_status_maps_to_one_outcome() {
        use Ending::{DroppedAfterOpening as Opened, DroppedBeforeOpening as Before, Result as Res};
        let rows: [(&str, Option<&str>, [&str; 3]); 9] = [
            ("sent", Some("good"), ["sent", "sent", "sent"]),
            ("dry_run", Some("good"), ["sent", "sent", "sent"]),
            ("sent", Some("forged"), ["maybe sent", "maybe sent", "maybe sent"]),
            ("sent", None, ["maybe sent", "maybe sent", "maybe sent"]),
            ("failed", None, ["not sent", "not sent", "not sent"]),
            ("expired", None, ["not sent", "not sent", "maybe sent"]),
            ("ready", None, ["not sent", "not sent", "maybe sent"]),
            ("unknown", None, ["maybe sent", "maybe sent", "maybe sent"]),
            ("uploading", None, ["maybe sent", "maybe sent", "maybe sent"]),
        ];
        for (status, receipt, want) in rows {
            for (ending, want) in [Res, Before, Opened].into_iter().zip(want) {
                assert_eq!(decided(ending, status, receipt), want, "{ending:?}, {status}, receipt {receipt:?}");
            }
        }
    }

    /// After the relay opened and dropped, a `ready` or `expired` tells the
    /// person BIR may have the return, not the server's own error.
    #[test]
    fn a_dropped_relay_that_opened_says_bir_may_have_the_return() {
        for status in ["ready", "expired"] {
            let answer = outcome(
                Ending::DroppedAfterOpening,
                status,
                Some("the ticket expired".into()),
                None,
                |_: &str| Ok::<(), String>(()),
            );
            let Err(SendError::MaybeSent(message)) = answer else {
                panic!("{status}: {answer:?}");
            };
            assert!(message.contains("BIR may have received this return"), "{message}");
            assert!(message.contains("confirmation email"), "{message}");
        }
    }

    /// The server's error and the verifier's text pass through as before.
    #[test]
    fn outcome_messages_are_unchanged_where_the_outcome_is() {
        let message = |answer: Result<(String, ()), SendError>| match answer {
            Err(SendError::NotSent(message) | SendError::MaybeSent(message)) => message,
            Err(SendError::NotATry { reason, .. } | SendError::Counted { reason, .. }) => reason,
            Ok(_) => panic!("sent"),
        };
        let ok = |_: &str| Ok::<(), String>(());
        for ending in [Ending::Result, Ending::DroppedBeforeOpening, Ending::DroppedAfterOpening] {
            assert_eq!(message(outcome(ending, "failed", Some("no ticket".into()), None, ok)), "no ticket");
            assert_eq!(
                message(outcome(ending, "failed", None, None, ok)),
                "the server did not send the return"
            );
            assert_eq!(
                message(outcome(ending, "unknown", Some("the connection ended during the send".into()), None, ok)),
                "the connection ended during the send"
            );
            assert_eq!(
                message(outcome(ending, "unknown", None, None, ok)),
                "BIR may have received this return; check for its confirmation email before sending again"
            );
            assert_eq!(
                message(outcome(ending, "sent", None, Some("x".into()), |_: &str| Err::<(), _>("bad signature"))),
                "the server reports the return sent, but its receipt does not check out (bad signature); check for BIR's confirmation email"
            );
        }
        for ending in [Ending::Result, Ending::DroppedBeforeOpening] {
            assert_eq!(message(outcome(ending, "expired", Some("the ticket expired".into()), None, ok)), "the ticket expired");
            assert_eq!(message(outcome(ending, "ready", None, None, ok)), "the server did not send the return");
        }
    }

    const TIN_CODES: [&str; 6] = [
        "tin_not_active",
        "tin_unverified",
        "tin_pending",
        "tin_needs_fix",
        "tin_paused",
        "account_banned",
    ];

    /// A ticket refusal reaches the worker as `ClientError::Server`'s text,
    /// which keeps the code for the one decoder.
    #[test]
    fn a_ticket_refusal_keeps_its_code_in_the_text() {
        for code in TIN_CODES {
            let error = ClientError::Server {
                status: 403,
                code: code.into(),
                message: "Some sentence (with brackets) the server chose.".into(),
            };
            let text = error.to_string();
            assert!(text.ends_with(&format!("({code})")), "{text}");
            let refusal = TinRefusal::from_reason(&text).expect(code);
            assert_eq!(refusal.code(), code);
            assert_eq!(error.tin_refusal(), Some(refusal));
            let not_sent = SendError::NotSent(format!("Not sent (nothing reached BIR): {text}"));
            assert_eq!(not_sent.refusal(), Some(refusal));
        }
        let other = ClientError::Server {
            status: 409,
            code: "already_submitted".into(),
            message: "x".into(),
        };
        assert_eq!(other.tin_refusal(), None);
        assert_eq!(
            ClientError::Network("tin_not_active".into()).tin_refusal(),
            None
        );
    }

    /// The relay may end `failed` with a TIN code when the TIN stopped
    /// being sendable after the ticket: nothing reached BIR, the same
    /// refusal. A maybe-sent text with a code is never one.
    #[test]
    fn a_relay_failed_with_a_tin_code_is_a_refusal() {
        let ok = |_: &str| Ok::<(), String>(());
        for ending in [
            Ending::Result,
            Ending::DroppedBeforeOpening,
            Ending::DroppedAfterOpening,
        ] {
            let error = Some(
                "Sending is paused for this TIN. Write to support@buwiz.com. (tin_paused)"
                    .to_owned(),
            );
            let Err(failure) = outcome(ending, "failed", error, None, ok) else {
                panic!("a failed relay is not sent");
            };
            assert!(matches!(failure, SendError::NotSent(_)), "{failure:?}");
            assert_eq!(failure.refusal(), Some(TinRefusal::Paused));
        }
        let maybe =
            SendError::MaybeSent("the connection ended during the send (tin_paused)".into());
        assert_eq!(maybe.refusal(), None);
        assert_eq!(
            SendError::NotSent("the relay closed early (relay_closed)".into()).refusal(),
            None
        );
    }

    /// A good receipt comes back with its JWS; `verify` runs only for one.
    #[test]
    fn outcome_verifies_only_a_sent_receipt() {
        let answer = outcome(Ending::Result, "dry_run", None, Some("jws".into()), |jws: &str| Ok::<_, String>(jws.len()));
        assert_eq!(answer.unwrap(), ("jws".to_owned(), 3));
        let answer = outcome(Ending::Result, "failed", None, Some("jws".into()), |_: &str| -> Result<(), String> {
            panic!("verified a failed send's receipt")
        });
        assert!(matches!(answer, Err(SendError::NotSent(_))));
    }

    /// A server without device keys (no `/api/devices`, as before v2) still
    /// signs the app in; the key registers before the first send instead.
    #[tokio::test]
    async fn sign_in_succeeds_when_the_server_cannot_register_devices() {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut paths = Vec::new();
            for stream in listener.incoming().take(4) {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap_or_default().to_owned();
                let mut length = 0;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header.trim().is_empty() {
                        break;
                    }
                    if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let (status, json) = match path.as_str() {
                    "/auth/device/start" => (
                        "200 OK",
                        r#"{"device_code":"d","user_code":"ABCD-EFGH","verification_uri":"http://x/device","verification_uri_complete":"http://x/device?code=ABCD-EFGH","expires_in":600,"interval":1}"#,
                    ),
                    "/auth/device/poll" => ("200 OK", r#"{"access_token":"a","refresh_token":"r","expires_in":3600}"#),
                    "/me" => ("200 OK", r#"{"email":"juana@example.com"}"#),
                    _ => ("404 Not Found", r#"{"error":{"code":"not_found","message":"no such route"}}"#),
                };
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{json}",
                    json.len()
                )
                .unwrap();
                paths.push(path);
            }
            paths
        });
        let dir = tempfile::tempdir().unwrap();
        let config = SubmitConfig {
            server_url: format!("http://{addr}"),
            client_id: CLIENT_ID.into(),
            device_name: "test".into(),
            trust_keys: Vec::new(),
        };
        let submitter = Submitter::new(config, Box::new(crate::secrets::FileStore::new(dir.path().to_owned())));
        submitter.start_sign_in().await.unwrap();
        assert_eq!(submitter.poll_sign_in().await.unwrap(), PollOutcome::SignedIn);
        assert_eq!(
            submitter.sign_in_status(),
            SignInStatus::SignedIn { email: Some("juana@example.com".into()) }
        );
        assert_eq!(server.join().unwrap(), ["/auth/device/start", "/auth/device/poll", "/me", "/api/devices"]);
    }

    // ----------------------------------------------- the version header

    #[tokio::test]
    async fn every_request_names_the_app_and_its_version() {
        use crate::testkit::{Answer, serve, submitter};
        let (url, seen) = serve(|request| match request.path.as_str() {
            "/auth/device/start" => Answer::json(
                200,
                &json!({"device_code":"d","user_code":"ABCD-EFGH","verification_uri":"http://x/device","verification_uri_complete":"http://x/device?code=ABCD-EFGH","expires_in":600,"interval":1}),
            ),
            _ => Answer::json(404, &json!({"error": {"code": "not_found", "message": "no such route"}})),
        });
        let client = submitter(&url, true, Vec::new());
        let desktop = format!("desktop/{}", env!("CARGO_PKG_VERSION"));
        assert_eq!(client.client(), desktop);
        client.start_sign_in().await.unwrap();
        let _ = client.trust_list().await;
        let _ = client.bearer_get("/api/tax-profiles").await;
        client.set_client("mini", "0.5.0");
        let _ = client.signed("POST", "/api/tickets", &json!({})).await;
        client.set_client("mini app", "0.5.0");
        assert_eq!(client.client(), "mini-app/0.5.0", "a space is cleaned, not refused");
        client.set_client("mini", "0.5.0\r\nx-evil: 1");
        assert_eq!(client.client(), "mini/0.5.0--x-evil--1", "no control character reaches a header");
        client.set_client("", "");
        assert_eq!(client.client(), "app/0");
        client.set_client("mini", "0.5.0");
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.iter().map(|request| request.method.as_str()).collect::<Vec<_>>(), ["POST", "GET", "GET", "POST"]);
        let labels: Vec<(&str, Option<&str>)> =
            seen.iter().map(|request| (request.path.as_str(), request.header(CLIENT_HEADER))).collect();
        assert_eq!(
            labels,
            [
                ("/auth/device/start", Some(desktop.as_str())),
                ("/.well-known/buwiz-trust.json", Some(desktop.as_str())),
                ("/api/tax-profiles", Some(desktop.as_str())),
                ("/api/tickets", Some("mini/0.5.0")),
            ]
        );
        let other = Submitter::new(
            SubmitConfig { client_id: "buwiz-direct".into(), ..SubmitConfig::new(None) },
            Box::new(crate::secrets::MemoryStore::default()),
        );
        assert_eq!(other.client(), format!("buwiz-direct/{}", env!("CARGO_PKG_VERSION")));
    }

    // ------------------------------------------------------- the sign

    #[tokio::test]
    async fn a_sign_on_any_answer_is_kept_and_the_highest_seq_wins() {
        use crate::testkit::{Answer, fixture, serve, submitter};
        use bir_core::bir_sign::SignState;
        let header = fixture("contract-compact-sign-header.txt").trim().to_owned();
        let (url, _) = serve(move |request| match request.path.as_str() {
            "/.well-known/buwiz-trust.json" => Answer::json(404, &json!({})).with("buwiz-bir", header.clone()),
            "/.well-known/buwiz-hpke.json" => Answer::json(200, &json!({"keys": []}))
                .with("buwiz-bir", "v=1, seq=4000, state=online"),
            "/api/tax-profiles" => Answer::json(200, &json!({"profiles": []}))
                .with("buwiz-bir", "v=1, seq=4190, state=online, upload=online"),
            _ => Answer::json(500, &json!({"error": {"code": "internal", "message": "x"}}))
                .with("buwiz-bir", "v=9, seq=99999, state=offline"),
        });
        let client = submitter(&url, true, Vec::new());
        assert_eq!(client.last_sign(), None, "no sign until a server sends one");
        let _ = client.trust_list().await;
        let first = client.last_sign().expect("the trust list's header");
        assert_eq!((first.sign.seq, first.sign.state), (4182, SignState::Offline));
        let (public, jws, _) = crate::testkit::trust_documents(&["127.0.0.1:9"]);
        let list = trust::verify(&[public], &jws, device::now()).unwrap();
        let _ = client.sealing_key(&list).await;
        assert_eq!(client.last_sign().unwrap().sign.seq, 4182, "an older, cached sign is ignored");
        client.bearer_get("/api/tax-profiles").await.unwrap();
        assert_eq!(client.last_sign().unwrap().sign.state, SignState::Online);
        let _ = client.authorized_get("/me").await;
        assert_eq!(client.last_sign().unwrap().sign.seq, 4190, "a v=9 sign is not read");
    }

    // ------------------------------------------------------- the holds

    fn hold(status: u16, retry_after: Option<i64>, fixture: &str) -> ClientError {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-11T09:00:00Z").unwrap().with_timezone(&Utc);
        server_error(status, retry_after, &crate::testkit::fixture_json(fixture), now)
    }

    /// The public contract (`docs/bir-uptime.md` in `buwiz-com/buwiz-com`): every hold is not a try, keeps its code in its
    /// text, and carries `wait_until` (the body's, else `Retry-After`).
    #[test]
    fn the_holds_of_api_5_2_and_10_are_not_tries() {
        let at = |text: &str| Some(bir_core::smart_queue::parse_time(text).unwrap());
        let offline = hold(503, Some(841), "contract-hold-bir-offline.json");
        let ClientError::NotATry { status, code, wait_until, .. } = &offline else { panic!("{offline:?}") };
        assert_eq!((*status, code.as_str()), (503, "bir_offline"));
        assert_eq!(*wait_until, at("2026-11-11T09:13:41Z"), "the body's exact time");
        assert!(offline.to_string().ends_with("(bir_offline)"));
        assert!(offline.is_not_a_try());
        assert_eq!(offline.tin_refusal(), None);

        let busy = hold(503, Some(600), "contract-hold-bir-busy.json");
        assert_eq!(busy.wait_until(), at("2026-11-11T09:10:00Z"), "Retry-After from now");
        assert!(busy.is_not_a_try());

        let paused = hold(503, None, "contract-error-relay-paused.json");
        assert!(matches!(&paused, ClientError::NotATry { code, wait_until: None, .. } if code == "relay_paused"));

        let restarting = server_error(
            503,
            Some(30),
            &json!({"error": {"code": "restarting", "message": "Buwiz is restarting."}}),
            Utc::now(),
        );
        assert!(restarting.is_not_a_try());
        assert!(restarting.wait_until().is_some());

        let internal = hold(500, None, "contract-error-internal-ours.json");
        assert!(matches!(&internal, ClientError::NotATry { code, .. } if code == "internal"), "class ours");

        // Everything else stays a server error, as before.
        for (status, body) in [
            (403, json!({"error": {"code": "tin_not_active", "message": "x"}})),
            (503, json!({"error": {"code": "a_code_from_the_future", "message": "x"}})),
            (500, json!({"error": {"code": "internal", "class": "bir", "message": "x"}})),
            (400, json!({"error": "invalid_grant", "error_description": "x"})),
        ] {
            let error = server_error(status, Some(10), &body, Utc::now());
            assert!(matches!(error, ClientError::Server { .. }), "{body}");
            assert!(!error.is_not_a_try());
        }
        assert!(ClientError::SignedOut.is_not_a_try());
        assert!(ClientError::Network("refused".into()).is_not_a_try());
        assert!(!ClientError::Other("Can't send: no list.".into()).is_not_a_try());
        // A Retry-After that makes no sense is no time.
        let silly = server_error(503, Some(-5), &json!({"error": {"code": "bir_busy"}}), Utc::now());
        assert_eq!(silly.wait_until(), None);
    }

    /// A relay refusal carrying class `ours` still goes by its code: a TIN
    /// refusal or a mismatch is final, and 429 limits are not tries but
    /// honour `Retry-After` (up to 7 days).
    #[test]
    fn a_refusal_code_wins_over_class_ours_and_a_429_is_not_a_try() {
        for code in ["tin_not_active", "tin_needs_fix", "account_banned", "return_mismatch"] {
            let body = json!({"error": {"code": code, "class": "ours", "message": "x"}});
            for status in [403, 409, 500, 503] {
                let error = server_error(status, Some(60), &body, Utc::now());
                assert!(matches!(error, ClientError::Server { .. }), "{code} {status}: {error:?}");
            }
        }
        let now = Utc::now();
        for code in ["rate_limited", "quota_exceeded", "try_anyway_limited"] {
            let body = json!({"error": {"code": code, "message": "x"}});
            let error = server_error(429, Some(600), &body, now);
            assert!(error.is_not_a_try(), "{code}");
            assert_eq!(error.wait_until(), Some(now + chrono::Duration::seconds(600)));
        }
        let body = json!({"error": {"code": "rate_limited", "message": "x"}});
        let year = server_error(429, Some(365 * 86_400), &body, now);
        assert_eq!(year.wait_until(), Some(now + chrono::Duration::days(7)), "beyond 7 days reads as 7 days");
        let far = json!({"error": {"code": "bir_busy", "wait_until": "2099-01-01T00:00:00Z", "message": "x"}});
        assert_eq!(server_error(503, None, &far, now).wait_until(), Some(now + chrono::Duration::days(7)));
    }

    /// Nothing decides a resend from `class`: a result is not a try only
    /// when no stream opened.
    #[test]
    fn a_relay_failure_is_not_a_try_only_when_nothing_opened() {
        let wait = Some(Utc::now());
        let failed = |text: &str| SendError::NotSent(text.into());
        let none = Touched::default();
        let opened = Touched { opened: true, ..none };
        let down = Touched { path_down: true, ..none };
        let dropped = Touched { dropped: true, ..none };
        let ours = Some(FailureClass::Ours);

        // Class ours, nothing opened: not a try. A hold is, whatever opened.
        assert_eq!(
            relay_failure(failed("held"), ours, false, wait, none),
            SendError::NotATry { reason: "held".into(), wait_until: wait }
        );
        assert!(matches!(
            relay_failure(failed("held"), None, true, wait, opened),
            SendError::NotATry { .. }
        ));
        // Class ours, a stream opened: a counted failure.
        assert_eq!(
            relay_failure(failed("write failed"), ours, false, wait, opened),
            SendError::Counted { reason: "write failed".into(), blame: Blame::Upload }
        );
        // Other classes, nothing opened and nothing known: the old reading.
        for class in [None, Some(FailureClass::Bir), Some(FailureClass::UserNetwork)] {
            assert_eq!(
                relay_failure(failed("x"), class, false, None, none),
                SendError::Counted { reason: "x".into(), blame: Blame::Upload }
            );
        }
        // The way in down, whatever the class says: the path.
        for class in [ours, None] {
            assert_eq!(
                relay_failure(failed("could not reach BIR"), class, false, None, down),
                SendError::Counted { reason: "could not reach BIR".into(), blame: Blame::Path }
            );
        }
        let desk = "BIR's dispatcher did not answer: tls://x: dispatcher request failed";
        assert!(matches!(
            relay_failure(failed(desk), None, false, None, none),
            SendError::Counted { blame: Blame::Path, .. }
        ));
        // A refused login after the stream opened is the return's own.
        assert!(matches!(
            relay_failure(failed("the login was refused"), None, false, None, opened),
            SendError::Counted { blame: Blame::Return, .. }
        ));
        // No upload service for the TIN.
        let none_for_tin = "dispatcher has no upload server for this TIN (mode 0)";
        assert!(matches!(
            relay_failure(failed(none_for_tin), None, false, None, none),
            SendError::Counted { blame: Blame::NoService, .. }
        ));
        // A pipe that broke before anything opened touched nothing.
        assert!(matches!(
            relay_failure(failed("the connection ended before sending"), None, false, None, dropped),
            SendError::NotATry { .. }
        ));
        // A refusal code or a mismatch wins over class and over a hold.
        for text in ["no (tin_not_active)", "no (account_banned)", "no (return_mismatch)"] {
            for (class, held) in [(ours, false), (ours, true), (None, false)] {
                assert_eq!(
                    relay_failure(failed(text), class, held, wait, none),
                    failed(text),
                    "{text}"
                );
            }
        }
        // Maybe sent stays maybe sent, whatever the class.
        let maybe = SendError::MaybeSent("the connection ended during the send".into());
        assert_eq!(relay_failure(maybe.clone(), ours, true, wait, none), maybe);
        assert_eq!(
            SendError::NotATry { reason: "x (tin_not_active)".into(), wait_until: None }.refusal(),
            None
        );
    }

    /// End to end on loopback: a held ticket comes back as not a try with
    /// the hold's time, and the hold's sign is kept.
    #[tokio::test]
    async fn a_held_ticket_is_not_sent_and_not_a_try() {
        use crate::testkit::{Answer, fixture_json, serve, submitter, trust_documents};
        let (public, jws, sealing) = trust_documents(&["127.0.0.1:9"]);
        let (url, seen) = serve(move |request| match request.path.as_str() {
            "/.well-known/buwiz-trust.json" => Answer { status: 200, headers: Vec::new(), body: jws.clone() },
            "/.well-known/buwiz-hpke.json" => Answer { status: 200, headers: Vec::new(), body: sealing.clone() },
            "/api/tickets" => Answer::json(503, &fixture_json("contract-hold-bir-offline.json"))
                .with("retry-after", "841")
                .with("buwiz-bir", "v=1, seq=4100, state=offline"),
            _ => Answer::json(404, &json!({"error": {"code": "not_found", "message": "x"}})),
        });
        let client = submitter(&url, true, vec![public]);
        let sent = client
            .send("00000000000000-1601Cv2018-102026#juana@example.test#.xml", b"<xml/>")
            .await;
        let Err(SendError::NotATry { reason, wait_until }) = sent else { panic!("{sent:?}") };
        assert!(reason.ends_with("(bir_offline)"), "{reason}");
        assert_eq!(wait_until, bir_core::smart_queue::parse_time("2026-11-11T09:13:41Z"));
        assert_eq!(client.last_sign().unwrap().sign.seq, 4182, "the body's sign is newer than the header's");
        let seen = seen.lock().unwrap();
        assert!(seen.iter().any(|request| request.path == "/api/tickets"));
        assert!(!seen.iter().any(|request| request.path == "/api/v2/submissions"), "no submission after a hold");
        assert!(seen.iter().all(|request| request.header(CLIENT_HEADER).is_some()));
    }

    /// Nothing reachable at all: not a try, never a counted failure.
    #[tokio::test]
    async fn an_unreachable_server_is_not_a_try() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let client = crate::testkit::submitter(&url, true, Vec::new());
        let sent = client.send("00000000000000-1601Cv2018-102026#juana@example.test#.xml", b"<xml/>").await;
        assert!(matches!(sent, Err(SendError::NotATry { wait_until: None, .. })), "{sent:?}");
    }
}
