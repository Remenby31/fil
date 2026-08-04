use std::io::{self, Read, Write};

// Frame format: [u8 type][u32 BE payload_len][payload]
const HEADER_LEN: usize = 5;

// Proxy → Daemon
pub const MSG_REGISTER: u8 = 0x01;
pub const MSG_OUTPUT: u8 = 0x02;
pub const MSG_RESIZE: u8 = 0x03;
pub const MSG_DESTROYED: u8 = 0x04;

// Daemon → Proxy
pub const MSG_INPUT: u8 = 0x81;
pub const MSG_REMOTE_RESIZE: u8 = 0x82;
pub const MSG_CLIENT_ATTACHED: u8 = 0x83;
pub const MSG_CLIENT_DETACHED: u8 = 0x84;

#[derive(Debug)]
pub enum ProxyMessage {
    Register {
        session_id: String,
        shell: String,
        cwd: String,
        cols: u32,
        rows: u32,
    },
    Output(Vec<u8>),
    Resize {
        cols: u16,
        rows: u16,
    },
    Destroyed {
        exit_code: u8,
    },
}

#[derive(Debug)]
pub enum DaemonMessage {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    ClientAttached,
    ClientDetached,
}

fn write_frame(w: &mut impl Write, msg_type: u8, payload: &[u8]) -> io::Result<()> {
    let len = payload.len() as u32;
    w.write_all(&[msg_type])?;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(payload)?;
    Ok(())
}

impl ProxyMessage {
    pub fn encode(&self, w: &mut impl Write) -> io::Result<()> {
        match self {
            ProxyMessage::Register {
                session_id,
                shell,
                cwd,
                cols,
                rows,
            } => {
                let json = format!(
                    r#"{{"session_id":"{}","shell":"{}","cwd":"{}","cols":{},"rows":{}}}"#,
                    session_id, shell, cwd, cols, rows
                );
                write_frame(w, MSG_REGISTER, json.as_bytes())
            }
            ProxyMessage::Output(data) => write_frame(w, MSG_OUTPUT, data),
            ProxyMessage::Resize { cols, rows } => {
                let mut buf = [0u8; 4];
                buf[0..2].copy_from_slice(&cols.to_be_bytes());
                buf[2..4].copy_from_slice(&rows.to_be_bytes());
                write_frame(w, MSG_RESIZE, &buf)
            }
            ProxyMessage::Destroyed { exit_code } => write_frame(w, MSG_DESTROYED, &[*exit_code]),
        }
    }
}

impl DaemonMessage {
    pub fn encode(&self, w: &mut impl Write) -> io::Result<()> {
        match self {
            DaemonMessage::Input(data) => write_frame(w, MSG_INPUT, data),
            DaemonMessage::Resize { cols, rows } => {
                let mut buf = [0u8; 4];
                buf[0..2].copy_from_slice(&cols.to_be_bytes());
                buf[2..4].copy_from_slice(&rows.to_be_bytes());
                write_frame(w, MSG_REMOTE_RESIZE, &buf)
            }
            DaemonMessage::ClientAttached => write_frame(w, MSG_CLIENT_ATTACHED, &[]),
            DaemonMessage::ClientDetached => write_frame(w, MSG_CLIENT_DETACHED, &[]),
        }
    }
}

pub struct FrameReader {
    buf: Vec<u8>,
    filled: usize,
}

impl FrameReader {
    pub fn new() -> Self {
        Self {
            buf: vec![0u8; 65536],
            filled: 0,
        }
    }

    pub fn feed(&mut self, data: &[u8]) {
        let needed = self.filled + data.len();
        if needed > self.buf.len() {
            self.buf.resize(needed.next_power_of_two(), 0);
        }
        self.buf[self.filled..self.filled + data.len()].copy_from_slice(data);
        self.filled += data.len();
    }

    pub fn read_from(&mut self, r: &mut impl Read) -> io::Result<usize> {
        if self.filled == self.buf.len() {
            self.buf.resize(self.buf.len() * 2, 0);
        }
        let n = r.read(&mut self.buf[self.filled..])?;
        self.filled += n;
        Ok(n)
    }

    pub fn read_from_fd(&mut self, fd: i32) -> isize {
        if self.filled == self.buf.len() {
            self.buf.resize(self.buf.len() * 2, 0);
        }
        let n = unsafe {
            libc::read(
                fd,
                self.buf[self.filled..].as_mut_ptr() as *mut _,
                self.buf.len() - self.filled,
            )
        };
        if n > 0 {
            self.filled += n as usize;
        }
        n
    }

    pub fn try_parse_frame(&mut self) -> Option<(u8, Vec<u8>)> {
        if self.filled < HEADER_LEN {
            return None;
        }
        let msg_type = self.buf[0];
        let payload_len =
            u32::from_be_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]) as usize;
        let total = HEADER_LEN + payload_len;
        if self.filled < total {
            return None;
        }
        let payload = self.buf[HEADER_LEN..total].to_vec();
        self.buf.copy_within(total..self.filled, 0);
        self.filled -= total;
        Some((msg_type, payload))
    }

    pub fn parse_proxy_message(msg_type: u8, payload: Vec<u8>) -> Option<ProxyMessage> {
        match msg_type {
            MSG_REGISTER => {
                let v: serde_json::Value = serde_json::from_slice(&payload).ok()?;
                Some(ProxyMessage::Register {
                    session_id: v["session_id"].as_str()?.to_string(),
                    shell: v["shell"].as_str()?.to_string(),
                    cwd: v["cwd"].as_str()?.to_string(),
                    cols: v["cols"].as_u64()? as u32,
                    rows: v["rows"].as_u64()? as u32,
                })
            }
            MSG_OUTPUT => Some(ProxyMessage::Output(payload)),
            MSG_RESIZE if payload.len() >= 4 => {
                let cols = u16::from_be_bytes([payload[0], payload[1]]);
                let rows = u16::from_be_bytes([payload[2], payload[3]]);
                Some(ProxyMessage::Resize { cols, rows })
            }
            MSG_DESTROYED => {
                let exit_code = payload.first().copied().unwrap_or(0);
                Some(ProxyMessage::Destroyed { exit_code })
            }
            _ => None,
        }
    }

    pub fn parse_daemon_message(msg_type: u8, payload: Vec<u8>) -> Option<DaemonMessage> {
        match msg_type {
            MSG_INPUT => Some(DaemonMessage::Input(payload)),
            MSG_REMOTE_RESIZE if payload.len() >= 4 => {
                let cols = u16::from_be_bytes([payload[0], payload[1]]);
                let rows = u16::from_be_bytes([payload[2], payload[3]]);
                Some(DaemonMessage::Resize { cols, rows })
            }
            MSG_CLIENT_ATTACHED => Some(DaemonMessage::ClientAttached),
            MSG_CLIENT_DETACHED => Some(DaemonMessage::ClientDetached),
            _ => None,
        }
    }
}

impl Default for FrameReader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_output() {
        let msg = ProxyMessage::Output(b"hello world".to_vec());
        let mut buf = Vec::new();
        msg.encode(&mut buf).unwrap();

        let mut reader = FrameReader::new();
        reader.buf[..buf.len()].copy_from_slice(&buf);
        reader.filled = buf.len();

        let (msg_type, payload) = reader.try_parse_frame().unwrap();
        let parsed = FrameReader::parse_proxy_message(msg_type, payload).unwrap();
        match parsed {
            ProxyMessage::Output(data) => assert_eq!(data, b"hello world"),
            _ => panic!("wrong type"),
        }
    }

    #[test]
    fn roundtrip_register() {
        let msg = ProxyMessage::Register {
            session_id: "abc-123".into(),
            shell: "zsh".into(),
            cwd: "/home/user".into(),
            cols: 120,
            rows: 40,
        };
        let mut buf = Vec::new();
        msg.encode(&mut buf).unwrap();

        let mut reader = FrameReader::new();
        reader.buf[..buf.len()].copy_from_slice(&buf);
        reader.filled = buf.len();

        let (msg_type, payload) = reader.try_parse_frame().unwrap();
        let parsed = FrameReader::parse_proxy_message(msg_type, payload).unwrap();
        match parsed {
            ProxyMessage::Register {
                session_id,
                shell,
                cols,
                rows,
                ..
            } => {
                assert_eq!(session_id, "abc-123");
                assert_eq!(shell, "zsh");
                assert_eq!(cols, 120);
                assert_eq!(rows, 40);
            }
            _ => panic!("wrong type"),
        }
    }

    #[test]
    fn roundtrip_daemon_input() {
        let msg = DaemonMessage::Input(b"ls -la\n".to_vec());
        let mut buf = Vec::new();
        msg.encode(&mut buf).unwrap();

        let mut reader = FrameReader::new();
        reader.buf[..buf.len()].copy_from_slice(&buf);
        reader.filled = buf.len();

        let (msg_type, payload) = reader.try_parse_frame().unwrap();
        let parsed = FrameReader::parse_daemon_message(msg_type, payload).unwrap();
        match parsed {
            DaemonMessage::Input(data) => assert_eq!(data, b"ls -la\n"),
            _ => panic!("wrong type"),
        }
    }

    #[test]
    fn roundtrip_resize() {
        let msg = DaemonMessage::Resize {
            cols: 200,
            rows: 50,
        };
        let mut buf = Vec::new();
        msg.encode(&mut buf).unwrap();

        let mut reader = FrameReader::new();
        reader.buf[..buf.len()].copy_from_slice(&buf);
        reader.filled = buf.len();

        let (msg_type, payload) = reader.try_parse_frame().unwrap();
        let parsed = FrameReader::parse_daemon_message(msg_type, payload).unwrap();
        match parsed {
            DaemonMessage::Resize { cols, rows } => {
                assert_eq!(cols, 200);
                assert_eq!(rows, 50);
            }
            _ => panic!("wrong type"),
        }
    }

    #[test]
    fn partial_frames() {
        let msg = ProxyMessage::Output(b"data".to_vec());
        let mut buf = Vec::new();
        msg.encode(&mut buf).unwrap();

        let mut reader = FrameReader::new();

        // Feed partial header
        reader.buf[..3].copy_from_slice(&buf[..3]);
        reader.filled = 3;
        assert!(reader.try_parse_frame().is_none());

        // Feed rest
        reader.buf[3..buf.len()].copy_from_slice(&buf[3..]);
        reader.filled = buf.len();
        assert!(reader.try_parse_frame().is_some());
    }

    #[test]
    fn multiple_frames_in_buffer() {
        let msg1 = ProxyMessage::Output(b"first".to_vec());
        let msg2 = ProxyMessage::Output(b"second".to_vec());
        let mut buf = Vec::new();
        msg1.encode(&mut buf).unwrap();
        msg2.encode(&mut buf).unwrap();

        let mut reader = FrameReader::new();
        reader.buf[..buf.len()].copy_from_slice(&buf);
        reader.filled = buf.len();

        let (t1, p1) = reader.try_parse_frame().unwrap();
        assert_eq!(t1, MSG_OUTPUT);
        assert_eq!(p1, b"first");

        let (t2, p2) = reader.try_parse_frame().unwrap();
        assert_eq!(t2, MSG_OUTPUT);
        assert_eq!(p2, b"second");

        assert!(reader.try_parse_frame().is_none());
    }
}
