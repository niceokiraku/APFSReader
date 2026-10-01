//! A tiny read-only block service over a stream, and its client.
//!
//! Reading a physical disk needs administrator rights, but a drive letter
//! mounted by an elevated process is not visible to the user's normal Explorer.
//! So the mount stays unelevated and a small elevated helper (the broker) only
//! serves raw reads of one disk over a named pipe. This module is the protocol
//! both ends speak, kept independent of the transport so it can be tested with
//! any `Read + Write`.
//!
//! All integers are little-endian.
//!
//! ```text
//! request : op u8 | a u64 | b u32 | payload
//!   HELLO (1): a = 0,      b = 16, payload = 16-byte token   (must be first)
//!   INFO  (2): a = 0,      b = 0
//!   READ  (3): a = offset, b = length (at most MAX_READ)
//! response: status u8 (0 ok, 1 error) | len u32 | payload
//!   HELLO, error: payload = UTF-8 message / empty
//!   INFO:  size u64 | sector u32
//!   READ:  the bytes
//! ```

use crate::aligned::{AlignedSource, RawRead};
use crate::device::BlockSource;
use crate::{Error, Result};
use std::io::{self, Read, Write};
use std::sync::Mutex;

pub const TOKEN_LEN: usize = 16;
/// Largest single read a client may ask for.
pub const MAX_READ: u32 = 1 << 20;

const OP_HELLO: u8 = 1;
const OP_INFO: u8 = 2;
const OP_READ: u8 = 3;
const OK: u8 = 0;
const ERR: u8 = 1;

fn write_request<W: Write>(w: &mut W, op: u8, a: u64, b: u32, payload: &[u8]) -> io::Result<()> {
    let mut msg = Vec::with_capacity(13 + payload.len());
    msg.push(op);
    msg.extend_from_slice(&a.to_le_bytes());
    msg.extend_from_slice(&b.to_le_bytes());
    msg.extend_from_slice(payload);
    w.write_all(&msg)?;
    w.flush()
}

fn write_response<W: Write>(w: &mut W, status: u8, payload: &[u8]) -> io::Result<()> {
    let mut msg = Vec::with_capacity(5 + payload.len());
    msg.push(status);
    msg.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    msg.extend_from_slice(payload);
    w.write_all(&msg)?;
    w.flush()
}

/// Constant-time comparison, so the token cannot be guessed byte by byte.
fn tokens_match(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Serve `src` to one client until it disconnects. A client that does not open
/// with the right token is dropped without being told anything useful.
pub fn serve_stream<S: Read + Write>(
    stream: &mut S,
    src: &dyn BlockSource,
    token: &[u8; TOKEN_LEN],
) -> io::Result<()> {
    let mut authenticated = false;
    loop {
        let mut head = [0u8; 13];
        match stream.read_exact(&mut head) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        }
        let op = head[0];
        let a = u64::from_le_bytes(head[1..9].try_into().unwrap());
        let b = u32::from_le_bytes(head[9..13].try_into().unwrap());

        if !authenticated {
            if op != OP_HELLO || b as usize != TOKEN_LEN {
                return Ok(());
            }
            let mut given = [0u8; TOKEN_LEN];
            stream.read_exact(&mut given)?;
            if !tokens_match(&given, token) {
                return Ok(());
            }
            authenticated = true;
            write_response(stream, OK, &[])?;
            continue;
        }

        match op {
            OP_INFO => {
                let mut p = Vec::with_capacity(12);
                p.extend_from_slice(&src.len().to_le_bytes());
                p.extend_from_slice(&(src.sector_size() as u32).to_le_bytes());
                write_response(stream, OK, &p)?;
            }
            OP_READ => {
                if b > MAX_READ || a.checked_add(b as u64).map_or(true, |e| e > src.len()) {
                    write_response(stream, ERR, b"read outside the source")?;
                    continue;
                }
                let mut buf = vec![0u8; b as usize];
                match src.read_at(a, &mut buf) {
                    Ok(()) => write_response(stream, OK, &buf)?,
                    Err(e) => write_response(stream, ERR, e.to_string().as_bytes())?,
                }
            }
            _ => return Ok(()), // unknown request: hang up
        }
    }
}

/// The client's end of a connection.
pub struct RemoteRaw<S: Read + Write + Send> {
    stream: Mutex<S>,
}

fn read_response<S: Read>(s: &mut S) -> Result<(u8, Vec<u8>)> {
    let mut head = [0u8; 5];
    s.read_exact(&mut head)?;
    let len = u32::from_le_bytes(head[1..5].try_into().unwrap());
    if len > MAX_READ + 4096 {
        return Err(Error::Format("oversized response from the disk helper"));
    }
    let mut payload = vec![0u8; len as usize];
    s.read_exact(&mut payload)?;
    Ok((head[0], payload))
}

impl<S: Read + Write + Send> RawRead for RemoteRaw<S> {
    fn read_aligned(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let mut stream = self.stream.lock().unwrap();
        let mut done = 0usize;
        while done < buf.len() {
            let n = (buf.len() - done).min(MAX_READ as usize);
            write_request(&mut *stream, OP_READ, offset + done as u64, n as u32, &[])?;
            let (status, payload) = read_response(&mut *stream)?;
            if status != OK || payload.len() != n {
                return Err(Error::Io(io::Error::other(format!(
                    "disk helper refused the read: {}",
                    String::from_utf8_lossy(&payload)
                ))));
            }
            buf[done..done + n].copy_from_slice(&payload);
            done += n;
        }
        Ok(())
    }
}

/// A disk served by a broker, as a block source.
pub type RemoteSource<S> = AlignedSource<RemoteRaw<S>>;

/// Authenticate to a broker over `stream` and describe the disk it serves.
pub fn connect<S: Read + Write + Send>(mut stream: S, token: &[u8; TOKEN_LEN]) -> Result<RemoteSource<S>> {
    write_request(&mut stream, OP_HELLO, 0, TOKEN_LEN as u32, token)?;
    let (status, _) = read_response(&mut stream)?;
    if status != OK {
        return Err(Error::Format("the disk helper rejected the connection"));
    }
    write_request(&mut stream, OP_INFO, 0, 0, &[])?;
    let (status, info) = read_response(&mut stream)?;
    if status != OK || info.len() != 12 {
        return Err(Error::Format("bad reply from the disk helper"));
    }
    let size = u64::from_le_bytes(info[0..8].try_into().unwrap());
    let sector = u32::from_le_bytes(info[8..12].try_into().unwrap());
    AlignedSource::new(RemoteRaw { stream: Mutex::new(stream) }, size, sector as u64)
}

#[cfg(windows)]
pub mod pipe;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::mpsc::{channel, Receiver, Sender};

    /// One end of an in-memory duplex stream.
    struct End {
        tx: Sender<Vec<u8>>,
        rx: Receiver<Vec<u8>>,
        pending: VecDeque<u8>,
    }
    fn duplex() -> (End, End) {
        let (a_tx, b_rx) = channel();
        let (b_tx, a_rx) = channel();
        (
            End { tx: a_tx, rx: a_rx, pending: VecDeque::new() },
            End { tx: b_tx, rx: b_rx, pending: VecDeque::new() },
        )
    }
    impl Read for End {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            while self.pending.is_empty() {
                match self.rx.recv() {
                    Ok(chunk) => self.pending.extend(chunk),
                    Err(_) => return Ok(0), // peer hung up
                }
            }
            let n = buf.len().min(self.pending.len());
            for slot in buf.iter_mut().take(n) {
                *slot = self.pending.pop_front().unwrap();
            }
            Ok(n)
        }
    }
    impl Write for End {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.tx.send(buf.to_vec()).map_err(|_| io::ErrorKind::BrokenPipe)?;
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct Mem(Vec<u8>);
    impl BlockSource for Mem {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
            let end = offset as usize + buf.len();
            if end > self.0.len() {
                return Err(Error::OutOfRange { offset, len: buf.len() });
            }
            buf.copy_from_slice(&self.0[offset as usize..end]);
            Ok(())
        }
        fn sector_size(&self) -> u64 {
            4096
        }
    }

    const TOKEN: [u8; TOKEN_LEN] = [7; TOKEN_LEN];

    fn data() -> Vec<u8> {
        (0..4096 * 700u32).map(|i| (i * 13 % 251) as u8).collect()
    }

    #[test]
    fn a_client_reads_the_served_disk_through_the_protocol() {
        let (mut server_end, client_end) = duplex();
        let d = data();
        let served = d.clone();
        let server = std::thread::spawn(move || serve_stream(&mut server_end, &Mem(served), &TOKEN));
        {
            let remote = connect(client_end, &TOKEN).unwrap();
            assert_eq!(remote.len(), d.len() as u64);
            assert_eq!(remote.sector_size(), 4096);
            for (off, len) in [(0usize, 10usize), (4095, 3), (123_457, 2_000_000), (d.len() - 5, 5)] {
                let mut buf = vec![0u8; len];
                remote.read_at(off as u64, &mut buf).unwrap();
                assert_eq!(&buf[..], &d[off..off + len], "off {off} len {len}");
            }
            assert!(remote.read_at(d.len() as u64 - 2, &mut [0u8; 4]).is_err());
        } // dropping the client hangs up
        server.join().unwrap().unwrap();
    }

    #[test]
    fn a_wrong_token_gets_nothing() {
        let (mut server_end, client_end) = duplex();
        let server = std::thread::spawn(move || serve_stream(&mut server_end, &Mem(data()), &TOKEN));
        let r = connect(client_end, &[9; TOKEN_LEN]);
        assert!(r.is_err());
        server.join().unwrap().unwrap();
    }

    #[test]
    fn requests_before_the_token_are_ignored() {
        let (mut server_end, mut client_end) = duplex();
        let server = std::thread::spawn(move || serve_stream(&mut server_end, &Mem(data()), &TOKEN));
        // A READ without HELLO: the server must hang up, not answer.
        write_request(&mut client_end, OP_READ, 0, 512, &[]).unwrap();
        let mut b = [0u8; 1];
        assert_eq!(client_end.read(&mut b).unwrap(), 0);
        server.join().unwrap().unwrap();
    }

    #[test]
    fn oversized_and_out_of_range_reads_are_refused_without_dropping_the_connection() {
        let (mut server_end, mut client_end) = duplex();
        let server = std::thread::spawn(move || serve_stream(&mut server_end, &Mem(data()), &TOKEN));
        write_request(&mut client_end, OP_HELLO, 0, TOKEN_LEN as u32, &TOKEN).unwrap();
        assert_eq!(read_response(&mut client_end).unwrap().0, OK);
        write_request(&mut client_end, OP_READ, 0, MAX_READ + 1, &[]).unwrap();
        assert_eq!(read_response(&mut client_end).unwrap().0, ERR);
        write_request(&mut client_end, OP_READ, u64::MAX - 10, 100, &[]).unwrap();
        assert_eq!(read_response(&mut client_end).unwrap().0, ERR);
        write_request(&mut client_end, OP_READ, 0, 512, &[]).unwrap(); // still alive
        assert_eq!(read_response(&mut client_end).unwrap().0, OK);
        drop(client_end);
        server.join().unwrap().unwrap();
    }

    #[test]
    fn the_token_comparison_is_exact() {
        assert!(tokens_match(&[1, 2, 3], &[1, 2, 3]));
        assert!(!tokens_match(&[1, 2, 3], &[1, 2, 4]));
        assert!(!tokens_match(&[1, 2, 3], &[1, 2]));
    }
}
