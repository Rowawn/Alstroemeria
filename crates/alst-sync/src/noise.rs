//! Noise over a byte stream (SPEC §9.3): `Noise_XX_25519_ChaChaPoly_BLAKE2s`, and `XXpsk3` for QR pairing.
//! Frames are a u16 length plus one Noise message. Application messages are a type byte, a u32 length and
//! the payload (JSON for control, raw bytes for file chunks), split across frames as needed.

use crate::proto::{Msg, Transport};
use crate::{Error, Result};
use sha2::{Digest, Sha256};
use snow::{Builder, HandshakeState, TransportState};
use std::io::{Read, Write};

const XX: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
const XXPSK3: &str = "Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s";
/// Plaintext bytes per Noise message (65,535 minus the 16-byte tag).
const MAX_PLAIN: usize = 65_519;
/// Larger application messages are refused (a batch of big parts is a few MB).
const MAX_MESSAGE: usize = 256 * 1024 * 1024;

const T_JSON: u8 = 0;
const T_CHUNK: u8 = 1;

fn noise<E: std::fmt::Display>(e: E) -> Error {
    Error::Protocol(format!("noise: {e}"))
}

#[derive(Clone)]
pub struct KeyPair {
    pub private: Vec<u8>,
    pub public: Vec<u8>,
}

pub fn generate_keypair() -> Result<KeyPair> {
    let kp = Builder::new(XX.parse().map_err(noise)?).generate_keypair().map_err(noise)?;
    Ok(KeyPair { private: kp.private, public: kp.public })
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

/// Short form of a public key shown to people and advertised on mDNS (`fp`): the first 8 bytes in hex.
pub fn fingerprint(public: &[u8]) -> String {
    hex(&public[..public.len().min(8)])
}

/// What the handshake established.
pub struct Handshake {
    pub transport: NoiseTransport,
    pub remote_static: Vec<u8>,
    /// The 6-digit code both sides show when pairing by code (derived from the handshake hash).
    pub code: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pattern {
    Xx,
    /// QR pairing: `XXpsk3` with the secret from the QR code.
    XxPsk3,
}

fn build(pattern: Pattern, kp: &KeyPair, psk: Option<&[u8]>, initiator: bool) -> Result<HandshakeState> {
    let name = if pattern == Pattern::Xx { XX } else { XXPSK3 };
    let mut b = Builder::new(name.parse().map_err(noise)?).local_private_key(&kp.private).map_err(noise)?;
    let key: [u8; 32];
    if pattern == Pattern::XxPsk3 {
        let secret = psk.ok_or_else(|| Error::Protocol("psk required".into()))?;
        key = Sha256::digest(secret).into();
        b = b.psk(3, &key).map_err(noise)?;
    }
    if initiator { b.build_initiator() } else { b.build_responder() }.map_err(noise)
}

fn write_frame<W: Write>(w: &mut W, data: &[u8]) -> Result<()> {
    w.write_all(&(data.len() as u16).to_be_bytes())?;
    w.write_all(data)?;
    Ok(())
}

fn read_frame<R: Read>(r: &mut R) -> Result<Vec<u8>> {
    let mut len = [0u8; 2];
    r.read_exact(&mut len).map_err(eof)?;
    let mut buf = vec![0u8; u16::from_be_bytes(len) as usize];
    r.read_exact(&mut buf).map_err(eof)?;
    Ok(buf)
}

fn eof(e: std::io::Error) -> Error {
    e.into()
}

fn pairing_code(hash: &[u8]) -> String {
    let d = Sha256::new().chain_update(b"alstroemeria pairing code").chain_update(hash).finalize();
    let n = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % 1_000_000;
    format!("{n:06}")
}

fn finish<S: Read + Write + Send + 'static>(hs: HandshakeState, stream: S) -> Result<Handshake> {
    let remote = hs.get_remote_static().ok_or_else(|| Error::Protocol("no remote key".into()))?.to_vec();
    let code = pairing_code(hs.get_handshake_hash());
    let transport = hs.into_transport_mode().map_err(noise)?;
    Ok(Handshake { transport: NoiseTransport::new(stream, transport), remote_static: remote, code })
}

/// Run the initiator side (three messages: `-> e`, `<- e ee s es`, `-> s se`).
pub fn handshake_initiator<S: Read + Write + Send + 'static>(mut stream: S, kp: &KeyPair, pattern: Pattern, psk: Option<&[u8]>) -> Result<Handshake> {
    let mut hs = build(pattern, kp, psk, true)?;
    let mut buf = vec![0u8; 65_535];
    let n = hs.write_message(&[], &mut buf).map_err(noise)?;
    write_frame(&mut stream, &buf[..n])?;
    let m = read_frame(&mut stream)?;
    hs.read_message(&m, &mut buf).map_err(noise)?;
    let n = hs.write_message(&[], &mut buf).map_err(noise)?;
    write_frame(&mut stream, &buf[..n])?;
    finish(hs, stream)
}

pub fn handshake_responder<S: Read + Write + Send + 'static>(mut stream: S, kp: &KeyPair, pattern: Pattern, psk: Option<&[u8]>) -> Result<Handshake> {
    let mut hs = build(pattern, kp, psk, false)?;
    let mut buf = vec![0u8; 65_535];
    let m = read_frame(&mut stream)?;
    hs.read_message(&m, &mut buf).map_err(noise)?;
    let n = hs.write_message(&[], &mut buf).map_err(noise)?;
    write_frame(&mut stream, &buf[..n])?;
    let m = read_frame(&mut stream)?;
    hs.read_message(&m, &mut buf).map_err(noise)?;
    finish(hs, stream)
}

/// An encrypted message stream over any `Read + Write` (a `TcpStream`, or a pipe in tests).
pub struct NoiseTransport {
    stream: Box<dyn ReadWrite>,
    state: TransportState,
    /// Decrypted bytes not yet consumed.
    plain: Vec<u8>,
}

trait ReadWrite: Read + Write + Send {}
impl<T: Read + Write + Send> ReadWrite for T {}

impl NoiseTransport {
    fn new<S: Read + Write + Send + 'static>(stream: S, state: TransportState) -> Self {
        NoiseTransport { stream: Box::new(stream), state, plain: Vec::new() }
    }

    fn write_plain(&mut self, mut data: &[u8]) -> Result<()> {
        let mut out = vec![0u8; MAX_PLAIN + 16];
        while !data.is_empty() {
            let take = data.len().min(MAX_PLAIN);
            let n = self.state.write_message(&data[..take], &mut out).map_err(noise)?;
            write_frame(&mut self.stream, &out[..n])?;
            data = &data[take..];
        }
        self.stream.flush()?;
        Ok(())
    }

    /// Make at least `n` plaintext bytes available.
    fn fill(&mut self, n: usize) -> Result<()> {
        let mut out = vec![0u8; 65_535];
        while self.plain.len() < n {
            let frame = read_frame(&mut self.stream)?;
            let len = self.state.read_message(&frame, &mut out).map_err(noise)?;
            self.plain.extend_from_slice(&out[..len]);
        }
        Ok(())
    }

    fn take(&mut self, n: usize) -> Result<Vec<u8>> {
        self.fill(n)?;
        Ok(self.plain.drain(..n).collect())
    }
}

impl Transport for NoiseTransport {
    fn send(&mut self, msg: &Msg) -> Result<()> {
        let (ty, payload) = match msg {
            Msg::Chunk { hash, offset, data } => {
                let mut p = Vec::with_capacity(72 + data.len());
                p.extend_from_slice(hash.as_bytes());
                p.extend_from_slice(&offset.to_be_bytes());
                p.extend_from_slice(data);
                (T_CHUNK, p)
            }
            other => (T_JSON, serde_json::to_vec(other).map_err(|e| Error::Protocol(e.to_string()))?),
        };
        if payload.len() > MAX_MESSAGE {
            return Err(Error::Protocol("message too large".into()));
        }
        let mut head = [0u8; 5];
        head[0] = ty;
        head[1..].copy_from_slice(&(payload.len() as u32).to_be_bytes());
        self.write_plain(&[&head[..], &payload].concat())
    }

    fn recv(&mut self) -> Result<Msg> {
        let head = self.take(5)?;
        let len = u32::from_be_bytes([head[1], head[2], head[3], head[4]]) as usize;
        if len > MAX_MESSAGE {
            return Err(Error::Protocol("message too large".into()));
        }
        let payload = self.take(len)?;
        match head[0] {
            T_JSON => serde_json::from_slice(&payload).map_err(|e| Error::Protocol(e.to_string())),
            T_CHUNK if payload.len() >= 72 => {
                let hash = String::from_utf8(payload[..64].to_vec()).map_err(|_| Error::Protocol("bad chunk".into()))?;
                let offset = u64::from_be_bytes(payload[64..72].try_into().unwrap());
                Ok(Msg::Chunk { hash, offset, data: payload[72..].to_vec() })
            }
            t => Err(Error::Protocol(format!("unknown message type {t}"))),
        }
    }
}
