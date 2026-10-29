//! The client of buwiz-server: device-flow sign-in, the device key,
//! tickets, sealed v2 submissions, the relay pipe and receipts.

use std::sync::Mutex;

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

/// Where the server is when neither `BUWIZ_SERVER_URL` nor the setting
/// says otherwise.
pub const DEFAULT_SERVER_URL: &str = "https://api.buwiz.com";
/// The OAuth client the desktop and phone apps sign in as.
pub const CLIENT_ID: &str = "buwiz-desktop";
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

impl SubmitConfig {
    /// `BUWIZ_SERVER_URL` when set, else the app's setting, else
    /// [`DEFAULT_SERVER_URL`].
    pub fn new(setting: Option<&str>) -> Self {
        let from_env = std::env::var("BUWIZ_SERVER_URL")
            .ok()
            .filter(|url| !url.trim().is_empty());
        let url = from_env
            .or_else(|| {
                setting
                    .map(str::trim)
                    .filter(|url| !url.is_empty())
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
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum SendError {
    /// Nothing reached BIR; sending again is safe.
    #[error("{0}")]
    NotSent(String),
    /// BIR may have the return. Never send it again automatically.
    #[error("{0}")]
    MaybeSent(String),
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

/// One app's connection to buwiz-server. Cheap to share behind an `Arc`.
pub struct Submitter {
    config: SubmitConfig,
    http: reqwest::Client,
    store: Box<dyn SecretStore>,
    pending: Mutex<Option<(DeviceCode, i64)>>,
    /// Serializes token refreshes and device registration.
    session_lock: tokio::sync::Mutex<()>,
}

impl Submitter {
    pub fn new(config: SubmitConfig, store: Box<dyn SecretStore>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent(concat!("buwiz-oss/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("an HTTP client");
        Self {
            config,
            http,
            store,
            pending: Mutex::new(None),
            session_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn config(&self) -> &SubmitConfig {
        &self.config
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.config.server_url)
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
            .http
            .post(self.url("/auth/device/start"))
            .json(&json!({ "client_id": self.config.client_id }))
            .send()
            .await?;
        let code: DeviceCode = ok_json(response).await?;
        *self.pending.lock().expect("pending") =
            Some((code.clone(), device::now() + code.expires_in));
        Ok(code)
    }

    /// One poll of the sign-in [`Self::start_sign_in`] began. On approval
    /// the tokens are stored and the device key registered.
    pub async fn poll_sign_in(&self) -> Result<PollOutcome, ClientError> {
        let Some((code, _)) = self.pending.lock().expect("pending").clone() else {
            return Ok(PollOutcome::Expired);
        };
        let response = self
            .http
            .post(self.url("/auth/device/poll"))
            .json(&json!({ "client_id": self.config.client_id, "device_code": code.device_code }))
            .send()
            .await?;
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
        *self.pending.lock().expect("pending") = None;
        if let Ok(me) = self.authorized_get("/me").await {
            let mut session = self.session()?.ok_or(ClientError::SignedOut)?;
            session.email = me["email"].as_str().map(str::to_owned);
            self.save_session(&session)?;
        }
        self.ensure_device().await?;
        Ok(PollOutcome::SignedIn)
    }

    /// Ends this app's sign-in, on the server too when it can be reached.
    pub async fn sign_out(&self) -> Result<(), ClientError> {
        if let Ok(Some(session)) = self.session() {
            let _ = self
                .http
                .post(self.url("/api/auth/logout"))
                .bearer_auth(&session.access_token)
                .send()
                .await;
        }
        self.forget_session()
    }

    fn forget_session(&self) -> Result<(), ClientError> {
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
            .http
            .post(self.url("/oauth/token"))
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
        let tokens: Value = ok_json(response).await?;
        self.store_tokens(&tokens, Some(&session))
    }

    async fn authorized_get(&self, path: &str) -> Result<Value, ClientError> {
        let session = self.live_session(false).await?;
        let response = self
            .http
            .get(self.url(path))
            .bearer_auth(&session.access_token)
            .send()
            .await?;
        ok_json(response).await
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
            .http
            .post(self.url(path))
            .bearer_auth(&session.access_token)
            .header("content-type", "application/json")
            .body(body.clone());
        for (name, value) in key.sign("POST", path, &body, device::now(), &device::nonce()) {
            request = request.header(name, value);
        }
        let registered: Value = ok_json(request.send().await?).await?;
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
                .http
                .request(method_value, self.url(path))
                .bearer_auth(&session.access_token)
                .header(HEADER_DEVICE, &device_id)
                .header("content-type", "application/json")
                .body(body.clone());
            for (name, value) in key.sign(method, path, &body, device::now(), &device::nonce()) {
                request = request.header(name, value);
            }
            match ok_json(request.send().await?).await {
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
            .http
            .get(self.url("/.well-known/buwiz-trust.json"))
            .send()
            .await?;
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
        let document: Value = ok_json(
            self.http
                .get(self.url("/.well-known/buwiz-hpke.json"))
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
    pub async fn ready(&self) -> Result<(), ClientError> {
        let list = self.trust_list().await?;
        self.ensure_device().await?;
        self.sealing_key(&list).await?;
        Ok(())
    }

    // --------------------------------------------------------------- send

    /// Sends one return: ticket, sealed envelope, relay, verified receipt.
    pub async fn send(&self, filename: &str, xml: &[u8]) -> Result<Sent, SendError> {
        let name = FileName::parse(filename)
            .ok_or_else(|| SendError::NotSent(format!("not an eBIRForms file name: {filename}")))?;
        let xml_sha256 = hex(&Sha256::digest(xml));
        let idempotency_key = hex(&Sha256::digest(
            format!("buwiz-send\0{filename}\0{xml_sha256}").as_bytes(),
        ))[..40]
            .to_owned();
        let not_sent = |error: ClientError| SendError::NotSent(error.to_string());
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
        let (status, error, receipt_jws) = match ended {
            Ended::Result {
                status,
                error,
                receipt,
            } => (status, error, receipt),
            Ended::Dropped { opened } => {
                match self.authorized_get(&format!("/api/submissions/{id}")).await {
                    Ok(view) => (
                        view["status"].as_str().unwrap_or("unknown").to_owned(),
                        view["error"].as_str().map(str::to_owned),
                        view["receipt_jws"].as_str().map(str::to_owned),
                    ),
                    Err(_) if !opened => (
                        "failed".to_owned(),
                        Some("the connection ended before sending".into()),
                        None,
                    ),
                    Err(_) => (
                        "unknown".to_owned(),
                        Some("the connection ended during the send".into()),
                        None,
                    ),
                }
            }
        };
        match (status.as_str(), receipt_jws) {
            ("sent" | "dry_run", Some(receipt_jws)) => match list.verify_receipt(&receipt_jws, &xml_sha256, Some(jti)) {
                Ok(receipt) => Ok(Sent { submission_id: id.to_owned(), receipt_jws, receipt }),
                Err(error) => Err(SendError::MaybeSent(format!(
                    "the server reports the return sent, but its receipt does not check out ({error}); check for BIR's confirmation email"
                ))),
            },
            ("failed" | "expired" | "ready", _) => {
                Err(SendError::NotSent(error.unwrap_or_else(|| "the server did not send the return".into())))
            }
            _ => Err(SendError::MaybeSent(error.unwrap_or_else(|| {
                "BIR may have received this return; check for its confirmation email before sending again".into()
            }))),
        }
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
        put(HEADER_DEVICE, device_id)?;
        for (name, value) in key.sign("GET", relay_path, b"", device::now(), &device::nonce()) {
            put(name, value)?;
        }
        let (socket, _) = tokio_tungstenite::connect_async(request)
            .await
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

/// The JSON body of a 2xx answer, or the server's `{"error":{…}}`.
async fn ok_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, ClientError> {
    let status = response.status();
    if status.is_success() {
        return response
            .json()
            .await
            .map_err(|error| ClientError::Other(format!("unreadable answer: {error}")));
    }
    let body: Value = response.json().await.unwrap_or_default();
    let (code, message) = match &body["error"] {
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
    };
    Err(ClientError::Server {
        status: status.as_u16(),
        code,
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        unsafe { std::env::remove_var("BUWIZ_SERVER_URL") };
        assert_eq!(SubmitConfig::new(None).server_url, DEFAULT_SERVER_URL);
        assert_eq!(
            SubmitConfig::new(Some("http://localhost:3008/")).server_url,
            "http://localhost:3008"
        );
        unsafe { std::env::set_var("BUWIZ_SERVER_URL", "http://127.0.0.1:9") };
        assert_eq!(
            SubmitConfig::new(Some("http://localhost:3008")).server_url,
            "http://127.0.0.1:9"
        );
        unsafe { std::env::remove_var("BUWIZ_SERVER_URL") };
    }
}
