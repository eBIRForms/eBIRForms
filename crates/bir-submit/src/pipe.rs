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

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

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
    },
    /// The socket closed first. `opened` tells whether any connection was
    /// made, so whether anything could have reached BIR.
    Dropped { opened: bool },
}

/// Forwards until the server's `result` or the socket ends.
pub async fn serve<S>(socket: S, allowed: &TrustList) -> Ended
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
    let ended = loop {
        let Some(Ok(message)) = stream.next().await else {
            break Ended::Dropped { opened };
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
                        match tokio::time::timeout(CONNECT_TIMEOUT, connect(&host, port)).await {
                            Ok(Ok(tcp)) => {
                                let _ = tcp.set_nodelay(true);
                                // `opened` goes out before any byte the far
                                // end sends (an SSH server speaks first).
                                let _ = out.send(control(&Control::Opened { stream })).await;
                                writers.insert(stream, forward(stream, tcp, out.clone()));
                                opened = true;
                                continue;
                            }
                            Ok(Err((kind, error))) => refused(stream, error.to_string(), kind, port),
                            Err(_) => refused(
                                stream,
                                "connection timed out".into(),
                                Some(RefusedKind::Timeout),
                                port,
                            ),
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
            Message::Close(_) => break Ended::Dropped { opened },
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
            ErrorKind::NetworkUnreachable => Some(Self::Unreachable),
            ErrorKind::PermissionDenied => Some(Self::Denied),
            _ => None,
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
    let frame = RefusedFrame { frame: "refused", stream, reason, kind, port };
    Message::Text(serde_json::to_string(&frame).expect("controls serialize").into())
}

/// Resolve `host`, then connect to its addresses in turn, as
/// `TcpStream::connect((host, port))` does, telling a failed lookup apart.
async fn connect(host: &str, port: u16) -> Result<TcpStream, (Option<RefusedKind>, std::io::Error)> {
    let addresses: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| (Some(RefusedKind::Dns), error))?
        .collect();
    if addresses.is_empty() {
        return Err((
            Some(RefusedKind::Dns),
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("no address for {host}")),
        ));
    }
    TcpStream::connect(&addresses[..]).await.map_err(|error| (RefusedKind::of(&error), error))
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
    async fn pipe(hosts: &[&str]) -> (tokio::task::JoinHandle<Ended>, WebSocketStream<tokio::io::DuplexStream>) {
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

    /// API.md §5.2: a v1 relay's hold arrives as `failed` with `hold`,
    /// `class: ours` and the sign.
    #[tokio::test]
    async fn a_result_with_a_hold_carries_class_hold_and_sign() {
        let (app, mut server) = pipe(&[]).await;
        let frame = fixture("api-5.2-v1-relay-hold-result.json");
        server.send(Message::Text(frame.into())).await.unwrap();
        let Ended::Result { status, error, receipt, class, hold, bir } = app.await.unwrap() else {
            panic!("a result")
        };
        assert_eq!(status, "failed");
        assert!(error.unwrap().contains("Nothing reached the BIR"));
        assert_eq!(receipt, None);
        assert_eq!(class.as_deref(), Some("ours"));
        let hold = hold.unwrap();
        assert_eq!(hold.code.as_deref(), Some("bir_offline"));
        assert_eq!(hold.wait_until, bir_core::smart_queue::parse_time("2026-11-11T09:13:41Z"));
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
            }
        );
        let (app, mut server) = pipe(&[]).await;
        let frame = fixture("api-10-relay-result-failed-bir.json");
        server.send(Message::Text(frame.into())).await.unwrap();
        let Ended::Result { class, hold, .. } = app.await.unwrap() else { panic!() };
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
            Message::Text(serde_json::to_string(&Control::Open { stream, host: "127.0.0.1".into(), port }).unwrap().into())
        };
        server.send(open(1, port + 1)).await.unwrap();
        let answer = next_text(&mut server).await;
        assert_eq!(answer["type"], "refused");
        assert_eq!(answer["kind"], "not_listed");
        assert_eq!(answer["port"], port + 1);
        assert_eq!(
            serde_json::from_value::<Control>(answer.clone()).unwrap(),
            Control::Refused { stream: 1, reason: format!("127.0.0.1:{} is not on the signed trust list", port + 1) }
        );
        server.send(open(2, port)).await.unwrap();
        let answer = next_text(&mut server).await;
        assert_eq!((answer["stream"].clone(), answer["kind"].clone()), (json!(2), json!("refused")), "{answer}");
        assert_eq!(answer["port"], port);
        server.close(None).await.unwrap();
        assert_eq!(app.await.unwrap(), Ended::Dropped { opened: false });
    }

    #[test]
    fn connect_errors_map_to_the_kinds_the_server_reads() {
        use std::io::{Error, ErrorKind};
        let rows = [
            (ErrorKind::ConnectionRefused, Some(RefusedKind::Refused)),
            (ErrorKind::HostUnreachable, Some(RefusedKind::Refused)),
            (ErrorKind::TimedOut, Some(RefusedKind::Timeout)),
            (ErrorKind::NetworkUnreachable, Some(RefusedKind::Unreachable)),
            (ErrorKind::PermissionDenied, Some(RefusedKind::Denied)),
            (ErrorKind::ConnectionReset, None),
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
            RefusedKind::NotListed,
        ]
        .into_iter()
        .map(|kind| serde_json::to_value(kind).unwrap())
        .collect();
        assert_eq!(words, [json!("dns"), json!("refused"), json!("timeout"), json!("unreachable"), json!("denied"), json!("not_listed")]);
    }

    #[tokio::test]
    async fn a_name_that_does_not_resolve_is_dns() {
        // An empty name fails in the resolver before any query goes out.
        let Err((kind, _)) = connect("", 443).await else { panic!("resolved an empty name") };
        assert_eq!(kind, Some(RefusedKind::Dns));
    }
}
