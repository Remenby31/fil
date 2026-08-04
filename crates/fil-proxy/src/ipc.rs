use crate::config::DaemonConfig;
use fil_protocol::ipc::{DaemonMessage, FrameReader, ProxyMessage};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::time::Duration;

const MAX_PENDING_BYTES: usize = 256 * 1024;
const REGISTER_WRITE_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
pub struct ProxyRegistration {
    session_id: String,
    shell: String,
    cwd: String,
}

impl ProxyRegistration {
    pub fn new(session_id: String, shell: String, cwd: String) -> Self {
        Self {
            session_id,
            shell,
            cwd,
        }
    }
}

/// Non-blocking IPC connection to fil-daemon.
///
/// The PTY is the critical path. Daemon output is therefore buffered only up to
/// a small, fixed limit; a stalled daemon disconnects instead of freezing the
/// local terminal.
pub struct DaemonConnection {
    stream: UnixStream,
    reader: FrameReader,
    pending: Vec<u8>,
    written: usize,
}

impl DaemonConnection {
    fn new(stream: UnixStream) -> io::Result<Self> {
        stream.set_write_timeout(None)?;
        stream.set_nonblocking(true)?;

        #[cfg(target_os = "macos")]
        unsafe {
            let enabled: libc::c_int = 1;
            libc::setsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_NOSIGPIPE,
                &enabled as *const _ as *const libc::c_void,
                std::mem::size_of_val(&enabled) as libc::socklen_t,
            );
        }

        Ok(Self {
            stream,
            reader: FrameReader::new(),
            pending: Vec::with_capacity(16 * 1024),
            written: 0,
        })
    }

    pub fn fd(&self) -> RawFd {
        self.stream.as_raw_fd()
    }

    pub fn wants_write(&self) -> bool {
        self.written < self.pending.len()
    }

    pub fn queue(&mut self, message: &ProxyMessage) -> io::Result<()> {
        let mut frame = Vec::with_capacity(16 * 1024 + 5);
        message.encode(&mut frame)?;

        self.compact_pending();
        if self.pending.len() + frame.len() > MAX_PENDING_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "fil-daemon IPC backlog exceeded",
            ));
        }

        self.pending.extend_from_slice(&frame);
        self.flush()
    }

    pub fn flush(&mut self) -> io::Result<()> {
        while self.written < self.pending.len() {
            match self.stream.write(&self.pending[self.written..]) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "fil-daemon IPC socket closed",
                    ));
                }
                Ok(n) => self.written += n,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) => return Err(error),
            }
        }

        self.pending.clear();
        self.written = 0;
        Ok(())
    }

    pub fn read_available(&mut self) -> io::Result<Vec<DaemonMessage>> {
        let mut messages = Vec::new();
        let mut buf = [0u8; 16 * 1024];

        loop {
            match self.stream.read(&mut buf) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "fil-daemon IPC socket closed",
                    ));
                }
                Ok(n) => {
                    self.reader.feed(&buf[..n]);
                    while let Some((msg_type, payload)) = self.reader.try_parse_frame() {
                        if let Some(message) = FrameReader::parse_daemon_message(msg_type, payload)
                        {
                            messages.push(message);
                        }
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
        }

        Ok(messages)
    }

    fn compact_pending(&mut self) {
        if self.written == 0 {
            return;
        }
        if self.written == self.pending.len() {
            self.pending.clear();
        } else {
            self.pending.copy_within(self.written.., 0);
            self.pending.truncate(self.pending.len() - self.written);
        }
        self.written = 0;
    }
}

pub fn connect_and_register(
    registration: &ProxyRegistration,
    cols: u32,
    rows: u32,
) -> Option<DaemonConnection> {
    let sock_path = DaemonConfig::config_dir().join("daemon.sock");
    let mut stream = UnixStream::connect(&sock_path).ok()?;
    stream
        .set_write_timeout(Some(REGISTER_WRITE_TIMEOUT))
        .ok()?;

    let msg = ProxyMessage::Register {
        session_id: registration.session_id.clone(),
        shell: registration.shell.clone(),
        cwd: registration.cwd.clone(),
        cols,
        rows,
    };

    let mut buf = Vec::with_capacity(256);
    msg.encode(&mut buf).ok()?;
    stream.write_all(&buf).ok()?;

    DaemonConnection::new(stream).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn stalled_daemon_hits_a_bounded_backlog_without_blocking() {
        let (client, _server) = UnixStream::pair().unwrap();

        unsafe {
            let send_buffer: libc::c_int = 4 * 1024;
            assert_eq!(
                libc::setsockopt(
                    client.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    &send_buffer as *const _ as *const libc::c_void,
                    std::mem::size_of_val(&send_buffer) as libc::socklen_t,
                ),
                0
            );
        }

        let mut connection = DaemonConnection::new(client).unwrap();
        let message = ProxyMessage::Output(vec![b'x'; 16 * 1024]);
        let started = Instant::now();

        let error = (0..100)
            .find_map(|_| connection.queue(&message).err())
            .expect("stalled daemon should exceed the bounded backlog");

        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
