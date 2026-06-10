//! Thin async client for the daemon IPC socket. Tracks request ids,
//! enforces the Hello handshake, and exposes `request()` + `events()`.

use crate::{codec, Event, Message, Op, PROTO_VERSION, Request, Response, ResponseData};
use anyhow::{anyhow, bail, Result};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;

pub struct Client {
    writer: OwnedWriteHalf,
    next_id: u64,
    inbox: mpsc::Receiver<Inbound>,
}

enum Inbound {
    Response(Response),
}

impl Client {
    pub async fn connect(path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(path)
            .await
            .map_err(|e| anyhow!("connect {}: {e}", path.display()))?;
        let (reader, writer) = stream.into_split();
        let (tx, rx) = mpsc::channel::<Inbound>(64);
        spawn_reader(reader, tx, None);

        let mut c = Self { writer, next_id: 1, inbox: rx };
        let data = c
            .request(Op::Hello {
                client: "sw".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            })
            .await?;
        match data {
            ResponseData::Hello { proto, .. } if proto == PROTO_VERSION => Ok(c),
            ResponseData::Hello { proto, .. } => {
                bail!("daemon proto {proto} != client {PROTO_VERSION}")
            }
            _ => bail!("unexpected response to Hello"),
        }
    }

    pub async fn request(&mut self, op: Op) -> Result<ResponseData> {
        let id = self.next_id;
        self.next_id += 1;
        let frame = codec::encode(&Message::Request(Request { id, op }))?;
        self.writer.write_all(frame.as_bytes()).await?;
        loop {
            let inbound = self
                .inbox
                .recv()
                .await
                .ok_or_else(|| anyhow!("daemon closed connection"))?;
            match inbound {
                Inbound::Response(r) if r.id == id => {
                    if r.ok {
                        return Ok(r.data.unwrap_or(ResponseData::Empty {}));
                    } else {
                        let e = r.error.unwrap();
                        bail!("daemon error: {e}");
                    }
                }
                Inbound::Response(_) => continue,
            }
        }
    }

}

/// Convenience: connect, Hello, Subscribe — return only the event stream.
pub async fn connect_subscriber(
    path: &Path,
    filter: Option<crate::EventFilter>,
) -> Result<mpsc::Receiver<Event>> {
    let stream = UnixStream::connect(path).await?;
    let (reader, mut writer) = stream.into_split();
    let (in_tx, mut in_rx) = mpsc::channel::<Inbound>(64);
    let (ev_tx, ev_rx) = mpsc::channel::<Event>(256);
    spawn_reader(reader, in_tx, Some(ev_tx));

    let hello = codec::encode(&Message::Request(Request {
        id: 1,
        op: Op::Hello {
            client: "sw".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
    }))?;
    writer.write_all(hello.as_bytes()).await?;
    match in_rx.recv().await {
        Some(Inbound::Response(r)) if r.ok => {}
        Some(Inbound::Response(r)) => bail!("Hello failed: {:?}", r.error),
        None => bail!("daemon closed during Hello"),
    }

    let sub = codec::encode(&Message::Request(Request {
        id: 2,
        op: Op::Subscribe { filter },
    }))?;
    writer.write_all(sub.as_bytes()).await?;
    match in_rx.recv().await {
        Some(Inbound::Response(r)) if r.ok => {}
        Some(Inbound::Response(r)) => bail!("Subscribe failed: {:?}", r.error),
        None => bail!("daemon closed during Subscribe"),
    }

    // Keep the writer alive so the connection stays open; spawn a task
    // that owns it and exits when the event channel closes.
    tokio::spawn(async move {
        let _w = writer;
        // Drain remaining responses (none expected) so the reader task
        // doesn't block on a full inbox.
        while in_rx.recv().await.is_some() {}
    });
    Ok(ev_rx)
}

fn spawn_reader(
    reader: OwnedReadHalf,
    inbox: mpsc::Sender<Inbound>,
    events: Option<mpsc::Sender<Event>>,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.is_empty() {
                continue;
            }
            match codec::decode(&line) {
                Ok(Message::Response(r)) => {
                    if inbox.send(Inbound::Response(r)).await.is_err() {
                        return;
                    }
                }
                Ok(Message::Event(e)) => {
                    if let Some(tx) = &events {
                        if tx.send(e).await.is_err() {
                            return;
                        }
                    }
                }
                Ok(Message::Request(_)) => {
                    tracing_unexpected("Request from daemon");
                }
                Err(e) => {
                    tracing_unexpected(&format!("decode error: {e}"));
                    return;
                }
            }
        }
    });
}

fn tracing_unexpected(_msg: &str) {
    // Kept as a hook; the ipc crate is intentionally tracing-free so
    // far. Callers see closed channels and surface their own errors.
}
