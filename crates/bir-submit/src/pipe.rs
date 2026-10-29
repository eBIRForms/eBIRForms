//! The app's end of the relay pipe: a dumb byte forwarder.
//!
//! The server sends `open` for a host and port; the app connects only if
//! the signed [`EgressList`] permits it, then copies bytes both ways without
//! looking at them. Everything that passes is TLS or SSH ciphertext, or
//! the dispatcher's wrapped fields: the BIR secrets and the SSH session
//! stay on the server. One WebSocket carries every stream. Text frames are
//! JSON controls, binary frames `[stream id][bytes]`.

use std::collections::HashMap;
use std::time::Duration;

use futures_util::{SinkExt, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::egress::EgressList;

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

/// How a pipe ended.
#[derive(Debug, PartialEq)]
pub enum Ended {
    /// The server's final `result`.
    Result {
        status: String,
        error: Option<String>,
        receipt: Option<String>,
    },
    /// The socket closed first. `opened` tells whether any connection was
    /// made, so whether anything could have reached BIR.
    Dropped { opened: bool },
}

/// Forwards until the server's `result` or the socket ends.
pub async fn serve<S>(socket: S, allowed: &EgressList) -> Ended
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
                            "relay: refused {host}:{port}, not on the signed egress list"
                        );
                        Control::Refused {
                            stream,
                            reason: format!("{host}:{port} is not on the signed egress list"),
                        }
                    } else {
                        match tokio::time::timeout(
                            CONNECT_TIMEOUT,
                            TcpStream::connect((host.as_str(), port)),
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
                            Ok(Err(error)) => Control::Refused {
                                stream,
                                reason: error.to_string(),
                            },
                            Err(_) => Control::Refused {
                                stream,
                                reason: "connection timed out".into(),
                            },
                        }
                    };
                    let _ = out.send(control(&answer)).await;
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
                    break Ended::Result {
                        status,
                        error,
                        receipt,
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
