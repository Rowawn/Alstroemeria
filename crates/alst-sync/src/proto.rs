//! Messages and the `Transport` they travel over (SPEC §9.3, §9.4).

use crate::{Error, Result};
use alst_core::sync::{Batch, Vector};
use serde::{Deserialize, Serialize};

pub const PROTOCOL: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Msg {
    Hello { protocol: u32, site_id: String, name: String, schema_version: i64, hlc_now: i64, vector: Vector },
    /// A slice of changes and events; the receiver applies it and answers `Ack`.
    Delta(Batch),
    Ack,
    /// The sender has nothing more in this direction.
    Done,
    Refuse { reason: String },
    /// Media hashes this side has on disk (§9.6).
    Have { hashes: Vec<String> },
    /// Hashes the sender wants from the other side.
    Want { hashes: Vec<String> },
    /// A piece of a file; travels as raw bytes, never JSON.
    #[serde(skip)]
    Chunk { hash: String, offset: u64, data: Vec<u8> },
    /// The file was sent completely.
    FileEnd { hash: String },
    /// The file was asked for but isn't here.
    Missing { hash: String },
    /// Pairing (§9.3): who is on the other end, and whether their user accepted.
    PairInfo { site_id: String, name: String, port: u16 },
    PairVerdict { accept: bool },
}

/// One ordered, reliable message stream between two devices.
pub trait Transport {
    fn send(&mut self, msg: &Msg) -> Result<()>;
    fn recv(&mut self) -> Result<Msg>;
}

impl<T: Transport + ?Sized> Transport for &mut T {
    fn send(&mut self, msg: &Msg) -> Result<()> {
        (**self).send(msg)
    }
    fn recv(&mut self) -> Result<Msg> {
        (**self).recv()
    }
}

pub(crate) fn unexpected<T>(got: &Msg, wanted: &str) -> Result<T> {
    match got {
        Msg::Refuse { reason } => Err(Error::Refused(reason.clone())),
        other => Err(Error::Protocol(format!("expected {wanted}, got {other:?}").chars().take(200).collect())),
    }
}

/// In-memory transport for tests: two ends joined by channels. `cut_after` makes the end fail after that
/// many sent messages, which is how tests cut a session off mid-batch.
pub mod mem {
    use super::*;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::time::Duration;

    pub struct MemTransport {
        tx: Sender<Msg>,
        rx: Receiver<Msg>,
        cut_after: Option<usize>,
        sent: usize,
    }

    pub fn pair(cut_a: Option<usize>, cut_b: Option<usize>) -> (MemTransport, MemTransport) {
        let (atx, brx) = channel();
        let (btx, arx) = channel();
        (
            MemTransport { tx: atx, rx: arx, cut_after: cut_a, sent: 0 },
            MemTransport { tx: btx, rx: brx, cut_after: cut_b, sent: 0 },
        )
    }

    impl Transport for MemTransport {
        fn send(&mut self, msg: &Msg) -> Result<()> {
            if self.cut_after.is_some_and(|n| self.sent >= n) {
                return Err(Error::Closed);
            }
            self.sent += 1;
            self.tx.send(msg.clone()).map_err(|_| Error::Closed)
        }

        fn recv(&mut self) -> Result<Msg> {
            self.rx.recv_timeout(Duration::from_secs(20)).map_err(|_| Error::Closed)
        }
    }
}
