//! The app's end of the relay pipe: a dumb byte forwarder.
//!
//! The server sends `open` for a host and port; the app connects only if
//! the signed [`TrustList`] permits it, then copies bytes both ways without
//! looking at them. Everything that passes is TLS or SSH ciphertext, or
//! the dispatcher's wrapped fields: the BIR secrets and the SSH session
//! stay on the server. One WebSocket carries every stream. Text frames are
//! JSON controls, binary frames `[stream id][bytes]`.

use std::collections::HashMap;
use std::time::Duration;

use futures_util::{SinkExt, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::trust::TrustList;

/// The whole of one `open`: the lookup and every address tried in turn.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The name lookup alone. One that takes longer is a `dns` failure, not a
/// slow connection.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a relay may run, and how silent the server may be, before the
/// app lets go of it. A send that was cut short this way is read back from
/// Buwiz: after any stream opened, the outcome is "maybe sent".
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// The most one pipe lives, from the first frame to the `result`.
    pub overall: Duration,
    /// No frame (a ping's pong included) from the server for this long ends
    /// it.
    pub idle: Duration,
    /// The app pings this often, so a quiet but live server is not idle.
    pub ping: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            overall: Duration::from_secs(9 * 60),
            idle: Duration::from_secs(3 * 60),
            ping: Duration::from_secs(30),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Control {
    Open {
        stream: u8,
        host: String,
        port: u16,
    },
    Opened {
        stream: u8,
    },
    Refused {
        stream: u8,
        reason: String,
    },
    Close {
        stream: u8,
    },
    Result {
        status: String,
        error: Option<String>,
        receipt: Option<Value>,
    },
}

/// A v1 relay's hold (`result.hold`): Buwiz held the send, nothing
/// reached BIR.
#[derive(Clone, Debug, PartialEq)]
pub struct Hold {
    pub code: Option<String>,
    pub wait_until: Option<chrono::DateTime<chrono::Utc>>,
}

/// How a pipe ended.
#[derive(Debug, PartialEq)]
pub enum Ended {
    /// The server's final `result`. `class` (`bir`, `user_network`,
    /// `ours`), `hold` and `bir` (the sign right after this try was
    /// recorded) are absent from servers before the BIR sign.
    Result {
        status: String,
        error: Option<String>,
        receipt: Option<String>,
        class: Option<String>,
        hold: Option<Hold>,
        bir: Option<Value>,
        /// Whether any connection the app made toward the BIR opened: when
        /// none did, nothing touched the BIR.
        opened: bool,
        /// Every `open` the app attempted failed on the way in (kinds `dns`,
        /// `refused`, `timeout`, `unreachable`) and none opened: the try
        /// never got past the BIR's front desk.
        path_down: bool,
    },
    /// The socket closed first. `opened` tells whether any connection was
    /// made, so whether anything could have reached BIR.
    Dropped {
        opened: bool,
        /// As [`Ended::Result`]'s `path_down`.
        path_down: bool,
    },
}

/// Forwards until the server's `result` or the socket ends, within the
/// default [`Limits`].
pub async fn serve<S>(socket: S, allowed: &TrustList) -> Ended
where
    S: Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + Unpin
        + Send
        + 'static,
{
    serve_within(socket, allowed, Limits::default()).await
}

/// [`serve`] with its own [`Limits`]: the pipe ends as dropped when it runs
/// past `overall` or the server is silent for `idle`.
pub async fn serve_within<S>(socket: S, allowed: &TrustList, limits: Limits) -> Ended
where
    S: Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + Unpin
        + Send
        + 'static,
{
    let (mut sink, mut stream) = socket.split();
    let (out, mut outgoing) = mpsc::channel::<Message>(64);
    let writer = tokio::spawn(async move {
        while let Some(message) = outgoing.recv().await {
            if sink.send(message).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });
    let mut writers: HashMap<u8, mpsc::UnboundedSender<Option<Vec<u8>>>> = HashMap::new();
    let mut opened = false;
    let (mut attempted, mut path_failed) = (0u32, 0u32);
    let started = tokio::time::Instant::now();
    let deadline = started + limits.overall;
    let mut last_heard = started;
    let mut ping = tokio::time::interval_at(started + limits.ping, limits.ping);
    let ended = loop {
        let message = tokio::select! {
            next = stream.next() => match next {
                Some(Ok(message)) => {
                    last_heard = tokio::time::Instant::now();
                    message
                }
                _ => break Ended::Dropped {
                        opened,
                        path_down: !opened && attempted > 0 && path_failed == attempted,
                    },
            },
            _ = ping.tick() => {
                if last_heard.elapsed() >= limits.idle {
                    tracing::warn!("relay: the server went quiet; letting go of the pipe");
                    break Ended::Dropped {
                        opened,
                        path_down: !opened && attempted > 0 && path_failed == attempted,
                    };
                }
                let _ = out.try_send(Message::Ping(Default::default()));
                continue;
            }
            _ = tokio::time::sleep_until(deadline) => {
                tracing::warn!("relay: the pipe ran past its time limit; letting go of it");
                break Ended::Dropped {
                        opened,
                        path_down: !opened && attempted > 0 && path_failed == attempted,
                    };
            }
        };
        match message {
            Message::Text(text) => match serde_json::from_str::<Control>(&text) {
                Ok(Control::Open { stream, host, port }) => {
                    let answer = if !allowed.permits(&host, port) {
                        tracing::warn!(
                            "relay: refused {host}:{port}, not on the signed trust list"
                        );
                        refused(
                            stream,
                            format!("{host}:{port} is not on the signed trust list"),
                            Some(RefusedKind::NotListed),
                            port,
                        )
                    } else {
                        attempted += 1;
                        match tokio::time::timeout(
                            CONNECT_TIMEOUT + LOOKUP_TIMEOUT + Duration::from_secs(1),
                            connect(&host, port),
                        )
                        .await
                        {
                            Ok(Ok(tcp)) => {
                                let _ = tcp.set_nodelay(true);
                                // `opened` goes out before any byte the far
                                // end sends (an SSH server speaks first).
                                let _ = out.send(control(&Control::Opened { stream })).await;
                                writers.insert(stream, forward(stream, tcp, out.clone()));
                                opened = true;
                                continue;
                            }
                            Ok(Err((kind, error))) => {
                                if kind.is_some_and(RefusedKind::is_path) {
                                    path_failed += 1;
                                }
                                refused(stream, error.to_string(), kind, port)
                            }
                            Err(_) => {
                                path_failed += 1;
                                refused(
                                    stream,
                                    "connection timed out".into(),
                                    Some(RefusedKind::Timeout),
                                    port,
                                )
                            }
                        }
                    };
                    let _ = out.send(answer).await;
                }
                Ok(Control::Close { stream }) => {
                    if let Some(writer) = writers.remove(&stream) {
                        let _ = writer.send(None);
                    }
                }
                Ok(Control::Result {
                    status,
                    error,
                    receipt,
                }) => {
                    let receipt = receipt.and_then(|value| value.as_str().map(str::to_owned));
                    // The fields servers with the BIR sign add, read from
                    // the raw frame: `Control` itself does not change.
                    let extras: ResultExtras = serde_json::from_str(&text).unwrap_or_default();
                    break Ended::Result {
                        status,
                        error,
                        receipt,
                        class: extras.class,
                        hold: extras.hold.map(|hold| Hold {
                            code: hold.code,
                            wait_until: hold
                                .wait_until
                                .as_deref()
                                .and_then(bir_core::smart_queue::parse_time),
                        }),
                        bir: extras.bir.filter(|bir| !bir.is_null()),
                        opened,
                        path_down: !opened && attempted > 0 && path_failed == attempted,
                    };
                }
                // Only the app sends these; anything else is ignored.
                Ok(Control::Opened { .. } | Control::Refused { .. }) | Err(_) => {}
            },
            Message::Binary(bytes) => {
                if let Some((&stream, data)) = bytes.split_first()
                    && let Some(writer) = writers.get(&stream)
                {
                    let _ = writer.send(Some(data.to_vec()));
                }
            }
            Message::Close(_) => break Ended::Dropped {
                        opened,
                        path_down: !opened && attempted > 0 && path_failed == attempted,
                    },
            _ => {}
        }
    };
    for (_, writer) in writers.drain() {
        let _ = writer.send(None);
    }
    drop(out);
    let _ = tokio::time::timeout(Duration::from_secs(5), writer).await;
    ended
}

/// The `result` frame's fields from servers with the BIR sign.
#[derive(Default, Deserialize)]
struct ResultExtras {
    #[serde(default)]
    class: Option<String>,
    #[serde(default)]
    hold: Option<HoldFrame>,
    #[serde(default)]
    bir: Option<Value>,
}

#[derive(Deserialize)]
struct HoldFrame {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    wait_until: Option<String>,
}

/// Why the app could not open a connection the server asked for, as the
/// `refused` frame's `kind` names it (the server reads it; an older server
/// ignores the extra key).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusedKind {
    /// The host's name did not resolve.
    Dns,
    /// The far end refused, or no route to its host.
    Refused,
    /// No answer within the connect timeout.
    Timeout,
    /// This machine has no route to that network.
    Unreachable,
    /// This machine forbids the connection (a firewall, a sandbox).
    Denied,
    /// The connection was reset or aborted on the way.
    Reset,
    /// Not on the signed trust list: the app never tried.
    NotListed,
}

impl RefusedKind {
    /// The kind of a failed connect, when it is one the server knows.
    pub fn of(error: &std::io::Error) -> Option<Self> {
        use std::io::ErrorKind;
        match error.kind() {
            ErrorKind::ConnectionRefused | ErrorKind::HostUnreachable => Some(Self::Refused),
            ErrorKind::TimedOut => Some(Self::Timeout),
            ErrorKind::NetworkUnreachable
            | ErrorKind::NetworkDown
            | ErrorKind::AddrNotAvailable => Some(Self::Unreachable),
            ErrorKind::PermissionDenied => Some(Self::Denied),
            ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted => Some(Self::Reset),
            _ => None,
        }
    }

    /// A failure on the way in: the connection never got to the far end.
    pub fn is_path(self) -> bool {
        matches!(
            self,
            Self::Dns | Self::Refused | Self::Timeout | Self::Unreachable
        )
    }

    /// How much a failed connect says, to pick one among several
    /// addresses: the far end answering "no" says the most.
    fn informative(kind: Option<Self>) -> u8 {
        match kind {
            Some(Self::Refused) => 6,
            Some(Self::Reset) => 5,
            Some(Self::Denied) => 4,
            Some(Self::Unreachable) => 3,
            Some(Self::Timeout) => 2,
            _ => 1,
        }
    }
}

/// The app's `refused` answer: [`Control::Refused`]'s fields plus `kind`
/// and the `port` it was for.
#[derive(Serialize)]
struct RefusedFrame {
    #[serde(rename = "type")]
    frame: &'static str,
    stream: u8,
    reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<RefusedKind>,
    port: u16,
}

fn refused(stream: u8, reason: String, kind: Option<RefusedKind>, port: u16) -> Message {
    let frame = RefusedFrame {
        frame: "refused",
        stream,
        reason,
        kind,
        port,
    };
    Message::Text(
        serde_json::to_string(&frame)
            .expect("controls serialize")
            .into(),
    )
}

/// Resolve `host`, then connect to its addresses in turn within the
/// connect timeout, telling a failed or timed-out lookup (`dns`) apart.
async fn connect(
    host: &str,
    port: u16,
) -> Result<TcpStream, (Option<RefusedKind>, std::io::Error)> {
    let lookup = async {
        tokio::net::lookup_host((host, port))
            .await
            .map(Iterator::collect::<Vec<std::net::SocketAddr>>)
    };
    connect_after(lookup, host, LOOKUP_TIMEOUT, CONNECT_TIMEOUT).await
}

/// [`connect`] over any lookup (a test's own, so no query leaves the
/// machine). `connect_budget` bounds every address together; when several
/// fail, the most informative error is the answer.
async fn connect_after(
    lookup: impl std::future::Future<Output = std::io::Result<Vec<std::net::SocketAddr>>>,
    host: &str,
    lookup_budget: Duration,
    connect_budget: Duration,
) -> Result<TcpStream, (Option<RefusedKind>, std::io::Error)> {
    use std::io::{Error, ErrorKind};
    let addresses = match tokio::time::timeout(lookup_budget, lookup).await {
        Ok(Ok(addresses)) => addresses,
        Ok(Err(error)) => return Err((Some(RefusedKind::Dns), error)),
        Err(_) => {
            return Err((
                Some(RefusedKind::Dns),
                Error::new(ErrorKind::TimedOut, format!("looking up {host} timed out")),
            ));
        }
    };
    if addresses.is_empty() {
        return Err((
            Some(RefusedKind::Dns),
            Error::new(ErrorKind::NotFound, format!("no address for {host}")),
        ));
    }
    let started = tokio::time::Instant::now();
    let mut failures: Vec<(Option<RefusedKind>, Error)> = Vec::new();
    for address in addresses {
        let left = connect_budget.saturating_sub(started.elapsed());
        if left.is_zero() {
            break;
        }
        match tokio::time::timeout(left, TcpStream::connect(address)).await {
            Ok(Ok(tcp)) => return Ok(tcp),
            Ok(Err(error)) => failures.push((RefusedKind::of(&error), error)),
            Err(_) => {
                failures.push((
                    Some(RefusedKind::Timeout),
                    Error::new(ErrorKind::TimedOut, "connection timed out"),
                ));
                break;
            }
        }
    }
    let mut best: Option<(Option<RefusedKind>, Error)> = None;
    for failure in failures {
        if best
            .as_ref()
            .is_none_or(|(kind, _)| RefusedKind::informative(failure.0) > RefusedKind::informative(*kind))
        {
            best = Some(failure);
        }
    }
    Err(best.unwrap_or_else(|| {
        (
            Some(RefusedKind::Timeout),
            Error::new(ErrorKind::TimedOut, "connection timed out"),
        )
    }))
}

fn control(control: &Control) -> Message {
    Message::Text(
        serde_json::to_string(control)
            .expect("controls serialize")
            .into(),
    )
}

/// Copies between one TCP connection and the pipe.
fn forward(
    stream: u8,
    tcp: TcpStream,
    out: mpsc::Sender<Message>,
) -> mpsc::UnboundedSender<Option<Vec<u8>>> {
    let (mut tcp_read, mut tcp_write) = tcp.into_split();
    let (sender, mut from_server) = mpsc::unbounded_channel::<Option<Vec<u8>>>();
    tokio::spawn(async move {
        while let Some(Some(bytes)) = from_server.recv().await {
            if tcp_write.write_all(&bytes).await.is_err() {
                return;
            }
        }
        let _ = tcp_write.shutdown().await;
    });
    tokio::spawn(async move {
        let mut buffer = vec![0u8; 16 * 1024];
        loop {
            match tcp_read.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let mut frame = Vec::with_capacity(read + 1);
                    frame.push(stream);
                    frame.extend_from_slice(&buffer[..read]);
                    if out.send(Message::Binary(frame.into())).await.is_err() {
                        return;
                    }
                }
            }
        }
        let _ = out.send(control(&Control::Close { stream })).await;
    });
    sender
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{fixture, trust_documents};
    use tokio_tungstenite::WebSocketStream;
    use tokio_tungstenite::tungstenite::protocol::Role;

    /// The app's end of a pipe over memory, and the server's end to drive.
    async fn pipe(
        hosts: &[&str],
    ) -> (
        tokio::task::JoinHandle<Ended>,
        WebSocketStream<tokio::io::DuplexStream>,
    ) {
        let (public, jws, _) = trust_documents(hosts);
        let list = crate::trust::verify(&[public], &jws, crate::device::now()).unwrap();
        let (app, server) = tokio::io::duplex(64 * 1024);
        let app = WebSocketStream::from_raw_socket(app, Role::Client, None).await;
        let server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
        (tokio::spawn(async move { serve(app, &list).await }), server)
    }

    async fn next_text(server: &mut WebSocketStream<tokio::io::DuplexStream>) -> Value {
        loop {
            match server.next().await.expect("a frame").expect("a frame") {
                Message::Text(text) => return serde_json::from_str(&text).unwrap(),
                _ => continue,
            }
        }
    }

    /// The public contract (`docs/bir-uptime.md` in `buwiz-com/buwiz-com`): a v1 relay's hold arrives as `failed` with `hold`,
    /// `class: ours` and the sign.
    #[tokio::test]
    async fn a_result_with_a_hold_carries_class_hold_and_sign() {
        let (app, mut server) = pipe(&[]).await;
        let frame = fixture("contract-relay-hold-result.json");
        server.send(Message::Text(frame.into())).await.unwrap();
        let Ended::Result {
            status,
            error,
            receipt,
            class,
            hold,
            bir,
            opened,
            path_down,
        } = app.await.unwrap()
        else {
            panic!("a result")
        };
        assert!(!opened && !path_down, "no open was asked for");
        assert_eq!(status, "failed");
        assert!(error.unwrap().contains("Nothing reached the BIR"));
        assert_eq!(receipt, None);
        assert_eq!(class.as_deref(), Some("ours"));
        let hold = hold.unwrap();
        assert_eq!(hold.code.as_deref(), Some("bir_offline"));
        assert_eq!(
            hold.wait_until,
            bir_core::smart_queue::parse_time("2026-11-11T09:13:41Z")
        );
        assert_eq!(bir.unwrap()["seq"], 4183);
    }

    /// An older server's result has none of them; a counted class reads as
    /// sent.
    #[tokio::test]
    async fn an_older_servers_result_has_no_class_hold_or_sign() {
        let (app, mut server) = pipe(&[]).await;
        let old = r#"{"type":"result","status":"failed","error":"the relay closed early","receipt":null}"#;
        server.send(Message::Text(old.into())).await.unwrap();
        let ended = app.await.unwrap();
        assert_eq!(
            ended,
            Ended::Result {
                status: "failed".into(),
                error: Some("the relay closed early".into()),
                receipt: None,
                class: None,
                hold: None,
                bir: None,
                opened: false,
                path_down: false,
            }
        );
        let (app, mut server) = pipe(&[]).await;
        let frame = fixture("contract-relay-result-failed-bir.json");
        server.send(Message::Text(frame.into())).await.unwrap();
        let Ended::Result { class, hold, .. } = app.await.unwrap() else {
            panic!()
        };
        assert_eq!((class.as_deref(), hold), (Some("bir"), None));
    }

    /// The `refused` answer names its kind and port; an older server's
    /// `Control` still reads it.
    #[tokio::test]
    async fn a_refused_open_says_why_and_for_which_port() {
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = closed.local_addr().unwrap().port();
        drop(closed);
        let listed = format!("127.0.0.1:{port}");
        let (app, mut server) = pipe(&[&listed]).await;
        let open = |stream: u8, port: u16| {
            Message::Text(
                serde_json::to_string(&Control::Open {
                    stream,
                    host: "127.0.0.1".into(),
                    port,
                })
                .unwrap()
                .into(),
            )
        };
        server.send(open(1, port + 1)).await.unwrap();
        let answer = next_text(&mut server).await;
        assert_eq!(answer["type"], "refused");
        assert_eq!(answer["kind"], "not_listed");
        assert_eq!(answer["port"], port + 1);
        assert_eq!(
            serde_json::from_value::<Control>(answer.clone()).unwrap(),
            Control::Refused {
                stream: 1,
                reason: format!("127.0.0.1:{} is not on the signed trust list", port + 1)
            }
        );
        server.send(open(2, port)).await.unwrap();
        let answer = next_text(&mut server).await;
        assert_eq!(
            (answer["stream"].clone(), answer["kind"].clone()),
            (json!(2), json!("refused")),
            "{answer}"
        );
        assert_eq!(answer["port"], port);
        server.close(None).await.unwrap();
        // The one open it tried was refused on the way in.
        assert_eq!(app.await.unwrap(), Ended::Dropped { opened: false, path_down: true });
    }

    #[test]
    fn connect_errors_map_to_the_kinds_the_server_reads() {
        use std::io::{Error, ErrorKind};
        let rows = [
            (ErrorKind::ConnectionRefused, Some(RefusedKind::Refused)),
            (ErrorKind::HostUnreachable, Some(RefusedKind::Refused)),
            (ErrorKind::TimedOut, Some(RefusedKind::Timeout)),
            (
                ErrorKind::NetworkUnreachable,
                Some(RefusedKind::Unreachable),
            ),
            (ErrorKind::PermissionDenied, Some(RefusedKind::Denied)),
            (ErrorKind::NetworkDown, Some(RefusedKind::Unreachable)),
            (ErrorKind::AddrNotAvailable, Some(RefusedKind::Unreachable)),
            (ErrorKind::ConnectionReset, Some(RefusedKind::Reset)),
            (ErrorKind::ConnectionAborted, Some(RefusedKind::Reset)),
            (ErrorKind::Other, None),
        ];
        for (kind, want) in rows {
            assert_eq!(RefusedKind::of(&Error::from(kind)), want, "{kind:?}");
        }
        let words: Vec<Value> = [
            RefusedKind::Dns,
            RefusedKind::Refused,
            RefusedKind::Timeout,
            RefusedKind::Unreachable,
            RefusedKind::Denied,
            RefusedKind::Reset,
            RefusedKind::NotListed,
        ]
        .into_iter()
        .map(|kind| serde_json::to_value(kind).unwrap())
        .collect();
        assert_eq!(
            words,
            [
                json!("dns"),
                json!("refused"),
                json!("timeout"),
                json!("unreachable"),
                json!("denied"),
                json!("reset"),
                json!("not_listed")
            ]
        );
    }

    /// The lookups here are the test's own: no query leaves the machine.
    #[tokio::test]
    async fn a_lookup_that_fails_or_times_out_is_dns() {
        let ms = Duration::from_millis;
        let failed = async { Err(std::io::Error::other("no such host")) };
        let Err((kind, _)) = connect_after(failed, "example.test", ms(50), ms(50)).await else {
            panic!("connected")
        };
        assert_eq!(kind, Some(RefusedKind::Dns));
        let empty = async { Ok(Vec::new()) };
        let Err((kind, _)) = connect_after(empty, "example.test", ms(50), ms(50)).await else {
            panic!("connected")
        };
        assert_eq!(kind, Some(RefusedKind::Dns));
        // A lookup that never answers is `dns`, not `timeout`.
        let hung = std::future::pending();
        let Err((kind, error)) = connect_after(hung, "example.test", ms(30), ms(50)).await else {
            panic!("connected")
        };
        assert_eq!(kind, Some(RefusedKind::Dns));
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }

    /// With several addresses, the far end saying no (a refusal) is more
    /// informative than the one that never answered. The budget is the real
    /// one: Windows takes about two seconds to report a refused port.
    #[tokio::test]
    async fn with_several_addresses_the_most_informative_error_is_reported() {
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = closed.local_addr().unwrap();
        drop(closed);
        let ms = Duration::from_millis;
        let both = async move { Ok(vec![address, address]) };
        let Err((kind, _)) = connect_after(both, "example.test", ms(50), Duration::from_secs(10)).await else {
            panic!("connected")
        };
        assert_eq!(kind, Some(RefusedKind::Refused));
        assert!(RefusedKind::informative(Some(RefusedKind::Refused)) > RefusedKind::informative(Some(RefusedKind::Timeout)));
        assert!(RefusedKind::informative(Some(RefusedKind::Reset)) > RefusedKind::informative(None));
    }

    /// Every open refused on the way in, none opened: `path_down`. One that
    /// opened: never. A try with no open at all: neither.
    #[tokio::test]
    async fn the_result_says_whether_a_stream_opened_and_whether_the_way_in_was_down() {
        let open = |stream: u8, port: u16| {
            Message::Text(
                serde_json::to_string(&Control::Open { stream, host: "127.0.0.1".into(), port })
                    .unwrap()
                    .into(),
            )
        };
        let result = || {
            Message::Text(
                r#"{"type":"result","status":"failed","error":"x","receipt":null,"class":"user_network"}"#
                    .into(),
            )
        };
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let down = closed.local_addr().unwrap().port();
        drop(closed);
        let live = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let up = live.local_addr().unwrap().port();
        let listed = format!("127.0.0.1:{down}");
        let listed_up = format!("127.0.0.1:{up}");

        // Both opens refused: the way in is down.
        let (app, mut server) = pipe(&[&listed, &listed_up]).await;
        server.send(open(1, down)).await.unwrap();
        next_text(&mut server).await;
        server.send(open(2, down)).await.unwrap();
        next_text(&mut server).await;
        server.send(result()).await.unwrap();
        let Ended::Result { opened, path_down, .. } = app.await.unwrap() else { panic!() };
        assert_eq!((opened, path_down), (false, true));

        // One opened, one refused: it got past the front desk.
        let (app, mut server) = pipe(&[&listed, &listed_up]).await;
        server.send(open(1, up)).await.unwrap();
        assert_eq!(next_text(&mut server).await["type"], "opened");
        server.send(open(2, down)).await.unwrap();
        next_text(&mut server).await;
        server.send(result()).await.unwrap();
        let Ended::Result { opened, path_down, .. } = app.await.unwrap() else { panic!() };
        assert_eq!((opened, path_down), (true, false));

        // A name off the trust list is never a try of the app's: not "down".
        let (app, mut server) = pipe(&[&listed]).await;
        server.send(open(1, down + 1)).await.unwrap();
        next_text(&mut server).await;
        server.send(result()).await.unwrap();
        let Ended::Result { opened, path_down, .. } = app.await.unwrap() else { panic!() };
        assert_eq!((opened, path_down), (false, false));
    }

    /// A server that goes quiet, or a pipe that runs on, is let go of.
    #[tokio::test]
    async fn a_quiet_server_and_a_pipe_that_runs_on_are_let_go_of() {
        let limits = |overall, idle, ping| Limits {
            overall: Duration::from_millis(overall),
            idle: Duration::from_millis(idle),
            ping: Duration::from_millis(ping),
        };
        for limit in [limits(60_000, 150, 50), limits(150, 60_000, 50)] {
            let (public, jws, _) = trust_documents(&[]);
            let list = crate::trust::verify(&[public], &jws, crate::device::now()).unwrap();
            let (app, server) = tokio::io::duplex(64 * 1024);
            let app = WebSocketStream::from_raw_socket(app, Role::Client, None).await;
            // The server end is held open and never answers.
            let _server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
            let ended = tokio::time::timeout(
                Duration::from_secs(5),
                serve_within(app, &list, limit),
            )
            .await
            .expect("let go of in time");
            assert_eq!(ended, Ended::Dropped { opened: false, path_down: false });
        }
    }
}
